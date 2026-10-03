//! `list_sources`: the indexed folders, with file and chunk counts.
//!
//! The backend does not keep a folder registry: the UI's source list lives in
//! the WebView, and indexing tags every chunk with the source's `space_id` and
//! the file's normalised path. Sources are therefore derived from the index:
//! chunks are grouped by `space_id`, and a source's folder is the deepest
//! directory shared by all of its files. If every file of a folder sits in one
//! subdirectory, the derived folder is that subdirectory.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::{HostTool, ToolContext, ToolError, ToolOutput};
use crate::harness::events::RiskTier;
use crate::storage::DocumentSourceRow;
use crate::RAGEngine;

pub const LIST_SOURCES: &str = "list_sources";

/// What a source holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Folder,
    Calendar,
    Notes,
}

/// One indexed source, derived from the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceSummary {
    /// The `space_id` the source was indexed under.
    pub source_id: String,
    pub kind: SourceKind,
    /// Deepest folder containing every file (folders only).
    pub folder: Option<String>,
    pub files: usize,
    pub chunks: usize,
}

fn kind_of(source: &str) -> SourceKind {
    if source.starts_with("calendar://") {
        SourceKind::Calendar
    } else if source.starts_with("note://") {
        SourceKind::Notes
    } else {
        SourceKind::Folder
    }
}

fn parent_components(path: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    parts
}

fn common_folder(paths: &[&str]) -> Option<String> {
    let mut iter = paths.iter();
    let mut common = parent_components(iter.next()?);
    for path in iter {
        let parts = parent_components(path);
        let shared = common
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a == b)
            .count();
        common.truncate(shared);
    }
    let folder = common.join("/");
    if folder.is_empty() {
        None
    } else if folder.ends_with(':') {
        Some(format!("{folder}/"))
    } else {
        Some(folder)
    }
}

/// Group document rows into sources. Rows without a `space_id` are skipped
/// because no tool can address them.
pub fn summarise_sources(rows: &[DocumentSourceRow]) -> Vec<SourceSummary> {
    let mut groups: BTreeMap<&str, Vec<&DocumentSourceRow>> = BTreeMap::new();
    for row in rows {
        if row.space_id.trim().is_empty() {
            continue;
        }
        groups.entry(row.space_id.as_str()).or_default().push(row);
    }
    groups
        .into_iter()
        .map(|(space_id, rows)| {
            let files: HashSet<&str> = rows.iter().map(|r| r.source.as_str()).collect();
            let chunks = rows.iter().map(|r| r.chunks).sum();
            let kind = rows
                .first()
                .map(|r| kind_of(&r.source))
                .unwrap_or(SourceKind::Folder);
            let folder = match kind {
                SourceKind::Folder => {
                    let mut paths: Vec<&str> = files.iter().copied().collect();
                    paths.sort_unstable();
                    common_folder(&paths)
                }
                SourceKind::Calendar | SourceKind::Notes => None,
            };
            SourceSummary {
                source_id: space_id.to_string(),
                kind,
                folder,
                files: files.len(),
                chunks,
            }
        })
        .collect()
}

const DEFAULT_FILES: usize = 50;
const MAX_FILES: usize = 500;

/// One indexed file of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceFile {
    pub path: String,
    pub chunks: usize,
}

/// The indexed files of `source_id`, sorted by path.
pub fn files_of(rows: &[DocumentSourceRow], source_id: &str) -> Vec<SourceFile> {
    let mut files: BTreeMap<&str, usize> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.space_id == source_id) {
        *files.entry(row.source.as_str()).or_default() += row.chunks;
    }
    files
        .into_iter()
        .map(|(path, chunks)| SourceFile {
            path: path.to_string(),
            chunks,
        })
        .collect()
}

/// Load every source from the index.
pub async fn load_sources(rag: &RAGEngine) -> Result<Vec<SourceSummary>, ToolError> {
    let rows = rag
        .document_sources()
        .await
        .map_err(|e| ToolError::Failed(format!("Could not read the index: {e}")))?;
    Ok(summarise_sources(&rows))
}

/// Find an indexed folder source by id.
pub async fn find_folder_source(
    rag: &RAGEngine,
    source_id: &str,
) -> Result<SourceSummary, ToolError> {
    let sources = load_sources(rag).await?;
    match sources.into_iter().find(|s| s.source_id == source_id) {
        Some(source) if source.kind == SourceKind::Folder => Ok(source),
        Some(_) => Err(ToolError::Forbidden(format!(
            "Source {source_id} is not a folder; only folder sources can be changed here"
        ))),
        None => Err(ToolError::NotFound(format!(
            "No indexed source has id {source_id}. Call list_sources for valid ids."
        ))),
    }
}

pub struct ListSourcesTool {
    rag: Arc<RwLock<RAGEngine>>,
}

impl ListSourcesTool {
    pub fn new(rag: Arc<RwLock<RAGEngine>>) -> Self {
        Self { rag }
    }
}

#[async_trait]
impl HostTool for ListSourcesTool {
    fn name(&self) -> &'static str {
        LIST_SOURCES
    }
    fn label(&self) -> &'static str {
        "List sources"
    }
    fn label_template(&self) -> &'static str {
        "Checking indexed folders"
    }
    fn description(&self) -> &'static str {
        "List the indexed sources: folders (with their path), the calendar and notes, with file \
         and chunk counts. Source ids can be passed to search_documents to restrict a search. \
         With source_id, list the indexed files of that source instead."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_id": {"type": "string", "minLength": 1, "maxLength": 200},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_FILES}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if let Some(source_id) = super::opt_str(&args, "source_id") {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(DEFAULT_FILES)
                .clamp(1, MAX_FILES);
            let rows = {
                let rag = self.rag.read().await;
                rag.document_sources()
                    .await
                    .map_err(|e| ToolError::Failed(format!("Could not read the index: {e}")))?
            };
            let files = files_of(&rows, source_id);
            if files.is_empty() {
                return Err(ToolError::NotFound(format!(
                    "No indexed source has id {source_id}. Call list_sources for valid ids."
                )));
            }
            let total = files.len();
            let shown: Vec<&SourceFile> = files.iter().take(limit).collect();
            let body = serde_json::to_string(&shown)
                .map_err(|e| ToolError::Failed(format!("Could not encode files: {e}")))?;
            let more = if total > shown.len() {
                format!(" Showing the first {}.", shown.len())
            } else {
                String::new()
            };
            return Ok(ToolOutput {
                text_for_model: format!(
                    "{total} indexed files in source {source_id}.{more}\n{body}"
                ),
                summary_for_ui: format!("{total} files"),
                detail: Some(json!({ "sourceId": source_id, "total": total, "files": shown })),
            });
        }
        let sources = {
            let rag = self.rag.read().await;
            load_sources(&rag).await?
        };
        let folders = sources
            .iter()
            .filter(|s| s.kind == SourceKind::Folder)
            .count();
        let files: usize = sources
            .iter()
            .filter(|s| s.kind == SourceKind::Folder)
            .map(|s| s.files)
            .sum();
        let body = serde_json::to_string(&sources)
            .map_err(|e| ToolError::Failed(format!("Could not encode sources: {e}")))?;
        let text = if sources.is_empty() {
            "Nothing is indexed yet. The user can add a folder in the Library view.".to_string()
        } else {
            format!(
                "{body}\nCounts are of indexed content. Indexing jobs still running are not reflected."
            )
        };
        let folder_noun = if folders == 1 { "folder" } else { "folders" };
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: format!("{folders} {folder_noun}, {files} files"),
            detail: Some(json!({ "sources": sources })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(doc: &str, source: &str, space: &str, chunks: usize) -> DocumentSourceRow {
        DocumentSourceRow {
            doc_id: doc.into(),
            source: source.into(),
            space_id: space.into(),
            chunks,
        }
    }

    #[test]
    fn files_of_a_source_are_listed_once_with_their_chunks() {
        let rows = vec![
            row("d1", "c:/docs/b.pdf", "s1", 3),
            row("d2", "c:/docs/a.pdf", "s1", 2),
            row("d3", "c:/docs/a.pdf", "s1", 1),
            row("d4", "c:/other/x.pdf", "s2", 9),
        ];
        let files = files_of(&rows, "s1");
        assert_eq!(
            files,
            vec![
                SourceFile {
                    path: "c:/docs/a.pdf".into(),
                    chunks: 3
                },
                SourceFile {
                    path: "c:/docs/b.pdf".into(),
                    chunks: 3
                },
            ]
        );
        assert!(files_of(&rows, "nope").is_empty());
    }

    #[test]
    fn sources_group_by_space_and_derive_the_folder() {
        let rows = vec![
            row("1", "c:/docs/contracts/acme.pdf", "s1", 4),
            row("2", "c:/docs/contracts/2024/bharat.pdf", "s1", 2),
            row("3", "calendar://task/9", "calendar", 1),
            row("4", "/home/u/notes/a.md", "s2", 3),
            row("5", "c:/orphan.txt", "", 1),
        ];
        let sources = summarise_sources(&rows);
        assert_eq!(sources.len(), 3);
        let s1 = sources.iter().find(|s| s.source_id == "s1").unwrap();
        assert_eq!(s1.folder.as_deref(), Some("c:/docs/contracts"));
        assert_eq!((s1.files, s1.chunks), (2, 6));
        let cal = sources.iter().find(|s| s.source_id == "calendar").unwrap();
        assert_eq!(cal.kind, SourceKind::Calendar);
        assert_eq!(cal.folder, None);
        let s2 = sources.iter().find(|s| s.source_id == "s2").unwrap();
        assert_eq!(s2.folder.as_deref(), Some("/home/u/notes"));
    }

    #[test]
    fn drive_root_folders_keep_their_separator() {
        assert_eq!(
            common_folder(&["c:/a.txt", "c:/b.txt"]).as_deref(),
            Some("c:/")
        );
        assert_eq!(common_folder(&["a.txt"]), None);
    }
}
