//! `search_documents`: hybrid search over the indexed documents.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::sources::{resolve_sources, SourceResolution, SOURCES_ACCEPTED};
use super::{
    is_unset_marker, req_str, CitedPassage, HostTool, RunScope, ScopedSnippet, ToolContext,
    ToolError, ToolOutput, MAX_MODEL_OUTPUT_CHARS, UNTRUSTED_NOTICE,
};
use crate::harness::events::RiskTier;
use crate::harness::protocol::ToolLoadMode;
use crate::harness::truncate_chars;
use crate::path_case::PathSpellings;
use crate::rag_engine::{normalize_source_path, page_numbers_from_metadata, DISPLAY_PATH_KEY};
use crate::types::{ComprehensiveResult, MetadataFilter, SourceSet};
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

/// Results fetched per file when the answer is limited to some pages:
/// the page filter is applied after retrieval (pages are chunk metadata, not
/// an index column), so more candidates are fetched than will be kept.
const PAGE_OVERFETCH: usize = 3;
/// Upper bound on that over-fetch per file, to bound reranking cost.
const MAX_PAGE_FETCH: usize = 48;

/// The pages a result covers, from the metadata written at indexing time
/// (`page_start`/`page_end`, else `page`). `None` for unpaged chunks.
fn result_pages(result: &ComprehensiveResult) -> Option<(u32, u32)> {
    let read = |key: &str| {
        result
            .metadata
            .get(key)
            .and_then(|v| v.trim().parse::<u32>().ok())
    };
    match (read("page_start"), read("page_end")) {
        (Some(start), Some(end)) => Some((start.min(end), start.max(end))),
        (Some(one), None) | (None, Some(one)) => Some((one, one)),
        (None, None) => read("page").map(|p| (p, p)),
    }
}

/// Whether `result` lies on one of `pages`. Unpaged chunks never do.
fn on_pages(result: &ComprehensiveResult, pages: &[u32]) -> bool {
    result_pages(result).is_some_and(|(start, end)| pages.iter().any(|p| (start..=end).contains(p)))
}

/// "pages 3, 4 and 9" / "page 3".
fn pages_text(pages: &[u32]) -> String {
    let list: Vec<String> = pages.iter().map(u32::to_string).collect();
    match list.as_slice() {
        [one] => format!("page {one}"),
        [rest @ .., last] => format!("pages {} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

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
            .unwrap_or(DEFAULT_K)
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
    /// Heading chain of the passage ("3 Method > 3.2 Chunkwise form").
    #[serde(skip_serializing_if = "Option::is_none")]
    section: Option<String>,
    score: f32,
    text: String,
    /// Bounding boxes of the passage on its pages, for the viewer only:
    /// sent in the step detail, never in the text the model reads.
    #[serde(skip)]
    regions: Option<Value>,
    /// The file's path as the file system spells it, for the UI only (`path`
    /// is the index's identity, lowercased on Windows): sent in the step
    /// detail, never in the text the model reads.
    #[serde(skip)]
    display_path: Option<String>,
}

impl Passage {
    /// The passage as shown to the UI: the model's fields plus `regions` and
    /// `display_path`.
    fn detail_value(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(map) = value.as_object_mut() {
            if let Some(regions) = &self.regions {
                map.insert("regions".to_string(), regions.clone());
            }
            if let Some(display_path) = &self.display_path {
                map.insert("display_path".to_string(), json!(display_path));
            }
        }
        value
    }
}

/// The chunk's stored boxes (`[{"page":3,"x0":..}]`), when valid JSON.
fn passage_regions(result: &ComprehensiveResult) -> Option<Value> {
    let raw = result.metadata.get("bboxes")?;
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_array().filter(|a| !a.is_empty())?;
    Some(value)
}

/// What the user sees for a source: the file name for files, in the case the
/// file has ([`original_case_name`]), and the record's title for in-app
/// records (`calendar://task/<id>` would otherwise show its id).
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
    match path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()) {
        Some(name) => original_case_name(name, result),
        None => result.citation.title.clone(),
    }
}

/// The file name `name` (the last part of a source path, an identity
/// lowercased on Windows: "spann - x.pdf") in the case the index also keeps
/// it: the chunk's `filename`, else its title (the file's stem when a folder
/// is indexed) with `name`'s extension, else `name`. A candidate is used only
/// when it names the same file in another case, never another name (a PDF's
/// own title). The frontend labels search results the same way
/// (`originalCaseName` in `app/src/features/ask/searchResults.ts`).
fn original_case_name(name: &str, result: &ComprehensiveResult) -> String {
    let extension = name
        .rfind('.')
        .filter(|&dot| dot > 0)
        .map_or("", |dot| &name[dot..]);
    let titled = |title: &str| {
        let title = title.trim();
        (!title.is_empty()).then(|| format!("{title}{extension}"))
    };
    let key = name.to_lowercase();
    result
        .metadata
        .get("filename")
        .map(|f| f.trim().to_string())
        .into_iter()
        .chain(
            result
                .metadata
                .get("title")
                .and_then(|t| titled(t.as_str())),
        )
        .chain(titled(result.citation.title.as_str()))
        .find(|candidate| !candidate.is_empty() && candidate.to_lowercase() == key)
        .unwrap_or_else(|| name.to_string())
}

/// The chunk's stored [`DISPLAY_PATH_KEY`] when it spells `path`, the file
/// the passage is from; `None` for in-app records.
fn stored_display_path(path: &str, result: &ComprehensiveResult) -> Option<String> {
    if path.is_empty() || path.contains("://") {
        return None;
    }
    let identity = normalize_source_path(Path::new(path));
    result
        .metadata
        .get(DISPLAY_PATH_KEY)
        .filter(|d| normalize_source_path(Path::new(d.as_str())) == identity)
        .cloned()
}

/// Fills the `display_path` of passages from files indexed before it was
/// stored with their chunks: each folder is resolved from disk once per
/// process and the file is named by the passage's original-case `file`
/// ([`PathSpellings`]). A file that cannot be resolved keeps none; the UI
/// then shows `path`.
async fn resolve_display_paths(passages: &mut [Passage]) {
    let missing: BTreeMap<String, String> = passages
        .iter()
        .filter(|p| p.display_path.is_none() && !p.path.is_empty() && !p.path.contains("://"))
        .map(|p| (p.path.clone(), p.file.clone()))
        .collect();
    if missing.is_empty() {
        return;
    }
    let resolved = tokio::task::spawn_blocking(move || {
        let spellings = PathSpellings::shared();
        missing
            .into_iter()
            .filter_map(|(path, file)| {
                let display = spellings.spelling(&path, &[file.as_str()])?;
                Some((path, display))
            })
            .collect::<HashMap<String, String>>()
    })
    .await;
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(e) => {
            tracing::warn!("resolving the passages' file paths was interrupted: {e}");
            return;
        }
    };
    for p in passages.iter_mut().filter(|p| p.display_path.is_none()) {
        p.display_path = resolved.get(&p.path).cloned();
    }
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
    let display_path = stored_display_path(&path, result);
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
        section: result
            .metadata
            .get("section_path")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        score: result.score,
        text: truncate_chars(result.snippet.trim(), max_chars),
        regions: passage_regions(result),
        display_path,
    }
}

/// The arguments of `search_documents`.
///
/// Bounds the host enforces itself (the number of passages, blank source
/// ids, the query's length) are normalised in `execute`, not refused here:
/// the runtime refuses a call that breaks the schema before the host sees
/// it, and models reasonably send `k: 25`, `sources: [""]` or a long query.
fn search_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "description": "What to look for, in words likely to appear in the passages."
            },
            "sources": {
                "type": "array",
                "items": {"type": "string", "maxLength": 1000},
                "maxItems": MAX_SOURCES,
                "description": format!(
                    "Search only these: {SOURCES_ACCEPTED}. Omit it to search everything. A value \
                     that matches nothing is an error listing the valid sources, never an empty \
                     result."
                )
            },
            "k": {
                "type": "integer",
                "description": "How many passages to return (1 to 20; larger values return 20). Omit for the default."
            }
        },
        "required": ["query"],
        "additionalProperties": false
    })
}

/// Longest query searched, in characters (a longer one is cut at a word).
const MAX_QUERY_CHARS: usize = 500;

/// `query` cut to [`MAX_QUERY_CHARS`] at a word boundary.
pub(crate) fn truncate_query(query: &str) -> String {
    let query = query.trim();
    if query.chars().count() <= MAX_QUERY_CHARS {
        return query.to_string();
    }
    let cut: String = query.chars().take(MAX_QUERY_CHARS).collect();
    match cut.rfind(char::is_whitespace) {
        Some(at) if at > 0 => cut[..at].trim_end().to_string(),
        _ => cut,
    }
}

/// The passage count the model asked for, within 1..=[`MAX_K`]; `None` when
/// it gave none (or something that is not a count).
fn requested_k(args: &Value) -> Option<usize> {
    let k = args.get("k")?;
    let n = k.as_i64().or_else(|| {
        k.as_f64()
            .filter(|f| f.is_finite())
            .map(|f| f.round() as i64)
    })?;
    Some(usize::try_from(n.clamp(1, MAX_K as i64)).unwrap_or(1))
}

/// Most snippet passages one search returns (they come before document passages).
const MAX_SNIPPET_PASSAGES: usize = 3;

/// What one search may cover, from the answer's scope and the sources the model asked
/// for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SearchPlan {
    /// Whether the search is limited to `space_ids` (and, with `use_scope_files`, the
    /// scope's files); otherwise it covers everything.
    limited: bool,
    space_ids: Vec<String>,
    use_scope_files: bool,
    /// The sources the model asked for, resolved to source ids.
    model_sources: Vec<String>,
    /// Files the model asked for (single files and parts of sources), by
    /// normalised path, within the scope.
    model_files: Vec<String>,
    /// Told to the model with the results.
    notes: Vec<String>,
}

/// The search a call may run. Without a restriction the model's own sources win over the
/// user's limit (as before workspaces); with one they are narrowed to the scope, and a
/// scope with nothing in it searches nothing.
fn plan_search(scope: &RunScope, model: &SourceResolution) -> SearchPlan {
    let mut sorted: Vec<String> = model.source_ids.clone();
    sorted.sort();
    let mut files: Vec<&super::sources::ResolvedFile> = model.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut plan = SearchPlan {
        model_sources: sorted.clone(),
        model_files: files.iter().map(|f| f.path.clone()).collect(),
        notes: model.notes.clone(),
        ..SearchPlan::default()
    };
    let asked = !sorted.is_empty() || !files.is_empty();
    if scope.restricted {
        plan.limited = true;
        if !asked {
            plan.space_ids = scope.source_ids.clone();
            plan.use_scope_files = true;
        } else {
            let (inside, outside): (Vec<String>, Vec<String>) =
                sorted.into_iter().partition(|s| scope.allows_source(s));
            if !outside.is_empty() {
                plan.notes
                    .push(scope.outside_message(&format!("Source {}", outside.join(", "))));
            }
            plan.space_ids = inside;
            let (inside, outside): (Vec<_>, Vec<_>) = files
                .into_iter()
                .partition(|f| scope.allows_source(&f.source_id) || scope.allows_path(&f.path));
            if !outside.is_empty() {
                let paths: Vec<&str> = outside.iter().map(|f| f.path.as_str()).collect();
                plan.notes
                    .push(scope.outside_message(&format!("File {}", paths.join(", "))));
            }
            plan.model_files = inside.into_iter().map(|f| f.path.clone()).collect();
        }
    } else if asked {
        plan.limited = true;
        plan.space_ids = sorted;
    } else if !scope.is_empty() {
        plan.limited = true;
        plan.space_ids = scope.source_ids.clone();
        plan.use_scope_files = true;
    }
    plan
}

/// What the model is told about the limit the results were searched under.
fn limit_note(
    scope: &RunScope,
    plan: &SearchPlan,
    set: &SourceSet,
    scoped_files: &[String],
    pages: &[u32],
) -> Option<String> {
    let mut notes: Vec<String> = plan.notes.clone();
    if !scoped_files.is_empty() && !pages.is_empty() {
        notes.push(format!(
            "The user limited this answer to {} of {}.",
            pages_text(pages),
            scoped_files.join(", ")
        ));
    } else if scope.restricted && plan.model_sources.is_empty() && plan.model_files.is_empty() {
        if set.is_empty() && scope.snippets.is_empty() {
            notes.push(match &scope.workspace_name {
                Some(name) => format!(
                    "The workspace \"{name}\" has no searchable sources yet (none added, or none \
                     indexed). Tell the user to add sources to the workspace, or to ask again \
                     with \"search all my library\" turned on."
                ),
                None => "The sources this answer is limited to are not indexed.".to_string(),
            });
        } else if let Some(name) = &scope.workspace_name {
            notes.push(format!(
                "This chat belongs to the workspace \"{name}\": only its sources were searched."
            ));
        } else if !scoped_files.is_empty() {
            notes.push(format!(
                "The user limited this answer to {}.",
                scoped_files.join(", ")
            ));
        }
    } else if !scoped_files.is_empty() {
        notes.push(format!(
            "The user limited this answer to {}.",
            scoped_files.join(", ")
        ));
    } else if plan.use_scope_files && !scope.source_ids.is_empty() {
        notes.push(format!(
            "The user limited this answer to sources {}.",
            scope.source_ids.join(", ")
        ));
    } else if plan.use_scope_files && !scope.files.is_empty() {
        notes.push(format!(
            "The files this answer is limited to are not indexed: {}.",
            scope.files.join(", ")
        ));
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

fn query_terms(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 2)
        .map(str::to_lowercase)
        .collect()
}

/// The snippets that share words with `query`, best first, as results (a snippet is the
/// text of a page region the user saved, so it is cited like a passage of its file).
fn snippet_results(
    query: &str,
    snippets: &[ScopedSnippet],
    max: usize,
) -> Vec<ComprehensiveResult> {
    let terms = query_terms(query);
    if terms.is_empty() || max == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(f32, &ScopedSnippet)> = snippets
        .iter()
        .filter_map(|s| {
            let words = query_terms(&format!("{} {}", s.title, s.text));
            let shared = terms.iter().filter(|t| words.contains(*t)).count();
            (shared > 0).then(|| (shared as f32 / terms.len() as f32, s))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    scored
        .into_iter()
        .take(max)
        .map(|(score, s)| {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("page_start".to_string(), s.page.to_string());
            metadata.insert("page_end".to_string(), s.page.to_string());
            metadata.insert("source_file".to_string(), s.file_path.clone());
            metadata.insert("snippet_id".to_string(), s.id.clone());
            if !s.title.trim().is_empty() {
                metadata.insert(
                    "heading".to_string(),
                    format!("Snippet: {}", s.title.trim()),
                );
            }
            ComprehensiveResult {
                id: uuid::Uuid::nil(),
                score,
                metadata,
                citation: crate::types::Citation {
                    title: s.file_name.clone(),
                    source: s.file_path.clone(),
                    ..crate::types::Citation::default()
                },
                snippet: s.text.clone(),
                source_index: "snippet".to_string(),
            }
        })
        .collect()
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
         file, path, page and score. Optionally restrict it with sources: source ids from \
         list_sources, folder or file paths inside an indexed folder, a folder or file name, \
         \"notes\" or \"calendar\". Use specific queries; search again with different wording \
         if the first results miss."
    }
    fn schema(&self) -> Value {
        search_schema()
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        ToolLoadMode::Essential
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let query = req_str(&args, "query", SEARCH_DOCUMENTS)?;
        let query_owned = truncate_query(query);
        let query = query_owned.as_str();
        let k = requested_k(&args).unwrap_or_else(|| self.default_k());
        let mut sources: Vec<String> = args
            .get("sources")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !is_unset_marker(s))
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default();
        sources.sort();

        let scope = ctx.scope();
        let rag = self.rag.read().await;
        let resolution = if sources.is_empty() {
            SourceResolution::default()
        } else {
            let rows = rag
                .document_sources()
                .await
                .map_err(|e| ToolError::Failed(format!("Could not read the index: {e}")))?;
            resolve_sources(&rows, &sources)?
        };
        let plan = plan_search(scope, &resolution);
        // The user's files, as the index stores them.
        let mut scoped_files = Vec::new();
        if plan.use_scope_files {
            for file in &scope.files {
                let matches = rag
                    .find_indexed_sources(file)
                    .await
                    .map_err(|e| ToolError::Failed(format!("Could not read the index: {e}")))?;
                scoped_files.extend(matches);
            }
            scoped_files.sort();
            scoped_files.dedup();
        }
        // Pages apply only to the user's file limit, not to sources the
        // model chose.
        let pages: &[u32] = if scoped_files.is_empty() {
            &[]
        } else {
            &scope.pages
        };
        let mut source_paths = scoped_files.clone();
        source_paths.extend(plan.model_files.iter().cloned());
        source_paths.sort();
        source_paths.dedup();
        let set = SourceSet {
            space_ids: plan.space_ids.clone(),
            source_paths,
        };
        let mut results = if !plan.limited {
            rag.search_comprehensive(query, k, None)
                .await
                .map_err(|e| ToolError::Failed(format!("Search failed: {e}")))?
        } else if set.is_empty() {
            // A limit that resolves to nothing finds nothing, never everything.
            Vec::new()
        } else {
            let fetch = if pages.is_empty() {
                k
            } else {
                (k * PAGE_OVERFETCH).min(MAX_PAGE_FETCH).max(k)
            };
            let filter = MetadataFilter {
                any_of: Some(set.clone()),
                ..MetadataFilter::default()
            };
            let mut hits = rag
                .search_comprehensive(query, fetch, Some(filter))
                .await
                .map_err(|e| ToolError::Failed(format!("Search failed: {e}")))?;
            if !pages.is_empty() {
                // The page limit applies to the user's files; passages of the
                // scope's sources are kept whatever their page.
                hits.retain(|h| {
                    let file = h.metadata.get("source_file").map_or("", String::as_str);
                    !scoped_files.iter().any(|f| f == file) || on_pages(h, pages)
                });
            }
            hits
        };
        drop(rag);
        // Defence in depth: nothing outside the limit reaches the model.
        if plan.limited {
            results.retain(|r| {
                set.contains(
                    r.metadata.get("space_id").map_or("", String::as_str),
                    r.metadata.get("source_file").map_or("", String::as_str),
                )
            });
        }
        let snippet_hits = if plan.limited {
            snippet_results(query, &scope.snippets, k.min(MAX_SNIPPET_PASSAGES))
        } else {
            Vec::new()
        };
        results.truncate(k.saturating_sub(snippet_hits.len()));
        let mut results: Vec<ComprehensiveResult> =
            snippet_hits.into_iter().chain(results).collect();
        results.truncate(k);

        let limited_to = limit_note(scope, &plan, &set, &scoped_files, pages);
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
        let mut passages: Vec<Passage> = results
            .iter()
            .zip(first..)
            .map(|(r, n)| passage(n, r, budget))
            .collect();
        resolve_display_paths(&mut passages).await;
        for p in &passages {
            ctx.record_passage(CitedPassage {
                n: p.n,
                file: p.file.clone(),
                path: p.path.clone(),
                page: p.page.clone(),
                web: false,
                text: p.text.clone(),
                checkable: true,
            });
        }
        let files: HashSet<&str> = passages.iter().map(|p| p.path.as_str()).collect();
        let body = serde_json::to_string(&passages)
            .map_err(|e| ToolError::Failed(format!("Could not encode results: {e}")))?;
        let detail = json!({
            "passages": passages.iter().map(Passage::detail_value).collect::<Vec<_>>()
        });
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
    use crate::harness::tools::{nullable_optionals, without_unaccepted_nulls};
    use crate::types::Citation;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// What the runtime validates (the schema it is sent) and what the host
    /// then validates (after dropping `null`s the tool does not accept).
    fn accepted(args: Value) -> bool {
        let runtime = jsonschema::validator_for(&nullable_optionals(search_schema())).unwrap();
        let host = jsonschema::validator_for(&search_schema()).unwrap();
        runtime.is_valid(&args) && host.is_valid(&without_unaccepted_nulls(args, &search_schema()))
    }

    #[test]
    fn arguments_capable_models_send_are_accepted() {
        // Unused optional parameters sent as null, a count above the
        // maximum the schema used to state, a blank source id and a query
        // longer than the old limit: each was refused before the call ran.
        for args in [
            json!({"query": "neighbor pruning", "sources": null, "k": null}),
            json!({"query": "neighbor pruning", "k": 25}),
            json!({"query": "neighbor pruning", "sources": [""]}),
            json!({"query": "pruning ".repeat(120)}),
            json!({"query": "neighbor pruning", "sources": ["src-1"], "k": 5}),
        ] {
            assert!(accepted(args.clone()), "{args}");
        }
    }

    #[test]
    fn garbage_arguments_are_still_refused() {
        for args in [
            json!({}),
            json!({"query": ""}),
            json!({"query": null}),
            json!({"query": "x", "k": "many"}),
            json!({"query": "x", "sources": [7]}),
        ] {
            assert!(!accepted(args.clone()), "{args}");
        }
    }

    #[test]
    fn counts_and_queries_are_normalised() {
        assert_eq!(requested_k(&json!({"k": 25})), Some(MAX_K));
        assert_eq!(requested_k(&json!({"k": 0})), Some(1));
        assert_eq!(requested_k(&json!({"k": -3})), Some(1));
        assert_eq!(requested_k(&json!({"k": 7.0})), Some(7));
        assert_eq!(requested_k(&json!({})), None);
        assert_eq!(requested_k(&json!({"k": "seven"})), None);
        let long = format!("{} tail", "word ".repeat(120));
        let cut = truncate_query(&long);
        assert!(cut.chars().count() <= MAX_QUERY_CHARS);
        assert!(cut.ends_with("word"), "cut at a word: {cut:?}");
        assert_eq!(truncate_query("  short query "), "short query");
    }

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
        assert!(p.section.is_none() && p.regions.is_none());
    }

    #[test]
    fn structured_passages_carry_section_and_regions_for_the_viewer_only() {
        let mut metadata = HashMap::new();
        metadata.insert("page_start".to_string(), "5".to_string());
        metadata.insert("page_end".to_string(), "6".to_string());
        metadata.insert(
            "section_path".to_string(),
            "3 Method > 3.2 Chunkwise form".to_string(),
        );
        metadata.insert("heading".to_string(), "3.2 Chunkwise form".to_string());
        metadata.insert(
            "bboxes".to_string(),
            r#"[{"page":5,"x0":72.0,"y0":400.0,"x1":300.0,"y1":700.0}]"#.to_string(),
        );
        let result = ComprehensiveResult {
            id: Uuid::nil(),
            score: 0.7,
            metadata,
            citation: Citation {
                source: "c:/papers/deltanet.pdf".into(),
                ..Citation::default()
            },
            snippet: "The chunkwise form computes...".into(),
            source_index: "hybrid".into(),
        };
        let p = passage(1, &result, MAX_PASSAGE_CHARS);
        assert_eq!(p.page.as_deref(), Some("5-6"));
        assert_eq!(p.section.as_deref(), Some("3 Method > 3.2 Chunkwise form"));
        let for_model = serde_json::to_value(&p).expect("json");
        assert!(for_model.get("regions").is_none());
        assert_eq!(for_model["section"], "3 Method > 3.2 Chunkwise form");
        let for_ui = p.detail_value();
        assert_eq!(for_ui["regions"][0]["page"], 5);
        assert_eq!(for_ui["regions"][0]["x0"], 72.0);
    }

    fn file_result(source: &str, metadata: &[(&str, &str)], title: &str) -> ComprehensiveResult {
        ComprehensiveResult {
            id: Uuid::nil(),
            score: 0.5,
            metadata: metadata
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            citation: Citation {
                title: title.into(),
                source: source.into(),
                ..Citation::default()
            },
            snippet: "Graph-based ANN search on disk.".into(),
            source_index: "hybrid".into(),
        }
    }

    /// Sources are identities, lowercased on Windows; a passage names its
    /// file in the case the index also keeps (`filename`, else the title
    /// with the file's extension), and never by a name of another file.
    #[test]
    fn passages_name_files_in_their_original_case() {
        let source = "c:/research/papers/spann - billion-scale ann search.pdf";
        let by_filename = file_result(
            source,
            &[
                ("filename", "SPANN - Billion-scale ANN Search.pdf"),
                ("title", "spann"),
            ],
            "",
        );
        assert_eq!(
            passage(1, &by_filename, MAX_PASSAGE_CHARS).file,
            "SPANN - Billion-scale ANN Search.pdf"
        );

        let by_title = file_result(
            source,
            &[("title", " SPANN - Billion-scale ANN Search ")],
            "",
        );
        assert_eq!(
            passage(1, &by_title, MAX_PASSAGE_CHARS).file,
            "SPANN - Billion-scale ANN Search.pdf"
        );

        let by_citation = file_result(source, &[], "SPANN - Billion-scale ANN Search");
        assert_eq!(
            passage(1, &by_citation, MAX_PASSAGE_CHARS).file,
            "SPANN - Billion-scale ANN Search.pdf"
        );

        // A PDF's own title names something else: the path's name is kept.
        let other = file_result(
            source,
            &[
                ("filename", "draft3.pdf"),
                ("title", "Microsoft Word - draft3"),
            ],
            "Microsoft Word - draft3",
        );
        assert_eq!(
            passage(1, &other, MAX_PASSAGE_CHARS).file,
            "spann - billion-scale ann search.pdf"
        );
    }

    /// A stored display path that names another file is never shown, and a
    /// display path never reaches the text the model reads.
    #[test]
    fn display_paths_of_other_files_are_dropped_and_never_sent_to_the_model() {
        let result = file_result(
            "c:/research/papers/spann.pdf",
            &[("display_path", "D:\\Elsewhere\\Other.pdf")],
            "",
        );
        let p = passage(1, &result, MAX_PASSAGE_CHARS);
        assert!(p.display_path.is_none());
        assert!(p.detail_value().get("display_path").is_none());
        assert!(serde_json::to_value(&p)
            .unwrap()
            .get("display_path")
            .is_none());
    }

    /// On Windows a passage's path is the lowercase identity; the UI is sent
    /// the stored spelling with the case on disk, the model is not.
    #[cfg(windows)]
    #[test]
    fn passages_carry_the_stored_spelling_for_the_ui_only() {
        let result = file_result(
            "c:/research/papers/spann.pdf",
            &[("display_path", "C:\\Research\\papers\\SPANN.pdf")],
            "",
        );
        let p = passage(1, &result, MAX_PASSAGE_CHARS);
        assert_eq!(p.path, "c:/research/papers/spann.pdf");
        assert_eq!(
            p.detail_value()["display_path"],
            "C:\\Research\\papers\\SPANN.pdf"
        );
        assert!(serde_json::to_value(&p)
            .unwrap()
            .get("display_path")
            .is_none());
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

    fn paged(start: Option<&str>, end: Option<&str>, page: Option<&str>) -> ComprehensiveResult {
        let mut metadata = HashMap::new();
        for (key, value) in [("page_start", start), ("page_end", end), ("page", page)] {
            if let Some(v) = value {
                metadata.insert(key.to_string(), v.to_string());
            }
        }
        ComprehensiveResult {
            id: Uuid::nil(),
            score: 0.5,
            metadata,
            citation: Citation::default(),
            snippet: String::new(),
            source_index: "hybrid".into(),
        }
    }

    #[test]
    fn page_scope_keeps_only_results_on_those_pages() {
        let page4 = paged(Some("4"), Some("4"), None);
        let span = paged(Some("6"), Some("8"), None);
        let legacy = paged(None, None, Some("9"));
        let unpaged = paged(None, None, None);
        assert!(on_pages(&page4, &[4]));
        assert!(!on_pages(&page4, &[3, 5]));
        assert!(
            on_pages(&span, &[7]),
            "a merged chunk covers its whole range"
        );
        assert!(!on_pages(&span, &[9]));
        assert!(on_pages(&legacy, &[9]));
        assert!(!on_pages(&unpaged, &[1]), "unpaged chunks are on no page");
        assert_eq!(
            result_pages(&paged(Some("8"), Some("6"), None)),
            Some((6, 8))
        );
        assert_eq!(pages_text(&[3]), "page 3");
        assert_eq!(pages_text(&[3, 4, 9]), "pages 3, 4 and 9");
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

    fn ids(source_ids: &[&str]) -> SourceResolution {
        SourceResolution {
            source_ids: source_ids.iter().map(|s| s.to_string()).collect(),
            ..SourceResolution::default()
        }
    }

    fn workspace_scope(source_ids: &[&str], files: &[&str]) -> RunScope {
        RunScope {
            source_ids: source_ids.iter().map(|s| s.to_string()).collect(),
            files: files.iter().map(|s| s.to_string()).collect(),
            workspace: Some("ws-1".into()),
            workspace_name: Some("Thesis".into()),
            restricted: true,
            folders: vec!["C:/docs/thesis".into()],
            ..RunScope::default()
        }
    }

    #[test]
    fn a_restricted_scope_narrows_the_models_sources_and_never_widens() {
        let scope = workspace_scope(&["space-a"], &["C:/x/one.pdf"]);
        // No sources from the model: the workspace's sources and files.
        let plan = plan_search(&scope, &ids(&[]));
        assert!(plan.limited && plan.use_scope_files);
        assert_eq!(plan.space_ids, vec!["space-a"]);
        // The model asks for another source: it is refused, and nothing widens.
        let plan = plan_search(&scope, &ids(&["space-b"]));
        assert!(plan.limited);
        assert!(plan.space_ids.is_empty());
        assert!(plan.notes[0].contains("outside the workspace \"Thesis\""));
        let plan = plan_search(&scope, &ids(&["space-a", "space-b"]));
        assert_eq!(plan.space_ids, vec!["space-a"]);
        // An empty workspace is still limited: it finds nothing, not everything.
        let empty = workspace_scope(&[], &[]);
        assert!(!empty.is_empty());
        let plan = plan_search(&empty, &ids(&[]));
        assert!(plan.limited && plan.space_ids.is_empty());
        // Without a restriction (no workspace, or "search all my library").
        assert!(!plan_search(&RunScope::default(), &ids(&[])).limited);
        let plan = plan_search(&RunScope::default(), &ids(&["space-b"]));
        assert_eq!(plan.space_ids, vec!["space-b"]);
        // Sources and files of an unrestricted limit are searched together.
        let library = RunScope {
            source_ids: vec!["space-a".into()],
            files: vec!["C:/x/one.pdf".into()],
            ..RunScope::default()
        };
        let plan = plan_search(&library, &ids(&[]));
        assert!(plan.limited && plan.use_scope_files);
        assert_eq!(plan.space_ids, vec!["space-a"]);
    }

    #[test]
    fn paths_outside_a_restricted_scope_are_not_allowed() {
        let mut scope = workspace_scope(&["space-a"], &["C:/x/one.pdf"]);
        scope.snippets.push(ScopedSnippet {
            id: "snippet:1".into(),
            file_path: "C:/y/two.pdf".into(),
            file_name: "two.pdf".into(),
            page: 3,
            title: String::new(),
            text: "text".into(),
        });
        assert!(scope.allows_path(r"C:\docs\thesis\ch1.pdf"));
        assert!(scope.allows_path("c:/x/one.pdf") || !cfg!(windows));
        assert!(scope.allows_path("C:/x/one.pdf"));
        assert!(scope.allows_path("C:/y/two.pdf"));
        assert!(!scope.allows_path("C:/docs/other/secret.pdf"));
        assert!(!scope.allows_path("C:/docs/thesis-old/a.pdf"));
        assert!(RunScope::default().allows_path("C:/anything.pdf"));
        assert!(scope.allows_source("space-a") && !scope.allows_source("space-b"));
    }

    #[test]
    fn snippets_match_by_shared_words() {
        let snippet = |id: &str, title: &str, text: &str| ScopedSnippet {
            id: id.into(),
            file_path: format!("C:/p/{id}.pdf"),
            file_name: format!("{id}.pdf"),
            page: 2,
            title: title.into(),
            text: text.into(),
        };
        let snippets = vec![
            snippet("a", "Recall table", "HNSW reaches 95.3 recall at 10"),
            snippet("b", "", "Unrelated passage about weather"),
            snippet("c", "", "Recall of IVF-PQ is lower than HNSW recall"),
        ];
        let hits = snippet_results("HNSW recall numbers", &snippets, 3);
        let ids: Vec<&str> = hits
            .iter()
            .map(|h| h.metadata["snippet_id"].as_str())
            .collect();
        assert_eq!(ids, vec!["a", "c"]);
        assert_eq!(hits[0].citation.source, "C:/p/a.pdf");
        assert_eq!(
            page_numbers_from_metadata(&hits[0].metadata).as_deref(),
            Some("2")
        );
        assert!(snippet_results("of", &snippets, 3).is_empty());
        assert_eq!(snippet_results("recall", &snippets, 1).len(), 1);
    }

    fn stored(path: &str) -> String {
        crate::rag_engine::normalize_source_path(std::path::Path::new(path))
    }

    async fn engine_with_two_spaces(dir: &std::path::Path) -> Arc<RwLock<RAGEngine>> {
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join("data");
        config.embedding.model_dir = dir.join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        let mut engine = RAGEngine::new(config).await.unwrap();
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .unwrap();
        for (space, path, text) in [
            (
                "space-a",
                "C:/docs/thesis/inside.txt",
                "The notice period for the office lease is sixty days. Rent is reviewed every spring, and the landlord repairs the roof and the heating.",
            ),
            (
                "space-b",
                "C:/docs/other/outside.txt",
                "The notice period for the bank loan is ninety days. Interest is fixed for five years; early repayment costs one percent of the balance.",
            ),
            (
                "space-b",
                "C:/docs/other/file.txt",
                "The notice period for the car rental is thirty days. Mileage above twenty thousand kilometres a year is charged per kilometre driven.",
            ),
        ] {
            // As indexing stores a file's path.
            let path = stored(path);
            let path = path.as_str();
            let mut metadata = HashMap::new();
            metadata.insert("space_id".to_string(), space.to_string());
            metadata.insert("file_path".to_string(), path.to_string());
            metadata.insert("title".to_string(), path.to_string());
            let ids = engine
                .add_document(
                    text,
                    crate::types::DocumentFormat::TXT,
                    metadata,
                    Citation {
                        title: path.into(),
                        source: path.into(),
                        ..Citation::default()
                    },
                )
                .await
                .unwrap();
            assert!(!ids.is_empty(), "{path} was not indexed");
        }
        Arc::new(RwLock::new(engine))
    }

    async fn search_paths(
        tool: &SearchDocumentsTool,
        scope: RunScope,
        args: Value,
    ) -> (Vec<String>, String) {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("run-1", "step-1", tx).with_scope(Arc::new(scope));
        // Boxed: the search future is large for a test thread's stack.
        let out = Box::pin(tool.execute(args, &ctx)).await.unwrap();
        let mut paths: Vec<String> = out.detail.unwrap()["passages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["path"].as_str().unwrap().to_string())
            .collect();
        paths.sort();
        paths.dedup();
        (paths, out.text_for_model)
    }

    #[tokio::test]
    async fn a_workspace_chat_never_retrieves_outside_its_sources() {
        let dir = tempfile::tempdir().unwrap();
        let rag = Box::pin(engine_with_two_spaces(dir.path())).await;
        let tool = SearchDocumentsTool::new(rag);
        let query = json!({"query": "notice period days"});

        // Unlimited: every space.
        let (all, _) = search_paths(&tool, RunScope::default(), query.clone()).await;
        assert_eq!(all.len(), 3, "{all:?}");

        // A workspace with folder space-a: only its file, also when the model asks
        // for the other source.
        let scope = workspace_scope(&["space-a"], &[]);
        let (inside, text) = search_paths(&tool, scope.clone(), query.clone()).await;
        assert_eq!(inside, vec![stored("C:/docs/thesis/inside.txt")]);
        assert!(text.contains("workspace \"Thesis\""));
        let (forced, text) = search_paths(
            &tool,
            scope,
            json!({"query": "notice period days", "sources": ["space-b"]}),
        )
        .await;
        assert!(forced.is_empty(), "{forced:?}");
        assert!(text.contains("outside the workspace"));

        // Folder and single file together: the union, nothing else.
        let both = workspace_scope(&["space-a"], &["C:/docs/other/file.txt"]);
        let (union, _) = search_paths(&tool, both, query.clone()).await;
        assert_eq!(
            union,
            vec![
                stored("C:/docs/other/file.txt"),
                stored("C:/docs/thesis/inside.txt")
            ]
        );

        // A workspace whose sources are not indexed finds nothing (never everything).
        let unindexed = workspace_scope(&[], &["C:/docs/missing.pdf"]);
        let (none, text) = search_paths(&tool, unindexed, query.clone()).await;
        assert!(none.is_empty());
        assert!(text.contains("no searchable sources"));

        // An unrestricted file limit that does not resolve does not search everything.
        let stale = RunScope {
            files: vec!["C:/docs/missing.pdf".into()],
            ..RunScope::default()
        };
        let (none, text) = search_paths(&tool, stale, query).await;
        assert!(none.is_empty());
        assert!(text.contains("not indexed"));
    }

    #[tokio::test]
    async fn sources_given_as_folders_files_and_names_search_what_they_name() {
        let dir = tempfile::tempdir().unwrap();
        let rag = Box::pin(engine_with_two_spaces(dir.path())).await;
        let tool = SearchDocumentsTool::new(rag);
        let search = |sources: Value| json!({"query": "notice period days", "sources": sources});
        let other = vec![
            stored("C:/docs/other/file.txt"),
            stored("C:/docs/other/outside.txt"),
        ];

        // A folder path as the model writes it (another case, backslashes,
        // a trailing separator), and the folder's name: its source.
        for folder in [
            json!([r"C:\Docs\Other\"]),
            json!(["c:/docs/other"]),
            json!(["other"]),
        ] {
            let (paths, text) =
                search_paths(&tool, RunScope::default(), search(folder.clone())).await;
            assert_eq!(paths, other, "{folder}");
            assert!(text.contains("is the folder"), "{text}");
        }
        // A file path and a file name: that file only.
        for file in [json!(["C:/docs/other/file.txt"]), json!(["file.txt"])] {
            let (paths, _) = search_paths(&tool, RunScope::default(), search(file.clone())).await;
            assert_eq!(paths, vec![stored("C:/docs/other/file.txt")], "{file}");
        }
        // "None" is not a source: the search covers everything.
        let (all, _) = search_paths(&tool, RunScope::default(), search(json!(["None"]))).await;
        assert_eq!(all.len(), 3, "{all:?}");

        // A source that matches nothing is an error naming the valid ones,
        // not an empty result.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("run-1", "step-1", tx);
        let error = Box::pin(tool.execute(search(json!(["c:/docs/thesis/drafts"])), &ctx))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("No indexed source matches"), "{error}");
        assert!(error.contains("space-a (\"thesis\""), "{error}");
        assert!(error.contains("space-b (\"other\""), "{error}");

        // In a workspace chat a file of another source stays outside.
        let (paths, text) = search_paths(
            &tool,
            workspace_scope(&["space-a"], &[]),
            search(json!(["file.txt"])),
        )
        .await;
        assert!(paths.is_empty(), "{paths:?}");
        assert!(text.contains("outside the workspace"), "{text}");
    }
}
