use std::path::{Path, PathBuf};

use anyhow::Context as _;
use async_trait::async_trait;
use tokio::fs;

use crate::app::model::{Job, StartJobRequest};

#[async_trait]
pub trait JobStore: Send + Sync {
    async fn create(&self, job: &Job, request: &StartJobRequest) -> anyhow::Result<()>;
    async fn get(&self, job_id: &str) -> anyhow::Result<Option<Job>>;
    async fn get_request(&self, job_id: &str) -> anyhow::Result<Option<StartJobRequest>>;
    async fn put(&self, job: &Job) -> anyhow::Result<()>;
}

#[derive(Debug, Clone)]
pub struct LocalFsJobStore {
    base_dir: PathBuf,
}

impl LocalFsJobStore {
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

    fn job_json_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("job.json")
    }

    fn request_json_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("request.json")
    }
}

#[async_trait]
impl JobStore for LocalFsJobStore {
    async fn create(&self, job: &Job, request: &StartJobRequest) -> anyhow::Result<()> {
        fs::create_dir_all(self.job_dir(&job.job_id))
            .await
            .with_context(|| format!("create job dir: {}", self.job_dir(&job.job_id).display()))?;

        write_json_atomic(&self.job_json_path(&job.job_id), job)
            .await
            .context("write job.json")?;
        write_json_atomic(&self.request_json_path(&job.job_id), request)
            .await
            .context("write request.json")?;

        Ok(())
    }

    async fn get(&self, job_id: &str) -> anyhow::Result<Option<Job>> {
        let path = self.job_json_path(job_id);
        read_json(&path)
            .await
            .with_context(|| format!("read: {}", path.display()))
    }

    async fn get_request(&self, job_id: &str) -> anyhow::Result<Option<StartJobRequest>> {
        let path = self.request_json_path(job_id);
        read_json(&path)
            .await
            .with_context(|| format!("read: {}", path.display()))
    }

    async fn put(&self, job: &Job) -> anyhow::Result<()> {
        fs::create_dir_all(self.job_dir(&job.job_id))
            .await
            .with_context(|| format!("create job dir: {}", self.job_dir(&job.job_id).display()))?;
        write_json_atomic(&self.job_json_path(&job.job_id), job)
            .await
            .context("write job.json")?;
        Ok(())
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let value = serde_json::from_slice(&bytes).context("parse json")?;
    Ok(Some(value))
}

async fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .await
        .with_context(|| format!("create parent dir: {}", parent.display()))?;

    let tmp_path = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4().simple()));
    let data = serde_json::to_vec_pretty(value).context("serialize json")?;
    fs::write(&tmp_path, &data)
        .await
        .with_context(|| format!("write tmp: {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .await
        .with_context(|| format!("rename tmp to final: {}", path.display()))?;
    Ok(())
}
