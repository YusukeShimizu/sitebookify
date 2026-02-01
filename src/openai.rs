use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::Context as _;

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    pub bin: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

impl OpenAiConfig {
    pub fn from_env() -> Self {
        let bin = std::env::var("SITEBOOKIFY_OPENAI_BIN").unwrap_or_else(|_| "openai".to_owned());
        let model = std::env::var("SITEBOOKIFY_OPENAI_MODEL").ok();
        let reasoning_effort = std::env::var("SITEBOOKIFY_OPENAI_REASONING_EFFORT").ok();
        Self {
            bin,
            model,
            reasoning_effort,
        }
    }
}

pub fn exec_readonly(prompt: &str, config: &OpenAiConfig) -> anyhow::Result<String> {
    let output = tempfile::NamedTempFile::new().context("create openai output temp file")?;
    let output_path = output.path();

    let mut cmd = Command::new(&config.bin);
    if let Some(model) = config.model.as_deref() {
        cmd.args(["--model", model]);
    }
    if let Some(reasoning_effort) = config.reasoning_effort.as_deref() {
        let reasoning_effort_arg = format!("model_reasoning_effort=\"{reasoning_effort}\"");
        cmd.args(["--config", &reasoning_effort_arg]);
    }
    cmd.args([
        "exec",
        "-",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--color",
        "never",
        "--output-last-message",
    ]);
    cmd.arg(output_path);

    tracing::info!(
        bin = %config.bin,
        model = ?config.model,
        reasoning_effort = ?config.reasoning_effort,
        "openai cli exec"
    );

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn openai cli: {}", config.bin))?;

    {
        let mut stdin = child.stdin.take().context("open openai stdin")?;
        stdin
            .write_all(prompt.as_bytes())
            .context("write openai stdin")?;
    }

    let status = child.wait().context("wait openai")?;
    if !status.success() {
        anyhow::bail!("openai cli failed ({status})");
    }

    std::fs::read_to_string(output_path).context("read openai last message")
}
