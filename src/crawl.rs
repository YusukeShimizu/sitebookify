use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use reqwest::header::{ACCEPT, USER_AGENT};
use url::Url;

use crate::cli::CrawlArgs;
use crate::formats::CrawlRecord;

#[derive(Debug, Clone)]
struct CrawlScope {
    scheme: String,
    host: String,
    port: Option<u16>,
    path_prefix: String,
}

impl CrawlScope {
    fn new(start_url: &Url) -> anyhow::Result<Self> {
        let scheme = start_url.scheme().to_owned();
        let host = start_url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("start url must have host: {start_url}"))?
            .to_owned();
        let port = start_url.port();
        let path_prefix = start_url.path().to_owned();

        Ok(Self {
            scheme,
            host,
            port,
            path_prefix,
        })
    }

    fn is_same_origin(&self, url: &Url) -> bool {
        url.scheme() == self.scheme
            && url.host_str() == Some(self.host.as_str())
            && url.port() == self.port
    }

    fn is_under_path_prefix(&self, path: &str) -> bool {
        if self.path_prefix == "/" {
            return true;
        }

        if self.path_prefix.ends_with('/') {
            return path.starts_with(&self.path_prefix);
        }

        path == self.path_prefix || path.starts_with(&format!("{}/", self.path_prefix))
    }

    fn is_in_scope(&self, url: &Url) -> bool {
        self.is_same_origin(url) && self.is_under_path_prefix(url.path())
    }
}

pub async fn resolve_start_url_for_crawl(url: &Url) -> Url {
    let url = normalize_crawl_url(url);
    if !should_try_trailing_slash(&url) {
        return url;
    }

    let with_slash = url_with_trailing_slash(&url);
    match probe_html_url(&with_slash).await {
        Ok(Some(resolved)) => resolved,
        Ok(None) => url,
        Err(err) => {
            tracing::debug!(?err, candidate = %with_slash, "start url probe failed; using input url");
            url
        }
    }
}

pub async fn run(args: CrawlArgs) -> anyhow::Result<()> {
    let out_dir = PathBuf::from(&args.out);
    crate::raw_store::ensure_raw_snapshot_dir_does_not_exist(&out_dir)
        .context("check raw snapshot output directory")?;
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("create raw snapshot dir: {}", out_dir.display()))?;

    let start_url = Url::parse(&args.url).context("parse --url")?;
    if start_url.scheme() != "http" && start_url.scheme() != "https" {
        anyhow::bail!("--url must be http/https: {start_url}");
    }
    let start_url = resolve_start_url_for_crawl(&start_url).await;
    let start_url_canonical = canonical_url(&start_url);

    let scope = CrawlScope::new(&start_url_canonical).context("build crawl scope")?;

    let crawl_jsonl_path = out_dir.join("crawl.jsonl");
    let crawl_jsonl_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&crawl_jsonl_path)
        .with_context(|| format!("create crawl log: {}", crawl_jsonl_path.display()))?;
    let mut crawl_jsonl = BufWriter::new(crawl_jsonl_file);

    let mut website = spider::website::Website::new(start_url.as_str());
    website.configuration.respect_robots_txt = false;
    website.configuration.subdomains = false;
    website.configuration.tld = false;
    website.with_block_assets(true);
    website.with_return_page_links(true);
    website.with_delay(args.delay_ms);
    website.with_concurrency_limit(Some(args.concurrency.max(1)));
    website.with_limit(args.max_pages.min(u32::MAX as usize) as u32);
    website.with_depth(args.max_depth as usize);
    website.with_whitelist_url(Some(vec![build_whitelist_regex(&scope).into()]));

    let link_scope = scope.clone();
    website.on_link_find_callback = Some(Arc::new(move |url_ci, html| {
        let url_str = url_ci.to_string();
        let Ok(parsed) = Url::parse(&url_str) else {
            return (url_ci, html);
        };
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return (url_ci, html);
        }

        let normalized = normalize_crawl_url(&parsed);
        let canonical = canonical_url(&normalized);
        if !link_scope.is_in_scope(&canonical) {
            return (url_ci, html);
        }

        let normalized_str = normalized.to_string();
        (spider::CaseInsensitiveString::new(&normalized_str), html)
    }));

    website.scrape().await;

    let pages = website
        .get_pages()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|page| {
            let Ok(url) = Url::parse(page.get_url()) else {
                return None;
            };
            let normalized = normalize_crawl_url(&url);
            let canonical = canonical_url(&normalized);
            if !scope.is_in_scope(&canonical) {
                return None;
            }
            Some((canonical.to_string(), page))
        })
        .collect::<Vec<_>>();

    let (edges, page_by_url) = build_page_graph(&scope, pages);
    let depths = compute_depths(start_url_canonical.as_str(), &edges, args.max_depth);

    let mut urls = page_by_url.keys().cloned().collect::<Vec<_>>();
    urls.sort();

    for normalized_url_str in urls {
        let page = page_by_url
            .get(&normalized_url_str)
            .ok_or_else(|| anyhow::anyhow!("missing page for url: {normalized_url_str}"))?;
        let normalized_url =
            Url::parse(&normalized_url_str).context("parse normalized url for output")?;

        let status = page.status_code.as_u16();
        let retrieved_at = chrono::Utc::now().to_rfc3339();

        let mut record = CrawlRecord {
            url: normalized_url_str.clone(),
            normalized_url: normalized_url_str.clone(),
            depth: depths.get(&normalized_url_str).copied().unwrap_or(0),
            status,
            content_type: None,
            retrieved_at,
            raw_html_path: None,
        };

        if (200..300).contains(&status) {
            let html = page.get_html();
            if should_save_html(&html) {
                let raw_html_path = crate::raw_store::raw_html_path(&out_dir, &normalized_url)
                    .context("compute raw html path")?;
                crate::raw_store::write_raw_html(&raw_html_path, &html)
                    .context("write raw html")?;
                record.raw_html_path = Some(raw_html_path.to_string_lossy().to_string());
            }
        }

        serde_json::to_writer(&mut crawl_jsonl, &record).context("write crawl record json")?;
        crawl_jsonl
            .write_all(b"\n")
            .context("write crawl record newline")?;
    }

    crawl_jsonl.flush().context("flush crawl log")?;
    Ok(())
}

fn build_whitelist_regex(scope: &CrawlScope) -> String {
    let port = match scope.port {
        Some(port) => format!(":{port}"),
        None => String::new(),
    };
    let origin = format!("{}://{}{port}", scope.scheme, scope.host);
    let prefix = format!("{origin}{}", scope.path_prefix);

    if scope.path_prefix == "/" {
        format!("^{}.*$", regex_escape(&origin))
    } else if scope.path_prefix.ends_with('/') {
        format!("^{}.*$", regex_escape(&prefix))
    } else {
        format!("^{}(?:/.*)?$", regex_escape(&prefix))
    }
}

fn regex_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn build_page_graph(
    scope: &CrawlScope,
    pages: Vec<(String, spider::page::Page)>,
) -> (
    HashMap<String, Vec<String>>,
    HashMap<String, spider::page::Page>,
) {
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut page_by_url: HashMap<String, spider::page::Page> = HashMap::new();

    for (normalized_url, page) in pages {
        let mut links: Vec<String> = Vec::new();
        if let Some(page_links) = page.page_links.as_deref() {
            for link in page_links {
                let Ok(url) = Url::parse(link.as_ref()) else {
                    continue;
                };
                let normalized = normalize_crawl_url(&url);
                let canonical = canonical_url(&normalized);
                if scope.is_in_scope(&canonical) {
                    links.push(canonical.to_string());
                }
            }
        }
        links.sort();
        links.dedup();

        edges.insert(normalized_url.clone(), links);
        page_by_url.insert(normalized_url, page);
    }

    (edges, page_by_url)
}

fn compute_depths(
    start_url: &str,
    edges: &HashMap<String, Vec<String>>,
    max_depth: u32,
) -> HashMap<String, u32> {
    let mut depths: HashMap<String, u32> = HashMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    depths.insert(start_url.to_owned(), 0);
    queue.push_back(start_url.to_owned());

    while let Some(current) = queue.pop_front() {
        let depth = depths.get(&current).copied().unwrap_or(0);
        if depth >= max_depth {
            continue;
        }

        let Some(neighbors) = edges.get(&current) else {
            continue;
        };
        for neighbor in neighbors {
            if depths.contains_key(neighbor) {
                continue;
            }
            depths.insert(neighbor.clone(), depth.saturating_add(1));
            queue.push_back(neighbor.clone());
        }
    }

    depths
}

fn should_save_html(html: &str) -> bool {
    if html.trim().is_empty() {
        return false;
    }
    let trimmed = html.trim_start().to_ascii_lowercase();
    trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<html")
        || trimmed.contains("<html")
}

fn normalize_crawl_url(url: &Url) -> Url {
    let mut normalized = url.clone();
    normalized.set_fragment(None);
    normalized.set_query(None);
    normalized
}

fn should_try_trailing_slash(url: &Url) -> bool {
    let path = url.path();
    if path.ends_with('/') {
        return false;
    }

    let last_segment = path.rsplit('/').next().unwrap_or_default();
    if last_segment.is_empty() {
        return false;
    }

    !last_segment.contains('.')
}

fn url_with_trailing_slash(url: &Url) -> Url {
    let mut out = url.clone();
    let path = out.path();
    if !path.ends_with('/') {
        out.set_path(&format!("{path}/"));
    }
    out
}

async fn probe_html_url(url: &Url) -> anyhow::Result<Option<Url>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("build url probe http client")?;

    let response = client
        .get(url.clone())
        .header(USER_AGENT, "sitebookify/0.1")
        .header(ACCEPT, "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !response.status().is_success() {
        return Ok(None);
    }

    if let Some(content_type) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        let content_type = content_type.to_ascii_lowercase();
        if !(content_type.starts_with("text/html")
            || content_type.starts_with("application/xhtml+xml"))
        {
            return Ok(None);
        }
    }

    Ok(Some(normalize_crawl_url(response.url())))
}

fn canonical_url(url: &Url) -> Url {
    let mut canonical = normalize_crawl_url(url);
    let mut path = canonical.path().to_owned();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    canonical.set_path(&path);
    canonical
}
