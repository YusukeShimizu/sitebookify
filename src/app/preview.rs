use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use anyhow::Context as _;
use serde::Serialize;
use url::Url;

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_SITEMAP_LOCS: usize = 20_000;
const MAX_SUB_SITEMAPS: usize = 5;
const MAX_LINK_HREFS: usize = 500;
const MAX_SAMPLE_URLS: usize = 20;
const MAX_CHAPTERS: usize = 12;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    Sitemap,
    SitemapIndex,
    Links,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreviewChapter {
    pub title: String,
    pub pages: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SitePreview {
    pub source: PreviewSource,
    pub estimated_pages: usize,
    pub estimated_chapters: usize,
    pub chapters: Vec<PreviewChapter>,
    pub sample_urls: Vec<String>,
    pub notes: Vec<String>,
}

pub async fn preview_site(start_url: &Url) -> anyhow::Result<SitePreview> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("build preview http client")?;

    preview_site_with_client(&client, start_url).await
}

async fn preview_site_with_client(
    client: &reqwest::Client,
    start_url: &Url,
) -> anyhow::Result<SitePreview> {
    if start_url.scheme() != "http" && start_url.scheme() != "https" {
        anyhow::bail!("url scheme must be http/https");
    }
    let Some(host) = start_url.host_str() else {
        anyhow::bail!("url must include host");
    };

    let sitemap_url = with_path(start_url, "/sitemap.xml")?;
    if let Ok(Some(sitemap)) = try_fetch_text(client, &sitemap_url).await {
        let lower = sitemap.text.to_ascii_lowercase();
        let is_index = lower.contains("<sitemapindex");
        if is_index {
            if let Some(out) =
                preview_from_sitemap_index(client, start_url, host, &sitemap.text).await?
            {
                return Ok(out);
            }
        } else if let Some(out) = preview_from_sitemap_urlset(start_url, host, &sitemap.text) {
            return Ok(out);
        }
    }

    preview_from_links(client, start_url, host).await
}

#[derive(Debug, Clone)]
struct FetchedText {
    text: String,
    truncated: bool,
}

async fn try_fetch_text(
    client: &reqwest::Client,
    url: &Url,
) -> anyhow::Result<Option<FetchedText>> {
    let resp = client
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, "sitebookify/0.1")
        .header(
            reqwest::header::ACCEPT,
            "application/xml,text/xml,text/html,application/xhtml+xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let (text, truncated) = read_text_limited(resp, MAX_BODY_BYTES).await?;
    Ok(Some(FetchedText { text, truncated }))
}

async fn read_text_limited(
    mut resp: reqwest::Response,
    limit: usize,
) -> anyhow::Result<(String, bool)> {
    let mut out: Vec<u8> = Vec::new();
    let mut truncated = false;

    while let Some(chunk) = resp.chunk().await.context("read response chunk")? {
        if out.len() + chunk.len() > limit {
            let remaining = limit.saturating_sub(out.len());
            out.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        out.extend_from_slice(&chunk);
    }

    Ok((String::from_utf8_lossy(&out).into_owned(), truncated))
}

fn with_path(base: &Url, path: &str) -> anyhow::Result<Url> {
    let mut out = base.clone();
    out.set_query(None);
    out.set_fragment(None);
    out.set_path(path);
    Ok(out)
}

fn extract_xml_locs(xml: &str) -> Vec<String> {
    let lower = xml.to_ascii_lowercase();
    let mut locs = Vec::new();

    let mut pos = 0usize;
    while locs.len() < MAX_SITEMAP_LOCS {
        let Some(start_rel) = lower[pos..].find("<loc>") else {
            break;
        };
        let start = pos + start_rel + "<loc>".len();
        let Some(end_rel) = lower[start..].find("</loc>") else {
            break;
        };
        let end = start + end_rel;
        let raw = xml[start..end].trim();
        if !raw.is_empty() {
            locs.push(raw.to_string());
        }
        pos = end + "</loc>".len();
    }

    locs
}

fn canonical_url(url: &Url) -> Url {
    let mut canonical = url.clone();
    canonical.set_fragment(None);
    canonical.set_query(None);

    let mut path = canonical.path().to_owned();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    canonical.set_path(&path);
    canonical
}

fn chapter_key(start_url: &Url, page_url: &Url) -> String {
    let base_path = {
        let p = start_url.path();
        if p.ends_with('/') {
            p.to_string()
        } else {
            format!("{p}/")
        }
    };

    let path = page_url.path();
    let key = if path.starts_with(&base_path) {
        let rest = &path[base_path.len()..];
        rest.split('/')
            .find(|s| !s.trim().is_empty())
            .unwrap_or("root")
            .to_string()
    } else {
        path.trim_start_matches('/')
            .split('/')
            .find(|s| !s.trim().is_empty())
            .unwrap_or("root")
            .to_string()
    };

    if key.trim().is_empty() {
        "root".to_string()
    } else {
        key
    }
}

fn summarize(
    start_url: &Url,
    source: PreviewSource,
    pages: &[Url],
    notes: Vec<String>,
) -> SitePreview {
    let mut by_chapter: BTreeMap<String, usize> = BTreeMap::new();
    for u in pages {
        let key = chapter_key(start_url, u);
        *by_chapter.entry(key).or_insert(0) += 1;
    }

    let mut chapters: Vec<PreviewChapter> = by_chapter
        .into_iter()
        .map(|(title, pages)| PreviewChapter { title, pages })
        .collect();
    chapters.sort_by(|a, b| b.pages.cmp(&a.pages).then_with(|| a.title.cmp(&b.title)));

    let sample_urls = pages
        .iter()
        .take(MAX_SAMPLE_URLS)
        .map(|u| u.to_string())
        .collect::<Vec<_>>();

    SitePreview {
        source,
        estimated_pages: pages.len(),
        estimated_chapters: chapters.len(),
        chapters: chapters.into_iter().take(MAX_CHAPTERS).collect(),
        sample_urls,
        notes,
    }
}

fn preview_from_sitemap_urlset(start_url: &Url, host: &str, xml: &str) -> Option<SitePreview> {
    let locs = extract_xml_locs(xml);
    if locs.is_empty() {
        return None;
    }

    let mut uniq: HashSet<String> = HashSet::new();
    let mut pages: Vec<Url> = Vec::new();
    for loc in locs {
        let Ok(u) = Url::parse(loc.trim()) else {
            continue;
        };
        if u.host_str() != Some(host) {
            continue;
        }
        let u = canonical_url(&u);
        if uniq.insert(u.to_string()) {
            pages.push(u);
        }
    }

    if pages.is_empty() {
        return None;
    }

    Some(summarize(
        start_url,
        PreviewSource::Sitemap,
        &pages,
        Vec::new(),
    ))
}

async fn preview_from_sitemap_index(
    client: &reqwest::Client,
    start_url: &Url,
    host: &str,
    xml: &str,
) -> anyhow::Result<Option<SitePreview>> {
    let sitemap_urls = extract_xml_locs(xml)
        .into_iter()
        .filter_map(|loc| Url::parse(loc.trim()).ok())
        .filter(|u| u.host_str() == Some(host))
        .take(MAX_SITEMAP_LOCS)
        .collect::<Vec<_>>();

    if sitemap_urls.is_empty() {
        return Ok(None);
    }

    let total = sitemap_urls.len();
    let mut fetched = 0usize;
    let mut truncated_any = false;

    let mut uniq: HashSet<String> = HashSet::new();
    let mut pages: Vec<Url> = Vec::new();

    for u in sitemap_urls.iter().take(MAX_SUB_SITEMAPS) {
        let Some(fetched_text) = try_fetch_text(client, u).await? else {
            continue;
        };
        fetched += 1;
        truncated_any |= fetched_text.truncated;

        let locs = extract_xml_locs(&fetched_text.text);
        for loc in locs {
            let Ok(page) = Url::parse(loc.trim()) else {
                continue;
            };
            if page.host_str() != Some(host) {
                continue;
            }
            let page = canonical_url(&page);
            if uniq.insert(page.to_string()) {
                pages.push(page);
            }
        }
    }

    if pages.is_empty() {
        return Ok(None);
    }

    let mut notes = vec![format!(
        "sitemapindex: fetched {fetched}/{total} child sitemaps"
    )];
    if truncated_any {
        notes.push("some sitemap responses were truncated".to_string());
    }

    let mut out = summarize(start_url, PreviewSource::SitemapIndex, &pages, notes);
    if fetched > 0 && total > fetched {
        let avg = (pages.len() as f64) / (fetched as f64);
        let estimated = (avg * (total as f64)).round() as usize;
        out.estimated_pages = out.estimated_pages.max(estimated);
    }
    Ok(Some(out))
}

async fn preview_from_links(
    client: &reqwest::Client,
    start_url: &Url,
    host: &str,
) -> anyhow::Result<SitePreview> {
    let Some(fetched) = try_fetch_text(client, start_url).await? else {
        anyhow::bail!("failed to fetch start url: {start_url}");
    };

    let hrefs = extract_html_hrefs(&fetched.text);
    let mut notes = Vec::new();
    if fetched.truncated {
        notes.push("html response was truncated".to_string());
    }

    let mut uniq: HashSet<String> = HashSet::new();
    let mut pages: Vec<Url> = Vec::new();
    for href in hrefs.into_iter().take(MAX_LINK_HREFS) {
        let href = href.trim();
        if href.is_empty() {
            continue;
        }
        let Ok(u) = start_url.join(href) else {
            continue;
        };
        if u.host_str() != Some(host) {
            continue;
        }
        if u.scheme() != "http" && u.scheme() != "https" {
            continue;
        }
        let u = canonical_url(&u);
        if uniq.insert(u.to_string()) {
            pages.push(u);
        }
    }

    Ok(summarize(start_url, PreviewSource::Links, &pages, notes))
}

fn extract_html_hrefs(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut hrefs = Vec::new();

    let mut pos = 0usize;
    while hrefs.len() < MAX_LINK_HREFS {
        let Some(rel) = lower[pos..].find("href=") else {
            break;
        };
        let start = pos + rel + "href=".len();
        let Some(quote) = html.as_bytes().get(start).copied() else {
            break;
        };
        if quote != b'"' && quote != b'\'' {
            pos = start;
            continue;
        }
        let quote = quote as char;
        let content_start = start + 1;
        let Some(end_rel) = html[content_start..].find(quote) else {
            break;
        };
        let end = content_start + end_rel;
        let raw = html[content_start..end].trim();
        if !raw.is_empty() && !raw.starts_with('#') {
            hrefs.push(raw.to_string());
        }
        pos = end + 1;
    }

    hrefs
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn spawn_preview_server(
        has_sitemap: bool,
    ) -> (String, mpsc::Sender<()>, thread::JoinHandle<()>) {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("start tiny_http server");
        let addr = server.server_addr();
        let base_url = format!("http://{addr}");
        let base_url_for_sitemap = base_url.clone();

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

                let path = request.url().split('?').next().unwrap_or(request.url());
                let (status, body, content_type) = match path {
                    "/sitemap.xml" if has_sitemap => (
                        200,
                        format!(
                            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>{base_url_for_sitemap}/docs/intro</loc></url>
  <url><loc>{base_url_for_sitemap}/docs/advanced</loc></url>
</urlset>
"#
                        ),
                        "application/xml",
                    ),
                    "/sitemap.xml" => (404, "not found".to_string(), "text/plain"),
                    "/docs/" => (
                        200,
                        r#"<!doctype html>
<html>
  <body>
    <a href="intro">Intro</a>
    <a href="advanced#top">Advanced</a>
    <a href="/outside">Outside</a>
  </body>
</html>
"#
                        .to_string(),
                        "text/html",
                    ),
                    "/docs/intro" => (200, "Intro".to_string(), "text/plain"),
                    "/docs/advanced" => (200, "Advanced".to_string(), "text/plain"),
                    "/outside" => (200, "Outside".to_string(), "text/plain"),
                    _ => (404, "not found".to_string(), "text/plain"),
                };

                let mut resp = tiny_http::Response::from_string(body).with_status_code(status);
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
                        .expect("content-type header");
                resp.add_header(header);
                let _ = request.respond(resp);
            }
        });

        (base_url, shutdown_tx, handle)
    }

    #[tokio::test]
    async fn preview_uses_sitemap_when_available() {
        let (base_url, shutdown_tx, handle) = spawn_preview_server(true);
        let start_url = Url::parse(&format!("{base_url}/docs/")).unwrap();

        let out = preview_site(&start_url).await.unwrap();
        assert_eq!(out.source, PreviewSource::Sitemap);
        assert_eq!(out.estimated_pages, 2);
        assert_eq!(out.estimated_chapters, 2);

        let _ = shutdown_tx.send(());
        let _ = handle.join();
    }

    #[tokio::test]
    async fn preview_falls_back_to_links_when_no_sitemap() {
        let (base_url, shutdown_tx, handle) = spawn_preview_server(false);
        let start_url = Url::parse(&format!("{base_url}/docs/")).unwrap();

        let out = preview_site(&start_url).await.unwrap();
        assert_eq!(out.source, PreviewSource::Links);
        assert!(out.estimated_pages >= 2);

        let _ = shutdown_tx.send(());
        let _ = handle.join();
    }
}
