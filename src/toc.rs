use std::collections::{HashMap, HashSet};
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
        .map(|r| {
            let extracted = std::fs::read_to_string(&r.extracted_md)
                .with_context(|| format!("read extracted page: {}", r.extracted_md))?;
            let extracted_md = strip_front_matter(&extracted).trim().to_owned();
            Ok(TocCreatePage {
                id: r.id.clone(),
                path: r.path.clone(),
                title: r.title.clone(),
                url: r.url.clone(),
                extracted_md,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let input = TocCreateInput {
        language: args.language.clone(),
        tone: args.tone.clone(),
        book_title_hint: args.book_title.clone(),
        pages,
    };
    let input_json = serde_json::to_string_pretty(&input).context("serialize toc input json")?;

    let prompt = format!(
        "You are a book editor.\n\
\n\
Task: Create a Table of Contents (TOC) for a book.\n\
\n\
Input:\n\
- A JSON object is provided below.\n\
- It contains `language`, `tone`, optional `book_title_hint`, and `pages`.\n\
- Each page has `extracted_md` containing the Markdown snapshot (YAML front matter removed).\n\
\n\
BEGIN_TOC_INPUT_JSON\n\
{input_json}\n\
END_TOC_INPUT_JSON\n\
\n\
You MUST:\n\
- Read *all* pages' `extracted_md` strings and consider the full content.\n\
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
        input_json = input_json.trim_end(),
    );

    let config = OpenAiConfig::from_env().context("load openai config")?;
    let raw = tokio::task::spawn_blocking({
        let prompt = prompt.clone();
        let config = config.clone();
        move || exec_readonly(&prompt, &config).context("openai exec for toc")
    })
    .await
    .context("join openai task")??;
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
    let manifest_ids = records
        .iter()
        .map(|r| r.id.as_str())
        .collect::<HashSet<_>>();
    let mut last_source_location: HashMap<&str, (usize, usize)> = HashMap::new();

    for (ch_idx, ch) in plan.chapters.iter().enumerate() {
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

        for (sec_idx, section) in ch.sections.iter().enumerate() {
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

                // Allow duplicates and treat them as "overwrite":
                // If the same page id appears multiple times across sections, keep only the last
                // occurrence and drop earlier ones.
                if let Some((prev_ch_idx, prev_sec_idx)) =
                    last_source_location.insert(src.as_str(), (ch_idx, sec_idx))
                {
                    tracing::info!(
                        source_id = src,
                        prev_chapter_index = prev_ch_idx,
                        prev_section_index = prev_sec_idx,
                        chapter_index = ch_idx,
                        section_index = sec_idx,
                        "toc plan duplicate source id; overwriting earlier occurrence"
                    );
                }
            }
        }
    }

    let selected = last_source_location.keys().copied().collect::<HashSet<_>>();
    if selected.is_empty() {
        anyhow::bail!("toc plan does not include any manifest pages");
    }

    if selected.len() != manifest_ids.len() {
        let mut omitted = Vec::new();
        for id in &manifest_ids {
            if !selected.contains(id) {
                omitted.push((*id).to_owned());
            }
        }
        omitted.sort();
        let sample = omitted.iter().take(10).cloned().collect::<Vec<_>>();
        tracing::info!(
            selected_pages = selected.len(),
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

    let mut chapters = Vec::new();
    for (ch_idx, ch) in plan.chapters.iter().enumerate() {
        let mut sections = Vec::new();
        for (sec_idx, s) in ch.sections.iter().enumerate() {
            let mut unique_in_section = HashSet::new();
            let sources = s
                .sources
                .iter()
                .filter(|src| last_source_location.get(src.as_str()) == Some(&(ch_idx, sec_idx)))
                .filter(|src| unique_in_section.insert(src.as_str()))
                .cloned()
                .collect::<Vec<_>>();

            if sources.is_empty() {
                tracing::info!(
                    chapter_index = ch_idx,
                    section_index = sec_idx,
                    section_title = %s.title,
                    "toc plan section has no sources after deduplication; dropping"
                );
                continue;
            }

            sections.push(TocSection {
                title: s.title.clone(),
                sources,
            });
        }

        if sections.is_empty() {
            tracing::info!(
                chapter_index = ch_idx,
                chapter_title = %ch.title,
                "toc plan chapter has no sections after deduplication; dropping"
            );
            continue;
        }

        let mut gains = ch.reader_gains.clone();
        gains.retain(|g| !g.trim().is_empty());
        chapters.push((ch, gains, sections));
    }

    if chapters.is_empty() {
        anyhow::bail!("toc plan has no chapters after deduplication");
    }
    if chapters.len() > 99 {
        anyhow::bail!(
            "too many chapters ({}); chapter ids are limited to ch01..ch99",
            chapters.len()
        );
    }

    let chapters = chapters
        .into_iter()
        .enumerate()
        .map(|(idx, (ch, gains, sections))| TocChapter {
            id: format!("ch{:02}", idx + 1),
            title: ch.title.clone(),
            intent: ch.intent.clone(),
            reader_gains: gains,
            sections,
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

fn strip_front_matter(contents: &str) -> &str {
    let mut lines = contents.lines();
    let Some(first) = lines.next() else {
        return contents;
    };
    if first.trim_end() != "---" {
        return contents;
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
            return &contents[offset..];
        }
    }

    contents
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_args() -> TocCreateArgs {
        TocCreateArgs {
            manifest: "manifest.jsonl".to_owned(),
            out: "toc.yaml".to_owned(),
            book_title: None,
            force: false,
            language: "日本語".to_owned(),
            tone: "丁寧".to_owned(),
            engine: LlmEngine::Noop,
        }
    }

    fn record(id: &str) -> ManifestRecord {
        ManifestRecord {
            id: id.to_owned(),
            url: format!("https://example.com/{id}"),
            title: id.to_owned(),
            path: "/docs".to_owned(),
            extracted_md: "extracted/pages/example.md".to_owned(),
            trust_tier: None,
        }
    }

    #[test]
    fn toc_from_plan_overwrites_duplicate_sources() -> anyhow::Result<()> {
        let args = test_args();
        let records = vec![record("p1"), record("p2")];

        let plan = TocPlan {
            book_title: "Test Book".to_owned(),
            chapters: vec![TocPlanChapter {
                title: "Chapter".to_owned(),
                intent: "Intent".to_owned(),
                reader_gains: vec!["Gain".to_owned()],
                sections: vec![
                    TocPlanSection {
                        title: "Section 1".to_owned(),
                        sources: vec!["p1".to_owned(), "p2".to_owned()],
                    },
                    TocPlanSection {
                        title: "Section 2".to_owned(),
                        sources: vec!["p1".to_owned()],
                    },
                ],
            }],
        };

        let toc = toc_from_plan(&args, &records, &plan)?;
        let sections = &toc.parts[0].chapters[0].sections;

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Section 1");
        assert_eq!(sections[0].sources, vec!["p2"]);
        assert_eq!(sections[1].title, "Section 2");
        assert_eq!(sections[1].sources, vec!["p1"]);

        Ok(())
    }

    #[test]
    fn toc_from_plan_drops_sections_that_become_empty() -> anyhow::Result<()> {
        let args = test_args();
        let records = vec![record("p1")];

        let plan = TocPlan {
            book_title: "Test Book".to_owned(),
            chapters: vec![TocPlanChapter {
                title: "Chapter".to_owned(),
                intent: "Intent".to_owned(),
                reader_gains: vec!["Gain".to_owned()],
                sections: vec![
                    TocPlanSection {
                        title: "Section 1".to_owned(),
                        sources: vec!["p1".to_owned()],
                    },
                    TocPlanSection {
                        title: "Section 2".to_owned(),
                        sources: vec!["p1".to_owned()],
                    },
                ],
            }],
        };

        let toc = toc_from_plan(&args, &records, &plan)?;
        let sections = &toc.parts[0].chapters[0].sections;

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Section 2");
        assert_eq!(sections[0].sources, vec!["p1"]);

        Ok(())
    }
}
