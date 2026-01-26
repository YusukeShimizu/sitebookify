use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Context as _;

use crate::cli::{LlmEngine, LlmTranslateArgs};
use crate::openai;

pub async fn translate(args: LlmTranslateArgs) -> anyhow::Result<()> {
    let input = std::fs::read_to_string(&args.input)
        .with_context(|| format!("read input markdown: {}", &args.input))?;

    let translated = match args.engine {
        LlmEngine::Noop => input,
        LlmEngine::Command => translate_via_command(&args, &input)?,
        LlmEngine::Openai => translate_via_openai(&args, &input).await?,
    };

    if translated.trim().is_empty() {
        anyhow::bail!("translation output is empty");
    }

    write_output(&args.out, &translated, args.force)?;
    Ok(())
}

fn translate_via_command(args: &LlmTranslateArgs, input: &str) -> anyhow::Result<String> {
    let Some(program) = args.command.as_deref() else {
        anyhow::bail!("missing --command (required when --engine=command)");
    };

    tracing::info!(
        engine = "command",
        command = program,
        out = %args.out,
        "llm translate"
    );

    let mut child = Command::new(program)
        .args(&args.command_args)
        .env("SITEBOOKIFY_TRANSLATE_TO", &args.to)
        .env("SITEBOOKIFY_TRANSLATE_IN", &args.input)
        .env("SITEBOOKIFY_TRANSLATE_OUT", &args.out)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn translator command: {program}"))?;

    {
        let mut stdin = child.stdin.take().context("open translator stdin")?;
        stdin
            .write_all(input.as_bytes())
            .context("write translator stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("wait translator process")?;
    if !output.status.success() {
        anyhow::bail!("translator command failed: {program} ({})", output.status);
    }

    let stdout =
        String::from_utf8(output.stdout).context("translator stdout is not valid UTF-8")?;
    Ok(stdout)
}

async fn translate_via_openai(args: &LlmTranslateArgs, input: &str) -> anyhow::Result<String> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY is not set"))?;

    if args.openai_max_chars == 0 {
        anyhow::bail!("--openai-max-chars must be > 0");
    }

    tracing::info!(
        engine = "openai",
        model = %args.openai_model,
        out = %args.out,
        "llm translate"
    );

    let mut store = TokenStore::new();
    let protected = protect_markdown(input, &mut store);

    let chunks = chunk_by_lines(&protected, args.openai_max_chars)
        .context("chunk protected markdown for OpenAI")?;

    let endpoint = openai::responses_endpoint(&args.openai_base_url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("build http client")?;

    let mut translated_protected = String::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        tracing::debug!(
            engine = "openai",
            chunk_index = idx,
            chunk_count = chunks.len(),
            "translate chunk"
        );

        let chunk_translated = openai_translate_chunk(
            &client,
            &endpoint,
            &api_key,
            &args.openai_model,
            &args.to,
            args.openai_temperature,
            chunk,
        )
        .await
        .with_context(|| format!("translate chunk {idx} via OpenAI"))?;
        translated_protected.push_str(&chunk_translated);
    }

    ensure_all_tokens_present(&translated_protected, store.tokens.keys())
        .context("verify tokens preserved")?;

    Ok(unprotect_markdown(&translated_protected, &store.tokens))
}

async fn openai_translate_chunk(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    to: &str,
    temperature: f32,
    input_markdown: &str,
) -> anyhow::Result<String> {
    let instructions = format!(
        "You are a translation engine.\n\
Task: Translate the input Markdown into {to}.\n\
\n\
Hard rules:\n\
- Preserve Markdown structure and formatting as much as possible.\n\
- Do not add, remove, or reorder sections.\n\
- Do not summarize and do not add commentary.\n\
- Do not change code blocks, inline code, URLs, or HTML tags.\n\
- Do not change tokens of the form {{SBY_TOKEN_000000}} (leave them exactly).\n\
- Keep the heading '## Sources' unchanged.\n\
\n\
Output:\n\
	- Output ONLY the translated Markdown.\n"
    );

    openai::responses_text(
        client,
        endpoint,
        api_key,
        model,
        &instructions,
        input_markdown,
        temperature,
    )
    .await
}

fn chunk_by_lines(input: &str, max_chars: usize) -> anyhow::Result<Vec<String>> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in input.split_inclusive('\n') {
        if line.len() > max_chars {
            anyhow::bail!(
                "a single line exceeds --openai-max-chars (line_len={}; max_chars={})",
                line.len(),
                max_chars
            );
        }

        if !current.is_empty() && current.len() + line.len() > max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    Ok(chunks)
}

fn ensure_all_tokens_present<'a>(
    output: &str,
    tokens: impl Iterator<Item = &'a String>,
) -> anyhow::Result<()> {
    let mut missing = Vec::new();
    for token in tokens {
        if !output.contains(token) {
            missing.push(token.clone());
        }
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "translation output is missing {} token(s); the model likely modified placeholders",
            missing.len()
        );
    }
    Ok(())
}

fn protect_markdown(input: &str, store: &mut TokenStore) -> String {
    let text = protect_fenced_code_blocks(input, store);
    let text = protect_sources_heading(&text, store);
    let text = protect_inline_code_spans(&text, store);
    let text = protect_markdown_link_destinations(&text, store);
    protect_autolinks_and_bare_urls(&text, store)
}

fn protect_sources_heading(input: &str, store: &mut TokenStore) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        if trimmed == "## Sources" {
            let mut original = line.to_owned();
            let trailing_newline = original.ends_with('\n');
            if trailing_newline {
                original.pop();
            }
            let token = store.insert(original);
            out.push_str(&token);
            if trailing_newline {
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
    }
    out
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
        // Unclosed fence: keep as-is (do not tokenize).
        out.push_str(&block);
    }

    out
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
            // Unclosed: keep the rest.
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

        // Bare URL: capture until whitespace.
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

        // Copy one char (valid UTF-8 boundary).
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
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

fn write_output(path: &str, contents: &str, force: bool) -> anyhow::Result<()> {
    if std::path::Path::new(path).exists() && !force {
        anyhow::bail!("output already exists: {path}");
    }
    if let Some(parent) = std::path::Path::new(path).parent()
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
        .with_context(|| format!("open output: {path}"))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("write output: {path}"))?;
    file.flush()
        .with_context(|| format!("flush output: {path}"))?;
    Ok(())
}
