//! Figures of a paper: each "Figure N" caption block with the region of the page the
//! figure occupies, its number and the paragraphs that refer to it.
//!
//! The layout parser keeps a figure as its caption (text extraction cannot see the
//! drawing), so the region is inferred from the page's text: the space between the caption
//! and the nearest body text above it (or below it, for captions set above their figure),
//! within the caption's column. Labels drawn inside the figure (axis ticks, legends) arrive
//! as short text blocks; they are not body text, so they stay inside the region.
//!
//! Coordinates are PDF user-space points with a bottom-left origin, like every
//! [`BBox`] of the document model.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::processing::document_model::{BBox, Block, BlockKind, PageInfo, StructuredDocument};

/// A figure as the agent and the app show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Figure {
    /// Stable within one paper: `fig-<number>` (e.g. `fig-3`, `fig-a1`), or
    /// `fig-p<page>-<k>` when the number is missing or repeated.
    pub id: String,
    /// The number as printed ("3", "3b", "A1"), when the caption has one.
    pub number: Option<String>,
    /// "Figure 3" (or "Figure" when unnumbered).
    pub label: String,
    pub caption: String,
    /// 1-based page.
    pub page: u32,
    /// The figure with its caption.
    pub bbox: BBox,
    pub caption_bbox: BBox,
    /// False when no drawing area could be found next to the caption; `bbox` is then the
    /// caption alone.
    pub region_found: bool,
    /// Paragraphs that refer to the figure ("as Figure 3 shows …"), clipped.
    pub mentions: Vec<String>,
}

/// Shortest drawing area accepted next to a caption, in points.
const MIN_FIGURE_HEIGHT: f32 = 36.0;
/// Characters a block needs to count as body text (figure labels are shorter).
const BODY_MIN_CHARS: usize = 60;
/// Share of the column a block must span to count as body text.
const BODY_MIN_WIDTH: f32 = 0.5;
/// Running heads and footers live in this share of the page at the top and bottom.
const MARGIN_SHARE: f32 = 0.07;
/// Padding around the region, in points.
const PAD: f32 = 3.0;
/// Most mentions kept per figure, and their length.
const MAX_MENTIONS: usize = 2;
const MENTION_CHARS: usize = 600;

static RE_NUMBER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(?i:figure|fig\.?)\s*([A-Z]?\d+[a-z]?)\b").ok());
static RE_QUERY_NUMBER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(?i:(?:figure|fig\.?)\s*)?([A-Za-z]?\d+[a-z]?)$").ok());

/// The number a caption starts with ("Figure 3b:" → "3b").
pub fn caption_number(caption: &str) -> Option<String> {
    RE_NUMBER
        .as_ref()?
        .captures(caption.trim_start())
        .map(|c| c[1].to_string())
}

fn char_count(block: &Block) -> usize {
    block.text.chars().filter(|c| !c.is_whitespace()).count()
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// The text area and column split of a page, from its long text blocks.
struct PageGeometry {
    height: f32,
    width: f32,
    left: f32,
    right: f32,
    two_columns: bool,
}

impl PageGeometry {
    fn mid(&self) -> f32 {
        (self.left + self.right) / 2.0
    }
}

fn page_geometry(info: Option<&PageInfo>, blocks: &[&Block]) -> PageGeometry {
    let (width, height) = info.map_or((612.0, 792.0), |p| (p.width, p.height));
    let long: Vec<BBox> = blocks
        .iter()
        .filter(|b| char_count(b) >= BODY_MIN_CHARS && is_text(&b.kind))
        .filter_map(|b| b.bbox)
        .collect();
    let (left, right) = if long.is_empty() {
        (width * 0.08, width * 0.92)
    } else {
        (
            long.iter().map(|b| b.x0).fold(f32::MAX, f32::min),
            long.iter().map(|b| b.x1).fold(f32::MIN, f32::max),
        )
    };
    let mid = (left + right) / 2.0;
    let text_width = (right - left).max(1.0);
    let narrow = |b: &&BBox| b.width() < 0.6 * text_width;
    let in_left = long.iter().filter(narrow).any(|b| b.x1 < mid + 10.0);
    let in_right = long.iter().filter(narrow).any(|b| b.x0 > mid - 10.0);
    PageGeometry {
        height,
        width,
        left,
        right,
        two_columns: in_left && in_right,
    }
}

fn is_text(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Paragraph
            | BlockKind::ListItem
            | BlockKind::Theorem { .. }
            | BlockKind::Definition { .. }
            | BlockKind::Proof
            | BlockKind::ReferenceEntry
            | BlockKind::Footnote
    )
}

/// Whether a block bounds a figure: body text, headings, tables, numbered equations,
/// other captions, and anything in the running head or footer zone.
fn is_boundary(block: &Block, bbox: &BBox, column: (f32, f32), page: &PageGeometry) -> bool {
    if bbox.y0 > page.height * (1.0 - MARGIN_SHARE) || bbox.y1 < page.height * MARGIN_SHARE {
        return true;
    }
    match &block.kind {
        BlockKind::Title
        | BlockKind::Heading { .. }
        | BlockKind::Table { .. }
        | BlockKind::Figure { .. } => true,
        BlockKind::Equation => block.text.trim_end().ends_with(')'),
        kind if is_text(kind) => {
            let column_width = (column.1 - column.0).max(1.0);
            char_count(block) >= BODY_MIN_CHARS
                && bbox.width() >= BODY_MIN_WIDTH * column_width.min(page.right - page.left)
        }
        _ => false,
    }
}

/// The column a caption belongs to: the whole text width for wide or centred captions
/// (and on single-column pages), else the half of the page it sits in.
fn caption_column(caption: &BBox, page: &PageGeometry) -> (f32, f32) {
    let mid = page.mid();
    let text_width = page.right - page.left;
    let crosses_middle = caption.x0 < mid - 20.0 && caption.x1 > mid + 20.0;
    if !page.two_columns || crosses_middle || caption.width() > 0.6 * text_width {
        (page.left.min(caption.x0), page.right.max(caption.x1))
    } else if caption.center_x() < mid {
        (page.left.min(caption.x0), mid.max(caption.x1))
    } else {
        (mid.min(caption.x0), page.right.max(caption.x1))
    }
}

/// The drawing area of the figure whose caption is `caption` on a page with `blocks`
/// (the caption excluded), or `None` when no area of at least [`MIN_FIGURE_HEIGHT`] lies
/// above or below it.
pub fn figure_region(info: Option<&PageInfo>, blocks: &[&Block], caption: &BBox) -> Option<BBox> {
    let page = page_geometry(info, blocks);
    let column = caption_column(caption, &page);
    let lines_up = |b: &BBox| {
        let o = overlap(b.x0, b.x1, column.0, column.1);
        o > 0.3 * b.width().max(1.0) || o > 0.3 * (column.1 - column.0)
    };
    let boundaries: Vec<BBox> = blocks
        .iter()
        .filter_map(|b| b.bbox.map(|bbox| (b, bbox)))
        .filter(|(b, bbox)| lines_up(bbox) && is_boundary(b, bbox, column, &page))
        .map(|(_, bbox)| bbox)
        .collect();
    let page_top = page.height * (1.0 - MARGIN_SHARE * 0.5);
    let page_bottom = page.height * MARGIN_SHARE * 0.5;

    // Above the caption: up to the nearest boundary.
    let top = boundaries
        .iter()
        .filter(|b| b.y0 >= caption.y1 - 2.0)
        .map(|b| b.y0)
        .fold(page_top, f32::min);
    if top - caption.y1 >= MIN_FIGURE_HEIGHT {
        return Some(clamp(BBox::new(column.0, caption.y1, column.1, top), &page));
    }
    // Below it (a caption set above its figure).
    let bottom = boundaries
        .iter()
        .filter(|b| b.y1 <= caption.y0 + 2.0)
        .map(|b| b.y1)
        .fold(page_bottom, f32::max);
    if caption.y0 - bottom >= MIN_FIGURE_HEIGHT {
        return Some(clamp(
            BBox::new(column.0, bottom, column.1, caption.y0),
            &page,
        ));
    }
    None
}

fn clamp(b: BBox, page: &PageGeometry) -> BBox {
    BBox::new(
        b.x0.max(0.0),
        b.y0.max(0.0),
        b.x1.min(page.width),
        b.y1.min(page.height),
    )
}

fn padded(b: BBox, page: Option<&PageInfo>) -> BBox {
    let (w, h) = page.map_or((f32::MAX, f32::MAX), |p| (p.width, p.height));
    BBox::new(
        (b.x0 - PAD).max(0.0),
        (b.y0 - PAD).max(0.0),
        (b.x1 + PAD).min(w),
        (b.y1 + PAD).min(h),
    )
    .rounded()
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

/// Paragraphs that name "Figure N" / "Fig. N" (not the caption itself).
fn mentions(doc: &StructuredDocument, number: &str) -> Vec<String> {
    let pattern = format!(
        r"(?i)\b(?:figures?|figs?\.?)\s*(?:\d+[a-z]?\s*(?:,|and|&)\s*)*{}(?:[a-z]|\([a-z]\))?\b",
        regex::escape(number)
    );
    let Ok(re) = Regex::new(&pattern) else {
        return Vec::new();
    };
    doc.blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Paragraph | BlockKind::ListItem))
        .filter(|b| re.is_match(&b.text))
        .take(MAX_MENTIONS)
        .map(|b| clip(&b.text, MENTION_CHARS))
        .collect()
}

/// Every figure of a document, in reading order.
pub fn figures_of(doc: &StructuredDocument) -> Vec<Figure> {
    let mut found: Vec<Figure> = Vec::new();
    for (index, block) in doc.blocks.iter().enumerate() {
        let BlockKind::Figure { caption } = &block.kind else {
            continue;
        };
        let (Some(page), Some(caption_bbox)) = (block.page, block.bbox) else {
            continue;
        };
        let info = doc.pages.iter().find(|p| p.number == page);
        let others: Vec<&Block> = doc
            .blocks
            .iter()
            .enumerate()
            .filter(|(i, b)| *i != index && b.page == Some(page))
            .map(|(_, b)| b)
            .collect();
        let region = figure_region(info, &others, &caption_bbox);
        let number = caption_number(caption);
        let bbox = region.map_or(caption_bbox, |r| r.union(&caption_bbox));
        found.push(Figure {
            id: String::new(),
            label: number
                .as_deref()
                .map_or_else(|| "Figure".to_string(), |n| format!("Figure {n}")),
            mentions: number
                .as_deref()
                .map(|n| mentions(doc, n))
                .unwrap_or_default(),
            number,
            caption: clip(caption, 4_000),
            page,
            bbox: padded(bbox, info),
            caption_bbox: caption_bbox.rounded(),
            region_found: region.is_some(),
        });
    }
    // Ids: the number when it is unique, else page and position on the page.
    let mut per_page: Vec<(u32, usize)> = Vec::new();
    for i in 0..found.len() {
        let page = found[i].page;
        let k = match per_page.iter_mut().find(|(p, _)| *p == page) {
            Some((_, k)) => {
                *k += 1;
                *k
            }
            None => {
                per_page.push((page, 1));
                1
            }
        };
        let unique = found[i].number.as_deref().filter(|n| {
            found
                .iter()
                .filter(|f| {
                    f.number
                        .as_deref()
                        .is_some_and(|m| m.eq_ignore_ascii_case(n))
                })
                .count()
                == 1
        });
        found[i].id = match unique {
            Some(n) => format!("fig-{}", n.to_ascii_lowercase()),
            None => format!("fig-p{page}-{k}"),
        };
    }
    found
}

const STOP_WORDS: [&str; 16] = [
    "the", "and", "for", "with", "that", "this", "from", "figure", "fig", "show", "shows", "plot",
    "paper", "which", "of", "in",
];

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// Figures matching `query`: a number ("3", "Figure 3", "fig. 3b"; "3b" falls back to
/// "3") or words of the caption and the paragraphs that cite the figure, best first.
pub fn find_figures<'f>(figures: &'f [Figure], query: &str) -> Vec<&'f Figure> {
    let query = query.trim();
    if let Some(number) = RE_QUERY_NUMBER
        .as_ref()
        .and_then(|re| re.captures(query))
        .map(|c| c[1].to_string())
    {
        let exact: Vec<&Figure> = figures
            .iter()
            .filter(|f| {
                f.number
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&number))
            })
            .collect();
        if !exact.is_empty() {
            return exact;
        }
        let base = number.trim_end_matches(|c: char| c.is_ascii_lowercase());
        let parent: Vec<&Figure> = figures
            .iter()
            .filter(|f| {
                f.number
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(base))
            })
            .collect();
        if !parent.is_empty() || number.chars().all(|c| c.is_ascii_digit()) {
            return parent;
        }
    }
    let wanted = words(query);
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f32, &Figure)> = figures
        .iter()
        .filter_map(|f| {
            let caption = words(&f.caption);
            let context = words(&f.mentions.join(" "));
            let score: f32 = wanted
                .iter()
                .map(|w| {
                    if caption.contains(w) {
                        1.0
                    } else if context.contains(w) {
                        0.4
                    } else {
                        0.0
                    }
                })
                .sum::<f32>()
                / wanted.len() as f32;
            (score >= 0.34).then_some((score, f))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().map(|(_, f)| f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 612.0;
    const H: f32 = 792.0;

    fn page() -> PageInfo {
        PageInfo {
            number: 1,
            width: W,
            height: H,
        }
    }

    fn body(x0: f32, y0: f32, x1: f32, y1: f32) -> Block {
        Block::new(
            BlockKind::Paragraph,
            "Body text of the paper that runs across the whole column width, long enough to count as prose.",
        )
        .on_page(1, Some(BBox::new(x0, y0, x1, y1)))
    }

    fn label(x0: f32, y0: f32, text: &str) -> Block {
        Block::new(BlockKind::Paragraph, text)
            .on_page(1, Some(BBox::new(x0, y0, x0 + 30.0, y0 + 8.0)))
    }

    fn caption(text: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> Block {
        Block::new(
            BlockKind::Figure {
                caption: text.to_string(),
            },
            text,
        )
        .on_page(1, Some(BBox::new(x0, y0, x1, y1)))
    }

    fn doc(blocks: Vec<Block>) -> StructuredDocument {
        StructuredDocument {
            pages: vec![page()],
            blocks,
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= PAD + 0.5
    }

    #[test]
    fn single_column_figure_spans_from_the_caption_to_the_text_above() {
        let d = doc(vec![
            body(72.0, 600.0, 540.0, 700.0),
            label(150.0, 520.0, "0.5"),
            label(300.0, 450.0, "Epochs"),
            caption("Figure 1: Loss curves.", 72.0, 400.0, 540.0, 412.0),
            body(72.0, 250.0, 540.0, 390.0),
        ]);
        let figs = figures_of(&d);
        assert_eq!(figs.len(), 1);
        let f = &figs[0];
        assert!(f.region_found);
        assert_eq!(f.id, "fig-1");
        assert_eq!(f.label, "Figure 1");
        // From the caption's bottom to the paragraph above; the labels are inside.
        assert!(
            near(f.bbox.y0, 400.0) && near(f.bbox.y1, 600.0),
            "{:?}",
            f.bbox
        );
        assert!(
            near(f.bbox.x0, 72.0) && near(f.bbox.x1, 540.0),
            "{:?}",
            f.bbox
        );
    }

    #[test]
    fn two_column_pages_keep_figures_in_their_column() {
        let d = doc(vec![
            body(72.0, 500.0, 300.0, 720.0),
            body(312.0, 600.0, 540.0, 720.0),
            caption("Figure 2: Left.", 72.0, 300.0, 300.0, 312.0),
            body(72.0, 100.0, 300.0, 290.0),
            caption("Fig. 3. Right.", 312.0, 420.0, 540.0, 432.0),
            body(312.0, 100.0, 540.0, 410.0),
        ]);
        let figs = figures_of(&d);
        let left = &figs[0];
        assert!(
            near(left.bbox.y1, 500.0) && near(left.bbox.y0, 300.0),
            "{:?}",
            left.bbox
        );
        assert!(left.bbox.x1 <= 312.0, "{:?}", left.bbox);
        let right = &figs[1];
        assert_eq!(right.id, "fig-3");
        assert!(
            near(right.bbox.y1, 600.0) && near(right.bbox.y0, 420.0),
            "{:?}",
            right.bbox
        );
        assert!(right.bbox.x0 >= 300.0, "{:?}", right.bbox);
    }

    #[test]
    fn a_full_width_figure_on_a_two_column_page_spans_both_columns() {
        let d = doc(vec![
            caption(
                "Figure 4: Overview of the architecture.",
                200.0,
                480.0,
                412.0,
                492.0,
            ),
            body(72.0, 100.0, 300.0, 470.0),
            body(312.0, 100.0, 540.0, 470.0),
        ]);
        let f = &figures_of(&d)[0];
        // Nothing above: up to the top of the page's text zone.
        assert!(
            near(f.bbox.x0, 72.0) && near(f.bbox.x1, 540.0),
            "{:?}",
            f.bbox
        );
        assert!(f.bbox.y1 > 700.0 && near(f.bbox.y0, 480.0), "{:?}", f.bbox);
    }

    #[test]
    fn running_heads_bound_a_figure_at_the_top_of_the_page() {
        let d = doc(vec![
            Block::new(BlockKind::Paragraph, "Published at ICLR")
                .on_page(1, Some(BBox::new(220.0, 750.0, 390.0, 760.0))),
            caption("Figure 5: Top figure.", 72.0, 560.0, 540.0, 572.0),
            body(72.0, 300.0, 540.0, 550.0),
        ]);
        let f = &figures_of(&d)[0];
        assert!(near(f.bbox.y1, 750.0), "{:?}", f.bbox);
    }

    #[test]
    fn a_caption_set_above_its_figure_takes_the_space_below() {
        let d = doc(vec![
            body(72.0, 620.0, 540.0, 720.0),
            caption("Figure 6: Caption first.", 72.0, 600.0, 540.0, 612.0),
            label(200.0, 500.0, "x"),
            body(72.0, 200.0, 540.0, 380.0),
        ]);
        let f = &figures_of(&d)[0];
        assert!(f.region_found);
        assert!(
            near(f.bbox.y0, 380.0) && near(f.bbox.y1, 612.0),
            "{:?}",
            f.bbox
        );
    }

    #[test]
    fn no_room_means_the_caption_alone() {
        let d = doc(vec![
            body(72.0, 420.0, 540.0, 700.0),
            caption("Figure 7: Squeezed.", 72.0, 400.0, 540.0, 412.0),
            body(72.0, 100.0, 540.0, 395.0),
        ]);
        let f = &figures_of(&d)[0];
        assert!(!f.region_found);
        assert!(near(f.bbox.y0, 400.0) && near(f.bbox.y1, 412.0));
    }

    #[test]
    fn ids_fall_back_to_page_and_position_for_repeated_numbers() {
        let d = doc(vec![
            caption("Figure 1: A.", 72.0, 600.0, 540.0, 612.0),
            caption("Figure 1: Again.", 72.0, 300.0, 540.0, 312.0),
            caption("Figure A2: Appendix.", 72.0, 100.0, 540.0, 112.0),
        ]);
        let ids: Vec<String> = figures_of(&d).into_iter().map(|f| f.id).collect();
        assert_eq!(ids, ["fig-p1-1", "fig-p1-2", "fig-a2"]);
    }

    #[test]
    fn figures_are_found_by_number_or_caption_words() {
        let mut d = doc(vec![
            caption(
                "Figure 2: Throughput of the chunkwise form versus sequence length.",
                72.0,
                600.0,
                540.0,
                612.0,
            ),
            caption(
                "Figure 3: Recall of HNSW on SIFT1M.",
                72.0,
                300.0,
                540.0,
                312.0,
            ),
            Block::new(
                BlockKind::Paragraph,
                "As Figure 3 shows, recall saturates early.",
            )
            .on_page(1, Some(BBox::new(72.0, 100.0, 540.0, 140.0))),
        ]);
        d.blocks
            .push(caption("Figure 4b: Ablation.", 72.0, 200.0, 540.0, 212.0));
        let figs = figures_of(&d);
        assert_eq!(
            figs[1].mentions,
            ["As Figure 3 shows, recall saturates early."]
        );
        assert_eq!(find_figures(&figs, "Figure 3")[0].id, "fig-3");
        assert_eq!(find_figures(&figs, "fig. 2")[0].id, "fig-2");
        assert_eq!(find_figures(&figs, "3")[0].id, "fig-3");
        assert_eq!(find_figures(&figs, "Figure 4b")[0].id, "fig-4b");
        assert!(find_figures(&figs, "Figure 9").is_empty());
        assert_eq!(find_figures(&figs, "throughput chunkwise")[0].id, "fig-2");
        assert_eq!(find_figures(&figs, "recall saturates")[0].id, "fig-3");
        assert!(find_figures(&figs, "transformer pretraining").is_empty());
        assert_eq!(
            caption_number("Fig. 12a. Something"),
            Some("12a".to_string())
        );
    }
}
