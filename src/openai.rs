use anyhow::Context as _;

pub fn responses_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    format!("{base_url}/responses")
}

pub async fn responses_text(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    instructions: &str,
    input: &str,
    temperature: f32,
) -> anyhow::Result<String> {
    let mut body = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "text": { "format": { "type": "text" } },
        "store": false,
    });

    // NOTE: Some GPT-5 models reject sampling params like `temperature`.
    // Keep compatibility by omitting it for the GPT-5 family by default.
    if !model.starts_with("gpt-5")
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("temperature".to_owned(), serde_json::json!(temperature));
    }

    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {endpoint}"))?;

    let status = response.status();
    let raw = response.text().await.context("read OpenAI response body")?;
    if !status.is_success() {
        let message = parse_error_message(&raw).unwrap_or_else(|| raw.clone());
        anyhow::bail!("OpenAI API error ({status}): {message}");
    }

    let value: serde_json::Value = serde_json::from_str(&raw).context("parse OpenAI response")?;
    extract_output_text(&value).context("extract output text")
}

fn parse_error_message(raw_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw_json).ok()?;
    let message = value.get("error")?.get("message")?.as_str()?.to_owned();
    Some(message)
}

fn extract_output_text(value: &serde_json::Value) -> anyhow::Result<String> {
    let output = value
        .get("output")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("missing `output` array in response"))?;

    let mut text = String::new();
    for item in output {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        let content = match item.get("content").and_then(|v| v.as_array()) {
            Some(content) => content,
            None => continue,
        };
        for part in content {
            if part.get("type").and_then(|v| v.as_str()) != Some("output_text") {
                continue;
            }
            let Some(part_text) = part.get("text").and_then(|v| v.as_str()) else {
                continue;
            };
            text.push_str(part_text);
        }
    }

    if text.trim().is_empty() {
        anyhow::bail!("OpenAI output text is empty");
    }
    Ok(text)
}
