use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use reqwest::StatusCode;

use crate::app::queue::InProcessQueue;
use crate::app::runner::JobRunner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    InProcess,
    Worker,
}

impl ExecutionMode {
    pub fn from_env() -> anyhow::Result<Self> {
        let raw =
            std::env::var("SITEBOOKIFY_EXECUTION_MODE").unwrap_or_else(|_| "inprocess".to_string());
        Self::parse(&raw).with_context(|| {
            format!(
                "invalid SITEBOOKIFY_EXECUTION_MODE={raw:?}. expected one of: inprocess, worker"
            )
        })
    }

    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "inprocess" => Ok(Self::InProcess),
            "worker" => Ok(Self::Worker),
            other => anyhow::bail!("unsupported execution mode: {other}"),
        }
    }
}

#[async_trait]
pub trait JobDispatcher: Send + Sync {
    async fn dispatch(&self, job_id: &str) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct InProcessJobDispatcher {
    queue: InProcessQueue,
    runner: Arc<JobRunner>,
}

impl InProcessJobDispatcher {
    pub fn new(queue: InProcessQueue, runner: Arc<JobRunner>) -> Self {
        Self { queue, runner }
    }
}

#[async_trait]
impl JobDispatcher for InProcessJobDispatcher {
    async fn dispatch(&self, job_id: &str) -> anyhow::Result<()> {
        let runner = Arc::clone(&self.runner);
        let job_id = job_id.to_string();
        self.queue.spawn(async move {
            runner.run_job(&job_id).await;
        });
        Ok(())
    }
}

#[derive(Clone)]
pub struct WorkerJobDispatcher {
    client: reqwest::Client,
    base_url: String,
    auth_token: Option<String>,
}

impl WorkerJobDispatcher {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = std::env::var("SITEBOOKIFY_WORKER_URL")
            .context("SITEBOOKIFY_WORKER_URL is required for worker execution mode")?;
        let base_url = base_url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            anyhow::bail!("SITEBOOKIFY_WORKER_URL is empty");
        }
        let auth_token = std::env::var("SITEBOOKIFY_WORKER_AUTH_TOKEN")
            .context("SITEBOOKIFY_WORKER_AUTH_TOKEN is required for worker execution mode")?;
        let auth_token = auth_token.trim().to_string();
        if auth_token.is_empty() {
            anyhow::bail!("SITEBOOKIFY_WORKER_AUTH_TOKEN is empty");
        }
        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
            auth_token: Some(auth_token),
        })
    }
}

#[async_trait]
impl JobDispatcher for WorkerJobDispatcher {
    async fn dispatch(&self, job_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/internal/jobs/{job_id}/run", self.base_url);
        let mut req = self.client.post(url);
        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.context("send worker dispatch request")?;
        if resp.status().is_success() || resp.status() == StatusCode::ACCEPTED {
            return Ok(());
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("worker dispatch failed ({status}): {body}");
    }
}

#[cfg(test)]
mod tests {
    use super::ExecutionMode;

    #[test]
    fn parse_inprocess_variants() {
        assert_eq!(
            ExecutionMode::parse("inprocess").unwrap(),
            ExecutionMode::InProcess
        );
        assert_eq!(
            ExecutionMode::parse("INPROCESS").unwrap(),
            ExecutionMode::InProcess
        );
        assert_eq!(ExecutionMode::parse("").unwrap(), ExecutionMode::InProcess);
    }

    #[test]
    fn parse_worker() {
        assert_eq!(
            ExecutionMode::parse("worker").unwrap(),
            ExecutionMode::Worker
        );
        assert_eq!(
            ExecutionMode::parse(" Worker ").unwrap(),
            ExecutionMode::Worker
        );
    }

    #[test]
    fn parse_invalid() {
        let err = ExecutionMode::parse("queue").unwrap_err().to_string();
        assert!(err.contains("unsupported execution mode"));
    }
}
