use std::collections::HashMap;
use std::collections::HashSet;
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

    let expected_tokens = extract_placeholder_tokens(&protected);

    let chunks = chunk_by_lines(&protected, args.openai_max_chars)
        .context("chunk protected markdown for OpenAI")?;

    if chunks.is_empty() {
        anyhow::bail!("input is empty after preprocessing");
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("build http client")?;

    let config = OpenaiTranslationConfig {
        client,
        endpoint: openai::responses_endpoint(&args.openai_base_url),
        api_key,
        model: args.openai_model.clone(),
        to: args.to.clone(),
        temperature: args.openai_temperature,
    };

    let total_chunks = chunks.len();
    let concurrency = args.openai_concurrency.max(1).min(chunks.len().max(1));
    tracing::info!(
        engine = "openai",
        chunks = total_chunks,
        concurrency = concurrency,
        max_chars = args.openai_max_chars,
        retries = args.openai_retries,
        "llm translate: chunked input"
    );

    enum ChunkOutcome {
        Ok(String),
        Fallback { original: String, error: String },
    }

    let mut join_set = tokio::task::JoinSet::new();
    let mut next_idx = 0usize;
    let mut results: Vec<Option<String>> = vec![None; chunks.len()];
    let mut done_chunks = 0usize;
    let mut failed_chunks = 0usize;
    let mut failed_chunk_samples = Vec::new();
    let started_at = std::time::Instant::now();
    let mut last_progress_log_at = started_at;

    while next_idx < chunks.len() || !join_set.is_empty() {
        while next_idx < chunks.len() && join_set.len() < concurrency {
            let chunk_index = next_idx;
            let chunk = chunks[chunk_index].clone();

            let config = config.clone();
            let retries = args.openai_retries;

            join_set.spawn(async move {
                tracing::debug!(
                    engine = "openai",
                    chunk_index = chunk_index,
                    "translate chunk"
                );
                let translated =
                    match translate_chunk_via_openai_checked(&config, &chunk, retries, 0)
                        .await
                        .with_context(|| format!("translate chunk {chunk_index} via OpenAI"))
                    {
                        Ok(translated) => ChunkOutcome::Ok(translated),
                        Err(err) => ChunkOutcome::Fallback {
                            original: chunk,
                            error: format!("{err:#}"),
                        },
                    };
                (chunk_index, translated)
            });

            next_idx += 1;
        }

        let Some(joined) = join_set.join_next().await else {
            break;
        };
        let (chunk_index, outcome) = joined.context("join OpenAI translation task")?;
        match outcome {
            ChunkOutcome::Ok(translated) => {
                results[chunk_index] = Some(translated);
            }
            ChunkOutcome::Fallback { original, error } => {
                failed_chunks += 1;
                if failed_chunk_samples.len() < 5 {
                    failed_chunk_samples.push(chunk_index);
                }
                tracing::warn!(
                    engine = "openai",
                    chunk_index = chunk_index,
                    error = %error,
                    "chunk translation failed; using original chunk"
                );
                results[chunk_index] = Some(original);
            }
        }

        done_chunks += 1;
        let elapsed = started_at.elapsed();
        if done_chunks == total_chunks || last_progress_log_at.elapsed() >= Duration::from_secs(2) {
            tracing::info!(
                engine = "openai",
                done = done_chunks,
                total = total_chunks,
                failed = failed_chunks,
                elapsed_ms = elapsed.as_millis() as u64,
                "llm translate: progress"
            );
            last_progress_log_at = std::time::Instant::now();
        }
    }

    if failed_chunks > 0 {
        tracing::warn!(
            engine = "openai",
            failed = failed_chunks,
            total = total_chunks,
            sample_chunk_indices = ?failed_chunk_samples,
            "llm translate: completed with failures; output contains original text for failed chunks"
        );
    }

    let mut results = results
        .into_iter()
        .enumerate()
        .map(|(idx, item)| {
            item.ok_or_else(|| anyhow::anyhow!("missing translation result for chunk {idx}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut translated_protected = concat_chunks(&results);
    translated_protected = normalize_placeholder_tokens(&translated_protected);

    let mut missing = missing_placeholder_tokens(&translated_protected, expected_tokens.iter());
    if !missing.is_empty() {
        tracing::warn!(
            engine = "openai",
            missing_tokens = missing.len(),
            "verify tokens preserved failed; patching translation output with original placeholders"
        );
        let patched = patch_translated_protected_with_original(&protected, &translated_protected);
        translated_protected = normalize_placeholder_tokens(&patched);

        missing = missing_placeholder_tokens(&translated_protected, expected_tokens.iter());
        if !missing.is_empty() {
            let restored =
                restore_original_chunks_for_missing_tokens(&missing, &chunks, &mut results);
            tracing::warn!(
                engine = "openai",
                missing_tokens = missing.len(),
                restored_chunks = restored.len(),
                restored_chunk_indices = ?restored,
                "verify tokens preserved failed after patch; restoring original chunks that contain missing placeholders"
            );

            translated_protected = concat_chunks(&results);
            translated_protected = normalize_placeholder_tokens(&translated_protected);
            missing = missing_placeholder_tokens(&translated_protected, expected_tokens.iter());
            if !missing.is_empty() {
                let sample = missing
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                tracing::error!(
                    engine = "openai",
                    missing_tokens = missing.len(),
                    missing_sample = %sample,
                    "failed to restore missing placeholders; returning original input"
                );
                return Ok(input.to_owned());
            }
        }
    }

    Ok(unprotect_markdown_fully(
        &translated_protected,
        &store.tokens,
    ))
}

#[derive(Clone)]
struct OpenaiTranslationConfig {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    to: String,
    temperature: f32,
}

async fn openai_translate_chunk(
    config: &OpenaiTranslationConfig,
    input_markdown: &str,
    strict: bool,
) -> anyhow::Result<String> {
    let mut instructions = format!(
        "You are a translation engine.\n\
Task: Translate the input Markdown into {to}.\n\
\n\
Hard rules:\n\
- Preserve Markdown structure and formatting as much as possible.\n\
- Do not add, remove, or reorder sections.\n\
- Do not summarize and do not add commentary.\n\
- Do not change code blocks, inline code, URLs, or HTML tags.\n\
- Do not change placeholder tokens of the form {{SBY_TOKEN_000000}} (copy them exactly; no spaces; two braces).\n\
- Keep the heading '## Sources' unchanged.\n",
        to = &config.to
    );
    if strict {
        instructions.push_str(
            "\n\
CRITICAL:\n\
- Your output MUST contain every placeholder token that appears in the input, unchanged.\n\
- Before replying, verify all placeholder tokens are present exactly.\n\
- If you cannot comply, output the input unchanged.\n",
        );
    }
    instructions.push_str("\nOutput:\n- Output ONLY the translated Markdown.\n");

    openai::responses_text(
        &config.client,
        &config.endpoint,
        &config.api_key,
        &config.model,
        &instructions,
        input_markdown,
        config.temperature,
    )
    .await
}

async fn translate_chunk_via_openai_checked(
    config: &OpenaiTranslationConfig,
    input_markdown: &str,
    retries: usize,
    split_depth: usize,
) -> anyhow::Result<String> {
    let attempts = retries.saturating_add(1);

    let mut stack: Vec<(String, usize)> = vec![(input_markdown.to_owned(), split_depth)];
    let mut translated = String::new();

    while let Some((chunk, depth)) = stack.pop() {
        let expected_tokens = extract_placeholder_tokens(&chunk);
        let mut last_missing = Vec::new();
        let mut chunk_translated = None;

        for attempt in 0..attempts {
            let strict = attempt > 0;
            let raw = openai_translate_chunk(config, &chunk, strict)
                .await
                .with_context(|| format!("call OpenAI (attempt {}/{})", attempt + 1, attempts))?;

            let out = normalize_placeholder_tokens(&raw);
            last_missing = missing_tokens(&out, &expected_tokens);
            if last_missing.is_empty() {
                chunk_translated = Some(out);
                break;
            }

            tracing::warn!(
                engine = "openai",
                attempt = attempt + 1,
                attempts = attempts,
                missing_tokens = last_missing.len(),
                "placeholder tokens modified; retrying"
            );
        }

        if let Some(chunk_translated) = chunk_translated {
            translated.push_str(&chunk_translated);
            continue;
        }

        if depth < 4
            && let Some((left, right)) = split_chunk_at_newline(&chunk)
        {
            tracing::info!(
                engine = "openai",
                split_depth = depth,
                "splitting chunk due to placeholder token mismatch"
            );
            stack.push((right, depth + 1));
            stack.push((left, depth + 1));
            continue;
        }

        let sample = last_missing
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "translation output is missing {} placeholder token(s) (e.g. {}); try reducing --openai-max-chars",
            last_missing.len(),
            sample
        );
    }

    Ok(translated)
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

fn split_chunk_at_newline(input: &str) -> Option<(String, String)> {
    let len = input.len();
    if len < 2 {
        return None;
    }

    let mid = len / 2;
    let left_split = input[..mid].rfind('\n').map(|idx| idx + 1);
    let right_split = input[mid..].find('\n').map(|rel| mid + rel + 1);

    let split_at = match (left_split, right_split) {
        (Some(a), Some(b)) => {
            if mid - a <= b - mid {
                a
            } else {
                b
            }
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };

    if split_at == 0 || split_at >= len {
        return None;
    }
    Some((input[..split_at].to_owned(), input[split_at..].to_owned()))
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

fn missing_placeholder_tokens<'a>(
    output: &str,
    tokens: impl Iterator<Item = &'a String>,
) -> Vec<String> {
    let mut missing = Vec::new();
    for token in tokens {
        if !output.contains(token) {
            missing.push(token.clone());
        }
    }
    missing
}

fn concat_chunks(chunks: &[String]) -> String {
    let total_len = chunks.iter().map(|c| c.len()).sum::<usize>();
    let mut out = String::with_capacity(total_len);
    for chunk in chunks {
        out.push_str(chunk);
    }
    out
}

fn restore_original_chunks_for_missing_tokens(
    missing_tokens: &[String],
    input_chunks: &[String],
    output_chunks: &mut [String],
) -> Vec<usize> {
    let mut restored = HashSet::<usize>::new();
    for token in missing_tokens {
        let Some(idx) = input_chunks.iter().position(|chunk| chunk.contains(token)) else {
            continue;
        };
        if restored.insert(idx) {
            output_chunks[idx] = input_chunks[idx].clone();
        }
    }

    let mut restored = restored.into_iter().collect::<Vec<_>>();
    restored.sort_unstable();
    restored
}

fn patch_translated_protected_with_original(
    original_protected: &str,
    translated_protected: &str,
) -> String {
    let original_tokens = placeholder_token_spans_in_order(original_protected);
    if original_tokens.is_empty() {
        return translated_protected.to_owned();
    }

    let mut translated_map = HashMap::<String, (usize, usize)>::new();
    for span in placeholder_token_spans_in_order(translated_protected) {
        translated_map
            .entry(span.token)
            .or_insert((span.start, span.end));
    }

    let mut out = String::with_capacity(original_protected.len() + 128);

    // Prefix: before first token.
    let first = &original_tokens[0];
    if let Some((t_start, _)) = translated_map.get(&first.token) {
        out.push_str(&translated_protected[..*t_start]);
    } else {
        out.push_str(&original_protected[..first.start]);
    }
    out.push_str(&first.token);

    // Middle segments.
    for window in original_tokens.windows(2) {
        let prev = &window[0];
        let next = &window[1];

        let orig_between = &original_protected[prev.end..next.start];
        let trans_between = match (
            translated_map.get(&prev.token),
            translated_map.get(&next.token),
        ) {
            (Some((_, prev_end)), Some((next_start, _))) if prev_end <= next_start => {
                Some(&translated_protected[*prev_end..*next_start])
            }
            _ => None,
        };

        out.push_str(trans_between.unwrap_or(orig_between));
        out.push_str(&next.token);
    }

    // Suffix: after last token.
    let last = original_tokens
        .last()
        .expect("original_tokens is non-empty");
    if let Some((_, t_end)) = translated_map.get(&last.token) {
        out.push_str(&translated_protected[*t_end..]);
    } else {
        out.push_str(&original_protected[last.end..]);
    }

    out
}

#[derive(Debug)]
struct PlaceholderTokenSpan {
    token: String,
    start: usize,
    end: usize,
}

fn placeholder_token_spans_in_order(input: &str) -> Vec<PlaceholderTokenSpan> {
    let mut spans = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel) = input[cursor..].find("{{SBY_TOKEN_") {
        let start = cursor + rel;
        let Some(rel_end) = input[start..].find("}}") else {
            break;
        };
        let end = start + rel_end + 2;
        let token = input[start..end].to_owned();
        spans.push(PlaceholderTokenSpan { token, start, end });
        cursor = end;
    }

    spans
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_placeholder_tokens_normalizes_braced_variants() {
        let cases = [
            ("{{SBY_TOKEN_000123}}", "{{SBY_TOKEN_000123}}"),
            ("{{ SBY_TOKEN_000123}}", "{{SBY_TOKEN_000123}}"),
            ("{{SBY_TOKEN_000123 }}", "{{SBY_TOKEN_000123}}"),
            ("{{ SBY_TOKEN_000123 }}", "{{SBY_TOKEN_000123}}"),
            ("{SBY_TOKEN_000123}", "{{SBY_TOKEN_000123}}"),
            ("{ SBY_TOKEN_000123 }", "{{SBY_TOKEN_000123}}"),
            ("SBY_TOKEN_000123", "{{SBY_TOKEN_000123}}"),
        ];

        for (input, expected) in cases {
            assert_eq!(
                normalize_placeholder_tokens(input),
                expected,
                "input={input}"
            );
        }
    }

    #[test]
    fn normalize_placeholder_tokens_normalizes_spaced_double_braces() {
        let cases = [
            ("{ {SBY_TOKEN_000123}}", "{{SBY_TOKEN_000123}}"),
            ("{ { SBY_TOKEN_000123 } }", "{{SBY_TOKEN_000123}}"),
            ("{{SBY_TOKEN_000123 } }", "{{SBY_TOKEN_000123}}"),
            (
                "{ { SBY_TOKEN_000123 } } trailing",
                "{{SBY_TOKEN_000123}} trailing",
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(
                normalize_placeholder_tokens(input),
                expected,
                "input={input}"
            );
        }
    }

    #[test]
    fn normalize_placeholder_tokens_ignores_invalid_tokens() {
        let input = "{SBY_TOKEN_00123} {SBY_TOKEN_abcdef} SBY_TOKEN_12345";
        assert_eq!(normalize_placeholder_tokens(input), input);
    }

    #[test]
    fn patch_translated_protected_with_original_reinserts_missing_tokens() {
        let original = "Hello {{SBY_TOKEN_000001}} world {{SBY_TOKEN_000002}}!\n";
        let translated_missing = "こんにちは {{SBY_TOKEN_000001}} 世界!\n";

        let patched = patch_translated_protected_with_original(original, translated_missing);
        assert!(patched.contains("{{SBY_TOKEN_000001}}"));
        assert!(patched.contains("{{SBY_TOKEN_000002}}"));
    }
}
