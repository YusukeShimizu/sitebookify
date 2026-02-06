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
        sitebookify::cli::Command::Toc {
            command: sitebookify::cli::TocCommand::Create(args),
        } => {
            sitebookify::toc::create(args).await.context("toc create")?;
        }
        sitebookify::cli::Command::Book {
            command: sitebookify::cli::BookCommand::Init(args),
        } => {
            sitebookify::book::init(args).context("book init")?;
        }
        sitebookify::cli::Command::Book {
            command: sitebookify::cli::BookCommand::Render(args),
        } => {
            tokio::task::block_in_place(|| sitebookify::book::render(args))
                .context("book render")?;
        }
        sitebookify::cli::Command::Book {
            command: sitebookify::cli::BookCommand::Bundle(args),
        } => {
            sitebookify::book::bundle(args).context("book bundle")?;
        }
        sitebookify::cli::Command::Book {
            command: sitebookify::cli::BookCommand::Epub(args),
        } => {
            sitebookify::book::epub(args).context("book epub")?;
        }
    }

    Ok(())
}
