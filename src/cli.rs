use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Build(BuildArgs),
    Crawl(CrawlArgs),
    Extract(ExtractArgs),
    Manifest(ManifestArgs),
    Toc {
        #[command(subcommand)]
        command: TocCommand,
    },
    Book {
        #[command(subcommand)]
        command: BookCommand,
    },
}

#[derive(Debug, Args)]
pub struct CrawlArgs {
    /// Start URL (must be http/https).
    #[arg(long)]
    pub url: String,

    /// Output directory for Raw snapshot.
    #[arg(long)]
    pub out: String,

    /// Maximum pages to retrieve.
    #[arg(long, default_value_t = 200)]
    pub max_pages: usize,

    /// Maximum link depth to traverse.
    #[arg(long, default_value_t = 8)]
    pub max_depth: u32,

    /// Maximum concurrent HTTP requests.
    #[arg(long, default_value_t = 4)]
    pub concurrency: usize,

    /// Delay before each request (politeness).
    #[arg(long, default_value_t = 200)]
    pub delay_ms: u64,
}

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Start URL (must be http/https).
    #[arg(long)]
    pub url: String,

    /// Output directory for workspace (raw/extracted/manifest/toc/book).
    #[arg(long)]
    pub out: String,

    /// Book title (written to `book/book.toml`).
    #[arg(long)]
    pub title: String,

    /// Maximum pages to retrieve.
    #[arg(long, default_value_t = 200)]
    pub max_pages: usize,

    /// Maximum link depth to traverse.
    #[arg(long, default_value_t = 8)]
    pub max_depth: u32,

    /// Maximum concurrent HTTP requests.
    #[arg(long, default_value_t = 4)]
    pub concurrency: usize,

    /// Delay before each request (politeness).
    #[arg(long, default_value_t = 200)]
    pub delay_ms: u64,
}

#[derive(Debug, Args)]
pub struct ExtractArgs {
    /// Raw snapshot directory (created by `crawl`).
    #[arg(long)]
    pub raw: String,

    /// Output directory for Extracted Pages snapshot.
    #[arg(long)]
    pub out: String,
}

#[derive(Debug, Args)]
pub struct ManifestArgs {
    /// Extracted Pages directory (created by `extract`).
    #[arg(long)]
    pub extracted: String,

    /// Output file path for `manifest.jsonl`.
    #[arg(long)]
    pub out: String,
}

#[derive(Debug, Subcommand)]
pub enum TocCommand {
    Init(TocInitArgs),
}

#[derive(Debug, Args)]
pub struct TocInitArgs {
    /// Input path to `manifest.jsonl`.
    #[arg(long)]
    pub manifest: String,

    /// Output file path for `toc.yaml`.
    #[arg(long)]
    pub out: String,

    /// Book title written to `toc.yaml` (default: derived from the manifest).
    #[arg(long)]
    pub book_title: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum BookCommand {
    Init(BookInitArgs),
    Render(BookRenderArgs),
}

#[derive(Debug, Args)]
pub struct BookInitArgs {
    /// Output directory for mdBook project.
    #[arg(long)]
    pub out: String,

    /// Book title (written to `book.toml`).
    #[arg(long)]
    pub title: String,
}

#[derive(Debug, Args)]
pub struct BookRenderArgs {
    /// Input path to `toc.yaml`.
    #[arg(long)]
    pub toc: String,

    /// Input path to `manifest.jsonl`.
    #[arg(long)]
    pub manifest: String,

    /// Output directory for mdBook project (created by `book init`).
    #[arg(long)]
    pub out: String,
}
