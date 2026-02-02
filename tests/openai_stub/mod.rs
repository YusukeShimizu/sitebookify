use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct OpenAiStubConfig {
    pub expected_reasoning_effort: Option<String>,
    pub rewrite_behavior: RewriteBehavior,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum RewriteBehavior {
    EchoInput,
    WrapTokens,
    DropTokens,
}

pub struct OpenAiStub {
    pub base_url: String,
    shutdown_tx: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl OpenAiStub {
    pub fn spawn(config: OpenAiStubConfig) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("start openai stub server");
        let addr = server.server_addr();
        let base_url = format!("http://{addr}/v1");

        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

        let handle = thread::spawn(move || {
            loop {
                if shutdown_rx.try_recv().is_ok() {
                    break;
                }

                let mut request = match server.recv_timeout(Duration::from_millis(50)) {
                    Ok(Some(req)) => req,
                    Ok(None) => continue,
                    Err(_) => break,
                };

                let path = request.url().to_string();
                if request.method() != &tiny_http::Method::Post || path != "/v1/responses" {
                    let _ = request.respond(
                        tiny_http::Response::from_string("not found").with_status_code(404),
                    );
                    continue;
                }

                let mut body = String::new();
                if request.as_reader().read_to_string(&mut body).is_err() {
                    let _ = request.respond(
                        tiny_http::Response::from_string("invalid request body")
                            .with_status_code(400),
                    );
                    continue;
                }

                let parsed: Value = match serde_json::from_str(&body) {
                    Ok(value) => value,
                    Err(_) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string("invalid json").with_status_code(400),
                        );
                        continue;
                    }
                };

                if let Some(expected) = config.expected_reasoning_effort.as_deref() {
                    let actual = parsed
                        .pointer("/reasoning/effort")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if actual != expected {
                        let _ = request.respond(
                            tiny_http::Response::from_string(format!(
                                "missing expected reasoning.effort: expected={expected} actual={actual}"
                            ))
                            .with_status_code(400),
                        );
                        continue;
                    }
                }

                let Some(prompt) = parsed.get("input").and_then(|v| v.as_str()) else {
                    let _ = request.respond(
                        tiny_http::Response::from_string("missing input").with_status_code(400),
                    );
                    continue;
                };

                let output_text = if prompt.contains("BEGIN_TOC_INPUT_JSON") {
                    match toc_response(prompt) {
                        Ok(text) => text,
                        Err(err) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(format!(
                                    "failed to build toc response: {err}"
                                ))
                                .with_status_code(400),
                            );
                            continue;
                        }
                    }
                } else if prompt.contains("BEGIN_MARKDOWN") {
                    match rewrite_response(prompt, config.rewrite_behavior) {
                        Ok(text) => text,
                        Err(err) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(format!(
                                    "failed to build rewrite response: {err}"
                                ))
                                .with_status_code(400),
                            );
                            continue;
                        }
                    }
                } else {
                    let _ = request.respond(
                        tiny_http::Response::from_string("unknown prompt mode")
                            .with_status_code(400),
                    );
                    continue;
                };

                let response_body = serde_json::json!({
                    "id": "resp_stub",
                    "object": "response",
                    "model": parsed.get("model").cloned().unwrap_or(Value::String("stub-model".to_owned())),
                    "output": [
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": [
                                { "type": "output_text", "text": output_text }
                            ]
                        }
                    ],
                    "output_text": output_text
                });

                let mut response = tiny_http::Response::from_string(response_body.to_string())
                    .with_status_code(200);
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .expect("build header");
                response = response.with_header(header);
                let _ = request.respond(response);
            }
        });

        Self {
            base_url,
            shutdown_tx: Some(shutdown_tx),
            handle: Some(handle),
        }
    }
}

impl Drop for OpenAiStub {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn extract_between<'a>(text: &'a str, begin: &str, end: &str) -> Option<&'a str> {
    let start = text.find(begin)? + begin.len();
    let rest = &text[start..];
    let end_rel = rest.find(end)?;
    Some(&rest[..end_rel])
}

fn toc_response(prompt: &str) -> anyhow::Result<String> {
    let begin = "BEGIN_TOC_INPUT_JSON\n";
    let end = "\nEND_TOC_INPUT_JSON";
    let raw = extract_between(prompt, begin, end)
        .ok_or_else(|| anyhow::anyhow!("missing toc input markers: {begin:?} .. {end:?}"))?;

    let input: Value = serde_json::from_str(raw).context("parse toc input json")?;
    let pages = input
        .get("pages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("missing pages array"))?;

    let mut ids = Vec::new();
    for page in pages {
        let Some(id) = page.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        ids.push(id.to_owned());
    }
    if ids.is_empty() {
        anyhow::bail!("no ids found in toc input");
    }

    let chapters = ids
        .iter()
        .enumerate()
        .map(|(idx, id)| {
            serde_json::json!({
                "title": format!("Chapter {}", idx + 1),
                "intent": "Test intent.",
                "reader_gains": ["Test gain."],
                "sections": [
                    {
                        "title": format!("Section {}", idx + 1),
                        "sources": [id],
                    }
                ],
            })
        })
        .collect::<Vec<_>>();

    Ok(serde_json::json!({
        "book_title": "Stub Book",
        "chapters": chapters,
    })
    .to_string())
}

fn rewrite_response(prompt: &str, behavior: RewriteBehavior) -> anyhow::Result<String> {
    if matches!(behavior, RewriteBehavior::DropTokens) {
        return Ok("short summary".to_owned());
    }

    let begin = "BEGIN_MARKDOWN\n";
    let end = "\nEND_MARKDOWN";
    let raw = extract_between(prompt, begin, end)
        .ok_or_else(|| anyhow::anyhow!("missing markdown markers: {begin:?} .. {end:?}"))?;

    let out = match behavior {
        RewriteBehavior::EchoInput => raw.to_owned(),
        RewriteBehavior::WrapTokens => raw
            .replace("{{SBY_TOKEN_", "{{{SBY_TOKEN_")
            .replace("}}", "}}}"),
        RewriteBehavior::DropTokens => unreachable!("handled above"),
    };

    Ok(out)
}
