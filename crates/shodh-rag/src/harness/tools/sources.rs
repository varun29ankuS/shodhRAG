//! `list_sources`: the indexed folders, with file and chunk counts.
//!
//! The backend does not keep a folder registry: the UI's source list lives in
//! the WebView, and indexing tags every chunk with the source's `space_id` and
//! the file's normalised path. Sources are therefore derived from the index:
//! chunks are grouped by `space_id`, and a source's folder is the deepest
//! directory shared by all of its files. If every file of a folder sits in one
//! subdirectory, the derived folder is that subdirectory.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::{HostTool, ToolContext, ToolError, ToolOutput};
use crate::harness::events::RiskTier;
use crate::storage::DocumentSourceRow;
use crate::workspaces::path_within;
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
    /// What the source is called: its folder's name, or "Calendar" / "Notes".
    pub name: String,
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

/// The last component of a normalised path (`c:/docs/papers` -> `papers`;
/// a drive root is itself).
fn last_component(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(name) if !name.is_empty() => name,
        _ => path,
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
            let name = match (kind, &folder) {
                (SourceKind::Calendar, _) => "Calendar".to_string(),
                (SourceKind::Notes, _) => "Notes".to_string(),
                (SourceKind::Folder, Some(folder)) => last_component(folder).to_string(),
                (SourceKind::Folder, None) => space_id.to_string(),
            };
            SourceSummary {
                source_id: space_id.to_string(),
                kind,
                folder,
                name,
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

/// What `sources` of `search_documents` accepts, for its description and its
/// errors.
pub const SOURCES_ACCEPTED: &str = "source ids from list_sources, folder paths at or \
inside an indexed folder (a folder is searched with its subfolders), paths of indexed files, \
the name of a folder or file when only one indexed folder or file has it, \"notes\" or \
\"calendar\"";

/// One indexed file a source argument named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFile {
    /// The source (`space_id`) the file was indexed under.
    pub source_id: String,
    /// Its normalised path, as stored with its chunks.
    pub path: String,
}

/// What the `sources` of a search name in the index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceResolution {
    /// Whole sources, by id.
    pub source_ids: Vec<String>,
    /// Single files, and the files of folders that hold only part of a
    /// source; never a file of `source_ids`.
    pub files: Vec<ResolvedFile>,
    /// What each argument that was not a source id stood for, for the model.
    pub notes: Vec<String>,
}

impl SourceResolution {
    fn add_source(&mut self, id: &str) {
        if !self.source_ids.iter().any(|s| s == id) {
            self.source_ids.push(id.to_string());
            self.files.retain(|f| f.source_id != id);
        }
    }

    fn add_file(&mut self, row: &DocumentSourceRow) {
        if self.source_ids.contains(&row.space_id)
            || self.files.iter().any(|f| f.path == row.source)
        {
            return;
        }
        self.files.push(ResolvedFile {
            source_id: row.space_id.clone(),
            path: row.source.clone(),
        });
    }
}

/// What one source argument matched.
enum Target {
    /// Whole sources, by id.
    Sources(Vec<String>),
    /// Every indexed file at or under this normalised folder path.
    Folder(String),
    /// One indexed file, by its normalised path.
    File(String),
}

/// Whether a source argument is written as a path rather than an id or name.
fn looks_like_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.contains('/')
        || value.contains('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// A file name without its extension (`hnsw.pdf` -> `hnsw`).
fn stem(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// A folder source's folder and every folder between it and its files.
fn folders_of(summary: &SourceSummary, rows: &[DocumentSourceRow]) -> BTreeSet<String> {
    let mut folders = BTreeSet::new();
    let Some(root) = summary.folder.as_deref() else {
        return folders;
    };
    folders.insert(root.to_string());
    let base = root.trim_end_matches('/');
    for row in rows.iter().filter(|r| r.space_id == summary.source_id) {
        let Some(rest) = row.source.strip_prefix(base) else {
            continue;
        };
        let mut parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        parts.pop();
        let mut current = base.to_string();
        for part in parts {
            current = format!("{current}/{part}");
            folders.insert(current.clone());
        }
    }
    folders
}

/// What `value` names among the indexed sources: `Err` with the candidates
/// when it names several folders or files, `Err` with none when nothing.
fn match_target(
    value: &str,
    summaries: &[SourceSummary],
    rows: &[DocumentSourceRow],
) -> Result<Target, Vec<String>> {
    if summaries.iter().any(|s| s.source_id == value) {
        return Ok(Target::Sources(vec![value.to_string()]));
    }
    let lower = value.to_lowercase();
    if let Some(s) = summaries
        .iter()
        .find(|s| s.source_id.to_lowercase() == lower)
    {
        return Ok(Target::Sources(vec![s.source_id.clone()]));
    }
    let kind = match lower.as_str() {
        "notes" | "note" => Some(SourceKind::Notes),
        "calendar" => Some(SourceKind::Calendar),
        _ => None,
    };
    if let Some(kind) = kind {
        let ids: Vec<String> = summaries
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.source_id.clone())
            .collect();
        return if ids.is_empty() {
            Err(Vec::new())
        } else {
            Ok(Target::Sources(ids))
        };
    }
    if looks_like_path(value) {
        let wanted = crate::rag_engine::normalize_source_path(std::path::Path::new(value));
        if rows.iter().any(|r| r.source == wanted) {
            return Ok(Target::File(wanted));
        }
        if rows.iter().any(|r| path_within(&r.source, &wanted)) {
            return Ok(Target::Folder(wanted));
        }
        return Err(Vec::new());
    }
    // A name: of a folder at or inside a source's folder, or of a file.
    let mut folders = BTreeSet::new();
    for summary in summaries.iter().filter(|s| s.kind == SourceKind::Folder) {
        for folder in folders_of(summary, rows) {
            if last_component(&folder).to_lowercase() == lower {
                folders.insert(folder);
            }
        }
    }
    let mut files = BTreeSet::new();
    for row in rows
        .iter()
        .filter(|r| kind_of(&r.source) == SourceKind::Folder)
    {
        let name = last_component(&row.source).to_lowercase();
        if name == lower || stem(&name) == lower {
            files.insert(row.source.clone());
        }
    }
    match (folders.len(), files.len()) {
        (1, 0) => Ok(Target::Folder(
            folders.into_iter().next().unwrap_or_default(),
        )),
        (0, 1) => Ok(Target::File(files.into_iter().next().unwrap_or_default())),
        _ => Err(folders
            .into_iter()
            .map(|f| format!("folder {f}"))
            .chain(files.into_iter().map(|f| format!("file {f}")))
            .collect()),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Resolve the `sources` of a search to sources and files of the index:
/// source ids, folder paths at or inside an indexed folder (normalised the
/// way the indexer stores paths), indexed file paths, and the name of a
/// folder or file when exactly one has it ([`SOURCES_ACCEPTED`]). A folder
/// that holds every file of a source is searched as that source; any other
/// as its files.
///
/// An argument that matches nothing, or several folders or files, is an
/// error that lists the valid sources, never an empty search: "no matching
/// passages" reads to the model as "the documents do not say", which was
/// false.
pub fn resolve_sources(
    rows: &[DocumentSourceRow],
    requested: &[String],
) -> Result<SourceResolution, ToolError> {
    let summaries = summarise_sources(rows);
    let mut out = SourceResolution::default();
    let mut unknown: Vec<String> = Vec::new();
    let mut ambiguous: Vec<String> = Vec::new();
    for value in requested.iter().map(|v| v.trim()) {
        match match_target(value, &summaries, rows) {
            Ok(Target::Sources(ids)) => {
                for id in &ids {
                    out.add_source(id);
                }
                if ids.len() != 1 || ids[0] != value {
                    out.notes
                        .push(format!("\"{value}\" is source {}.", ids.join(", ")));
                }
            }
            Ok(Target::File(path)) => {
                if let Some(row) = rows.iter().find(|r| r.source == path) {
                    out.add_file(row);
                }
                out.notes
                    .push(format!("\"{value}\" is the indexed file {path}."));
            }
            Ok(Target::Folder(folder)) => {
                let whole: Vec<&str> = summaries
                    .iter()
                    .filter(|s| {
                        rows.iter()
                            .filter(|r| r.space_id == s.source_id)
                            .all(|r| path_within(&r.source, &folder))
                    })
                    .map(|s| s.source_id.as_str())
                    .collect();
                for id in &whole {
                    out.add_source(id);
                }
                let inside: BTreeSet<&str> = rows
                    .iter()
                    .filter(|r| !whole.contains(&r.space_id.as_str()))
                    .filter(|r| path_within(&r.source, &folder))
                    .map(|r| r.source.as_str())
                    .collect();
                for row in rows.iter().filter(|r| inside.contains(r.source.as_str())) {
                    out.add_file(row);
                }
                let what = match (whole.is_empty(), inside.len()) {
                    (false, 0) => format!("source {}", whole.join(", ")),
                    (true, n) => plural(n, "indexed file", "indexed files"),
                    (false, n) => format!(
                        "source {} and {}",
                        whole.join(", "),
                        plural(n, "more indexed file", "more indexed files")
                    ),
                };
                out.notes
                    .push(format!("\"{value}\" is the folder {folder}: {what}."));
            }
            Err(candidates) if candidates.is_empty() => unknown.push(format!("\"{value}\"")),
            Err(candidates) => ambiguous.push(format!(
                "\"{value}\" names more than one: {}; pass its path instead.",
                candidates.join(", ")
            )),
        }
    }
    if unknown.is_empty() && ambiguous.is_empty() {
        return Ok(out);
    }
    let mut message = Vec::new();
    if !unknown.is_empty() {
        message.push(format!("No indexed source matches {}.", unknown.join(", ")));
    }
    message.extend(ambiguous);
    message.push(format!("sources takes {SOURCES_ACCEPTED}."));
    message.push(valid_sources_text(&summaries));
    message.push(
        "Nothing was searched: search again with valid sources, or without sources to search \
         everything."
            .to_string(),
    );
    Err(ToolError::NotFound(message.join(" ")))
}

/// Most sources listed in an error.
const MAX_LISTED_SOURCES: usize = 20;

/// The valid sources, for an error.
pub fn valid_sources_text(summaries: &[SourceSummary]) -> String {
    if summaries.is_empty() {
        return "Nothing is indexed yet.".to_string();
    }
    let listed: Vec<String> = summaries
        .iter()
        .take(MAX_LISTED_SOURCES)
        .map(|s| match &s.folder {
            Some(folder) => format!(
                "{} (\"{}\", folder {folder}, {})",
                s.source_id,
                s.name,
                plural(s.files, "file", "files")
            ),
            None => format!("{} (\"{}\")", s.source_id, s.name),
        })
        .collect();
    let more = match summaries.len().saturating_sub(MAX_LISTED_SOURCES) {
        0 => String::new(),
        n => format!(" and {n} more (call list_sources)"),
    };
    format!("Valid source ids: {}{more}.", listed.join("; "))
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
        "List what is indexed for search_documents: folders (with their path and name), and the \
         calendar and notes as searchable text, with file and chunk counts. It does not list the \
         user's tasks or events: list_tasks and list_events do. Source ids can be passed to \
         search_documents to restrict a search. With source_id (an id this tool listed), list the \
         indexed files of that source instead; leave it out to list every source."
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
                    "No indexed source has id {source_id}. {} Leave source_id out to list every \
                     source.",
                    valid_sources_text(&summarise_sources(&rows))
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
                summary_for_ui: format!("{total} {}", if total == 1 { "file" } else { "files" }),
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
        let file_noun = if files == 1 { "file" } else { "files" };
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: format!("{folders} {folder_noun}, {files} {file_noun}"),
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

    fn library() -> Vec<DocumentSourceRow> {
        vec![
            row("1", "c:/docs/papers/survey.pdf", "s1", 4),
            row("2", "c:/docs/papers/ann/hnsw.pdf", "s1", 6),
            row("3", "c:/docs/papers/ann/nsg.pdf", "s1", 5),
            row("4", "c:/docs/contracts/acme.pdf", "s2", 3),
            row("5", "c:/docs/contracts/old/acme.pdf", "s2", 2),
            row("6", "note://n1", "notes", 1),
            row("7", "calendar://task/9", "calendar", 1),
        ]
    }

    fn resolve(requested: &[&str]) -> Result<SourceResolution, ToolError> {
        let requested: Vec<String> = requested.iter().map(|s| s.to_string()).collect();
        resolve_sources(&library(), &requested)
    }

    fn file_paths(resolution: &SourceResolution) -> Vec<&str> {
        let mut paths: Vec<&str> = resolution.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        paths
    }

    #[test]
    fn sources_resolve_ids_folders_files_and_names() {
        // A source id, and an id in other case.
        assert_eq!(resolve(&["s1"]).unwrap().source_ids, vec!["s1"]);
        assert_eq!(resolve(&["S1"]).unwrap().source_ids, vec!["s1"]);
        // A source's folder, or a folder above it: the whole source.
        assert_eq!(resolve(&["c:/docs/papers"]).unwrap().source_ids, vec!["s1"]);
        let both = resolve(&["c:/docs"]).unwrap();
        assert_eq!(both.source_ids, vec!["s1", "s2"]);
        assert!(both.files.is_empty());
        // A subfolder: its files, with their source.
        let sub = resolve(&["c:/docs/papers/ann"]).unwrap();
        assert!(sub.source_ids.is_empty());
        assert_eq!(
            file_paths(&sub),
            vec!["c:/docs/papers/ann/hnsw.pdf", "c:/docs/papers/ann/nsg.pdf"]
        );
        assert!(sub.files.iter().all(|f| f.source_id == "s1"));
        assert!(sub.notes[0].contains("2 indexed files"), "{:?}", sub.notes);
        // The same subfolder by its name, and a source by its folder's name.
        assert_eq!(file_paths(&resolve(&["ann"]).unwrap()).len(), 2);
        assert_eq!(resolve(&["Papers"]).unwrap().source_ids, vec!["s1"]);
        // A file by path, by name, or by name without its extension.
        for file in ["c:/docs/papers/ann/hnsw.pdf", "hnsw.pdf", "HNSW"] {
            let one = resolve(&[file]).unwrap();
            assert_eq!(
                file_paths(&one),
                vec!["c:/docs/papers/ann/hnsw.pdf"],
                "{file}"
            );
        }
        // Notes and the calendar by name.
        assert_eq!(resolve(&["notes"]).unwrap().source_ids, vec!["notes"]);
        assert_eq!(resolve(&["Calendar"]).unwrap().source_ids, vec!["calendar"]);
        // A file inside a source that is also asked for whole is not repeated.
        let merged = resolve(&["hnsw.pdf", "s1"]).unwrap();
        assert_eq!(merged.source_ids, vec!["s1"]);
        assert!(merged.files.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn folder_paths_are_normalised_like_the_indexer_stores_them() {
        for folder in [
            r"C:\Docs\Papers\ann",
            r"C:\Docs\Papers\ann\",
            "C:/Docs/Papers/ann/",
        ] {
            let sub = resolve(&[folder]).unwrap();
            assert_eq!(file_paths(&sub).len(), 2, "{folder}");
        }
        assert_eq!(
            resolve(&[r"\\?\C:\Docs\Papers"]).unwrap().source_ids,
            vec!["s1"]
        );
    }

    #[test]
    fn unmatched_and_ambiguous_sources_are_errors_that_list_the_valid_ones() {
        for wanted in ["c:/elsewhere", "thesis", "c:/docs/papers/ann/missing.pdf"] {
            let Err(ToolError::NotFound(message)) = resolve(&["s1", wanted]) else {
                panic!("{wanted} resolved");
            };
            assert!(message.contains(&format!("No indexed source matches \"{wanted}\"")));
            assert!(message.contains("s1 (\"papers\", folder c:/docs/papers, 3 files)"));
            assert!(message.contains("s2 (\"contracts\""));
            assert!(message.contains("notes (\"Notes\")"));
            assert!(message.contains("Nothing was searched"));
        }
        // Two files share the name: neither is guessed.
        let Err(ToolError::NotFound(message)) = resolve(&["acme.pdf"]) else {
            panic!("an ambiguous name resolved");
        };
        assert!(message.contains("file c:/docs/contracts/acme.pdf"));
        assert!(message.contains("file c:/docs/contracts/old/acme.pdf"));
        // Nothing indexed at all.
        let Err(ToolError::NotFound(message)) = resolve_sources(&[], &["s1".to_string()]) else {
            panic!("resolved in an empty index");
        };
        assert!(message.contains("Nothing is indexed yet"));
    }

    #[test]
    fn sources_are_listed_with_their_names() {
        let sources = summarise_sources(&library());
        let names: Vec<(&str, &str)> = sources
            .iter()
            .map(|s| (s.source_id.as_str(), s.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("calendar", "Calendar"),
                ("notes", "Notes"),
                ("s1", "papers"),
                ("s2", "contracts")
            ]
        );
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
