use std::time::Duration;

use anyhow::Context as _;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

impl OpenAiConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = std::env::var("SITEBOOKIFY_OPENAI_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .context(
                "missing OpenAI API key: set OPENAI_API_KEY (or SITEBOOKIFY_OPENAI_API_KEY)",
            )?;

        let base_url = std::env::var("SITEBOOKIFY_OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());

        let model = std::env::var("SITEBOOKIFY_OPENAI_MODEL")
            .or_else(|_| std::env::var("OPENAI_MODEL"))
            .unwrap_or_else(|_| "gpt-5.2".to_owned());

        let reasoning_effort = std::env::var("SITEBOOKIFY_OPENAI_REASONING_EFFORT")
            .ok()
            .filter(|effort| !effort.trim().is_empty())
            .or_else(|| Some("medium".to_owned()));

        Ok(Self {
            api_key,
            base_url,
            model,
            reasoning_effort,
        })
    }
}

#[derive(Debug, Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    input: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Reasoning<'a>>,
}

#[derive(Debug, Serialize)]
struct Reasoning<'a> {
    effort: &'a str,
}

pub fn exec_readonly(prompt: &str, config: &OpenAiConfig) -> anyhow::Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .context("build openai http client")?;

    let url = format!("{}/responses", config.base_url.trim_end_matches('/'));

    tracing::info!(
        base_url = %config.base_url,
        model = %config.model,
        reasoning_effort = ?config.reasoning_effort,
        "openai responses api"
    );

    let request = ResponsesRequest {
        model: &config.model,
        input: prompt,
        reasoning: config
            .reasoning_effort
            .as_deref()
            .map(|effort| Reasoning { effort }),
    };

    let response = client
        .post(url)
        .bearer_auth(&config.api_key)
        .json(&request)
        .send()
        .context("POST /responses")?;

    let status = response.status();
    let body = response.text().context("read openai response body")?;

    if !status.is_success() {
        if let Ok(value) = serde_json::from_str::<Value>(&body)
            && let Some(message) = value.pointer("/error/message").and_then(|v| v.as_str())
        {
            anyhow::bail!("openai responses api failed ({status}): {message}");
        }
        anyhow::bail!("openai responses api failed ({status}): {body}");
    }

    let value: Value = serde_json::from_str(&body).context("parse openai responses json")?;
    extract_output_text(&value).context("extract openai output text")
}

fn extract_output_text(value: &Value) -> anyhow::Result<String> {
    if let Some(text) = value.get("output_text").and_then(|v| v.as_str()) {
        return Ok(text.to_owned());
    }

    let Some(output) = value.get("output").and_then(|v| v.as_array()) else {
        anyhow::bail!("missing `output` in openai responses json");
    };

    let mut parts = Vec::new();
    for item in output {
        let Some(content) = item.get("content").and_then(|v| v.as_array()) else {
            continue;
        };
        for chunk in content {
            if let Some(text) = chunk.get("text").and_then(|v| v.as_str()) {
                parts.push(text);
                continue;
            }
            if let Some(text) = chunk.get("output_text").and_then(|v| v.as_str()) {
                parts.push(text);
                continue;
            }
        }
    }

    if parts.is_empty() {
        anyhow::bail!("missing output text in openai responses json");
    }

    Ok(parts.join(""))
}
