use std::collections::HashSet;
use std::fs;
use std::io::Read as _;
use std::path::Path;

use predicates::prelude::*;
use sitebookify::formats::{ManifestRecord, Toc};

mod openai_stub;

/// Simulates the output of `llm_crawl::run()` by writing extracted pages and
/// a manifest.jsonl into `workspace_dir`.  Returns the list of ManifestRecords.
fn seed_workspace(workspace_dir: &Path) -> Vec<ManifestRecord> {
    let extracted_dir = workspace_dir.join("extracted").join("pages");
    fs::create_dir_all(&extracted_dir).expect("create extracted dir");

    let pages: Vec<(&str, &str, &str, &str)> = vec![
        (
            "id_intro",
            "https://example.com/intro",
            "Intro",
            "# Intro\n\nThis is an introductory page.\n\n[Go to Advanced](https://example.com/advanced)\n",
        ),
        (
            "id_advanced",
            "https://example.com/advanced",
            "Advanced",
            "# Advanced\n\nAdvanced content here.\n\n![Logo](https://via.placeholder.com/1x1.png)\n",
        ),
        (
            "id_faq",
            "https://example.com/faq",
            "FAQ",
            "# FAQ\n\nFrequently asked questions.\n",
        ),
    ];

    let mut records = Vec::new();

    for (id, url, title, body) in &pages {
        let filename = format!("p_{id}.md");
        let md_path = extracted_dir.join(&filename);

        let front_matter = format!(
            "---\nid: {id}\nurl: {url}\nretrieved_at: \"2025-01-01T00:00:00Z\"\ntitle: {title}\n---\n\n"
        );
        let content = format!("{front_matter}{body}");
        fs::write(&md_path, &content).expect("write extracted page");

        let absolute_md = md_path.to_str().expect("UTF-8 path").to_string();
        records.push(ManifestRecord {
            id: id.to_string(),
            url: url.to_string(),
            title: title.to_string(),
            path: absolute_md.clone(),
            extracted_md: absolute_md,
            trust_tier: Some("medium".to_string()),
        });
    }

    let manifest_path = workspace_dir.join("manifest.jsonl");
    let manifest_content: Vec<String> = records
        .iter()
        .map(|r| serde_json::to_string(r).expect("serialize manifest record"))
        .collect();
    fs::write(&manifest_path, manifest_content.join("\n")).expect("write manifest");

    records
}

#[test]
fn pipeline_generates_book_from_extracted_sources() -> anyhow::Result<()> {
    let temp = tempfile::TempDir::new()?;
    let _openai = openai_stub::OpenAiStub::spawn(openai_stub::OpenAiStubConfig {
        expected_reasoning_effort: Some("high".to_owned()),
        rewrite_behavior: openai_stub::RewriteBehavior::EchoInput,
    });

    let workspace_dir = temp.path().join("workspace");
    fs::create_dir_all(&workspace_dir)?;

    let manifest_records = seed_workspace(&workspace_dir);

    let manifest_path = workspace_dir.join("manifest.jsonl");
    let toc_path = workspace_dir.join("toc.yaml");
    let book_dir = workspace_dir.join("book");
    let bundle_md_path = workspace_dir.join("book.md");

    // --- toc create ---
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.current_dir(&workspace_dir)
        .env("OPENAI_API_KEY", "test-key")
        .env("SITEBOOKIFY_OPENAI_BASE_URL", &_openai.base_url)
        .env("SITEBOOKIFY_OPENAI_MODEL", "stub-model")
        .env("SITEBOOKIFY_OPENAI_REASONING_EFFORT", "high")
        .args([
            "toc",
            "create",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            toc_path.to_str().unwrap(),
            "--book-title",
            "Test Book",
            "--language",
            "日本語",
            "--tone",
            "丁寧",
            "--engine",
            "openai",
        ])
        .assert()
        .success();

    assert!(toc_path.exists(), "expected toc.yaml to exist");

    let toc_yaml = fs::read_to_string(&toc_path)?;
    let toc: Toc = serde_yaml::from_str(&toc_yaml).expect("parse toc yaml");
    // The stub returns "Stub Book" but --book-title overrides it.
    assert_eq!(toc.book_title, "Test Book");
    assert_eq!(toc.parts.len(), 1, "expected exactly one part");

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

    // --- book init ---
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "book",
        "init",
        "--out",
        book_dir.to_str().unwrap(),
        "--title",
        "Test Book",
    ])
    .assert()
    .success();

    // --- book render ---
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.current_dir(&workspace_dir)
        .env("OPENAI_API_KEY", "test-key")
        .env("SITEBOOKIFY_OPENAI_BASE_URL", &_openai.base_url)
        .env("SITEBOOKIFY_OPENAI_MODEL", "stub-model")
        .env("SITEBOOKIFY_OPENAI_REASONING_EFFORT", "high")
        .args([
            "book",
            "render",
            "--toc",
            toc_path.to_str().unwrap(),
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            book_dir.to_str().unwrap(),
            "--language",
            "日本語",
            "--tone",
            "丁寧",
            "--engine",
            "openai",
        ])
        .assert()
        .success();

    // Verify chapters were generated.
    for (idx, _chapter) in toc.parts[0].chapters.iter().enumerate() {
        let chapter_path = book_dir
            .join("src")
            .join("chapters")
            .join(format!("ch{:02}.md", idx + 1));
        assert!(
            chapter_path.exists(),
            "expected chapter file: {}",
            chapter_path.display()
        );
        let chapter_md = fs::read_to_string(&chapter_path)?;
        assert!(
            chapter_md.contains("## Sources"),
            "expected Sources section in chapter"
        );
        assert!(
            !chapter_md.contains("TODO"),
            "expected no TODO markers in chapter"
        );
    }

    // --- book bundle ---
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "book",
        "bundle",
        "--book",
        book_dir.to_str().unwrap(),
        "--out",
        bundle_md_path.to_str().unwrap(),
    ])
    .assert()
    .success();

    assert!(bundle_md_path.exists(), "expected book.md to exist");
    let bundle_md = fs::read_to_string(&bundle_md_path)?;
    assert!(bundle_md.contains("# Test Book"));
    assert!(bundle_md.contains("## Sources"));
    assert!(
        !bundle_md.contains("../assets/"),
        "expected no ../assets/ in bundled output"
    );

    // --- book epub ---
    let epub_path = workspace_dir.join("book.epub");
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "book",
        "epub",
        "--book",
        book_dir.to_str().unwrap(),
        "--out",
        epub_path.to_str().unwrap(),
    ])
    .assert()
    .success();

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

    // --- book bundle refuses to overwrite ---
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

    // Second run without --force should fail.
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

    Ok(())
}
