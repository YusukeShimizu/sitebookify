use std::path::{Path, PathBuf};

use anyhow::Context as _;
use url::Url;

pub fn ensure_raw_snapshot_dir_does_not_exist(out_dir: &Path) -> anyhow::Result<()> {
    if out_dir.exists() {
        anyhow::bail!(
            "raw snapshot output directory already exists: {}",
            out_dir.display()
        );
    }
    Ok(())
}

pub fn raw_html_path(out_dir: &Path, url: &Url) -> anyhow::Result<PathBuf> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have host: {url}"))?;

    let host_segment = match url.port() {
        Some(port) => format!("{host}_{port}"),
        None => host.to_owned(),
    };

    let mut path = out_dir.join("html").join(host_segment);
    for segment in url.path().split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            anyhow::bail!("url path must not contain '..': {url}");
        }
        path = path.join(segment);
    }
    path = path.join("index.html");

    Ok(path)
}

pub fn write_raw_html(path: &Path, html: &str) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!("raw html output already exists: {}", path.display());
    }

    let parent_dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("raw html path must have parent: {}", path.display()))?;
    std::fs::create_dir_all(parent_dir)
        .with_context(|| format!("create raw html parent dir: {}", parent_dir.display()))?;

    std::fs::write(path, html).with_context(|| format!("write raw html: {}", path.display()))?;

    Ok(())
}
