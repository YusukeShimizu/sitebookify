use std::path::PathBuf;

use anyhow::Context as _;

use crate::cli::{
    BookBundleArgs, BookInitArgs, BookRenderArgs, BuildArgs, CrawlArgs, ExtractArgs,
    LlmRewritePagesArgs, ManifestArgs, TocInitArgs, TocRefineArgs,
};
use crate::formats::Toc;

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
    let bundled_md_path = workspace_dir.join("book.md");

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

    if args.toc_refine {
        tracing::info!("build: toc refine");
        crate::toc::refine(TocRefineArgs {
            manifest: manifest_path.to_string_lossy().to_string(),
            out: toc_path.to_string_lossy().to_string(),
            book_title: args.title.clone(),
            engine: args.toc_refine_engine,
            command: args.toc_refine_command.clone(),
            command_args: args.toc_refine_command_args.clone(),
            openai_model: args.openai_model.clone(),
            openai_base_url: args.openai_base_url.clone(),
            openai_temperature: args.openai_temperature,
            force: false,
        })
        .await
        .context("toc refine")?;
    } else {
        tracing::info!("build: toc init");
        crate::toc::init(TocInitArgs {
            manifest: manifest_path.to_string_lossy().to_string(),
            out: toc_path.to_string_lossy().to_string(),
            book_title: args.title.clone(),
        })
        .context("toc init")?;
    }

    let toc_yaml = std::fs::read_to_string(&toc_path)
        .with_context(|| format!("read toc: {}", toc_path.display()))?;
    let toc: Toc = serde_yaml::from_str(&toc_yaml).context("parse toc")?;

    let (manifest_for_book, _manuscript_dir) = if let Some(prompt) = args.rewrite_prompt.clone() {
        let manuscript_dir = args
            .rewrite_out
            .as_deref()
            .map(PathBuf::from)
            .map(|p| {
                if p.is_absolute() {
                    p
                } else {
                    workspace_dir.join(p)
                }
            })
            .unwrap_or_else(|| workspace_dir.join("manuscript"));

        let manuscript_manifest_path = workspace_dir.join("manifest.manuscript.jsonl");

        tracing::info!(
            out = %manuscript_dir.display(),
            "build: llm rewrite-pages"
        );
        crate::llm::rewrite_pages(LlmRewritePagesArgs {
            toc: toc_path.to_string_lossy().to_string(),
            manifest: manifest_path.to_string_lossy().to_string(),
            out: manuscript_dir.to_string_lossy().to_string(),
            prompt,
            engine: args.rewrite_engine,
            command: args.rewrite_command.clone(),
            command_args: args.rewrite_command_args.clone(),
            openai_model: args.openai_model.clone(),
            openai_base_url: args.openai_base_url.clone(),
            openai_max_chars: args.openai_max_chars,
            openai_temperature: args.openai_temperature,
            openai_concurrency: args.openai_concurrency,
            openai_retries: args.openai_retries,
            allow_missing_tokens: args.rewrite_allow_missing_tokens,
            force: false,
        })
        .await
        .context("llm rewrite-pages")?;

        tracing::info!("build: manifest (manuscript)");
        crate::manifest::run(ManifestArgs {
            extracted: manuscript_dir.to_string_lossy().to_string(),
            out: manuscript_manifest_path.to_string_lossy().to_string(),
        })
        .context("manifest (manuscript)")?;

        (manuscript_manifest_path, Some(manuscript_dir))
    } else {
        (manifest_path.clone(), None)
    };

    tracing::info!("build: book init");
    crate::book::init(BookInitArgs {
        out: book_dir.to_string_lossy().to_string(),
        title: toc.book_title,
    })
    .context("book init")?;

    tracing::info!("build: book render");
    let render_args = BookRenderArgs {
        toc: toc_path.to_string_lossy().to_string(),
        manifest: manifest_for_book.to_string_lossy().to_string(),
        out: book_dir.to_string_lossy().to_string(),
    };
    tokio::task::block_in_place(|| crate::book::render(render_args)).context("book render")?;

    tracing::info!("build: book bundle");
    crate::book::bundle(BookBundleArgs {
        book: book_dir.to_string_lossy().to_string(),
        out: bundled_md_path.to_string_lossy().to_string(),
        force: false,
    })
    .context("book bundle")?;

    Ok(())
}
