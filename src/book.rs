use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use sha2::Digest as _;
use sha2::Sha256;
use url::Url;

use crate::cli::{BookBundleArgs, BookEpubArgs, BookInitArgs, BookRenderArgs, LlmEngine};
use crate::formats::{ManifestRecord, Toc};
use crate::rewrite;

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

    let dir_index_ids = compute_dir_index_ids(manifest.values());
    let url_to_location = build_url_to_location(&toc, &manifest);

    let out_dir = PathBuf::from(&args.out);
    let chapters_dir = out_dir.join("src").join("chapters");
    let assets_dir = out_dir.join("src").join("assets");
    std::fs::create_dir_all(&chapters_dir)
        .with_context(|| format!("create chapters dir: {}", chapters_dir.display()))?;

    let assets = AssetDownloader::new(assets_dir).context("initialize book asset downloader")?;

    let summary_md = render_summary_md(&toc);
    std::fs::write(out_dir.join("src").join("SUMMARY.md"), summary_md)
        .with_context(|| format!("write SUMMARY.md: {}", out_dir.display()))?;

    let chapters_in_order = toc
        .parts
        .iter()
        .flat_map(|part| part.chapters.iter())
        .collect::<Vec<_>>();
    if chapters_in_order.is_empty() {
        return Ok(());
    }
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(chapters_in_order.len());

    let engine = args.engine;
    let language = args.language.as_str();
    let tone = args.tone.as_str();
    let manifest = &manifest;
    let url_to_location = &url_to_location;
    let dir_index_ids = &dir_index_ids;
    let assets = &assets;

    let next_idx = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|scope| -> anyhow::Result<()> {
        let chapters_in_order = &chapters_in_order;
        let mut handles = Vec::new();

        for _ in 0..worker_count {
            let chapters_dir = chapters_dir.clone();
            let next_idx = Arc::clone(&next_idx);
            handles.push(scope.spawn(move || -> anyhow::Result<()> {
                loop {
                    let idx = next_idx.fetch_add(1, Ordering::Relaxed);
                    let Some(chapter) = chapters_in_order.get(idx) else {
                        break;
                    };

                    let chapter_id = chapter.id.clone();
                    let ctx = ChapterRenderContext {
                        engine,
                        language,
                        tone,
                        manifest,
                        url_to_location,
                        dir_index_ids,
                        assets,
                    };

                    let chapter_md = render_chapter_md(chapter, &ctx)
                        .with_context(|| format!("render chapter: {}", chapter_id))?;
                    std::fs::write(chapters_dir.join(format!("{}.md", chapter_id)), chapter_md)
                        .with_context(|| format!("write chapter: {}", chapter_id))?;
                }

                Ok(())
            }));
        }

        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("chapter render thread panicked"))??;
        }

        Ok(())
    })?;

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

    let bundled = rewrite_bundled_internal_links(&bundled);
    copy_assets_for_bundle(&src_dir.join("assets"), &out_path, args.force)
        .context("copy assets for bundle")?;

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

pub fn epub(args: BookEpubArgs) -> anyhow::Result<()> {
    let book_dir = PathBuf::from(&args.book);
    let out_path = PathBuf::from(&args.out);

    crate::epub::create_from_mdbook(
        &book_dir,
        &out_path,
        &crate::epub::CreateEpubOptions {
            force: args.force,
            lang: args.lang,
        },
    )
    .context("create epub from mdBook")
}

fn copy_assets_for_bundle(
    src_assets_dir: &Path,
    out_path: &Path,
    force: bool,
) -> anyhow::Result<()> {
    if !src_assets_dir.exists() {
        return Ok(());
    }

    let dest_assets_dir = match out_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("assets"),
        _ => PathBuf::from("assets"),
    };

    if dest_assets_dir.exists() && force {
        std::fs::remove_dir_all(&dest_assets_dir).with_context(|| {
            format!(
                "remove existing bundle assets dir: {}",
                dest_assets_dir.display()
            )
        })?;
    }

    std::fs::create_dir_all(&dest_assets_dir)
        .with_context(|| format!("create bundle assets dir: {}", dest_assets_dir.display()))?;

    copy_dir_recursive_skip_existing(src_assets_dir, &dest_assets_dir)
        .with_context(|| format!("copy assets from {}", src_assets_dir.display()))?;

    Ok(())
}

fn copy_dir_recursive_skip_existing(src: &Path, dest: &Path) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(src).with_context(|| format!("read dir: {}", src.display()))? {
        let entry = entry.context("read dir entry")?;
        let src_path = entry.path();
        let file_type = entry.file_type().context("read file type")?;
        let name = entry.file_name();
        let dest_path = dest.join(name);

        if file_type.is_dir() {
            std::fs::create_dir_all(&dest_path)
                .with_context(|| format!("create dir: {}", dest_path.display()))?;
            copy_dir_recursive_skip_existing(&src_path, &dest_path)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        if dest_path.exists() {
            continue;
        }
        std::fs::copy(&src_path, &dest_path).with_context(|| {
            format!(
                "copy file {} -> {}",
                src_path.display(),
                dest_path.display()
            )
        })?;
    }

    Ok(())
}

fn rewrite_bundled_internal_links(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut in_fence = false;
    let mut fence_marker = String::new();

    for line in markdown.split_inclusive('\n') {
        if !in_fence {
            if let Some(marker) = fence_start_marker(line) {
                in_fence = true;
                fence_marker.clear();
                fence_marker.push_str(marker);
                out.push_str(line);
                continue;
            }
            out.push_str(&rewrite_inline_bundled_line(line));
            continue;
        }

        out.push_str(line);
        if fence_end_marker(line, &fence_marker) {
            in_fence = false;
        }
    }

    out
}

fn rewrite_inline_bundled_line(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < input.len() {
        let rest = &input[i..];

        if rest.starts_with('`')
            && let Some(consumed) = consume_code_span(rest)
        {
            out.push_str(&rest[..consumed]);
            i += consumed;
            continue;
        }

        if rest.starts_with("![")
            && let Some((consumed, rewritten)) = try_rewrite_bundled_link_like(rest, true)
        {
            out.push_str(&rewritten);
            i += consumed;
            continue;
        }

        if rest.starts_with('[')
            && let Some((consumed, rewritten)) = try_rewrite_bundled_link_like(rest, false)
        {
            out.push_str(&rewritten);
            i += consumed;
            continue;
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn try_rewrite_bundled_link_like(input: &str, is_image: bool) -> Option<(usize, String)> {
    let mut i = if is_image { 2 } else { 1 };
    let mut bracket_depth = 1u32;

    while i < input.len() {
        let rest = &input[i..];
        let ch = rest.chars().next().unwrap();

        if ch == '\\' {
            i += ch.len_utf8();
            if i < input.len() {
                let next = input[i..].chars().next().unwrap();
                i += next.len_utf8();
            }
            continue;
        }

        if ch == '[' {
            bracket_depth += 1;
        } else if ch == ']' {
            bracket_depth = bracket_depth.saturating_sub(1);
            if bracket_depth == 0 {
                break;
            }
        }
        i += ch.len_utf8();
    }

    if bracket_depth != 0 {
        return None;
    }
    let close_bracket = i;
    let after = &input[close_bracket + 1..];
    let mut after_idx = 0usize;
    while after_idx < after.len() {
        let ch = after[after_idx..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        after_idx += ch.len_utf8();
    }
    if !after[after_idx..].starts_with('(') {
        return None;
    }

    let paren_open = close_bracket + 1 + after_idx;
    let mut j = paren_open + 1;
    let mut paren_depth = 1u32;

    while j < input.len() {
        let rest = &input[j..];
        let ch = rest.chars().next().unwrap();

        if ch == '\\' {
            j += ch.len_utf8();
            if j < input.len() {
                let next = input[j..].chars().next().unwrap();
                j += next.len_utf8();
            }
            continue;
        }

        if ch == '(' {
            paren_depth += 1;
        } else if ch == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            if paren_depth == 0 {
                break;
            }
        }
        j += ch.len_utf8();
    }

    if paren_depth != 0 {
        return None;
    }
    let paren_close = j;

    let dest = &input[paren_open + 1..paren_close];
    let rewritten_dest = rewrite_bundled_link_destination(dest);

    let mut rewritten = String::with_capacity(paren_close + 1);
    rewritten.push_str(&input[..paren_open + 1]);
    rewritten.push_str(&rewritten_dest);
    rewritten.push(')');

    Some((paren_close + 1, rewritten))
}

fn rewrite_bundled_link_destination(dest: &str) -> String {
    let mut i = 0usize;
    while i < dest.len() {
        let ch = dest[i..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }

    let (core_start, core_end) = if dest[i..].starts_with('<') {
        let core_start = i + 1;
        let after = &dest[core_start..];
        let Some(rel_end) = after.find('>') else {
            return dest.to_owned();
        };
        (core_start, core_start + rel_end)
    } else {
        let mut end = i;
        while end < dest.len() {
            let ch = dest[end..].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            end += ch.len_utf8();
        }
        (i, end)
    };

    let core = &dest[core_start..core_end];
    let (core_inner, brace_wrapped) =
        if core.starts_with('{') && core.ends_with('}') && core.len() >= 2 {
            (&core[1..core.len() - 1], true)
        } else {
            (core, false)
        };

    let rewritten_inner = if let Some(stripped) = core_inner.strip_prefix("../assets/") {
        format!("assets/{stripped}")
    } else if let Some((chapter_ref, fragment)) = core_inner.split_once(".md#")
        && chapter_ref.len() == 4
        && chapter_ref.starts_with("ch")
        && chapter_ref[2..].chars().all(|c| c.is_ascii_digit())
        && fragment.starts_with("p_")
    {
        format!("#{fragment}")
    } else {
        core_inner.to_owned()
    };

    let rewritten = if brace_wrapped {
        format!("{{{rewritten_inner}}}")
    } else {
        rewritten_inner
    };

    if rewritten == core {
        return dest.to_owned();
    }

    let mut out = String::with_capacity(dest.len());
    out.push_str(&dest[..core_start]);
    out.push_str(&rewritten);
    out.push_str(&dest[core_end..]);
    out
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

struct ChapterRenderContext<'a> {
    engine: LlmEngine,
    language: &'a str,
    tone: &'a str,
    manifest: &'a HashMap<String, ManifestRecord>,
    url_to_location: &'a HashMap<String, PageLocation>,
    dir_index_ids: &'a HashSet<String>,
    assets: &'a AssetDownloader,
}

fn render_chapter_md(
    chapter: &crate::formats::TocChapter,
    ctx: &ChapterRenderContext<'_>,
) -> anyhow::Result<String> {
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", chapter.title));

    let mut chapter_source_ids_in_order = Vec::new();
    let mut chapter_source_ids_seen = HashSet::new();

    for section in &chapter.sections {
        if section.title.trim().is_empty() {
            continue;
        }

        md.push_str(&format!("## {}\n\n", section.title.trim()));

        // Insert stable anchors for each referenced source page id (for internal link rewriting).
        for source_id in &section.sources {
            if chapter_source_ids_seen.insert(source_id.clone()) {
                chapter_source_ids_in_order.push(source_id.clone());
            }
            md.push_str(&format!("<a id=\"{source_id}\"></a>\n"));
        }
        md.push('\n');

        let mut source_material = String::new();
        for source_id in &section.sources {
            let record = ctx
                .manifest
                .get(source_id)
                .ok_or_else(|| anyhow::anyhow!("source id not found in manifest: {source_id}"))?;

            let extracted = std::fs::read_to_string(&record.extracted_md).with_context(|| {
                format!(
                    "read extracted page for {}: {}",
                    chapter.id, record.extracted_md
                )
            })?;
            let body = strip_front_matter(&extracted).context("strip front matter")?;
            let body = strip_leading_h1(body);
            let body = rewrite_markdown_links_and_images(
                body,
                &record.url,
                &chapter.id,
                ctx.url_to_location,
                ctx.dir_index_ids.contains(&record.id),
                ctx.assets,
            )
            .with_context(|| format!("rewrite links/images for {}", record.url))?;

            if !source_material.is_empty() && !source_material.ends_with('\n') {
                source_material.push('\n');
            }
            if !source_material.is_empty() {
                source_material.push('\n');
            }
            source_material.push_str(&format!("### {}\n\n", record.title));
            source_material.push_str(body.trim());
            source_material.push('\n');
        }

        let section_body = match ctx.engine {
            LlmEngine::Noop => source_material.trim_end().to_owned(),
            LlmEngine::Openai => rewrite::rewrite_section_via_openai(
                ctx.language,
                ctx.tone,
                &chapter.title,
                &section.title,
                source_material.trim_end(),
            )
            .with_context(|| {
                format!("openai rewrite section: {} / {}", chapter.id, section.title)
            })?,
        };

        if !section_body.trim().is_empty() {
            md.push_str(section_body.trim_end());
            md.push_str("\n\n");
        }
    }

    md.push_str("## Sources\n");
    for source_id in &chapter_source_ids_in_order {
        let record = ctx
            .manifest
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("source id not found in manifest: {source_id}"))?;
        md.push_str(&format!("- {}\n", record.url));
    }

    Ok(md)
}

#[derive(Debug, Clone)]
struct PageLocation {
    chapter_id: String,
    page_id: String,
}

fn build_url_to_location(
    toc: &Toc,
    manifest: &HashMap<String, ManifestRecord>,
) -> HashMap<String, PageLocation> {
    let mut map = HashMap::new();
    for part in &toc.parts {
        for chapter in &part.chapters {
            for section in &chapter.sections {
                for source_id in &section.sources {
                    let Some(record) = manifest.get(source_id) else {
                        continue;
                    };
                    map.insert(
                        record.url.clone(),
                        PageLocation {
                            chapter_id: chapter.id.clone(),
                            page_id: record.id.clone(),
                        },
                    );
                }
            }
        }
    }
    map
}

fn compute_dir_index_ids<'a>(
    records: impl IntoIterator<Item = &'a ManifestRecord>,
) -> HashSet<String> {
    let records = records.into_iter().collect::<Vec<_>>();
    let mut ids = HashSet::new();

    for record in &records {
        let prefix = if record.path == "/" {
            "/".to_owned()
        } else {
            format!("{}/", record.path.trim_end_matches('/'))
        };
        if records
            .iter()
            .any(|other| other.path != record.path && other.path.starts_with(&prefix))
        {
            ids.insert(record.id.clone());
        }
    }

    ids
}

struct AssetDownloader {
    client: reqwest::blocking::Client,
    assets_dir: PathBuf,
    cache: Arc<Mutex<HashMap<String, String>>>,
}

impl AssetDownloader {
    fn new(assets_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&assets_dir).with_context(|| {
            format!("create book asset dir: {}", assets_dir.as_path().display())
        })?;

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("build asset download http client")?;

        Ok(Self {
            client,
            assets_dir,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn download_image(&self, url: &Url) -> anyhow::Result<String> {
        let key = normalize_asset_url_key(url);
        if let Ok(cache) = self.cache.lock()
            && let Some(cached) = cache.get(&key)
        {
            return Ok(cached.to_owned());
        }

        if url.scheme() != "http" && url.scheme() != "https" {
            anyhow::bail!(
                "unsupported url scheme for asset download: {}",
                url.scheme()
            );
        }

        let hash = sha256_hex(&key);
        if let Some(ext) = image_extension_from_path(url) {
            let file_name = format!("img_{hash}.{ext}");
            let local = format!("../assets/{file_name}");
            let dest_path = self.assets_dir.join(&file_name);
            if dest_path.exists() {
                if let Ok(mut cache) = self.cache.lock() {
                    cache.insert(key, local.clone());
                }
                return Ok(local);
            }
            self.download_to(&key, url, &dest_path)
                .with_context(|| format!("download image: {url}"))?;
            if let Ok(mut cache) = self.cache.lock() {
                cache.insert(key, local.clone());
            }
            return Ok(local);
        }

        let response = self
            .client
            .get(url.as_str())
            .send()
            .with_context(|| format!("GET {url}"))?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("asset download failed ({status})");
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        let ext = content_type
            .and_then(image_extension_from_content_type)
            .unwrap_or("bin");

        let file_name = format!("img_{hash}.{ext}");
        let local = format!("../assets/{file_name}");
        let dest_path = self.assets_dir.join(&file_name);
        if dest_path.exists() {
            if let Ok(mut cache) = self.cache.lock() {
                cache.insert(key, local.clone());
            }
            return Ok(local);
        }

        let bytes = response.bytes().context("read asset response body")?;
        write_file_if_missing(&dest_path, &bytes)
            .with_context(|| format!("write asset: {}", dest_path.display()))?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key, local.clone());
        }
        Ok(local)
    }

    fn download_to(&self, key: &str, url: &Url, dest_path: &Path) -> anyhow::Result<()> {
        tracing::info!(url = %url, path = %dest_path.display(), "download asset");

        if dest_path.exists() {
            return Ok(());
        }

        let response = self
            .client
            .get(url.as_str())
            .send()
            .with_context(|| format!("GET {url}"))?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("asset download failed ({status})");
        }

        let bytes = response.bytes().context("read asset response body")?;
        if bytes.is_empty() {
            anyhow::bail!("asset download returned empty body");
        }

        let expected_hash = sha256_hex(key);
        if !dest_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.contains(&expected_hash))
            .unwrap_or(false)
        {
            anyhow::bail!("refusing to write asset with unexpected name");
        }

        write_file_if_missing(dest_path, &bytes)
            .with_context(|| format!("write asset: {}", dest_path.display()))?;
        Ok(())
    }
}

fn normalize_asset_url_key(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_fragment(None);
    normalized.to_string()
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

fn image_extension_from_path(url: &Url) -> Option<&'static str> {
    let ext = Path::new(url.path()).extension()?.to_str()?;
    let ext = ext.trim().to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "gif" => Some("gif"),
        "svg" => Some("svg"),
        "webp" => Some("webp"),
        "avif" => Some("avif"),
        "bmp" => Some("bmp"),
        _ => None,
    }
}

fn image_extension_from_content_type(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next()?.trim().to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/svg+xml" => Some("svg"),
        "image/webp" => Some("webp"),
        "image/avif" => Some("avif"),
        "image/bmp" => Some("bmp"),
        _ => None,
    }
}

fn write_file_if_missing(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create asset dir: {}", parent.display()))?;
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(bytes)
                .with_context(|| format!("write asset file: {}", path.display()))?;
            file.flush()
                .with_context(|| format!("flush asset file: {}", path.display()))?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

fn rewrite_markdown_links_and_images(
    body: &str,
    page_url: &str,
    chapter_id: &str,
    url_to_location: &HashMap<String, PageLocation>,
    page_is_dir_index: bool,
    assets: &AssetDownloader,
) -> anyhow::Result<String> {
    let base_url = Url::parse(page_url).context("parse page url")?;
    let base_for_join = if page_is_dir_index {
        url_with_trailing_slash(&base_url)
    } else {
        base_url.clone()
    };

    let mut out = String::with_capacity(body.len());
    let mut in_fence = false;
    let mut fence_marker = String::new();

    for line in body.split_inclusive('\n') {
        if !in_fence {
            if let Some(marker) = fence_start_marker(line) {
                in_fence = true;
                fence_marker.clear();
                fence_marker.push_str(marker);
                out.push_str(line);
                continue;
            }
            out.push_str(&rewrite_inline_markdown(
                line,
                &base_for_join,
                chapter_id,
                url_to_location,
                assets,
            )?);
            continue;
        }

        out.push_str(line);
        if fence_end_marker(line, &fence_marker) {
            in_fence = false;
        }
    }

    Ok(out)
}

fn rewrite_inline_markdown(
    input: &str,
    base_url: &Url,
    current_chapter_id: &str,
    url_to_location: &HashMap<String, PageLocation>,
    assets: &AssetDownloader,
) -> anyhow::Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < input.len() {
        let rest = &input[i..];

        if rest.starts_with('`')
            && let Some(consumed) = consume_code_span(rest)
        {
            out.push_str(&rest[..consumed]);
            i += consumed;
            continue;
        }

        if rest.starts_with("![")
            && let Some((consumed, rewritten)) = try_rewrite_link_like(
                rest,
                true,
                base_url,
                current_chapter_id,
                url_to_location,
                assets,
            )?
        {
            out.push_str(&rewritten);
            i += consumed;
            continue;
        }

        if rest.starts_with('[')
            && let Some((consumed, rewritten)) = try_rewrite_link_like(
                rest,
                false,
                base_url,
                current_chapter_id,
                url_to_location,
                assets,
            )?
        {
            out.push_str(&rewritten);
            i += consumed;
            continue;
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    Ok(out)
}

fn consume_code_span(input: &str) -> Option<usize> {
    let marker_len = input.chars().take_while(|c| *c == '`').count();
    if marker_len == 0 {
        return None;
    }
    let marker = "`".repeat(marker_len);
    let after_open = &input[marker_len..];
    let close = after_open.find(&marker)?;
    Some(marker_len + close + marker_len)
}

fn try_rewrite_link_like(
    input: &str,
    is_image: bool,
    base_url: &Url,
    current_chapter_id: &str,
    url_to_location: &HashMap<String, PageLocation>,
    assets: &AssetDownloader,
) -> anyhow::Result<Option<(usize, String)>> {
    let mut i = if is_image { 2 } else { 1 };
    let mut bracket_depth = 1u32;

    while i < input.len() {
        let rest = &input[i..];
        let ch = rest.chars().next().unwrap();

        if ch == '\\' {
            i += ch.len_utf8();
            if i < input.len() {
                let next = input[i..].chars().next().unwrap();
                i += next.len_utf8();
            }
            continue;
        }

        if ch == '[' {
            bracket_depth += 1;
        } else if ch == ']' {
            bracket_depth = bracket_depth.saturating_sub(1);
            if bracket_depth == 0 {
                break;
            }
        }
        i += ch.len_utf8();
    }

    if bracket_depth != 0 {
        return Ok(None);
    }
    let close_bracket = i;
    let after = &input[close_bracket + 1..];
    let mut after_idx = 0usize;
    while after_idx < after.len() {
        let ch = after[after_idx..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        after_idx += ch.len_utf8();
    }
    if !after[after_idx..].starts_with('(') {
        return Ok(None);
    }

    let paren_open = close_bracket + 1 + after_idx;
    let mut j = paren_open + 1;
    let mut paren_depth = 1u32;

    while j < input.len() {
        let rest = &input[j..];
        let ch = rest.chars().next().unwrap();

        if ch == '\\' {
            j += ch.len_utf8();
            if j < input.len() {
                let next = input[j..].chars().next().unwrap();
                j += next.len_utf8();
            }
            continue;
        }

        if ch == '(' {
            paren_depth += 1;
        } else if ch == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            if paren_depth == 0 {
                break;
            }
        }
        j += ch.len_utf8();
    }

    if paren_depth != 0 {
        return Ok(None);
    }
    let paren_close = j;

    let dest = &input[paren_open + 1..paren_close];
    let rewritten_dest = rewrite_link_destination(
        dest,
        is_image,
        base_url,
        current_chapter_id,
        url_to_location,
        assets,
    )?;

    let mut rewritten = String::with_capacity(paren_close + 1);
    rewritten.push_str(&input[..paren_open + 1]);
    rewritten.push_str(&rewritten_dest);
    rewritten.push_str(&input[paren_close..=paren_close]);

    Ok(Some((paren_close + 1, rewritten)))
}

fn rewrite_link_destination(
    dest: &str,
    is_image: bool,
    base_url: &Url,
    current_chapter_id: &str,
    url_to_location: &HashMap<String, PageLocation>,
    assets: &AssetDownloader,
) -> anyhow::Result<String> {
    let mut i = 0usize;
    while i < dest.len() {
        let ch = dest[i..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }

    let (core_start, core_end) = if dest[i..].starts_with('<') {
        let core_start = i + 1;
        let after = &dest[core_start..];
        let Some(rel_end) = after.find('>') else {
            return Ok(dest.to_owned());
        };
        (core_start, core_start + rel_end)
    } else {
        let mut end = i;
        while end < dest.len() {
            let ch = dest[end..].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            end += ch.len_utf8();
        }
        (i, end)
    };

    let core = &dest[core_start..core_end];
    let rewritten = if is_image {
        match resolve_url_for_output(base_url, core) {
            Some(resolved) => match assets.download_image(&resolved) {
                Ok(local) => local,
                Err(err) => {
                    tracing::debug!(url = %resolved, ?err, "image download failed; using URL");
                    resolved.to_string()
                }
            },
            None => core.to_owned(),
        }
    } else {
        rewrite_page_link(base_url, core, current_chapter_id, url_to_location)?
    };

    let mut out = String::with_capacity(dest.len() + 16);
    out.push_str(&dest[..core_start]);
    out.push_str(&rewritten);
    out.push_str(&dest[core_end..]);
    Ok(out)
}

fn rewrite_page_link(
    base_url: &Url,
    raw: &str,
    current_chapter_id: &str,
    url_to_location: &HashMap<String, PageLocation>,
) -> anyhow::Result<String> {
    if raw.is_empty() || raw.starts_with('#') {
        return Ok(raw.to_owned());
    }
    if raw.starts_with("mailto:") || raw.starts_with("javascript:") {
        return Ok(raw.to_owned());
    }

    let Some(resolved) = resolve_url_for_output(base_url, raw) else {
        return Ok(raw.to_owned());
    };
    let canonical = canonicalize_url_for_lookup(&resolved);
    if let Some(loc) = url_to_location.get(canonical.as_str()) {
        if loc.chapter_id == current_chapter_id {
            return Ok(format!("#{}", loc.page_id));
        }
        return Ok(format!("{}.md#{}", loc.chapter_id, loc.page_id));
    }

    Ok(resolved.to_string())
}

fn resolve_url_for_output(base_url: &Url, raw: &str) -> Option<Url> {
    if let Ok(url) = Url::parse(raw) {
        return Some(url);
    }
    if raw.starts_with("//") {
        let scheme = base_url.scheme();
        return Url::parse(&format!("{scheme}:{raw}")).ok();
    }

    base_url.join(raw).ok()
}

fn canonicalize_url_for_lookup(url: &Url) -> String {
    let mut canonical = url.clone();
    canonical.set_fragment(None);
    canonical.set_query(None);

    let mut path = canonical.path().to_owned();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    canonical.set_path(&path);
    canonical.to_string()
}

fn url_with_trailing_slash(url: &Url) -> Url {
    let mut out = url.clone();
    let path = out.path();
    if !path.ends_with('/') {
        out.set_path(&format!("{path}/"));
    }
    out
}

fn fence_start_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        let len = trimmed.chars().take_while(|c| *c == '`').count();
        return Some(&trimmed[..len]);
    }
    if trimmed.starts_with("~~~") {
        let len = trimmed.chars().take_while(|c| *c == '~').count();
        return Some(&trimmed[..len]);
    }
    None
}

fn fence_end_marker(line: &str, marker: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with(marker)
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
