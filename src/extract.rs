use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;
use readability_js::{Readability, ReadabilityError, ReadabilityOptions};

use crate::cli::ExtractArgs;
use crate::formats::{CrawlRecord, ExtractedFrontMatter};

pub fn run(args: ExtractArgs) -> anyhow::Result<()> {
    let raw_dir = PathBuf::from(&args.raw);
    let out_dir = PathBuf::from(&args.out);

    if out_dir.exists() {
        anyhow::bail!(
            "extracted snapshot output directory already exists: {}",
            out_dir.display()
        );
    }

    let readability = Readability::new().context("initialize readability-js")?;

    let crawl_jsonl_path = raw_dir.join("crawl.jsonl");
    let crawl_jsonl = OpenOptions::new()
        .read(true)
        .open(&crawl_jsonl_path)
        .with_context(|| format!("open crawl log: {}", crawl_jsonl_path.display()))?;
    let reader = BufReader::new(crawl_jsonl);

    let pages_dir = out_dir.join("pages");
    std::fs::create_dir_all(&pages_dir)
        .with_context(|| format!("create extracted pages dir: {}", pages_dir.display()))?;

    for line in reader.lines() {
        let line = line.context("read crawl jsonl line")?;
        if line.trim().is_empty() {
            continue;
        }

        let record: CrawlRecord = serde_json::from_str(&line).context("parse crawl record")?;
        let Some(raw_html_path) = record.raw_html_path.as_deref() else {
            continue;
        };

        let html = std::fs::read_to_string(raw_html_path)
            .with_context(|| format!("read raw html: {raw_html_path}"))?;

        let extracted = extract_with_readability(&readability, &html, &record.normalized_url);
        let (mut title, mut body_md) = match extracted {
            Ok(content) => (content.title, content.body_md),
            Err(err) => {
                tracing::debug!(
                    url = %record.normalized_url,
                    ?err,
                    "readability extraction failed; writing placeholder"
                );
                (
                    record.normalized_url.clone(),
                    format!("Extraction failed for {}\n", record.normalized_url),
                )
            }
        };
        if title.trim().is_empty() {
            title = record.normalized_url.clone();
        }

        let id = page_id_from_normalized_url(&record.normalized_url);

        let front_matter = ExtractedFrontMatter {
            id: id.clone(),
            url: record.normalized_url.clone(),
            retrieved_at: record.retrieved_at.clone(),
            raw_html_path: raw_html_path.to_owned(),
            title: title.clone(),
        };

        body_md = body_md.trim().to_owned();
        if !body_md.trim_start().starts_with('#') {
            body_md = format!("# {}\n\n{body_md}", front_matter.title);
        }

        body_md = strip_known_boilerplate_sections(&body_md);
        body_md = body_md.trim().to_owned();
        if !body_md.trim_start().starts_with('#') {
            body_md = format!("# {}\n\n{body_md}", front_matter.title);
        }

        let yaml =
            serde_yaml::to_string(&front_matter).context("serialize extracted front matter")?;
        let markdown = format!("---\n{yaml}---\n\n{body_md}\n");

        let out_path = pages_dir.join(format!("{id}.md"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&out_path)
            .with_context(|| format!("create extracted page: {}", out_path.display()))?;
        file.write_all(markdown.as_bytes())
            .with_context(|| format!("write extracted page: {}", out_path.display()))?;
    }

    Ok(())
}

fn strip_known_boilerplate_sections(markdown: &str) -> String {
    strip_mdbook_keyboard_shortcuts_help(markdown)
}

fn strip_mdbook_keyboard_shortcuts_help(markdown: &str) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut fence_marker = String::new();

    let lines = markdown.lines().collect::<Vec<_>>();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];

        if !in_fence {
            if let Some(marker) = fence_start_marker(line) {
                in_fence = true;
                fence_marker.clear();
                fence_marker.push_str(marker);
                out.push(line);
                i += 1;
                continue;
            }
        } else {
            out.push(line);
            if fence_end_marker(line, &fence_marker) {
                in_fence = false;
            }
            i += 1;
            continue;
        }

        if let Some(heading) = parse_heading_at(&lines, i) {
            if is_keyboard_shortcuts_title(heading.title) {
                let lookahead_start = i + heading.consumed_lines;
                let lookahead_end = usize::min(lookahead_start + 20, lines.len());
                let score = mdbook_keyboard_shortcuts_score(&lines[lookahead_start..lookahead_end]);
                if score >= 2 {
                    let mut j = lookahead_start;
                    while j < lines.len() {
                        let next = lines[j];
                        if parse_heading_at(&lines, j).is_some() {
                            break;
                        }
                        if next.trim().is_empty() || mdbook_keyboard_shortcuts_score(&[next]) > 0 {
                            j += 1;
                            continue;
                        }
                        break;
                    }

                    i = j;
                    continue;
                }
            }
            for offset in 0..heading.consumed_lines {
                out.push(lines[i + offset]);
            }
            i += heading.consumed_lines;
            continue;
        }

        let title = line.trim();
        if is_keyboard_shortcuts_title(title) {
            let lookahead_end = usize::min(i + 20, lines.len());
            let score = mdbook_keyboard_shortcuts_score(&lines[i + 1..lookahead_end]);
            if score >= 2 {
                let mut j = i + 1;
                while j < lines.len() {
                    let next = lines[j];
                    if parse_heading_at(&lines, j).is_some() {
                        break;
                    }
                    if next.trim().is_empty() || mdbook_keyboard_shortcuts_score(&[next]) > 0 {
                        j += 1;
                        continue;
                    }
                    break;
                }
                i = j;
                continue;
            }
        }

        let lower = title.to_ascii_lowercase();
        if lower.starts_with("press") {
            let lookahead_end = usize::min(i + 20, lines.len());
            let score = mdbook_keyboard_shortcuts_score(&lines[i..lookahead_end]);
            if score >= 3 {
                let mut j = i;
                while j < lines.len() {
                    let next = lines[j];
                    if parse_heading_at(&lines, j).is_some() {
                        break;
                    }
                    let next_trimmed = next.trim();
                    if next_trimmed.is_empty()
                        || next_trimmed.to_ascii_lowercase().starts_with("press")
                        || mdbook_keyboard_shortcuts_score(&[next]) > 0
                    {
                        j += 1;
                        continue;
                    }
                    break;
                }
                i = j;
                continue;
            }
        }

        out.push(line);
        i += 1;
    }

    out.join("\n")
}

#[derive(Debug, Clone, Copy)]
struct ParsedHeading<'a> {
    title: &'a str,
    consumed_lines: usize,
}

fn parse_heading_at<'a>(lines: &[&'a str], i: usize) -> Option<ParsedHeading<'a>> {
    if let Some((_level, title)) = parse_heading(lines[i]) {
        return Some(ParsedHeading {
            title,
            consumed_lines: 1,
        });
    }

    if i + 1 < lines.len() {
        let title = lines[i].trim();
        if title.is_empty() {
            return None;
        }
        if let Some(_level) = parse_setext_underline_level(lines[i + 1]) {
            return Some(ParsedHeading {
                title,
                consumed_lines: 2,
            });
        }
    }

    None
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }

    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }

    let rest = &trimmed[level..];
    if rest.trim().is_empty() {
        return None;
    }

    Some((level, rest.trim()))
}

fn parse_setext_underline_level(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return None;
    }

    if trimmed.chars().all(|c| c == '=') {
        return Some(1);
    }
    if trimmed.chars().all(|c| c == '-') {
        return Some(2);
    }

    None
}

fn is_keyboard_shortcuts_title(title: &str) -> bool {
    let t = title.trim();
    if t == "キーボードショートカット" {
        return true;
    }
    t.eq_ignore_ascii_case("keyboard shortcuts")
}

fn mdbook_keyboard_shortcuts_score(lines: &[&str]) -> usize {
    let mut score = 0usize;

    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }

        if t.contains("章間の移動") {
            score += 1;
        }
        if t.contains("本の検索") {
            score += 1;
        }
        if t.contains("このヘルプ") {
            score += 1;
        }
        if t.contains("Esc") {
            score += 1;
        }

        let lower = t.to_ascii_lowercase();
        if (t.contains('←') || t.contains('→')) && lower.contains("chapter") {
            score += 1;
        }
        if lower.contains("navigate between chapters") {
            score += 1;
        }
        if lower.contains("search the book") {
            score += 1;
        }
        if lower.contains("search in the book") {
            score += 1;
        }
        if lower.contains("this help") && lower.contains("press") {
            score += 1;
        }
        if lower.contains("hide") && lower.contains("help") {
            score += 1;
        }
        if lower.contains("arrow") && lower.contains("chapter") {
            score += 1;
        }
    }

    score
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

fn page_id_from_normalized_url(normalized_url: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest as _;
    hasher.update(normalized_url.as_bytes());
    let digest = hasher.finalize();
    format!("p_{}", hex::encode(digest))
}

#[derive(Debug)]
struct ExtractedContent {
    title: String,
    body_md: String,
}

pub fn preview_character_count_from_html(
    readability: &Readability,
    html: &str,
    url: &str,
) -> Result<usize, ReadabilityError> {
    let extracted = extract_with_readability(readability, html, url)?;
    let mut title = extracted.title;
    if title.trim().is_empty() {
        title = url.to_string();
    }

    let mut body_md = extracted.body_md.trim().to_owned();
    if !body_md.trim_start().starts_with('#') {
        body_md = format!("# {title}\n\n{body_md}");
    }
    body_md = strip_known_boilerplate_sections(&body_md);
    body_md = body_md.trim().to_owned();
    if !body_md.trim_start().starts_with('#') {
        body_md = format!("# {title}\n\n{body_md}");
    }
    Ok(body_md.chars().count())
}

fn extract_with_readability(
    readability: &Readability,
    html: &str,
    url: &str,
) -> Result<ExtractedContent, ReadabilityError> {
    match readability.parse_with_url(html, url) {
        Ok(article) => Ok(ExtractedContent {
            title: article.title,
            body_md: html2md::parse_html(&article.content),
        }),
        Err(ReadabilityError::ReadabilityCheckFailed) => {
            let options = ReadabilityOptions::new()
                .char_threshold(0)
                .nb_top_candidates(10)
                .link_density_modifier(2.0);
            let article = readability.parse_with_options(html, Some(url), Some(options))?;
            Ok(ExtractedContent {
                title: article.title,
                body_md: html2md::parse_html(&article.content),
            })
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_mdbook_keyboard_shortcuts_help_japanese() {
        let input = "\
# Title

## キーボードショートカット
章間の移動には ← または → を押します
本の検索には S または / を押します
? を押すとこのヘルプを表示します
Esc を押すとこのヘルプを非表示にします

## Next
Keep.
";

        let out = strip_known_boilerplate_sections(input);
        assert!(!out.contains("キーボードショートカット"));
        assert!(!out.contains("章間の移動には"));
        assert!(out.contains("## Next"));
        assert!(out.contains("Keep."));
    }

    #[test]
    fn strip_mdbook_keyboard_shortcuts_help_english_without_heading() {
        let input = "\
# Title

### Manual Coin Selection - LWK Documentation

Press ← or → to navigate between chapters

Press S or / to search in the book

Press ? to show this help

Press Esc to hide this help

## Next
Keep.
";

        let out = strip_known_boilerplate_sections(input);
        assert!(!out.contains("navigate between chapters"));
        assert!(!out.contains("search in the book"));
        assert!(!out.contains("show this help"));
        assert!(!out.contains("hide this help"));
        assert!(out.contains("### Manual Coin Selection - LWK Documentation"));
        assert!(out.contains("## Next"));
        assert!(out.contains("Keep."));
    }
}
