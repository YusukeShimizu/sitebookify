use std::collections::HashSet;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use predicates::prelude::*;
use sha2::Digest as _;
use sha2::Sha256;
use sitebookify::formats::{CrawlRecord, ManifestRecord, Toc};

mod openai_stub;

static LOGO_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 23, 0, 2, 3, 1, 128,
    110, 220, 25, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

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

            enum Body {
                Text(&'static str),
                Bytes(&'static [u8]),
            }

            let (status, body) = match path {
                "/docs" | "/docs/" => (
                    200,
                    Body::Text(
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
                ),
                "/docs/intro" => (
                    200,
                    Body::Text(
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
                ),
                "/docs/advanced" => (
                    200,
                    Body::Text(
                        r#"<!doctype html>
<html>
  <head><title>Advanced</title></head>
  <body>
    <h1>Advanced</h1>
    <p>Advanced content.</p>
    <h2>キーボードショートカット</h2>
    <p>章間の移動には ← または → を押します</p>
    <p>本の検索には S または / を押します</p>
    <p>? を押すとこのヘルプを表示します</p>
    <p>Esc を押すとこのヘルプを非表示にします</p>
    <p><img src="/docs/assets/logo.png" alt="Logo" /></p>
  </body>
</html>
"#,
                    ),
                ),
                "/docs/assets/logo.png" => (200, Body::Bytes(LOGO_PNG)),
                "/outside" => (
                    200,
                    Body::Text(
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
                ),
                _ => (404, Body::Text("not found")),
            };

            let mut response = match body {
                Body::Text(text) => tiny_http::Response::from_string(text),
                Body::Bytes(bytes) => tiny_http::Response::from_data(bytes.to_vec()),
            }
            .with_status_code(status);

            if status == 200 {
                let content_type_value = match body {
                    Body::Text(_) => "text/html; charset=utf-8",
                    Body::Bytes(_) => "image/png",
                };
                let header = tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    content_type_value.as_bytes(),
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
    let _openai = openai_stub::OpenAiStub::spawn(openai_stub::OpenAiStubConfig {
        expected_reasoning_effort: Some("high".to_owned()),
        rewrite_behavior: openai_stub::RewriteBehavior::EchoInput,
    });

    let workspace_dir = temp.path().join("workspace");
    let raw_dir = workspace_dir.join("raw");
    let extracted_dir = workspace_dir.join("extracted");
    let manifest_path = workspace_dir.join("manifest.jsonl");
    let toc_path = workspace_dir.join("toc.yaml");
    let book_dir = workspace_dir.join("book");

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.env("OPENAI_API_KEY", "test-key")
        .env("SITEBOOKIFY_OPENAI_BASE_URL", &_openai.base_url)
        .env("SITEBOOKIFY_OPENAI_MODEL", "stub-model")
        .env("SITEBOOKIFY_OPENAI_REASONING_EFFORT", "high")
        .args([
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
            "--language",
            "日本語",
            "--tone",
            "丁寧",
            "--toc-engine",
            "openai",
            "--render-engine",
            "openai",
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
    assert_eq!(toc.parts.len(), 1, "expected exactly one part");

    let manifest_records: Vec<ManifestRecord> = fs::read_to_string(&manifest_path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse manifest record json"))
        .collect();
    assert!(
        !manifest_records.is_empty(),
        "expected manifest to be non-empty"
    );

    assert_eq!(
        toc.parts[0].chapters.len(),
        manifest_records.len(),
        "expected one chapter per manifest record from stub openai"
    );

    let mut manifest_sorted = manifest_records.clone();
    manifest_sorted.sort_by(|a, b| a.path.cmp(&b.path));

    let mut seen_ids = HashSet::new();
    for (idx, chapter) in toc.parts[0].chapters.iter().enumerate() {
        assert_eq!(chapter.id, format!("ch{:02}", idx + 1));
        assert!(!chapter.title.trim().is_empty());
        assert!(!chapter.intent.trim().is_empty());
        assert!(!chapter.reader_gains.is_empty());
        assert_eq!(chapter.sections.len(), 1);
        assert_eq!(chapter.sections[0].sources.len(), 1);
        assert_eq!(chapter.sections[0].sources[0], manifest_sorted[idx].id);
        assert!(seen_ids.insert(chapter.sections[0].sources[0].clone()));
    }
    let manifest_ids: HashSet<String> = manifest_records.iter().map(|r| r.id.clone()).collect();
    assert_eq!(seen_ids, manifest_ids);

    let advanced_record = manifest_sorted
        .iter()
        .find(|r| r.path == "/docs/advanced")
        .expect("expected /docs/advanced in manifest");
    let advanced_id = advanced_record.id.clone();
    let expected_image_url = format!("{base_url}/docs/assets/logo.png");
    let mut hasher = Sha256::new();
    hasher.update(expected_image_url.as_bytes());
    let expected_image_hash = hex::encode(hasher.finalize());
    let expected_asset_file = format!("img_{expected_image_hash}.png");
    let expected_chapter_image_ref = format!("../assets/{expected_asset_file}");

    let advanced_idx = manifest_sorted
        .iter()
        .position(|r| r.id == advanced_id)
        .expect("advanced is in manifest");
    let advanced_chapter_id = format!("ch{:02}", advanced_idx + 1);

    let advanced_chapter_path = book_dir
        .join("src")
        .join("chapters")
        .join(format!("{advanced_chapter_id}.md"));
    let advanced_chapter = fs::read_to_string(&advanced_chapter_path)?;
    assert!(advanced_chapter.contains("## Sources"));
    assert!(!advanced_chapter.contains("TODO"));
    assert!(!advanced_chapter.contains("キーボードショートカット"));
    assert!(!advanced_chapter.contains("章間の移動には"));
    assert!(advanced_chapter.contains(&format!(
        "<span id=\"{advanced_id}\" style=\"display:none\" aria-hidden=\"true\"></span>"
    )));
    assert!(advanced_chapter.contains(&expected_chapter_image_ref));
    assert!(!advanced_chapter.contains(&expected_image_url));

    let book_asset_path = book_dir
        .join("src")
        .join("assets")
        .join(&expected_asset_file);
    assert!(book_asset_path.exists(), "expected asset to exist");
    assert!(
        fs::metadata(&book_asset_path)?.len() > 0,
        "expected asset to be non-empty"
    );

    // Cross-chapter link rewriting: Docs Root links to Advanced.
    let docs_root_idx = manifest_sorted
        .iter()
        .position(|r| r.path == "/docs")
        .expect("/docs in manifest");
    let docs_root_chapter_id = format!("ch{:02}", docs_root_idx + 1);
    let docs_root_chapter_path = book_dir
        .join("src")
        .join("chapters")
        .join(format!("{docs_root_chapter_id}.md"));
    let docs_root_chapter = fs::read_to_string(&docs_root_chapter_path)?;
    assert!(
        docs_root_chapter.contains(&format!("{advanced_chapter_id}.md#{advanced_id}")),
        "expected cross-chapter link to be rewritten"
    );

    let bundle_md_path = workspace_dir.join("book.md");
    assert!(bundle_md_path.exists(), "expected book.md to exist");
    let bundle_md = fs::read_to_string(&bundle_md_path)?;
    assert!(bundle_md.contains("# Test Book"));
    assert!(bundle_md.contains("## Sources"));
    assert!(bundle_md.contains(&format!("assets/{expected_asset_file}")));
    assert!(!bundle_md.contains("../assets/"));
    assert!(
        bundle_md.contains(&format!("#{advanced_id}")),
        "expected bundled cross-chapter link to be rewritten"
    );
    assert!(
        !bundle_md.contains(&format!("{advanced_chapter_id}.md#{advanced_id}")),
        "expected no cross-chapter links in bundle"
    );

    let bundled_asset_path = workspace_dir.join("assets").join(&expected_asset_file);
    assert!(
        bundled_asset_path.exists(),
        "expected bundled asset to exist"
    );
    assert!(
        fs::metadata(&bundled_asset_path)?.len() > 0,
        "expected bundled asset to be non-empty"
    );

    let epub_path = workspace_dir.join("book.epub");
    assert!(epub_path.exists(), "expected book.epub to exist");

    let epub_file = fs::File::open(&epub_path)?;
    let mut epub = zip::ZipArchive::new(epub_file)?;

    {
        let mut mimetype = epub.by_name("mimetype")?;
        let mut mimetype_contents = String::new();
        mimetype.read_to_string(&mut mimetype_contents)?;
        assert_eq!(mimetype_contents, "application/epub+zip");
    }

    {
        let mut container = epub.by_name("META-INF/container.xml")?;
        let mut container_xml = String::new();
        container.read_to_string(&mut container_xml)?;
        assert!(container_xml.contains("OEBPS/content.opf"));
    }

    {
        let mut asset_in_epub = epub.by_name(&format!("OEBPS/assets/{expected_asset_file}"))?;
        let mut asset_bytes = Vec::new();
        asset_in_epub.read_to_end(&mut asset_bytes)?;
        assert!(
            !asset_bytes.is_empty(),
            "expected epub asset to be non-empty"
        );
    }

    {
        let docs_root_xhtml_path = format!("OEBPS/{docs_root_chapter_id}.xhtml");
        let mut docs_root_xhtml = epub.by_name(&docs_root_xhtml_path)?;
        let mut docs_root_xhtml_contents = String::new();
        docs_root_xhtml.read_to_string(&mut docs_root_xhtml_contents)?;
        assert!(
            docs_root_xhtml_contents
                .contains(&format!("{advanced_chapter_id}.xhtml#{advanced_id}")),
            "expected cross-chapter link to point to xhtml in epub"
        );
    }

    {
        let advanced_xhtml_path = format!("OEBPS/{advanced_chapter_id}.xhtml");
        let mut advanced_xhtml = epub.by_name(&advanced_xhtml_path)?;
        let mut advanced_xhtml_contents = String::new();
        advanced_xhtml.read_to_string(&mut advanced_xhtml_contents)?;
        assert!(advanced_xhtml_contents.contains(&format!("assets/{expected_asset_file}")));
        assert!(!advanced_xhtml_contents.contains("../assets/"));
        assert!(
            !advanced_xhtml_contents.contains(&expected_image_url),
            "expected epub to reference local asset instead of remote URL"
        );
    }

    let bundle2_md_path = workspace_dir.join("book.bundle2.md");
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "book",
        "bundle",
        "--book",
        book_dir.to_str().unwrap(),
        "--out",
        bundle2_md_path.to_str().unwrap(),
    ])
    .assert()
    .success();
    assert_eq!(fs::read_to_string(&bundle2_md_path)?, bundle_md);

    // Bundled outputs MUST NOT be overwritten.
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "book",
        "bundle",
        "--book",
        book_dir.to_str().unwrap(),
        "--out",
        bundle2_md_path.to_str().unwrap(),
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains("already exists"));

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();
    Ok(())
}
