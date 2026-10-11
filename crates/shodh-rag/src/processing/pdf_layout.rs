//! Layout-aware PDF parsing: positioned text runs from `pdf_oxide` are
//! assembled into lines and then into semantic blocks (headings with levels,
//! paragraphs, list items, display equations, code, footnotes, tables,
//! bibliography entries), each with its page and bounding box.
//!
//! Choice of backend and the heuristics are documented in
//! `docs/adr/0002-document-parser.md`. Everything here is geometric and
//! font-statistical; no model is loaded.
//!
//! Coordinates are PDF user space (points, bottom-left origin), see
//! [`super::document_model::BBox`].

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::document_model::{
    is_references_heading, is_table_caption, BBox, Block, BlockKind, PageInfo, StructuredDocument,
    TABLE_COVERAGE_THRESHOLD,
};
use super::form_layout::{self, LayoutLine};
use super::pdf_forms::FieldKind;
use super::table_model::{ModelTable, TableModel};
use super::table_structure::resolve_cells;

/// Why a PDF could not be laid out.
#[derive(Debug, thiserror::Error)]
pub enum PdfLayoutError {
    #[error("cannot open PDF: {0}")]
    Open(String),
    #[error("PDF has no pages")]
    NoPages,
    #[error("PDF layout extraction panicked")]
    Panicked,
}

/// Parse a PDF into a [`StructuredDocument`]. Pages that fail to extract are
/// skipped (and logged); the document fails only when it cannot be opened.
/// The parser never panics: a panic inside the PDF library is caught and
/// reported as [`PdfLayoutError::Panicked`].
pub fn parse_pdf_layout(bytes: &[u8]) -> Result<StructuredDocument, PdfLayoutError> {
    parse_pdf_layout_with(bytes, TableMode::Heuristic).map(|parsed| parsed.document)
}

/// How tables are found.
#[derive(Clone, Copy)]
pub enum TableMode<'a> {
    /// pdf_oxide's detector on pages with a `Table N` caption (every page when the
    /// document has no caption).
    Heuristic,
    /// As [`TableMode::Heuristic`], and the table-candidate pages are reported.
    Candidates,
    /// The table model structures the tables of the candidate pages; pages where it
    /// finds none fall back to the heuristic detector.
    Model(&'a TableModel),
}

/// Why a page is a table candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateCue {
    /// A `Table N` caption.
    Caption,
    /// Three or more nearby lines that are split into cells and mostly numbers.
    NumericCluster,
    /// Lines whose cells start at the same x positions in three or more columns.
    AlignedColumns,
    /// Three or more horizontal rules on a page that has a numeric row.
    RulingLines,
    /// The heuristic table detector found a table on the page.
    DetectedTable,
}

/// A table-candidate page and why.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TableCandidate {
    pub page: u32,
    pub cues: Vec<CandidateCue>,
}

/// What the table model did on one page.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPageReport {
    pub page: u32,
    pub regions: usize,
    pub tables: usize,
    pub millis: u64,
}

/// Table detection of one parse.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableReport {
    pub candidates: Vec<TableCandidate>,
    /// Pages the model ran on (empty without the model).
    pub model_pages: Vec<ModelPageReport>,
    /// Pages the model failed on, with the reason (they keep the heuristic tables).
    pub model_errors: Vec<String>,
    /// Table blocks in the document.
    pub tables: usize,
    /// Table blocks whose structure came from the model.
    pub model_tables: usize,
}

/// A parsed PDF with its table report.
#[derive(Debug, Clone)]
pub struct ParsedLayout {
    pub document: StructuredDocument,
    pub tables: TableReport,
}

/// [`parse_pdf_layout`] with a choice of table detection.
pub fn parse_pdf_layout_with(
    bytes: &[u8],
    mode: TableMode<'_>,
) -> Result<ParsedLayout, PdfLayoutError> {
    let owned = bytes.to_vec();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        parse_inner(owned, mode)
    }))
    .unwrap_or(Err(PdfLayoutError::Panicked))
}

fn parse_inner(bytes: Vec<u8>, mode: TableMode<'_>) -> Result<ParsedLayout, PdfLayoutError> {
    let model_bytes = match mode {
        TableMode::Model(_) => Some(bytes.clone()),
        _ => None,
    };
    let doc = pdf_oxide::PdfDocument::from_bytes(bytes)
        .map_err(|e| PdfLayoutError::Open(e.to_string()))?;
    let page_count = doc
        .page_count()
        .map_err(|e| PdfLayoutError::Open(e.to_string()))?;
    if page_count == 0 {
        return Err(PdfLayoutError::NoPages);
    }

    let mut pages: Vec<PageLines> = Vec::with_capacity(page_count);
    for index in 0..page_count {
        let number = (index + 1) as u32;
        // Column-aware order (XY-cut), or the structure tree's order for
        // trustworthy tagged PDFs; the default top-to-bottom order
        // interleaves the columns of two-column papers line by line.
        match page_text_in_reading_order(&doc, index) {
            Ok(page_text) => {
                let runs: Vec<Run> = page_text.spans.iter().filter_map(Run::from_span).collect();
                pages.push(PageLines {
                    number,
                    width: page_text.page_width,
                    height: page_text.page_height,
                    lines: build_lines(runs),
                    tables: Vec::new(),
                });
            }
            Err(e) => {
                tracing::debug!(page = number, error = %e, "PDF page text extraction failed");
                let (width, height) = doc
                    .get_page_media_box(index)
                    .map(|(x0, y0, x1, y1)| ((x1 - x0).abs(), (y1 - y0).abs()))
                    .unwrap_or((612.0, 792.0));
                pages.push(PageLines {
                    number,
                    width,
                    height,
                    lines: Vec::new(),
                    tables: Vec::new(),
                });
            }
        }
    }

    remove_page_furniture(&mut pages);

    let mut report = TableReport::default();
    if !matches!(mode, TableMode::Heuristic) {
        report.candidates = pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| {
                let cues = candidate_cues(page, || horizontal_rules(&doc, index, page.width));
                (!cues.is_empty()).then_some(TableCandidate {
                    page: page.number,
                    cues,
                })
            })
            .collect();
    }
    // Table detection is the most expensive heuristic step. Academic PDFs announce
    // their tables with captions, so only captioned pages are scanned; a document
    // without any caption (forms, reports) is scanned fully.
    let captioned: Vec<bool> = pages
        .iter()
        .map(|p| p.lines.iter().any(|l| is_table_caption(&l.text)))
        .collect();
    let any_captions = captioned.iter().any(|&c| c);
    for (index, page) in pages.iter_mut().enumerate() {
        if any_captions && !captioned[index] {
            continue;
        }
        match doc.extract_tables(index) {
            Ok(tables) => {
                page.tables = tables
                    .into_iter()
                    .filter_map(|t| RawTable::from_oxide(t, page.height))
                    .collect();
                recover_table_headers(page);
            }
            Err(e) => tracing::debug!(page = page.number, error = %e, "table detection failed"),
        }
    }
    if !matches!(mode, TableMode::Heuristic) {
        // A table the heuristic detector found makes its page a candidate too.
        for page in pages.iter().filter(|p| !p.tables.is_empty()) {
            match report.candidates.iter_mut().find(|c| c.page == page.number) {
                Some(candidate) => candidate.cues.push(CandidateCue::DetectedTable),
                None => report.candidates.push(TableCandidate {
                    page: page.number,
                    cues: vec![CandidateCue::DetectedTable],
                }),
            }
        }
        report.candidates.sort_by_key(|c| c.page);
    }

    // The model's tables replace the heuristic ones on the pages where it finds any.
    if let (TableMode::Model(model), Some(bytes)) = (mode, model_bytes.as_deref()) {
        let numbers: Vec<u32> = report.candidates.iter().map(|c| c.page).collect();
        let (results, errors) = model.structure_pages(bytes, &numbers);
        report.model_errors = errors.iter().map(|e| e.to_string()).collect();
        for result in results {
            let index = result.number.saturating_sub(1) as usize;
            let tables: Vec<RawTable> = result
                .tables
                .iter()
                .filter_map(RawTable::from_model)
                .collect();
            report.model_pages.push(ModelPageReport {
                page: result.number,
                regions: result.regions,
                tables: tables.len(),
                millis: u64::try_from(result.elapsed.as_millis()).unwrap_or(u64::MAX),
            });
            if let Some(page) = pages.get_mut(index) {
                if !tables.is_empty() {
                    page.tables = tables;
                }
            }
        }
    }

    let stats = DocStats::compute(&pages);
    let mut blocks = Vec::new();
    let mut in_references = false;
    for page in &pages {
        segment_page(page, &stats, &mut in_references, &mut blocks);
    }
    assign_heading_levels(&mut blocks, &stats);

    report.model_tables = blocks
        .iter()
        .filter(|b| b.table.as_ref().is_some_and(|t| t.from_model))
        .count();
    let mut out = StructuredDocument {
        pages: pages
            .iter()
            .map(|p| PageInfo {
                number: p.number,
                width: p.width,
                height: p.height,
            })
            .collect(),
        blocks: blocks.into_iter().map(|b| b.into_block()).collect(),
    };
    out.finalize();
    report.tables = out
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Table { .. }))
        .count();
    Ok(ParsedLayout {
        document: out,
        tables: report,
    })
}

/// Page text in reading order: the structure tree's order when the PDF is
/// trustworthily tagged, else column-aware (XY-cut). Either strategy can fail
/// or panic on unusual files independently, so each is tried on its own.
fn page_text_in_reading_order(
    doc: &pdf_oxide::PdfDocument,
    index: usize,
) -> Result<pdf_oxide::layout::PageText, String> {
    use pdf_oxide::document::ReadingOrder;
    let mut last_error = String::new();
    for order in [ReadingOrder::Structure, ReadingOrder::ColumnAware] {
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            doc.extract_page_text_with_options(index, order)
        }));
        match attempt {
            Ok(Ok(text)) => return Ok(text),
            Ok(Err(e)) => last_error = format!("{order:?}: {e}"),
            Err(_) => last_error = format!("{order:?}: panicked"),
        }
    }
    Err(last_error)
}

// ── Table-candidate pages ───────────────────────────────────────────────────

/// Fewest nearby tabular lines that make a numeric cluster.
const CLUSTER_LINES: usize = 3;
/// Fewest lines sharing aligned column starts.
const ALIGNED_LINES: usize = 4;
/// Fewest aligned columns.
const ALIGNED_COLUMNS: usize = 3;
/// Fewest horizontal rules (at distinct heights) that cue a ruled table.
const MIN_RULES: usize = 3;

/// The cells of a line: its runs, split where the gap to the next run exceeds
/// most of a character's height (word spaces are far narrower). Returns each
/// cell's text and left edge.
fn line_cells(line: &Line) -> Vec<(String, f32)> {
    line_segments(line)
        .into_iter()
        .map(|s| (s.text, s.bbox.x0))
        .collect()
}

/// The table whose region holds a line's centre.
fn table_of(tables: &[RawTable], line: &Line) -> Option<usize> {
    tables.iter().position(|t| {
        t.bbox
            .contains_point(line.bbox.center_x(), line.bbox.center_y())
    })
}

/// Tokens of text compared by the completeness check: lower-cased words with
/// surrounding punctuation removed (`(1,234)` and `1,234` match).
fn coverage_tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| c.is_whitespace() || c == '|')
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|t| !t.is_empty())
}

/// Share of the text-layer tokens in a table's region (the lines it replaces) that
/// its header and cells hold. 1.0 for a region without tokens.
fn cell_coverage(table: &RawTable, region: &[&Line]) -> f32 {
    let mut cells: HashMap<String, usize> = HashMap::new();
    for cell in table.header.iter().chain(table.rows.iter().flatten()) {
        for token in coverage_tokens(cell) {
            *cells.entry(token).or_default() += 1;
        }
    }
    let mut total = 0usize;
    let mut held = 0usize;
    for line in region {
        for token in coverage_tokens(&line.text) {
            total += 1;
            if let Some(n) = cells.get_mut(&token).filter(|n| **n > 0) {
                *n -= 1;
                held += 1;
            }
        }
    }
    if total == 0 {
        1.0
    } else {
        held as f32 / total as f32
    }
}

/// Lines as the form pairing reads them ([`form_layout`]).
fn layout_lines(lines: &[Line]) -> Vec<LayoutLine> {
    lines
        .iter()
        .map(|l| LayoutLine {
            text: l.text.clone(),
            bbox: l.bbox,
            size: l.size,
            segments: line_segments(l)
                .into_iter()
                .map(|s| (s.text, s.bbox))
                .collect(),
        })
        .collect()
}

/// A run of text on one line between wide gaps (a table cell or a form label/value).
#[derive(Debug, Clone)]
struct Segment {
    text: String,
    bbox: BBox,
}

/// The segments of a line: its runs, split where the gap to the next run exceeds
/// most of a character's height. Runs closer than that join, with a space when they
/// are visibly apart.
fn line_segments(line: &Line) -> Vec<Segment> {
    let gap = 0.8 * line.size.max(1.0);
    let mut cells: Vec<Segment> = Vec::new();
    for (text, bbox) in &line.pieces {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        match cells.last_mut() {
            Some(cell) if bbox.x0 - cell.bbox.x1 <= gap => {
                let apart = bbox.x0 - cell.bbox.x1 > 0.12 * line.size.max(1.0)
                    || text.starts_with(char::is_whitespace);
                if apart && !cell.text.ends_with(' ') {
                    cell.text.push(' ');
                }
                cell.text.push_str(trimmed);
                cell.bbox = cell.bbox.union(bbox);
            }
            _ => cells.push(Segment {
                text: trimmed.to_string(),
                bbox: *bbox,
            }),
        }
    }
    cells
}

/// Whether a line reads as a table row: at least two cells, at least two numbers,
/// and numbers make up at least 40% of its tokens.
fn is_tabular_line(line: &Line) -> bool {
    let cells = line_cells(line);
    if cells.len() < 2 {
        return false;
    }
    let tokens: Vec<&str> = cells
        .iter()
        .flat_map(|(t, _)| t.split_whitespace())
        .collect();
    let numbers = tokens
        .iter()
        .filter(|t| {
            t.chars().any(|c| c.is_ascii_digit()) && super::document_model::is_numeric_cell(t)
        })
        .count();
    numbers >= 2 && numbers * 5 >= tokens.len() * 2
}

/// The cues that make `page` a table candidate. `rules` counts the page's
/// horizontal rules; it is only called when no cheaper cue fired and the page has
/// a numeric row.
fn candidate_cues(page: &PageLines, rules: impl FnOnce() -> usize) -> Vec<CandidateCue> {
    let mut cues = Vec::new();
    if page.lines.iter().any(|l| is_table_caption(&l.text)) {
        cues.push(CandidateCue::Caption);
    }
    // Numeric clusters: tabular lines close to each other vertically.
    let mut tabular: Vec<&Line> = page.lines.iter().filter(|l| is_tabular_line(l)).collect();
    tabular.sort_by(|a, b| b.bbox.y0.total_cmp(&a.bbox.y0));
    let mut run = 1usize;
    let mut best = usize::from(!tabular.is_empty());
    for pair in tabular.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let close = a.bbox.y0 - b.bbox.y1 <= 3.0 * a.size.max(b.size).max(1.0);
        run = if close { run + 1 } else { 1 };
        best = best.max(run);
    }
    if best >= CLUSTER_LINES {
        cues.push(CandidateCue::NumericCluster);
    }
    // Aligned columns: cell left edges (3 pt buckets) shared by many multi-cell lines.
    let rows: Vec<Vec<i32>> = page
        .lines
        .iter()
        .map(line_cells)
        .filter(|c| c.len() >= ALIGNED_COLUMNS)
        .map(|c| c.iter().map(|(_, x)| (x / 3.0).round() as i32).collect())
        .collect();
    if rows.len() >= ALIGNED_LINES {
        let mut counts: HashMap<i32, usize> = HashMap::new();
        for row in &rows {
            let mut seen: Vec<i32> = Vec::new();
            for &x in row {
                if !seen.iter().any(|s| (s - x).abs() <= 1) {
                    seen.push(x);
                    *counts.entry(x).or_default() += 1;
                }
            }
        }
        let column = |x: i32| {
            (-1..=1)
                .map(|d| counts.get(&(x + d)).copied().unwrap_or(0))
                .sum::<usize>()
                >= ALIGNED_LINES
        };
        let aligned_rows = rows
            .iter()
            .filter(|row| row.iter().filter(|&&x| column(x)).count() >= ALIGNED_COLUMNS)
            .count();
        if aligned_rows >= ALIGNED_LINES {
            cues.push(CandidateCue::AlignedColumns);
        }
    }
    if cues.is_empty() && !tabular.is_empty() && rules() >= MIN_RULES {
        cues.push(CandidateCue::RulingLines);
    }
    cues
}

/// Horizontal rules on a page at distinct heights: stroked lines or thin filled
/// rectangles at least 15% of the page wide (booktabs `\toprule`, `\midrule`, ...).
fn horizontal_rules(doc: &pdf_oxide::PdfDocument, index: usize, page_width: f32) -> usize {
    let paths = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| doc.extract_paths(index)));
    let Ok(Ok(paths)) = paths else {
        return 0;
    };
    let mut heights: Vec<f32> = paths
        .iter()
        .filter(|p| p.is_horizontal_line(1.5))
        .filter(|p| p.rendered_bbox().width.abs() >= 0.15 * page_width)
        .map(|p| p.bbox.y)
        .collect();
    heights.sort_by(|a, b| a.total_cmp(b));
    heights.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    heights.len()
}

// ── Runs and lines ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Run {
    text: String,
    bbox: BBox,
    size: f32,
    bold: bool,
    mono: bool,
}

impl Run {
    fn from_span(span: &pdf_oxide::layout::TextSpan) -> Option<Run> {
        let text = normalize_ligatures(&span.text);
        if text.trim().is_empty() {
            return None;
        }
        let r = span.bbox;
        let bbox = BBox::new(r.x, r.y, r.x + r.width, r.y + r.height);
        let size = if span.font_size > 0.0 {
            span.font_size
        } else {
            bbox.height().max(1.0)
        };
        // Vertical (rotated) text such as the arXiv identifier in the margin.
        let visible_chars = text.chars().filter(|c| !c.is_whitespace()).count();
        if visible_chars > 2 && bbox.height() > 2.5 * size && bbox.width() < 1.5 * size {
            return None;
        }
        Some(Run {
            text,
            bbox,
            size,
            bold: span_is_bold(&span.font_name, span.font_weight),
            mono: span.is_monospace || font_is_mono(&span.font_name),
        })
    }
}

fn normalize_ligatures(text: &str) -> String {
    if !text.chars().any(|c| ('\u{FB00}'..='\u{FB06}').contains(&c)) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' => out.push_str("fi"),
            '\u{FB02}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => out.push_str("st"),
            other => out.push(other),
        }
    }
    out
}

/// Bold from the font name when the PDF names its fonts; from the declared
/// weight when the name is only a resource key such as `F48`.
fn span_is_bold(name: &str, weight: pdf_oxide::layout::FontWeight) -> bool {
    let base = name.rsplit('+').next().unwrap_or(name);
    let letters = base.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let resource_key = letters <= 2 && base.chars().any(|c| c.is_ascii_digit());
    if resource_key {
        weight >= pdf_oxide::layout::FontWeight::SemiBold
    } else {
        font_is_bold(name)
    }
}

/// Bold from the font name. The weight flags some PDFs carry are unreliable
/// (body text in common LaTeX fonts is often reported as bold), while the
/// names are not: `CMBX10`, `NimbusRomNo9L-Medi`, `Arial-BoldMT`, ...
fn font_is_bold(name: &str) -> bool {
    let base = name.rsplit('+').next().unwrap_or(name);
    let lower = base.to_ascii_lowercase();
    lower.contains("bold")
        || lower.contains("black")
        || lower.contains("heavy")
        || lower.contains("semibold")
        || lower.contains("demi")
        || lower.ends_with("-medi")
        || lower.ends_with("-med")
        || base.starts_with("CMBX")
        || base.starts_with("CMB10")
        || base.starts_with("SFBX")
        || base.contains("-BX")
        || base.ends_with("-B")
}

fn font_is_mono(name: &str) -> bool {
    let base = name.rsplit('+').next().unwrap_or(name);
    let lower = base.to_ascii_lowercase();
    lower.contains("mono")
        || lower.contains("courier")
        || lower.contains("consol")
        || lower.contains("menlo")
        || lower.contains("typewriter")
        || base.starts_with("CMTT")
        || base.starts_with("SFTT")
        || lower.starts_with("inconsolata")
}

#[derive(Debug, Clone)]
struct Line {
    text: String,
    bbox: BBox,
    size: f32,
    bold: bool,
    mono: bool,
    chars: usize,
    /// The runs the line was built from, with their boxes (used to split table
    /// header lines into columns).
    pieces: Vec<(String, BBox)>,
}

/// Group runs (in content order) into visual lines. A run joins the current
/// line when it overlaps it vertically and does not move back to the left;
/// otherwise it starts a new line. Superscripts and subscripts overlap their
/// line and therefore stay on it.
fn build_lines(runs: Vec<Run>) -> Vec<Line> {
    struct Acc {
        text: String,
        pieces: Vec<(String, BBox)>,
        bbox: BBox,
        sizes: HashMap<i32, usize>,
        bold_chars: usize,
        mono_chars: usize,
        chars: usize,
        last_x1: f32,
        base_size: f32,
    }
    fn finish(acc: Acc) -> Line {
        let size = acc
            .sizes
            .iter()
            .max_by_key(|(size, count)| (**count, **size))
            .map(|(size, _)| *size as f32 / 2.0)
            .unwrap_or(acc.base_size);
        Line {
            text: acc.text.trim().to_string(),
            bbox: acc.bbox,
            size,
            bold: acc.bold_chars * 2 > acc.chars,
            mono: acc.mono_chars * 2 > acc.chars,
            chars: acc.chars,
            pieces: acc.pieces,
        }
    }
    let mut lines = Vec::new();
    let mut current: Option<Acc> = None;
    for run in runs {
        let visible = run.text.chars().filter(|c| !c.is_whitespace()).count();
        let joins = current.as_ref().is_some_and(|acc| {
            let line_h = acc.bbox.height().max(1.0);
            let overlap = acc.bbox.y1.min(run.bbox.y1) - acc.bbox.y0.max(run.bbox.y0);
            let min_h = line_h.min(run.bbox.height().max(1.0));
            overlap > 0.35 * min_h && run.bbox.x0 >= acc.last_x1 - 0.6 * acc.base_size
        });
        if joins {
            if let Some(acc) = current.as_mut() {
                let gap = run.bbox.x0 - acc.last_x1;
                let needs_space = gap > 0.12 * run.size.min(acc.base_size)
                    && !acc.text.ends_with(char::is_whitespace)
                    && !run.text.starts_with(char::is_whitespace);
                if needs_space {
                    acc.text.push(' ');
                }
                acc.text.push_str(&run.text);
                acc.pieces.push((run.text.clone(), run.bbox));
                acc.bbox = acc.bbox.union(&run.bbox);
                acc.last_x1 = acc.last_x1.max(run.bbox.x1);
                *acc.sizes
                    .entry((run.size * 2.0).round() as i32)
                    .or_default() += visible;
                acc.chars += visible;
                if run.bold {
                    acc.bold_chars += visible;
                }
                if run.mono {
                    acc.mono_chars += visible;
                }
            }
            continue;
        }
        if let Some(acc) = current.take() {
            lines.push(finish(acc));
        }
        let mut sizes = HashMap::new();
        sizes.insert((run.size * 2.0).round() as i32, visible);
        current = Some(Acc {
            bbox: run.bbox,
            last_x1: run.bbox.x1,
            base_size: run.size,
            bold_chars: if run.bold { visible } else { 0 },
            mono_chars: if run.mono { visible } else { 0 },
            chars: visible,
            sizes,
            pieces: vec![(run.text.clone(), run.bbox)],
            text: run.text,
        });
    }
    if let Some(acc) = current.take() {
        lines.push(finish(acc));
    }
    lines.retain(|l| !l.text.is_empty());
    lines
}

// ── Pages, furniture, tables ────────────────────────────────────────────────

struct PageLines {
    number: u32,
    width: f32,
    height: f32,
    lines: Vec<Line>,
    tables: Vec<RawTable>,
}

/// Widest column span a table cell is expanded to (guards against corrupt spans).
const MAX_COLSPAN: u32 = 64;

#[derive(Debug, Clone)]
struct RawTable {
    bbox: BBox,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    /// Cell boxes, header row first, aligned with the expanded cells.
    cell_boxes: Vec<Vec<Option<BBox>>>,
    /// The structure came from the table model.
    from_model: bool,
}

impl RawTable {
    /// Accept a detected table only when it looks like one: at least two
    /// rows and two columns, mostly filled short cells, and not covering most
    /// of the page (two-column body text is the classic false positive).
    fn from_oxide(
        table: pdf_oxide::structure::table_extractor::Table,
        page_height: f32,
    ) -> Option<RawTable> {
        let r = table.bbox?;
        let bbox = BBox::new(r.x, r.y, r.x + r.width, r.y + r.height);
        if table.col_count < 2 || table.rows.len() < 2 || bbox.height() > 0.75 * page_height {
            return None;
        }
        // A cell spanning several columns is expanded to one position per column so
        // that a cell's index is its column: a spanning header names every column under
        // it (its text is repeated), a spanning body cell holds its text once (repeating
        // a value would report it several times). Every position keeps the cell's box.
        let mut rows: Vec<Vec<String>> = Vec::with_capacity(table.rows.len());
        let mut cell_boxes: Vec<Vec<Option<BBox>>> = Vec::with_capacity(table.rows.len());
        for (index, row) in table.rows.iter().enumerate() {
            let header_row = index == 0 || row.is_header;
            let mut texts = Vec::with_capacity(row.cells.len());
            let mut boxes = Vec::with_capacity(row.cells.len());
            for cell in &row.cells {
                let text = normalize_ligatures(&super::document_model::collapse_ws(&cell.text));
                let cell_box = cell
                    .bbox
                    .map(|b| BBox::new(b.x, b.y, b.x + b.width, b.y + b.height).rounded());
                let span = usize::try_from(cell.colspan.clamp(1, MAX_COLSPAN)).unwrap_or(1);
                for position in 0..span {
                    if position == 0 || header_row || cell.is_header {
                        texts.push(text.clone());
                    } else {
                        texts.push(String::new());
                    }
                    boxes.push(cell_box);
                }
            }
            rows.push(texts);
            cell_boxes.push(boxes);
        }
        let cells: Vec<&String> = rows.iter().flatten().collect();
        let filled = cells.iter().filter(|c| !c.is_empty()).count();
        if cells.is_empty() || filled * 2 < cells.len() {
            return None;
        }
        let mean_len =
            cells.iter().map(|c| c.chars().count()).sum::<usize>() as f32 / filled.max(1) as f32;
        if mean_len > 48.0 {
            return None;
        }
        let header = rows.remove(0);
        Some(RawTable {
            bbox,
            header,
            rows,
            cell_boxes,
            from_model: false,
        })
    }

    /// A table the model structured (spans resolved, multi-row headers flattened).
    fn from_model(table: &ModelTable) -> Option<RawTable> {
        let resolved = resolve_cells(&table.cells)?;
        let clean = |t: &String| tidy_cell_text(&normalize_ligatures(t));
        // The table's box is its cells' extent: the detected region can reach into the
        // caption above it, whose lines must stay outside the table.
        let bbox = table
            .cells
            .iter()
            .filter_map(|c| c.bbox)
            .reduce(|a, b| a.union(&b))
            .unwrap_or(table.bbox);
        Some(RawTable {
            bbox: bbox.rounded(),
            header: resolved.header.iter().map(clean).collect(),
            rows: resolved
                .rows
                .iter()
                .map(|r| r.iter().map(clean).collect())
                .collect(),
            cell_boxes: resolved
                .cell_boxes
                .iter()
                .map(|r| r.iter().map(|b| b.map(|b| b.rounded())).collect())
                .collect(),
            from_model: true,
        })
    }
}

/// A sign the text layer separated from its number (`− 0.68`, the minus drawn as
/// its own glyph run) is joined back to it (`−0.68`), so the cell reads as one
/// signed value.
fn join_detached_sign(text: &str) -> String {
    static DETACHED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([+\-−–])\s+(\d)").expect("static regex"));
    DETACHED.replace(text, "$1$2").into_owned()
}

/// Spacing the model's word matching puts inside a cell, removed: after an opening
/// and before a closing bracket, before a comma, inside a signed number after a
/// bracket or comma (`[ − 1 , 1]` → `[−1, 1]`), and before a hyphenated suffix
/// (`Moneta -H` → `Moneta-H`, `L 1 -norm` → `L 1-norm`).
fn tidy_cell_text(text: &str) -> String {
    static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"([\[(])\s+", "$1"),
            (r"\s+([\])])", "$1"),
            (r"\s+,", ","),
            (r"([\[(,]\s*)([+\-−–])\s+(\d)", "$1$2$3"),
            (r"(\w) -(\pL)", "$1-$2"),
        ]
        .into_iter()
        .map(|(pattern, with)| (Regex::new(pattern).expect("static regex"), with))
        .collect()
    });
    let mut out = join_detached_sign(text);
    for (re, with) in RULES.iter() {
        out = re.replace_all(&out, *with).into_owned();
    }
    out
}

/// Most header lines recovered above one table.
const MAX_HEADER_LINES: usize = 3;

/// Table detection often stops below a table's header: the column titles sit in
/// lines just above the detected region and the first data row becomes the
/// "header". When a table's header row is mostly numbers, the lines directly above
/// it that split into the table's columns are taken as its header (outermost line
/// first, joined per column) and the numeric row is moved back into the body. The
/// table's box grows to cover them, so the lines are not emitted twice.
fn recover_table_headers(page: &mut PageLines) {
    let boxes: Vec<BBox> = page.tables.iter().map(|t| t.bbox).collect();
    for index in 0..page.tables.len() {
        let table = &page.tables[index];
        if !mostly_numeric(&table.header) {
            continue;
        }
        let columns = column_extents(&table.cell_boxes, table.header.len());
        if columns.iter().filter(|c| c.is_some()).count() < 2 {
            continue;
        }
        let mut chosen: Vec<&Line> = Vec::new();
        let mut top = table.bbox.y1;
        while chosen.len() < MAX_HEADER_LINES {
            // The nearest line above the current top that lies over the table.
            let candidate = page
                .lines
                .iter()
                .filter(|l| l.bbox.y0 >= top - 1.0)
                .filter(|l| {
                    let overlap = l.bbox.x1.min(table.bbox.x1) - l.bbox.x0.max(table.bbox.x0);
                    overlap > 0.5 * l.bbox.width()
                })
                .min_by(|a, b| a.bbox.y0.total_cmp(&b.bbox.y0));
            let Some(line) = candidate else { break };
            let gap = line.bbox.y0 - top;
            if gap > 1.8 * line.size.max(1.0)
                || is_table_caption(&line.text)
                || boxes.iter().enumerate().any(|(j, b)| {
                    j != index && b.contains_point(line.bbox.center_x(), line.bbox.center_y())
                })
            {
                break;
            }
            let assigned = split_into_columns(line, &columns);
            if assigned.iter().filter(|c| c.is_some()).count() < 2 {
                break;
            }
            chosen.push(line);
            top = line.bbox.y1;
        }
        if chosen.is_empty() {
            continue;
        }
        let width = table.header.len();
        let mut texts: Vec<Vec<String>> = vec![Vec::new(); width];
        let mut header_boxes: Vec<Option<BBox>> = vec![None; width];
        // Outermost (highest) line first.
        for line in chosen.iter().rev() {
            for (column, piece) in split_into_columns(line, &columns).into_iter().enumerate() {
                if let Some((text, bbox)) = piece {
                    texts[column].push(text);
                    header_boxes[column] = Some(match header_boxes[column] {
                        Some(b) => b.union(&bbox),
                        None => bbox,
                    });
                }
            }
        }
        let header: Vec<String> = texts.into_iter().map(|t| t.join(" ")).collect();
        let grown = chosen
            .iter()
            .fold(page.tables[index].bbox, |acc, l| acc.union(&l.bbox));
        let table = &mut page.tables[index];
        let old_header = std::mem::replace(&mut table.header, header);
        table.rows.insert(0, old_header);
        table.cell_boxes.insert(
            0,
            header_boxes
                .iter()
                .map(|b| b.map(|b| b.rounded()))
                .collect(),
        );
        table.bbox = grown;
    }
}

/// Whether most non-empty cells of a row are numbers.
fn mostly_numeric(row: &[String]) -> bool {
    let filled: Vec<&String> = row.iter().filter(|c| !c.trim().is_empty()).collect();
    let numeric = filled
        .iter()
        .filter(|c| super::document_model::is_numeric_cell(c))
        .count();
    numeric >= 1 && numeric * 2 >= filled.len()
}

/// Horizontal extent of each column: the narrowest box seen in it (a box shared by
/// a spanning cell is wider than its columns).
fn column_extents(cell_boxes: &[Vec<Option<BBox>>], width: usize) -> Vec<Option<(f32, f32)>> {
    let mut out: Vec<Option<(f32, f32)>> = vec![None; width];
    for row in cell_boxes {
        for (column, cell) in row.iter().enumerate().take(width) {
            let Some(b) = cell else { continue };
            if out[column].is_none_or(|(x0, x1)| b.width() < x1 - x0) {
                out[column] = Some((b.x0, b.x1));
            }
        }
    }
    out
}

/// Splits a header line into the table's columns: each word goes to the column
/// whose extent is nearest to the word's centre. Word positions inside a run are
/// estimated from character offsets. Returns per column the text and its box.
fn split_into_columns(line: &Line, columns: &[Option<(f32, f32)>]) -> Vec<Option<(String, BBox)>> {
    let mut out: Vec<Option<(String, BBox)>> = vec![None; columns.len()];
    for (text, bbox) in &line.pieces {
        let chars = text.chars().count().max(1) as f32;
        let mut offset = 0usize;
        for word in text.split(' ') {
            let len = word.chars().count();
            let trimmed = word.trim();
            if !trimmed.is_empty() {
                let x0 = bbox.x0 + bbox.width() * offset as f32 / chars;
                let x1 = bbox.x0 + bbox.width() * (offset + len) as f32 / chars;
                let centre = (x0 + x1) / 2.0;
                let nearest = columns
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| c.map(|(a, b)| (i, a, b)))
                    .min_by(|(_, a0, a1), (_, b0, b1)| {
                        distance(centre, *a0, *a1).total_cmp(&distance(centre, *b0, *b1))
                    });
                if let Some((column, _, _)) = nearest {
                    let word_box = BBox::new(x0, bbox.y0, x1, bbox.y1);
                    out[column] = Some(match out[column].take() {
                        Some((t, b)) => (format!("{t} {trimmed}"), b.union(&word_box)),
                        None => (trimmed.to_string(), word_box),
                    });
                }
            }
            offset += len + 1;
        }
    }
    out
}

/// Distance from `x` to the interval `[x0, x1]` (0 inside it).
fn distance(x: f32, x0: f32, x1: f32) -> f32 {
    if x < x0 {
        x0 - x
    } else if x > x1 {
        x - x1
    } else {
        0.0
    }
}

static RE_PAGE_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:page\s+)?(?:\d{1,4}|[ivxlc]{1,7})(?:\s*(?:/|of)\s*\d{1,4})?$")
        .expect("static regex")
});
static RE_ARXIV_STAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^arXiv:\d{4}\.\d{4,5}(?:v\d+)?").expect("static regex"));

/// Drop running headers/footers, page numbers and the arXiv margin stamp.
/// A line in the top or bottom 8% of the page is furniture when the same
/// text (digits ignored) recurs on at least a third of the pages, or when it
/// is just a page number.
///
/// A recurring line that carries data (letters and digits, such as a return's
/// `Acknowledgement Number: … Date of filing: …` header) is kept once, where it
/// first appears: dropping it everywhere would lose the only copy of its values.
fn remove_page_furniture(pages: &mut [PageLines]) {
    let key = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_digit() {
                    '#'
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .filter(|c| !c.is_whitespace())
            .collect()
    };
    let in_margin =
        |line: &Line, height: f32| line.bbox.y0 > height * 0.92 || line.bbox.y1 < height * 0.08;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for page in pages.iter() {
        let mut seen = std::collections::HashSet::new();
        for line in page.lines.iter().filter(|l| in_margin(l, page.height)) {
            let k = key(&line.text);
            if seen.insert(k.clone()) {
                *counts.entry(k).or_default() += 1;
            }
        }
    }
    let threshold = if pages.len() >= 3 {
        (pages.len() / 3).max(2)
    } else {
        usize::MAX
    };
    let mut kept_once: std::collections::HashSet<String> = std::collections::HashSet::new();
    for page in pages.iter_mut() {
        let height = page.height;
        page.lines.retain(|line| {
            if RE_ARXIV_STAMP.is_match(&line.text) {
                return false;
            }
            if !in_margin(line, height) {
                return true;
            }
            let text = line.text.trim();
            if RE_PAGE_NUMBER.is_match(text) {
                return false;
            }
            let k = key(text);
            if counts.get(&k).copied().unwrap_or(0) < threshold {
                return true;
            }
            let carries_data = text.chars().any(|c| c.is_ascii_digit())
                && text.chars().filter(|c| c.is_alphabetic()).count() >= 3;
            carries_data && kept_once.insert(k)
        });
    }
}

// ── Document statistics ─────────────────────────────────────────────────────

struct DocStats {
    body_size: f32,
    /// Typical baseline-to-baseline distance of body lines.
    leading: f32,
}

impl DocStats {
    fn compute(pages: &[PageLines]) -> DocStats {
        let mut sizes: HashMap<i32, usize> = HashMap::new();
        for line in pages.iter().flat_map(|p| p.lines.iter()) {
            *sizes.entry((line.size * 2.0).round() as i32).or_default() += line.chars;
        }
        let body_size = sizes
            .iter()
            .max_by_key(|(size, count)| (**count, -**size))
            .map(|(size, _)| *size as f32 / 2.0)
            .unwrap_or(10.0)
            .max(4.0);
        let mut deltas: Vec<f32> = Vec::new();
        for page in pages {
            for pair in page.lines.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                if (a.size - body_size).abs() > 0.6 || (b.size - body_size).abs() > 0.6 {
                    continue;
                }
                let delta = a.bbox.y0 - b.bbox.y0;
                if delta > 0.5 * body_size && delta < 2.5 * body_size {
                    deltas.push(delta);
                }
            }
        }
        deltas.sort_by(|a, b| a.total_cmp(b));
        let leading = deltas
            .get(deltas.len() / 2)
            .copied()
            .unwrap_or(1.2 * body_size);
        DocStats { body_size, leading }
    }
}

// ── Line classification ─────────────────────────────────────────────────────

static RE_NUMBERED_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<num>(?:\d{1,2}(?:\.\d{1,2}){0,3}|[A-Z](?:\.\d{1,2}){0,3}|[IVX]{1,5}))\.?\s+\S",
    )
    .expect("static regex")
});
static RE_KNOWN_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:(?:\d{1,2}|[A-Z])\.?\s+)?(?:abstract|introduction|related work|background|preliminaries|methods?|methodology|approach|experiments?|experimental setup|evaluation|results|discussion|conclusions?|conclusion and future work|limitations|future work|acknowledg(?:e)?ments?|references|bibliography|appendix(?:\s+[A-Z])?|supplementary material)\s*[.:]?$",
    )
    .expect("static regex")
});
static RE_LIST_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[•●◦▪‣∙·\-–—*]\s+|\(?[a-z]\)\s+|\(?[ivx]{1,4}\)\s+|\d{1,2}[.)]\s+[A-Z])")
        .expect("static regex")
});
static RE_SPECIAL_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:Theorem|Lemma|Proposition|Corollary|Claim|Conjecture|Assumption|Remark|Example|Definition|Hypothesis|Observation|Fact)\s*[A-Z]?\d*(?:\.\d+)*\s*(?:\([^)]{0,120}\))?\s*[.:]|Proof\b|(?i:figure|fig\.|table|algorithm)\s*(?:[A-Z]?\d+[a-z]?|[IVX]{1,6})\s*[.:|])",
    )
    .expect("static regex")
});
static RE_REF_ENTRY_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\[\d{1,4}\]|\d{1,3}\.\s+\S)").expect("static regex"));
static RE_URL_OR_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)https?://|www\.|[\w.+-]+@[\w-]+\.\w|github\.com|arxiv\.org")
        .expect("static regex")
});
static RE_EQUATION_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\(\s*[A-Z]?\d{1,3}(?:\.\d{1,3})?[a-z]?\s*\)$").expect("static regex")
});

fn is_math_char(c: char) -> bool {
    matches!(
        c,
        '=' | '+'
            | '<'
            | '>'
            | '^'
            | '_'
            | '|'
            | '∑'
            | '∏'
            | '∫'
            | '√'
            | '≤'
            | '≥'
            | '≈'
            | '∈'
            | '∉'
            | '∀'
            | '∃'
            | '→'
            | '←'
            | '↦'
            | '⇒'
            | '⊙'
            | '⊗'
            | '⊕'
            | '·'
            | '×'
            | '÷'
            | '∂'
            | '∇'
            | '∞'
            | '≜'
            | '≡'
            | '∼'
            | '∝'
            | '⋅'
            | '−'
            | '⊤'
            | '⊆'
            | '⊂'
            | '∪'
            | '∩'
            | '‖'
            | '∥'
    ) || ('\u{0391}'..='\u{03C9}').contains(&c)
        || ('\u{1D400}'..='\u{1D7FF}').contains(&c)
}

/// Whether a line reads as mathematics rather than prose: math symbols are
/// present and fewer than half of its tokens are ordinary words.
fn is_math_line(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    let words = tokens
        .iter()
        .filter(|t| {
            let letters = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
            letters >= 3
                && t.chars()
                    .all(|c| c.is_ascii_alphabetic() || ",.;:'()-\u{2019}".contains(c))
        })
        .count();
    let math = text.chars().filter(|&c| is_math_char(c)).count();
    (math >= 1 && words * 2 < tokens.len()) || (math >= 3 && words * 3 < tokens.len() * 2)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HeadingCue {
    /// Numbered heading with this depth ("3.2" → 2).
    Numbered(u8),
    /// Unnumbered heading recognised by size, boldness or a known name.
    Plain,
}

fn heading_cue(line: &Line, stats: &DocStats) -> Option<HeadingCue> {
    let text = line.text.trim();
    let chars = text.chars().count();
    if !(2..=120).contains(&chars) || text.split_whitespace().count() > 16 {
        return None;
    }
    if text.ends_with(',')
        || text.ends_with(';')
        || text.chars().filter(|c| c.is_alphabetic()).count() < 2
    {
        return None;
    }
    if is_math_line(text) || RE_SPECIAL_START.is_match(text) {
        return None;
    }
    let numbered = RE_NUMBERED_HEADING.captures(text).map(|c| {
        let num = &c["num"];
        let depth = num.split('.').filter(|p| !p.is_empty()).count().max(1) as u8;
        depth.min(6)
    });
    if text.matches(',').count() >= 2 {
        // Author lists and enumerations, not titles.
        return None;
    }
    let known = RE_KNOWN_SECTION.is_match(text);
    let ratio = line.size / stats.body_size;
    // Title text after the number; a sentence break inside it means a
    // run-in paragraph lead ("K Layer widths. In the ..."), not a heading.
    let title_part = RE_NUMBERED_HEADING
        .captures(text)
        .and_then(|c| c.name("num"))
        .map(|m| text[m.end()..].trim_start_matches('.').trim())
        .unwrap_or(text);
    let run_in = title_part.contains(". ");
    let all_caps = title_part.chars().filter(|c| c.is_alphabetic()).count() >= 3
        && title_part
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(|c| c.is_uppercase());
    if numbered.is_some() && all_caps && !run_in && ratio >= 0.8 {
        return numbered.map(HeadingCue::Numbered);
    }
    // A larger font is the strongest cue; numbered lines also need to look
    // like a title (start with a capital after the number, not end a sentence).
    let looks_titled = !text.ends_with('.') || known;
    if ratio >= 1.15 && looks_titled {
        return Some(
            numbered
                .map(HeadingCue::Numbered)
                .unwrap_or(HeadingCue::Plain),
        );
    }
    if line.bold && ratio >= 0.85 && looks_titled && !run_in {
        if let Some(depth) = numbered {
            return Some(HeadingCue::Numbered(depth));
        }
        if known {
            return Some(HeadingCue::Plain);
        }
    }
    if known
        && text
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(|c| c.is_uppercase())
    {
        return Some(HeadingCue::Plain);
    }
    None
}

// ── Segmentation into blocks ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum RawKind {
    Heading(HeadingCue),
    Text,
    ListItem,
    Equation,
    Code,
    Footnote,
    Table,
    /// A label–value row of a flattened form ([`super::form_layout`]).
    FormField,
    /// The text of a table region its cells did not fully capture.
    TableText,
}

struct RawBlock {
    kind: RawKind,
    lines: Vec<Line>,
    page: u32,
    bbox: BBox,
    size: f32,
    level: u8,
    title: bool,
    table: Option<RawTable>,
    /// Label and value of a [`RawKind::FormField`] block.
    field: Option<(String, String)>,
    /// Share of the region's text-layer tokens a [`RawKind::Table`] block's cells hold.
    coverage: Option<f32>,
}

impl RawBlock {
    fn new(kind: RawKind, line: Line, page: u32) -> RawBlock {
        RawBlock {
            kind,
            bbox: line.bbox,
            size: line.size,
            lines: vec![line],
            page,
            level: 0,
            title: false,
            table: None,
            field: None,
            coverage: None,
        }
    }

    fn push(&mut self, line: Line) {
        self.bbox = self.bbox.union(&line.bbox);
        self.lines.push(line);
    }

    fn min_x0(&self) -> f32 {
        self.lines
            .iter()
            .map(|l| l.bbox.x0)
            .fold(f32::MAX, f32::min)
    }

    fn max_x1(&self) -> f32 {
        self.lines
            .iter()
            .map(|l| l.bbox.x1)
            .fold(f32::MIN, f32::max)
    }

    fn joined_text(&self) -> String {
        let separator_newline = matches!(
            self.kind,
            RawKind::Code | RawKind::Equation | RawKind::TableText
        );
        let mut out = String::new();
        for line in &self.lines {
            let text = line.text.trim();
            if out.is_empty() {
                out.push_str(text);
                continue;
            }
            if separator_newline {
                out.push('\n');
                out.push_str(text);
                continue;
            }
            // De-hyphenate words broken across lines ("hard-" + "ware").
            let hyphenated = out.ends_with('-')
                && out[..out.len() - 1].ends_with(|c: char| c.is_alphabetic())
                && text.starts_with(|c: char| c.is_lowercase());
            if hyphenated {
                out.pop();
            } else {
                out.push(' ');
            }
            out.push_str(text);
        }
        out
    }

    fn into_block(self) -> Block {
        if let (RawKind::FormField, Some((label, value))) = (self.kind, &self.field) {
            return Block::form_field(label, value, FieldKind::Layout)
                .on_page(self.page, Some(self.bbox.rounded()));
        }
        let text = self.joined_text();
        let kind = match self.kind {
            RawKind::Heading(_) if self.title => BlockKind::Title,
            RawKind::Heading(_) => BlockKind::Heading {
                level: self.level.max(1),
            },
            RawKind::Text => BlockKind::Paragraph,
            RawKind::ListItem => BlockKind::ListItem,
            RawKind::Equation => BlockKind::Equation,
            RawKind::Code => BlockKind::Code,
            RawKind::Footnote => BlockKind::Footnote,
            // Without its pair (never built that way) the row is plain text.
            RawKind::FormField => BlockKind::Paragraph,
            RawKind::TableText => BlockKind::TableText,
            RawKind::Table => match self.table {
                Some(t) => BlockKind::Table {
                    header: t.header,
                    rows: t.rows,
                    caption: None,
                    cell_boxes: t.cell_boxes,
                    cell_coverage: self.coverage,
                },
                None => BlockKind::Paragraph,
            },
        };
        let text = if matches!(kind, BlockKind::Table { .. }) {
            String::new()
        } else {
            text
        };
        Block::new(kind, text).on_page(self.page, Some(self.bbox.rounded()))
    }
}

/// Left edges of the text columns on a page: x positions (2 pt buckets) at
/// which at least three body lines start.
fn column_lefts(lines: &[Line], body_size: f32) -> Vec<f32> {
    let mut buckets: HashMap<i32, usize> = HashMap::new();
    for line in lines
        .iter()
        .filter(|l| (l.size - body_size).abs() < 1.0 && l.chars > 20)
    {
        *buckets
            .entry((line.bbox.x0 / 2.0).round() as i32)
            .or_default() += 1;
    }
    let mut lefts: Vec<f32> = buckets
        .into_iter()
        .filter(|(_, n)| *n >= 3)
        .map(|(b, _)| b as f32 * 2.0)
        .collect();
    lefts.sort_by(|a, b| a.total_cmp(b));
    // Merge neighbouring buckets.
    let mut merged: Vec<f32> = Vec::new();
    for x in lefts {
        if merged.last().is_none_or(|last| x - last > 6.0) {
            merged.push(x);
        }
    }
    merged
}

fn column_left_of(x0: f32, lefts: &[f32]) -> f32 {
    lefts
        .iter()
        .rev()
        .find(|&&l| l <= x0 + 1.5)
        .copied()
        .unwrap_or(x0)
}

fn segment_page(
    page: &PageLines,
    stats: &DocStats,
    in_references: &mut bool,
    out: &mut Vec<RawBlock>,
) {
    let body = stats.body_size;
    let lefts = column_lefts(&page.lines, body);
    let mut pending_tables: Vec<Option<&RawTable>> = page.tables.iter().map(Some).collect();
    let mut current: Option<RawBlock> = None;
    let in_table: Vec<bool> = page
        .lines
        .iter()
        .map(|l| {
            page.tables
                .iter()
                .any(|t| t.bbox.contains_point(l.bbox.center_x(), l.bbox.center_y()))
        })
        .collect();
    // Label–value pairs of a flattened form: each pair becomes one block, emitted at
    // the first line of its row in reading order; the row's lines are consumed.
    let rows = form_layout::form_rows(&layout_lines(&page.lines), &in_table);
    let mut consumed = vec![false; page.lines.len()];
    let mut anchored: HashMap<usize, Vec<usize>> = HashMap::new();
    for (r, row) in rows.iter().enumerate() {
        for &i in &row.lines {
            consumed[i] = true;
        }
        if let Some(&anchor) = row.lines.iter().min() {
            anchored.entry(anchor).or_default().push(r);
        }
    }

    let flush = |current: &mut Option<RawBlock>, out: &mut Vec<RawBlock>| {
        if let Some(block) = current.take() {
            out.push(block);
        }
    };

    for (index, line) in page.lines.iter().enumerate() {
        if consumed[index] {
            for &r in anchored.get(&index).map(Vec::as_slice).unwrap_or(&[]) {
                flush(&mut current, out);
                let row = &rows[r];
                let mut block = RawBlock::new(RawKind::FormField, line.clone(), page.number);
                block.bbox = row.bbox;
                block.field = Some((row.label.clone(), row.value.clone()));
                out.push(block);
            }
            continue;
        }
        // Lines inside a detected table are replaced by the table block,
        // emitted where the first of them appeared.
        if let Some(t_index) = table_of(&page.tables, line) {
            if let Some(table) = pending_tables[t_index].take() {
                flush(&mut current, out);
                let region: Vec<&Line> = page
                    .lines
                    .iter()
                    .filter(|l| table_of(&page.tables, l) == Some(t_index))
                    .collect();
                let coverage = cell_coverage(table, &region);
                let mut block = RawBlock::new(RawKind::Table, line.clone(), page.number);
                block.bbox = table.bbox;
                block.table = Some(table.clone());
                block.coverage = Some(coverage);
                out.push(block);
                if coverage < TABLE_COVERAGE_THRESHOLD {
                    // Nothing in the region may be lost from search: its text as printed
                    // follows the table.
                    let mut rows = region.clone();
                    rows.sort_by(|a, b| {
                        let same_row = (a.bbox.center_y() - b.bbox.center_y()).abs()
                            < 0.5 * a.size.min(b.size).max(1.0);
                        if same_row {
                            a.bbox.x0.total_cmp(&b.bbox.x0)
                        } else {
                            b.bbox.center_y().total_cmp(&a.bbox.center_y())
                        }
                    });
                    let mut lines = rows.into_iter().cloned();
                    if let Some(first) = lines.next() {
                        let mut raw = RawBlock::new(RawKind::TableText, first, page.number);
                        for l in lines {
                            raw.push(l);
                        }
                        out.push(raw);
                    }
                }
            }
            continue;
        }

        let text = line.text.trim();
        let cue = heading_cue(line, stats);
        let col_left = column_left_of(line.bbox.x0, &lefts);
        let display_math = is_math_line(text)
            && (line.bbox.x0 > col_left + 1.5 * line.size || RE_EQUATION_NUMBER.is_match(text));
        let footnote_like = line.size <= body * 0.88
            && line.bbox.y1 < page.height * 0.3
            && text.starts_with(|c: char| c.is_ascii_digit() || "*†‡§¶".contains(c));
        let line_kind = if let Some(cue) = cue {
            RawKind::Heading(cue)
        } else if line.mono && !*in_references && !RE_URL_OR_EMAIL.is_match(text) {
            RawKind::Code
        } else if display_math {
            RawKind::Equation
        } else if footnote_like {
            RawKind::Footnote
        } else if !*in_references && RE_LIST_START.is_match(text) {
            RawKind::ListItem
        } else {
            RawKind::Text
        };

        let Some(block) = current.as_mut() else {
            current = Some(RawBlock::new(line_kind, line.clone(), page.number));
            continue;
        };
        let prev = block.lines.last().cloned().unwrap_or_else(|| line.clone());
        let size = line.size;

        let starts_new = match (block.kind, line_kind) {
            // Consecutive heading lines of the same style form one wrapped heading.
            (RawKind::Heading(a), RawKind::Heading(b)) => {
                !(matches!(b, HeadingCue::Plain)
                    && (prev.size - size).abs() < 0.6
                    && prev.bold == line.bold
                    && prev.bbox.y0 - line.bbox.y1 < 0.8 * size
                    && matches!(a, HeadingCue::Plain | HeadingCue::Numbered(_)))
            }
            (RawKind::Heading(_), _) | (_, RawKind::Heading(_)) => true,
            // Footnotes continue as small text at the page bottom.
            (RawKind::Footnote, RawKind::Text) => size > body * 0.9,
            (RawKind::ListItem, RawKind::Text) => false,
            (a, b) => a != b,
        };

        let vertical_gap = prev.bbox.y0 - line.bbox.y1;
        let moved_up =
            line.bbox.y1 > prev.bbox.y0 + 0.5 * size && line.bbox.y0 > prev.bbox.y1 - 0.2 * size;
        let column_change = (line.bbox.x0 - prev.bbox.x0).abs() > page.width * 0.3;
        let size_change = (line.size - block.size).abs() > 0.9 && line_kind != RawKind::Equation;
        let big_gap = vertical_gap > (stats.leading - body).max(0.0) + 0.75 * body;
        let block_left = block.min_x0();
        let block_right = block.max_x1();
        let indent = line.bbox.x0 - block_left;
        // In a bibliography, indentation marks continuation lines (hanging
        // indent), so paragraph rules give way to the entry-start rule.
        let paragraph_indent = !*in_references
            && block.kind == RawKind::Text
            && indent > 0.8 * size
            && indent < 4.0 * size
            && (prev.bbox.x0 - block_left).abs() < 0.8 * size
            && prev.bbox.x1 > block_right - 2.0 * size;
        let previous_ended = !*in_references
            && prev.bbox.x1 < block_right - 4.0 * size
            && prev.text.trim_end().ends_with(['.', '!', '?', ':'])
            && block.kind == RawKind::Text;
        let special_start = RE_SPECIAL_START.is_match(text)
            || (line_kind == RawKind::ListItem && block.kind == RawKind::ListItem);
        let reference_start = *in_references
            && (RE_REF_ENTRY_START.is_match(text)
                || ((line.bbox.x0 - block_left).abs() < 0.5 * size
                    && prev.bbox.x0 > block_left + 0.5 * size)
                || (prev.bbox.x1 < block_right - 4.0 * size
                    && prev.text.trim_end().ends_with('.')
                    && (line.bbox.x0 - block_left).abs() < 0.5 * size));

        if starts_new
            || moved_up
            || column_change
            || size_change
            || big_gap
            || paragraph_indent
            || previous_ended
            || special_start
            || reference_start
        {
            flush(&mut current, out);
            current = Some(RawBlock::new(line_kind, line.clone(), page.number));
        } else {
            block.push(line.clone());
        }

        if let RawKind::Heading(_) = line_kind {
            *in_references = is_references_heading(text)
                || (*in_references
                    && !matches!(cue, Some(HeadingCue::Numbered(1)))
                    && !text.starts_with("Appendix"));
        }
    }
    flush(&mut current, out);
}

/// Assign heading levels: numbered headings take their numbering depth;
/// unnumbered headings take the level of numbered headings of the nearest
/// font size, or a rank by font size when the document has no numbering. The
/// largest heading on the first page, above the first numbered heading, is
/// the title.
fn assign_heading_levels(blocks: &mut [RawBlock], stats: &DocStats) {
    let mut size_level: Vec<(f32, u8)> = Vec::new();
    for block in blocks.iter() {
        if let RawKind::Heading(HeadingCue::Numbered(depth)) = block.kind {
            match size_level
                .iter_mut()
                .find(|(s, _)| (s - block.size).abs() < 0.6)
            {
                Some(entry) => entry.1 = entry.1.min(depth),
                None => size_level.push((block.size, depth)),
            }
        }
    }
    let mut plain_sizes: Vec<f32> = blocks
        .iter()
        .filter(|b| matches!(b.kind, RawKind::Heading(HeadingCue::Plain)))
        .map(|b| (b.size * 2.0).round() / 2.0)
        .collect();
    plain_sizes.sort_by(|a, b| b.total_cmp(a));
    plain_sizes.dedup();

    // Title: the largest heading of the document, on the first page, clearly
    // larger than body text.
    let largest = blocks
        .iter()
        .filter(|b| matches!(b.kind, RawKind::Heading(_)))
        .map(|b| b.size)
        .fold(0.0f32, f32::max);
    let title_index = blocks.iter().position(|b| {
        b.page == 1
            && matches!(b.kind, RawKind::Heading(_))
            && b.size >= stats.body_size * 1.4
            && (b.size - largest).abs() < 0.01
    });

    for (index, block) in blocks.iter_mut().enumerate() {
        match block.kind {
            RawKind::Heading(HeadingCue::Numbered(depth)) => block.level = depth,
            RawKind::Heading(HeadingCue::Plain) => {
                block.level = if size_level.is_empty() {
                    let rounded = (block.size * 2.0).round() / 2.0;
                    plain_sizes
                        .iter()
                        .position(|s| (s - rounded).abs() < 0.01)
                        .map(|p| (p as u8 + 1).min(6))
                        .unwrap_or(1)
                } else {
                    size_level
                        .iter()
                        .min_by(|a, b| {
                            (a.0 - block.size)
                                .abs()
                                .total_cmp(&(b.0 - block.size).abs())
                        })
                        .map(|(_, level)| *level)
                        .unwrap_or(1)
                };
            }
            _ => {}
        }
        if Some(index) == title_index {
            block.title = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x0: f32, y0: f32, x1: f32, size: f32, bold: bool) -> Line {
        Line {
            text: text.to_string(),
            bbox: BBox::new(x0, y0, x1, y0 + size),
            size,
            bold,
            mono: false,
            chars: text.chars().filter(|c| !c.is_whitespace()).count(),
            pieces: vec![(text.to_string(), BBox::new(x0, y0, x1, y0 + size))],
        }
    }

    #[test]
    fn header_lines_above_a_numeric_header_row_become_the_header() {
        let cell = |x0: f32, y0: f32, x1: f32, y1: f32| Some(BBox::new(x0, y0, x1, y1));
        let table = RawTable {
            bbox: BBox::new(100.0, 628.0, 250.0, 650.0),
            header: vec!["Mamba".into(), "98.6".into(), "61.4".into()],
            rows: vec![vec!["DeltaNet".into(), "96.8".into(), "98.8".into()]],
            cell_boxes: vec![
                vec![
                    cell(100.0, 640.0, 150.0, 650.0),
                    cell(160.0, 640.0, 200.0, 650.0),
                    cell(210.0, 640.0, 250.0, 650.0),
                ],
                vec![
                    cell(100.0, 628.0, 150.0, 638.0),
                    cell(160.0, 628.0, 200.0, 638.0),
                    cell(210.0, 628.0, 250.0, 638.0),
                ],
            ],
            from_model: false,
        };
        let mut page = PageLines {
            number: 1,
            width: 612.0,
            height: 792.0,
            lines: vec![
                // The caption above the header stops the search.
                line("Table 1: Recall.", 100.0, 662.0, 200.0, 8.0, false),
                line("Model SIFT1M GIST1M", 100.0, 652.0, 250.0, 8.0, false),
            ],
            tables: vec![table],
        };
        recover_table_headers(&mut page);
        let table = &page.tables[0];
        assert_eq!(table.header, vec!["Model", "SIFT1M", "GIST1M"]);
        assert_eq!(table.rows[0], vec!["Mamba", "98.6", "61.4"]);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.cell_boxes.len(), 3);
        assert!(table.cell_boxes[0].iter().all(Option::is_some));
        assert_eq!(table.bbox, BBox::new(100.0, 628.0, 250.0, 660.0));

        // A textual header row is left as it is.
        let mut page = PageLines {
            number: 1,
            width: 612.0,
            height: 792.0,
            lines: vec![line("Model SIFT1M GIST1M", 100.0, 652.0, 250.0, 8.0, false)],
            tables: vec![RawTable {
                bbox: BBox::new(100.0, 628.0, 250.0, 650.0),
                header: vec!["Method".into(), "R@10".into(), "QPS".into()],
                rows: vec![vec!["HNSW".into(), "95.3".into(), "1200".into()]],
                cell_boxes: vec![],
                from_model: false,
            }],
        };
        recover_table_headers(&mut page);
        assert_eq!(page.tables[0].header, vec!["Method", "R@10", "QPS"]);
        assert_eq!(page.tables[0].rows.len(), 1);
    }

    fn stats() -> DocStats {
        DocStats {
            body_size: 10.0,
            leading: 12.0,
        }
    }

    /// A page with one table region of four lines; the table holds `rows` of them.
    fn bill_page(rows: Vec<Vec<String>>) -> PageLines {
        PageLines {
            number: 2,
            width: 612.0,
            height: 792.0,
            lines: vec![
                line(
                    "Charges for the month follow.",
                    72.0,
                    700.0,
                    300.0,
                    10.0,
                    false,
                ),
                line("Item Amount", 72.0, 660.0, 300.0, 9.0, false),
                line("Rent 12,000", 72.0, 648.0, 300.0, 9.0, false),
                line("Water 250", 72.0, 636.0, 300.0, 9.0, false),
                line("Total 12,250", 72.0, 624.0, 300.0, 9.0, false),
                line(
                    "Payable within fifteen days.",
                    72.0,
                    590.0,
                    300.0,
                    10.0,
                    false,
                ),
            ],
            tables: vec![RawTable {
                bbox: BBox::new(70.0, 622.0, 302.0, 671.0),
                header: vec!["Item".into(), "Amount".into()],
                rows,
                cell_boxes: vec![],
                from_model: false,
            }],
        }
    }

    fn row(a: &str, b: &str) -> Vec<String> {
        vec![a.to_string(), b.to_string()]
    }

    #[test]
    fn a_table_that_misses_region_text_is_flagged_and_its_text_kept() {
        // The detector lost the "Water" row: 2 of the region's 8 tokens are missing.
        let page = bill_page(vec![row("Rent", "12,000"), row("Total", "12,250")]);
        let mut blocks = Vec::new();
        segment_page(&page, &stats(), &mut false, &mut blocks);
        let doc: Vec<Block> = blocks.into_iter().map(RawBlock::into_block).collect();
        let table = doc
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Table { .. }))
            .expect("table");
        assert!(table.table_incomplete());
        let BlockKind::Table { cell_coverage, .. } = table.kind else {
            unreachable!()
        };
        assert_eq!(cell_coverage, Some(0.75));
        let at = doc
            .iter()
            .position(|b| b.kind == BlockKind::TableText)
            .expect("region text");
        assert!(
            matches!(doc[at - 1].kind, BlockKind::Table { .. }),
            "right after its table"
        );
        assert_eq!(
            doc[at].text,
            "Item Amount\nRent 12,000\nWater 250\nTotal 12,250"
        );
        assert_eq!(doc[at].page, Some(2));
        // Text outside the region is untouched.
        assert!(doc.iter().any(|b| b.text == "Payable within fifteen days."));

        // A table holding its whole region is complete and has no extra block.
        let page = bill_page(vec![
            row("Rent", "12,000"),
            row("Water", "250"),
            row("Total", "12,250"),
        ]);
        let mut blocks = Vec::new();
        segment_page(&page, &stats(), &mut false, &mut blocks);
        let doc: Vec<Block> = blocks.into_iter().map(RawBlock::into_block).collect();
        assert!(doc.iter().all(|b| !b.table_incomplete()));
        assert!(!doc.iter().any(|b| b.kind == BlockKind::TableText));
    }

    #[test]
    fn math_lines_are_detected_but_prose_with_symbols_is_not() {
        assert!(is_math_line(
            "S t = S t−1 + β t ( v t − S t−1 k t ) k ⊤ t (3)"
        ));
        assert!(is_math_line("o t = ∑ i v i"));
        assert!(!is_math_line(
            "where the state S is updated with learning rate β at every step."
        ));
    }

    #[test]
    fn heading_cues() {
        let s = stats();
        assert_eq!(
            heading_cue(
                &line(
                    "3.2 Chunkwise Parallel Form",
                    72.0,
                    500.0,
                    300.0,
                    10.0,
                    true
                ),
                &s
            ),
            Some(HeadingCue::Numbered(2))
        );
        assert_eq!(
            heading_cue(&line("Introduction", 72.0, 500.0, 160.0, 12.0, false), &s),
            Some(HeadingCue::Plain)
        );
        assert_eq!(
            heading_cue(&line("References", 72.0, 500.0, 160.0, 10.0, true), &s),
            Some(HeadingCue::Plain)
        );
        // Body-size, non-bold numbered line is a list, not a heading.
        assert_eq!(
            heading_cue(
                &line("1 The model is trained", 72.0, 500.0, 300.0, 10.0, false),
                &s
            ),
            None
        );
        // Bold run-in paragraph lead is not a heading.
        assert_eq!(
            heading_cue(
                &line(
                    "Efficient training. Let Q, K, V be the stacked query vectors and",
                    72.0,
                    500.0,
                    500.0,
                    10.0,
                    true
                ),
                &s
            ),
            None
        );
        assert_eq!(
            heading_cue(
                &line("Theorem 1. Let x be", 72.0, 500.0, 300.0, 12.0, true),
                &s
            ),
            None
        );
        // Multi-byte characters right after the number must not panic.
        assert_eq!(
            heading_cue(&line("1 ′ prime", 72.0, 500.0, 300.0, 10.0, false), &s),
            None
        );
        assert_eq!(
            heading_cue(
                &line("II. RELATED WORK", 72.0, 500.0, 300.0, 9.0, false),
                &s
            ),
            Some(HeadingCue::Numbered(1))
        );
        // Author lists are not headings even when large.
        assert_eq!(
            heading_cue(
                &line("Ann Lee, Bo Chen, Cy Diaz", 72.0, 500.0, 300.0, 12.0, false),
                &s
            ),
            None
        );
    }

    #[test]
    fn runs_on_one_baseline_join_and_backward_jumps_split() {
        let run = |t: &str, x0: f32, y0: f32, x1: f32| Run {
            text: t.to_string(),
            bbox: BBox::new(x0, y0, x1, y0 + 10.0),
            size: 10.0,
            bold: false,
            mono: false,
        };
        let lines = build_lines(vec![
            run("Hello", 72.0, 700.0, 100.0),
            run("world", 102.0, 700.0, 130.0),
            run("next", 72.0, 688.0, 95.0),
            // Right column starts at the top again: new line.
            run("right", 320.0, 700.0, 350.0),
        ]);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["Hello world", "next", "right"]);
    }

    #[test]
    fn dehyphenates_and_joins_lines() {
        let mut block = RawBlock::new(
            RawKind::Text,
            line("efficient hard-", 72.0, 700.0, 300.0, 10.0, false),
            1,
        );
        block.push(line("ware training", 72.0, 688.0, 200.0, 10.0, false));
        assert_eq!(block.joined_text(), "efficient hardware training");
    }

    #[test]
    fn model_cell_spacing_is_tidied() {
        assert_eq!(tidy_cell_text("DeltaNet [ − 1 , 1]"), "DeltaNet [−1, 1]");
        assert_eq!(tidy_cell_text("Mamba ( w. conv )"), "Mamba (w. conv)");
        assert_eq!(tidy_cell_text("Moneta -H"), "Moneta-H");
        assert_eq!(tidy_cell_text("w. L 1 -norm &1+ELU"), "w. L 1-norm &1+ELU");
        assert_eq!(tidy_cell_text("Transformer -"), "Transformer -");
        assert_eq!(tidy_cell_text("− 0.68"), "−0.68");
    }

    #[test]
    fn detached_signs_join_their_numbers() {
        assert_eq!(join_detached_sign("− 0.68"), "−0.68");
        assert_eq!(join_detached_sign("+ 1.2"), "+1.2");
        assert_eq!(join_detached_sign("+ learnable W 0"), "+ learnable W 0");
        assert_eq!(join_detached_sign("15.91"), "15.91");
    }

    #[test]
    fn bold_detection_uses_font_names() {
        assert!(font_is_bold("ABCDEF+CMBX10"));
        assert!(font_is_bold("XYZ+NimbusRomNo9L-Medi"));
        assert!(font_is_bold("Arial-BoldMT"));
        assert!(!font_is_bold("ABCDEF+CMR10"));
        assert!(!font_is_bold("NimbusRomNo9L-Regu"));
        assert!(font_is_mono("ABC+CMTT10"));
    }
}

#[cfg(test)]
mod generated_pdf_tests {
    use super::super::pdf_fixtures::{bold, build_pdf, column, text, Text};
    use super::*;

    fn parse(pages: &[Vec<Text>]) -> StructuredDocument {
        parse_pdf_layout(&build_pdf(pages, None)).expect("layout")
    }

    fn texts_of<'a>(doc: &'a StructuredDocument, kind: &str) -> Vec<&'a str> {
        doc.blocks
            .iter()
            .filter(|b| b.kind.name() == kind)
            .map(|b| b.text.as_str())
            .collect()
    }

    #[test]
    fn two_column_page_reads_left_column_before_right() {
        // Content stream interleaves the columns line by line.
        let left = column(72.0, 700.0, "LEFT", 8);
        let right = column(320.0, 700.0, "RIGHT", 8);
        let mut page = Vec::new();
        for (l, r) in left.into_iter().zip(right) {
            page.push(l);
            page.push(r);
        }
        let doc = parse(&[page]);
        let all = doc.plain_text();
        let last_left = all.rfind("LEFT line 7").expect("left text present");
        let first_right = all.find("RIGHT line 0").expect("right text present");
        assert!(last_left < first_right, "columns interleaved: {all}");
        assert!(
            doc.blocks
                .iter()
                .all(|b| !(b.text.contains("LEFT") && b.text.contains("RIGHT"))),
            "a block mixes both columns: {all}"
        );
    }

    /// A printed return: codes, labels and amounts in separate columns, drawn column
    /// by column (labels first, then amounts), as tax portals generate them.
    fn printed_return() -> Vec<Text> {
        let rows = [
            ("B1", "Gross salary (ia + ib)", "i", "9,10,000"),
            ("a", "Salary as per section 17(1)", "ia", "9,10,000"),
            ("b", "Value of perquisites", "ib", "0"),
            ("B2", "Standard deduction u/s 16", "ii", "75,000"),
            ("B3", "Income chargeable under Salaries", "iii", "8,35,000"),
        ];
        let mut page = vec![bold(40.0, 740.0, 10.0, "PART B GROSS TOTAL INCOME")];
        for (i, (code, ..)) in rows.iter().enumerate() {
            page.push(text(40.0, 700.0 - 20.0 * i as f32, 9.0, code));
        }
        for (i, (_, label, ..)) in rows.iter().enumerate() {
            page.push(text(90.0, 700.0 - 20.0 * i as f32, 9.0, label));
        }
        for (i, (.., line_ref, amount)) in rows.iter().enumerate() {
            page.push(text(420.0, 700.0 - 20.0 * i as f32, 9.0, line_ref));
            page.push(text(500.0, 700.0 - 20.0 * i as f32, 9.0, amount));
        }
        page
    }

    #[test]
    fn printed_form_rows_become_label_value_fields_with_boxes() {
        let doc = parse(&[printed_return()]);
        let fields: Vec<(&str, &str, Option<BBox>)> = doc
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::FormField {
                    label,
                    value,
                    field,
                } => {
                    assert_eq!(*field, FieldKind::Layout);
                    Some((label.as_str(), value.as_str(), b.bbox))
                }
                _ => None,
            })
            .collect();
        assert_eq!(fields.len(), 5, "{fields:?}");
        assert_eq!(fields[0].0, "B1 Gross salary (ia + ib) i");
        assert_eq!(fields[0].1, "9,10,000");
        assert_eq!(fields[3].1, "75,000");
        // The field's box spans its row from the code to the amount.
        let first = fields[0].2.expect("box");
        assert!(first.x0 <= 41.0 && first.x1 >= 520.0, "{first:?}");
        assert!(doc.blocks.iter().all(|b| b.page == Some(1)));
        // No amount is left as a stray paragraph.
        assert!(!doc
            .blocks
            .iter()
            .any(|b| b.kind == BlockKind::Paragraph && b.text.contains("9,10,000")));
        let rendered = doc.plain_text();
        assert!(
            rendered.contains("B2 Standard deduction u/s 16 ii: 75,000"),
            "{rendered}"
        );
    }

    #[test]
    fn two_column_prose_and_numeric_tables_get_no_form_fields() {
        let left = column(72.0, 700.0, "LEFT", 12);
        let right = column(320.0, 700.0, "RIGHT", 12);
        let doc = parse(&[left.into_iter().chain(right).collect()]);
        assert!(texts_of(&doc, "form_field").is_empty());
        let mut table = vec![text(72.0, 720.0, 10.0, "Results of the indexes follow.")];
        for (i, (m, a, b)) in [
            ("HNSW", "95.3", "96.1"),
            ("IVF-PQ", "88.0", "91.4"),
            ("Flat", "99.9", "99.9"),
            ("LSH", "71.2", "75.0"),
        ]
        .iter()
        .enumerate()
        {
            let y = 690.0 - 14.0 * i as f32;
            table.extend([
                text(72.0, y, 9.0, m),
                text(200.0, y, 9.0, a),
                text(260.0, y, 9.0, b),
            ]);
        }
        let doc = parse(&[table]);
        assert!(texts_of(&doc, "form_field").is_empty());
    }

    #[test]
    fn headings_get_levels_titles_and_section_paths() {
        let mut page = vec![bold(72.0, 740.0, 18.0, "A Study of Delta Rules")];
        page.push(bold(72.0, 700.0, 12.0, "1 Introduction"));
        page.extend(column(72.0, 684.0, "intro", 4));
        page.push(bold(72.0, 620.0, 10.0, "1.1 Background"));
        page.extend(column(72.0, 604.0, "background", 4));
        page.push(bold(72.0, 540.0, 12.0, "2 Method"));
        page.extend(column(72.0, 524.0, "method", 4));
        let doc = parse(&[page]);
        assert_eq!(texts_of(&doc, "title"), vec!["A Study of Delta Rules"]);
        let headings: Vec<(String, u8)> = doc
            .blocks
            .iter()
            .filter_map(|b| match b.kind {
                BlockKind::Heading { level } => Some((b.text.clone(), level)),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            vec![
                ("1 Introduction".to_string(), 1),
                ("1.1 Background".to_string(), 2),
                ("2 Method".to_string(), 1)
            ]
        );
        let background = doc
            .blocks
            .iter()
            .find(|b| b.text.contains("background line 0"))
            .expect("background paragraph");
        assert_eq!(
            background.section_path,
            vec!["1 Introduction", "1.1 Background"]
        );
        assert_eq!(background.page, Some(1));
        // Bottom-left origin: the paragraph sits below its heading (y = 620).
        let bbox = background.bbox.expect("bbox");
        assert!(bbox.y1 <= 620.0 && bbox.y0 > 540.0, "{bbox:?}");
        assert!((bbox.x0 - 72.0).abs() < 2.0);
    }

    #[test]
    fn display_equation_is_its_own_block() {
        let mut page = column(72.0, 700.0, "before", 4);
        page.push(text(200.0, 640.0, 10.0, "S = S + b (v - S k) k + = (3)"));
        page.extend(column(72.0, 620.0, "after", 4));
        let doc = parse(&[page]);
        let equations = texts_of(&doc, "equation");
        assert_eq!(equations.len(), 1, "{:?}", doc.blocks);
        assert!(equations[0].starts_with("S = S + b"));
    }

    #[test]
    fn bibliography_entries_are_split_with_hanging_continuations() {
        let mut page = column(72.0, 700.0, "body", 4);
        page.push(bold(72.0, 640.0, 12.0, "References"));
        page.push(text(
            72.0,
            620.0,
            9.0,
            "[1] A. Author and B. Writer. A long title about linear",
        ));
        page.push(text(
            86.0,
            609.0,
            9.0,
            "attention and delta rules. In Proceedings, 2020.",
        ));
        page.push(text(
            72.0,
            596.0,
            9.0,
            "[2] C. Person. Another paper on fast weights. 2021.",
        ));
        page.push(text(
            72.0,
            583.0,
            9.0,
            "[3] D. Scholar. Third entry on test-time training and",
        ));
        page.push(text(86.0, 572.0, 9.0, "memory. Journal of Things, 2022."));
        let doc = parse(&[page]);
        let refs = texts_of(&doc, "reference_entry");
        assert_eq!(refs.len(), 3, "{refs:?}");
        assert!(refs[0].ends_with("In Proceedings, 2020."));
        assert!(refs[2].starts_with("[3] D. Scholar"));
    }

    #[test]
    fn running_headers_and_page_numbers_are_dropped() {
        let pages: Vec<Vec<Text>> = (1..=3)
            .map(|n| {
                let mut page = vec![text(72.0, 760.0, 9.0, "Preprint under review")];
                page.extend(column(72.0, 700.0, &format!("page{n}"), 5));
                page.push(text(300.0, 30.0, 9.0, &n.to_string()));
                page
            })
            .collect();
        let doc = parse(&pages);
        let all = doc.plain_text();
        assert!(!all.contains("Preprint under review"), "{all}");
        assert!(doc.blocks.iter().all(|b| b.text.trim() != "2"));
        assert_eq!(doc.pages.len(), 3);
        assert!(doc.blocks.iter().any(|b| b.page == Some(3)));
    }

    #[test]
    fn a_running_header_that_carries_data_is_kept_once() {
        let pages: Vec<Vec<Text>> = (1..=4)
            .map(|n| {
                let mut page = vec![text(
                    40.0,
                    770.0,
                    8.0,
                    "Acknowledgement Number: 990011223344556 Date of filing: 30-Jul-2026",
                )];
                page.extend(column(72.0, 700.0, &format!("page{n}"), 5));
                page.push(text(300.0, 30.0, 9.0, &format!("Page {n} of 4")));
                page
            })
            .collect();
        let doc = parse(&pages);
        let headers: Vec<&Block> = doc
            .blocks
            .iter()
            .filter(|b| b.text.contains("990011223344556"))
            .collect();
        assert_eq!(headers.len(), 1, "kept exactly once");
        assert_eq!(headers[0].page, Some(1));
        assert!(!doc.plain_text().contains("Page 2 of 4"));
    }

    #[test]
    fn figure_and_theorem_blocks_are_classified() {
        let mut page = column(72.0, 700.0, "intro", 4);
        page.push(text(
            72.0,
            640.0,
            10.0,
            "Theorem 1 (Capacity). The delta rule state has rank at most d.",
        ));
        page.extend(column(72.0, 620.0, "mid", 3));
        page.push(text(
            72.0,
            570.0,
            9.0,
            "Figure 2: Throughput of the chunkwise form.",
        ));
        let doc = parse(&[page]);
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(&b.kind, BlockKind::Theorem { label } if label == "Theorem 1")));
        assert!(doc.blocks.iter().any(
            |b| matches!(&b.kind, BlockKind::Figure { caption } if caption.starts_with("Figure 2:"))
        ));
    }

    #[test]
    fn table_candidate_pages_are_flagged_by_their_cues() {
        let prose = column(72.0, 700.0, "prose", 10);
        let mut numeric = column(72.0, 700.0, "intro", 3);
        for (i, (name, a, b)) in [
            ("HNSW", "95.3", "88.1"),
            ("IVF", "71.4", "60.2"),
            ("PQ", "65.0", "52.9"),
            ("Flat", "99.9", "99.1"),
        ]
        .into_iter()
        .enumerate()
        {
            let y = 600.0 - 12.0 * i as f32;
            numeric.push(text(72.0, y, 9.0, name));
            numeric.push(text(200.0, y, 9.0, a));
            numeric.push(text(260.0, y, 9.0, b));
        }
        let mut captioned = column(72.0, 700.0, "body", 3);
        captioned.push(text(72.0, 600.0, 9.0, "Table 2: Recall of the indexes."));
        let parsed = parse_pdf_layout_with(
            &build_pdf(&[prose, numeric, captioned], None),
            TableMode::Candidates,
        )
        .expect("layout");
        let cues: Vec<(u32, Vec<CandidateCue>)> = parsed
            .tables
            .candidates
            .iter()
            .map(|c| (c.page, c.cues.clone()))
            .collect();
        assert_eq!(cues[0].0, 2, "{cues:?}");
        assert!(
            cues[0].1.contains(&CandidateCue::NumericCluster),
            "{cues:?}"
        );
        assert!(
            cues[0].1.contains(&CandidateCue::AlignedColumns),
            "{cues:?}"
        );
        assert_eq!(cues[1].0, 3, "{cues:?}");
        assert!(cues[1].1.contains(&CandidateCue::Caption), "{cues:?}");
        assert_eq!(cues.len(), 2, "the prose page is not a candidate: {cues:?}");
        // The plain parser reports no candidates.
        let plain = parse_pdf_layout_with(
            &build_pdf(&[column(72.0, 700.0, "x", 3)], None),
            TableMode::Heuristic,
        )
        .expect("layout");
        assert!(plain.tables.candidates.is_empty());
    }

    #[test]
    fn page_without_text_layer_has_no_text() {
        let doc = parse(&[Vec::new(), Vec::new()]);
        assert_eq!(doc.pages.len(), 2);
        assert_eq!(doc.text_chars(), 0);
    }
}
