use std::path::PathBuf;

use anyhow::Context as _;

use crate::cli::{
    BookBundleArgs, BookInitArgs, BookRenderArgs, BuildArgs, CrawlArgs, ExtractArgs,
    LlmTranslateArgs, ManifestArgs, TocInitArgs, TocRefineArgs,
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

    tracing::info!("build: book init");
    crate::book::init(BookInitArgs {
        out: book_dir.to_string_lossy().to_string(),
        title: toc.book_title,
    })
    .context("book init")?;

    tracing::info!("build: book render");
    let render_args = BookRenderArgs {
        toc: toc_path.to_string_lossy().to_string(),
        manifest: manifest_path.to_string_lossy().to_string(),
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

    if let Some(to) = &args.translate_to {
        let translated_out = args.translate_out.clone().unwrap_or_else(|| {
            default_translated_md_path(workspace_dir.as_path(), to)
                .to_string_lossy()
                .to_string()
        });

        tracing::info!(to = %to, out = %translated_out, "build: llm translate");
        crate::llm::translate(LlmTranslateArgs {
            input: bundled_md_path.to_string_lossy().to_string(),
            out: translated_out,
            to: to.clone(),
            engine: args.translate_engine,
            command: args.translate_command.clone(),
            command_args: args.translate_command_args.clone(),
            openai_model: args.openai_model.clone(),
            openai_base_url: args.openai_base_url.clone(),
            openai_max_chars: args.openai_max_chars,
            openai_temperature: args.openai_temperature,
            openai_concurrency: args.openai_concurrency,
            openai_retries: args.openai_retries,
            force: false,
        })
        .await
        .context("llm translate")?;
    }

    Ok(())
}

fn default_translated_md_path(workspace_dir: &std::path::Path, to: &str) -> PathBuf {
    let mut slug = String::new();
    for ch in to.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch);
        } else {
            slug.push('_');
        }
    }
    if slug.trim_matches('_').is_empty() {
        slug = "translated".to_owned();
    }
    workspace_dir.join(format!("book.{slug}.md"))
}
