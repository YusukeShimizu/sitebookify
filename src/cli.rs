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
    Llm {
        #[command(subcommand)]
        command: LlmCommand,
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

    /// Refine chapter grouping and reading order via LLM before rendering.
    #[arg(long, default_value_t = false)]
    pub toc_refine: bool,

    /// TOC refinement engine used when `--toc-refine` is set.
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
    pub toc_refine_engine: LlmEngine,

    /// TOC refinement command (used when toc-refine-engine=command).
    #[arg(long, value_name = "PROGRAM")]
    pub toc_refine_command: Option<String>,

    /// TOC refinement argument (repeatable, used when toc-refine-engine=command).
    #[arg(long = "toc-refine-command-arg")]
    pub toc_refine_command_args: Vec<String>,

    /// Rewrite pages into book-first prose using the given prompt.
    /// When unset, rewrite is skipped and the book is rendered from `extracted/` as-is.
    #[arg(long)]
    pub rewrite_prompt: Option<String>,

    /// Rewrite engine used when `--rewrite-prompt` is set.
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
    pub rewrite_engine: LlmEngine,

    /// Output directory for rewritten pages (default: `<OUT>/manuscript`).
    #[arg(long)]
    pub rewrite_out: Option<String>,

    /// Rewrite command (used when rewrite-engine=command).
    #[arg(long, value_name = "PROGRAM")]
    pub rewrite_command: Option<String>,

    /// Rewrite command argument (repeatable, used when rewrite-engine=command).
    #[arg(long = "rewrite-command-arg")]
    pub rewrite_command_args: Vec<String>,

    /// OpenAI model (used when an engine uses OpenAI).
    #[arg(long, default_value = "gpt-5-mini")]
    pub openai_model: String,

    /// OpenAI API base URL (used when an engine uses OpenAI).
    #[arg(long, default_value = "https://api.openai.com/v1")]
    pub openai_base_url: String,

    /// Maximum characters per OpenAI request (used when an engine uses OpenAI).
    #[arg(long, default_value_t = 12_000)]
    pub openai_max_chars: usize,

    /// OpenAI temperature (used when an engine uses OpenAI; ignored for `gpt-5*` models).
    #[arg(long, default_value_t = 0.0)]
    pub openai_temperature: f32,

    /// Maximum concurrent OpenAI requests (used when an engine uses OpenAI).
    #[arg(long, default_value_t = 1)]
    pub openai_concurrency: usize,

    /// Retries per OpenAI chunk when placeholder tokens are modified (used by some OpenAI flows).
    #[arg(long, default_value_t = 1)]
    pub openai_retries: usize,

    /// Allow rewritten output even if placeholder tokens are missing.
    ///
    /// When enabled, the tool will keep the LLM output and restore only the placeholder tokens
    /// that remain. This can drop code/URLs if the model removed them.
    #[arg(long, default_value_t = false)]
    pub rewrite_allow_missing_tokens: bool,
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
    Refine(TocRefineArgs),
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

#[derive(Debug, Args)]
pub struct TocRefineArgs {
    /// Input path to `manifest.jsonl`.
    #[arg(long)]
    pub manifest: String,

    /// Output file path for refined `toc.yaml`.
    #[arg(long)]
    pub out: String,

    /// Book title written to `toc.yaml` (default: derived from the manifest).
    #[arg(long)]
    pub book_title: Option<String>,

    /// LLM engine.
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
    pub engine: LlmEngine,

    /// LLM command (required when engine=command).
    #[arg(long, value_name = "PROGRAM")]
    pub command: Option<String>,

    /// LLM command arguments (use `--` before the args).
    #[arg(trailing_var_arg = true)]
    pub command_args: Vec<String>,

    /// OpenAI model (used when engine=openai).
    #[arg(long, default_value = "gpt-5-mini")]
    pub openai_model: String,

    /// OpenAI API base URL (used when engine=openai).
    #[arg(long, default_value = "https://api.openai.com/v1")]
    pub openai_base_url: String,

    /// OpenAI temperature (used when engine=openai; ignored for `gpt-5*` models).
    #[arg(long, default_value_t = 0.0)]
    pub openai_temperature: f32,

    /// Overwrite output file if it already exists.
    #[arg(long, default_value_t = false)]
    pub force: bool,
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

#[derive(Debug, Subcommand)]
pub enum LlmCommand {
    RewritePages(LlmRewritePagesArgs),
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum LlmEngine {
    /// Do nothing (copy input to output).
    Noop,

    /// Invoke an external command as a filter (stdin -> stdout).
    Command,

    /// Translate via OpenAI API.
    Openai,
}

#[derive(Debug, Args)]
pub struct LlmRewritePagesArgs {
    /// Input path to `toc.yaml` (only referenced page ids are rewritten).
    #[arg(long)]
    pub toc: String,

    /// Input path to `manifest.jsonl`.
    #[arg(long)]
    pub manifest: String,

    /// Output directory for rewritten pages (writes `<OUT>/pages/*.md`).
    #[arg(long)]
    pub out: String,

    /// Rewrite prompt (free-form).
    #[arg(long)]
    pub prompt: String,

    /// Rewrite engine.
    #[arg(long, value_enum, default_value_t = LlmEngine::Openai)]
    pub engine: LlmEngine,

    /// Rewrite command (required when engine=command).
    #[arg(long, value_name = "PROGRAM")]
    pub command: Option<String>,

    /// Rewrite command arguments (use `--` before the args).
    #[arg(trailing_var_arg = true)]
    pub command_args: Vec<String>,

    /// OpenAI model (used when engine=openai).
    #[arg(long, default_value = "gpt-5-mini")]
    pub openai_model: String,

    /// OpenAI API base URL (used when engine=openai).
    #[arg(long, default_value = "https://api.openai.com/v1")]
    pub openai_base_url: String,

    /// Maximum characters per OpenAI request (used when engine=openai).
    #[arg(long, default_value_t = 12_000)]
    pub openai_max_chars: usize,

    /// OpenAI temperature (used when engine=openai; ignored for `gpt-5*` models).
    #[arg(long, default_value_t = 0.0)]
    pub openai_temperature: f32,

    /// Maximum concurrent OpenAI requests (used when engine=openai).
    #[arg(long, default_value_t = 1)]
    pub openai_concurrency: usize,

    /// Retries per OpenAI chunk when placeholder tokens are modified (used when engine=openai).
    #[arg(long, default_value_t = 1)]
    pub openai_retries: usize,

    /// Allow rewritten output even if placeholder tokens are missing.
    ///
    /// When enabled, the tool will keep the LLM output and restore only the placeholder tokens
    /// that remain. This can drop code/URLs if the model removed them.
    #[arg(long, default_value_t = false)]
    pub allow_missing_tokens: bool,

    /// Overwrite output file if it already exists.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}
