use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use async_trait::async_trait;
use reqwest::StatusCode;
use tokio::fs;
use tokio::sync::RwLock;

use crate::app::model::{Job, StartJobRequest};

#[async_trait]
pub trait JobStore: Send + Sync {
    async fn create(&self, job: &Job, request: &StartJobRequest) -> anyhow::Result<()>;
    async fn get(&self, job_id: &str) -> anyhow::Result<Option<Job>>;
    async fn get_request(&self, job_id: &str) -> anyhow::Result<Option<StartJobRequest>>;
    async fn put(&self, job: &Job) -> anyhow::Result<()>;
    async fn list_job_ids(&self) -> anyhow::Result<Vec<String>>;
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

    async fn list_job_ids(&self) -> anyhow::Result<Vec<String>> {
        let jobs_dir = self.jobs_dir();
        let mut entries = match fs::read_dir(&jobs_dir).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(err).with_context(|| format!("read dir: {}", jobs_dir.display()));
            }
        };

        let mut ids = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("iterate dir: {}", jobs_dir.display()))?
        {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                ids.push(name.to_string());
            }
        }

        ids.sort();
        Ok(ids)
    }
}

#[derive(Debug, Clone)]
pub struct GcsJobStore {
    bucket: String,
    client: reqwest::Client,
    access_token_cache: Arc<RwLock<Option<CachedAccessToken>>>,
}

#[derive(Debug, Clone)]
struct CachedAccessToken {
    token: String,
    expires_at: Instant,
}

impl CachedAccessToken {
    fn is_valid(&self, now: Instant) -> bool {
        self.expires_at > now
    }
}

impl GcsJobStore {
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            client: reqwest::Client::new(),
            access_token_cache: Arc::new(RwLock::new(None)),
        }
    }

    fn job_json_object(&self, job_id: &str) -> String {
        format!("jobs/{job_id}/job.json")
    }

    fn request_json_object(&self, job_id: &str) -> String {
        format!("jobs/{job_id}/request.json")
    }

    async fn access_token(&self) -> anyhow::Result<String> {
        #[derive(Debug, serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            #[serde(default)]
            expires_in: u64,
        }

        let now = Instant::now();
        if let Some(cached) = self.access_token_cache.read().await.as_ref()
            && cached.is_valid(now)
        {
            return Ok(cached.token.clone());
        }

        let mut cache = self.access_token_cache.write().await;
        let now = Instant::now();
        if let Some(cached) = cache.as_ref()
            && cached.is_valid(now)
        {
            return Ok(cached.token.clone());
        }

        let url = "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
        let resp = self
            .client
            .get(url)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .context("request metadata access token")?;
        if !resp.status().is_success() {
            anyhow::bail!("metadata token request failed ({})", resp.status());
        }
        let token: TokenResponse = resp.json().await.context("parse metadata token json")?;
        let ttl = token.expires_in.max(60);
        let refresh_in = ttl.saturating_sub(30).max(1);
        *cache = Some(CachedAccessToken {
            token: token.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(refresh_in),
        });
        Ok(token.access_token)
    }

    async fn upload_json<T: serde::Serialize>(
        &self,
        object_name: &str,
        value: &T,
    ) -> anyhow::Result<()> {
        let access_token = self.access_token().await.context("get access token")?;
        let url = format!(
            "https://storage.googleapis.com/upload/storage/v1/b/{bucket}/o",
            bucket = self.bucket
        );
        let body = serde_json::to_vec_pretty(value).context("serialize json")?;
        let resp = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .query(&[("uploadType", "media"), ("name", object_name)])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .with_context(|| format!("upload object: gs://{}/{}", self.bucket, object_name))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("gcs upload failed ({status}): {body}");
        }
        Ok(())
    }

    async fn download_json<T: serde::de::DeserializeOwned>(
        &self,
        object_name: &str,
    ) -> anyhow::Result<Option<T>> {
        let access_token = self.access_token().await.context("get access token")?;
        let object_name_encoded = percent_encode_rfc3986(object_name);
        let url = format!(
            "https://storage.googleapis.com/storage/v1/b/{bucket}/o/{object_name_encoded}?alt=media",
            bucket = self.bucket
        );
        let resp = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .with_context(|| format!("download object: gs://{}/{}", self.bucket, object_name))?;

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("gcs download failed ({status}): {body}");
        }

        let bytes = resp.bytes().await.context("read gcs response body")?;
        let value = serde_json::from_slice::<T>(&bytes).context("parse json")?;
        Ok(Some(value))
    }
}

#[async_trait]
impl JobStore for GcsJobStore {
    async fn create(&self, job: &Job, request: &StartJobRequest) -> anyhow::Result<()> {
        self.upload_json(&self.job_json_object(&job.job_id), job)
            .await
            .context("upload job.json")?;
        self.upload_json(&self.request_json_object(&job.job_id), request)
            .await
            .context("upload request.json")?;
        Ok(())
    }

    async fn get(&self, job_id: &str) -> anyhow::Result<Option<Job>> {
        self.download_json(&self.job_json_object(job_id))
            .await
            .context("download job.json")
    }

    async fn get_request(&self, job_id: &str) -> anyhow::Result<Option<StartJobRequest>> {
        self.download_json(&self.request_json_object(job_id))
            .await
            .context("download request.json")
    }

    async fn put(&self, job: &Job) -> anyhow::Result<()> {
        self.upload_json(&self.job_json_object(&job.job_id), job)
            .await
            .context("upload job.json")?;
        Ok(())
    }

    async fn list_job_ids(&self) -> anyhow::Result<Vec<String>> {
        #[derive(Debug, serde::Deserialize)]
        struct ObjectItem {
            name: String,
        }

        #[derive(Debug, serde::Deserialize)]
        struct ListResponse {
            #[serde(default)]
            items: Vec<ObjectItem>,
            #[serde(rename = "nextPageToken")]
            next_page_token: Option<String>,
        }

        let access_token = self.access_token().await.context("get access token")?;
        let mut page_token: Option<String> = None;
        let mut ids: BTreeSet<String> = BTreeSet::new();

        loop {
            let url = format!(
                "https://storage.googleapis.com/storage/v1/b/{bucket}/o",
                bucket = self.bucket
            );
            let mut req = self
                .client
                .get(url)
                .bearer_auth(&access_token)
                .query(&[("prefix", "jobs/"), ("fields", "items/name,nextPageToken")]);
            if let Some(token) = &page_token {
                req = req.query(&[("pageToken", token)]);
            }

            let resp = req.send().await.context("list gcs objects for jobs")?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("gcs list objects failed ({status}): {body}");
            }

            let page: ListResponse = resp.json().await.context("parse gcs list response")?;
            for item in page.items {
                if !item.name.ends_with("/job.json") {
                    continue;
                }
                let Some(stripped) = item.name.strip_prefix("jobs/") else {
                    continue;
                };
                let Some(job_id) = stripped.strip_suffix("/job.json") else {
                    continue;
                };
                if !job_id.is_empty() {
                    ids.insert(job_id.to_string());
                }
            }

            match page.next_page_token {
                Some(token) if !token.is_empty() => {
                    page_token = Some(token);
                }
                _ => break,
            }
        }

        Ok(ids.into_iter().collect())
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

fn percent_encode_rfc3986(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let is_unreserved = matches!(
            b,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
        );
        if is_unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
