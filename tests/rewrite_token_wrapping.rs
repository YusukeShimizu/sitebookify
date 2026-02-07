use std::fs;

use sitebookify::formats::{ManifestRecord, Toc, TocChapter, TocPart, TocSection};

mod openai_stub;

#[test]
fn rewrite_unwraps_extra_braces_around_tokens() -> anyhow::Result<()> {
    let temp = tempfile::TempDir::new()?;

    let page_id =
        "p_test_wrapped_tokens_0000000000000000000000000000000000000000000000000000000000000000";
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
Here is https://example.com and `code` and [link](https://openai.com/).\n\
\n\
```toml\n\
[dependencies]\n\
lwk_wollet = \"0.11.0\"\n\
```\n\
"
    );
    fs::write(&extracted_path, extracted)?;

    let manifest_path = temp.path().join("manifest.jsonl");
    let record = ManifestRecord {
        id: page_id.to_owned(),
        url: "https://example.com/docs".to_owned(),
        title: "Test Page".to_owned(),
        path: "/docs".to_owned(),
        extracted_md: extracted_path.to_string_lossy().to_string(),
        trust_tier: None,
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
                intent: "Intent".to_owned(),
                reader_gains: vec!["Gain".to_owned()],
                sections: vec![TocSection {
                    title: "Section".to_owned(),
                    sources: vec![page_id.to_owned()],
                }],
            }],
        }],
    };
    fs::write(&toc_path, serde_yaml::to_string(&toc)?)?;

    let book_dir = temp.path().join("book");
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

    let _openai = openai_stub::OpenAiStub::spawn(openai_stub::OpenAiStubConfig {
        expected_reasoning_effort: None,
        rewrite_behavior: openai_stub::RewriteBehavior::WrapTokens,
    });

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.env("OPENAI_API_KEY", "test-key")
        .env("SITEBOOKIFY_OPENAI_BASE_URL", &_openai.base_url)
        .env("SITEBOOKIFY_OPENAI_MODEL", "stub-model")
        .args([
            "book",
            "render",
            "--toc",
            toc_path.to_str().unwrap(),
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            book_dir.to_str().unwrap(),
            "--engine",
            "openai",
        ])
        .assert()
        .success();

    let ch01_path = book_dir.join("src").join("chapters").join("ch01.md");
    let ch01 = fs::read_to_string(ch01_path)?;

    assert!(ch01.contains("Here is https://example.com and `code`"));
    assert!(ch01.contains("[link](https://openai.com/)"));
    assert!(ch01.contains("```toml"));
    assert!(ch01.contains("lwk_wollet = \"0.11.0\""));

    // Ensure no `{...}` wrappers leak into Markdown.
    assert!(!ch01.contains("{```"));
    assert!(!ch01.contains("```}\n"));
    assert!(!ch01.contains("{`code`}"));
    assert!(!ch01.contains("]({"));

    Ok(())
}
