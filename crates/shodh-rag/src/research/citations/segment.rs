//! Turning the bibliography blocks of a parsed paper into one text per reference.
//!
//! The layout parser marks every paragraph after a "References" heading as a bibliography
//! entry until the next top-level heading, so its blocks are not yet one reference each:
//! - an entry split across columns or pages arrives as two blocks;
//! - two short entries can arrive as one block;
//! - appendix paragraphs under a lower-level heading after the bibliography arrive as
//!   entries too.
//!
//! [`segment`] re-splits blocks on numbered markers (`[12]`, `12.`), merges continuation
//! fragments into the entry before them and rejects what is not shaped like a reference,
//! counting each rejection with its reason rather than guessing.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::reference::take_authors;
use super::text::clean_block_text;
use crate::processing::document_model::{is_references_heading, BBox};

/// Longest text accepted as one reference, in characters.
pub const MAX_ENTRY_CHARS: usize = 1_200;
/// Shortest text accepted as one reference, in words.
pub const MIN_ENTRY_WORDS: usize = 4;

/// One bibliography block of the parsed document.
#[derive(Debug, Clone, PartialEq)]
pub struct BibBlock {
    pub text: String,
    pub page: Option<u32>,
    pub bbox: Option<BBox>,
    pub section_path: Vec<String>,
}

/// A box of an entry on a page (bottom-left origin, PDF points).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EntryRegion {
    pub page: u32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// The text of one reference and where it is printed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawEntry {
    pub text: String,
    pub page: Option<u32>,
    /// Boxes of the blocks the entry was assembled from. A block that held several
    /// entries gives its box to each of them.
    pub regions: Vec<EntryRegion>,
}

/// Why a block was not used as a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// Under a heading other than the bibliography's (an appendix after it).
    OutsideBibliography,
    /// Longer than any reference ([`MAX_ENTRY_CHARS`]).
    TooLong,
    /// Fewer than [`MIN_ENTRY_WORDS`] words.
    TooShort,
    /// No year, identifier or author list.
    NotAReference,
    /// A continuation fragment with no entry before it.
    Orphan,
}

/// The segmentation of one paper's bibliography.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segmentation {
    pub entries: Vec<RawEntry>,
    /// Rejected blocks or assembled texts, with the reason.
    pub rejected: Vec<(RejectReason, String)>,
    /// The bibliography is numbered (`[n]` or `n.`).
    pub numbered: bool,
}

static NUMBER_MARKER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\s*(?:\[(\d{1,3})\]|(\d{1,3})\.\s)").ok());
static INNER_MARKER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\s\[(\d{1,3})\]\s").ok());
static HAS_YEAR: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])(?:19[5-9]\d|20[0-4]\d)[a-z]?(?:[^0-9]|$)").ok());
static HAS_IDENTIFIER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)10\.\d{4,9}/|arxiv|abs/\d{4}\.\d{4,5}|https?://").ok());

fn marker_number(text: &str) -> Option<u32> {
    let c = NUMBER_MARKER.as_ref()?.captures(text)?;
    c.get(1).or_else(|| c.get(2))?.as_str().parse().ok()
}

fn matches(re: &LazyLock<Option<Regex>>, text: &str) -> bool {
    re.as_ref().is_some_and(|r| r.is_match(text))
}

fn region_of(block: &BibBlock) -> Option<EntryRegion> {
    match (block.page, block.bbox) {
        (Some(page), Some(b)) => Some(EntryRegion {
            page,
            x0: b.x0,
            y0: b.y0,
            x1: b.x1,
            y1: b.y1,
        }),
        _ => None,
    }
}

/// Whether the block sits directly under the bibliography heading.
fn in_bibliography(block: &BibBlock) -> bool {
    match block.section_path.last() {
        Some(heading) => is_references_heading(heading),
        // No heading chain (unpaged sources): trust the parser.
        None => true,
    }
}

/// Whether text reads as the start of an author–year entry: an author list at its start.
fn starts_like_entry(text: &str) -> bool {
    let first = text.chars().next();
    if !first.is_some_and(char::is_uppercase) {
        return false;
    }
    let (authors, _, _) = take_authors(text);
    !authors.is_empty()
}

/// Whether text continues the entry before it (it cannot start one).
fn continues_entry(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    if first.is_lowercase() || first.is_ascii_digit() || matches!(first, '(' | ',' | ';' | ':') {
        return true;
    }
    let lower = text.to_lowercase();
    [
        "in ",
        "pp.",
        "pages ",
        "proceedings",
        "arxiv",
        "url ",
        "doi",
        "advances in",
        "journal ",
        "transactions",
        "springer",
        "ieee",
        "acm ",
        "pmlr",
        "openreview",
    ]
    .iter()
    .any(|p| lower.starts_with(p))
}

/// Splits a block of a numbered bibliography at inner markers that continue the numbering
/// (`… 2021. [13] K. Irie …` after entry 12).
fn split_numbered(text: &str, expected_next: Option<u32>) -> Vec<String> {
    let Some(re) = INNER_MARKER.as_ref() else {
        return vec![text.to_string()];
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut next = marker_number(text).map(|n| n + 1).or(expected_next);
    for c in re.captures_iter(text) {
        let (Some(whole), Some(number)) = (c.get(0), c.get(1)) else {
            continue;
        };
        let n: Option<u32> = number.as_str().parse().ok();
        if n.is_some() && n == next {
            let piece = text[start..whole.start()].trim();
            if !piece.is_empty() {
                out.push(piece.to_string());
            }
            start = whole.start() + 1;
            next = n.map(|v| v + 1);
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn reject_shape(text: &str) -> Option<RejectReason> {
    if text.chars().count() > MAX_ENTRY_CHARS {
        return Some(RejectReason::TooLong);
    }
    if text.split_whitespace().count() < MIN_ENTRY_WORDS {
        return Some(RejectReason::TooShort);
    }
    let body = match NUMBER_MARKER.as_ref().and_then(|r| r.find(text)) {
        Some(m) => &text[m.end()..],
        None => text,
    };
    let has_authors = !take_authors(body.trim_start()).0.is_empty();
    if !matches(&HAS_YEAR, text) && !matches(&HAS_IDENTIFIER, text) && !has_authors {
        return Some(RejectReason::NotAReference);
    }
    None
}

fn preview(text: &str) -> String {
    let mut p: String = text.chars().take(120).collect();
    if text.chars().count() > 120 {
        p.push('…');
    }
    p
}

/// Segments the bibliography blocks of one paper, in reading order.
pub fn segment(blocks: &[BibBlock]) -> Segmentation {
    let mut out = Segmentation::default();
    let mut kept: Vec<(String, &BibBlock)> = Vec::new();
    for block in blocks {
        let text = clean_block_text(&block.text);
        if text.is_empty() {
            continue;
        }
        if !in_bibliography(block) {
            out.rejected
                .push((RejectReason::OutsideBibliography, preview(&text)));
            continue;
        }
        kept.push((text, block));
    }
    let numbered_blocks = kept
        .iter()
        .filter(|(t, _)| marker_number(t).is_some())
        .count();
    // Line-split bibliographies have many continuation blocks per numbered entry, so a
    // quarter of blocks starting with `[n]` already marks the style.
    out.numbered = !kept.is_empty() && numbered_blocks * 4 >= kept.len();

    let mut assembled: Vec<RawEntry> = Vec::new();
    let mut last_number: Option<u32> = None;
    for (text, block) in kept {
        let region = region_of(block);
        let pieces = if out.numbered {
            split_numbered(&text, last_number.map(|n| n + 1))
        } else {
            vec![text]
        };
        for piece in pieces {
            let starts = if out.numbered {
                marker_number(&piece).is_some()
            } else {
                match assembled.last() {
                    None => true,
                    // A line-split entry: the previous text stops mid-sentence.
                    Some(prev) if !ends_sentence(&prev.text) => false,
                    Some(prev) => {
                        !continues_entry(&piece)
                            && (starts_like_entry(&piece) || entry_complete(&prev.text))
                    }
                }
            };
            if starts {
                if let Some(n) = marker_number(&piece) {
                    last_number = Some(n);
                }
                assembled.push(RawEntry {
                    text: piece,
                    page: block.page,
                    regions: region.into_iter().collect(),
                });
            } else if let Some(prev) = assembled.last_mut() {
                // A line break, so the parser joins a word hyphenated across it.
                prev.text.push('\n');
                prev.text.push_str(&piece);
                if let Some(r) = region {
                    if !prev.regions.contains(&r) {
                        prev.regions.push(r);
                    }
                }
            } else {
                out.rejected.push((RejectReason::Orphan, preview(&piece)));
            }
        }
    }
    for entry in assembled {
        match reject_shape(&entry.text) {
            Some(reason) => out.rejected.push((reason, preview(&entry.text))),
            None => out.entries.push(entry),
        }
    }
    out
}

/// Whether text ends a sentence (a period, possibly before a closing bracket or quote).
fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches([')', ']', '"', '\u{201d}', '\''])
        .ends_with('.')
}

/// Whether an author–year entry already looks complete: it has a year and ends with a
/// period (a fragment after it starts a new entry).
fn entry_complete(text: &str) -> bool {
    matches(&HAS_YEAR, text) && ends_sentence(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, page: u32) -> BibBlock {
        BibBlock {
            text: text.to_string(),
            page: Some(page),
            bbox: Some(BBox::new(72.0, 100.0, 300.0, 120.0)),
            section_path: vec!["References".to_string()],
        }
    }

    #[test]
    fn numbered_blocks_split_at_markers_and_merge_continuations() {
        let blocks = vec![
            block("[1] S. Hochreiter and J. Schmidhuber. Long short-term memory. Neural Computation, 1997. [2] A. Vaswani, N. Shazeer, and N. Parmar. Attention is all you need.", 9),
            block("In NeurIPS, 2017.", 9),
            block("[3] I. Schlag, K. Irie, and J. Schmidhuber. Linear transformers are secretly fast weight programmers. In ICML, 2021.", 10),
        ];
        let s = segment(&blocks);
        assert!(s.numbered);
        assert_eq!(s.entries.len(), 3, "{:?}", s.entries);
        assert!(s.entries[1]
            .text
            .ends_with("Attention is all you need.\nIn NeurIPS, 2017."));
        assert!(s.entries[2].text.starts_with("[3] I. Schlag"));
        assert_eq!(s.entries[2].page, Some(10));
        assert!(s.rejected.is_empty());
    }

    #[test]
    fn author_year_fragments_join_the_entry_before_them() {
        let blocks = vec![
            block("Imanol Schlag, Kazuki Irie, and Jürgen Schmidhuber. 2021. Linear transformers are secretly fast weight programmers. In International Conference on Machine", 11),
            block("Learning, pages 9355–9366. PMLR.", 12),
            block("Songlin Yang, Bailin Wang, Yu Zhang, Yikang Shen, and Yoon Kim. 2024. Parallelizing linear transformers with the delta rule over sequence length. In NeurIPS.", 12),
        ];
        let s = segment(&blocks);
        assert!(!s.numbered);
        assert_eq!(s.entries.len(), 2, "{:?}", s.entries);
        assert!(s.entries[0]
            .text
            .ends_with("Machine\nLearning, pages 9355–9366. PMLR."));
        assert_eq!(s.entries[0].regions.len(), 2);
    }

    #[test]
    fn line_split_author_lists_join_until_the_entry_ends() {
        let blocks = vec![
            block("Achiam, J., Adler, S., Agarwal, S., Ahmad, L., Akkaya, I., Aleman, F. L., Almeida, D., Altenschmidt,", 14),
            block("J., Altman, S., Anadkat, S., et al. Gpt-4 technical report. arXiv preprint arXiv:2303.08774, 2023.", 14),
            block("Artetxe, M., Ruder, S., and Yogatama, D. On the cross-lingual transferability of monolingual represen-", 14),
            block("tations. In ACL, 2020.", 14),
        ];
        let s = segment(&blocks);
        assert_eq!(s.entries.len(), 2, "{:?}", s.entries);
        assert!(clean_block_text(&s.entries[0].text).contains("Altenschmidt, J., Altman"));
        assert!(s.entries[1].text.contains("represen-\ntations"));
        assert!(clean_block_text(&s.entries[1].text).contains("monolingual representations"));
    }

    #[test]
    fn appendix_paragraphs_and_non_references_are_rejected_with_reasons() {
        let mut appendix = block(
            "We additionally conduct experiments on RegBench, a synthetic data set designed to assess in-context learning.",
            14,
        );
        appendix.section_path = vec!["References".into(), "A.2 Synthetic tasks".into()];
        let blocks = vec![
            block("Hochreiter, S., & Schmidhuber, J. (1997). Long short-term memory. Neural Computation, 9(8), 1735–1780.", 9),
            appendix,
            block(&"Word ".repeat(400), 9),
        ];
        let s = segment(&blocks);
        assert_eq!(s.entries.len(), 1);
        let reasons: Vec<RejectReason> = s.rejected.iter().map(|(r, _)| *r).collect();
        assert!(reasons.contains(&RejectReason::OutsideBibliography));
        assert!(reasons.contains(&RejectReason::TooLong));
    }
}
