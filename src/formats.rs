use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFrontMatter {
    pub id: String,
    pub url: String,
    pub retrieved_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_html_path: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestRecord {
    pub id: String,
    pub url: String,
    pub title: String,
    pub path: String,
    pub extracted_md: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Toc {
    pub book_title: String,
    pub parts: Vec<TocPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocPart {
    pub title: String,
    pub chapters: Vec<TocChapter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocChapter {
    pub id: String,
    pub title: String,
    pub intent: String,
    pub reader_gains: Vec<String>,
    pub sections: Vec<TocSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocSection {
    pub title: String,
    pub sources: Vec<String>,
}
