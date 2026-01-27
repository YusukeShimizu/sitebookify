use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::cli::{LlmEngine, TocInitArgs, TocRefineArgs};
use crate::formats::{ManifestRecord, Toc, TocChapter, TocPart};
use crate::openai;

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

pub async fn refine(args: TocRefineArgs) -> anyhow::Result<()> {
    let manifest_path = PathBuf::from(&args.manifest);
    let out_path = PathBuf::from(&args.out);

    if out_path.exists() && !args.force {
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
    if records.is_empty() {
        anyhow::bail!("manifest is empty: {}", manifest_path.display());
    }

    records.sort_by(|a, b| a.path.cmp(&b.path));

    let chapter_title = derive_chapter_title(&records);
    let book_title = args
        .book_title
        .clone()
        .unwrap_or_else(|| format!("{chapter_title} Textbook"));

    let plan = match args.engine {
        LlmEngine::Noop => refine_noop(&records),
        LlmEngine::Command => refine_via_command(&args, &book_title, &records)?,
        LlmEngine::Openai => refine_via_openai(&args, &book_title, &records).await?,
    };

    let toc = toc_from_plan(&plan, &book_title, &records).context("build toc from plan")?;

    let yaml = serde_yaml::to_string(&toc).context("serialize toc")?;
    let mut out = open_output_file(&out_path, args.force)?;
    out.write_all(yaml.as_bytes())
        .with_context(|| format!("write toc: {}", out_path.display()))?;
    out.flush().context("flush toc")?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TocPlan {
    #[serde(default)]
    book_title: Option<String>,
    chapters: Vec<TocPlanChapter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TocPlanChapter {
    title: String,
    sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TocRefineInput {
    book_title: String,
    pages: Vec<TocRefinePage>,
}

#[derive(Debug, Clone, Serialize)]
struct TocRefinePage {
    id: String,
    path: String,
    title: String,
    url: String,
}

fn refine_noop(records: &[ManifestRecord]) -> TocPlan {
    TocPlan {
        book_title: None,
        chapters: records
            .iter()
            .map(|r| TocPlanChapter {
                title: r.title.clone(),
                sources: vec![r.id.clone()],
            })
            .collect(),
    }
}

fn refine_via_command(
    args: &TocRefineArgs,
    book_title: &str,
    records: &[ManifestRecord],
) -> anyhow::Result<TocPlan> {
    let Some(program) = args.command.as_deref() else {
        anyhow::bail!("missing --command (required when --engine=command)");
    };

    let input_json = build_refine_input_json(book_title, records).context("build refine input")?;

    tracing::info!(engine = "command", command = program, "toc refine");

    let mut child = Command::new(program)
        .args(&args.command_args)
        .env("SITEBOOKIFY_TOC_REFINE_MANIFEST", &args.manifest)
        .env("SITEBOOKIFY_TOC_REFINE_OUT", &args.out)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn toc refine command: {program}"))?;

    {
        let mut stdin = child.stdin.take().context("open toc refine stdin")?;
        stdin
            .write_all(input_json.as_bytes())
            .context("write toc refine stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("wait toc refine process")?;
    if !output.status.success() {
        anyhow::bail!("toc refine command failed: {program} ({})", output.status);
    }

    let stdout =
        String::from_utf8(output.stdout).context("toc refine stdout is not valid UTF-8")?;
    let json = extract_json_object(&stdout).context("extract json object from stdout")?;
    serde_json::from_str(json).context("parse toc plan json")
}

async fn refine_via_openai(
    args: &TocRefineArgs,
    book_title: &str,
    records: &[ManifestRecord],
) -> anyhow::Result<TocPlan> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY is not set"))?;

    let input_json = build_refine_input_json(book_title, records).context("build refine input")?;

    tracing::info!(
        engine = "openai",
        model = %args.openai_model,
        out = %args.out,
        "toc refine"
    );

    let endpoint = openai::responses_endpoint(&args.openai_base_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("build http client")?;

    let instructions = "You are a book editor.\n\
Task: Given the input JSON (book_title and pages), propose a good book title, an improved chapter grouping, and a reading order suitable for a book.\n\
\n\
Rules:\n\
- Use ONLY the provided page IDs.\n\
- Each page ID MUST appear at most once across all chapters.\n\
- You MAY omit pages that are not suitable for a book (e.g. nav/search/index/legal/changelog).\n\
- Each chapter MUST have a non-empty title and at least 1 source page.\n\
- book_title MUST be a non-empty string.\n\
- Output MUST be valid JSON and MUST match this schema:\n\
  {\"book_title\":\"...\",\"chapters\":[{\"title\":\"...\",\"sources\":[\"p_...\"]}]}\n\
- Output JSON ONLY (no markdown fences, no commentary).\n";

    let raw = openai::responses_text(
        &client,
        &endpoint,
        &api_key,
        &args.openai_model,
        instructions,
        &input_json,
        args.openai_temperature,
    )
    .await
    .context("call OpenAI Responses API")?;

    let json = extract_json_object(&raw).context("extract json object from OpenAI output")?;
    serde_json::from_str(json).context("parse toc plan json")
}

fn build_refine_input_json(book_title: &str, records: &[ManifestRecord]) -> anyhow::Result<String> {
    let pages = records
        .iter()
        .map(|r| TocRefinePage {
            id: r.id.clone(),
            path: r.path.clone(),
            title: r.title.clone(),
            url: r.url.clone(),
        })
        .collect::<Vec<_>>();
    let input = TocRefineInput {
        book_title: book_title.to_owned(),
        pages,
    };
    serde_json::to_string(&input).context("serialize toc refine input json")
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

fn toc_from_plan(
    plan: &TocPlan,
    default_book_title: &str,
    records: &[ManifestRecord],
) -> anyhow::Result<Toc> {
    if plan.chapters.is_empty() {
        anyhow::bail!("toc plan has no chapters");
    }

    let mut manifest_ids = std::collections::HashSet::new();
    for r in records {
        manifest_ids.insert(r.id.as_str());
    }

    let mut seen = std::collections::HashSet::new();
    for ch in &plan.chapters {
        if ch.title.trim().is_empty() {
            anyhow::bail!("toc plan chapter title is empty");
        }
        if ch.sources.is_empty() {
            anyhow::bail!("toc plan chapter sources is empty");
        }
        for src in &ch.sources {
            if !manifest_ids.contains(src.as_str()) {
                anyhow::bail!("unknown source id in toc plan: {src}");
            }
            if !seen.insert(src.as_str()) {
                anyhow::bail!("duplicate source id in toc plan: {src}");
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

    if plan.chapters.len() > 99 {
        anyhow::bail!(
            "too many chapters ({}); chapter ids are limited to ch01..ch99",
            plan.chapters.len()
        );
    }

    let chapters = plan
        .chapters
        .iter()
        .enumerate()
        .map(|(idx, ch)| TocChapter {
            id: format!("ch{:02}", idx + 1),
            title: ch.title.clone(),
            sources: ch.sources.clone(),
        })
        .collect::<Vec<_>>();

    Ok(Toc {
        book_title: plan
            .book_title
            .clone()
            .unwrap_or_else(|| default_book_title.to_owned()),
        parts: vec![TocPart {
            title: "Part 1".to_owned(),
            chapters,
        }],
    })
}

fn open_output_file(path: &PathBuf, force: bool) -> anyhow::Result<std::fs::File> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create toc dir: {}", parent.display()))?;
    }

    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    options
        .open(path)
        .with_context(|| format!("open toc output: {}", path.display()))
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
