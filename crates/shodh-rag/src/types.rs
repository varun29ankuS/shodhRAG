use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub title: String,
    pub authors: Vec<String>,
    pub source: String,
    pub year: String,
    pub url: Option<String>,
    pub doi: Option<String>,
    pub page_numbers: Option<String>,
}

impl Default for Citation {
    fn default() -> Self {
        Self {
            title: String::new(),
            authors: Vec::new(),
            source: String::new(),
            year: String::new(),
            url: None,
            doi: None,
            page_numbers: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleSearchResult {
    pub id: Uuid,
    pub score: f32,
    pub text: String,
    pub metadata: HashMap<String, String>,
    pub title: String,
    pub source: String,
    pub heading: Option<String>,
    pub citation: Option<Citation>,
    pub doc_id: Uuid,
    pub chunk_id: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComprehensiveResult {
    pub id: Uuid,
    pub score: f32,
    pub metadata: HashMap<String, String>,
    pub citation: Citation,
    pub snippet: String,
    pub source_index: String,
}

impl crate::rag::query_decomposer::HasIdAndScore for ComprehensiveResult {
    fn result_id(&self) -> String {
        self.id.to_string()
    }
    fn result_score(&self) -> f32 {
        self.score
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum DocumentFormat {
    TXT,
    MD,
    HTML,
    JSON,
    PDF,
    CSV,
    Spreadsheet,
    Presentation,
    Code,
}

impl DocumentFormat {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "txt" => Self::TXT,
            "md" | "markdown" => Self::MD,
            "html" | "htm" => Self::HTML,
            "json" => Self::JSON,
            "pdf" => Self::PDF,
            "csv" => Self::CSV,
            "xlsx" | "xls" | "ods" | "xlsm" | "xlsb" => Self::Spreadsheet,
            "pptx" | "ppt" | "odp" => Self::Presentation,
            "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "java" | "c" | "cpp" | "h"
            | "hpp" | "cs" | "rb" | "php" | "swift" | "kt" | "scala" | "r" | "sql" | "sh"
            | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "yaml" | "yml" | "toml" | "xml"
            | "ini" | "cfg" | "conf" | "env" | "dockerfile" | "makefile" => Self::Code,
            _ => Self::TXT,
        }
    }
}

/// Structured section extracted from a document (PDF form, table, etc.).
/// Used to produce high-quality, relationship-preserving chunks.
#[derive(Debug, Clone)]
pub enum DocumentSection {
    /// Narrative text from a page.
    Text {
        content: String,
        page: usize,
        heading: Option<String>,
    },
    /// Form field key-value pairs (AcroForm, annotations).
    FormFields {
        fields: Vec<(String, String)>,
        page: usize,
    },
    /// Tabular data.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        page: usize,
        caption: Option<String>,
    },
    /// Synthesized relationship text from form data + annotations.
    Relationships { content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetadataFilter {
    pub space_id: Option<String>,
    pub source_type: Option<String>,
    pub source_path: Option<String>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    pub custom: Option<HashMap<String, String>>,
}

impl MetadataFilter {
    pub fn to_lance_predicate(&self) -> Option<String> {
        let mut predicates = Vec::new();

        if let Some(ref space_id) = self.space_id {
            predicates.push(format!("space_id = '{}'", space_id.replace('\'', "''")));
        }
        if let Some(ref source_path) = self.source_path {
            predicates.push(format!("source = '{}'", source_path.replace('\'', "''")));
        }
        if let Some(from) = self.date_from {
            predicates.push(format!("created_at >= {}", from));
        }
        if let Some(to) = self.date_to {
            predicates.push(format!("created_at <= {}", to));
        }

        if predicates.is_empty() {
            None
        } else {
            Some(predicates.join(" AND "))
        }
    }
}

/// Internal chunk record for storage operations
#[derive(Debug, Clone)]
pub struct ChunkRecord {
    pub id: String,
    pub doc_id: String,
    pub chunk_index: u32,
    pub text: String,
    pub title: String,
    pub source: String,
    pub heading: String,
    pub vector: Vec<f32>,
    pub space_id: String,
    pub metadata_json: String,
    pub citation_json: String,
    pub created_at: i64,
}
