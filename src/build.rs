use std::path::PathBuf;

use anyhow::Context as _;

use crate::cli::{
    BookInitArgs, BookRenderArgs, BuildArgs, CrawlArgs, ExtractArgs, ManifestArgs, TocInitArgs,
};

pub async fn run(args: BuildArgs) -> anyhow::Result<()> {
    let workspace_dir = PathBuf::from(&args.out);
    if workspace_dir.exists() {
        anyhow::bail!(
            "workspace output directory already exists: {}",
            workspace_dir.display()
        );
    }
    std::fs::create_dir_all(&workspace_dir)
        .with_context(|| format!("create workspace dir: {}", workspace_dir.display()))?;

    let raw_dir = workspace_dir.join("raw");
    let extracted_dir = workspace_dir.join("extracted");
    let manifest_path = workspace_dir.join("manifest.jsonl");
    let toc_path = workspace_dir.join("toc.yaml");
    let book_dir = workspace_dir.join("book");

    tracing::info!(url = %args.url, out = %workspace_dir.display(), "build: crawl");
    crate::crawl::run(CrawlArgs {
        url: args.url.clone(),
        out: raw_dir.to_string_lossy().to_string(),
        max_pages: args.max_pages,
        max_depth: args.max_depth,
        concurrency: args.concurrency,
        delay_ms: args.delay_ms,
    })
    .await
    .context("crawl")?;

    tracing::info!("build: extract");
    crate::extract::run(ExtractArgs {
        raw: raw_dir.to_string_lossy().to_string(),
        out: extracted_dir.to_string_lossy().to_string(),
    })
    .context("extract")?;

    tracing::info!("build: manifest");
    crate::manifest::run(ManifestArgs {
        extracted: extracted_dir.to_string_lossy().to_string(),
        out: manifest_path.to_string_lossy().to_string(),
    })
    .context("manifest")?;

    tracing::info!("build: toc init");
    crate::toc::init(TocInitArgs {
        manifest: manifest_path.to_string_lossy().to_string(),
        out: toc_path.to_string_lossy().to_string(),
        book_title: Some(args.title.clone()),
    })
    .context("toc init")?;

    tracing::info!("build: book init");
    crate::book::init(BookInitArgs {
        out: book_dir.to_string_lossy().to_string(),
        title: args.title.clone(),
    })
    .context("book init")?;

    tracing::info!("build: book render");
    crate::book::render(BookRenderArgs {
        toc: toc_path.to_string_lossy().to_string(),
        manifest: manifest_path.to_string_lossy().to_string(),
        out: book_dir.to_string_lossy().to_string(),
    })
    .context("book render")?;

    Ok(())
}
