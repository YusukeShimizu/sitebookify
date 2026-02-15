use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::Duration;

use anyhow::Context as _;
use readability_js::Readability;
use serde::Serialize;
use url::Url;

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_SITEMAP_LOCS: usize = 20_000;
const MAX_SUB_SITEMAPS: usize = 5;
const MAX_LINK_HREFS: usize = 500;
const MAX_LINKS_PER_PAGE: usize = 200;
const MAX_LINK_CRAWL_DEPTH: usize = 2;
const MAX_LINK_CRAWL_PAGES: usize = 200;
const MAX_SAMPLE_URLS: usize = 20;
const MAX_CHAPTERS: usize = 12;
const TOKEN_RANGE_MIN_RATIO: f64 = 0.85;
const TOKEN_RANGE_MAX_RATIO: f64 = 1.15;
const DEFAULT_TOKEN_PER_CHAR_INPUT: f64 = 0.25;
const DEFAULT_TOKEN_PER_CHAR_OUTPUT: f64 = 0.125;

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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewCharacterBasis {
    ExtractedMarkdown,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SitePreview {
    pub source: PreviewSource,
    pub estimated_pages: usize,
    pub estimated_chapters: usize,
    pub chapters: Vec<PreviewChapter>,
    pub sample_urls: Vec<String>,
    pub notes: Vec<String>,
    pub total_characters: u64,
    pub character_basis: PreviewCharacterBasis,
    pub estimated_input_tokens_min: u64,
    pub estimated_input_tokens_max: u64,
    pub estimated_output_tokens_min: u64,
    pub estimated_output_tokens_max: u64,
    pub estimated_cost_usd_min: Option<f64>,
    pub estimated_cost_usd_max: Option<f64>,
    pub pricing_model: String,
    pub pricing_note: Option<String>,
}

#[derive(Debug, Clone)]
struct PreviewPricingConfig {
    model: String,
    input_usd_per_1m: Option<f64>,
    output_usd_per_1m: Option<f64>,
    token_per_char_input: f64,
    token_per_char_output: f64,
}

impl PreviewPricingConfig {
    fn from_env() -> Self {
        let model = std::env::var("SITEBOOKIFY_PRICING_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                std::env::var("SITEBOOKIFY_OPENAI_MODEL")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .unwrap_or_else(|| "gpt-5.2".to_string());

        let input_usd_per_1m = parse_env_non_negative_f64("SITEBOOKIFY_PRICING_INPUT_USD_PER_1M");
        let output_usd_per_1m = parse_env_non_negative_f64("SITEBOOKIFY_PRICING_OUTPUT_USD_PER_1M");
        let token_per_char_input = parse_env_positive_f64(
            "SITEBOOKIFY_PRICING_TOKEN_PER_CHAR_INPUT",
            DEFAULT_TOKEN_PER_CHAR_INPUT,
        );
        let token_per_char_output = parse_env_positive_f64(
            "SITEBOOKIFY_PRICING_TOKEN_PER_CHAR_OUTPUT",
            DEFAULT_TOKEN_PER_CHAR_OUTPUT,
        );

        Self {
            model,
            input_usd_per_1m,
            output_usd_per_1m,
            token_per_char_input,
            token_per_char_output,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TokenRange {
    min: u64,
    max: u64,
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

    let mut preview = {
        let sitemap_url = with_path(start_url, "/sitemap.xml")?;
        if let Ok(Some(sitemap)) = try_fetch_text(client, &sitemap_url).await {
            let lower = sitemap.text.to_ascii_lowercase();
            let is_index = lower.contains("<sitemapindex");
            if is_index {
                if let Some(out) =
                    preview_from_sitemap_index(client, start_url, host, &sitemap.text).await?
                {
                    out
                } else {
                    preview_from_links(client, start_url, host).await?
                }
            } else if let Some(out) = preview_from_sitemap_urlset(start_url, host, &sitemap.text) {
                out
            } else {
                preview_from_links(client, start_url, host).await?
            }
        } else {
            preview_from_links(client, start_url, host).await?
        }
    };

    enrich_preview_with_estimates(client, &mut preview).await;
    Ok(preview)
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

fn join_href(base_url: &Url, href: &str) -> Result<Url, url::ParseError> {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with('/') {
        return base_url.join(href);
    }

    if base_url.path().ends_with('/') {
        return base_url.join(href);
    }

    let mut adjusted = base_url.clone();
    let last_segment = adjusted.path().rsplit('/').next().unwrap_or("");
    if !last_segment.contains('.') {
        let mut path = adjusted.path().to_string();
        path.push('/');
        adjusted.set_path(&path);
    }
    adjusted.join(href)
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
        total_characters: 0,
        character_basis: PreviewCharacterBasis::ExtractedMarkdown,
        estimated_input_tokens_min: 0,
        estimated_input_tokens_max: 0,
        estimated_output_tokens_min: 0,
        estimated_output_tokens_max: 0,
        estimated_cost_usd_min: None,
        estimated_cost_usd_max: None,
        pricing_model: String::new(),
        pricing_note: None,
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
    let start_url = canonical_url(start_url);
    let mut notes = Vec::new();
    let mut queued: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(Url, usize)> = VecDeque::new();
    let mut pages = Vec::new();
    let mut truncated_any = false;
    let mut page_limit_reached = false;
    let mut per_page_link_cap_hit = false;
    let mut max_depth_reached = false;

    queued.insert(start_url.to_string());
    queue.push_back((start_url.clone(), 0));

    while let Some((current_url, depth)) = queue.pop_front() {
        if pages.len() >= MAX_LINK_CRAWL_PAGES {
            page_limit_reached = true;
            break;
        }

        let Some(fetched) = try_fetch_text(client, &current_url).await? else {
            continue;
        };
        truncated_any |= fetched.truncated;
        pages.push(current_url.clone());

        let hrefs = extract_html_hrefs(&fetched.text);
        if hrefs.len() > MAX_LINKS_PER_PAGE {
            per_page_link_cap_hit = true;
        }
        if depth >= MAX_LINK_CRAWL_DEPTH {
            if !hrefs.is_empty() {
                max_depth_reached = true;
            }
            continue;
        }

        for href in hrefs.into_iter().take(MAX_LINKS_PER_PAGE) {
            let href = href.trim();
            if href.is_empty() {
                continue;
            }
            let Ok(next_url) = join_href(&current_url, href) else {
                continue;
            };
            if next_url.host_str() != Some(host) {
                continue;
            }
            if next_url.scheme() != "http" && next_url.scheme() != "https" {
                continue;
            }
            let next_url = canonical_url(&next_url);
            if queued.insert(next_url.to_string()) {
                queue.push_back((next_url, depth + 1));
            }
        }
    }

    if pages.is_empty() {
        anyhow::bail!("failed to fetch start url: {start_url}");
    }

    if truncated_any {
        notes.push("some html responses were truncated".to_string());
    }
    if page_limit_reached {
        notes.push(format!(
            "link crawl reached page limit ({MAX_LINK_CRAWL_PAGES})"
        ));
    }
    if max_depth_reached {
        notes.push(format!(
            "link crawl reached depth limit ({MAX_LINK_CRAWL_DEPTH})"
        ));
    }
    if per_page_link_cap_hit {
        notes.push(format!(
            "some pages exceeded per-page link cap ({MAX_LINKS_PER_PAGE})"
        ));
    }

    Ok(summarize(&start_url, PreviewSource::Links, &pages, notes))
}

async fn enrich_preview_with_estimates(client: &reqwest::Client, preview: &mut SitePreview) {
    let pricing = PreviewPricingConfig::from_env();
    preview.pricing_model = pricing.model.clone();

    let mut sampled_pages = 0usize;
    let mut failed_pages = 0usize;
    let mut truncated_pages = 0usize;
    let mut sampled_characters = 0u64;
    let mut fetched_samples: Vec<(String, String)> = Vec::new();

    for sample_url in preview.sample_urls.iter().take(MAX_SAMPLE_URLS) {
        let Ok(url) = Url::parse(sample_url) else {
            failed_pages += 1;
            continue;
        };
        let fetched = match try_fetch_text(client, &url).await {
            Ok(Some(fetched)) => fetched,
            Ok(None) => {
                failed_pages += 1;
                continue;
            }
            Err(_) => {
                failed_pages += 1;
                continue;
            }
        };

        if fetched.truncated {
            truncated_pages += 1;
        }
        fetched_samples.push((url.to_string(), fetched.text));
    }

    let readability = match Readability::new() {
        Ok(readability) => readability,
        Err(err) => {
            preview.pricing_note = Some(format!(
                "character/cost estimation unavailable: failed to init readability ({err})"
            ));
            return;
        }
    };

    for (sample_url, html) in fetched_samples {
        match crate::extract::preview_character_count_from_html(&readability, &html, &sample_url) {
            Ok(count) => {
                sampled_pages += 1;
                sampled_characters = sampled_characters.saturating_add(count as u64);
            }
            Err(_) => {
                failed_pages += 1;
            }
        }
    }

    if truncated_pages > 0 {
        preview.notes.push(format!(
            "character estimate: {truncated_pages} sampled html responses were truncated"
        ));
    }
    if failed_pages > 0 {
        preview.notes.push(format!(
            "character estimate: failed to sample {failed_pages} pages"
        ));
    }

    let total_characters = if sampled_pages == 0 {
        preview
            .notes
            .push("character estimate: no sample pages could be parsed".to_string());
        0
    } else if preview.estimated_pages > sampled_pages {
        let avg = sampled_characters as f64 / sampled_pages as f64;
        let scaled = (avg * preview.estimated_pages as f64).round() as u64;
        preview.notes.push(format!(
            "character estimate extrapolated from {sampled_pages}/{} sampled pages",
            preview.estimated_pages
        ));
        scaled
    } else {
        sampled_characters
    };

    preview.total_characters = total_characters;

    let input_base = ceil_to_u64(total_characters as f64 * pricing.token_per_char_input);
    let output_base = ceil_to_u64(total_characters as f64 * pricing.token_per_char_output);
    let input_range = estimate_token_range(input_base);
    let output_range = estimate_token_range(output_base);
    preview.estimated_input_tokens_min = input_range.min;
    preview.estimated_input_tokens_max = input_range.max;
    preview.estimated_output_tokens_min = output_range.min;
    preview.estimated_output_tokens_max = output_range.max;

    if let (Some(input_price), Some(output_price)) =
        (pricing.input_usd_per_1m, pricing.output_usd_per_1m)
    {
        let input_unit = input_price / 1_000_000.0;
        let output_unit = output_price / 1_000_000.0;
        let cost_min = input_range.min as f64 * input_unit + output_range.min as f64 * output_unit;
        let cost_max = input_range.max as f64 * input_unit + output_range.max as f64 * output_unit;
        preview.estimated_cost_usd_min = Some(round_money(cost_min));
        preview.estimated_cost_usd_max = Some(round_money(cost_max));
        preview.pricing_note = Some(format!(
            "cost estimate uses model={} and env rates input=${input_price}/1M output=${output_price}/1M",
            pricing.model
        ));
    } else {
        preview.pricing_note = Some(
            "cost estimate unavailable: set SITEBOOKIFY_PRICING_INPUT_USD_PER_1M and SITEBOOKIFY_PRICING_OUTPUT_USD_PER_1M".to_string(),
        );
    }
}

fn estimate_token_range(base: u64) -> TokenRange {
    if base == 0 {
        return TokenRange { min: 0, max: 0 };
    }
    let min = floor_to_u64(base as f64 * TOKEN_RANGE_MIN_RATIO);
    let max = ceil_to_u64(base as f64 * TOKEN_RANGE_MAX_RATIO);
    TokenRange {
        min: min.max(1),
        max: max.max(min.max(1)),
    }
}

fn parse_env_non_negative_f64(name: &str) -> Option<f64> {
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Some(v),
        _ => {
            tracing::warn!(env_var = name, value = %trimmed, "invalid float env; ignoring");
            None
        }
    }
}

fn parse_env_positive_f64(name: &str, default: f64) -> f64 {
    let raw = match std::env::var(name) {
        Ok(raw) => raw,
        Err(_) => return default,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default;
    }
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => v,
        _ => {
            tracing::warn!(
                env_var = name,
                value = %trimmed,
                default = default,
                "invalid positive float env; fallback to default"
            );
            default
        }
    }
}

fn round_money(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn ceil_to_u64(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    value.ceil() as u64
}

fn floor_to_u64(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    value.floor() as u64
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
                    "/docs" | "/docs/" => (
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
                    "/docs/intro" => (
                        200,
                        r#"<!doctype html>
<html>
  <body>
    <a href="/docs/guide/part-1">Part 1</a>
  </body>
</html>
"#
                        .to_string(),
                        "text/html",
                    ),
                    "/docs/advanced" => (200, "Advanced".to_string(), "text/plain"),
                    "/docs/guide/part-1" => (
                        200,
                        r#"<!doctype html>
<html>
  <body>
    <a href="/docs/guide/part-2">Part 2</a>
  </body>
</html>
"#
                        .to_string(),
                        "text/html",
                    ),
                    "/docs/guide/part-2" => (200, "Part 2".to_string(), "text/plain"),
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

    #[test]
    fn token_range_has_expected_spread() {
        let range = estimate_token_range(100);
        assert_eq!(range.min, 85);
        assert_eq!(range.max, 115);
    }

    #[tokio::test]
    async fn preview_uses_sitemap_when_available() {
        let (base_url, shutdown_tx, handle) = spawn_preview_server(true);
        let start_url = Url::parse(&format!("{base_url}/docs/")).unwrap();

        let out = preview_site(&start_url).await.unwrap();
        assert_eq!(out.source, PreviewSource::Sitemap);
        assert_eq!(out.estimated_pages, 2);
        assert_eq!(out.estimated_chapters, 2);
        assert_eq!(
            out.character_basis,
            PreviewCharacterBasis::ExtractedMarkdown
        );

        let _ = shutdown_tx.send(());
        let _ = handle.join();
    }

    #[tokio::test]
    async fn preview_falls_back_to_link_crawl_when_no_sitemap() {
        let (base_url, shutdown_tx, handle) = spawn_preview_server(false);
        let start_url = Url::parse(&format!("{base_url}/docs/")).unwrap();

        let out = preview_site(&start_url).await.unwrap();
        assert_eq!(out.source, PreviewSource::Links);
        assert!(out.estimated_pages >= 4);
        assert!(
            out.sample_urls
                .iter()
                .any(|u| u.ends_with("/docs/guide/part-1"))
        );
        assert!(
            !out.sample_urls
                .iter()
                .any(|u| u.ends_with("/docs/guide/part-2"))
        );
        assert!(
            out.notes.iter().any(|n| n.contains("depth limit (2)")),
            "expected depth-limit note, got {:?}",
            out.notes
        );

        let _ = shutdown_tx.send(());
        let _ = handle.join();
    }
}
