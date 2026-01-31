use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::LlmEngine;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub job_id: String,
    pub status: JobStatus,
    pub progress_percent: u32,
    pub message: String,

    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,

    pub work_dir: PathBuf,
    pub artifact_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartJobRequest {
    pub url: String,
    pub title: Option<String>,

    pub max_pages: usize,
    pub max_depth: u32,
    pub concurrency: usize,
    pub delay_ms: u64,

    pub language: String,
    pub tone: String,

    pub toc_engine: LlmEngine,
    pub render_engine: LlmEngine,
}

impl StartJobRequest {
    pub fn default_max_pages() -> usize {
        200
    }
    pub fn default_max_depth() -> u32 {
        8
    }
    pub fn default_concurrency() -> usize {
        4
    }
    pub fn default_delay_ms() -> u64 {
        200
    }
    pub fn default_language() -> String {
        "日本語".to_string()
    }
    pub fn default_tone() -> String {
        "丁寧".to_string()
    }
    pub fn default_engine() -> LlmEngine {
        LlmEngine::Noop
    }
}
