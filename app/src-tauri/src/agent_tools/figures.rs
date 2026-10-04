//! Paper object tools (both read): `show_figure` (a paper's figures by number or caption
//! words, with what an answer needs to show the original figure) and `get_equation` (its
//! display equations by number or symbols, with LaTeX and where it came from).
//!
//! Both number what they return as passages of the paper: a figure's passage is its
//! caption, an equation's is its text as printed, and each carries its box on the page, so
//! a citation pill opens the PDF at the figure or equation and the answer check verifies
//! claims against the paper's own words.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    CitedPassage, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
    UNTRUSTED_NOTICE,
};
use shodh_rag::harness::RiskTier;
use shodh_rag::processing::document_model::BBox;
use shodh_rag::research::equations::{find_equations, Equation, EquationOrigin};
use shodh_rag::research::figures::{find_figures, Figure};
use shodh_rag::research::objects::PaperParts;

use super::{invalid, str_arg, AgentHost};
use crate::research_commands::{indexed_pdf, ResearchCommandError};

/// Most figures or equations returned for one query, and in a listing.
const MAX_MATCHES: usize = 5;
const MAX_LISTED: usize = 40;
/// Characters of a caption or equation text in a listing line.
const LINE_CHARS: usize = 220;
const MENTION_CHARS: usize = 400;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(ShowFigureTool { host: host.clone() }))?;
    registry.register(Arc::new(GetEquationTool { host: host.clone() }))?;
    Ok(())
}

fn tool_error(error: ResearchCommandError) -> ToolError {
    match error.code {
        "not_found" => ToolError::NotFound(error.message),
        _ => ToolError::Failed(error.message),
    }
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The indexed PDF a `paper` argument names: a path or unique file name in the index, or
/// a title, arXiv id or DOI of a library paper in the citation graph.
async fn paper_pdf(host: &AgentHost, tool: &str, paper: &str) -> Result<String, ToolError> {
    match indexed_pdf(&host.rag, paper).await {
        Ok(path) => return Ok(path),
        Err(e) if e.code != "not_found" => return Err(tool_error(e)),
        Err(_) => {}
    }
    let from_graph = match host.research.services().await {
        Ok(services) => services
            .citations
            .graph(&services.results)
            .await
            .ok()
            .and_then(|graph| {
                graph
                    .find(paper)
                    .filter(|p| p.in_library)
                    .and_then(|p| p.file_path.clone())
            }),
        Err(_) => None,
    };
    match from_graph {
        Some(file) => indexed_pdf(&host.rag, &file).await.map_err(tool_error),
        None => Err(ToolError::NotFound(format!(
            "{tool}: \"{paper}\" is not an indexed PDF or a library paper. Pass a file path from \
             search_documents, or the paper's title."
        ))),
    }
}

async fn parts_of(host: &AgentHost, tool: &str, paper: &str) -> Result<Arc<PaperParts>, ToolError> {
    let pdf = paper_pdf(host, tool, paper).await?;
    host.research
        .paper_parts(&host.rag, &pdf)
        .await
        .map_err(tool_error)
}

fn region(page: u32, b: &BBox) -> Value {
    json!([{ "page": page, "x0": b.x0, "y0": b.y0, "x1": b.x1, "y1": b.y1 }])
}

/// Numbers one paper object as a passage with its box.
fn cite(
    ctx: &ToolContext,
    parts: &PaperParts,
    page: Option<u32>,
    bbox: Option<&BBox>,
    text: &str,
    passages: &mut Vec<Value>,
) -> u32 {
    let n = ctx.reserve_passages(1);
    ctx.record_passage(CitedPassage {
        n,
        file: parts.file_name.clone(),
        path: parts.file_path.clone(),
        page: page.map(|p| p.to_string()),
        web: false,
        text: text.to_string(),
        checkable: true,
    });
    let mut passage = json!({
        "n": n,
        "file": parts.file_name,
        "path": parts.file_path,
        "page": page.map(|p| p.to_string()),
        "score": 1.0,
        "text": text,
    });
    if let (Some(page), Some(b), Some(map)) = (page, bbox, passage.as_object_mut()) {
        map.insert("regions".to_string(), region(page, b));
    }
    passages.push(passage);
    n
}

/// The ```figure block that shows `figure` in an answer.
pub fn figure_block(parts: &PaperParts, figure: &Figure) -> String {
    let b = figure.bbox;
    let spec = json!({
        "paper": parts.file_path,
        "figureId": figure.id,
        "page": figure.page,
        "bbox": [b.x0, b.y0, b.x1, b.y1],
        "caption": clip(&figure.caption, 300),
    });
    format!("```figure\n{spec}\n```")
}

/// What `show_figure` returns for `query` (all figures when `None`).
pub fn figure_output(ctx: &ToolContext, parts: &PaperParts, query: Option<&str>) -> ToolOutput {
    if parts.figures.is_empty() {
        return ToolOutput {
            text_for_model: format!(
                "No figure captions were found in {}. Its figures may be unlabelled or the PDF \
                 a scan; describe what the text says instead.",
                parts.file_name
            ),
            summary_for_ui: format!("No figures found in {}", parts.file_name),
            detail: Some(json!({ "passages": [], "figures": [] })),
        };
    }
    let Some(query) = query else {
        let mut lines = vec![format!(
            "{} figures in {} (call again with `figure` for one figure and its block):",
            parts.figures.len(),
            parts.file_name
        )];
        for f in parts.figures.iter().take(MAX_LISTED) {
            lines.push(format!(
                "- {} (page {}): {}",
                f.id,
                f.page,
                clip(&f.caption, LINE_CHARS)
            ));
        }
        return ToolOutput {
            text_for_model: format!("{UNTRUSTED_NOTICE}\n{}", lines.join("\n")),
            summary_for_ui: format!("{} figures in {}", parts.figures.len(), parts.file_name),
            detail: Some(json!({ "passages": [], "figures": parts.figures })),
        };
    };
    let found: Vec<&Figure> = find_figures(&parts.figures, query)
        .into_iter()
        .take(MAX_MATCHES)
        .collect();
    if found.is_empty() {
        let known: Vec<String> = parts.figures.iter().map(|f| f.label.clone()).collect();
        return ToolOutput {
            text_for_model: format!(
                "No figure of {} matches \"{query}\". Its figures: {}.",
                parts.file_name,
                known.join(", ")
            ),
            summary_for_ui: format!("No matching figure in {}", parts.file_name),
            detail: Some(json!({ "passages": [], "figures": [] })),
        };
    }
    let mut passages = Vec::new();
    let mut lines = vec![UNTRUSTED_NOTICE.to_string()];
    for f in &found {
        let n = cite(
            ctx,
            parts,
            Some(f.page),
            Some(&f.bbox),
            &clip(&f.caption, 2_000),
            &mut passages,
        );
        lines.push(format!(
            "[{n}] {} — {}, page {}: {}",
            f.id,
            f.label,
            f.page,
            clip(&f.caption, 1_000)
        ));
        for m in &f.mentions {
            lines.push(format!("  Text near it: {}", clip(m, MENTION_CHARS)));
        }
        if f.region_found {
            lines.push(format!("  Show it with:\n{}", figure_block(parts, f)));
        } else {
            lines.push(
                "  Its drawing area could not be located on the page; cite the caption and \
                 describe the figure instead of showing it."
                    .to_string(),
            );
        }
    }
    let first = found[0];
    ToolOutput {
        text_for_model: lines.join("\n"),
        summary_for_ui: if found.len() == 1 {
            format!(
                "{} of {}, page {}",
                first.label, parts.file_name, first.page
            )
        } else {
            format!("{} figures of {}", found.len(), parts.file_name)
        },
        detail: Some(json!({ "passages": passages, "figures": found })),
    }
}

fn origin_note(e: &Equation) -> &'static str {
    match e.origin {
        EquationOrigin::Source => "from the paper's LaTeX source",
        EquationOrigin::Reconstructed => {
            "reconstructed from the PDF text (fractions and sub/superscript placement may be lost; check it against the text)"
        }
    }
}

/// What `get_equation` returns for `query` (all equations when `None`).
pub fn equation_output(ctx: &ToolContext, parts: &PaperParts, query: Option<&str>) -> ToolOutput {
    if parts.equations.is_empty() {
        return ToolOutput {
            text_for_model: format!("No display equations were found in {}.", parts.file_name),
            summary_for_ui: format!("No equations found in {}", parts.file_name),
            detail: Some(json!({ "passages": [], "equations": [] })),
        };
    }
    let Some(query) = query else {
        let mut lines = vec![format!(
            "{} display equations in {}{} (call again with `equation` for one, numbered to cite):",
            parts.equations.len(),
            parts.file_name,
            parts
                .tex_source
                .as_deref()
                .map(|t| format!(", LaTeX matched to {}", shodh_rag::research::file_name(t)))
                .unwrap_or_default()
        )];
        for e in parts.equations.iter().take(MAX_LISTED) {
            lines.push(format!(
                "- {}{}{}: {}",
                e.id,
                e.number
                    .as_deref()
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default(),
                e.page.map(|p| format!(", page {p}")).unwrap_or_default(),
                clip(&e.latex, LINE_CHARS)
            ));
        }
        return ToolOutput {
            text_for_model: format!("{UNTRUSTED_NOTICE}\n{}", lines.join("\n")),
            summary_for_ui: format!("{} equations in {}", parts.equations.len(), parts.file_name),
            detail: Some(json!({ "passages": [], "equations": parts.equations })),
        };
    };
    let found: Vec<&Equation> = find_equations(&parts.equations, query)
        .into_iter()
        .take(MAX_MATCHES)
        .collect();
    if found.is_empty() {
        let numbered: Vec<String> = parts
            .equations
            .iter()
            .filter_map(|e| e.number.clone())
            .collect();
        return ToolOutput {
            text_for_model: format!(
                "No equation of {} matches \"{query}\". Numbered equations: {}.",
                parts.file_name,
                if numbered.is_empty() {
                    "none".to_string()
                } else {
                    numbered.join(", ")
                }
            ),
            summary_for_ui: format!("No matching equation in {}", parts.file_name),
            detail: Some(json!({ "passages": [], "equations": [] })),
        };
    }
    let mut passages = Vec::new();
    let mut lines = vec![UNTRUSTED_NOTICE.to_string()];
    for e in &found {
        let label = e
            .number
            .as_deref()
            .map_or_else(|| "Equation".to_string(), |n| format!("Equation ({n})"));
        let n = cite(
            ctx,
            parts,
            e.page,
            e.bbox.as_ref(),
            &e.passage(),
            &mut passages,
        );
        lines.push(format!(
            "[{n}] {} — {label}{}, LaTeX {}:\n$$\n{}\n$$",
            e.id,
            e.page.map(|p| format!(", page {p}")).unwrap_or_default(),
            origin_note(e),
            e.latex
        ));
    }
    lines.push("Cite an equation with its [n]; the pill opens its box in the PDF.".to_string());
    let first = found[0];
    ToolOutput {
        text_for_model: lines.join("\n"),
        summary_for_ui: if found.len() == 1 {
            format!(
                "{} of {}",
                first
                    .number
                    .as_deref()
                    .map_or_else(|| "An equation".to_string(), |n| format!("Equation ({n})")),
                parts.file_name
            )
        } else {
            format!("{} equations of {}", found.len(), parts.file_name)
        },
        detail: Some(json!({ "passages": passages, "equations": found })),
    }
}

pub struct ShowFigureTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ShowFigureTool {
    fn name(&self) -> &'static str {
        app_tools::SHOW_FIGURE
    }
    fn label(&self) -> &'static str {
        "Find figure"
    }
    fn label_template(&self) -> &'static str {
        "Finding[ {figure} in] {paper}"
    }
    fn description(&self) -> &'static str {
        "Find a figure of an indexed paper (`paper`: file path, file name or title) by number \
         or caption words (`figure`, e.g. \"3\" or \"attention heatmap\"); without `figure`, \
         list the paper's figures. Each match is numbered n (its caption, cite with [n]) and \
         comes with the text that refers to it and a ready ```figure block that shows the \
         original figure cropped from the PDF: paste it to show the paper's own figure."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "paper": {"type": "string", "minLength": 1, "maxLength": 1000},
                "figure": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "required": ["paper"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = self.name();
        let paper = str_arg(&args, "paper").ok_or_else(|| invalid(tool, "`paper` is required"))?;
        let parts = parts_of(&self.host, tool, paper).await?;
        Ok(figure_output(ctx, &parts, str_arg(&args, "figure")))
    }
}

pub struct GetEquationTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for GetEquationTool {
    fn name(&self) -> &'static str {
        app_tools::GET_EQUATION
    }
    fn label(&self) -> &'static str {
        "Read equation"
    }
    fn label_template(&self) -> &'static str {
        "Reading[ equation {equation} of] {paper}"
    }
    fn description(&self) -> &'static str {
        "Get a display equation of an indexed paper (`paper`: file path, file name or title) \
         by its number (`equation`, e.g. \"3\" or \"2.1\") or by symbols in it; without \
         `equation`, list the paper's equations. Returns its LaTeX, marked as taken from the \
         paper's LaTeX source (when the .tex is in the library) or reconstructed from the PDF \
         text, numbered n with its box on the page: cite it with [n]."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "paper": {"type": "string", "minLength": 1, "maxLength": 1000},
                "equation": {"type": "string", "minLength": 1, "maxLength": 300}
            },
            "required": ["paper"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = self.name();
        let paper = str_arg(&args, "paper").ok_or_else(|| invalid(tool, "`paper` is required"))?;
        let parts = parts_of(&self.host, tool, paper).await?;
        Ok(equation_output(ctx, &parts, str_arg(&args, "equation")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_tools::testing;
    use shodh_rag::processing::document_model::{Block, BlockKind, PageInfo, StructuredDocument};

    fn paper() -> PaperParts {
        let page = |n: u32| PageInfo {
            number: n,
            width: 612.0,
            height: 792.0,
        };
        let body = |page: u32, y0: f32, y1: f32, text: &str| {
            Block::new(BlockKind::Paragraph, text)
                .on_page(page, Some(BBox::new(72.0, y0, 540.0, y1)))
        };
        let doc = StructuredDocument {
            pages: vec![page(3), page(4)],
            blocks: vec![
                body(3, 620.0, 720.0, "We study the delta rule for linear attention and its chunkwise parallel form in detail."),
                Block::new(
                    BlockKind::Figure {
                        caption: "Figure 2: Throughput of the chunkwise form.".into(),
                    },
                    "Figure 2: Throughput of the chunkwise form.",
                )
                .on_page(3, Some(BBox::new(72.0, 400.0, 540.0, 412.0))),
                body(3, 200.0, 390.0, "As Figure 2 shows, the chunkwise form is faster than the recurrent form for long sequences."),
                Block::new(BlockKind::Equation, "St = St−1 + βt(vt − St−1kt)k⊤t (1)")
                    .on_page(4, Some(BBox::new(150.0, 600.0, 460.0, 620.0))),
            ],
        };
        PaperParts::build("C:/papers/delta.pdf", &doc, &[])
    }

    #[test]
    fn a_figure_is_cited_by_its_caption_with_its_box_and_a_figure_block() {
        let (ctx, _rx) = testing::ctx();
        let parts = paper();
        let out = figure_output(&ctx, &parts, Some("Figure 2"));
        assert!(out.text_for_model.contains("[1] fig-2 — Figure 2, page 3"));
        assert!(out
            .text_for_model
            .contains("Text near it: As Figure 2 shows"));
        assert!(out.text_for_model.contains("```figure\n{"));
        assert!(out.text_for_model.contains("\"figureId\":\"fig-2\""));
        let detail = out.detail.unwrap();
        assert_eq!(detail["passages"][0]["regions"][0]["page"], json!(3));
        assert_eq!(
            ctx.cited_passage(1).map(|p| p.text),
            Some("Figure 2: Throughput of the chunkwise form.".to_string())
        );
        // By caption words, and a miss names the figures there are.
        let (ctx, _rx) = testing::ctx();
        let out = figure_output(&ctx, &parts, Some("chunkwise throughput"));
        assert!(out.text_for_model.contains("fig-2"));
        let out = figure_output(&ctx, &parts, Some("Figure 9"));
        assert!(out.text_for_model.contains("Its figures: Figure 2."));
        assert_eq!(ctx.passages_issued(), 1);
        // A listing numbers nothing.
        let (ctx, _rx) = testing::ctx();
        let out = figure_output(&ctx, &parts, None);
        assert!(out.text_for_model.contains("- fig-2 (page 3)"));
        assert_eq!(ctx.passages_issued(), 0);
    }

    #[test]
    fn the_figure_block_carries_path_page_box_and_caption() {
        let parts = paper();
        let block = figure_block(&parts, &parts.figures[0]);
        let body = block
            .strip_prefix("```figure\n")
            .and_then(|b| b.strip_suffix("\n```"))
            .unwrap();
        let spec: Value = serde_json::from_str(body).unwrap();
        assert_eq!(spec["paper"], json!("C:/papers/delta.pdf"));
        assert_eq!(spec["page"], json!(3));
        assert_eq!(spec["bbox"].as_array().unwrap().len(), 4);
        assert!(spec["bbox"][3].as_f64().unwrap() > 412.0);
    }

    #[test]
    fn an_equation_is_cited_by_its_text_with_latex_and_its_origin() {
        let (ctx, _rx) = testing::ctx();
        let parts = paper();
        let out = equation_output(&ctx, &parts, Some("(1)"));
        assert!(out
            .text_for_model
            .contains("[1] eq-1 — Equation (1), page 4"));
        assert!(out
            .text_for_model
            .contains("reconstructed from the PDF text"));
        assert!(out.text_for_model.contains("\\beta"));
        assert_eq!(
            ctx.cited_passage(1).map(|p| p.text),
            Some("Equation (1), page 4: As Figure 2 shows, the chunkwise form is faster than the recurrent form for long sequences. St = St−1 + βt(vt − St−1kt)k⊤t".to_string())
        );
        assert_eq!(
            out.detail.unwrap()["passages"][0]["regions"][0]["y1"],
            json!(620.0)
        );
        let out = equation_output(&ctx, &parts, Some("7"));
        assert!(out.text_for_model.contains("Numbered equations: 1."));
    }

    #[tokio::test]
    async fn papers_must_be_indexed_pdfs() {
        let t = testing::host().await;
        let (ctx, _rx) = testing::ctx();
        let figure = ShowFigureTool {
            host: t.host.clone(),
        };
        let err = figure
            .execute(json!({ "paper": "C:/Windows/win.ini" }), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)), "{err:?}");
        let equation = GetEquationTool {
            host: t.host.clone(),
        };
        let err = equation.execute(json!({}), &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
        assert_eq!(figure.tier(), RiskTier::Read);
        assert_eq!(equation.tier(), RiskTier::Read);
    }
}
