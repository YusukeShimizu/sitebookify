use anyhow::Context as _;

use crate::codex::{CodexConfig, exec_readonly};

pub fn rewrite_section_via_codex(
    language: &str,
    tone: &str,
    chapter_title: &str,
    section_title: &str,
    source_markdown: &str,
) -> anyhow::Result<String> {
    let mut store = TokenStore::new();
    let protected = protect_markdown(source_markdown, &mut store);

    let input_file = tempfile::NamedTempFile::new().context("create rewrite input temp file")?;
    std::fs::write(input_file.path(), protected)
        .with_context(|| format!("write rewrite input: {}", input_file.path().display()))?;

    let config = CodexConfig::from_env();
    let input_path = input_file.path().to_string_lossy();

    let prompt = build_codex_rewrite_prompt(
        language,
        tone,
        chapter_title,
        section_title,
        input_path.as_ref(),
    );

    let raw = exec_readonly(&prompt, &config).context("codex exec for rewrite")?;
    let rewritten = normalize_placeholder_tokens(raw.trim_end());

    if rewritten.trim().is_empty() {
        tracing::warn!("rewrite output is empty; keeping original section");
        return Ok(unprotect_markdown_fully(source_markdown, &store.tokens));
    }

    Ok(unprotect_markdown_fully(&rewritten, &store.tokens))
}

fn build_codex_rewrite_prompt(
    language: &str,
    tone: &str,
    chapter_title: &str,
    section_title: &str,
    input_path: &str,
) -> String {
    format!(
        "You are a book editor and technical writer.\n\
\n\
Task: Rewrite the input Markdown into book-first prose for a single book section.\n\
\n\
Context:\n\
- Chapter title: {chapter_title}\n\
- Section title: {section_title}\n\
- Language: {language}\n\
- Tone: {tone}\n\
\n\
Hard rules:\n\
- Use ONLY the facts present in the input Markdown. Do not add new facts.\n\
- If something is unclear, explicitly say it is unknown/unclear (in the specified language).\n\
- Prefer paragraphs with smooth transitions.\n\
- Headings MUST be minimal.\n\
  - Do NOT output Markdown headings (`#`, `##`, `###`).\n\
  - The tool will add the section heading.\n\
- Bullet lists MUST be limited to summarizing key points.\n\
  - Do not overuse lists.\n\
- Avoid web/article vocabulary like \"この記事では\".\n\
  - Prefer chapter/section vocabulary like \"本章では\" / \"本節では\".\n\
- If helpful, use a compact pattern: short intro → explanation → example → short wrap-up.\n\
- Figures/images should be included ONLY when truly necessary.\n\
  - If you keep a figure, explain it in text before placing it.\n\
- Do NOT change code blocks, inline code, URLs, or HTML tags.\n\
- You MUST preserve placeholder tokens of the form {{{{SBY_TOKEN_000000}}}} exactly as they appear (do not remove or alter them).\n\
- Do NOT mention this instruction text.\n\
\n\
Input:\n\
- Read the Markdown from the file at: {input_path}\n\
\n\
Output:\n\
- Output ONLY the rewritten Markdown body for this section.\n",
        chapter_title = chapter_title,
        section_title = section_title,
        language = language,
        tone = tone,
        input_path = input_path,
    )
}

#[derive(Debug, Default)]
struct TokenStore {
    tokens: Vec<String>,
}

impl TokenStore {
    fn new() -> Self {
        Self { tokens: Vec::new() }
    }

    fn insert(&mut self, original: String) -> String {
        let idx = self.tokens.len();
        self.tokens.push(original);
        format!("{{{{SBY_TOKEN_{idx:06}}}}}")
    }
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
            let token = store.insert(std::mem::take(&mut block));
            out.push_str(&token);
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

fn normalize_placeholder_tokens(input: &str) -> String {
    // A small normalizer for common model mistakes:
    // - `{SBY_TOKEN_0}` -> `{{SBY_TOKEN_000000}}`
    // - `SBY_TOKEN_0` -> `{{SBY_TOKEN_000000}}`
    // - `{{{SBY_TOKEN_000000}}}` -> `{{SBY_TOKEN_000000}}`
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < input.len() {
        let rest = &input[i..];

        if rest.starts_with('{')
            && let Some((consumed, token)) = parse_any_braced_placeholder(rest)
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

fn parse_any_braced_placeholder(input: &str) -> Option<(usize, String)> {
    let bytes = input.as_bytes();
    let mut open_count = 0usize;
    while open_count < bytes.len() && bytes[open_count] == b'{' {
        open_count += 1;
    }
    if open_count == 0 {
        return None;
    }

    let mut i = open_count;
    i = skip_ws(input, i);
    if !input[i..].starts_with("SBY_TOKEN_") {
        return None;
    }

    let (consumed_inner, token) = parse_bare_placeholder(&input[i..])?;
    i += consumed_inner;
    i = skip_ws(input, i);

    let mut close_count = 0usize;
    while i + close_count < bytes.len() && bytes[i + close_count] == b'}' {
        close_count += 1;
    }
    if close_count == 0 {
        return None;
    }

    Some((i + close_count, token))
}

fn parse_bare_placeholder(input: &str) -> Option<(usize, String)> {
    if !input.starts_with("SBY_TOKEN_") {
        return None;
    }
    let rest = &input["SBY_TOKEN_".len()..];
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let token_digits = &rest[..digits];
    let idx: usize = token_digits.parse().ok()?;
    let canonical = format!("SBY_TOKEN_{idx:06}");
    let consumed = "SBY_TOKEN_".len() + digits;
    Some((consumed, format!("{{{{{canonical}}}}}")))
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

fn unprotect_markdown_fully(input: &str, originals: &[String]) -> String {
    let mut out = input.to_owned();
    for (idx, original) in originals.iter().enumerate() {
        let token = format!("{{{{SBY_TOKEN_{idx:06}}}}}");
        out = out.replace(&token, original);
    }
    out
}
