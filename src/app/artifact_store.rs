use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use async_trait::async_trait;
use tokio::fs;

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    fn artifact_path(&self, job_id: &str) -> PathBuf;
    async fn create_zip_from_workspace(
        &self,
        job_id: &str,
        workspace_dir: &Path,
    ) -> anyhow::Result<PathBuf>;
}

#[derive(Debug, Clone)]
pub struct LocalFsArtifactStore {
    base_dir: PathBuf,
}

impl LocalFsArtifactStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn jobs_dir(&self) -> PathBuf {
        self.base_dir.join("jobs")
    }

    fn job_dir(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(job_id)
    }
}

#[async_trait]
impl ArtifactStore for LocalFsArtifactStore {
    fn artifact_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("artifact.zip")
    }

    async fn create_zip_from_workspace(
        &self,
        job_id: &str,
        workspace_dir: &Path,
    ) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(self.job_dir(job_id))
            .await
            .with_context(|| format!("create job dir: {}", self.job_dir(job_id).display()))?;

        let artifact_path = self.artifact_path(job_id);
        let workspace_dir = workspace_dir.to_path_buf();
        let artifact_path_for_blocking = artifact_path.clone();

        tokio::task::spawn_blocking(move || {
            create_zip_from_workspace_blocking(&workspace_dir, &artifact_path_for_blocking)
        })
        .await
        .context("join zip task")??;

        Ok(artifact_path)
    }
}

fn create_zip_from_workspace_blocking(workspace_dir: &Path, out_zip: &Path) -> anyhow::Result<()> {
    let book_md_path = workspace_dir.join("book.md");
    if !book_md_path.exists() {
        anyhow::bail!("missing book.md: {}", book_md_path.display());
    }

    let file =
        File::create(out_zip).with_context(|| format!("create zip: {}", out_zip.display()))?;
    let mut zip = zip::ZipWriter::new(file);

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    zip.start_file("book.md", options)
        .context("zip start_file book.md")?;
    let mut book_md = File::open(&book_md_path)
        .with_context(|| format!("open book.md: {}", book_md_path.display()))?;
    io::copy(&mut book_md, &mut zip).context("zip write book.md")?;

    let assets_dir = workspace_dir.join("assets");
    if assets_dir.exists() {
        add_dir_recursive(&mut zip, &assets_dir, Path::new("assets"), options)
            .context("zip add assets")?;
    }

    zip.finish().context("zip finish")?;
    Ok(())
}

fn add_dir_recursive<W: io::Write + io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &Path,
    zip_prefix: &Path,
    options: zip::write::SimpleFileOptions,
) -> anyhow::Result<()> {
    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("read dir: {}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("list dir: {}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let zip_path = zip_prefix.join(name.to_string_lossy().as_ref());

        let file_type = entry.file_type().context("read file type")?;
        if file_type.is_dir() {
            // Ensure the directory entry exists in the zip.
            zip.add_directory(zip_path.to_string_lossy(), options)
                .with_context(|| format!("zip add_directory: {}", zip_path.display()))?;
            add_dir_recursive(zip, &path, &zip_path, options)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        zip.start_file(zip_path.to_string_lossy(), options)
            .with_context(|| format!("zip start_file: {}", zip_path.display()))?;
        let mut f = File::open(&path).with_context(|| format!("open: {}", path.display()))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)
            .with_context(|| format!("read: {}", path.display()))?;
        zip.write_all(&buf)
            .with_context(|| format!("zip write: {}", zip_path.display()))?;
    }

    Ok(())
}
