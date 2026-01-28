use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::Context as _;

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub bin: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

impl CodexConfig {
    pub fn from_env() -> Self {
        let bin = std::env::var("SITEBOOKIFY_CODEX_BIN").unwrap_or_else(|_| "codex".to_owned());
        let model = std::env::var("SITEBOOKIFY_CODEX_MODEL").ok();
        let reasoning_effort = std::env::var("SITEBOOKIFY_CODEX_REASONING_EFFORT").ok();
        Self {
            bin,
            model,
            reasoning_effort,
        }
    }
}

pub fn exec_readonly(prompt: &str, config: &CodexConfig) -> anyhow::Result<String> {
    let output = tempfile::NamedTempFile::new().context("create codex output temp file")?;
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
        "codex exec"
    );

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn codex: {}", config.bin))?;

    {
        let mut stdin = child.stdin.take().context("open codex stdin")?;
        stdin
            .write_all(prompt.as_bytes())
            .context("write codex stdin")?;
    }

    let status = child.wait().context("wait codex")?;
    if !status.success() {
        anyhow::bail!("codex failed ({status})");
    }

    std::fs::read_to_string(output_path).context("read codex last message")
}
