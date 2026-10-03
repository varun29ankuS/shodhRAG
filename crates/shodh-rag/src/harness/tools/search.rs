//! `search_documents`: hybrid search over the indexed documents.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::{
    req_str, CitedPassage, HostTool, ToolContext, ToolError, ToolOutput, MAX_MODEL_OUTPUT_CHARS,
    UNTRUSTED_NOTICE,
};
use crate::harness::events::RiskTier;
use crate::harness::protocol::ToolLoadMode;
use crate::harness::truncate_chars;
use crate::rag_engine::page_numbers_from_metadata;
use crate::types::{ComprehensiveResult, MetadataFilter};
use crate::RAGEngine;

pub const SEARCH_DOCUMENTS: &str = "search_documents";

const DEFAULT_K: usize = 8;
const MAX_K: usize = 20;
const MAX_SOURCES: usize = 20;
const MAX_PASSAGE_CHARS: usize = 1_500;
/// Shortest passage text kept when many passages share the output budget.
const MIN_PASSAGE_CHARS: usize = 300;
/// Characters reserved per passage for its JSON fields other than `text`.
const PASSAGE_OVERHEAD_CHARS: usize = 400;

/// Text budget per passage so that all `k` numbered passages reach the model
/// whole: the registry caps tool output at [`MAX_MODEL_OUTPUT_CHARS`], and a
/// passage cut off there would leave a citation number the model never saw.
fn passage_budget(k: usize) -> usize {
    let available = MAX_MODEL_OUTPUT_CHARS.saturating_sub(UNTRUSTED_NOTICE.len() + 200);
    (available / k.max(1))
        .saturating_sub(PASSAGE_OVERHEAD_CHARS)
        .clamp(MIN_PASSAGE_CHARS, MAX_PASSAGE_CHARS)
}

/// Supplies the number of passages a search returns when the model does
/// not pass `k` (the user's preference).
pub type DefaultK = Arc<dyn Fn() -> usize + Send + Sync>;

pub struct SearchDocumentsTool {
    rag: Arc<RwLock<RAGEngine>>,
    default_k: Option<DefaultK>,
}

impl SearchDocumentsTool {
    pub fn new(rag: Arc<RwLock<RAGEngine>>) -> Self {
        Self {
            rag,
            default_k: None,
        }
    }

    /// Use the user's preferred passage count when `k` is not given.
    pub fn with_default_k(mut self, default_k: DefaultK) -> Self {
        self.default_k = Some(default_k);
        self
    }

    fn default_k(&self) -> usize {
        self.default_k
            .as_ref()
            .map(|f| f())
            .unwrap_or_else(|| self.default_k())
            .clamp(1, MAX_K)
    }
}

/// One numbered passage. `n` is the citation number the model writes as
/// `[n]`; numbers continue across every search of one answer.
#[derive(Debug, Serialize)]
struct Passage {
    n: u32,
    file: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    heading: Option<String>,
    score: f32,
    text: String,
}

/// What the user sees for a source: the file name for files, and the record's
/// title for in-app records (`calendar://task/<id>` would otherwise show its id).
fn display_name(path: &str, result: &ComprehensiveResult) -> String {
    if let Some((scheme, rest)) = path.split_once("://") {
        let title = result
            .metadata
            .get("title")
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .or_else(|| Some(result.citation.title.trim()).filter(|t| !t.is_empty()));
        let kind = match (scheme, rest.split('/').next()) {
            ("calendar", Some("task")) => "Task",
            ("calendar", Some("event")) => "Event",
            ("calendar", _) => "Calendar",
            ("note", _) => "Note",
            _ => "",
        };
        return match (kind, title) {
            ("", Some(t)) => t.to_string(),
            ("", None) => path.to_string(),
            (k, Some(t)) => format!("{k}: {t}"),
            (k, None) => format!("{k} (untitled)"),
        };
    }
    path.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| result.citation.title.clone())
}

fn passage(n: u32, result: &ComprehensiveResult, max_chars: usize) -> Passage {
    let path = if result.citation.source.is_empty() {
        result
            .metadata
            .get("file_path")
            .or_else(|| result.metadata.get("source_file"))
            .cloned()
            .unwrap_or_default()
    } else {
        result.citation.source.clone()
    };
    let file = display_name(&path, result);
    Passage {
        n,
        file,
        path,
        page: page_numbers_from_metadata(&result.metadata),
        heading: result
            .metadata
            .get("heading")
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty()),
        score: result.score,
        text: truncate_chars(result.snippet.trim(), max_chars),
    }
}

#[async_trait]
impl HostTool for SearchDocumentsTool {
    fn name(&self) -> &'static str {
        SEARCH_DOCUMENTS
    }
    fn label(&self) -> &'static str {
        "Search documents"
    }
    fn label_template(&self) -> &'static str {
        "Searching {query}"
    }
    fn description(&self) -> &'static str {
        "Search the user's indexed documents (hybrid keyword + semantic). Returns passages with \
         file, path, page and score. Optionally restrict to source ids from list_sources. Use \
         specific queries; search again with different wording if the first results miss."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1, "maxLength": 500},
                "sources": {
                    "type": "array",
                    "items": {"type": "string", "minLength": 1, "maxLength": 200},
                    "maxItems": MAX_SOURCES
                },
                "k": {"type": "integer", "minimum": 1, "maximum": MAX_K}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        ToolLoadMode::Essential
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let query = req_str(&args, "query", SEARCH_DOCUMENTS)?;
        let k = args
            .get("k")
            .and_then(Value::as_u64)
            .and_then(|k| usize::try_from(k).ok())
            .unwrap_or_else(|| self.default_k())
            .clamp(1, MAX_K);
        let sources: Vec<String> = args
            .get("sources")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default();

        let rag = self.rag.read().await;
        // The user's limit for this answer applies unless the model chose
        // sources itself.
        let scope = ctx.scope();
        let (sources, scoped_files) = if sources.is_empty() && !scope.is_empty() {
            let mut files = Vec::new();
            for file in &scope.files {
                let matches = rag
                    .find_indexed_sources(file)
                    .await
                    .map_err(|e| ToolError::Failed(format!("Could not read the index: {e}")))?;
                files.extend(matches);
            }
            files.sort();
            files.dedup();
            (scope.source_ids.clone(), files)
        } else {
            (sources, Vec::new())
        };
        let mut results = if !scoped_files.is_empty() {
            let mut merged = Vec::new();
            for file in &scoped_files {
                let filter = MetadataFilter {
                    source_path: Some(file.clone()),
                    ..MetadataFilter::default()
                };
                let hits = rag
                    .search_comprehensive(query, k, Some(filter))
                    .await
                    .map_err(|e| ToolError::Failed(format!("Search failed: {e}")))?;
                merged.extend(hits);
            }
            merged.sort_by(|a, b| b.score.total_cmp(&a.score));
            merged
        } else if sources.is_empty() {
            rag.search_comprehensive(query, k, None)
                .await
                .map_err(|e| ToolError::Failed(format!("Search failed: {e}")))?
        } else {
            let mut merged = Vec::new();
            for source in &sources {
                let filter = MetadataFilter {
                    space_id: Some(source.clone()),
                    ..MetadataFilter::default()
                };
                let hits = rag
                    .search_comprehensive(query, k, Some(filter))
                    .await
                    .map_err(|e| ToolError::Failed(format!("Search failed: {e}")))?;
                merged.extend(hits);
            }
            merged.sort_by(|a, b| b.score.total_cmp(&a.score));
            merged
        };
        drop(rag);
        results.truncate(k);

        let limited_to = if !scoped_files.is_empty() {
            Some(format!(
                "The user limited this answer to {}.",
                scoped_files.join(", ")
            ))
        } else if args.get("sources").is_none() && !ctx.scope().source_ids.is_empty() {
            Some(format!(
                "The user limited this answer to sources {}.",
                ctx.scope().source_ids.join(", ")
            ))
        } else {
            None
        };
        if results.is_empty() {
            return Ok(ToolOutput {
                text_for_model: format!(
                    "No passages matched \"{query}\". Try different wording or check list_sources.{}",
                    limited_to.as_deref().map(|l| format!(" {l}")).unwrap_or_default()
                ),
                summary_for_ui: "No matching passages".to_string(),
                detail: Some(json!({ "passages": [] })),
            });
        }

        let count = u32::try_from(results.len()).unwrap_or(u32::MAX);
        let first = ctx.reserve_passages(count);
        let budget = passage_budget(results.len());
        let passages: Vec<Passage> = results
            .iter()
            .zip(first..)
            .map(|(r, n)| passage(n, r, budget))
            .collect();
        for p in &passages {
            ctx.record_passage(CitedPassage {
                n: p.n,
                file: p.file.clone(),
                path: p.path.clone(),
                page: p.page.clone(),
                web: false,
            });
        }
        let files: HashSet<&str> = passages.iter().map(|p| p.path.as_str()).collect();
        let body = serde_json::to_string(&passages)
            .map_err(|e| ToolError::Failed(format!("Could not encode results: {e}")))?;
        let detail = json!({ "passages": passages });
        let noun = if passages.len() == 1 {
            "passage"
        } else {
            "passages"
        };
        let file_noun = if files.len() == 1 { "file" } else { "files" };
        Ok(ToolOutput {
            text_for_model: match &limited_to {
                Some(limit) => format!("{UNTRUSTED_NOTICE}\n{limit}\n{body}"),
                None => format!("{UNTRUSTED_NOTICE}\n{body}"),
            },
            summary_for_ui: format!("{} {noun} from {} {file_noun}", passages.len(), files.len()),
            detail: Some(detail),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Citation;
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn passages_carry_file_page_and_number() {
        let mut metadata = HashMap::new();
        metadata.insert("page_start".to_string(), "4".to_string());
        metadata.insert("page_end".to_string(), "4".to_string());
        let result = ComprehensiveResult {
            id: Uuid::nil(),
            score: 0.5,
            metadata,
            citation: Citation {
                title: "Acme MSA".into(),
                source: "c:/docs/acme_msa.pdf".into(),
                ..Citation::default()
            },
            snippet: "  Notice period is sixty days. ".into(),
            source_index: "hybrid".into(),
        };
        let p = passage(3, &result, MAX_PASSAGE_CHARS);
        assert_eq!(p.n, 3);
        assert_eq!(p.file, "acme_msa.pdf");
        assert_eq!(p.page.as_deref(), Some("4"));
        assert_eq!(p.text, "Notice period is sixty days.");
    }

    #[test]
    fn calendar_records_are_labelled_by_title_not_id() {
        let mut metadata = HashMap::new();
        metadata.insert("title".to_string(), "  File GST return ".to_string());
        let result = ComprehensiveResult {
            id: Uuid::nil(),
            score: 0.5,
            metadata,
            citation: Citation {
                source: "calendar://task/ae887fee-eaa5-4c05-b53d-9b3e9905edef".into(),
                ..Citation::default()
            },
            snippet: "Task: File GST return.".into(),
            source_index: "hybrid".into(),
        };
        let p = passage(1, &result, MAX_PASSAGE_CHARS);
        assert_eq!(p.file, "Task: File GST return");
        assert_eq!(
            p.path,
            "calendar://task/ae887fee-eaa5-4c05-b53d-9b3e9905edef"
        );

        let mut untitled = result;
        untitled.metadata.clear();
        untitled.citation.source = "calendar://event/1".into();
        assert_eq!(
            passage(2, &untitled, MAX_PASSAGE_CHARS).file,
            "Event (untitled)"
        );
    }

    #[test]
    fn every_numbered_passage_fits_the_model_output_cap() {
        for k in 1..=MAX_K {
            let budget = passage_budget(k);
            assert!(budget <= MAX_PASSAGE_CHARS);
            let worst = UNTRUSTED_NOTICE.len() + k * (budget + PASSAGE_OVERHEAD_CHARS);
            assert!(
                worst <= MAX_MODEL_OUTPUT_CHARS,
                "k={k}: {worst} > {MAX_MODEL_OUTPUT_CHARS}"
            );
        }
        assert_eq!(passage_budget(DEFAULT_K), MAX_PASSAGE_CHARS);
    }
}
