use std::fs;

use predicates::prelude::*;
use sitebookify::formats::{ManifestRecord, Toc, TocChapter, TocPart};

#[test]
fn rewrite_pages_can_keep_llm_output_even_when_tokens_are_missing() -> anyhow::Result<()> {
    let temp = tempfile::TempDir::new()?;

    let page_id = "p_test_missing_tokens";
    let extracted_path = temp.path().join("extracted.md");
    let extracted = format!(
        "---\n\
id: {page_id}\n\
url: https://example.com/docs\n\
retrieved_at: 2026-01-01T00:00:00Z\n\
raw_html_path: raw/index.html\n\
title: Test Page\n\
---\n\
\n\
Here is https://example.com and `code` and [link](https://openai.com/).\n"
    );
    fs::write(&extracted_path, extracted)?;

    let manifest_path = temp.path().join("manifest.jsonl");
    let record = ManifestRecord {
        id: page_id.to_owned(),
        url: "https://example.com/docs".to_owned(),
        title: "Test Page".to_owned(),
        path: "/docs".to_owned(),
        extracted_md: extracted_path.to_string_lossy().to_string(),
    };
    fs::write(
        &manifest_path,
        format!("{}\n", serde_json::to_string(&record)?),
    )?;

    let toc_path = temp.path().join("toc.yaml");
    let toc = Toc {
        book_title: "Test Book".to_owned(),
        parts: vec![TocPart {
            title: "Part".to_owned(),
            chapters: vec![TocChapter {
                id: "ch01".to_owned(),
                title: "Chapter".to_owned(),
                sources: vec![page_id.to_owned()],
            }],
        }],
    };
    fs::write(&toc_path, serde_yaml::to_string(&toc)?)?;

    let script = "cat >/dev/null; echo 'short summary'";

    // Default (strict): missing tokens => keep original section.
    let out_strict = temp.path().join("out_strict");
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "llm",
        "rewrite-pages",
        "--toc",
        toc_path.to_str().unwrap(),
        "--manifest",
        manifest_path.to_str().unwrap(),
        "--out",
        out_strict.to_str().unwrap(),
        "--prompt",
        "rewrite",
        "--engine",
        "command",
        "--command",
        "sh",
        "--",
        "-c",
        script,
    ])
    .assert()
    .success();
    let strict_page = fs::read_to_string(out_strict.join("pages").join(format!("{page_id}.md")))?;
    assert!(strict_page.contains("Here is https://example.com and `code`"));
    assert!(!strict_page.contains("short summary"));

    // Lenient: missing tokens => keep rewritten output.
    let out_lenient = temp.path().join("out_lenient");
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "llm",
        "rewrite-pages",
        "--toc",
        toc_path.to_str().unwrap(),
        "--manifest",
        manifest_path.to_str().unwrap(),
        "--out",
        out_lenient.to_str().unwrap(),
        "--prompt",
        "rewrite",
        "--engine",
        "command",
        "--allow-missing-tokens",
        "--command",
        "sh",
        "--",
        "-c",
        script,
    ])
    .assert()
    .success();
    let lenient_page = fs::read_to_string(out_lenient.join("pages").join(format!("{page_id}.md")))?;
    assert!(lenient_page.contains("short summary"));
    assert!(!lenient_page.contains("Here is https://example.com and `code`"));

    // Sanity: strict mode should log the warning about missing tokens.
    let out_strict2 = temp.path().join("out_strict2");
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.args([
        "llm",
        "rewrite-pages",
        "--toc",
        toc_path.to_str().unwrap(),
        "--manifest",
        manifest_path.to_str().unwrap(),
        "--out",
        out_strict2.to_str().unwrap(),
        "--prompt",
        "rewrite",
        "--engine",
        "command",
        "--command",
        "sh",
        "--",
        "-c",
        script,
    ])
    .assert()
    .success()
    .stderr(predicate::str::contains(
        "rewrite output is missing placeholder tokens",
    ));

    Ok(())
}
