//! Turning predicted table cells (anchors with row and column spans and header tags)
//! into the document model's table: one header row and body rows of equal width, a box
//! per position, and the units the headers state.
//!
//! - **Spans.** A spanning header cell names every column under it, so its text is
//!   repeated across its span (a merged dataset header over `R@10 | QPS` belongs to
//!   both). A spanning body cell is repeated down and across only when it is a label
//!   (row header or text): a value is kept once at its anchor, because repeating a
//!   number would report it several times.
//! - **Multi-row headers.** The leading rows the model tags as column headers are
//!   flattened per column into one header, top-down, as `Parent / Child`; a parent
//!   repeated by its own vertical span is written once.
//! - **Section rows.** A full-width group label (`srow`) keeps its text in the first
//!   column and leaves the others empty, so downstream readers treat it as a group
//!   heading over the rows below it.
//! - **Units.** A unit stated in a flattened header (`Latency (ms)`, `Acc. [%]`,
//!   `Error in %`) is recorded per column.
//!
//! Pure functions; the model that produced the cells is in [`super::table_model`].

use std::sync::LazyLock;

use regex::Regex;

use super::document_model::{collapse_ws, is_numeric_cell, BBox};
use super::table_model::ModelCell;

/// Widest or tallest span honoured (guards against corrupt predictions).
const MAX_SPAN: usize = 64;
/// Most rows or columns of a resolved table.
const MAX_DIM: usize = 512;
/// Separator between the levels of a flattened multi-row header.
pub const HEADER_LEVEL_SEPARATOR: &str = " / ";

/// A resolved table.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTable {
    /// One flattened header per column.
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Header row first, then one row per body row, aligned with the cells.
    pub cell_boxes: Vec<Vec<Option<BBox>>>,
    /// Unit stated in each column's header, if any.
    pub units: Vec<Option<String>>,
    /// How many grid rows the header was flattened from.
    pub header_rows: usize,
}

#[derive(Debug, Clone)]
struct Slot {
    text: String,
    bbox: Option<BBox>,
    header: bool,
    section: bool,
    /// Whether this slot is the cell's anchor (top-left of its span).
    anchor: bool,
}

/// Resolves model cells into a table. `None` when the cells do not make a table of at
/// least two columns and one body row.
pub fn resolve_cells(cells: &[ModelCell]) -> Option<ResolvedTable> {
    let height = cells
        .iter()
        .map(|c| c.row + c.row_span.clamp(1, MAX_SPAN))
        .max()?
        .min(MAX_DIM);
    let width = cells
        .iter()
        .map(|c| c.col + c.col_span.clamp(1, MAX_SPAN))
        .max()?
        .min(MAX_DIM);
    if width < 2 || height < 2 {
        return None;
    }
    let mut grid: Vec<Vec<Option<Slot>>> = vec![vec![None; width]; height];
    for cell in cells {
        if cell.row >= height || cell.col >= width {
            continue;
        }
        let text = collapse_ws(&cell.text);
        let row_end = (cell.row + cell.row_span.clamp(1, MAX_SPAN)).min(height);
        let col_end = (cell.col + cell.col_span.clamp(1, MAX_SPAN)).min(width);
        for (r, row) in grid.iter_mut().enumerate().take(row_end).skip(cell.row) {
            for (c, slot) in row.iter_mut().enumerate().take(col_end).skip(cell.col) {
                // A later cell never overwrites an anchor already placed.
                if slot.as_ref().is_some_and(|s| s.anchor) {
                    continue;
                }
                *slot = Some(Slot {
                    text: text.clone(),
                    bbox: cell.bbox,
                    header: cell.column_header,
                    section: cell.row_section,
                    anchor: r == cell.row && c == cell.col,
                });
            }
        }
    }

    // Header rows: the leading rows whose filled positions are all column headers.
    // Without any header tag, the first row is the header.
    let mut header_rows = 0;
    for row in &grid {
        let filled: Vec<&Slot> = row
            .iter()
            .flatten()
            .filter(|s| !s.text.is_empty())
            .collect();
        if !filled.is_empty() && filled.iter().all(|s| s.header) {
            header_rows += 1;
        } else {
            break;
        }
    }
    let header_rows = header_rows.clamp(1, height - 1);

    let mut header = Vec::with_capacity(width);
    let mut header_boxes = Vec::with_capacity(width);
    for column in 0..width {
        let mut parts: Vec<&str> = Vec::new();
        let mut bbox: Option<BBox> = None;
        for row in grid.iter().take(header_rows) {
            let Some(slot) = &row[column] else { continue };
            if slot.text.is_empty() {
                continue;
            }
            if parts.last() != Some(&slot.text.as_str()) {
                parts.push(&slot.text);
            }
            // The lowest level's box locates the column's header.
            bbox = slot.bbox.or(bbox);
        }
        header.push(parts.join(HEADER_LEVEL_SEPARATOR));
        header_boxes.push(bbox);
    }

    let mut rows = Vec::with_capacity(height - header_rows);
    let mut cell_boxes = vec![header_boxes];
    for row in grid.iter().skip(header_rows) {
        let section = row.iter().flatten().any(|s| s.section);
        let mut texts = Vec::with_capacity(width);
        let mut boxes = Vec::with_capacity(width);
        for (column, slot) in row.iter().enumerate() {
            let Some(slot) = slot else {
                texts.push(String::new());
                boxes.push(None);
                continue;
            };
            let keep = if section {
                column == 0
            } else {
                slot.anchor || !is_value(&slot.text)
            };
            texts.push(if keep { slot.text.clone() } else { String::new() });
            boxes.push(slot.bbox);
        }
        if section && texts.first().is_some_and(String::is_empty) {
            if let Some(label) = row.iter().flatten().find(|s| !s.text.is_empty()) {
                texts[0] = label.text.clone();
            }
        }
        rows.push(texts);
        cell_boxes.push(boxes);
    }
    // Drop columns that are empty in the header and every row.
    let keep: Vec<bool> = (0..width)
        .map(|c| !header[c].is_empty() || rows.iter().any(|r| !r[c].is_empty()))
        .collect();
    let project = |v: &mut Vec<String>| {
        let mut i = 0;
        v.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    };
    project(&mut header);
    for row in &mut rows {
        project(row);
    }
    for row in &mut cell_boxes {
        let mut i = 0;
        row.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    }
    rows.retain(|r| r.iter().any(|c| !c.is_empty()));
    if header.len() < 2 || rows.is_empty() {
        return None;
    }
    let units = header.iter().map(|h| header_unit(h)).collect();
    Some(ResolvedTable {
        header,
        rows,
        cell_boxes,
        units,
        header_rows,
    })
}

/// Whether a cell holds a value (a number) rather than a label.
fn is_value(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && (is_numeric_cell(t) || !t.chars().any(char::is_alphabetic))
}

/// The unit a header states: in brackets (`(ms)`, `[%]`, `(×)`), after `in`
/// (`Error in %`, `Time in s`), or a lone `%` column.
pub fn header_unit(header: &str) -> Option<String> {
    static UNIT: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r"(?:[(\[]\s*(%|ms|s|sec|µs|us|ns|min|h|mJ|J|kJ|W|kWh|GB|MB|KB|TB|GiB|MiB|K|M|B|x|×|FLOPs|GFLOPs|TFLOPs|tokens/s|it/s)\s*[)\]]|\bin\s+(%|ms|s|mJ|J|GB|MB)(?:$|[^\w]))",
        )
        .ok()
    });
    let caps = UNIT.as_ref()?.captures(header)?;
    caps.get(1).or_else(|| caps.get(2)).map(|m| match m.as_str() {
        "×" => "x".to_string(),
        "us" => "µs".to_string(),
        "sec" => "s".to_string(),
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(row: usize, col: usize, rs: usize, cs: usize, text: &str, header: bool) -> ModelCell {
        ModelCell {
            text: text.to_string(),
            bbox: Some(BBox::new(
                col as f32 * 50.0,
                700.0 - row as f32 * 12.0,
                col as f32 * 50.0 + 45.0,
                710.0 - row as f32 * 12.0,
            )),
            row,
            col,
            row_span: rs,
            col_span: cs,
            column_header: header,
            row_header: false,
            row_section: false,
        }
    }

    #[test]
    fn merged_headers_expand_to_child_columns_and_flatten() {
        // Model | SIFT1M (R@10, QPS) | GIST1M (R@10)
        let cells = vec![
            cell(0, 0, 2, 1, "Method", true),
            cell(0, 1, 1, 2, "SIFT1M", true),
            cell(0, 3, 1, 1, "GIST1M", true),
            cell(1, 1, 1, 1, "R@10", true),
            cell(1, 2, 1, 1, "QPS", true),
            cell(1, 3, 1, 1, "R@10", true),
            cell(2, 0, 1, 1, "HNSW", false),
            cell(2, 1, 1, 1, "95.3", false),
            cell(2, 2, 1, 1, "12,400", false),
            cell(2, 3, 1, 1, "88.1", false),
        ];
        let t = resolve_cells(&cells).unwrap();
        assert_eq!(t.header_rows, 2);
        assert_eq!(
            t.header,
            vec!["Method", "SIFT1M / R@10", "SIFT1M / QPS", "GIST1M / R@10"]
        );
        assert_eq!(t.rows, vec![vec!["HNSW", "95.3", "12,400", "88.1"]]);
        assert_eq!(t.cell_boxes.len(), 2);
        // The header box of a child column is the child's own box.
        assert_eq!(t.cell_boxes[0][2], cells[4].bbox);
    }

    #[test]
    fn spanning_values_are_kept_once_and_labels_repeat() {
        let cells = vec![
            cell(0, 0, 1, 1, "Model", true),
            cell(0, 1, 1, 1, "Size", true),
            cell(0, 2, 1, 1, "PIQA acc", true),
            // A label spanning two rows names both.
            cell(1, 0, 2, 1, "DeltaNet", false),
            cell(1, 1, 1, 1, "340M", false),
            cell(2, 1, 1, 1, "1.3B", false),
            // A value spanning two rows is reported once.
            cell(1, 2, 2, 1, "70.1", false),
            cell(3, 0, 1, 1, "GLA", false),
            cell(3, 1, 1, 2, "64.0", false),
        ];
        let t = resolve_cells(&cells).unwrap();
        assert_eq!(
            t.rows,
            vec![
                vec!["DeltaNet", "340M", "70.1"],
                vec!["DeltaNet", "1.3B", ""],
                vec!["GLA", "64.0", ""],
            ]
        );
    }

    #[test]
    fn section_rows_become_group_labels() {
        let mut section = cell(1, 0, 1, 3, "Small models", false);
        section.row_section = true;
        let cells = vec![
            cell(0, 0, 1, 1, "Model", true),
            cell(0, 1, 1, 1, "Wiki. ppl", true),
            cell(0, 2, 1, 1, "LMB. acc", true),
            section,
            cell(2, 0, 1, 1, "GLA", false),
            cell(2, 1, 1, 1, "28.39", false),
            cell(2, 2, 1, 1, "31.0", false),
        ];
        let t = resolve_cells(&cells).unwrap();
        assert_eq!(t.rows[0], vec!["Small models", "", ""]);
        assert_eq!(t.rows[1], vec!["GLA", "28.39", "31.0"]);
    }

    #[test]
    fn untagged_tables_take_the_first_row_as_header_and_empty_columns_go() {
        let cells = vec![
            cell(0, 0, 1, 1, "Config", false),
            cell(0, 2, 1, 1, "Latency (ms)", false),
            cell(1, 0, 1, 1, "A", false),
            cell(1, 2, 1, 1, "4.2", false),
        ];
        let t = resolve_cells(&cells).unwrap();
        assert_eq!(t.header, vec!["Config", "Latency (ms)"]);
        assert_eq!(t.units, vec![None, Some("ms".to_string())]);
        assert_eq!(t.cell_boxes[1].len(), 2);
        assert!(resolve_cells(&[cell(0, 0, 1, 1, "x", true)]).is_none());
    }

    #[test]
    fn units_are_read_from_headers() {
        assert_eq!(header_unit("Accuracy (%)").as_deref(), Some("%"));
        assert_eq!(header_unit("Latency [ms]").as_deref(), Some("ms"));
        assert_eq!(header_unit("Error in %").as_deref(), Some("%"));
        assert_eq!(header_unit("Speedup (×)").as_deref(), Some("x"));
        assert_eq!(header_unit("Throughput (tokens/s)").as_deref(), Some("tokens/s"));
        assert_eq!(header_unit("Energy (mJ)").as_deref(), Some("mJ"));
        assert_eq!(header_unit("Recall@10"), None);
        assert_eq!(header_unit("Trained in Books"), None);
    }
}
