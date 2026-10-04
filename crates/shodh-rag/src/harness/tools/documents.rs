//! `open_document`: the text of a page or character range of an indexed file.
//!
//! Every read is numbered like search results: the text is cut into
//! page-level passages (longer pages and ranges into pieces of at most
//! [`MAX_READ_PASSAGE_CHARS`]), each registered in the run's citation
//! numbering with file, path, page, section and boxes, so an answer built
//! only from reads cites passages the grounding check can resolve.
//!
//! Access rule: only files present in the index can be opened. The argument
//! is normalised the same way the indexer normalises paths and matched
//! against the stored `source`; the stored path (never the raw argument) is
//! what gets read. Calendar and note pseudo-sources are not files.

use std::path::{Component, Path};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::{
    req_str, CitedPassage, HostTool, ToolContext, ToolError, ToolOutput, UNTRUSTED_NOTICE,
};
use crate::harness::events::RiskTier;
use crate::processing::parser::{DocumentParser, ParsedDocument};
use crate::types::{DocumentFormat, DocumentSection};
use crate::RAGEngine;

pub const OPEN_DOCUMENT: &str = "open_document";

/// Characters returned per call.
pub const MAX_RANGE_CHARS: usize = 12_000;

pub struct OpenDocumentTool {
    rag: Arc<RwLock<RAGEngine>>,
}

impl OpenDocumentTool {
    pub fn new(rag: Arc<RwLock<RAGEngine>>) -> Self {
        Self { rag }
    }
}

/// What part of the document to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    Page(usize),
    Range {
        start: usize,
        end: usize,
    },
    /// Both were given: the page when the document has page text, else the
    /// range. The output says which was used.
    PageOrRange {
        page: usize,
        start: usize,
        end: usize,
    },
    Beginning,
}

impl Selection {
    fn wants_page(self) -> bool {
        matches!(self, Selection::Page(_) | Selection::PageOrRange { .. })
    }
}

fn range_bounds(range: &Value) -> Result<(usize, usize), ToolError> {
    let start = range.get("start").and_then(Value::as_u64).unwrap_or(0);
    let end = range
        .get("end")
        .and_then(Value::as_u64)
        .unwrap_or(start + MAX_RANGE_CHARS as u64);
    if end <= start {
        return Err(ToolError::InvalidArguments {
            tool: OPEN_DOCUMENT.to_string(),
            reasons: "range.end must be greater than range.start".to_string(),
        });
    }
    Ok((
        usize::try_from(start).unwrap_or(usize::MAX),
        usize::try_from(end).unwrap_or(usize::MAX),
    ))
}

fn selection(args: &Value) -> Result<Selection, ToolError> {
    let page = args
        .get("page")
        .and_then(Value::as_u64)
        .map(|p| usize::try_from(p).unwrap_or(usize::MAX));
    let range = args.get("range").map(range_bounds).transpose()?;
    Ok(match (page, range) {
        (Some(page), Some((start, end))) => Selection::PageOrRange { page, start, end },
        (Some(page), None) => Selection::Page(page),
        (None, Some((start, end))) => Selection::Range { start, end },
        (None, None) => Selection::Beginning,
    })
}

/// Page texts of a PDF read straight from the file with pdf-extract, used
/// when the parser found no page structure (e.g. fonts lopdf cannot
/// decode). Pages are numbered by position, from 1. `None` when the file
/// cannot be split into pages.
fn pdf_pages_from_file(path: &Path) -> Option<Vec<(usize, String)>> {
    let bytes = std::fs::read(path).ok()?;
    // pdf-extract can panic on malformed font tables; a panic means "no
    // page text", not a crashed tool call.
    let pages = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem_by_pages(&bytes)
    }))
    .ok()?
    .ok()?;
    let numbered: Vec<(usize, String)> = pages
        .into_iter()
        .enumerate()
        .map(|(i, text)| (i + 1, text))
        .collect();
    numbered
        .iter()
        .any(|(_, t)| !t.trim().is_empty())
        .then_some(numbered)
}

/// Reject pseudo-sources and parent-directory traversal before any lookup.
fn check_requested_path(path: &str) -> Result<(), ToolError> {
    if path.contains("://") {
        return Err(ToolError::Forbidden(
            "Only indexed files can be opened; calendar items and notes are not files".to_string(),
        ));
    }
    if Path::new(path)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
        || path.split(['/', '\\']).any(|part| part == "..")
    {
        return Err(ToolError::Forbidden(
            "Paths containing '..' are not allowed".to_string(),
        ));
    }
    Ok(())
}

fn slice_chars(text: &str, start: usize, end: usize) -> (String, usize) {
    let total = text.chars().count();
    let end = end.min(start.saturating_add(MAX_RANGE_CHARS)).min(total);
    let slice: String = text
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    (slice, total)
}

/// Page texts recorded by the parser: from the structured document's
/// blocks when the layout parser read the file, else from paged sections.
fn parsed_pages(parsed: &ParsedDocument) -> Vec<(usize, String)> {
    if let Some(doc) = parsed.document.as_ref().filter(|d| !d.pages.is_empty()) {
        let pages: Vec<(usize, String)> = doc
            .pages
            .iter()
            .map(|p| (p.number as usize, doc.page_text(p.number)))
            .collect();
        if pages.iter().any(|(_, text)| !text.trim().is_empty()) {
            return pages;
        }
    }
    parsed
        .structured_sections
        .iter()
        .filter_map(|s| match s {
            DocumentSection::Text { content, page, .. } if *page > 0 => {
                Some((*page, content.clone()))
            }
            _ => None,
        })
        .collect()
}

fn page_text(pages: &[(usize, String)], page: usize) -> Result<String, ToolError> {
    if pages.is_empty() {
        return Err(ToolError::NotFound(
            "This document has no page structure; request a character range instead".to_string(),
        ));
    }
    let text: Vec<&str> = pages
        .iter()
        .filter(|(p, _)| *p == page)
        .map(|(_, t)| t.as_str())
        .collect();
    if text.is_empty() {
        let last = pages.iter().map(|(p, _)| *p).max().unwrap_or(1);
        return Err(ToolError::NotFound(format!(
            "Page {page} not found; the document has pages 1 to {last}"
        )));
    }
    Ok(text.join("\n"))
}

/// Longest numbered passage of a read. A page is usually one passage; a
/// longer page or range is split (at paragraph, line or sentence breaks) so
/// a citation points at a few paragraphs, not at 12 000 characters.
pub const MAX_READ_PASSAGE_CHARS: usize = 4_000;

/// Shortest text used to find which page a piece of a range read is on;
/// shorter probes match too many pages to say.
const PAGE_PROBE_MIN_CHARS: usize = 24;
/// Characters of a piece's start (and end) looked up in the page texts.
const PAGE_PROBE_CHARS: usize = 80;

/// One numbered piece of a read: what the model sees for one `[n]`.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    /// "4", or "4-5" for a piece of a range read that crosses a page break.
    page: Option<String>,
    /// Heading chain the piece starts under ("3 Method > 3.2 Chunkwise form").
    section: Option<String>,
    /// Boxes of the piece's blocks (`{"page":4,"x0":..}`), for the viewer only.
    regions: Vec<Value>,
    text: String,
}

/// The selected part of a document, cut into passages, before numbering.
#[derive(Debug, Clone, PartialEq)]
struct Read {
    title: String,
    /// "page 4" or "characters 0–12000 of 48000", plus a note when the
    /// page-or-range choice was made for the model.
    location: String,
    segments: Vec<Segment>,
    /// How to continue when the selection was cut.
    hint: Option<String>,
}

/// A unit the packer never splits unless it alone exceeds the budget: a
/// layout block or a paragraph.
struct Unit {
    text: String,
    section: Option<String>,
    region: Option<Value>,
}

/// `text` cut into pieces of at most `max` characters, preferring a line
/// break, then a sentence end, then a space in the second half of a piece.
fn split_hard(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > max {
        let window = &rest[..max];
        let floor = max / 2;
        let breaks: [&[char]; 3] = [&['\n'], &['.', '!', '?', ';'], &[' ', '\t']];
        let cut = breaks
            .iter()
            .find_map(|set| {
                (floor..max)
                    .rev()
                    .find(|&i| set.contains(&window[i]))
                    .map(|i| i + 1)
            })
            .unwrap_or(max);
        let piece: String = rest[..cut].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
        rest.drain(..cut);
    }
    let piece: String = rest.into_iter().collect();
    let piece = piece.trim();
    if !piece.is_empty() {
        out.push(piece.to_string());
    }
    out
}

/// Units packed into pieces of at most `max` characters, joined by `sep`.
/// A piece takes the section of its first unit and the boxes of all of them.
fn pack(units: Vec<Unit>, max: usize, sep: &str) -> Vec<Segment> {
    let gap = sep.chars().count();
    let mut out: Vec<Segment> = Vec::new();
    let mut current: Option<(Segment, usize)> = None;
    for unit in units {
        let pieces = split_hard(&unit.text, max);
        let whole = pieces.len() == 1;
        for piece in pieces {
            let len = piece.chars().count();
            let region = if whole { unit.region.clone() } else { None };
            match current.as_mut() {
                Some((segment, used)) if *used + gap + len <= max => {
                    segment.text.push_str(sep);
                    segment.text.push_str(&piece);
                    segment.regions.extend(region);
                    *used += gap + len;
                }
                _ => {
                    out.extend(current.take().map(|(s, _)| s));
                    current = Some((
                        Segment {
                            page: None,
                            section: unit.section.clone(),
                            regions: region.into_iter().collect(),
                            text: piece,
                        },
                        len,
                    ));
                }
            }
        }
    }
    out.extend(current.map(|(s, _)| s));
    out
}

/// Units cut to `limit` characters in total (with separators), the last
/// one shortened to fit.
fn limit_units(units: Vec<Unit>, limit: usize, sep: &str) -> Vec<Unit> {
    let gap = sep.chars().count();
    let mut out = Vec::new();
    let mut used = 0usize;
    for mut unit in units {
        let between = if out.is_empty() { 0 } else { gap };
        let room = limit.saturating_sub(used + between);
        if room == 0 {
            break;
        }
        let len = unit.text.chars().count();
        if len > room {
            unit.text = unit.text.chars().take(room).collect();
            unit.region = None;
            out.push(unit);
            break;
        }
        used += between + len;
        out.push(unit);
    }
    out
}

/// Paragraphs of plain text as units.
fn paragraph_units(text: &str) -> Vec<Unit> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| Unit {
            text: p.to_string(),
            section: None,
            region: None,
        })
        .collect()
}

/// The layout blocks of `page` as units (with their section and box), when
/// the parser read the document's layout and the page text came from it.
fn block_units(parsed: &ParsedDocument, page: usize, page_text: &str) -> Option<Vec<Unit>> {
    let doc = parsed.document.as_ref()?;
    let number = u32::try_from(page).ok()?;
    if doc.page_text(number) != page_text {
        return None;
    }
    let units: Vec<Unit> = doc
        .blocks
        .iter()
        .filter(|b| b.page == Some(number))
        .filter_map(|b| {
            let text = b.render().trim().to_string();
            if text.is_empty() {
                return None;
            }
            let section = (!b.section_path.is_empty()).then(|| b.section_path.join(" > "));
            let region = b.bbox.map(
                |bb| json!({ "page": number, "x0": bb.x0, "y0": bb.y0, "x1": bb.x1, "y1": bb.y1 }),
            );
            Some(Unit {
                text,
                section,
                region,
            })
        })
        .collect();
    (!units.is_empty()).then_some(units)
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The one page whose text contains `probe`; `None` when no page or
/// several do (a wrong page is worse than none).
fn page_of(pages: &[(usize, String)], probe: &str) -> Option<usize> {
    if probe.chars().count() < PAGE_PROBE_MIN_CHARS {
        return None;
    }
    let mut found = pages
        .iter()
        .filter(|(_, text)| text.contains(probe))
        .map(|(p, _)| *p);
    let first = found.next()?;
    found.all(|p| p == first).then_some(first)
}

/// The page (or "start-end" pages) a piece of a range read lies on, found
/// by looking the start of its first paragraph and the end of its last up
/// in the page texts (a probe never spans two paragraphs, which a page
/// break may separate).
fn locate_pages(normalized_pages: &[(usize, String)], text: &str) -> Option<String> {
    let mut paragraphs = text.split("\n\n").filter(|p| !p.trim().is_empty());
    let first: Vec<char> = normalized(paragraphs.next()?).chars().collect();
    let last: Vec<char> = paragraphs
        .last()
        .map(|p| normalized(p).chars().collect())
        .unwrap_or_else(|| first.clone());
    let head: String = first.iter().take(PAGE_PROBE_CHARS).collect();
    let tail: String = last[last.len().saturating_sub(PAGE_PROBE_CHARS)..]
        .iter()
        .collect();
    let start = page_of(normalized_pages, &head)?;
    match page_of(normalized_pages, &tail) {
        Some(end) if end > start => Some(format!("{start}-{end}")),
        _ => Some(start.to_string()),
    }
}

/// A character range cut into passages, each with the page(s) it was found
/// on when the document has page text.
fn range_segments(slice: &str, pages: &[(usize, String)]) -> Vec<Segment> {
    let mut segments = pack(paragraph_units(slice), MAX_READ_PASSAGE_CHARS, "\n\n");
    if !pages.is_empty() {
        let normalized_pages: Vec<(usize, String)> =
            pages.iter().map(|(p, t)| (*p, normalized(t))).collect();
        for segment in &mut segments {
            segment.page = locate_pages(&normalized_pages, &segment.text);
        }
    }
    segments
}

/// Select and cut the requested part of a parsed document. `pages` are its
/// page texts (from the parser, or read from the PDF directly).
fn render(
    parsed: &ParsedDocument,
    pages: &[(usize, String)],
    selection: Selection,
) -> Result<Read, ToolError> {
    let (selection, note) = match selection {
        Selection::PageOrRange { page, start, end } => {
            if pages.is_empty() {
                (
                    Selection::Range { start, end },
                    Some("used the range: this document has no page text"),
                )
            } else {
                (
                    Selection::Page(page),
                    Some("used the page; the range was ignored"),
                )
            }
        }
        other => (other, None),
    };
    let (segments, location, hint) = match selection {
        Selection::Page(page) => {
            let text = page_text(pages, page)?;
            let total = text.chars().count();
            let hint = (total > MAX_RANGE_CHARS).then(|| {
                format!(
                    "[page truncated: {} more characters]",
                    total - MAX_RANGE_CHARS
                )
            });
            let units = block_units(parsed, page, &text).unwrap_or_else(|| paragraph_units(&text));
            let mut segments = pack(
                limit_units(units, MAX_RANGE_CHARS, "\n\n"),
                MAX_READ_PASSAGE_CHARS,
                "\n\n",
            );
            for segment in &mut segments {
                segment.page = Some(page.to_string());
            }
            (segments, format!("page {page}"), hint)
        }
        Selection::Range { start, end } => {
            let (slice, total) = slice_chars(&parsed.content, start, end);
            let shown_end = start + slice.chars().count();
            let hint = (shown_end < total).then(|| {
                format!(
                    "[{} more characters; continue with range start={shown_end}]",
                    total - shown_end
                )
            });
            (
                range_segments(&slice, pages),
                format!("characters {start}–{shown_end} of {total}"),
                hint,
            )
        }
        Selection::Beginning => {
            let (slice, total) = slice_chars(&parsed.content, 0, MAX_RANGE_CHARS);
            let shown = slice.chars().count();
            let hint = (shown < total).then(|| {
                format!(
                    "[{} more characters; continue with range start={shown}]",
                    total - shown
                )
            });
            (
                range_segments(&slice, pages),
                format!("characters 0–{shown} of {total}"),
                hint,
            )
        }
        Selection::PageOrRange { .. } => {
            return Err(ToolError::Failed(
                "internal error: page-or-range was not resolved".to_string(),
            ))
        }
    };
    let location = match note {
        Some(note) => format!("{location} ({note})"),
        None => location,
    };
    if segments.is_empty() {
        return Err(ToolError::NotFound(format!(
            "No text found at {location} of {}",
            parsed.title
        )));
    }
    Ok(Read {
        title: parsed.title.clone(),
        location,
        segments,
        hint,
    })
}

/// What the user sees for a file: its name.
fn file_name(source: &str) -> String {
    source
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(source)
        .to_string()
}

/// Number every piece of `read` in the run (a piece read before keeps its
/// number) and build the tool output: the pieces with their numbers for the
/// model, and the same passages in the step detail for the transcript.
fn cite_read(read: &Read, source: &str, ctx: &ToolContext) -> ToolOutput {
    let file = file_name(source);
    let mut text = format!(
        "{UNTRUSTED_NOTICE}\nDocument: {} ({source}), {}\nCite what you use with the passage numbers below.\n",
        read.title, read.location
    );
    let mut passages = Vec::with_capacity(read.segments.len());
    for segment in &read.segments {
        let n = ctx.cite_passage(CitedPassage {
            n: 0,
            file: file.clone(),
            path: source.to_string(),
            page: segment.page.clone(),
            web: false,
            text: segment.text.clone(),
            checkable: true,
        });
        let mut heading = format!("[{n}] {file}");
        if let Some(page) = &segment.page {
            heading.push_str(&format!(", p. {page}"));
        }
        if let Some(section) = &segment.section {
            heading.push_str(&format!(" — {section}"));
        }
        text.push('\n');
        text.push_str(&heading);
        text.push('\n');
        text.push_str(&segment.text);
        text.push('\n');
        let mut passage = json!({
            "n": n,
            "file": file,
            "path": source,
            "page": segment.page,
            "section": segment.section,
            "score": 1.0,
            "text": segment.text,
        });
        if let (false, Some(map)) = (segment.regions.is_empty(), passage.as_object_mut()) {
            map.insert("regions".to_string(), Value::Array(segment.regions.clone()));
        }
        passages.push(passage);
    }
    if let Some(hint) = &read.hint {
        text.push_str(hint);
        text.push('\n');
    }
    ToolOutput {
        text_for_model: text.trim_end().to_string(),
        summary_for_ui: format!("Read {}, {}", read.title, read.location),
        detail: Some(json!({
            "path": source,
            "location": read.location,
            "passages": passages,
        })),
    }
}

#[async_trait]
impl HostTool for OpenDocumentTool {
    fn name(&self) -> &'static str {
        OPEN_DOCUMENT
    }
    fn label(&self) -> &'static str {
        "Open document"
    }
    fn label_template(&self) -> &'static str {
        "Reading {path}"
    }
    fn description(&self) -> &'static str {
        "Read part of an indexed document: a page (PDFs) or a character range. Pass the path from \
         search_documents (or a unique file name). Without page or range, returns the beginning. \
         Long sections are cut at 12,000 characters with a hint for the next range. Examples: \
         {\"path\":\"x.pdf\",\"page\":3} or {\"path\":\"x.pdf\",\"range\":{\"start\":0,\"end\":6000}}. \
         If both are given, the page is used when the document has page text, else the range."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 1024},
                "page": {"type": "integer", "minimum": 1},
                "range": {
                    "type": "object",
                    "properties": {
                        "start": {"type": "integer", "minimum": 0},
                        "end": {"type": "integer", "minimum": 1}
                    },
                    "required": ["start"],
                    "additionalProperties": false
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let requested = req_str(&args, "path", OPEN_DOCUMENT)?;
        check_requested_path(requested)?;
        let selection = selection(&args)?;

        let matches = {
            let rag = self.rag.read().await;
            rag.find_indexed_sources(requested)
                .await
                .map_err(|e| ToolError::Failed(format!("Could not read the index: {e}")))?
        };
        let source = match matches.as_slice() {
            [] => {
                return Err(ToolError::NotFound(format!(
                    "{requested} is not an indexed file. Use a path returned by search_documents."
                )))
            }
            [one] => one.clone(),
            many => {
                return Err(ToolError::NotFound(format!(
                    "{requested} matches several indexed files; pass the full path: {}",
                    many.join(", ")
                )))
            }
        };
        if source.contains("://") {
            return Err(ToolError::Forbidden(
                "Only indexed files can be opened; calendar items and notes are not files"
                    .to_string(),
            ));
        }

        let path = source.clone();
        let wants_page = selection.wants_page();
        let (parsed, pages) = tokio::task::spawn_blocking(move || {
            let file = Path::new(&path);
            let parsed = DocumentParser::new().parse_file(file)?;
            let mut pages = parsed_pages(&parsed);
            if wants_page && pages.is_empty() && parsed.format == DocumentFormat::PDF {
                pages = pdf_pages_from_file(file).unwrap_or_default();
            }
            Ok::<_, anyhow::Error>((parsed, pages))
        })
        .await
        .map_err(|e| ToolError::Failed(format!("Reading the document was interrupted: {e}")))?
        .map_err(|e| {
            ToolError::Unavailable(format!(
                "{source} is indexed but could not be read from disk ({e}). It may have been moved or deleted; re-index its folder."
            ))
        })?;
        let read = render(&parsed, &pages, selection)?;
        // Claims citing any passage of this file (a search hit too) are
        // also checked against what the model read here.
        for segment in &read.segments {
            ctx.record_opened(&source, &segment.text);
        }
        Ok(cite_read(&read, &source, ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn doc(content: &str, pages: &[(usize, &str)]) -> ParsedDocument {
        ParsedDocument {
            content: content.to_string(),
            title: "Acme MSA".to_string(),
            metadata: HashMap::new(),
            format: DocumentFormat::from_extension("pdf"),
            structured_sections: pages
                .iter()
                .map(|(page, text)| DocumentSection::Text {
                    content: text.to_string(),
                    page: *page,
                    heading: None,
                })
                .collect(),
            document: None,
        }
    }

    #[test]
    fn traversal_and_pseudo_sources_are_rejected() {
        assert!(check_requested_path("c:/docs/../secrets.txt").is_err());
        assert!(check_requested_path("..\\x").is_err());
        assert!(check_requested_path("calendar://task/1").is_err());
        assert!(check_requested_path("c:/docs/acme.pdf").is_ok());
        assert!(check_requested_path("acme..v2.pdf").is_ok());
    }

    #[test]
    fn selection_parsing() {
        assert_eq!(selection(&json!({})).unwrap(), Selection::Beginning);
        assert_eq!(selection(&json!({"page": 3})).unwrap(), Selection::Page(3));
        assert_eq!(
            selection(&json!({"range": {"start": 10}})).unwrap(),
            Selection::Range {
                start: 10,
                end: 10 + MAX_RANGE_CHARS
            }
        );
        assert!(selection(&json!({"range": {"start": 10, "end": 5}})).is_err());
        assert_eq!(
            selection(&json!({"page": 1, "range": {"start": 0, "end": 6000}})).unwrap(),
            Selection::PageOrRange {
                page: 1,
                start: 0,
                end: 6000
            }
        );
    }

    fn ctx() -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext::new("run", "step", tx)
    }

    fn open(
        ctx: &ToolContext,
        parsed: &ParsedDocument,
        pages: &[(usize, String)],
        source: &str,
        selection: Selection,
    ) -> ToolOutput {
        cite_read(&render(parsed, pages, selection).unwrap(), source, ctx)
    }

    fn passages_of(out: &ToolOutput) -> Vec<Value> {
        out.detail.as_ref().unwrap()["passages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn page_and_range_together_use_the_page_when_there_is_page_text() {
        let paged = doc("abcdefghij", &[(1, "first page"), (2, "second page")]);
        let both = Selection::PageOrRange {
            page: 2,
            start: 0,
            end: 3,
        };
        let ctx = ctx();
        let out = open(
            &ctx,
            &paged,
            &parsed_pages(&paged),
            "c:/docs/acme.pdf",
            both,
        );
        assert!(out
            .text_for_model
            .contains("[1] acme.pdf, p. 2\nsecond page"));
        assert_eq!(
            ctx.cited_passage(1).unwrap().text,
            "second page",
            "the text recorded for checking claims is the text shown"
        );
        assert!(out
            .summary_for_ui
            .contains("page 2 (used the page; the range was ignored)"));

        let unpaged = doc("abcdefghij", &[]);
        let out = open(&ctx, &unpaged, &[], "c:/docs/scan.pdf", both);
        assert!(out.text_for_model.contains("abc"));
        assert!(!out.text_for_model.contains("abcd"));
        assert!(out
            .summary_for_ui
            .contains("(used the range: this document has no page text)"));
    }

    #[test]
    fn pages_read_from_the_pdf_replace_missing_page_structure() {
        let unpaged = doc("whole text", &[]);
        let from_file = vec![(1, "cover".to_string()), (2, "terms".to_string())];
        let out = open(
            &ctx(),
            &unpaged,
            &from_file,
            "c:/docs/a.pdf",
            Selection::Page(2),
        );
        assert!(out.text_for_model.contains("[1] a.pdf, p. 2\nterms"));
        assert!(pdf_pages_from_file(Path::new("c:/definitely/missing.pdf")).is_none());
    }

    #[test]
    fn pages_and_ranges_render() {
        let parsed = doc("abcdefghij", &[(1, "first page"), (2, "second page")]);
        let pages = parsed_pages(&parsed);
        let ctx = ctx();
        let out = open(
            &ctx,
            &parsed,
            &pages,
            "c:/docs/acme.pdf",
            Selection::Page(2),
        );
        assert!(out.text_for_model.contains("second page"));
        assert!(out.text_for_model.starts_with(UNTRUSTED_NOTICE));
        assert_eq!(out.summary_for_ui, "Read Acme MSA, page 2");
        let missing = render(&parsed, &pages, Selection::Page(9)).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "Page 9 not found; the document has pages 1 to 2"
        );
        let range = open(
            &ctx,
            &parsed,
            &pages,
            "c:/docs/acme.pdf",
            Selection::Range { start: 2, end: 5 },
        );
        assert!(range.text_for_model.contains("cde"));
        assert!(range.text_for_model.contains("continue with range start=5"));
        let unpaged = doc("plain", &[]);
        assert!(render(&unpaged, &[], Selection::Page(1)).is_err());
    }

    #[test]
    fn reads_are_numbered_after_earlier_search_passages() {
        let parsed = doc("", &[(1, "first page"), (2, "second page")]);
        let pages = parsed_pages(&parsed);
        let ctx = ctx();
        // A search earlier in the run numbered 1 to 3.
        assert_eq!(ctx.reserve_passages(3), 1);
        let out = open(
            &ctx,
            &parsed,
            &pages,
            "c:/docs/acme.pdf",
            Selection::Page(2),
        );
        assert!(out.text_for_model.contains("[4] acme.pdf, p. 2"));
        let passages = passages_of(&out);
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0]["n"], json!(4));
        assert_eq!(passages[0]["file"], json!("acme.pdf"));
        assert_eq!(passages[0]["path"], json!("c:/docs/acme.pdf"));
        assert_eq!(passages[0]["page"], json!("2"));
        assert_eq!(passages[0]["text"], json!("second page"));
        let cited = ctx.cited_passage(4).unwrap();
        assert_eq!(cited.file, "acme.pdf");
        assert_eq!(cited.page.as_deref(), Some("2"));
        assert!(!cited.web);
        assert_eq!(ctx.passages_issued(), 4);
    }

    #[test]
    fn rereading_a_span_reuses_its_number() {
        let parsed = doc("", &[(1, "first page"), (2, "second page")]);
        let pages = parsed_pages(&parsed);
        let ctx = ctx();
        let first = open(
            &ctx,
            &parsed,
            &pages,
            "c:/docs/acme.pdf",
            Selection::Page(2),
        );
        let again = open(
            &ctx,
            &parsed,
            &pages,
            "c:/docs/acme.pdf",
            Selection::Page(2),
        );
        assert_eq!(passages_of(&first)[0]["n"], json!(1));
        assert_eq!(passages_of(&again)[0]["n"], json!(1));
        let other = open(
            &ctx,
            &parsed,
            &pages,
            "c:/docs/acme.pdf",
            Selection::Page(1),
        );
        assert_eq!(passages_of(&other)[0]["n"], json!(2));
        assert_eq!(ctx.passages_issued(), 2);
    }

    #[test]
    fn an_answer_built_only_from_reads_resolves_every_citation() {
        use crate::harness::events::ClaimOutcome;
        use crate::harness::grounding::{
            verify_answer, AnswerMessage, Evidence, Scorers, VerifyInput, THRESHOLDS,
        };
        let parsed = doc(
            "",
            &[
                (
                    1,
                    "Either party may terminate the agreement with 60 days written notice.",
                ),
                (
                    2,
                    "The renewal fee is 900 EUR per year, payable in advance.",
                ),
            ],
        );
        let pages = parsed_pages(&parsed);
        let ctx = ctx();
        open(&ctx, &parsed, &pages, "c:/docs/msa.pdf", Selection::Page(1));
        open(&ctx, &parsed, &pages, "c:/docs/msa.pdf", Selection::Page(2));
        let evidence: Vec<Evidence> = (1..=ctx.passages_issued())
            .filter_map(|n| ctx.cited_passage(n))
            .map(|p| Evidence {
                n: p.n,
                path: p.path,
                text: p.text,
                checkable: p.checkable,
            })
            .collect();
        assert_eq!(evidence.len(), 2);
        let messages = [AnswerMessage {
            id: "m1".into(),
            text: "Either party may terminate the agreement with 60 days written notice [1]. \
                   The renewal fee is 900 EUR per year [2]."
                .into(),
        }];
        let (checks, _) = verify_answer(
            &VerifyInput {
                messages: &messages,
                passages: &evidence,
                opened: &[],
            },
            Scorers {
                relevance: None,
                entailment: None,
            },
            &THRESHOLDS,
        );
        assert_eq!(checks.len(), 2);
        for check in &checks {
            assert_ne!(check.outcome, ClaimOutcome::InvalidCitation, "{check:?}");
            assert_eq!(check.outcome, ClaimOutcome::Supported, "{check:?}");
        }
    }

    #[test]
    fn long_pages_are_split_into_bounded_passages_of_that_page() {
        let paragraph = |c: char| format!("{} end.", c.to_string().repeat(2_500));
        let page = [paragraph('a'), paragraph('b'), paragraph('c')].join("\n\n");
        let parsed = doc("", &[(7, page.as_str())]);
        let ctx = ctx();
        let out = open(
            &ctx,
            &parsed,
            &parsed_pages(&parsed),
            "c:/x.pdf",
            Selection::Page(7),
        );
        let passages = passages_of(&out);
        assert_eq!(passages.len(), 3);
        for (i, p) in passages.iter().enumerate() {
            assert_eq!(p["page"], json!("7"));
            assert_eq!(p["n"], json!(i + 1));
            assert!(p["text"].as_str().unwrap().chars().count() <= MAX_READ_PASSAGE_CHARS);
        }
    }

    #[test]
    fn layout_pages_carry_section_and_boxes() {
        use crate::processing::document_model::{
            BBox, Block, BlockKind, PageInfo, StructuredDocument,
        };
        let mut heading = Block::new(BlockKind::Heading { level: 1 }, "3 Method")
            .on_page(3, Some(BBox::new(72.0, 90.0, 300.0, 110.0)));
        heading.section_path = vec!["3 Method".to_string()];
        let mut body = Block::new(BlockKind::Paragraph, "We use the delta rule.")
            .on_page(3, Some(BBox::new(72.0, 120.0, 520.0, 160.0)));
        body.section_path = vec!["3 Method".to_string()];
        let mut parsed = doc("", &[]);
        parsed.document = Some(StructuredDocument {
            pages: vec![PageInfo {
                number: 3,
                width: 612.0,
                height: 792.0,
            }],
            blocks: vec![heading, body],
        });
        let ctx = ctx();
        let out = open(
            &ctx,
            &parsed,
            &parsed_pages(&parsed),
            "c:/p.pdf",
            Selection::Page(3),
        );
        assert!(out
            .text_for_model
            .contains("[1] p.pdf, p. 3 — 3 Method\n3 Method\n\nWe use the delta rule."));
        let passages = passages_of(&out);
        assert_eq!(passages[0]["section"], json!("3 Method"));
        let regions = passages[0]["regions"].as_array().unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[1]["page"], json!(3));
        assert_eq!(regions[1]["x0"], json!(72.0));
        assert_eq!(regions[1]["y1"], json!(160.0));
    }

    #[test]
    fn range_reads_find_their_pages_only_when_unambiguous() {
        let p1 = "Alpha section explains the setup of the experiment in detail.";
        let p2 = "Beta section reports the results of the experiment in detail.";
        let content = format!("{p1}\n\n{p2}");
        let parsed = doc(&content, &[(1, p1), (2, p2)]);
        let ctx = ctx();
        let out = open(
            &ctx,
            &parsed,
            &parsed_pages(&parsed),
            "c:/r.pdf",
            Selection::Beginning,
        );
        assert_eq!(passages_of(&out)[0]["page"], json!("1-2"));

        let normalized_pages = vec![(1, p1.to_string()), (2, p1.to_string())];
        assert_eq!(locate_pages(&normalized_pages, p1), None, "on two pages");
        assert_eq!(locate_pages(&normalized_pages, "too short"), None);
    }

    #[test]
    fn hard_splits_prefer_sentence_ends_and_stay_within_the_budget() {
        let text = format!("{}. {}", "a".repeat(60), "b".repeat(60));
        let pieces = split_hard(&text, 100);
        assert_eq!(pieces, vec![format!("{}.", "a".repeat(60)), "b".repeat(60)]);
        let unbroken = "x".repeat(250);
        let pieces = split_hard(&unbroken, 100);
        assert_eq!(pieces.len(), 3);
        assert!(pieces.iter().all(|p| p.chars().count() <= 100));
    }
}
