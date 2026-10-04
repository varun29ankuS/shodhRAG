//! `open_document`: the text of a page or character range of an indexed file.
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

use super::{req_str, HostTool, ToolContext, ToolError, ToolOutput, UNTRUSTED_NOTICE};
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

/// Render the selected part of a parsed document for the model. `pages`
/// are its page texts (from the parser, or read from the PDF directly).
fn render(
    parsed: &ParsedDocument,
    pages: &[(usize, String)],
    source: &str,
    selection: Selection,
) -> Result<(ToolOutput, String), ToolError> {
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
    let (body, location, next_hint) = match selection {
        Selection::Page(page) => {
            let text = page_text(pages, page)?;
            let (slice, total) = slice_chars(&text, 0, MAX_RANGE_CHARS);
            let hint = (total > MAX_RANGE_CHARS).then(|| {
                format!(
                    "[page truncated: {} more characters]",
                    total - MAX_RANGE_CHARS
                )
            });
            (slice, format!("page {page}"), hint)
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
                slice,
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
            (slice, format!("characters 0–{shown} of {total}"), hint)
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
    if body.trim().is_empty() {
        return Err(ToolError::NotFound(format!(
            "No text found at {location} of {}",
            parsed.title
        )));
    }
    let mut text = format!(
        "{UNTRUSTED_NOTICE}\nDocument: {} ({source}), {location}\n\n{body}",
        parsed.title
    );
    if let Some(hint) = next_hint {
        text.push('\n');
        text.push_str(&hint);
    }
    Ok((
        ToolOutput {
            text_for_model: text,
            summary_for_ui: format!("Read {}, {location}", parsed.title),
            detail: Some(json!({ "path": source, "location": location })),
        },
        body,
    ))
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
        let (output, body) = render(&parsed, &pages, &source, selection)?;
        // Claims citing a passage of this file are also checked against
        // what the model read here.
        ctx.record_opened(&source, &body);
        Ok(output)
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

    #[test]
    fn page_and_range_together_use_the_page_when_there_is_page_text() {
        let paged = doc("abcdefghij", &[(1, "first page"), (2, "second page")]);
        let both = Selection::PageOrRange {
            page: 2,
            start: 0,
            end: 3,
        };
        let (out, body) = render(&paged, &parsed_pages(&paged), "c:/docs/acme.pdf", both).unwrap();
        assert_eq!(body, "second page", "the text recorded for checking claims");
        assert!(out.text_for_model.contains("second page"));
        assert!(out
            .summary_for_ui
            .contains("page 2 (used the page; the range was ignored)"));

        let unpaged = doc("abcdefghij", &[]);
        let (out, _) = render(&unpaged, &[], "c:/docs/scan.pdf", both).unwrap();
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
        let (out, _) = render(&unpaged, &from_file, "c:/docs/a.pdf", Selection::Page(2)).unwrap();
        assert!(out.text_for_model.contains("terms"));
        assert!(pdf_pages_from_file(Path::new("c:/definitely/missing.pdf")).is_none());
    }

    #[test]
    fn pages_and_ranges_render() {
        let parsed = doc("abcdefghij", &[(1, "first page"), (2, "second page")]);
        let pages = parsed_pages(&parsed);
        let (out, _) = render(&parsed, &pages, "c:/docs/acme.pdf", Selection::Page(2)).unwrap();
        assert!(out.text_for_model.contains("second page"));
        assert!(out.text_for_model.starts_with(UNTRUSTED_NOTICE));
        assert_eq!(out.summary_for_ui, "Read Acme MSA, page 2");
        let missing = render(&parsed, &pages, "c:/docs/acme.pdf", Selection::Page(9)).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "Page 9 not found; the document has pages 1 to 2"
        );
        let (range, _) = render(
            &parsed,
            &pages,
            "c:/docs/acme.pdf",
            Selection::Range { start: 2, end: 5 },
        )
        .unwrap();
        assert!(range.text_for_model.contains("cde"));
        assert!(range.text_for_model.contains("continue with range start=5"));
        let unpaged = doc("plain", &[]);
        assert!(render(&unpaged, &[], "a.txt", Selection::Page(1)).is_err());
    }
}
