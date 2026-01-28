use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;

use crate::cli::{LlmEngine, LlmRewritePagesArgs};
use crate::formats::{ExtractedFrontMatter, ManifestRecord, Toc};
use crate::openai;

pub async fn rewrite_pages(args: LlmRewritePagesArgs) -> anyhow::Result<()> {
    if args.prompt.trim().is_empty() {
        anyhow::bail!("--prompt must be non-empty");
    }

    let out_dir = PathBuf::from(&args.out);
    if out_dir.exists() {
        if args.force {
            std::fs::remove_dir_all(&out_dir)
                .with_context(|| format!("remove existing out dir: {}", out_dir.display()))?;
        } else {
            anyhow::bail!("output already exists: {}", out_dir.display());
        }
    }

    let pages_dir = out_dir.join("pages");
    std::fs::create_dir_all(&pages_dir)
        .with_context(|| format!("create out pages dir: {}", pages_dir.display()))?;

    let toc = read_toc(&args.toc).context("read toc")?;
    let page_ids = toc_page_ids_in_order(&toc).context("collect toc page ids")?;
    if page_ids.is_empty() {
        anyhow::bail!("toc contains no sources: {}", args.toc);
    }

    let manifest = read_manifest_map(&args.manifest).context("read manifest")?;
    let mut jobs = Vec::new();
    for page_id in page_ids {
        let record = manifest
            .get(&page_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("page id not found in manifest: {page_id}"))?;
        jobs.push(PageJob { record });
    }

    let shared = Arc::new(RewriteShared::new(&args, pages_dir).await?);
    let concurrency = args.openai_concurrency.max(1).min(jobs.len().max(1));

    tracing::info!(
        engine = ?args.engine,
        pages = jobs.len(),
        concurrency = concurrency,
        "llm rewrite-pages: start"
    );

    let started_at = std::time::Instant::now();
    let mut join_set = tokio::task::JoinSet::new();
    let mut next_idx = 0usize;
    let mut done = 0usize;
    let mut failed = 0usize;
    let mut last_progress_log_at = started_at;

    while next_idx < jobs.len() || !join_set.is_empty() {
        while next_idx < jobs.len() && join_set.len() < concurrency {
            let job = jobs[next_idx].clone();
            let shared = Arc::clone(&shared);
            join_set.spawn(async move { rewrite_one_page(shared.as_ref(), job).await });
            next_idx += 1;
        }

        let Some(joined) = join_set.join_next().await else {
            break;
        };

        match joined {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                failed += 1;
                tracing::warn!(error = %format!("{err:#}"), "llm rewrite-pages: page failed");
            }
            Err(err) => {
                failed += 1;
                tracing::warn!(error = %format!("{err:#}"), "llm rewrite-pages: task failed");
            }
        }

        done += 1;
        if done == jobs.len() || last_progress_log_at.elapsed() >= Duration::from_secs(2) {
            tracing::info!(
                done = done,
                total = jobs.len(),
                failed = failed,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                "llm rewrite-pages: progress"
            );
            last_progress_log_at = std::time::Instant::now();
        }
    }

    if failed > 0 {
        anyhow::bail!("llm rewrite-pages completed with failures (failed={failed})");
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct PageJob {
    record: ManifestRecord,
}

struct RewriteShared {
    engine: LlmEngine,
    prompt: String,
    pages_dir: PathBuf,
    command: Option<String>,
    command_args: Vec<String>,
    openai: Option<OpenaiRewriteConfig>,
    allow_missing_tokens: bool,
}

impl RewriteShared {
    async fn new(args: &LlmRewritePagesArgs, pages_dir: PathBuf) -> anyhow::Result<Self> {
        let openai = match args.engine {
            LlmEngine::Openai => {
                let api_key = std::env::var("OPENAI_API_KEY")
                    .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY is not set"))?;

                if args.openai_max_chars == 0 {
                    anyhow::bail!("--openai-max-chars must be > 0");
                }

                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(300))
                    .build()
                    .context("build http client")?;

                Some(OpenaiRewriteConfig {
                    client,
                    endpoint: openai::responses_endpoint(&args.openai_base_url),
                    api_key,
                    model: args.openai_model.clone(),
                    temperature: args.openai_temperature,
                    max_chars: args.openai_max_chars,
                    retries: args.openai_retries,
                })
            }
            _ => None,
        };

        Ok(Self {
            engine: args.engine,
            prompt: args.prompt.clone(),
            pages_dir,
            command: args.command.clone(),
            command_args: args.command_args.clone(),
            openai,
            allow_missing_tokens: args.allow_missing_tokens,
        })
    }
}

#[derive(Clone)]
struct OpenaiRewriteConfig {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    temperature: f32,
    max_chars: usize,
    retries: usize,
}

async fn rewrite_one_page(shared: &RewriteShared, job: PageJob) -> anyhow::Result<()> {
    let extracted_path = PathBuf::from(&job.record.extracted_md);
    let extracted = std::fs::read_to_string(&extracted_path)
        .with_context(|| format!("read extracted page: {}", extracted_path.display()))?;

    let (mut front, body) = split_front_matter(&extracted)
        .with_context(|| format!("parse front matter: {}", job.record.id))?;

    if front.id != job.record.id {
        anyhow::bail!(
            "page id mismatch (manifest.id={} front_matter.id={})",
            job.record.id,
            front.id
        );
    }

    let out_path = shared.pages_dir.join(format!("{}.md", job.record.id));

    if matches!(shared.engine, LlmEngine::Noop) {
        write_output_file(&out_path, &extracted, false)?;
        return Ok(());
    }

    let rewritten = rewrite_body(shared, &job.record, &body)
        .await
        .with_context(|| format!("rewrite body: {}", job.record.id))?;

    let (maybe_title, body_without_h1) = strip_leading_h1(&rewritten);
    if let Some(title) = maybe_title.filter(|t| !t.trim().is_empty()) {
        front.title = title;
    }

    let page_md = assemble_extracted_page(&front, &body_without_h1);
    write_output_file(&out_path, &page_md, false)?;
    Ok(())
}

async fn rewrite_body(
    shared: &RewriteShared,
    record: &ManifestRecord,
    body: &str,
) -> anyhow::Result<String> {
    let mut store = TokenStore::new();
    let sections = split_markdown_by_h2(body);
    let total_sections = sections.len().max(1);

    let mut rewritten_protected = String::new();
    for (idx, section) in sections.into_iter().enumerate() {
        if idx != 0 && !rewritten_protected.ends_with('\n') {
            rewritten_protected.push('\n');
        }
        if idx != 0 {
            rewritten_protected.push('\n');
        }

        let protected = protect_markdown(&section, &mut store);
        let expected_tokens = extract_placeholder_tokens(&protected);
        let rewritten = match shared.engine {
            LlmEngine::Command => {
                rewrite_protected_via_command(shared, record, idx + 1, total_sections, &protected)?
            }
            LlmEngine::Openai => {
                rewrite_protected_via_openai(
                    shared
                        .openai
                        .as_ref()
                        .expect("openai config is present when engine=openai"),
                    &shared.prompt,
                    record,
                    idx + 1,
                    total_sections,
                    &protected,
                )
                .await?
            }
            LlmEngine::Noop => protected.clone(),
        };

        let rewritten = normalize_placeholder_tokens(&rewritten);
        if rewritten.trim().is_empty() {
            tracing::warn!(
                page_id = %record.id,
                section = idx + 1,
                "rewrite output is empty; keeping original section"
            );
            rewritten_protected.push_str(protected.trim_end());
        } else {
            let missing = missing_tokens(&rewritten, &expected_tokens);
            if !missing.is_empty() {
                let sample = missing
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                if shared.allow_missing_tokens {
                    tracing::warn!(
                        page_id = %record.id,
                        section = idx + 1,
                        missing_tokens = missing.len(),
                        missing_sample = %sample,
                        "rewrite output is missing placeholder tokens; keeping rewritten output"
                    );
                    rewritten_protected.push_str(rewritten.trim_end());
                } else {
                    tracing::warn!(
                        page_id = %record.id,
                        section = idx + 1,
                        missing_tokens = missing.len(),
                        missing_sample = %sample,
                        "rewrite output is missing placeholder tokens; keeping original section"
                    );
                    rewritten_protected.push_str(protected.trim_end());
                }
            } else {
                rewritten_protected.push_str(rewritten.trim_end());
            }
        }
        rewritten_protected.push('\n');
    }

    Ok(unprotect_markdown_fully(
        rewritten_protected.trim_end(),
        &store.tokens,
    ))
}

fn rewrite_protected_via_command(
    shared: &RewriteShared,
    record: &ManifestRecord,
    section_index: usize,
    section_total: usize,
    input_protected: &str,
) -> anyhow::Result<String> {
    let Some(program) = shared.command.as_deref() else {
        anyhow::bail!("missing --command (required when --engine=command)");
    };

    tracing::info!(
        engine = "command",
        command = program,
        page_id = %record.id,
        section = section_index,
        out_dir = %shared.pages_dir.display(),
        "llm rewrite-pages"
    );

    let mut child = Command::new(program)
        .args(&shared.command_args)
        .env("SITEBOOKIFY_REWRITE_PROMPT", &shared.prompt)
        .env("SITEBOOKIFY_REWRITE_PAGE_ID", &record.id)
        .env("SITEBOOKIFY_REWRITE_PAGE_URL", &record.url)
        .env("SITEBOOKIFY_REWRITE_PAGE_TITLE", &record.title)
        .env(
            "SITEBOOKIFY_REWRITE_SECTION_INDEX",
            section_index.to_string(),
        )
        .env(
            "SITEBOOKIFY_REWRITE_SECTION_TOTAL",
            section_total.to_string(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn rewrite command: {program}"))?;

    {
        let mut stdin = child.stdin.take().context("open rewrite stdin")?;
        stdin
            .write_all(input_protected.as_bytes())
            .context("write rewrite stdin")?;
    }

    let output = child.wait_with_output().context("wait rewrite process")?;
    if !output.status.success() {
        anyhow::bail!("rewrite command failed: {program} ({})", output.status);
    }

    String::from_utf8(output.stdout).context("rewrite stdout is not valid UTF-8")
}

async fn rewrite_protected_via_openai(
    config: &OpenaiRewriteConfig,
    prompt: &str,
    record: &ManifestRecord,
    section_index: usize,
    section_total: usize,
    input_protected: &str,
) -> anyhow::Result<String> {
    let chunks = if input_protected.len() <= config.max_chars {
        vec![input_protected.to_owned()]
    } else {
        chunk_by_lines(input_protected, config.max_chars).context("chunk section input")?
    };

    let mut out = String::new();
    for (chunk_idx, chunk) in chunks.into_iter().enumerate() {
        if chunk_idx != 0 && !out.ends_with('\n') {
            out.push('\n');
        }

        let mut last_err: Option<anyhow::Error> = None;
        let attempts = config.retries.saturating_add(1);
        for attempt in 0..attempts {
            let instructions = build_openai_rewrite_instructions(
                prompt,
                record,
                section_index,
                section_total,
                chunk_idx + 1,
                attempts,
            );

            tracing::debug!(
                engine = "openai",
                model = %config.model,
                page_id = %record.id,
                section = section_index,
                attempt = attempt + 1,
                attempts = attempts,
                "rewrite chunk"
            );

            let raw = match openai::responses_text(
                &config.client,
                &config.endpoint,
                &config.api_key,
                &config.model,
                &instructions,
                &chunk,
                config.temperature,
            )
            .await
            {
                Ok(text) => text,
                Err(err) => {
                    last_err = Some(err);
                    continue;
                }
            };

            if raw.trim().is_empty() {
                last_err = Some(anyhow::anyhow!("OpenAI output is empty"));
                continue;
            }

            out.push_str(raw.trim_end());
            out.push('\n');
            last_err = None;
            break;
        }

        if let Some(err) = last_err {
            tracing::warn!(
                engine = "openai",
                page_id = %record.id,
                section = section_index,
                error = %format!("{err:#}"),
                "rewrite chunk failed; keeping original chunk"
            );
            out.push_str(chunk.trim_end());
            out.push('\n');
        }
    }

    Ok(out)
}

fn build_openai_rewrite_instructions(
    prompt: &str,
    record: &ManifestRecord,
    section_index: usize,
    section_total: usize,
    chunk_index: usize,
    chunk_total: usize,
) -> String {
    format!(
        "You are a book editor and technical writer.\n\
Task: Rewrite the input Markdown into book-first prose.\n\
\n\
Context:\n\
- Page title: {title}\n\
- Page URL: {url}\n\
- Section: {section_index}/{section_total}\n\
- Chunk: {chunk_index}/{chunk_total}\n\
\n\
User prompt:\n\
{prompt}\n\
\n\
Hard rules:\n\
- Use ONLY the facts present in the input Markdown. Do not add new facts.\n\
- If something is unclear, write \"不明\".\n\
- Write in a book-first style: mostly paragraphs with smooth transitions.\n\
- Reduce headings and lists. Avoid Markdown headings (`#`, `##`, `###`) and bullet/numbered lists unless truly necessary.\n\
  - If the input starts with a heading, you MAY drop it and instead weave the idea into the first paragraph.\n\
  - If a list is unavoidable, keep it short and write full-sentence items.\n\
- Keep tables/figures/code minimal.\n\
- Do NOT change code blocks, inline code, URLs, or HTML tags.\n\
- You MUST preserve placeholder tokens of the form {{SBY_TOKEN_000000}} exactly as they appear in the input (do not remove or alter them).\n\
- Do NOT mention chunk/section numbers or this instruction text.\n\
- Do NOT add a Sources section (the tool will add it elsewhere).\n\
\n\
Output:\n\
- Output ONLY the rewritten Markdown for this input.\n",
        title = record.title,
        url = record.url,
        section_index = section_index,
        section_total = section_total,
        chunk_index = chunk_index,
        chunk_total = chunk_total,
        prompt = prompt,
    )
}

fn read_toc(path: &str) -> anyhow::Result<Toc> {
    let toc_path = PathBuf::from(path);
    let toc_yaml = std::fs::read_to_string(&toc_path)
        .with_context(|| format!("read toc: {}", toc_path.display()))?;
    serde_yaml::from_str(&toc_yaml).context("parse toc yaml")
}

fn toc_page_ids_in_order(toc: &Toc) -> anyhow::Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for part in &toc.parts {
        for chapter in &part.chapters {
            for source_id in &chapter.sources {
                if !seen.insert(source_id.clone()) {
                    anyhow::bail!("duplicate page id in toc: {source_id}");
                }
                ids.push(source_id.clone());
            }
        }
    }
    Ok(ids)
}

fn read_manifest_map(path: &str) -> anyhow::Result<HashMap<String, ManifestRecord>> {
    let manifest_path = PathBuf::from(path);
    let file = OpenOptions::new()
        .read(true)
        .open(&manifest_path)
        .with_context(|| format!("open manifest: {}", manifest_path.display()))?;
    let reader = BufReader::new(file);

    let mut map = HashMap::new();
    for line in reader.lines() {
        let line = line.context("read manifest jsonl line")?;
        if line.trim().is_empty() {
            continue;
        }
        let record: ManifestRecord =
            serde_json::from_str(&line).context("parse manifest record")?;
        map.insert(record.id.clone(), record);
    }
    Ok(map)
}

fn split_front_matter(contents: &str) -> anyhow::Result<(ExtractedFrontMatter, String)> {
    let mut lines = contents.lines();
    let first = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("page is empty"))?;
    if first.trim_end() != "---" {
        anyhow::bail!("page must start with YAML front matter ('---')");
    }

    let mut yaml_lines = Vec::new();
    let mut reached_end = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            reached_end = true;
            break;
        }
        yaml_lines.push(line);
    }
    if !reached_end {
        anyhow::bail!("front matter is not closed ('---')");
    }

    let yaml = yaml_lines.join("\n");
    let front: ExtractedFrontMatter =
        serde_yaml::from_str(&yaml).context("deserialize extracted front matter")?;

    let body = lines.collect::<Vec<_>>().join("\n");
    Ok((front, body))
}

fn assemble_extracted_page(front: &ExtractedFrontMatter, body: &str) -> String {
    let yaml = serde_yaml::to_string(front).expect("serialize extracted front matter");
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(yaml.trim_end());
    out.push('\n');
    out.push_str("---\n\n");
    out.push_str(body.trim_end());
    out.push('\n');
    out
}

fn write_output_file(path: &Path, contents: &str, force: bool) -> anyhow::Result<()> {
    if path.exists() && !force {
        anyhow::bail!("output already exists: {}", path.display());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create output dir: {}", parent.display()))?;
    }

    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("open output: {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("write output: {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flush output: {}", path.display()))?;
    Ok(())
}

fn strip_leading_h1(input: &str) -> (Option<String>, String) {
    let mut iter = input.split_inclusive('\n').peekable();
    while let Some(line) = iter.peek() {
        if line.trim().is_empty() {
            iter.next();
            continue;
        }
        break;
    }

    let Some(line) = iter.next() else {
        return (None, String::new());
    };

    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("# ") else {
        return (None, input.to_owned());
    };

    let title = rest.trim().trim_end_matches('\n').to_owned();
    let mut remaining = iter.collect::<String>();
    remaining = remaining.trim_start_matches('\n').to_owned();
    (Some(title), remaining)
}

fn split_markdown_by_h2(input: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let mut current = String::new();
    let mut in_fence = false;
    let mut fence_marker = String::new();

    for line in input.split_inclusive('\n') {
        if !in_fence {
            if let Some(marker) = fence_start_marker(line) {
                in_fence = true;
                fence_marker.clear();
                fence_marker.push_str(marker);
                current.push_str(line);
                continue;
            }

            if is_h2_heading(line) && !current.is_empty() {
                sections.push(std::mem::take(&mut current));
            }

            current.push_str(line);
            continue;
        }

        current.push_str(line);
        if fence_end_marker(line, &fence_marker) {
            in_fence = false;
        }
    }

    if !current.is_empty() {
        sections.push(current);
    }

    if sections.is_empty() {
        vec![input.to_owned()]
    } else {
        sections
    }
}

fn is_h2_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("##") else {
        return false;
    };
    rest.chars().next().is_some_and(|c| c.is_whitespace())
}

fn fence_start_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        let len = trimmed.chars().take_while(|c| *c == '`').count();
        return Some(&trimmed[..len]);
    }
    if trimmed.starts_with("~~~") {
        let len = trimmed.chars().take_while(|c| *c == '~').count();
        return Some(&trimmed[..len]);
    }
    None
}

fn fence_end_marker(line: &str, marker: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with(marker)
}

fn protect_markdown(input: &str, store: &mut TokenStore) -> String {
    let text = protect_fenced_code_blocks(input, store);
    let text = protect_inline_code_spans(&text, store);
    let text = protect_markdown_link_destinations(&text, store);
    protect_autolinks_and_bare_urls(&text, store)
}

fn protect_fenced_code_blocks(input: &str, store: &mut TokenStore) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_fence = false;
    let mut fence_marker = String::new();
    let mut block = String::new();

    for piece in input.split_inclusive('\n') {
        if !in_fence {
            if let Some(marker) = fence_start_marker(piece) {
                in_fence = true;
                fence_marker.clear();
                fence_marker.push_str(marker);
                block.clear();
                block.push_str(piece);
                continue;
            }
            out.push_str(piece);
            continue;
        }

        block.push_str(piece);
        if fence_end_marker(piece, &fence_marker) {
            in_fence = false;
            let mut original = std::mem::take(&mut block);
            let trailing_newline = original.ends_with('\n');
            if trailing_newline {
                original.pop();
            }
            let token = store.insert(original);
            out.push_str(&token);
            if trailing_newline {
                out.push('\n');
            }
        }
    }

    if in_fence {
        out.push_str(&block);
    }

    out
}

fn protect_inline_code_spans(input: &str, store: &mut TokenStore) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while let Some(rel) = input[cursor..].find('`') {
        let start = cursor + rel;
        out.push_str(&input[cursor..start]);

        let bytes = input.as_bytes();
        let mut run_len = 0usize;
        while start + run_len < bytes.len() && bytes[start + run_len] == b'`' {
            run_len += 1;
        }

        let delimiter = "`".repeat(run_len);
        let after = start + run_len;
        let Some(close_rel) = input[after..].find(&delimiter) else {
            out.push('`');
            cursor = start + 1;
            continue;
        };

        let close_start = after + close_rel;
        let close_end = close_start + run_len;
        let original = input[start..close_end].to_owned();
        let token = store.insert(original);
        out.push_str(&token);
        cursor = close_end;
    }

    out.push_str(&input[cursor..]);
    out
}

fn protect_markdown_link_destinations(input: &str, store: &mut TokenStore) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut cursor = 0usize;

    while let Some(rel) = input[cursor..].find("](") {
        let start = cursor + rel;
        out.push_str(&input[cursor..start + 2]);

        let mut i = start + 2;
        let mut depth = 1usize;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }

        if depth != 0 {
            out.push_str(&input[start + 2..]);
            return out;
        }

        let original = input[start + 2..i].to_owned();
        let token = store.insert(original);
        out.push_str(&token);
        out.push(')');
        cursor = i + 1;
    }

    out.push_str(&input[cursor..]);
    out
}

fn protect_autolinks_and_bare_urls(input: &str, store: &mut TokenStore) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while cursor < input.len() {
        let next_autolink = input[cursor..].find("<http");
        let next_http = input[cursor..].find("http://");
        let next_https = input[cursor..].find("https://");

        let next = [next_autolink, next_http, next_https]
            .into_iter()
            .flatten()
            .min();

        let Some(rel_start) = next else {
            out.push_str(&input[cursor..]);
            break;
        };

        let start = cursor + rel_start;
        out.push_str(&input[cursor..start]);

        if input[start..].starts_with("<http")
            && let Some(rel_end) = input[start..].find('>')
        {
            let end = start + rel_end + 1;
            let original = input[start..end].to_owned();
            let token = store.insert(original);
            out.push_str(&token);
            cursor = end;
            continue;
        }

        let end = input[start..]
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(rel, _)| start + rel)
            .unwrap_or_else(|| input.len());
        let original = input[start..end].to_owned();
        let token = store.insert(original);
        out.push_str(&token);
        cursor = end;
    }

    out
}

fn normalize_placeholder_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < input.len() {
        let rest = &input[i..];

        if rest.starts_with('{')
            && let Some((consumed, token)) = parse_double_braced_placeholder(rest)
        {
            out.push_str(&token);
            i += consumed;
            continue;
        }

        if rest.starts_with('{')
            && let Some((consumed, token)) = parse_braced_placeholder(rest, 1, "}")
        {
            out.push_str(&token);
            i += consumed;
            continue;
        }

        if rest.starts_with("SBY_TOKEN_")
            && let Some((consumed, token)) = parse_bare_placeholder(rest)
        {
            out.push_str(&token);
            i += consumed;
            continue;
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

fn parse_double_braced_placeholder(input: &str) -> Option<(usize, String)> {
    if !input.starts_with('{') {
        return None;
    }

    let mut i = 1;

    if input[i..].starts_with('{') {
        i += 1;
    } else {
        i = skip_ws(input, i);
        if !input[i..].starts_with('{') {
            return None;
        }
        i += 1;
    }

    i = skip_ws(input, i);
    if !input[i..].starts_with("SBY_TOKEN_") {
        return None;
    }
    i += "SBY_TOKEN_".len();

    if input.len() < i + 6 {
        return None;
    }
    let digits = &input[i..i + 6];
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    i += 6;

    i = skip_ws(input, i);
    if !input[i..].starts_with('}') {
        return None;
    }
    i += 1;

    if input[i..].starts_with('}') {
        i += 1;
    } else {
        i = skip_ws(input, i);
        if !input[i..].starts_with('}') {
            return None;
        }
        i += 1;
    }

    Some((i, format!("{{{{SBY_TOKEN_{digits}}}}}")))
}

fn parse_braced_placeholder(input: &str, open_len: usize, close: &str) -> Option<(usize, String)> {
    if input.len() < open_len + close.len() {
        return None;
    }
    let mut i = open_len;
    i = skip_ws(input, i);
    if !input[i..].starts_with("SBY_TOKEN_") {
        return None;
    }
    i += "SBY_TOKEN_".len();

    let digits = input[i..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .take(6)
        .collect::<String>();
    if digits.len() != 6 {
        return None;
    }
    i += 6;
    i = skip_ws(input, i);

    if !input[i..].starts_with(close) {
        return None;
    }
    i += close.len();

    Some((i, format!("{{{{SBY_TOKEN_{digits}}}}}")))
}

fn parse_bare_placeholder(input: &str) -> Option<(usize, String)> {
    if !input.starts_with("SBY_TOKEN_") {
        return None;
    }
    let prefix_len = "SBY_TOKEN_".len();
    if input.len() < prefix_len + 6 {
        return None;
    }
    let digits = &input[prefix_len..prefix_len + 6];
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((prefix_len + 6, format!("{{{{SBY_TOKEN_{digits}}}}}")))
}

fn skip_ws(input: &str, mut i: usize) -> usize {
    while i < input.len() {
        let ch = input[i..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }
    i
}

fn placeholder_spans(input: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel) = input[cursor..].find("{{SBY_TOKEN_") {
        let start = cursor + rel;
        let Some(rel_end) = input[start..].find("}}") else {
            break;
        };
        let end = start + rel_end + 2;
        spans.push((start, end));
        cursor = end;
    }

    spans
}

fn split_long_line_preserving_tokens(line: &str, max_chars: usize) -> anyhow::Result<Vec<&str>> {
    let spans = placeholder_spans(line);
    let mut parts = Vec::new();
    let mut cursor = 0usize;

    while cursor < line.len() {
        let mut end = (cursor + max_chars).min(line.len());
        while end > cursor && !line.is_char_boundary(end) {
            end -= 1;
        }
        if end == cursor {
            anyhow::bail!("unable to split UTF-8 line with max_chars={max_chars}");
        }

        loop {
            let mut adjusted = false;
            for (start, finish) in &spans {
                if *start < end && end < *finish {
                    end = if *start == cursor { *finish } else { *start };
                    adjusted = true;
                    break;
                }
            }
            if !adjusted {
                break;
            }

            if end > cursor + max_chars {
                anyhow::bail!(
                    "a placeholder token exceeds --openai-max-chars (token_len={}; max_chars={})",
                    end - cursor,
                    max_chars
                );
            }
            while end > cursor && !line.is_char_boundary(end) {
                end -= 1;
            }
            if end == cursor {
                anyhow::bail!(
                    "unable to split line without breaking placeholder tokens (max_chars={max_chars})"
                );
            }
        }

        parts.push(&line[cursor..end]);
        cursor = end;
    }

    Ok(parts)
}

fn chunk_by_lines(input: &str, max_chars: usize) -> anyhow::Result<Vec<String>> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in input.split_inclusive('\n') {
        let parts = if line.len() <= max_chars {
            vec![line]
        } else {
            split_long_line_preserving_tokens(line, max_chars).context("split long line")?
        };

        for part in parts {
            if !current.is_empty() && current.len() + part.len() > max_chars {
                chunks.push(std::mem::take(&mut current));
            }
            current.push_str(part);
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    Ok(chunks)
}

fn extract_placeholder_tokens(input: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel) = input[cursor..].find("{{SBY_TOKEN_") {
        let start = cursor + rel;
        let Some(rel_end) = input[start..].find("}}") else {
            break;
        };
        let end = start + rel_end + 2;
        let token = input[start..end].to_owned();
        if seen.insert(token.clone()) {
            tokens.push(token);
        }
        cursor = end;
    }

    tokens
}

fn missing_tokens(output: &str, expected: &[String]) -> Vec<String> {
    let mut missing = Vec::new();
    for token in expected {
        if !output.contains(token) {
            missing.push(token.clone());
        }
    }
    missing
}

fn unprotect_markdown(input: &str, tokens: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let start = i;
            if let Some(rel_end) = input[i..].find("}}") {
                let end = i + rel_end + 2;
                let candidate = &input[start..end];
                if let Some(original) = tokens.get(candidate) {
                    out.push_str(original);
                    i = end;
                    continue;
                }
            }
        }

        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

fn unprotect_markdown_fully(input: &str, tokens: &HashMap<String, String>) -> String {
    let mut current = input.to_owned();
    for _ in 0..8 {
        let next = unprotect_markdown(&current, tokens);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

struct TokenStore {
    next_id: usize,
    tokens: HashMap<String, String>,
}

impl TokenStore {
    fn new() -> Self {
        Self {
            next_id: 0,
            tokens: HashMap::new(),
        }
    }

    fn insert(&mut self, original: String) -> String {
        let token = format!("{{{{SBY_TOKEN_{:06}}}}}", self.next_id);
        self.next_id += 1;
        self.tokens.insert(token.clone(), original);
        token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_by_lines_splits_long_line_without_modifying_contents() -> anyhow::Result<()> {
        let input = "a".repeat(50);
        let chunks = chunk_by_lines(&input, 20)?;
        assert!(chunks.len() > 1, "expected multiple chunks");
        assert_eq!(chunks.concat(), input);
        assert!(chunks.iter().all(|c| c.len() <= 20));
        Ok(())
    }

    #[test]
    fn chunk_by_lines_does_not_split_placeholder_tokens() -> anyhow::Result<()> {
        let token = "{{SBY_TOKEN_000001}}";
        let prefix = "a".repeat(15);
        let suffix = "b".repeat(50);
        let input = format!("{prefix}{token}{suffix}");

        let chunks = chunk_by_lines(&input, 30)?;
        assert_eq!(chunks.concat(), input);
        assert!(chunks.iter().all(|c| c.len() <= 30));

        let spans = placeholder_spans(&input);
        assert_eq!(spans.len(), 1, "expected exactly one token span");

        let mut cursor = 0usize;
        for chunk in &chunks[..chunks.len().saturating_sub(1)] {
            cursor += chunk.len();
            for (start, end) in &spans {
                assert!(
                    !(*start < cursor && cursor < *end),
                    "chunk boundary splits placeholder token"
                );
            }
        }

        Ok(())
    }

    #[test]
    fn chunk_by_lines_preserves_utf8_boundaries() -> anyhow::Result<()> {
        let input = "あ".repeat(20);
        let chunks = chunk_by_lines(&input, 10)?;
        assert_eq!(chunks.concat(), input);
        assert!(chunks.iter().all(|c| c.len() <= 10));
        Ok(())
    }
}
