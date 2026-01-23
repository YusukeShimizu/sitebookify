use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;

use crate::cli::TocInitArgs;
use crate::formats::{ManifestRecord, Toc, TocChapter, TocPart};

pub fn init(args: TocInitArgs) -> anyhow::Result<()> {
    let manifest_path = PathBuf::from(&args.manifest);
    let out_path = PathBuf::from(&args.out);

    if out_path.exists() {
        anyhow::bail!("toc output already exists: {}", out_path.display());
    }

    let file = OpenOptions::new()
        .read(true)
        .open(&manifest_path)
        .with_context(|| format!("open manifest: {}", manifest_path.display()))?;
    let reader = BufReader::new(file);

    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line.context("read manifest jsonl line")?;
        if line.trim().is_empty() {
            continue;
        }
        let record: ManifestRecord =
            serde_json::from_str(&line).context("parse manifest record")?;
        records.push(record);
    }

    records.sort_by(|a, b| a.path.cmp(&b.path));

    let chapter_title = derive_chapter_title(&records);
    let book_title = args
        .book_title
        .clone()
        .unwrap_or_else(|| format!("{chapter_title} Textbook"));
    let sources = records.into_iter().map(|r| r.id).collect::<Vec<_>>();

    let toc = Toc {
        book_title,
        parts: vec![TocPart {
            title: "Part 1".to_owned(),
            chapters: vec![TocChapter {
                id: "ch01".to_owned(),
                title: chapter_title,
                sources,
            }],
        }],
    };

    let yaml = serde_yaml::to_string(&toc).context("serialize toc")?;
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&out_path)
        .with_context(|| format!("create toc: {}", out_path.display()))?;
    out.write_all(yaml.as_bytes())
        .with_context(|| format!("write toc: {}", out_path.display()))?;
    out.flush().context("flush toc")?;

    Ok(())
}

fn derive_chapter_title(records: &[ManifestRecord]) -> String {
    let mut paths = records
        .iter()
        .map(|r| r.path.trim_matches('/'))
        .filter(|p| !p.is_empty());

    let Some(first) = paths.next() else {
        return "Chapter 1".to_owned();
    };

    let mut common = first.split('/').collect::<Vec<_>>();
    for path in paths {
        let segments = path.split('/').collect::<Vec<_>>();
        let mut next_common = Vec::new();
        for (a, b) in common.iter().zip(segments.iter()) {
            if a == b {
                next_common.push(*a);
            } else {
                break;
            }
        }
        common = next_common;
        if common.is_empty() {
            break;
        }
    }

    let Some(first_segment) = common.first().copied() else {
        return "Chapter 1".to_owned();
    };

    title_case_segment(first_segment)
}

fn title_case_segment(segment: &str) -> String {
    let mut chars = segment.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => segment.to_owned(),
    }
}
