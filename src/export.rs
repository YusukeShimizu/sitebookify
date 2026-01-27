use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;

use crate::cli::{ExportArgs, ExportFormat};

pub fn run(args: ExportArgs) -> anyhow::Result<()> {
    if std::path::Path::new(&args.out).exists() && !args.force {
        anyhow::bail!("export output already exists: {}", args.out);
    }
    if let Some(parent) = std::path::Path::new(&args.out).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create export output dir: {}", parent.display()))?;
    }

    match args.format {
        ExportFormat::Md => export_md(&args.input, &args.out, args.force)?,
        ExportFormat::Epub => export_via_pandoc(&args, "epub")?,
        ExportFormat::Pdf => export_pdf_via_pandoc(&args)?,
    }

    Ok(())
}

fn export_md(input: &str, out: &str, force: bool) -> anyhow::Result<()> {
    let contents =
        std::fs::read_to_string(input).with_context(|| format!("read input: {input}"))?;
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(out)
        .with_context(|| format!("open output: {out}"))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("write output: {out}"))?;
    file.flush()
        .with_context(|| format!("flush output: {out}"))?;
    Ok(())
}

fn export_via_pandoc(args: &ExportArgs, to: &str) -> anyhow::Result<()> {
    tracing::info!(
        format = to,
        pandoc = %args.pandoc,
        out = %args.out,
        "export via pandoc"
    );

    let pandoc_args = build_pandoc_args(args, to, None, None);
    let output = run_pandoc(args, &pandoc_args)?;
    if !output.status.success() {
        anyhow::bail!(
            "pandoc failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn export_pdf_via_pandoc(args: &ExportArgs) -> anyhow::Result<()> {
    let engines = match args.pdf_engine.as_deref() {
        Some(engine) => vec![engine],
        None => vec!["weasyprint", "tectonic"],
    };

    let mut last_failure: Option<anyhow::Error> = None;
    for engine in engines {
        tracing::info!(
            format = "pdf",
            pdf_engine = engine,
            pandoc = %args.pandoc,
            out = %args.out,
            "export via pandoc"
        );

        let pandoc_args =
            build_pandoc_args(args, "pdf", Some(engine), Some("gfm-tex_math_dollars"));
        let output = run_pandoc(args, &pandoc_args)?;
        if output.status.success() {
            return Ok(());
        }

        last_failure = Some(anyhow::anyhow!(
            "pandoc failed with pdf_engine={engine} ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    match last_failure {
        Some(err) => Err(err),
        None => anyhow::bail!("export pdf failed: no pdf engine candidates"),
    }
}

fn build_pandoc_args(
    args: &ExportArgs,
    to: &str,
    pdf_engine: Option<&str>,
    from: Option<&str>,
) -> Vec<OsString> {
    let mut pandoc_args = Vec::new();
    pandoc_args.push(OsString::from(&args.input));
    pandoc_args.push(OsString::from("-o"));
    pandoc_args.push(OsString::from(&args.out));

    if let Some(parent) = Path::new(&args.input).parent()
        && !parent.as_os_str().is_empty()
    {
        pandoc_args.push(OsString::from("--resource-path"));
        pandoc_args.push(parent.as_os_str().to_owned());
    }

    pandoc_args.push(OsString::from("--to"));
    pandoc_args.push(OsString::from(to));

    if let Some(from) = from {
        pandoc_args.push(OsString::from("--from"));
        pandoc_args.push(OsString::from(from));
    }

    if let Some(engine) = pdf_engine {
        pandoc_args.push(OsString::from("--pdf-engine"));
        pandoc_args.push(OsString::from(engine));
    }

    if let Some(title) = &args.title {
        pandoc_args.push(OsString::from("--metadata"));
        pandoc_args.push(OsString::from(format!("title={title}")));
    }

    pandoc_args
}

fn run_pandoc(args: &ExportArgs, pandoc_args: &[OsString]) -> anyhow::Result<std::process::Output> {
    match Command::new(&args.pandoc).args(pandoc_args).output() {
        Ok(output) => Ok(output),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            tracing::warn!(
                pandoc = %args.pandoc,
                "pandoc not found; trying `nix develop -c pandoc` fallback"
            );
            run_pandoc_via_nix(pandoc_args).with_context(|| format!("run pandoc: {}", args.pandoc))
        }
        Err(err) => Err(err).with_context(|| format!("run pandoc: {}", args.pandoc)),
    }
}

fn run_pandoc_via_nix(pandoc_args: &[OsString]) -> anyhow::Result<std::process::Output> {
    let Some(nix) = find_nix_executable() else {
        anyhow::bail!(
            "pandoc is not installed and `nix` was not found; install pandoc or run in a Nix devShell (`nix develop`)"
        );
    };
    let Some(flake_dir) = find_flake_dir()? else {
        anyhow::bail!(
            "pandoc is not installed and `flake.nix` was not found; install pandoc or pass `--pandoc <PATH>`"
        );
    };

    let mut cmd = Command::new(nix);
    cmd.arg("develop")
        .arg(flake_dir)
        .arg("-c")
        .arg("pandoc")
        .args(pandoc_args);

    cmd.output()
        .context("run pandoc via `nix develop -c pandoc`")
}

fn find_nix_executable() -> Option<PathBuf> {
    if Command::new("nix").arg("--version").output().is_ok() {
        return Some(PathBuf::from("nix"));
    }

    let nix_profile = PathBuf::from("/nix/var/nix/profiles/default/bin/nix");
    if nix_profile.exists() {
        return Some(nix_profile);
    }

    None
}

fn find_flake_dir() -> anyhow::Result<Option<PathBuf>> {
    let Ok(mut dir) = std::env::current_dir() else {
        return Ok(None);
    };

    loop {
        if dir.join("flake.nix").exists() {
            return Ok(Some(dir));
        }

        let Some(parent) = dir.parent() else {
            return Ok(None);
        };
        dir = parent.to_path_buf();
    }
}
