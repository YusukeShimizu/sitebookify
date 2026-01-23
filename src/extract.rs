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
                    format!("TODO: extraction failed for {}\n", record.normalized_url),
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
