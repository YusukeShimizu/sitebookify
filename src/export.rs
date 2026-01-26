use std::fs::OpenOptions;
use std::io::Write as _;
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

    let mut cmd = Command::new(&args.pandoc);
    cmd.arg(&args.input).arg("-o").arg(&args.out);
    cmd.arg("--to").arg(to);
    if let Some(title) = &args.title {
        cmd.arg("--metadata").arg(format!("title={title}"));
    }

    let output = cmd
        .output()
        .with_context(|| format!("run pandoc: {}", args.pandoc))?;
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
    let engine = args.pdf_engine.as_deref().unwrap_or("tectonic");

    tracing::info!(
        format = "pdf",
        pdf_engine = engine,
        pandoc = %args.pandoc,
        out = %args.out,
        "export via pandoc"
    );

    let mut cmd = Command::new(&args.pandoc);
    cmd.arg(&args.input).arg("-o").arg(&args.out);
    cmd.arg("--to").arg("pdf");
    cmd.arg("--pdf-engine").arg(engine);
    if let Some(title) = &args.title {
        cmd.arg("--metadata").arg(format!("title={title}"));
    }

    let output = cmd
        .output()
        .with_context(|| format!("run pandoc: {}", args.pandoc))?;
    if !output.status.success() {
        anyhow::bail!(
            "pandoc failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
