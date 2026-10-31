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
use crate::embeddings::{SearchModelsMissing, SEARCH_MODELS_MISSING_CODE};
use crate::harness::events::RiskTier;
use crate::harness::protocol::ToolLoadMode;
use crate::harness::truncate_chars;
use crate::harness::web::relevance::{is_stopword, words};
use crate::path_case::PathSpellings;
use crate::rag_engine::{
    normalize_source_path, page_numbers_from_metadata, page_range_from_metadata, NeighborPart,
    SnippetParts, DISPLAY_PATH_KEY,
};
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
    page_range_from_metadata(&result.metadata)
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
    /// The file's path (the index's identity): sent in the step detail, never in
    /// the text the model reads, which names the file by `file` (a path holds the
    /// user's name and folders, which an answer must not quote).
    #[serde(skip)]
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
    /// The document's title from its metadata, when it has one: sent in the
    /// step detail (a Slack citation names the document by it), not to the
    /// model, which has `file`.
    #[serde(skip)]
    title: Option<String>,
    /// The Slack permalink of an indexed Slack thread, which its citation opens: sent in
    /// the step detail, never in the text the model reads.
    #[serde(skip)]
    url: Option<String>,
}

/// What the model is told when search may not run yet: the search models are not
/// installed (the first run, before or while setup downloads them). It is told
/// what to say, so the answer is a clear message, never an empty reply or an
/// answer from memory that reads as if it came from the user's files.
pub const SEARCH_NOT_READY: &str = "Search is not ready yet: Shodh is still setting up the search \
     models on this computer (a one-time download). Tell the user that their files can be \
     searched once setup finishes, which Ask shows with its progress, and do not answer from \
     memory as if you had searched their files.";

/// A failed search as the model reads it.
fn search_failure(error: anyhow::Error) -> ToolError {
    let models_missing = error
        .chain()
        .any(|cause| cause.downcast_ref::<SearchModelsMissing>().is_some())
        || format!("{error:#}").contains(SEARCH_MODELS_MISSING_CODE);
    if models_missing {
        ToolError::Unavailable(SEARCH_NOT_READY.to_string())
    } else {
        ToolError::Failed(format!("Search failed: {error}"))
    }
}

impl Passage {
    /// The passage as shown to the UI: the model's fields plus `path`, `regions`
    /// and `display_path`.
    fn detail_value(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(map) = value.as_object_mut() {
            map.insert("path".to_string(), json!(self.path));
            if let Some(regions) = &self.regions {
                map.insert("regions".to_string(), regions.clone());
            }
            if let Some(display_path) = &self.display_path {
                map.insert("display_path".to_string(), json!(display_path));
            }
            if let Some(title) = &self.title {
                map.insert("title".to_string(), json!(title));
            }
            if let Some(url) = &self.url {
                map.insert("url".to_string(), json!(url));
            }
        }
        value
    }
}

/// The document title of `result` from its metadata (`title`, else the
/// citation's), `None` when it has none.
fn document_title(result: &ComprehensiveResult) -> Option<String> {
    result
        .metadata
        .get("title")
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .or_else(|| Some(result.citation.title.trim()).filter(|t| !t.is_empty()))
        .map(str::to_string)
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

/// Whether a neighbouring chunk on `pages` may join a passage limited to
/// `allowed` pages: always without a limit; else only when every page it
/// covers is allowed (an unpaged chunk never is).
fn neighbor_allowed(pages: Option<(u32, u32)>, allowed: &[u32]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    pages.is_some_and(|(start, end)| {
        end.checked_sub(start)
            .and_then(|span| usize::try_from(span).ok())
            .is_some_and(|span| span < allowed.len())
            && (start..=end).all(|p| allowed.contains(&p))
    })
}

/// The last characters of `text` within `max_chars`, a `…` in front marking
/// the cut; all of it when it fits. `None` when nothing fits.
fn tail_within(text: &str, max_chars: usize) -> Option<String> {
    let count = text.chars().count();
    if count <= max_chars {
        return (!text.is_empty()).then(|| text.to_string());
    }
    let keep = max_chars.checked_sub(1).filter(|&k| k > 0)?;
    let at = text
        .char_indices()
        .nth(count - keep)
        .map_or(text.len(), |(i, _)| i);
    let tail = text[at..].trim_start();
    (!tail.is_empty()).then(|| format!("\u{2026}{tail}"))
}

/// The first characters of `text` within `max_chars`, a `…` after them
/// marking the cut; all of it when it fits. `None` when nothing fits.
fn head_within(text: &str, max_chars: usize) -> Option<String> {
    if text.chars().count() <= max_chars {
        return (!text.is_empty()).then(|| text.to_string());
    }
    if max_chars < 2 {
        return None;
    }
    let head: String = text.chars().take(max_chars - 1).collect();
    let head = head.trim_end();
    (!head.is_empty()).then(|| format!("{head}\u{2026}"))
}

/// The words of `query` a passage window looks for: its lower-cased
/// alphanumeric words, stopwords and single characters left out.
fn window_terms(query: &str) -> HashSet<String> {
    words(query)
        .into_iter()
        .filter(|w| w.chars().count() > 1 && !is_stopword(w))
        .collect()
}

/// Whether a window may start at char `at` of `chars` (`Some(true)` at a
/// sentence start, `Some(false)` at a word start, `None` inside a word).
fn start_boundary(chars: &[char], at: usize) -> Option<bool> {
    if at == 0 {
        return Some(true);
    }
    let prev = chars[at - 1];
    if !prev.is_whitespace() {
        return None;
    }
    let before_space = chars[..at]
        .iter()
        .rev()
        .find(|c| !c.is_whitespace())
        .copied();
    Some(
        prev == '\n'
            || before_space.is_none_or(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？')),
    )
}

/// Whether a window may end at char `at` of `chars` (`Some(true)` after a
/// sentence end, `Some(false)` at a word end, `None` inside a word).
fn end_boundary(chars: &[char], at: usize) -> Option<bool> {
    if at == chars.len() {
        return Some(true);
    }
    let prev = chars[at - 1];
    if matches!(prev, '.' | '!' | '?' | '。' | '！' | '？') && chars[at].is_whitespace() {
        return Some(true);
    }
    (chars[at].is_whitespace() || prev.is_whitespace()).then_some(false)
}

/// `text`, longer than `max_chars` characters, cut to the window of that
/// size holding the most occurrences of `terms` (whole words, any case; the
/// earliest such window on a tie), widened to sentence or word boundaries
/// inside it when there are any, a `…` marking each cut. The start of `text`
/// when none of `terms` occurs in it.
fn query_window(text: &str, max_chars: usize, terms: &HashSet<String>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    // Room for a `…` at both ends and some text between them.
    let room = max_chars.saturating_sub(2);
    if total <= max_chars || terms.is_empty() || room == 0 {
        return truncate_chars(text, max_chars);
    }
    // Where the terms occur, as char ranges.
    let mut hits: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < total {
        if !chars[at].is_alphanumeric() {
            at += 1;
            continue;
        }
        let start = at;
        while at < total && chars[at].is_alphanumeric() {
            at += 1;
        }
        let word: String = chars[start..at].iter().collect::<String>().to_lowercase();
        if terms.contains(&word) {
            hits.push((start, at));
        }
    }
    // The run of hits that fits in `room` with the most of them; the earliest wins a tie.
    // Hits are in order and do not overlap, so the run's last hit only moves forward.
    let mut best: Option<(usize, usize, usize)> = None;
    let mut last = 0;
    for (first, &(start, end)) in hits.iter().enumerate() {
        if end - start > room {
            continue;
        }
        last = last.max(first);
        while last + 1 < hits.len() && hits[last + 1].1 - start <= room {
            last += 1;
        }
        let count = last - first + 1;
        if best.is_none_or(|(_, _, c)| count > c) {
            best = Some((first, last, count));
        }
    }
    let Some((first, last, _)) = best else {
        return truncate_chars(text, max_chars);
    };
    let (hits_start, hits_end) = (hits[first].0, hits[last].1);
    // Centre the hits in the window, inside the text.
    let slack = room - (hits_end - hits_start);
    let mut start = hits_start.saturating_sub(slack / 2).min(total - room);
    let mut end = start + room;
    // Snap inward to a sentence boundary, else a word boundary, never past the hits.
    let snap_start = (start..=hits_start)
        .find(|&i| start_boundary(&chars, i) == Some(true))
        .or_else(|| (start..=hits_start).find(|&i| start_boundary(&chars, i).is_some()));
    if let Some(i) = snap_start {
        start = i;
    }
    let snap_end = (hits_end..=end)
        .rev()
        .find(|&i| end_boundary(&chars, i) == Some(true))
        .or_else(|| {
            (hits_end..=end)
                .rev()
                .find(|&i| end_boundary(&chars, i).is_some())
        });
    if let Some(i) = snap_end {
        end = i;
    }
    let body: String = chars[start..end].iter().collect();
    let body = body.trim();
    let mut out = String::new();
    if start > 0 {
        out.push('\u{2026}');
    }
    out.push_str(body);
    if end < total {
        out.push('\u{2026}');
    }
    out
}

/// `matched` with as much of `before` (cut from its start) and `after` (cut
/// from its end) as fits in `max_chars` characters, one line break apart.
/// The room is shared evenly, a side that needs less leaving the rest to the
/// other. A `matched` that does not fit alone is cut to the window around
/// the query's `terms` ([`query_window`]).
fn fit_around(
    matched: &str,
    before: &str,
    after: &str,
    max_chars: usize,
    terms: &HashSet<String>,
) -> String {
    let matched_chars = matched.chars().count();
    if matched_chars >= max_chars {
        return query_window(matched, max_chars, terms);
    }
    let room = max_chars - matched_chars;
    // A side costs its text and the line break that joins it.
    let need = |side: &str| {
        if side.is_empty() {
            0
        } else {
            side.chars().count() + 1
        }
    };
    let (before_need, after_need) = (need(before), need(after));
    let before_room = before_need.min((room / 2).max(room.saturating_sub(after_need)));
    let after_room = after_need.min(room - before_room);
    let mut out = String::new();
    if let Some(text) = tail_within(before, before_room.saturating_sub(1)) {
        out.push_str(&text);
        out.push('\n');
    }
    out.push_str(matched);
    if let Some(text) = head_within(after, after_room.saturating_sub(1)) {
        out.push('\n');
        out.push_str(&text);
    }
    out
}

/// The text of `result` within `max_chars` characters. A result widened with
/// its neighbours ([`SnippetParts`]) keeps the chunk that matched (whole when
/// it fits) and spends the rest on the neighbours allowed on `neighbor_pages`
/// ([`neighbor_allowed`]). A chunk that does not fit is cut to the window
/// around the query's `terms` ([`query_window`]).
fn passage_text(
    result: &ComprehensiveResult,
    max_chars: usize,
    neighbor_pages: &[u32],
    terms: &HashSet<String>,
) -> String {
    let snippet = result.snippet.as_str();
    let Some(parts) = SnippetParts::of(snippet, &result.metadata) else {
        return query_window(snippet.trim(), max_chars, terms);
    };
    let side = |list: &[NeighborPart]| -> String {
        list.iter()
            .filter(|p| neighbor_allowed(p.pages, neighbor_pages))
            .map(|p| snippet[p.start..p.end].trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (start, end) = parts.matched;
    fit_around(
        snippet[start..end].trim(),
        &side(&parts.before),
        &side(&parts.after),
        max_chars,
        terms,
    )
}

/// Passage `n` from `result`: its text within `max_chars` characters, with
/// neighbouring chunks only from `neighbor_pages` (empty: any page), and cut
/// around the query's `terms` ([`window_terms`]) when the chunk does not fit.
fn passage(
    n: u32,
    result: &ComprehensiveResult,
    max_chars: usize,
    neighbor_pages: &[u32],
    terms: &HashSet<String>,
) -> Passage {
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
    let url = slack_permalink(&path, result);
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
        text: passage_text(result, max_chars, neighbor_pages, terms),
        regions: passage_regions(result),
        display_path,
        title: document_title(result),
        url,
    }
}

/// The permalink an indexed Slack thread (`slack://…`) carries in its metadata; only an
/// `https://` link of a Slack thread.
fn slack_permalink(path: &str, result: &ComprehensiveResult) -> Option<String> {
    if !path.starts_with(crate::connectors::slack::ids::SOURCE_PREFIX) {
        return None;
    }
    result
        .metadata
        .get("url")
        .map(|u| u.trim())
        .filter(|u| u.starts_with("https://") && !u.contains(char::is_whitespace))
        .map(str::to_string)
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
                let paths: Vec<String> = outside.iter().map(|f| f.path.clone()).collect();
                plan.notes
                    .push(scope.outside_message(&format!("File {}", file_names(&paths))));
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

/// Whether `set` holds none of the scope's own sources: nothing, or only the
/// owners of the files added in chats, which every limited scope searches.
fn has_no_sources(set: &SourceSet) -> bool {
    set.source_paths.is_empty()
        && set
            .space_ids
            .iter()
            .all(|id| crate::rag_engine::is_chat_uploads(id))
}

/// Source ids as the model is told them: the owners of files added in chats
/// are named for what they hold, not by their internal ids.
fn describe_sources(ids: &[String]) -> String {
    let uploads = |id: &String| crate::rag_engine::is_chat_uploads(id);
    let mut named: Vec<&str> = ids
        .iter()
        .filter(|id| !uploads(id))
        .map(String::as_str)
        .collect();
    if ids.iter().any(uploads) {
        named.push("files added in chats");
    }
    named.join(", ")
}

/// File names of `paths`, for notes to the model: the user's folder layout (and the
/// user name in it) never goes into a note, and from there into an answer.
fn file_names(paths: &[String]) -> String {
    paths
        .iter()
        .map(|p| {
            let trimmed = p.trim_end_matches(['/', '\\']);
            trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
        })
        .collect::<Vec<_>>()
        .join(", ")
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
            file_names(scoped_files)
        ));
    } else if scope.restricted && plan.model_sources.is_empty() && plan.model_files.is_empty() {
        if has_no_sources(set) && scope.snippets.is_empty() {
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
                file_names(scoped_files)
            ));
        }
    } else if !scoped_files.is_empty() {
        notes.push(format!(
            "The user limited this answer to {}.",
            file_names(scoped_files)
        ));
    } else if plan.use_scope_files && !scope.source_ids.is_empty() {
        notes.push(format!(
            "The user limited this answer to sources {}.",
            describe_sources(&scope.source_ids)
        ));
    } else if plan.use_scope_files && !scope.files.is_empty() {
        notes.push(format!(
            "The files this answer is limited to are not indexed (removed or renamed since): {}. \
             Tell the user so.",
            file_names(&scope.files)
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
                .map_err(search_failure)?
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
                .map_err(search_failure)?;
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
        let terms = window_terms(query);
        let mut passages: Vec<Passage> = results
            .iter()
            .zip(first..)
            .map(|(r, n)| {
                // The page limit holds for the neighbours joined to a passage of
                // the user's files too, not only for the chunk that matched.
                let file = r.metadata.get("source_file").map_or("", String::as_str);
                let neighbor_pages: &[u32] = if scoped_files.iter().any(|f| f == file) {
                    pages
                } else {
                    &[]
                };
                passage(n, r, budget, neighbor_pages, &terms)
            })
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
    fn a_search_before_the_models_are_installed_tells_the_model_what_to_say() {
        for error in [
            anyhow::Error::from(SearchModelsMissing),
            anyhow::Error::from(SearchModelsMissing).context("embedding the query"),
            anyhow::anyhow!("{}", SearchModelsMissing),
        ] {
            match search_failure(error) {
                ToolError::Unavailable(message) => {
                    assert_eq!(message, SEARCH_NOT_READY);
                    assert!(!message.contains(SEARCH_MODELS_MISSING_CODE));
                }
                other => panic!("expected Unavailable, got {other:?}"),
            }
        }
        assert_eq!(
            search_failure(anyhow::anyhow!("disk full")),
            ToolError::Failed("Search failed: disk full".to_string())
        );
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
        let p = passage(3, &result, MAX_PASSAGE_CHARS, &[], &HashSet::new());
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
        let p = passage(1, &result, MAX_PASSAGE_CHARS, &[], &HashSet::new());
        assert_eq!(p.page.as_deref(), Some("5-6"));
        assert_eq!(p.section.as_deref(), Some("3 Method > 3.2 Chunkwise form"));
        let for_model = serde_json::to_value(&p).expect("json");
        assert!(for_model.get("regions").is_none());
        assert_eq!(for_model["section"], "3 Method > 3.2 Chunkwise form");
        let for_ui = p.detail_value();
        assert_eq!(for_ui["regions"][0]["page"], 5);
        assert_eq!(for_ui["regions"][0]["x0"], 72.0);
        assert!(
            for_ui.get("title").is_none(),
            "no title in this chunk's metadata"
        );
    }

    #[test]
    fn the_document_title_goes_to_the_detail_not_to_the_model() {
        let result = file_result(
            "c:/papers/q3 plan.pdf",
            &[("title", " Quarterly plan ")],
            "",
        );
        let p = passage(1, &result, MAX_PASSAGE_CHARS, &[], &HashSet::new());
        assert_eq!(p.detail_value()["title"], "Quarterly plan");
        assert!(serde_json::to_value(&p).unwrap().get("title").is_none());
        let untitled = file_result("c:/papers/q3 plan.pdf", &[], "  ");
        let p = passage(1, &untitled, MAX_PASSAGE_CHARS, &[], &HashSet::new());
        assert!(p.detail_value().get("title").is_none());
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
            passage(1, &by_filename, MAX_PASSAGE_CHARS, &[], &HashSet::new()).file,
            "SPANN - Billion-scale ANN Search.pdf"
        );

        let by_title = file_result(
            source,
            &[("title", " SPANN - Billion-scale ANN Search ")],
            "",
        );
        assert_eq!(
            passage(1, &by_title, MAX_PASSAGE_CHARS, &[], &HashSet::new()).file,
            "SPANN - Billion-scale ANN Search.pdf"
        );

        let by_citation = file_result(source, &[], "SPANN - Billion-scale ANN Search");
        assert_eq!(
            passage(1, &by_citation, MAX_PASSAGE_CHARS, &[], &HashSet::new()).file,
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
            passage(1, &other, MAX_PASSAGE_CHARS, &[], &HashSet::new()).file,
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
        let p = passage(1, &result, MAX_PASSAGE_CHARS, &[], &HashSet::new());
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
        let p = passage(1, &result, MAX_PASSAGE_CHARS, &[], &HashSet::new());
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
        let p = passage(1, &result, MAX_PASSAGE_CHARS, &[], &HashSet::new());
        assert_eq!(p.file, "Task: File GST return");
        assert_eq!(
            p.path,
            "calendar://task/ae887fee-eaa5-4c05-b53d-9b3e9905edef"
        );

        let mut untitled = result;
        untitled.metadata.clear();
        untitled.citation.source = "calendar://event/1".into();
        assert_eq!(
            passage(2, &untitled, MAX_PASSAGE_CHARS, &[], &HashSet::new()).file,
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
    fn notes_to_the_model_name_files_never_their_paths() {
        let path = "c:/users/someone/documents/research papers/markov memory.pdf";
        // "Ask about this file": the user's file limit.
        let scope = RunScope {
            files: vec![path.into()],
            ..RunScope::default()
        };
        let plan = plan_search(&scope, &ids(&[]));
        let set = SourceSet {
            space_ids: Vec::new(),
            source_paths: vec![path.into()],
        };
        let scoped = vec![path.to_string()];
        let note = limit_note(&scope, &plan, &set, &scoped, &[]).unwrap();
        assert_eq!(note, "The user limited this answer to markov memory.pdf.");
        let note = limit_note(&scope, &plan, &set, &scoped, &[4]).unwrap();
        assert!(note.ends_with("of markov memory.pdf."), "{note}");
        assert!(!note.contains("someone"), "{note}");
        // The file is gone from the index: said plainly, by name.
        let empty = SourceSet::default();
        let note = limit_note(&scope, &plan, &empty, &[], &[]).unwrap();
        assert!(
            note.starts_with("The files this answer is limited to are not indexed (removed or renamed since): markov memory.pdf."),
            "{note}"
        );
        assert!(!note.contains("someone"), "{note}");
        // A file the model asked for outside a workspace: named, not located.
        let ws = workspace_scope(&["space-a"], &[]);
        let model = SourceResolution {
            files: vec![super::super::sources::ResolvedFile {
                source_id: "space-b".into(),
                path: r"C:\Users\someone\notes\other.pdf".into(),
            }],
            ..SourceResolution::default()
        };
        let plan = plan_search(&ws, &model);
        assert!(
            plan.notes[0].starts_with("File other.pdf is outside"),
            "{:?}",
            plan.notes
        );
        assert!(!plan.notes[0].contains("someone"), "{:?}", plan.notes);
    }

    #[test]
    fn files_added_in_chats_are_named_and_are_not_a_workspace_source() {
        let only_uploads = SourceSet {
            space_ids: vec!["chat-uploads:ws-1".into()],
            source_paths: Vec::new(),
        };
        assert!(has_no_sources(&only_uploads));
        assert!(!has_no_sources(&SourceSet {
            space_ids: vec!["space-a".into(), "chat-uploads:ws-1".into()],
            source_paths: Vec::new(),
        }));
        // A workspace with no sources of its own is told so, uploads or not.
        let scope = workspace_scope(&["chat-uploads:ws-1"], &[]);
        let plan = plan_search(&scope, &ids(&[]));
        let note = limit_note(&scope, &plan, &only_uploads, &[], &[]).unwrap();
        assert!(note.contains("has no searchable sources yet"), "{note}");
        // Internal ids never reach the model.
        let limited = RunScope {
            source_ids: vec!["space-a".into(), "chat-uploads:main".into()],
            ..RunScope::default()
        };
        let plan = plan_search(&limited, &ids(&[]));
        let set = SourceSet {
            space_ids: plan.space_ids.clone(),
            source_paths: Vec::new(),
        };
        let note = limit_note(&limited, &plan, &set, &[], &[]).unwrap();
        assert!(note.contains("space-a, files added in chats"), "{note}");
        assert!(!note.contains("chat-uploads"), "{note}");
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
        assert!(
            plan.notes[0].contains("only uses the workspace's sources; the user can add"),
            "{}",
            plan.notes[0]
        );
        assert!(!plan.notes[0].contains("  "), "{:?}", plan.notes[0]);
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
    async fn search_results_name_files_never_their_paths() {
        let dir = tempfile::tempdir().unwrap();
        let rag = Box::pin(engine_with_two_spaces(dir.path())).await;
        let tool = SearchDocumentsTool::new(rag);
        let (paths, text) = search_paths(
            &tool,
            RunScope::default(),
            json!({"query": "notice period days"}),
        )
        .await;
        // The app keeps every path (citations open the file)...
        assert_eq!(paths.len(), 3, "{paths:?}");
        assert!(paths.iter().all(|p| p.contains("docs/")), "{paths:?}");
        // ...the model reads file names only.
        let lower = text.to_lowercase();
        assert!(
            !lower.contains("c:/docs") && !lower.contains("docs/"),
            "{text}"
        );
        assert!(!text.contains("\"path\""), "{text}");
        assert!(text.contains("inside.txt"), "{text}");
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

    /// A result as search returns it after widening it with neighbours: the
    /// snippet joined in reading order and where each part lies in it.
    fn widened(
        before: &[(&str, Option<(u32, u32)>)],
        matched: &str,
        after: &[(&str, Option<(u32, u32)>)],
    ) -> ComprehensiveResult {
        fn push(snippet: &mut String, piece: &str) -> (usize, usize) {
            if !snippet.is_empty() {
                snippet.push('\n');
            }
            let start = snippet.len();
            snippet.push_str(piece);
            (start, snippet.len())
        }
        let mut snippet = String::new();
        let mut parts = SnippetParts::default();
        for (text, pages) in before {
            let (start, end) = push(&mut snippet, text);
            parts.before.push(NeighborPart {
                start,
                end,
                pages: *pages,
            });
        }
        parts.matched = push(&mut snippet, matched);
        for (text, pages) in after {
            let (start, end) = push(&mut snippet, text);
            parts.after.push(NeighborPart {
                start,
                end,
                pages: *pages,
            });
        }
        let mut metadata = HashMap::new();
        metadata.insert(
            crate::rag_engine::SNIPPET_PARTS_KEY.to_string(),
            serde_json::to_string(&parts).unwrap(),
        );
        metadata.insert("page_start".to_string(), "4".to_string());
        metadata.insert("page_end".to_string(), "4".to_string());
        ComprehensiveResult {
            id: Uuid::nil(),
            score: 0.5,
            metadata,
            citation: Citation {
                title: "notes".into(),
                source: "c:/docs/notes.txt".into(),
                ..Citation::default()
            },
            snippet,
            source_index: "hybrid".into(),
        }
    }

    #[test]
    fn a_widened_passage_keeps_the_chunk_that_matched() {
        let before = format!("Spring ledger. {}", "b".repeat(985));
        let matched = format!("Summer ledger. {} The saffron comet.", "m".repeat(560));
        let after = format!("{} Autumn ledger.", "a".repeat(985));
        let result = widened(&[(&before, None)], &matched, &[(&after, None)]);
        let matched_chars = matched.chars().count();
        // Cut from its start, the snippet would be mostly the chunk before.
        assert!(!truncate_chars(result.snippet.trim(), 785).contains("saffron comet"));

        for budget in [MAX_PASSAGE_CHARS, passage_budget(MAX_K), MIN_PASSAGE_CHARS] {
            let text = passage(1, &result, budget, &[], &HashSet::new()).text;
            assert!(text.chars().count() <= budget, "{budget}: {text}");
            assert!(
                !text.contains("Spring ledger"),
                "the start of the chunk before goes first"
            );
            if matched_chars < budget {
                assert!(
                    text.contains(&matched),
                    "{budget}: the matched chunk is whole"
                );
                // The room left is shared by the two neighbours.
                assert!(text.starts_with('\u{2026}'), "{budget}: {text}");
                assert!(text.ends_with('\u{2026}'), "{budget}: {text}");
                assert!(
                    !text.contains("Autumn ledger"),
                    "the end of the chunk after goes first"
                );
            } else {
                // Alone too long for the budget: the matched chunk from its start.
                assert_eq!(text, truncate_chars(&matched, budget));
            }
        }

        // A short neighbour leaves its share to the other.
        let short = widened(&[(&before, None)], &matched, &[("Autumn ledger.", None)]);
        let text = passage(1, &short, MAX_PASSAGE_CHARS, &[], &HashSet::new()).text;
        assert!(
            text.ends_with(&format!("{matched}\nAutumn ledger.")),
            "{text}"
        );
        assert_eq!(text.chars().count(), MAX_PASSAGE_CHARS);

        // A result that was not widened is cut from its end, as before.
        let plain = ComprehensiveResult {
            metadata: HashMap::new(),
            ..widened(&[], &"p".repeat(900), &[])
        };
        assert_eq!(
            passage(1, &plain, MIN_PASSAGE_CHARS, &[], &HashSet::new()).text,
            truncate_chars(&"p".repeat(900), MIN_PASSAGE_CHARS)
        );
    }

    #[test]
    fn widened_passages_are_cut_on_character_boundaries() {
        let before = "東京の港は朝に開き、漁船が沖へ向かう。".repeat(30);
        let matched = "灯台守は北の崖の上に彗星を見た。".repeat(12);
        let after = "秋の帳簿には潮の満ち引きが記されている。".repeat(30);
        let result = widened(&[(&before, None)], &matched, &[(&after, None)]);
        for budget in [1, 2, 3, 7, 100, 191, 192, 193, 194, 300, 785, 1_500] {
            let text = passage(1, &result, budget, &[], &HashSet::new()).text;
            assert!(text.chars().count() <= budget, "{budget}: {text}");
            if matched.chars().count() < budget {
                assert!(text.contains(&matched), "{budget}");
            } else {
                assert!(
                    matched.starts_with(text.trim_end_matches('\u{2026}')),
                    "{budget}"
                );
            }
        }
    }

    #[test]
    fn a_page_limited_passage_takes_neighbours_from_its_pages_only() {
        let matched = "Page four holds the lighthouse keeper's log of the saffron comet.";
        let result = widened(
            &[
                ("Page two: the spring tide tables.", Some((2, 2))),
                ("Page three: the ferry timetable.", Some((3, 3))),
            ],
            matched,
            &[
                ("Pages four and five: the weather.", Some((4, 5))),
                ("A note without a page.", None),
            ],
        );
        let text =
            |pages: &[u32]| passage(1, &result, MAX_PASSAGE_CHARS, pages, &HashSet::new()).text;
        // Page 4 only: nothing from pages 2, 3 or 5, nor from no page.
        assert_eq!(text(&[4]), matched);
        assert_eq!(
            text(&[3, 4]),
            format!("Page three: the ferry timetable.\n{matched}")
        );
        assert_eq!(
            text(&[2, 3, 4, 5]),
            format!(
                "Page two: the spring tide tables.\nPage three: the ferry timetable.\n\
                 {matched}\nPages four and five: the weather."
            )
        );
        // Without a page limit every neighbour joins.
        assert!(text(&[]).ends_with("A note without a page."));
        assert!(!neighbor_allowed(Some((1, u32::MAX)), &[1, 2]));
        assert!(!neighbor_allowed(None, &[1]));
        assert!(neighbor_allowed(None, &[]));
    }

    /// A plain-text document long enough for several window chunks, indexed
    /// with windows that end at its paragraphs.
    async fn engine_with_long_notes(dir: &std::path::Path) -> RAGEngine {
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.join("data");
        config.embedding.model_dir = dir.join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        config.chunking.chunk_size = 600;
        config.chunking.chunk_overlap = 0;
        let mut engine = RAGEngine::new(config).await.unwrap();
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                crate::statements::testing::WordEmbedder::default(),
            )))
            .unwrap();
        let filler = "The harbour archive lists the tide tables, the ferry times and the \
                      weather for each week of the season. ";
        let text = [
            format!("Spring ledger. {}", filler.repeat(5)),
            format!(
                "Summer ledger. {}The lighthouse keeper recorded a saffron comet above the \
                 northern cliffs.",
                filler.repeat(4)
            ),
            format!("Autumn ledger. {}", filler.repeat(5)),
            format!("Winter ledger. {}", filler.repeat(5)),
        ]
        .map(|p| p.trim().to_string())
        .join("\n\n");
        let path = stored("C:/docs/notes/harbour.txt");
        let mut metadata = HashMap::new();
        metadata.insert("space_id".to_string(), "notes".to_string());
        metadata.insert("file_path".to_string(), path.clone());
        metadata.insert("title".to_string(), "harbour".to_string());
        let ids = engine
            .add_document(
                &text,
                crate::types::DocumentFormat::TXT,
                metadata,
                Citation {
                    title: "harbour".into(),
                    source: path,
                    ..Citation::default()
                },
            )
            .await
            .unwrap();
        assert!(ids.len() >= 3, "{} chunks", ids.len());
        engine
    }

    #[tokio::test]
    async fn a_txt_passage_shows_the_chunk_that_matched_not_the_one_before() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Box::pin(engine_with_long_notes(dir.path())).await;
        let results =
            Box::pin(engine.search_comprehensive("saffron comet lighthouse keeper", 3, None))
                .await
                .unwrap();
        let top = &results[0];
        assert_eq!(
            top.metadata.get("chunk_index").map(String::as_str),
            Some("1"),
            "{}",
            top.snippet
        );
        let parts = SnippetParts::of(&top.snippet, &top.metadata).expect("widened");
        let matched = top.snippet[parts.matched.0..parts.matched.1].trim();
        assert!(matched.starts_with("Summer ledger."), "{matched}");
        assert!(matched.ends_with("northern cliffs."), "{matched}");
        assert!(
            top.snippet.starts_with("Spring ledger."),
            "the chunk before comes first"
        );

        let terms = window_terms("saffron comet lighthouse keeper");
        for budget in [MAX_PASSAGE_CHARS, passage_budget(MAX_K), MIN_PASSAGE_CHARS] {
            let text = passage(1, top, budget, &[], &terms).text;
            assert!(text.chars().count() <= budget, "{budget}: {text}");
            if matched.chars().count() < budget {
                assert!(text.contains(matched), "{budget}: {text}");
            } else {
                // Too long for the budget: the part of it the query matched.
                assert!(text.contains("saffron comet"), "{budget}: {text}");
            }
            assert!(!text.contains("Spring ledger"), "{budget}: {text}");
        }
    }

    /// A 1,750-character chunk whose only match is in its last 250 characters.
    fn late_match_chunk() -> String {
        let filler = "The harbour archive lists the tide tables and the ferry times. ";
        let mut text = filler.repeat(1_500 / filler.len() + 1);
        text.truncate(1_500);
        let text = text.trim_end().to_string();
        let tail = format!(
            " The lighthouse keeper recorded a saffron comet above the northern cliffs. {}",
            filler.repeat(4)
        );
        let mut chunk = format!("{text}{tail}");
        chunk.truncate(1_750);
        assert_eq!(chunk.chars().count(), 1_750);
        assert!(chunk.find("saffron").unwrap() > 1_500);
        chunk
    }

    #[test]
    fn an_oversized_chunk_is_cut_around_the_query() {
        let chunk = late_match_chunk();
        let terms = window_terms("Which saffron comet did the keeper see?");
        assert!(terms.contains("saffron") && terms.contains("keeper"));
        assert!(
            !terms.contains("which") && !terms.contains("the"),
            "stopwords: {terms:?}"
        );

        // Unexpanded and widened alike.
        let plain = ComprehensiveResult {
            metadata: HashMap::new(),
            ..widened(&[], &chunk, &[])
        };
        let in_context = widened(
            &[("Spring ledger.", None)],
            &chunk,
            &[("Autumn ledger.", None)],
        );
        for result in [&plain, &in_context] {
            for budget in [MAX_PASSAGE_CHARS, passage_budget(MAX_K), MIN_PASSAGE_CHARS] {
                let text = passage(1, result, budget, &[], &terms).text;
                assert!(text.chars().count() <= budget, "{budget}: {text}");
                assert!(text.contains("saffron comet"), "{budget}: {text}");
                assert!(text.starts_with('\u{2026}'), "{budget}: {text}");
                assert!(!text.contains("ledger"), "no room for neighbours: {text}");
            }
        }
        // Snapped to the sentence the match is in when the window starts mid-text.
        let text = passage(1, &plain, MIN_PASSAGE_CHARS, &[], &terms).text;
        let body = text.trim_start_matches('\u{2026}');
        assert!(body.starts_with("The "), "{text}");

        // None of the query's words in the chunk: its start, as before.
        let other = window_terms("monsoon rainfall");
        assert_eq!(
            passage(1, &plain, MIN_PASSAGE_CHARS, &[], &other).text,
            truncate_chars(&chunk, MIN_PASSAGE_CHARS)
        );
        assert_eq!(
            passage(1, &plain, MIN_PASSAGE_CHARS, &[], &HashSet::new()).text,
            truncate_chars(&chunk, MIN_PASSAGE_CHARS)
        );
    }

    #[test]
    fn the_window_holds_the_most_query_words_and_the_earliest_on_a_tie() {
        let gap = "plain words of the archive ".repeat(20);
        let text = format!("Alpha opens it. {gap}Beta sits here. {gap}Beta again, beta.");
        let terms = window_terms("alpha beta");
        let window = query_window(&text, 60, &terms);
        assert!(window.chars().count() <= 60, "{window}");
        assert!(window.contains("Beta again, beta."), "{window}");
        // One occurrence each: the first one.
        let tie = format!("Alpha opens it. {gap}Beta sits here. {gap}");
        let window = query_window(&tie, 60, &terms);
        assert!(window.contains("Alpha"), "{window}");
        assert!(!window.contains("Beta"), "{window}");
        // Whole words only, in any case.
        let partial = format!("{gap}Alphabetical order. {gap}ALPHA at last.");
        let window = query_window(&partial, 40, &window_terms("alpha"));
        assert!(window.contains("ALPHA at last."), "{window}");
    }

    #[test]
    fn query_windows_are_cut_on_character_boundaries() {
        let cjk = "灯台守は北の崖の上に彗星を見た。".repeat(40);
        let text = format!("{cjk} saffron comet {cjk}");
        let terms = window_terms("saffron comet");
        for budget in 1..=40 {
            let window = query_window(&text, budget, &terms);
            assert!(window.chars().count() <= budget, "{budget}: {window}");
        }
        for budget in [20, 33, 191, 300, 785] {
            let window = query_window(&text, budget, &terms);
            assert!(window.chars().count() <= budget, "{budget}: {window}");
            assert!(window.contains("saffron comet"), "{budget}: {window}");
        }
        // A CJK term is a word run of its own and matched whole.
        let runs = format!(
            "{} 彗星 {}",
            "東京の港。".repeat(100),
            "東京の港。".repeat(100)
        );
        let window = query_window(&runs, 30, &window_terms("彗星"));
        assert!(window.contains("彗星"), "{window}");
    }
}
