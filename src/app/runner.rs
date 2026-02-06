use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use chrono::Utc;

use crate::app::artifact_store::ArtifactStore;
use crate::app::job_store::JobStore;
use crate::app::model::{Job, JobStatus, StartJobRequest};
use crate::cli::{BookBundleArgs, BookInitArgs, BookRenderArgs, TocCreateArgs};
use crate::formats::Toc;
use crate::llm_crawl::LlmCrawlArgs;

pub struct JobRunner {
    job_store: Arc<dyn JobStore>,
    artifact_store: Arc<dyn ArtifactStore>,
}

impl JobRunner {
    pub fn new(job_store: Arc<dyn JobStore>, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        Self {
            job_store,
            artifact_store,
        }
    }

    pub async fn run_job(&self, job_id: &str) {
        if let Err(err) = self.try_run_job(job_id).await {
            tracing::error!(job_id, ?err, "job failed");
            let _ = self.mark_error(job_id, format!("{err:#}")).await;
        }
    }

    async fn try_run_job(&self, job_id: &str) -> anyhow::Result<()> {
        let mut job = self
            .job_store
            .get(job_id)
            .await
            .context("load job")?
            .ok_or_else(|| anyhow::anyhow!("job not found: {job_id}"))?;
        let request = self
            .job_store
            .get_request(job_id)
            .await
            .context("load request")?
            .ok_or_else(|| anyhow::anyhow!("request not found: {job_id}"))?;

        self.mark_running(&mut job).await.context("mark running")?;
        self.run_pipeline(&mut job, &request).await?;

        let artifact_path = self
            .artifact_store
            .create_zip_from_workspace(job_id, &job.work_dir)
            .await
            .context("create artifact zip")?;

        job.status = JobStatus::Done;
        job.progress_percent = 100;
        job.message = "done".to_string();
        job.finished_at = Some(Utc::now());
        job.artifact_path = Some(artifact_path);
        job.artifact_uri = Some(self.artifact_store.artifact_uri(job_id));

        self.job_store.put(&job).await.context("save job")?;
        Ok(())
    }

    async fn mark_running(&self, job: &mut Job) -> anyhow::Result<()> {
        job.status = JobStatus::Running;
        job.started_at = Some(Utc::now());
        job.progress_percent = 0;
        job.message = "starting".to_string();
        self.job_store.put(job).await.context("save job")?;
        Ok(())
    }

    async fn mark_error(&self, job_id: &str, message: String) -> anyhow::Result<()> {
        let Some(mut job) = self.job_store.get(job_id).await? else {
            return Ok(());
        };
        job.status = JobStatus::Error;
        job.message = message;
        job.finished_at = Some(Utc::now());
        self.job_store.put(&job).await?;
        Ok(())
    }

    async fn update_progress(
        &self,
        job: &mut Job,
        percent: u32,
        message: &str,
    ) -> anyhow::Result<()> {
        job.progress_percent = percent.min(100);
        job.message = message.to_string();
        self.job_store.put(job).await.context("save job")?;
        Ok(())
    }

    async fn run_pipeline(&self, job: &mut Job, request: &StartJobRequest) -> anyhow::Result<()> {
        ensure_dir_does_not_exist(&job.work_dir).context("check work dir")?;
        std::fs::create_dir_all(&job.work_dir)
            .with_context(|| format!("create work dir: {}", job.work_dir.display()))?;

        let manifest_path = job.work_dir.join("manifest.jsonl");
        let toc_path = job.work_dir.join("toc.yaml");
        let book_dir = job.work_dir.join("book");
        let bundled_md_path = job.work_dir.join("book.md");
        let epub_path = job.work_dir.join("book.epub");

        self.update_progress(job, 5, "llm_crawl").await?;
        crate::llm_crawl::run(LlmCrawlArgs {
            query: request.query.clone(),
            out_dir: job.work_dir.clone(),
            max_chars: request.max_chars,
            min_sources: request.min_sources,
            search_limit: request.search_limit,
            max_pages: request.max_pages,
        })
        .await
        .context("llm_crawl")?;

        self.update_progress(job, 40, "toc").await?;
        crate::toc::create(TocCreateArgs {
            manifest: manifest_path.to_string_lossy().to_string(),
            out: toc_path.to_string_lossy().to_string(),
            book_title: request.title.clone(),
            force: false,
            language: request.language.clone(),
            tone: request.tone.clone(),
            engine: request.toc_engine,
        })
        .await
        .context("toc create")?;

        let toc_yaml = std::fs::read_to_string(&toc_path)
            .with_context(|| format!("read toc: {}", toc_path.display()))?;
        let toc: Toc = serde_yaml::from_str(&toc_yaml).context("parse toc")?;

        self.update_progress(job, 55, "book init").await?;
        crate::book::init(BookInitArgs {
            out: book_dir.to_string_lossy().to_string(),
            title: toc.book_title,
        })
        .context("book init")?;

        self.update_progress(job, 65, "book render").await?;
        let render_args = BookRenderArgs {
            toc: toc_path.to_string_lossy().to_string(),
            manifest: manifest_path.to_string_lossy().to_string(),
            out: book_dir.to_string_lossy().to_string(),
            language: request.language.clone(),
            tone: request.tone.clone(),
            engine: request.render_engine,
        };
        tokio::task::block_in_place(|| crate::book::render(render_args)).context("book render")?;

        self.update_progress(job, 90, "book bundle").await?;
        crate::book::bundle(BookBundleArgs {
            book: book_dir.to_string_lossy().to_string(),
            out: bundled_md_path.to_string_lossy().to_string(),
            force: false,
        })
        .context("book bundle")?;

        self.update_progress(job, 95, "book epub").await?;
        crate::epub::create_from_mdbook(
            &book_dir,
            &epub_path,
            &crate::epub::CreateEpubOptions {
                force: false,
                lang: crate::epub::guess_lang_tag(&request.language),
            },
        )
        .context("book epub")?;

        Ok(())
    }
}

fn ensure_dir_does_not_exist(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!("output directory already exists: {}", path.display());
    }
    Ok(())
}

pub fn default_job_work_dir(base_dir: &Path, job_id: &str) -> PathBuf {
    base_dir.join("jobs").join(job_id).join("work")
}
