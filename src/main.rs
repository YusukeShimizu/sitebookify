use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser as _;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = try_main().await {
        eprintln!("{err:#}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

async fn try_main() -> anyhow::Result<()> {
    sitebookify::logging::init().context("init logging")?;

    let cli = sitebookify::cli::Cli::parse();
    tracing::debug!(?cli, "parsed cli");

    match cli.command {
        sitebookify::cli::Command::Build(args) => {
            sitebookify::build::run(args).await.context("build")?;
        }
        sitebookify::cli::Command::Crawl(args) => {
            sitebookify::crawl::run(args).await.context("crawl")?;
        }
        sitebookify::cli::Command::Extract(args) => {
            sitebookify::extract::run(args).context("extract")?;
        }
        sitebookify::cli::Command::Manifest(args) => {
            sitebookify::manifest::run(args).context("manifest")?;
        }
        sitebookify::cli::Command::Toc {
            command: sitebookify::cli::TocCommand::Init(args),
        } => {
            sitebookify::toc::init(args).context("toc init")?;
        }
        sitebookify::cli::Command::Book {
            command: sitebookify::cli::BookCommand::Init(args),
        } => {
            sitebookify::book::init(args).context("book init")?;
        }
        sitebookify::cli::Command::Book {
            command: sitebookify::cli::BookCommand::Render(args),
        } => {
            sitebookify::book::render(args).context("book render")?;
        }
    }

    Ok(())
}
