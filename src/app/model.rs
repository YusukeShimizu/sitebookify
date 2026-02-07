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
    pub artifact_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartJobRequest {
    pub query: String,
    pub title: Option<String>,

    pub max_chars: usize,
    pub min_sources: usize,
    pub search_limit: usize,
    pub max_pages: usize,

    pub language: String,
    pub tone: String,

    pub toc_engine: LlmEngine,
    pub render_engine: LlmEngine,
}

impl StartJobRequest {
    pub fn default_max_chars() -> usize {
        50000
    }
    pub fn default_min_sources() -> usize {
        3
    }
    pub fn default_search_limit() -> usize {
        3
    }
    pub fn default_max_pages() -> usize {
        7
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
