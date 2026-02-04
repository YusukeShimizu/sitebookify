use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use async_trait::async_trait;
use base64::Engine as _;
use sha2::Digest as _;
use tokio::fs;

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    fn artifact_path(&self, job_id: &str) -> PathBuf;
    fn artifact_uri(&self, job_id: &str) -> String;
    async fn create_zip_from_workspace(
        &self,
        job_id: &str,
        workspace_dir: &Path,
    ) -> anyhow::Result<PathBuf>;

    async fn generate_download_url(&self, job_id: &str, ttl_secs: u32) -> anyhow::Result<String>;
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

    fn artifact_uri(&self, job_id: &str) -> String {
        let path = self.artifact_path(job_id);
        format!("file://{}", path.display())
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

    async fn generate_download_url(&self, job_id: &str, _ttl_secs: u32) -> anyhow::Result<String> {
        Ok(format!("/artifacts/{job_id}"))
    }
}

#[derive(Debug, Clone)]
pub struct GcsArtifactStore {
    base_dir: PathBuf,
    bucket: String,
    client: reqwest::Client,
}

impl GcsArtifactStore {
    pub fn new(base_dir: impl Into<PathBuf>, bucket: impl Into<String>) -> Self {
        Self {
            base_dir: base_dir.into(),
            bucket: bucket.into(),
            client: reqwest::Client::new(),
        }
    }

    fn jobs_dir(&self) -> PathBuf {
        self.base_dir.join("jobs")
    }

    fn job_dir(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(job_id)
    }

    fn object_name(&self, job_id: &str) -> String {
        format!("jobs/{job_id}/artifact.zip")
    }

    async fn access_token(&self) -> anyhow::Result<String> {
        #[derive(Debug, serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
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
        Ok(token.access_token)
    }

    async fn service_account_email(&self) -> anyhow::Result<String> {
        let url = "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/email";
        let resp = self
            .client
            .get(url)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .context("request metadata service account email")?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "metadata service account email request failed ({})",
                resp.status()
            );
        }
        let text = resp.text().await.context("read metadata email response")?;
        Ok(text.trim().to_string())
    }

    async fn sign_blob(
        &self,
        access_token: &str,
        service_account_email: &str,
        blob: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        #[derive(Debug, serde::Serialize)]
        struct SignBlobRequest<'a> {
            payload: &'a str,
        }

        #[derive(Debug, serde::Deserialize)]
        struct SignBlobResponse {
            #[serde(rename = "signedBlob")]
            signed_blob: String,
        }

        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(blob);
        let url = format!(
            "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/{service_account_email}:signBlob"
        );
        let resp = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .json(&SignBlobRequest {
                payload: payload_b64.as_str(),
            })
            .send()
            .await
            .context("request iamcredentials signBlob")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("iamcredentials signBlob failed ({status}): {body}");
        }
        let result: SignBlobResponse = resp.json().await.context("parse signBlob json")?;
        let signature = base64::engine::general_purpose::STANDARD
            .decode(result.signed_blob)
            .context("decode signedBlob base64")?;
        Ok(signature)
    }

    async fn upload_zip(&self, object_name: &str, local_zip_path: &Path) -> anyhow::Result<()> {
        let access_token = self.access_token().await.context("get access token")?;
        let object_name_encoded = percent_encode_rfc3986(object_name);
        let url = format!(
            "https://storage.googleapis.com/upload/storage/v1/b/{bucket}/o?uploadType=media&name={object_name_encoded}",
            bucket = self.bucket
        );

        let bytes = tokio::fs::read(local_zip_path)
            .await
            .with_context(|| format!("read zip: {}", local_zip_path.display()))?;
        let resp = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .header(reqwest::header::CONTENT_TYPE, "application/zip")
            .body(bytes)
            .send()
            .await
            .context("upload artifact to gcs")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("gcs upload failed ({status}): {body}");
        }
        Ok(())
    }

    async fn signed_download_url(
        &self,
        service_account_email: &str,
        object_name: &str,
        ttl_secs: u32,
        now: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<String> {
        let access_token = self.access_token().await.context("get access token")?;

        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let datestamp = now.format("%Y%m%d").to_string();

        let canonical_uri = format!(
            "/{}/{}",
            percent_encode_rfc3986(&self.bucket),
            percent_encode_path(object_name)
        );

        let credential_scope = format!("{datestamp}/auto/storage/goog4_request");
        let credential = format!("{service_account_email}/{credential_scope}");

        let mut query_params = [
            ("X-Goog-Algorithm", "GOOG4-RSA-SHA256".to_string()),
            ("X-Goog-Credential", credential),
            ("X-Goog-Date", timestamp.clone()),
            ("X-Goog-Expires", ttl_secs.to_string()),
            ("X-Goog-SignedHeaders", "host".to_string()),
        ];
        query_params.sort_by(|(a_name, a_value), (b_name, b_value)| {
            a_name.cmp(b_name).then_with(|| a_value.cmp(b_value))
        });
        let canonical_query = query_params
            .iter()
            .map(|(name, value)| {
                format!(
                    "{}={}",
                    percent_encode_rfc3986(name),
                    percent_encode_rfc3986(value)
                )
            })
            .collect::<Vec<_>>()
            .join("&");

        let canonical_headers = "host:storage.googleapis.com\n";
        let signed_headers = "host";
        let hashed_payload = "UNSIGNED-PAYLOAD";

        let canonical_request = format!(
            "GET\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{hashed_payload}"
        );
        let canonical_request_hash = sha256_hex(&canonical_request);

        let string_to_sign =
            format!("GOOG4-RSA-SHA256\n{timestamp}\n{credential_scope}\n{canonical_request_hash}");
        let signature = self
            .sign_blob(
                &access_token,
                service_account_email,
                string_to_sign.as_bytes(),
            )
            .await
            .context("sign string_to_sign")?;
        let signature_hex = hex::encode(signature);

        Ok(format!(
            "https://storage.googleapis.com{canonical_uri}?{canonical_query}&X-Goog-Signature={signature_hex}"
        ))
    }
}

#[async_trait]
impl ArtifactStore for GcsArtifactStore {
    fn artifact_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("artifact.zip")
    }

    fn artifact_uri(&self, job_id: &str) -> String {
        format!("gs://{}/{}", self.bucket, self.object_name(job_id))
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

        let object_name = self.object_name(job_id);
        tracing::info!(
            bucket = %self.bucket,
            object = %object_name,
            path = %artifact_path.display(),
            "uploading artifact to gcs"
        );
        self.upload_zip(&object_name, &artifact_path)
            .await
            .context("upload zip")?;

        if let Err(err) = tokio::fs::remove_file(&artifact_path).await {
            tracing::warn!(path = %artifact_path.display(), ?err, "failed to remove local artifact zip after upload");
        }

        Ok(artifact_path)
    }

    async fn generate_download_url(&self, job_id: &str, ttl_secs: u32) -> anyhow::Result<String> {
        let service_account_email = self
            .service_account_email()
            .await
            .context("get service account email")?;
        let object_name = self.object_name(job_id);
        self.signed_download_url(
            &service_account_email,
            &object_name,
            ttl_secs,
            chrono::Utc::now(),
        )
        .await
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

fn sha256_hex(input: &str) -> String {
    let digest = sha2::Sha256::digest(input.as_bytes());
    hex::encode(digest)
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

fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_rfc3986)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_rfc3986_encodes_reserved_chars() {
        assert_eq!(percent_encode_rfc3986("a b"), "a%20b");
        assert_eq!(percent_encode_rfc3986("a/b"), "a%2Fb");
        assert_eq!(percent_encode_rfc3986("me@example.com"), "me%40example.com");
        assert_eq!(percent_encode_rfc3986("a+b"), "a%2Bb");
        assert_eq!(percent_encode_rfc3986("~"), "~");
    }

    #[test]
    fn percent_encode_path_preserves_slash_separators() {
        assert_eq!(percent_encode_path("a/b c"), "a/b%20c");
    }

    #[tokio::test]
    async fn local_fs_download_url_is_app_route() -> anyhow::Result<()> {
        let store = LocalFsArtifactStore::new("data");
        let url = store.generate_download_url("job-123", 3600).await?;
        assert_eq!(url, "/artifacts/job-123");
        Ok(())
    }

    #[test]
    fn local_fs_artifact_uri_is_file_scheme() {
        let store = LocalFsArtifactStore::new("data");
        let expected = format!("file://{}", store.artifact_path("job-123").display());
        assert_eq!(store.artifact_uri("job-123"), expected);
    }
}
