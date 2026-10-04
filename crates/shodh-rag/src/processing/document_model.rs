//! Structured document model produced by the parsers and consumed by the
//! structure-aware chunker.
//!
//! A [`StructuredDocument`] is an ordered list of [`Block`]s — semantic units
//! such as a heading, a paragraph, a table with its header, a theorem, an
//! equation or one bibliography entry — each carrying the page it came from,
//! its bounding box and the chain of headings it sits under.
//!
//! Coordinates: [`BBox`] is in PDF user-space points with the origin at the
//! bottom-left of the page (y grows upwards), the convention of the PDF
//! specification and of pdf.js' `PDFPageProxy` coordinate space. Pages are
//! 1-based.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Axis-aligned rectangle in PDF points, bottom-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BBox {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }

    pub fn union(&self, other: &BBox) -> BBox {
        BBox {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn center_y(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }

    pub fn center_x(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }

    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// The same box with coordinates rounded to 0.1 pt, for compact storage.
    pub fn rounded(&self) -> BBox {
        let r = |v: f32| (v * 10.0).round() / 10.0;
        BBox {
            x0: r(self.x0),
            y0: r(self.y0),
            x1: r(self.x1),
            y1: r(self.y1),
        }
    }
}

/// What a block is. Serialized with a `kind` tag in snake_case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockKind {
    /// The document title (first page, largest type). Not part of section paths.
    Title,
    /// A section heading; level 1 is a top-level section.
    Heading {
        level: u8,
    },
    Paragraph,
    ListItem,
    /// A table. `header` is the header row; `rows` are body rows.
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        caption: Option<String>,
        /// Box of every cell, header row first, aligned with `header` and `rows`
        /// (`None` where the parser has no box). Empty when the source has no layout
        /// (text, LaTeX and Markdown tables).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cell_boxes: Vec<Vec<Option<BBox>>>,
    },
    /// A figure, represented by its caption.
    Figure {
        caption: String,
    },
    /// A display equation (kept as extracted text).
    Equation,
    Code,
    /// Theorem-like statement: theorem, lemma, proposition, corollary, claim,
    /// conjecture, assumption, remark or example. `label` is e.g. `"Lemma 3.2"`.
    Theorem {
        label: String,
    },
    Definition {
        label: String,
    },
    /// The start of a proof.
    Proof,
    /// One bibliography entry.
    ReferenceEntry,
    Footnote,
}

impl BlockKind {
    /// Stable snake_case name, as stored in chunk metadata.
    pub fn name(&self) -> &'static str {
        match self {
            BlockKind::Title => "title",
            BlockKind::Heading { .. } => "heading",
            BlockKind::Paragraph => "paragraph",
            BlockKind::ListItem => "list_item",
            BlockKind::Table { .. } => "table",
            BlockKind::Figure { .. } => "figure",
            BlockKind::Equation => "equation",
            BlockKind::Code => "code",
            BlockKind::Theorem { .. } => "theorem",
            BlockKind::Definition { .. } => "definition",
            BlockKind::Proof => "proof",
            BlockKind::ReferenceEntry => "reference_entry",
            BlockKind::Footnote => "footnote",
        }
    }
}

/// One semantic unit of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    #[serde(flatten)]
    pub kind: BlockKind,
    pub text: String,
    /// 1-based page, `None` for unpaged sources (plain text, LaTeX, Markdown).
    pub page: Option<u32>,
    pub bbox: Option<BBox>,
    /// Heading chain this block sits under, outermost first.
    pub section_path: Vec<String>,
}

impl Block {
    pub fn new(kind: BlockKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            page: None,
            bbox: None,
            section_path: Vec::new(),
        }
    }

    pub fn on_page(mut self, page: u32, bbox: Option<BBox>) -> Self {
        self.page = Some(page);
        self.bbox = bbox;
        self
    }

    /// Text used for retrieval: tables are rendered as Markdown with their
    /// caption, figures as their caption, everything else verbatim.
    pub fn render(&self) -> String {
        match &self.kind {
            BlockKind::Table {
                header,
                rows,
                caption,
                ..
            } => render_table(caption.as_deref(), header, rows),
            BlockKind::Figure { caption } => caption.clone(),
            _ => self.text.clone(),
        }
    }
}

/// Render a table (or a slice of its rows) as a Markdown table, preceded by
/// its caption when present.
pub fn render_table(caption: Option<&str>, header: &[String], rows: &[Vec<String>]) -> String {
    let clean = |s: &str| s.replace('|', "\\|").replace('\n', " ").trim().to_string();
    let mut out = String::new();
    if let Some(caption) = caption.filter(|c| !c.trim().is_empty()) {
        out.push_str(caption.trim());
        out.push('\n');
    }
    let width = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if width == 0 {
        return out.trim_end().to_string();
    }
    let line = |cells: &[String]| {
        let mut l = String::from("|");
        for i in 0..width {
            l.push(' ');
            l.push_str(&clean(cells.get(i).map(String::as_str).unwrap_or("")));
            l.push_str(" |");
        }
        l
    };
    if !header.is_empty() {
        out.push_str(&line(header));
        out.push('\n');
        out.push('|');
        for _ in 0..width {
            out.push_str(" --- |");
        }
        out.push('\n');
    }
    for row in rows {
        out.push_str(&line(row));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Size of one page in PDF points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    pub number: u32,
    pub width: f32,
    pub height: f32,
}

/// A parsed document as an ordered list of semantic blocks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StructuredDocument {
    pub pages: Vec<PageInfo>,
    pub blocks: Vec<Block>,
}

impl StructuredDocument {
    /// Plain text of the whole document, blocks separated by blank lines.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            let text = block.render();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(text);
        }
        out
    }

    /// Number of non-whitespace characters across all blocks.
    pub fn text_chars(&self) -> usize {
        self.blocks
            .iter()
            .map(|b| b.render().chars().filter(|c| !c.is_whitespace()).count())
            .sum()
    }

    /// Text of one page (blocks on that page in reading order), for page reads.
    pub fn page_text(&self, page: u32) -> String {
        let mut out = String::new();
        for block in self.blocks.iter().filter(|b| b.page == Some(page)) {
            let text = block.render();
            if text.trim().is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(text.trim());
        }
        out
    }

    /// Classify theorem-like statements, definitions, proofs, figure captions
    /// and bibliography entries from block text, attach table captions, and
    /// assign every block its section path. Idempotent.
    pub fn finalize(&mut self) {
        classify_semantics(&mut self.blocks);
        attach_table_captions(&mut self.blocks);
        assign_section_paths(&mut self.blocks);
    }
}

static RE_THEOREM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<label>(?:Theorem|Lemma|Proposition|Corollary|Claim|Conjecture|Assumption|Remark|Example|Hypothesis|Observation|Fact)\s*(?:[A-Z]?\d+(?:\.\d+)*)?)\s*(?:\([^)]{0,120}\))?\s*[.:]",
    )
    .expect("static regex")
});
static RE_DEFINITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<label>Definition\s*(?:[A-Z]?\d+(?:\.\d+)*)?)\s*(?:\([^)]{0,120}\))?\s*[.:]")
        .expect("static regex")
});
static RE_PROOF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Proof(?:\s+of\s+[^.:]{1,80})?\s*[.:]").expect("static regex"));
static RE_FIGURE_CAPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i:figure|fig\.)\s*[A-Z]?\d+[a-z]?\s*[.:|]").expect("static regex")
});
static RE_TABLE_CAPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i:table)\s*(?:[A-Z]?\d+|[IVX]{1,6})\s*[.:|]").expect("static regex")
});
static RE_REFERENCES_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:[0-9A-Z]{1,3}\.?\s+)?(?:references|bibliography|works cited|literature cited)$",
    )
    .expect("static regex")
});

/// Whether a heading text names a bibliography section.
pub fn is_references_heading(text: &str) -> bool {
    RE_REFERENCES_HEADING.is_match(text.trim())
}

/// Whether a block starts a figure caption ("Figure 3:", "Fig. 2.").
pub fn is_figure_caption(text: &str) -> bool {
    RE_FIGURE_CAPTION.is_match(text.trim_start())
}

/// Whether a block starts a table caption ("Table 2:").
pub fn is_table_caption(text: &str) -> bool {
    RE_TABLE_CAPTION.is_match(text.trim_start())
}

fn classify_semantics(blocks: &mut [Block]) {
    let mut in_references = false;
    for block in blocks.iter_mut() {
        match &block.kind {
            BlockKind::Heading { level } => {
                if is_references_heading(&block.text) {
                    in_references = true;
                } else if in_references && *level <= 1 {
                    in_references = false;
                }
                continue;
            }
            BlockKind::Paragraph | BlockKind::ListItem => {}
            _ => continue,
        }
        let text = block.text.trim_start();
        if in_references {
            block.kind = BlockKind::ReferenceEntry;
        } else if let Some(c) = RE_DEFINITION.captures(text) {
            block.kind = BlockKind::Definition {
                label: c["label"].trim().to_string(),
            };
        } else if let Some(c) = RE_THEOREM.captures(text) {
            block.kind = BlockKind::Theorem {
                label: c["label"].trim().to_string(),
            };
        } else if RE_PROOF.is_match(text) {
            block.kind = BlockKind::Proof;
        } else if is_figure_caption(text) {
            block.kind = BlockKind::Figure {
                caption: block.text.trim().to_string(),
            };
        }
    }
}

/// Farthest a caption may sit above or below its table, in points.
const MAX_CAPTION_GAP: f32 = 72.0;
/// Added to the distance of a caption below its table: papers set table captions
/// above, so of two equally close tables the one below the caption is preferred.
const CAPTION_BELOW_PENALTY: f32 = 6.0;
/// Score of a pairing by reading order alone (no geometry): worse than any
/// geometric pairing.
const ORDER_ONLY_SCORE: f32 = 1_000.0;

/// Moves every "Table N" caption paragraph into its table on the same page.
///
/// With boxes, a caption pairs with a table it overlaps horizontally and sits at most
/// [`MAX_CAPTION_GAP`] above or below; the closest pairs are taken first (a caption
/// above its table wins a tie), and each caption and table pairs once, so two tables
/// stacked on a page keep their own captions. Without boxes, a caption pairs with a
/// table at most two blocks away in reading order.
fn attach_table_captions(blocks: &mut Vec<Block>) {
    let captions: Vec<usize> = (0..blocks.len())
        .filter(|&i| {
            matches!(blocks[i].kind, BlockKind::Paragraph) && is_table_caption(&blocks[i].text)
        })
        .collect();
    let tables: Vec<usize> = (0..blocks.len())
        .filter(|&i| matches!(blocks[i].kind, BlockKind::Table { caption: None, .. }))
        .collect();
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for &c in &captions {
        for &t in &tables {
            if blocks[c].page != blocks[t].page {
                continue;
            }
            let order = c.abs_diff(t);
            match (blocks[c].bbox, blocks[t].bbox) {
                (Some(cb), Some(tb)) => {
                    if let Some(d) = caption_distance(&cb, &tb) {
                        pairs.push((d, c, t));
                    }
                }
                _ if order <= 2 => pairs.push((ORDER_ONLY_SCORE + order as f32, c, t)),
                _ => {}
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut used_captions: Vec<usize> = Vec::new();
    let mut used_tables: Vec<usize> = Vec::new();
    for (_, c, t) in pairs {
        if used_captions.contains(&c) || used_tables.contains(&t) {
            continue;
        }
        used_captions.push(c);
        used_tables.push(t);
        let caption_text = blocks[c].text.trim().to_string();
        let caption_bbox = blocks[c].bbox;
        if let BlockKind::Table { caption, .. } = &mut blocks[t].kind {
            *caption = Some(caption_text);
        }
        if let (Some(a), Some(b)) = (blocks[t].bbox, caption_bbox) {
            blocks[t].bbox = Some(a.union(&b));
        }
    }
    used_captions.sort_unstable();
    for c in used_captions.into_iter().rev() {
        blocks.remove(c);
    }
}

/// Distance score of a caption to a table: the vertical gap when the caption sits
/// above or below the table and overlaps it horizontally, with
/// [`CAPTION_BELOW_PENALTY`] for a caption below. `None` when they do not line up.
fn caption_distance(caption: &BBox, table: &BBox) -> Option<f32> {
    let overlap = caption.x1.min(table.x1) - caption.x0.max(table.x0);
    if overlap <= 0.0 {
        return None;
    }
    let above = caption.y0 - table.y1;
    let below = table.y0 - caption.y1;
    let (gap, penalty) = if above >= -2.0 {
        (above.max(0.0), 0.0)
    } else if below >= -2.0 {
        (below.max(0.0), CAPTION_BELOW_PENALTY)
    } else {
        // The caption overlaps the table vertically: inside a region that grew over it.
        (0.0, 0.0)
    };
    (gap <= MAX_CAPTION_GAP).then_some(gap + penalty)
}

fn assign_section_paths(blocks: &mut [Block]) {
    let mut stack: Vec<(u8, String)> = Vec::new();
    for block in blocks.iter_mut() {
        if let BlockKind::Heading { level } = block.kind {
            let level = level.max(1);
            while stack.last().is_some_and(|(l, _)| *l >= level) {
                stack.pop();
            }
            stack.push((level, collapse_ws(&block.text)));
            block.section_path = stack.iter().map(|(_, t)| t.clone()).collect();
        } else {
            block.section_path = stack.iter().map(|(_, t)| t.clone()).collect();
        }
    }
}

/// Collapse runs of whitespace into single spaces.
/// Whether a table cell holds one number as printed: digits with an optional sign,
/// thousands separators, decimal point, percent sign, a `±` spread or trailing
/// markers (`*`, `†`), e.g. `95.3`, `-0.68`, `1,234`, `78.0%`, `95.3±0.2`.
pub fn is_numeric_cell(text: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^[+\-−–]?(?:\d{1,3}(?:,\d{3})+|\d+)?(?:\.\d+)?%?(?:\s*±\s*\d+(?:\.\d+)?%?)?[*†‡]*$",
        )
        .expect("static regex")
    });
    let t = text.trim();
    t.chars().any(|c| c.is_ascii_digit()) && RE.is_match(t)
}

pub fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> Block {
        Block::new(BlockKind::Paragraph, text)
    }

    #[test]
    fn section_paths_follow_heading_levels() {
        let mut doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![
                Block::new(BlockKind::Heading { level: 1 }, "1 Introduction"),
                para("intro"),
                Block::new(BlockKind::Heading { level: 2 }, "1.1 Setup"),
                para("setup"),
                Block::new(BlockKind::Heading { level: 1 }, "2 Method"),
                para("method"),
            ],
        };
        doc.finalize();
        assert_eq!(doc.blocks[1].section_path, vec!["1 Introduction"]);
        assert_eq!(
            doc.blocks[3].section_path,
            vec!["1 Introduction", "1.1 Setup"]
        );
        assert_eq!(doc.blocks[5].section_path, vec!["2 Method"]);
    }

    #[test]
    fn classifies_theorems_definitions_proofs_captions_and_references() {
        let mut doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![
                para("Theorem 3.1 (Convergence). Let f be convex."),
                para("Lemma 2. For all x, y holds."),
                para("Definition 4.2 (Delta rule). The update is S = S + b."),
                para("Proof. By induction on t."),
                para("Figure 3: Throughput versus sequence length."),
                para("Theorems are discussed below."),
                Block::new(BlockKind::Heading { level: 1 }, "References"),
                para("[1] A. Author. A paper. 2020."),
                Block::new(BlockKind::Heading { level: 1 }, "A Appendix"),
                para("Appendix text."),
            ],
        };
        doc.finalize();
        assert_eq!(
            doc.blocks[0].kind,
            BlockKind::Theorem {
                label: "Theorem 3.1".into()
            }
        );
        assert_eq!(
            doc.blocks[1].kind,
            BlockKind::Theorem {
                label: "Lemma 2".into()
            }
        );
        assert_eq!(
            doc.blocks[2].kind,
            BlockKind::Definition {
                label: "Definition 4.2".into()
            }
        );
        assert_eq!(doc.blocks[3].kind, BlockKind::Proof);
        assert!(matches!(doc.blocks[4].kind, BlockKind::Figure { .. }));
        assert_eq!(doc.blocks[5].kind, BlockKind::Paragraph);
        assert_eq!(doc.blocks[7].kind, BlockKind::ReferenceEntry);
        assert_eq!(doc.blocks[9].kind, BlockKind::Paragraph);
    }

    #[test]
    fn table_caption_moves_into_adjacent_table() {
        let table = Block::new(
            BlockKind::Table {
                header: vec!["Model".into(), "PPL".into()],
                rows: vec![vec!["DeltaNet".into(), "17.7".into()]],
                caption: None,
                cell_boxes: Vec::new(),
            },
            "",
        )
        .on_page(2, Some(BBox::new(100.0, 100.0, 300.0, 200.0)));
        let caption = para("Table 1: Perplexity on WikiText.")
            .on_page(2, Some(BBox::new(100.0, 205.0, 300.0, 215.0)));
        let mut doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![caption, table],
        };
        doc.finalize();
        assert_eq!(doc.blocks.len(), 1);
        let rendered = doc.blocks[0].render();
        assert!(rendered.starts_with("Table 1: Perplexity on WikiText."));
        assert!(rendered.contains("| Model | PPL |"));
        assert!(rendered.contains("| DeltaNet | 17.7 |"));
        assert_eq!(doc.blocks[0].bbox.map(|b| b.y1), Some(215.0));
    }

    fn table_at(y0: f32, y1: f32, model: &str) -> Block {
        Block::new(
            BlockKind::Table {
                header: vec!["Model".into(), "PPL".into()],
                rows: vec![vec![model.into(), "17.7".into()]],
                caption: None,
                cell_boxes: Vec::new(),
            },
            "",
        )
        .on_page(3, Some(BBox::new(100.0, y0, 300.0, y1)))
    }

    fn caption_of(block: &Block) -> Option<&str> {
        match &block.kind {
            BlockKind::Table { caption, .. } => caption.as_deref(),
            _ => None,
        }
    }

    #[test]
    fn stacked_tables_keep_the_captions_above_them() {
        // Caption 1, table A, caption 2, table B (top to bottom). Caption 2 is closer to
        // table A's bottom than table A's own caption is to its top, but captions above
        // their tables win and each caption pairs once.
        let mut doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![
                para("Table 1: Results on SIFT1M.")
                    .on_page(3, Some(BBox::new(100.0, 712.0, 300.0, 722.0))),
                table_at(600.0, 708.0, "A"),
                para("Table 2: Results on GIST1M.")
                    .on_page(3, Some(BBox::new(100.0, 584.0, 300.0, 594.0))),
                table_at(480.0, 580.0, "B"),
            ],
        };
        doc.finalize();
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(
            caption_of(&doc.blocks[0]),
            Some("Table 1: Results on SIFT1M.")
        );
        assert_eq!(
            caption_of(&doc.blocks[1]),
            Some("Table 2: Results on GIST1M.")
        );
    }

    #[test]
    fn a_caption_below_its_table_attaches_and_far_or_offset_captions_do_not() {
        let mut doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![
                table_at(600.0, 700.0, "A"),
                para("Table 3: Ablations.").on_page(3, Some(BBox::new(100.0, 588.0, 300.0, 596.0))),
                // In the other column of the page: no horizontal overlap with B.
                para("Table 4: Elsewhere.").on_page(3, Some(BBox::new(320.0, 300.0, 520.0, 310.0))),
                table_at(200.0, 296.0, "B"),
            ],
        };
        doc.finalize();
        assert_eq!(caption_of(&doc.blocks[0]), Some("Table 3: Ablations."));
        // Table 4's caption is adjacent in reading order but lines up with nothing; the
        // order-only fallback applies only without boxes, so it stays a paragraph.
        assert!(doc.blocks.iter().any(|b| b.text.starts_with("Table 4")));
        assert_eq!(caption_of(doc.blocks.last().unwrap()), None);
    }

    #[test]
    fn bbox_normalizes_corners() {
        let b = BBox::new(10.0, 50.0, 5.0, 20.0);
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (5.0, 20.0, 10.0, 50.0));
    }
}
