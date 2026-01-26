use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;

use crate::cli::{BookBundleArgs, BookInitArgs, BookRenderArgs};
use crate::formats::{ManifestRecord, Toc};

pub fn init(args: BookInitArgs) -> anyhow::Result<()> {
    let out_dir = PathBuf::from(&args.out);
    std::fs::create_dir_all(out_dir.join("src").join("chapters"))
        .with_context(|| format!("create book dirs: {}", out_dir.display()))?;

    let book_toml = out_dir.join("book.toml");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&book_toml)
        .with_context(|| format!("create book.toml: {}", book_toml.display()))?;
    writeln!(file, "[book]")?;
    writeln!(file, "title = {:?}", args.title)?;

    let summary = out_dir.join("src").join("SUMMARY.md");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&summary)
        .with_context(|| format!("create SUMMARY.md: {}", summary.display()))?;
    writeln!(file, "# Summary\n")?;
    writeln!(file, "- [Chapter 1](chapters/ch01.md)")?;

    let ch01 = out_dir.join("src").join("chapters").join("ch01.md");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&ch01)
        .with_context(|| format!("create chapter: {}", ch01.display()))?;
    writeln!(file, "# Chapter 1\n")?;
    writeln!(file, "## Objectives\nTODO\n")?;
    writeln!(file, "## Prerequisites\nTODO\n")?;
    writeln!(file, "## Body\nTODO\n")?;
    writeln!(file, "## Summary\nTODO\n")?;
    writeln!(file, "## Sources\n")?;

    Ok(())
}

pub fn render(args: BookRenderArgs) -> anyhow::Result<()> {
    let toc_path = PathBuf::from(&args.toc);
    let toc_yaml = std::fs::read_to_string(&toc_path)
        .with_context(|| format!("read toc: {}", toc_path.display()))?;
    let toc: Toc = serde_yaml::from_str(&toc_yaml).context("parse toc")?;

    let manifest_path = PathBuf::from(&args.manifest);
    let manifest_file = OpenOptions::new()
        .read(true)
        .open(&manifest_path)
        .with_context(|| format!("open manifest: {}", manifest_path.display()))?;
    let reader = BufReader::new(manifest_file);

    let mut manifest: HashMap<String, ManifestRecord> = HashMap::new();
    for line in reader.lines() {
        let line = line.context("read manifest jsonl line")?;
        if line.trim().is_empty() {
            continue;
        }
        let record: ManifestRecord =
            serde_json::from_str(&line).context("parse manifest record")?;
        manifest.insert(record.id.clone(), record);
    }

    let out_dir = PathBuf::from(&args.out);
    let chapters_dir = out_dir.join("src").join("chapters");
    std::fs::create_dir_all(&chapters_dir)
        .with_context(|| format!("create chapters dir: {}", chapters_dir.display()))?;

    let summary_md = render_summary_md(&toc);
    std::fs::write(out_dir.join("src").join("SUMMARY.md"), summary_md)
        .with_context(|| format!("write SUMMARY.md: {}", out_dir.display()))?;

    for part in toc.parts {
        for chapter in part.chapters {
            let chapter_md =
                render_chapter_md(&chapter.id, &chapter.title, &chapter.sources, &manifest)
                    .with_context(|| format!("render chapter: {}", chapter.id))?;
            std::fs::write(chapters_dir.join(format!("{}.md", chapter.id)), chapter_md)
                .with_context(|| format!("write chapter: {}", chapter.id))?;
        }
    }

    Ok(())
}

pub fn bundle(args: BookBundleArgs) -> anyhow::Result<()> {
    let book_dir = PathBuf::from(&args.book);
    let src_dir = book_dir.join("src");
    let summary_path = src_dir.join("SUMMARY.md");
    let summary_md = std::fs::read_to_string(&summary_path)
        .with_context(|| format!("read SUMMARY.md: {}", summary_path.display()))?;

    let chapter_rel_paths = parse_summary_chapter_paths(&summary_md);
    if chapter_rel_paths.is_empty() {
        anyhow::bail!(
            "no chapter links found in SUMMARY.md: {}",
            summary_path.display()
        );
    }

    let out_path = PathBuf::from(&args.out);
    if out_path.exists() && !args.force {
        anyhow::bail!("bundle output already exists: {}", out_path.display());
    }
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create bundle parent dir: {}", parent.display()))?;
    }

    let mut bundled = String::new();
    if let Some(title) = read_book_title(&book_dir)? {
        bundled.push_str(&format!("# {title}\n\n"));
    }

    for (idx, rel_path) in chapter_rel_paths.iter().enumerate() {
        let chapter_path = src_dir.join(rel_path);
        let chapter_md = std::fs::read_to_string(&chapter_path)
            .with_context(|| format!("read chapter: {}", chapter_path.display()))?;

        if idx != 0 && !bundled.ends_with('\n') {
            bundled.push('\n');
        }
        if idx != 0 {
            bundled.push('\n');
        }

        bundled.push_str(chapter_md.trim_end());
        bundled.push('\n');
    }

    let mut out_options = OpenOptions::new();
    out_options.write(true);
    if args.force {
        out_options.create(true).truncate(true);
    } else {
        out_options.create_new(true);
    }
    let mut out = out_options
        .open(&out_path)
        .with_context(|| format!("open bundle output: {}", out_path.display()))?;
    out.write_all(bundled.as_bytes())
        .with_context(|| format!("write bundle output: {}", out_path.display()))?;
    out.flush()
        .with_context(|| format!("flush bundle output: {}", out_path.display()))?;

    Ok(())
}

fn render_summary_md(toc: &Toc) -> String {
    let mut md = String::new();
    md.push_str("# Summary\n\n");
    for part in &toc.parts {
        md.push_str(&format!("- {}\n", part.title));
        for chapter in &part.chapters {
            md.push_str(&format!(
                "  - [{}](chapters/{}.md)\n",
                chapter.title, chapter.id
            ));
        }
    }
    md
}

fn render_chapter_md(
    chapter_id: &str,
    chapter_title: &str,
    source_ids: &[String],
    manifest: &HashMap<String, ManifestRecord>,
) -> anyhow::Result<String> {
    let mut md = String::new();
    md.push_str(&format!("# {chapter_title}\n\n"));

    md.push_str("## Objectives\n");
    md.push_str("TODO\n\n");

    md.push_str("## Prerequisites\n");
    md.push_str("TODO\n\n");

    md.push_str("## Body\n\n");
    for source_id in source_ids {
        let record = manifest
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("source id not found in manifest: {source_id}"))?;

        let extracted = std::fs::read_to_string(&record.extracted_md).with_context(|| {
            format!(
                "read extracted page for {chapter_id}: {}",
                record.extracted_md
            )
        })?;
        let body = strip_front_matter(&extracted).context("strip front matter")?;
        let body = strip_leading_h1(body);

        md.push_str(&format!("### {}\n\n", record.title));
        if !body.trim().is_empty() {
            md.push_str(body.trim());
            md.push_str("\n\n");
        }
    }

    md.push_str("## Summary\n");
    md.push_str("TODO\n\n");

    md.push_str("## Sources\n");
    for source_id in source_ids {
        let record = manifest
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("source id not found in manifest: {source_id}"))?;
        md.push_str(&format!("- {}\n", record.url));
    }

    Ok(md)
}

fn strip_front_matter(contents: &str) -> anyhow::Result<&str> {
    let mut lines = contents.lines();
    let first = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("extracted page is empty"))?;
    if first.trim_end() != "---" {
        return Ok(contents);
    }

    for (idx, line) in contents.lines().enumerate().skip(1) {
        if line.trim_end() == "---" {
            let mut offset = 0usize;
            for (i, l) in contents.lines().enumerate() {
                if i <= idx {
                    offset += l.len() + 1;
                } else {
                    break;
                }
            }
            return Ok(&contents[offset..]);
        }
    }

    Ok(contents)
}

fn strip_leading_h1(body: &str) -> &str {
    let body = body.trim_start_matches('\n');
    let mut lines = body.lines();
    let Some(first) = lines.next() else {
        return body;
    };
    if !first.starts_with("# ") {
        return body;
    }

    let mut offset = first.len() + 1;
    if body.get(offset..offset + 1) == Some("\n") {
        offset += 1;
    }

    &body[offset..]
}

fn parse_summary_chapter_paths(summary_md: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in summary_md.lines() {
        let Some(target) = parse_markdown_link_target(line) else {
            continue;
        };
        let path = match target.split_once('#') {
            Some((path, _)) => path,
            None => target.as_str(),
        };
        let path = path.trim();
        if path.starts_with("http://") || path.starts_with("https://") {
            continue;
        }
        if !path.ends_with(".md") {
            continue;
        }
        paths.push(path.to_owned());
    }
    paths
}

fn parse_markdown_link_target(line: &str) -> Option<String> {
    let link_start = line.find("](")?;
    let after = &line[link_start + 2..];
    let link_end = after.find(')')?;
    Some(after[..link_end].to_owned())
}

fn read_book_title(book_dir: &std::path::Path) -> anyhow::Result<Option<String>> {
    let book_toml_path = book_dir.join("book.toml");
    if !book_toml_path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&book_toml_path)
        .with_context(|| format!("read book.toml: {}", book_toml_path.display()))?;

    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with("title") {
            let Some((_, rhs)) = line.split_once('=') else {
                continue;
            };
            let rhs = rhs.trim();
            if let Some(stripped) = rhs.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                return Ok(Some(stripped.to_owned()));
            }
        }
    }
    Ok(None)
}
