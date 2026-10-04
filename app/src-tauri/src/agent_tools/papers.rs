//! Research tools: `list_snippets` (read), `create_snippet` (write) and `query_results`
//! (read).
//!
//! Snippets are saved regions of PDF pages (image, text, file, page, rectangle); results
//! are Result statements read from paper tables (see `shodh_rag::research`). Both read
//! tools number what they return as citable passages, so the answer check verifies every
//! claim — and every number of a comparison — against the snippet or table cell it cites.
//! Editing, deleting and reviewing snippets and results is the user's and has no tool.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    ApprovalPreview, CitedPassage, HostTool, RegistryError, ToolContext, ToolError, ToolOutput,
    ToolRegistry, UNTRUSTED_NOTICE,
};
use shodh_rag::harness::RiskTier;
use shodh_rag::research::pdf_text::{text_in_rect, PageRect};
use shodh_rag::research::results::{Comparison, ComparisonCell};
use shodh_rag::research::snippets::{
    NewSnippet, Snippet, SnippetAuthor, SnippetKind, SnippetQuery,
};
use shodh_rag::statements::Scope;

use super::sources::strip_verbatim;
use super::{invalid, limit_arg, str_arg, AgentHost};
use crate::research_commands::{indexed_pdfs, scopes_for, ResearchCommandError, ResultFilterInput};

const DEFAULT_SNIPPETS: usize = 10;
const MAX_SNIPPETS: usize = 50;
/// Characters of snippet text returned per snippet in a listing.
const SNIPPET_TEXT_CHARS: usize = 1_200;
const DEFAULT_ROWS: usize = 30;
const MAX_ROWS: usize = 100;
/// Most cited cells in one comparison (each becomes a numbered passage).
const MAX_CELLS: usize = 300;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(ListSnippetsTool { host: host.clone() }))?;
    registry.register(Arc::new(CreateSnippetTool { host: host.clone() }))?;
    registry.register(Arc::new(QueryResultsTool { host: host.clone() }))?;
    Ok(())
}

fn tool_error(error: ResearchCommandError) -> ToolError {
    match error.code {
        "not_found" => ToolError::NotFound(error.message),
        "invalid" => ToolError::Failed(error.message),
        _ => ToolError::Failed(error.message),
    }
}

fn research_error(error: shodh_rag::research::ResearchError) -> ToolError {
    tool_error(error.into())
}

/// The scopes a run sees: its workspace and global ones, or everything in a global
/// conversation (snippets and results are the user's own documents).
fn run_scopes(ctx: &ToolContext) -> Vec<Scope> {
    scopes_for(ctx.scope().workspace.as_deref())
}

fn truncate(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    (out, true)
}

fn snippet_json(s: &Snippet, n: u32, text: &str, truncated: bool) -> Value {
    json!({
        "n": n,
        "id": s.id,
        "title": s.title,
        "kind": s.kind.as_str(),
        "file": s.file_name,
        "path": s.file_path,
        "page": s.page,
        "rect": s.rect,
        "text": text,
        "textTruncated": truncated,
        "note": s.note,
        "tags": s.tags,
        "latex": s.latex,
        "hasImage": s.has_image,
    })
}

pub struct ListSnippetsTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ListSnippetsTool {
    fn name(&self) -> &'static str {
        app_tools::LIST_SNIPPETS
    }
    fn label(&self) -> &'static str {
        "List snippets"
    }
    fn label_template(&self) -> &'static str {
        "Looking through saved snippets[ for {query}]"
    }
    fn description(&self) -> &'static str {
        "Find the user's saved snippets: regions of PDF pages (figures, tables, equations, \
         passages) with their text, file, page and rectangle. Search by meaning or words in \
         their title, text and note (`query`), limit to one file (`file`), or give \
         `snippet_id` for one snippet with its full text. Each snippet is numbered n like a \
         search passage: cite it with [n]."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "snippet_id": {"type": "string", "minLength": 1, "maxLength": 200},
                "query": {"type": "string", "minLength": 1, "maxLength": 300},
                "file": {"type": "string", "minLength": 1, "maxLength": 1000},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_SNIPPETS}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let services = self.host.research.services().await.map_err(tool_error)?;
        let snippets: Vec<Snippet> = match str_arg(&args, "snippet_id") {
            Some(id) => vec![services.snippets.get(id).await.map_err(research_error)?],
            None => services
                .snippets
                .list(&SnippetQuery {
                    file_path: str_arg(&args, "file").map(str::to_string),
                    text: str_arg(&args, "query").map(str::to_string),
                    scopes: run_scopes(ctx),
                    limit: Some(limit_arg(&args, DEFAULT_SNIPPETS, MAX_SNIPPETS)),
                })
                .await
                .map_err(research_error)?,
        };
        if snippets.is_empty() {
            return Ok(ToolOutput {
                text_for_model: "No saved snippets match. The user saves snippets from the PDF \
                                 viewer; create_snippet saves one from a file, page and rectangle."
                    .to_string(),
                summary_for_ui: "No matching snippets".to_string(),
                detail: Some(json!({ "passages": [] })),
            });
        }
        let full = args.get("snippet_id").is_some();
        let max = if full { 20_000 } else { SNIPPET_TEXT_CHARS };
        let first = ctx.reserve_passages(u32::try_from(snippets.len()).unwrap_or(u32::MAX));
        let mut listed = Vec::new();
        let mut passages = Vec::new();
        for (s, n) in snippets.iter().zip(first..) {
            let body = if s.text.trim().is_empty() {
                s.latex.clone().unwrap_or_default()
            } else {
                s.text.clone()
            };
            let (text, truncated) = truncate(&body, max);
            let passage_text = format!(
                "{}{}",
                if s.title.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", s.title)
                },
                text
            );
            ctx.record_passage(CitedPassage {
                n,
                file: s.file_name.clone(),
                path: s.file_path.clone(),
                page: Some(s.page.to_string()),
                web: false,
                text: passage_text.clone(),
                checkable: true,
            });
            listed.push(snippet_json(s, n, &text, truncated));
            passages.push(json!({
                "n": n,
                "file": s.file_name,
                "path": s.file_path,
                "page": s.page.to_string(),
                "score": 1.0,
                "text": passage_text,
            }));
        }
        let noun = if snippets.len() == 1 {
            "snippet"
        } else {
            "snippets"
        };
        Ok(ToolOutput {
            text_for_model: format!(
                "{UNTRUSTED_NOTICE}\n{}",
                serde_json::to_string(&listed)
                    .map_err(|e| ToolError::Failed(format!("Could not encode snippets: {e}")))?
            ),
            summary_for_ui: format!("{} {noun}", snippets.len()),
            detail: Some(json!({ "passages": passages, "snippets": listed })),
        })
    }
}

pub struct CreateSnippetTool {
    host: Arc<AgentHost>,
}

struct SnippetArgs {
    file: String,
    page: u32,
    rect: PageRect,
    title: Option<String>,
    note: Option<String>,
    kind: SnippetKind,
    tags: Vec<String>,
}

fn snippet_args(args: &Value) -> Result<SnippetArgs, ToolError> {
    let tool = app_tools::CREATE_SNIPPET;
    let file = str_arg(args, "file")
        .ok_or_else(|| invalid(tool, "`file` is required"))?
        .to_string();
    let page = args
        .get("page")
        .and_then(Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p >= 1)
        .ok_or_else(|| invalid(tool, "`page` (1-based) is required"))?;
    let rect: PageRect = args
        .get("rect")
        .cloned()
        .ok_or_else(|| invalid(tool, "`rect` is required"))
        .and_then(|r| {
            serde_json::from_value(r).map_err(|e| invalid(tool, format!("`rect`: {e}")))
        })?;
    rect.validate().map_err(|e| invalid(tool, e.to_string()))?;
    let kind = match str_arg(args, "kind") {
        Some(k) => {
            SnippetKind::parse(k).ok_or_else(|| invalid(tool, format!("unknown kind {k}")))?
        }
        None => SnippetKind::default(),
    };
    let tags = args
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(SnippetArgs {
        file,
        page,
        rect,
        title: str_arg(args, "title").map(str::to_string),
        note: str_arg(args, "note").map(str::to_string),
        kind,
        tags,
    })
}

impl CreateSnippetTool {
    /// The indexed PDF `requested` names (a full path, or a file name that is unique), in
    /// its on-disk spelling.
    async fn resolve(&self, requested: &str) -> Result<String, ToolError> {
        let matches = {
            let rag = self.host.rag.read().await;
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
        if source.contains("://") || !source.to_ascii_lowercase().ends_with(".pdf") {
            return Err(ToolError::Forbidden(
                "Snippets are taken from indexed PDF files.".to_string(),
            ));
        }
        Ok(std::fs::canonicalize(Path::new(&source))
            .map(|p| strip_verbatim(&p).display().to_string())
            .unwrap_or(source))
    }
}

#[async_trait]
impl HostTool for CreateSnippetTool {
    fn name(&self) -> &'static str {
        app_tools::CREATE_SNIPPET
    }
    fn label(&self) -> &'static str {
        "Save snippet"
    }
    fn label_template(&self) -> &'static str {
        "Saving a snippet from {file}[, page {page}]"
    }
    fn description(&self) -> &'static str {
        "Save a region of an indexed PDF page as a snippet in the user's Library: `file` (a \
         path from search_documents), `page` (1-based) and `rect` in PDF points from the \
         page's top-left corner ({x, y, width, height}; search passages carry boxes you can \
         convert). The text inside the region is read from the PDF; the image is rendered \
         when the user first opens it. Optional `title`, `note`, `kind` (figure, table, \
         equation, passage) and `tags`. Only when the user asks to save or clip something."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {"type": "string", "minLength": 1, "maxLength": 1000},
                "page": {"type": "integer", "minimum": 1, "maximum": 100000},
                "rect": {
                    "type": "object",
                    "properties": {
                        "x": {"type": "number", "minimum": 0},
                        "y": {"type": "number", "minimum": 0},
                        "width": {"type": "number", "minimum": 1},
                        "height": {"type": "number", "minimum": 1}
                    },
                    "required": ["x", "y", "width", "height"],
                    "additionalProperties": false
                },
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "note": {"type": "string", "minLength": 1, "maxLength": 4000},
                "kind": {"type": "string", "enum": ["figure", "table", "equation", "passage"]},
                "tags": {"type": "array", "items": {"type": "string", "minLength": 1, "maxLength": 60}, "maxItems": 20}
            },
            "required": ["file", "page", "rect"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let a = snippet_args(args)?;
        let path = self.resolve(&a.file).await?;
        let name = shodh_rag::research::file_name(&path);
        Ok(ApprovalPreview {
            label: Some(format!("Save a snippet of {name}, page {}", a.page)),
            details: json!({
                "file": path,
                "page": a.page,
                "rect": a.rect,
                "title": a.title,
                "kind": a.kind.as_str(),
            }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let a = snippet_args(&args)?;
        let path = self.resolve(&a.file).await?;
        let read_path = path.clone();
        let (page, rect) = (a.page, a.rect);
        let region = tokio::task::spawn_blocking(move || {
            let bytes = std::fs::read(&read_path)
                .map_err(|e| ToolError::Failed(format!("The PDF could not be read: {e}")))?;
            text_in_rect(bytes, page, rect).map_err(research_error)
        })
        .await
        .map_err(|e| ToolError::Failed(format!("Reading the PDF failed: {e}")))??;
        let model = self
            .host
            .effects
            .model_info()
            .and_then(|m| m.model)
            .unwrap_or_else(|| "assistant".to_string());
        let services = self.host.research.services().await.map_err(tool_error)?;
        let snippet = services
            .snippets
            .create(NewSnippet {
                file_path: path,
                page: a.page,
                rect: a.rect,
                text: region.text,
                image_png: None,
                title: a.title,
                note: a.note,
                tags: a.tags,
                kind: a.kind,
                scope: Scope::for_workspace(ctx.scope().workspace.as_deref()),
                author: SnippetAuthor::Agent { model },
            })
            .await
            .map_err(research_error)?;
        self.host
            .effects
            .research_changed("snippet", &snippet.file_path);
        let (text, _) = truncate(&snippet.text, SNIPPET_TEXT_CHARS);
        Ok(ToolOutput {
            text_for_model: format!(
                "Saved snippet {} from {} page {}. Its image is rendered the first time the user \
                 opens it. Text inside the region ({} characters): {}",
                snippet.id,
                snippet.file_name,
                snippet.page,
                snippet.text.chars().count(),
                if text.is_empty() {
                    "(none: the region holds no text)".to_string()
                } else {
                    text
                }
            ),
            summary_for_ui: format!(
                "Saved a snippet of {}, page {}",
                snippet.file_name, snippet.page
            ),
            detail: Some(
                json!({ "snippetId": snippet.id, "file": snippet.file_path, "page": snippet.page }),
            ),
        })
    }
}

pub struct QueryResultsTool {
    host: Arc<AgentHost>,
}

fn cell_line(method: &str, dataset: &str, metric: &str, cell: &ComparisonCell) -> String {
    let mut text = format!(
        "{method} — {metric} on {dataset}: {}{}",
        cell.value_text,
        cell.unit
            .as_deref()
            .filter(|u| !cell.value_text.contains(*u))
            .map(|u| format!(" {u}"))
            .unwrap_or_default()
    );
    if let Some(setting) = cell.setting.as_deref().filter(|s| !s.is_empty()) {
        text.push_str(&format!(" ({setting})"));
    }
    text.push_str(&format!(", {} page {}", cell.file_name, cell.page));
    text
}

/// The comparison as text for the model, with one numbered passage per cell.
fn render_comparison(
    comparison: &Comparison,
    rows: usize,
    ctx: &ToolContext,
) -> (String, Vec<Value>) {
    let mut cells: Vec<(&str, &str, &str, &ComparisonCell)> = Vec::new();
    for row in comparison.rows.iter().take(rows) {
        for column in &comparison.columns {
            for cell in row.cells.get(&column.key).into_iter().flatten() {
                if cells.len() < MAX_CELLS {
                    cells.push((&row.method, &column.dataset, &column.metric, cell));
                }
            }
        }
    }
    let first = ctx.reserve_passages(u32::try_from(cells.len()).unwrap_or(u32::MAX));
    let mut lines = Vec::new();
    let mut passages = Vec::new();
    for ((method, dataset, metric, cell), n) in cells.into_iter().zip(first..) {
        let text = cell_line(method, dataset, metric, cell);
        ctx.record_passage(CitedPassage {
            n,
            file: cell.file_name.clone(),
            path: cell.file_path.clone(),
            page: Some(cell.page.to_string()),
            web: false,
            text: text.clone(),
            checkable: true,
        });
        lines.push(format!("[{n}] {text}"));
        let mut passage = json!({
            "n": n,
            "file": cell.file_name,
            "path": cell.file_path,
            "page": cell.page.to_string(),
            "score": 1.0,
            "text": text,
        });
        if let (Some(region), Some(map)) = (cell.region, passage.as_object_mut()) {
            map.insert(
                "regions".to_string(),
                json!([{ "page": region.page, "x0": region.x0, "y0": region.y0, "x1": region.x1, "y1": region.y1 }]),
            );
        }
        passages.push(passage);
    }
    let mut text = String::new();
    if lines.is_empty() {
        text.push_str("No accepted results match.\n");
    } else {
        text.push_str("Values (each from one table cell; cite each number with its [n]):\n");
        text.push_str(&lines.join("\n"));
        text.push('\n');
    }
    if !comparison.notes.is_empty() {
        text.push_str("Coverage:\n");
        for note in &comparison.notes {
            text.push_str(&format!("- {note}\n"));
        }
    }
    if comparison.rows.len() > rows {
        text.push_str(&format!(
            "- {} more methods are not shown; narrow the filters.\n",
            comparison.rows.len() - rows
        ));
    }
    (text, passages)
}

#[async_trait]
impl HostTool for QueryResultsTool {
    fn name(&self) -> &'static str {
        app_tools::QUERY_RESULTS
    }
    fn label(&self) -> &'static str {
        "Compare results"
    }
    fn label_template(&self) -> &'static str {
        "Comparing reported results[ for {metric}][ on {dataset}]"
    }
    fn description(&self) -> &'static str {
        "Compare results reported in the user's papers, read from their tables: filter by \
         `method`, `dataset`, `metric` (names as printed, e.g. \"R@10\", \"SIFT1M\") and \
         `papers` (file paths). Returns every matching value with its paper, page and table \
         cell, numbered n — cite each number with its [n] — and coverage notes (which papers \
         report the metric, which report nothing comparable, which were not scanned, which \
         values await the user's review). Render the values as a Markdown table or a chart \
         using only these numbers; values awaiting review are never included."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": {"type": "string", "minLength": 1, "maxLength": 200},
                "dataset": {"type": "string", "minLength": 1, "maxLength": 200},
                "metric": {"type": "string", "minLength": 1, "maxLength": 200},
                "papers": {"type": "array", "items": {"type": "string", "minLength": 1, "maxLength": 1000}, "maxItems": 50},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_ROWS}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let papers: Option<Vec<String>> = args.get("papers").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });
        let known = indexed_pdfs(&self.host.rag).await;
        let mut filter = ResultFilterInput {
            method: str_arg(&args, "method").map(str::to_string),
            dataset: str_arg(&args, "dataset").map(str::to_string),
            metric: str_arg(&args, "metric").map(str::to_string),
            papers,
            workspace: None,
        }
        .into_filter(known);
        filter.scopes = run_scopes(ctx);
        let services = self.host.research.services().await.map_err(tool_error)?;
        let comparison = services
            .results
            .query(&filter)
            .await
            .map_err(research_error)?;
        let rows = limit_arg(&args, DEFAULT_ROWS, MAX_ROWS);
        let (body, passages) = render_comparison(&comparison, rows, ctx);
        let summary = if comparison.rows.is_empty() {
            "No matching results".to_string()
        } else {
            format!(
                "{} {} × {} {} from {} {}",
                comparison.rows.len(),
                if comparison.rows.len() == 1 {
                    "method"
                } else {
                    "methods"
                },
                comparison.columns.len(),
                if comparison.columns.len() == 1 {
                    "column"
                } else {
                    "columns"
                },
                comparison.papers.len(),
                if comparison.papers.len() == 1 {
                    "paper"
                } else {
                    "papers"
                },
            )
        };
        Ok(ToolOutput {
            text_for_model: format!("{UNTRUSTED_NOTICE}\n{body}"),
            summary_for_ui: summary,
            detail: Some(json!({
                "passages": passages,
                "notes": comparison.notes,
                "pendingReview": comparison.pending_review,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_tools::testing;
    use shodh_rag::harness::tools::HostTool;
    use shodh_rag::processing::document_model::{
        BBox, Block, BlockKind, PageInfo, StructuredDocument,
    };

    fn table_doc() -> StructuredDocument {
        let b = |x0: f32, y0: f32, x1: f32, y1: f32| Some(BBox::new(x0, y0, x1, y1));
        StructuredDocument {
            pages: vec![PageInfo {
                number: 6,
                width: 612.0,
                height: 792.0,
            }],
            blocks: vec![Block::new(
                BlockKind::Table {
                    header: vec!["Method".into(), "SIFT1M R@10".into()],
                    rows: vec![
                        vec!["HNSW".into(), "95.3".into()],
                        vec!["IVF-PQ".into(), "71.4".into()],
                    ],
                    caption: Some("Table 2: Recall.".into()),
                    cell_boxes: vec![
                        vec![b(100.0, 700.0, 150.0, 710.0), b(160.0, 700.0, 220.0, 710.0)],
                        vec![b(100.0, 688.0, 150.0, 698.0), b(160.0, 688.0, 220.0, 698.0)],
                        vec![b(100.0, 676.0, 150.0, 686.0), b(160.0, 676.0, 220.0, 686.0)],
                    ],
                },
                "",
            )
            .on_page(6, b(100.0, 676.0, 220.0, 710.0))],
        }
    }

    #[tokio::test]
    async fn query_results_numbers_every_cell_and_keeps_its_box() {
        let t = testing::host().await;
        let services = t.host.research.services().await.unwrap();
        services
            .results
            .extract_document("C:/papers/ann.pdf", &table_doc(), Scope::Global, None)
            .await
            .unwrap();
        let tool = QueryResultsTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let out = tool
            .execute(json!({ "metric": "recall@10", "dataset": "SIFT1M" }), &ctx)
            .await
            .unwrap();
        assert!(out
            .text_for_model
            .contains("[1] HNSW — R@10 on SIFT1M: 95.3 (Table 2), ann.pdf page 6"));
        assert!(out
            .text_for_model
            .contains("[2] IVF-PQ — R@10 on SIFT1M: 71.4"));
        assert!(out
            .text_for_model
            .contains("1 paper reports R@10 on SIFT1M."));
        let detail = out.detail.unwrap();
        assert_eq!(detail["passages"][0]["regions"][0]["x0"], json!(160.0));
        assert_eq!(detail["passages"][0]["page"], json!("6"));
        assert_eq!(ctx.passages_issued(), 2);
        assert_eq!(out.summary_for_ui, "2 methods × 1 column from 1 paper");
    }

    #[tokio::test]
    async fn snippets_are_listed_as_citable_passages_and_created_only_from_indexed_pdfs() {
        let t = testing::host().await;
        let services = t.host.research.services().await.unwrap();
        let pdf = t.dir.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        services
            .snippets
            .create(NewSnippet {
                file_path: pdf.display().to_string(),
                page: 2,
                rect: PageRect {
                    x: 10.0,
                    y: 20.0,
                    width: 100.0,
                    height: 30.0,
                },
                text: "The encoder stacks six identical layers".into(),
                image_png: None,
                title: Some("Encoder".into()),
                note: None,
                tags: vec![],
                kind: SnippetKind::Passage,
                scope: Scope::Global,
                author: SnippetAuthor::User,
            })
            .await
            .unwrap();
        let list = ListSnippetsTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let out = list
            .execute(json!({ "query": "encoder layers" }), &ctx)
            .await
            .unwrap();
        assert!(out.text_for_model.contains("six identical layers"));
        assert_eq!(
            out.detail.unwrap()["passages"][0]["text"],
            json!("Encoder: The encoder stacks six identical layers")
        );
        assert_eq!(ctx.passages_issued(), 1);

        let create = CreateSnippetTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let err = create
            .execute(
                json!({ "file": "missing.pdf", "page": 1, "rect": { "x": 0, "y": 0, "width": 10, "height": 10 } }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
        let err = create
            .execute(json!({ "file": "a.pdf", "page": 0, "rect": { "x": 0, "y": 0, "width": 10, "height": 10 } }), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
        assert_eq!(create.tier(), RiskTier::Write);
        assert_eq!(list.tier(), RiskTier::Read);
    }
}
