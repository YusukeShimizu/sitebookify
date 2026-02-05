use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::Utc;
use pulldown_cmark::{Options, Parser};
use zip::write::SimpleFileOptions;

#[derive(Debug, Clone)]
pub struct CreateEpubOptions {
    pub force: bool,
    /// BCP-47 language tag used for EPUB metadata and XHTML documents.
    pub lang: String,
}

impl Default for CreateEpubOptions {
    fn default() -> Self {
        Self {
            force: false,
            lang: "und".to_string(),
        }
    }
}

pub fn guess_lang_tag(user_language: &str) -> String {
    let raw = user_language.trim();
    if raw.is_empty() {
        return "und".to_string();
    }

    // If the user already passed a plausible BCP-47 tag, keep it.
    let looks_like_tag = raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && raw.chars().any(|c| c.is_ascii_alphabetic())
        && raw.len() <= 35;
    if looks_like_tag && raw.contains('-') {
        return raw.replace('_', "-");
    }

    let lower = raw.to_ascii_lowercase();
    if raw.contains("日本") || lower.contains("japanese") || lower == "ja" {
        return "ja".to_string();
    }
    if raw.contains("英") || lower.contains("english") || lower == "en" {
        return "en".to_string();
    }

    "und".to_string()
}

pub fn create_from_mdbook(
    book_dir: &Path,
    out_path: &Path,
    options: &CreateEpubOptions,
) -> anyhow::Result<()> {
    if !book_dir.is_dir() {
        anyhow::bail!("book directory not found: {}", book_dir.display());
    }

    if out_path.exists() && !options.force {
        anyhow::bail!("epub output already exists: {}", out_path.display());
    }
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create epub parent dir: {}", parent.display()))?;
    }

    let title = read_book_title(book_dir)?.unwrap_or_else(|| "Book".to_string());
    let lang = options.lang.trim();
    let lang = if lang.is_empty() { "und" } else { lang };

    let src_dir = book_dir.join("src");
    let summary_path = src_dir.join("SUMMARY.md");
    let summary_md = fs::read_to_string(&summary_path)
        .with_context(|| format!("read SUMMARY.md: {}", summary_path.display()))?;

    let chapter_rel_paths = parse_summary_chapter_paths(&summary_md);
    if chapter_rel_paths.is_empty() {
        anyhow::bail!(
            "no chapter links found in SUMMARY.md: {}",
            summary_path.display()
        );
    }

    let chapters = chapter_rel_paths
        .into_iter()
        .map(|rel| {
            let md_path = src_dir.join(&rel);
            let stem = md_path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("invalid chapter filename: {}", md_path.display()))?
                .to_string();
            let md = fs::read_to_string(&md_path)
                .with_context(|| format!("read chapter: {}", md_path.display()))?;
            let title = extract_first_heading(&md).unwrap_or_else(|| stem.clone());
            anyhow::Ok(ChapterSpec { stem, title, md })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let assets_dir = src_dir.join("assets");
    let assets = if assets_dir.exists() {
        list_files_recursively_sorted(&assets_dir)
            .with_context(|| format!("list assets: {}", assets_dir.display()))?
            .into_iter()
            .map(|path| {
                let rel_path = path
                    .strip_prefix(&assets_dir)
                    .with_context(|| format!("strip asset prefix: {}", path.display()))?;
                let rel_str = rel_path.to_string_lossy().replace('\\', "/");
                anyhow::Ok(AssetSpec {
                    rel_path: rel_str,
                    abs_path: path,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };

    let uuid = uuid::Uuid::new_v4();
    let modified = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let container_xml = render_container_xml();
    let css = default_style_css();
    let nav_xhtml = render_nav_xhtml(&title, lang, &chapters);
    let toc_ncx = render_toc_ncx(&title, uuid, &chapters);
    let content_opf = render_content_opf(&title, lang, uuid, &modified, &chapters, &assets);

    let mut out_options = OpenOptions::new();
    out_options.write(true);
    if options.force {
        out_options.create(true).truncate(true);
    } else {
        out_options.create_new(true);
    }
    let out_file = out_options
        .open(out_path)
        .with_context(|| format!("open epub output: {}", out_path.display()))?;

    let mut zip = zip::ZipWriter::new(out_file);

    // Per EPUB spec, `mimetype` MUST be the first entry and MUST be stored (no compression).
    let mimetype_options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    zip.start_file("mimetype", mimetype_options)
        .context("epub start_file mimetype")?;
    zip.write_all(b"application/epub+zip")
        .context("epub write mimetype")?;

    let deflated_options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    zip.start_file("META-INF/container.xml", deflated_options)
        .context("epub start_file container.xml")?;
    zip.write_all(container_xml.as_bytes())
        .context("epub write container.xml")?;

    zip.start_file("OEBPS/content.opf", deflated_options)
        .context("epub start_file content.opf")?;
    zip.write_all(content_opf.as_bytes())
        .context("epub write content.opf")?;

    zip.start_file("OEBPS/nav.xhtml", deflated_options)
        .context("epub start_file nav.xhtml")?;
    zip.write_all(nav_xhtml.as_bytes())
        .context("epub write nav.xhtml")?;

    zip.start_file("OEBPS/toc.ncx", deflated_options)
        .context("epub start_file toc.ncx")?;
    zip.write_all(toc_ncx.as_bytes())
        .context("epub write toc.ncx")?;

    zip.start_file("OEBPS/style.css", deflated_options)
        .context("epub start_file style.css")?;
    zip.write_all(css.as_bytes())
        .context("epub write style.css")?;

    let chapter_stems = chapters.iter().map(|c| c.stem.as_str()).collect::<Vec<_>>();
    for chapter in &chapters {
        let html = markdown_to_html_fragment(&chapter.md);
        let html = rewrite_html_for_epub(&html, &chapter_stems);
        let html = ensure_xhtml_void_tags(&html);
        let xhtml = wrap_xhtml_document(&chapter.title, lang, &html);

        zip.start_file(format!("OEBPS/{}.xhtml", chapter.stem), deflated_options)
            .with_context(|| format!("epub start_file chapter: {}", chapter.stem))?;
        zip.write_all(xhtml.as_bytes())
            .with_context(|| format!("epub write chapter: {}", chapter.stem))?;
    }

    for asset in &assets {
        let mut f = fs::File::open(&asset.abs_path)
            .with_context(|| format!("open asset: {}", asset.abs_path.display()))?;
        zip.start_file(format!("OEBPS/assets/{}", asset.rel_path), deflated_options)
            .with_context(|| format!("epub start_file asset: {}", asset.rel_path))?;
        std::io::copy(&mut f, &mut zip)
            .with_context(|| format!("epub write asset: {}", asset.rel_path))?;
    }

    zip.finish().context("epub finish zip")?;
    Ok(())
}

#[derive(Debug)]
struct ChapterSpec {
    stem: String,
    title: String,
    md: String,
}

#[derive(Debug)]
struct AssetSpec {
    rel_path: String,
    abs_path: PathBuf,
}

fn render_container_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#
    .to_string()
}

fn default_style_css() -> String {
    r#"@charset "utf-8";

html { font-family: serif; }
body { margin: 0; padding: 0 1.2em; line-height: 1.6; }
img { max-width: 100%; height: auto; }
pre, code { font-family: ui-monospace, Menlo, Consolas, monospace; }
pre { overflow-x: auto; padding: 0.75em; background: #f6f8fa; border-radius: 6px; }
blockquote { margin: 1em 0; padding: 0 1em; border-left: 4px solid #ddd; color: #333; }
"#
    .to_string()
}

fn render_nav_xhtml(title: &str, lang: &str, chapters: &[ChapterSpec]) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<!DOCTYPE html>\n");
    out.push_str(&format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{}\" xml:lang=\"{}\">\n",
        xml_escape(lang),
        xml_escape(lang)
    ));
    out.push_str("<head>\n");
    out.push_str(&format!("  <title>{}</title>\n", xml_escape(title)));
    out.push_str("  <meta charset=\"utf-8\" />\n");
    out.push_str("  <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\" />\n");
    out.push_str("</head>\n");
    out.push_str("<body>\n");
    out.push_str(&format!("  <h1>{}</h1>\n", xml_escape(title)));
    out.push_str("  <nav epub:type=\"toc\" id=\"toc\">\n");
    out.push_str("    <ol>\n");
    for ch in chapters {
        out.push_str(&format!(
            "      <li><a href=\"{}.xhtml\">{}</a></li>\n",
            xml_escape(&ch.stem),
            xml_escape(&ch.title)
        ));
    }
    out.push_str("    </ol>\n");
    out.push_str("  </nav>\n");
    out.push_str("</body>\n");
    out.push_str("</html>\n");
    out
}

fn render_toc_ncx(title: &str, uuid: uuid::Uuid, chapters: &[ChapterSpec]) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str(
        "<!DOCTYPE ncx PUBLIC \"-//NISO//DTD ncx 2005-1//EN\" \"http://www.daisy.org/z3986/2005/ncx-2005-1.dtd\">\n",
    );
    out.push_str("<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n");
    out.push_str("  <head>\n");
    out.push_str(&format!(
        "    <meta name=\"dtb:uid\" content=\"urn:uuid:{}\" />\n",
        xml_escape(&uuid.to_string())
    ));
    out.push_str("    <meta name=\"dtb:depth\" content=\"1\" />\n");
    out.push_str("    <meta name=\"dtb:totalPageCount\" content=\"0\" />\n");
    out.push_str("    <meta name=\"dtb:maxPageNumber\" content=\"0\" />\n");
    out.push_str("  </head>\n");
    out.push_str("  <docTitle><text>");
    out.push_str(&xml_escape(title));
    out.push_str("</text></docTitle>\n");
    out.push_str("  <navMap>\n");
    for (idx, ch) in chapters.iter().enumerate() {
        let play = idx + 1;
        out.push_str(&format!(
            "    <navPoint id=\"navPoint-{}\" playOrder=\"{}\">\n",
            play, play
        ));
        out.push_str("      <navLabel><text>");
        out.push_str(&xml_escape(&ch.title));
        out.push_str("</text></navLabel>\n");
        out.push_str(&format!(
            "      <content src=\"{}.xhtml\" />\n",
            xml_escape(&ch.stem)
        ));
        out.push_str("    </navPoint>\n");
    }
    out.push_str("  </navMap>\n");
    out.push_str("</ncx>\n");
    out
}

fn render_content_opf(
    title: &str,
    lang: &str,
    uuid: uuid::Uuid,
    modified: &str,
    chapters: &[ChapterSpec],
    assets: &[AssetSpec],
) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str(&format!(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"bookid\" version=\"3.0\" xml:lang=\"{}\">\n",
        xml_escape(lang)
    ));
    out.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n");
    out.push_str(&format!(
        "    <dc:identifier id=\"bookid\">urn:uuid:{}</dc:identifier>\n",
        xml_escape(&uuid.to_string())
    ));
    out.push_str(&format!("    <dc:title>{}</dc:title>\n", xml_escape(title)));
    out.push_str(&format!(
        "    <dc:language>{}</dc:language>\n",
        xml_escape(lang)
    ));
    out.push_str(&format!(
        "    <meta property=\"dcterms:modified\">{}</meta>\n",
        xml_escape(modified)
    ));
    out.push_str("  </metadata>\n");
    out.push_str("  <manifest>\n");
    out.push_str(
        "    <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\" />\n",
    );
    out.push_str(
        "    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\" />\n",
    );
    out.push_str("    <item id=\"css\" href=\"style.css\" media-type=\"text/css\" />\n");

    for ch in chapters {
        out.push_str(&format!(
            "    <item id=\"{}\" href=\"{}.xhtml\" media-type=\"application/xhtml+xml\" />\n",
            xml_escape(&ch.stem),
            xml_escape(&ch.stem)
        ));
    }

    for (idx, asset) in assets.iter().enumerate() {
        let media_type = media_type_for_asset(&asset.rel_path);
        out.push_str(&format!(
            "    <item id=\"asset-{}\" href=\"assets/{}\" media-type=\"{}\" />\n",
            idx + 1,
            xml_escape(&asset.rel_path),
            xml_escape(media_type)
        ));
    }

    out.push_str("  </manifest>\n");
    out.push_str("  <spine toc=\"ncx\">\n");
    for ch in chapters {
        out.push_str(&format!(
            "    <itemref idref=\"{}\" />\n",
            xml_escape(&ch.stem)
        ));
    }
    out.push_str("  </spine>\n");
    out.push_str("</package>\n");
    out
}

fn media_type_for_asset(rel_path: &str) -> &'static str {
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

fn wrap_xhtml_document(title: &str, lang: &str, body_html: &str) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<!DOCTYPE html>\n");
    out.push_str(&format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"{}\" xml:lang=\"{}\">\n",
        xml_escape(lang),
        xml_escape(lang)
    ));
    out.push_str("<head>\n");
    out.push_str(&format!("  <title>{}</title>\n", xml_escape(title)));
    out.push_str("  <meta charset=\"utf-8\" />\n");
    out.push_str("  <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\" />\n");
    out.push_str("</head>\n");
    out.push_str("<body>\n");
    out.push_str(body_html);
    if !body_html.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("</body>\n");
    out.push_str("</html>\n");
    out
}

fn markdown_to_html_fragment(md: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(md, options);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}

fn rewrite_html_for_epub(html: &str, chapter_stems: &[&str]) -> String {
    let mut out = html.to_string();

    // Assets are stored under `OEBPS/assets/` and referenced as `assets/...` from each chapter.
    out = out.replace("src=\"../assets/", "src=\"assets/");
    out = out.replace("src='../assets/", "src='assets/");
    out = out.replace("href=\"../assets/", "href=\"assets/");
    out = out.replace("href='../assets/", "href='assets/");

    // Chapter links inside the mdBook output commonly look like `chXX.md#...` (same directory).
    // In EPUB we emit `chXX.xhtml`.
    for stem in chapter_stems {
        let md = format!("{stem}.md");
        let xhtml = format!("{stem}.xhtml");

        out = out.replace(&format!("href=\"chapters/{md}"), &format!("href=\"{xhtml}"));
        out = out.replace(
            &format!("href=\"./chapters/{md}"),
            &format!("href=\"{xhtml}"),
        );
        out = out.replace(&format!("href=\"{md}"), &format!("href=\"{xhtml}"));
        out = out.replace(&format!("href=\"./{md}"), &format!("href=\"{xhtml}"));

        out = out.replace(&format!("href='chapters/{md}"), &format!("href='{xhtml}"));
        out = out.replace(&format!("href='./chapters/{md}"), &format!("href='{xhtml}"));
        out = out.replace(&format!("href='{md}"), &format!("href='{xhtml}"));
        out = out.replace(&format!("href='./{md}"), &format!("href='{xhtml}"));
    }

    out
}

fn ensure_xhtml_void_tags(html: &str) -> String {
    // Convert void tags like `<img ...>` into `<img ... />` to keep EPUB XHTML well-formed.
    const VOID_TAGS: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];

    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;

    while let Some(rel_lt) = html[cursor..].find('<') {
        let lt = cursor + rel_lt;

        // Copy text before the tag (keeps UTF-8 intact).
        out.push_str(&html[cursor..lt]);

        // Find end of the tag `>` while respecting quotes.
        let mut in_quote: Option<u8> = None;
        let mut gt = lt + 1;
        while gt < bytes.len() {
            let b = bytes[gt];
            if let Some(q) = in_quote {
                if b == q {
                    in_quote = None;
                }
                gt += 1;
                continue;
            }
            if b == b'"' || b == b'\'' {
                in_quote = Some(b);
                gt += 1;
                continue;
            }
            if b == b'>' {
                break;
            }
            gt += 1;
        }
        if gt >= bytes.len() {
            // Malformed HTML; copy the rest as-is.
            out.push_str(&html[lt..]);
            return out;
        }

        let raw_tag = &html[lt..=gt];

        // Keep comments/doctype/processing instructions/end tags as-is.
        if raw_tag
            .as_bytes()
            .get(1)
            .is_some_and(|b| matches!(b, b'!' | b'?' | b'/'))
        {
            out.push_str(raw_tag);
            cursor = gt + 1;
            continue;
        }

        // Parse tag name.
        let name_start = lt + 1;
        let mut name_end = name_start;
        while name_end < gt && (bytes[name_end] as char).is_ascii_alphabetic() {
            name_end += 1;
        }
        if name_end == name_start {
            out.push_str(raw_tag);
            cursor = gt + 1;
            continue;
        }

        let tag_name = &html[name_start..name_end];
        let tag_name_lower = tag_name.to_ascii_lowercase();
        if !VOID_TAGS.contains(&tag_name_lower.as_str()) {
            out.push_str(raw_tag);
            cursor = gt + 1;
            continue;
        }

        let tag_without_gt = &html[lt..gt];
        let already_self_closed = tag_without_gt.trim_end().ends_with('/');
        if already_self_closed {
            out.push_str(raw_tag);
        } else {
            out.push_str(tag_without_gt);
            out.push_str(" />");
        }

        cursor = gt + 1;
    }

    out.push_str(&html[cursor..]);
    out
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

fn read_book_title(book_dir: &Path) -> anyhow::Result<Option<String>> {
    let book_toml_path = book_dir.join("book.toml");
    if !book_toml_path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&book_toml_path)
        .with_context(|| format!("read book.toml: {}", book_toml_path.display()))?;

    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("title") {
            continue;
        }
        let Some((_, rhs)) = line.split_once('=') else {
            continue;
        };
        let rhs = rhs.trim();
        if let Some(stripped) = rhs.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            return Ok(Some(stripped.to_owned()));
        }
    }
    Ok(None)
}

fn extract_first_heading(md: &str) -> Option<String> {
    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.starts_with('#') {
            continue;
        }
        let title = trimmed.trim_start_matches('#').trim();
        if title.is_empty() {
            continue;
        }
        return Some(title.to_string());
    }
    None
}

fn list_files_recursively_sorted(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let mut entries = fs::read_dir(&current)
            .with_context(|| format!("read dir: {}", current.display()))?
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("list dir: {}", current.display()))?;
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().context("read entry type")?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if file_type.is_file() {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_xhtml_void_tags_preserves_utf8_text() {
        let input = "<p>日本語のテスト</p><img src=\"x.png\">";
        let out = ensure_xhtml_void_tags(input);
        assert!(out.contains("日本語のテスト"));
        assert!(out.contains("<img src=\"x.png\" />"));
    }
}
