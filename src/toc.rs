use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::cli::{LlmEngine, TocCreateArgs};
use crate::formats::{ManifestRecord, Toc, TocChapter, TocPart, TocSection};
use crate::openai::{OpenAiConfig, exec_readonly};

pub async fn create(args: TocCreateArgs) -> anyhow::Result<()> {
    let manifest_path = PathBuf::from(&args.manifest);
    let out_path = PathBuf::from(&args.out);

    if out_path.exists() && !args.force {
        anyhow::bail!("toc output already exists: {}", out_path.display());
    }

    let records = read_manifest_records(&manifest_path).context("read manifest")?;
    if records.is_empty() {
        anyhow::bail!("manifest is empty: {}", manifest_path.display());
    }

    let plan = match args.engine {
        LlmEngine::Noop => plan_noop(&args, &records),
        LlmEngine::Openai => plan_via_openai(&args, &records).await?,
    };

    let toc = toc_from_plan(&args, &records, &plan).context("build toc from plan")?;

    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create toc dir: {}", parent.display()))?;
    }

    let yaml = serde_yaml::to_string(&toc).context("serialize toc yaml")?;

    let mut options = OpenOptions::new();
    options.write(true);
    if args.force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut out = options
        .open(&out_path)
        .with_context(|| format!("open toc output: {}", out_path.display()))?;
    out.write_all(yaml.as_bytes())
        .with_context(|| format!("write toc: {}", out_path.display()))?;
    out.flush().context("flush toc")?;

    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct TocCreateInput {
    language: String,
    tone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    book_title_hint: Option<String>,
    pages: Vec<TocCreatePage>,
}

#[derive(Debug, Clone, Serialize)]
struct TocCreatePage {
    id: String,
    path: String,
    title: String,
    url: String,
    extracted_md: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TocPlan {
    book_title: String,
    chapters: Vec<TocPlanChapter>,
}

#[derive(Debug, Clone, Deserialize)]
struct TocPlanChapter {
    title: String,
    intent: String,
    reader_gains: Vec<String>,
    sections: Vec<TocPlanSection>,
}

#[derive(Debug, Clone, Deserialize)]
struct TocPlanSection {
    title: String,
    sources: Vec<String>,
}

fn plan_noop(args: &TocCreateArgs, records: &[ManifestRecord]) -> TocPlan {
    let chapter_title = derive_chapter_title(records);
    let book_title = args
        .book_title
        .clone()
        .unwrap_or_else(|| format!("{chapter_title} Textbook"));

    TocPlan {
        book_title,
        chapters: vec![TocPlanChapter {
            title: chapter_title,
            intent: "素材を整理し、本として読める順序に並べる。".to_owned(),
            reader_gains: vec!["原典ページを参照しながら、全体像をたどれる。".to_owned()],
            sections: records
                .iter()
                .map(|r| TocPlanSection {
                    title: r.title.clone(),
                    sources: vec![r.id.clone()],
                })
                .collect(),
        }],
    }
}

async fn plan_via_openai(
    args: &TocCreateArgs,
    records: &[ManifestRecord],
) -> anyhow::Result<TocPlan> {
    let pages = records
        .iter()
        .map(|r| TocCreatePage {
            id: r.id.clone(),
            path: r.path.clone(),
            title: r.title.clone(),
            url: r.url.clone(),
            extracted_md: r.extracted_md.clone(),
        })
        .collect::<Vec<_>>();

    let input = TocCreateInput {
        language: args.language.clone(),
        tone: args.tone.clone(),
        book_title_hint: args.book_title.clone(),
        pages,
    };
    let input_json = serde_json::to_string_pretty(&input).context("serialize toc input json")?;

    let input_file = tempfile::NamedTempFile::new().context("create toc input temp file")?;
    std::fs::write(input_file.path(), input_json)
        .with_context(|| format!("write toc input: {}", input_file.path().display()))?;

    let prompt = format!(
        "You are a book editor.\n\
\n\
Task: Create a Table of Contents (TOC) for a book.\n\
\n\
Input:\n\
- A JSON file exists at: {input_path}\n\
- It contains `language`, `tone`, optional `book_title_hint`, and `pages`.\n\
- Each page has an `extracted_md` path to a Markdown snapshot.\n\
\n\
You MUST:\n\
- Read *all* pages' `extracted_md` files and consider the full content (ignore YAML front matter).\n\
- Make editorial decisions at the TOC level:\n\
  - Merge overlapping topics.\n\
  - Consolidate near-duplicate pages.\n\
  - Omit pages that are not suitable for a book (e.g. nav/search/index/legal/changelog).\n\
\n\
Hard rules:\n\
- Use ONLY the provided page IDs.\n\
- A page ID MUST appear at most once across all sections (no duplicates).\n\
- Each chapter MUST have:\n\
  - `title` (non-empty)\n\
  - `intent` (non-empty)\n\
  - `reader_gains` (>= 1 item)\n\
  - `sections` (>= 1 item)\n\
- Each section MUST have:\n\
  - `title` (non-empty)\n\
  - `sources` (>= 1 page id)\n\
\n\
Language & tone:\n\
- Titles and chapter fields MUST follow `language` and `tone` from the input.\n\
\n\
Output:\n\
- Output ONLY a single JSON object (no markdown fences, no commentary).\n\
- Schema:\n\
  {{\"book_title\":\"...\",\"chapters\":[{{\"title\":\"...\",\"intent\":\"...\",\"reader_gains\":[\"...\"],\"sections\":[{{\"title\":\"...\",\"sources\":[\"p_...\"]}}]}}]}}\n",
        input_path = input_file.path().display(),
    );

    let config = OpenAiConfig::from_env();
    let raw = exec_readonly(&prompt, &config).context("openai exec for toc")?;
    let json = extract_json_object(&raw).context("extract json object from openai output")?;
    serde_json::from_str(json).context("parse toc plan json")
}

fn toc_from_plan(
    args: &TocCreateArgs,
    records: &[ManifestRecord],
    plan: &TocPlan,
) -> anyhow::Result<Toc> {
    if plan.book_title.trim().is_empty() {
        anyhow::bail!("toc plan book_title is empty");
    }
    if plan.chapters.is_empty() {
        anyhow::bail!("toc plan has no chapters");
    }
    if plan.chapters.len() > 99 {
        anyhow::bail!(
            "too many chapters ({}); chapter ids are limited to ch01..ch99",
            plan.chapters.len()
        );
    }

    let manifest_ids = records
        .iter()
        .map(|r| r.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();

    for ch in &plan.chapters {
        if ch.title.trim().is_empty() {
            anyhow::bail!("toc plan chapter title is empty");
        }
        if ch.intent.trim().is_empty() {
            anyhow::bail!("toc plan chapter intent is empty");
        }
        if ch.reader_gains.is_empty() || ch.reader_gains.iter().all(|g| g.trim().is_empty()) {
            anyhow::bail!("toc plan chapter reader_gains is empty");
        }
        if ch.sections.is_empty() {
            anyhow::bail!("toc plan chapter sections is empty");
        }

        for section in &ch.sections {
            if section.title.trim().is_empty() {
                anyhow::bail!("toc plan section title is empty");
            }
            if section.sources.is_empty() {
                anyhow::bail!("toc plan section sources is empty");
            }
            for src in &section.sources {
                if !manifest_ids.contains(src.as_str()) {
                    anyhow::bail!("unknown source id in toc plan: {src}");
                }
                if !seen.insert(src.as_str()) {
                    anyhow::bail!("duplicate source id in toc plan: {src}");
                }
            }
        }
    }

    if seen.is_empty() {
        anyhow::bail!("toc plan does not include any manifest pages");
    }

    if seen.len() != manifest_ids.len() {
        let mut omitted = Vec::new();
        for id in &manifest_ids {
            if !seen.contains(id) {
                omitted.push((*id).to_owned());
            }
        }
        omitted.sort();
        let sample = omitted.iter().take(10).cloned().collect::<Vec<_>>();
        tracing::info!(
            selected_pages = seen.len(),
            total_pages = manifest_ids.len(),
            omitted_pages = omitted.len(),
            omitted_sample = ?sample,
            "toc plan omitted pages"
        );
    }

    let book_title = args
        .book_title
        .clone()
        .unwrap_or_else(|| plan.book_title.clone());

    let chapters = plan
        .chapters
        .iter()
        .enumerate()
        .map(|(idx, ch)| {
            let mut gains = ch.reader_gains.clone();
            gains.retain(|g| !g.trim().is_empty());
            TocChapter {
                id: format!("ch{:02}", idx + 1),
                title: ch.title.clone(),
                intent: ch.intent.clone(),
                reader_gains: gains,
                sections: ch
                    .sections
                    .iter()
                    .map(|s| TocSection {
                        title: s.title.clone(),
                        sources: s.sources.clone(),
                    })
                    .collect(),
            }
        })
        .collect::<Vec<_>>();

    Ok(Toc {
        book_title,
        parts: vec![TocPart {
            title: "Part 1".to_owned(),
            chapters,
        }],
    })
}

fn read_manifest_records(manifest_path: &PathBuf) -> anyhow::Result<Vec<ManifestRecord>> {
    let file = OpenOptions::new()
        .read(true)
        .open(manifest_path)
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
    Ok(records)
}

fn extract_json_object(text: &str) -> anyhow::Result<&str> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("missing `{{`"))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| anyhow::anyhow!("missing `}}`"))?;
    if end <= start {
        anyhow::bail!("invalid json object span");
    }
    Ok(&text[start..=end])
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
