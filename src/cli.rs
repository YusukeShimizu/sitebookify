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

    /// Book title (default: derived from TOC / LLM).
    #[arg(long)]
    pub title: Option<String>,

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

    /// Language for TOC creation and book rendering.
    ///
    /// Examples: "日本語", "English"
    #[arg(long, default_value = "日本語")]
    pub language: String,

    /// Tone for TOC creation and book rendering.
    ///
    /// Examples: "丁寧", "フレンドリー", "堅め"
    #[arg(long, default_value = "丁寧")]
    pub tone: String,

    /// TOC creation engine (default: codex).
    #[arg(long, value_enum, default_value_t = LlmEngine::Codex)]
    pub toc_engine: LlmEngine,

    /// Book rendering engine (default: codex).
    #[arg(long, value_enum, default_value_t = LlmEngine::Codex)]
    pub render_engine: LlmEngine,
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
    Create(TocCreateArgs),
}

#[derive(Debug, Args)]
pub struct TocCreateArgs {
    /// Input path to `manifest.jsonl`.
    #[arg(long)]
    pub manifest: String,

    /// Output file path for `toc.yaml`.
    #[arg(long)]
    pub out: String,

    /// Book title written to `toc.yaml` (default: derived from the manifest / Codex).
    #[arg(long)]
    pub book_title: Option<String>,

    /// Overwrite output file if it already exists.
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Language for TOC creation.
    #[arg(long, default_value = "日本語")]
    pub language: String,

    /// Tone for TOC creation.
    #[arg(long, default_value = "丁寧")]
    pub tone: String,

    /// TOC creation engine (default: codex).
    #[arg(long, value_enum, default_value_t = LlmEngine::Codex)]
    pub engine: LlmEngine,
}

#[derive(Debug, Subcommand)]
pub enum BookCommand {
    Init(BookInitArgs),
    Render(BookRenderArgs),
    Bundle(BookBundleArgs),
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

    /// Language for book rendering.
    #[arg(long, default_value = "日本語")]
    pub language: String,

    /// Tone for book rendering.
    #[arg(long, default_value = "丁寧")]
    pub tone: String,

    /// Book rendering engine (default: codex).
    #[arg(long, value_enum, default_value_t = LlmEngine::Codex)]
    pub engine: LlmEngine,
}

#[derive(Debug, Args)]
pub struct BookBundleArgs {
    /// Input directory for mdBook project (created by `book init` and `book render`).
    #[arg(long)]
    pub book: String,

    /// Output file path for bundled Markdown.
    #[arg(long)]
    pub out: String,

    /// Overwrite output file if it already exists.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmEngine {
    /// Do nothing (copy input to output).
    Noop,

    /// Run Codex CLI.
    Codex,
}
