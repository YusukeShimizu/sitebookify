use std::time::Duration;

use anyhow::Context as _;
use sha2::{Digest, Sha256};

use crate::formats::{ExtractedFrontMatter, ManifestRecord};

pub struct LlmCrawlArgs {
    pub query: String,
    pub out_dir: std::path::PathBuf,
    pub max_chars: usize,
    pub min_sources: usize,
    pub search_limit: usize,
    pub max_pages: usize,
}

pub async fn run(args: LlmCrawlArgs) -> anyhow::Result<()> {
    let out_dir = std::fs::canonicalize(&args.out_dir)
        .with_context(|| format!("canonicalize out_dir: {}", args.out_dir.display()))?;
    let extracted_dir = out_dir.join("extracted").join("pages");
    std::fs::create_dir_all(&extracted_dir)
        .with_context(|| format!("create extracted dir: {}", extracted_dir.display()))?;

    let manifest_path = out_dir.join("manifest.jsonl");

    let request = llm_spider::spider::UserRequest {
        query: args.query,
        max_chars: args.max_chars,
        min_sources: args.min_sources,
        search_limit: args.search_limit,
        max_pages: args.max_pages,
        max_depth: 1,
        max_elapsed: Duration::from_secs(120),
        max_child_candidates: 20,
        max_children_per_page: 3,
        allow_local: false,
    };

    let result = tokio::task::spawn_blocking(move || {
        let openai =
            llm_spider::openai::OpenAiClient::from_env().context("initialize OpenAI client")?;
        llm_spider::spider::crawl(&request, &openai).context("llm-spider crawl")
    })
    .await
    .context("spawn_blocking join")??;

    let now = chrono::Utc::now().to_rfc3339();
    let mut manifest_lines: Vec<String> = Vec::new();

    for source in &result.sources {
        let url_str = source.url.to_string();
        let id = page_id(&url_str);
        let filename = format!("p_{id}.md");
        let md_path = extracted_dir.join(&filename);

        let title = extract_title(&source.content).unwrap_or_else(|| url_str.clone());

        let front_matter = ExtractedFrontMatter {
            id: id.clone(),
            url: url_str.clone(),
            retrieved_at: now.clone(),
            raw_html_path: None,
            title: title.clone(),
            trust_tier: Some(source.trust_tier.as_str().to_string()),
        };

        let yaml = serde_yaml::to_string(&front_matter).context("serialize front matter")?;
        let content = format!("---\n{yaml}---\n\n{}", source.content);
        std::fs::write(&md_path, &content)
            .with_context(|| format!("write extracted page: {}", md_path.display()))?;

        let absolute_md = md_path
            .to_str()
            .with_context(|| format!("non-UTF-8 path: {}", md_path.display()))?
            .to_string();

        let record = ManifestRecord {
            id,
            url: url_str,
            title,
            path: absolute_md.clone(),
            extracted_md: absolute_md,
            trust_tier: Some(source.trust_tier.as_str().to_string()),
        };
        let line = serde_json::to_string(&record).context("serialize manifest record")?;
        manifest_lines.push(line);
    }

    let manifest_content = manifest_lines.join("\n");
    std::fs::write(&manifest_path, &manifest_content)
        .with_context(|| format!("write manifest: {}", manifest_path.display()))?;

    tracing::info!(
        sources = result.sources.len(),
        manifest = %manifest_path.display(),
        "llm_crawl complete"
    );

    Ok(())
}

fn page_id(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..16])
}

fn extract_title(excerpt: &str) -> Option<String> {
    let first_line = excerpt.lines().next()?.trim();
    let title = first_line.strip_prefix('#').unwrap_or(first_line).trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}
