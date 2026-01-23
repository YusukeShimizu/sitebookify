use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use predicates::prelude::*;
use sitebookify::formats::{CrawlRecord, Toc};

fn spawn_docs_server() -> (String, mpsc::Sender<()>, thread::JoinHandle<()>) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start tiny_http server");
    let addr = server.server_addr();
    let base_url = format!("http://{addr}");

    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let handle = thread::spawn(move || {
        loop {
            if shutdown_rx.try_recv().is_ok() {
                break;
            }

            let request = match server.recv_timeout(Duration::from_millis(50)) {
                Ok(Some(req)) => req,
                Ok(None) => continue,
                Err(_) => break,
            };

            let url = request.url().to_string();
            if url.contains('?') {
                let _ = request.respond(
                    tiny_http::Response::from_string("query is not supported in this test server")
                        .with_status_code(400),
                );
                continue;
            }

            let path = url.split('?').next().unwrap_or(&url);

            let (status, body) = match path {
                "/docs" | "/docs/" => (
                    200,
                    r#"<!doctype html>
<html>
  <head><title>Docs Root</title></head>
  <body>
    <h1>Docs Root</h1>
    <p>Welcome.</p>
    <a href="intro?ref=1#top">Intro (query+fragment)</a>
    <a href="./advanced">Advanced (relative)</a>
    <a href="/outside">Outside scope</a>
  </body>
</html>
"#,
                ),
                "/docs/intro" => (
                    200,
                    r#"<!doctype html>
<html>
  <head><title>Intro</title></head>
  <body>
    <h1>Intro</h1>
    <p>This is intro.</p>
    <a href="/docs/advanced#more">Advanced</a>
  </body>
</html>
"#,
                ),
                "/docs/advanced" => (
                    200,
                    r#"<!doctype html>
<html>
  <head><title>Advanced</title></head>
  <body>
    <h1>Advanced</h1>
    <p>Advanced content.</p>
  </body>
</html>
"#,
                ),
                "/outside" => (
                    200,
                    r#"<!doctype html>
<html>
  <head><title>Outside</title></head>
  <body>
    <h1>Outside</h1>
    <p>This page MUST NOT be crawled when starting under /docs/.</p>
  </body>
</html>
"#,
                ),
                _ => (404, "not found"),
            };

            let mut response = tiny_http::Response::from_string(body).with_status_code(status);
            if status == 200 {
                let header = tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/html; charset=utf-8"[..],
                )
                .expect("build header");
                response = response.with_header(header);
            }

            let _ = request.respond(response);
        }
    });

    (base_url, shutdown_tx, handle)
}

fn count_files_recursively(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            files.push(path);
        }
    }
    Ok(files)
}

fn count_files_with_name(dir: &Path, file_name: &str) -> anyhow::Result<usize> {
    let mut count = 0_usize;
    for path in count_files_recursively(dir)? {
        if path.file_name().and_then(|n| n.to_str()) == Some(file_name) {
            count += 1;
        }
    }
    Ok(count)
}

fn count_files_with_extension(dir: &Path, extension: &str) -> anyhow::Result<usize> {
    let mut count = 0_usize;
    for path in count_files_recursively(dir)? {
        if path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
            count += 1;
        }
    }
    Ok(count)
}

#[test]
fn pipeline_generates_mdbook_with_sources() -> anyhow::Result<()> {
    let (base_url, shutdown_tx, server_handle) = spawn_docs_server();
    let temp = tempfile::TempDir::new()?;
    let start_url = format!("{base_url}/docs/");

    let workspace_dir = temp.path().join("workspace");
    let raw_dir = workspace_dir.join("raw");
    let extracted_dir = workspace_dir.join("extracted");
    let manifest_path = workspace_dir.join("manifest.jsonl");
    let toc_path = workspace_dir.join("toc.yaml");
    let book_dir = workspace_dir.join("book");

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "build",
        "--url",
        &start_url,
        "--out",
        workspace_dir.to_str().unwrap(),
        "--title",
        "Test Book",
        "--max-pages",
        "20",
        "--max-depth",
        "8",
        "--concurrency",
        "2",
        "--delay-ms",
        "0",
    ])
    .assert()
    .success();

    let crawl_jsonl = raw_dir.join("crawl.jsonl");
    let crawl_log = fs::read_to_string(&crawl_jsonl)?;
    let crawl_records: Vec<CrawlRecord> = crawl_log
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse crawl record json"))
        .collect();
    assert!(
        crawl_records
            .iter()
            .any(|record| record.normalized_url == format!("{base_url}/docs"))
    );
    assert!(
        !crawl_records
            .iter()
            .any(|record| record.normalized_url.contains("/outside"))
    );
    assert!(
        crawl_records
            .iter()
            .all(|record| !record.normalized_url.contains('?'))
    );
    assert!(
        crawl_records
            .iter()
            .all(|record| !record.normalized_url.contains('#'))
    );

    let html_dir = raw_dir.join("html");
    let saved_pages = count_files_with_name(&html_dir, "index.html")?;
    assert!(
        saved_pages >= 3,
        "expected >= 3 saved pages, got {saved_pages}"
    );

    let extracted_pages_dir = extracted_dir.join("pages");
    let extracted_pages = count_files_with_extension(&extracted_pages_dir, "md")?;
    assert!(extracted_pages >= 3);

    assert!(manifest_path.exists(), "expected manifest.jsonl to exist");
    assert!(toc_path.exists(), "expected toc.yaml to exist");

    let toc_yaml = fs::read_to_string(&toc_path)?;
    let toc: Toc = serde_yaml::from_str(&toc_yaml).expect("parse toc yaml");
    assert_eq!(toc.book_title, "Test Book");

    let ch01_path = book_dir.join("src").join("chapters").join("ch01.md");
    let ch01 = fs::read_to_string(ch01_path)?;
    assert!(ch01.contains("## Sources"));
    assert!(ch01.contains(&format!("{base_url}/docs")));

    // Snapshot outputs MUST NOT be overwritten.
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "build",
        "--url",
        &start_url,
        "--out",
        workspace_dir.to_str().unwrap(),
        "--title",
        "Test Book",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("already exists"));

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "crawl",
        "--url",
        &start_url,
        "--out",
        raw_dir.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("already exists"));

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "extract",
        "--raw",
        raw_dir.to_str().unwrap(),
        "--out",
        extracted_dir.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("already exists"));

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();

    Ok(())
}
