use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::Context as _;
use url::Url;

use crate::cli::ManifestArgs;
use crate::formats::{ExtractedFrontMatter, ManifestRecord};

pub fn run(args: ManifestArgs) -> anyhow::Result<()> {
    let extracted_dir = PathBuf::from(&args.extracted);
    let out_path = PathBuf::from(&args.out);

    if out_path.exists() {
        anyhow::bail!("manifest output already exists: {}", out_path.display());
    }

    let pages_dir = extracted_dir.join("pages");
    let mut records = Vec::new();

    for entry in std::fs::read_dir(&pages_dir)
        .with_context(|| format!("read extracted pages dir: {}", pages_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("read extracted page: {}", path.display()))?;
        let front = parse_front_matter(&contents)
            .with_context(|| format!("parse front matter: {}", path.display()))?;

        let url = Url::parse(&front.url).context("parse front matter url")?;
        let record = ManifestRecord {
            id: front.id,
            url: front.url,
            title: front.title,
            path: url.path().to_owned(),
            extracted_md: path.to_string_lossy().to_string(),
        };
        records.push(record);
    }

    records.sort_by(|a, b| a.path.cmp(&b.path));

    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&out_path)
        .with_context(|| format!("create manifest: {}", out_path.display()))?;
    for record in records {
        serde_json::to_writer(&mut out, &record).context("serialize manifest record")?;
        out.write_all(b"\n").context("write manifest newline")?;
    }
    out.flush().context("flush manifest")?;

    Ok(())
}

fn parse_front_matter(contents: &str) -> anyhow::Result<ExtractedFrontMatter> {
    let mut lines = contents.lines();
    let first = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("extracted page is empty"))?;
    if first.trim_end() != "---" {
        anyhow::bail!("extracted page must start with YAML front matter ('---')");
    }

    let mut yaml_lines = Vec::new();
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        yaml_lines.push(line);
    }

    let yaml = yaml_lines.join("\n");
    let front: ExtractedFrontMatter =
        serde_yaml::from_str(&yaml).context("deserialize extracted front matter")?;
    Ok(front)
}
