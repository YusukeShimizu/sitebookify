use std::fs;
use std::path::Path;

use sitebookify::formats::{ManifestRecord, Toc, TocChapter, TocPart, TocSection};

fn write_stub_codex_wrapping_tokens(bin_path: &Path) -> anyhow::Result<()> {
    let script = r#"#!/bin/sh
set -eu

out=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output-last-message)
      out="$2"
      shift 2
      ;;
    --output-last-message=*)
      out="${1#*=}"
      shift 1
      ;;
    *)
      shift 1
      ;;
  esac
done

if [ -z "$out" ]; then
  echo "missing --output-last-message" >&2
  exit 2
fi

prompt="$(cat)"

if echo "$prompt" | grep -q "Rewrite the input Markdown"; then
  input_path="$(echo "$prompt" | sed -n 's/^- Read the Markdown from the file at: //p' | head -n 1)"
  if [ -z "$input_path" ]; then
    echo "missing rewrite input path" >&2
    exit 2
  fi

  # Mimic a model that "escapes" our placeholder tokens by wrapping them in extra braces.
  # Example: `{{SBY_TOKEN_000000}}` -> `{{{SBY_TOKEN_000000}}}`
  cat "$input_path" \
    | sed 's/{{SBY_TOKEN_/{{{SBY_TOKEN_/g' \
    | sed 's/}}/}}}/g' \
    >"$out"
  exit 0
fi

echo "unknown stub mode" >&2
exit 2
"#;

    fs::write(bin_path, script)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(bin_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_path, perms)?;
    }

    Ok(())
}

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

    let stub_codex = temp.path().join("codex-wrap-tokens");
    write_stub_codex_wrapping_tokens(&stub_codex)?;

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("sitebookify");
    cmd.env("SITEBOOKIFY_CODEX_BIN", stub_codex.to_str().unwrap())
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
            "codex",
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
