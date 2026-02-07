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
pub struct BuildArgs {
    /// Natural-language prompt describing the book to generate.
    #[arg(long)]
    pub query: String,

    /// Output directory for workspace (extracted/manifest/toc/book).
    #[arg(long)]
    pub out: String,

    /// Book title (default: derived from TOC / LLM).
    #[arg(long)]
    pub title: Option<String>,

    /// Maximum characters in composed Markdown from llm-spider.
    #[arg(long, default_value_t = 50000)]
    pub max_chars: usize,

    /// Minimum number of sources to gather.
    #[arg(long, default_value_t = 5)]
    pub min_sources: usize,

    /// Maximum results from the initial web search.
    #[arg(long, default_value_t = 10)]
    pub search_limit: usize,

    /// Maximum pages to crawl.
    #[arg(long, default_value_t = 20)]
    pub max_pages: usize,

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

    /// TOC creation engine (default: openai).
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
    pub toc_engine: LlmEngine,

    /// Book rendering engine (default: openai).
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
    pub render_engine: LlmEngine,
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

    /// Book title written to `toc.yaml` (default: derived from the manifest / LLM).
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

    /// TOC creation engine (default: openai).
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
    pub engine: LlmEngine,
}

#[derive(Debug, Subcommand)]
pub enum BookCommand {
    Init(BookInitArgs),
    Render(BookRenderArgs),
    Bundle(BookBundleArgs),
    Epub(BookEpubArgs),
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

    /// Book rendering engine (default: openai).
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
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

#[derive(Debug, Args)]
pub struct BookEpubArgs {
    /// Input directory for mdBook project (created by `book init` and `book render`).
    #[arg(long)]
    pub book: String,

    /// Output file path for EPUB.
    #[arg(long)]
    pub out: String,

    /// Overwrite output file if it already exists.
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Language tag (BCP-47) for EPUB metadata.
    #[arg(long, default_value = "und")]
    pub lang: String,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmEngine {
    /// Do nothing (copy input to output).
    Noop,

    /// Use OpenAI via Responses API.
    Openai,
}
