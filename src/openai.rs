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
        let bin = std::env::var("SITEBOOKIFY_OPENAI_BIN").unwrap_or_else(|_| default_openai_bin());
        let model = std::env::var("SITEBOOKIFY_OPENAI_MODEL").ok();
        let reasoning_effort = std::env::var("SITEBOOKIFY_OPENAI_REASONING_EFFORT").ok();
        Self {
            bin,
            model,
            reasoning_effort,
        }
    }
}

fn default_openai_bin() -> String {
    // `codex` is the current name; `openai` was used by earlier releases.
    if bin_exists_in_path("codex") {
        return "codex".to_owned();
    }
    if bin_exists_in_path("openai") {
        return "openai".to_owned();
    }
    "codex".to_owned()
}

fn bin_exists_in_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        if is_executable(&dir.join(bin)) {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
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

    let mut child = match cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "OpenAI engine requires the `codex` CLI (preferred) or legacy `openai` CLI, but it was not found: `{}`\n\
\n\
Fix:\n\
- Install Codex CLI and ensure it's in PATH, or\n\
- Set SITEBOOKIFY_OPENAI_BIN to the full path of the CLI binary (e.g. `codex`), or\n\
- Use `--engine noop` (or select noop in the Web UI).",
                config.bin
            )
        }
        Err(err) => return Err(err).with_context(|| format!("spawn openai cli: {}", config.bin)),
    };

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
