//! Label–value pairing for flattened (printed) forms.
//!
//! Tax returns, receipts, invoices and statements are usually printed forms: a
//! label on the left and its value in a column to the right, on the same baseline
//! (`Gross Salary (ia + ib + ic) | i | 8,51,416`). Reading order that follows
//! columns (XY-cut) reads all labels first and all values later, so a label and its
//! value end up in different blocks and chunks, and a question about the label
//! retrieves no value. This module finds such rows on form-like pages and returns
//! them as label/value pairs, so the parser emits one form-field block per row.
//!
//! A row is the set of lines whose vertical extents overlap, read left to right
//! and split into segments at wide gaps. It is a label–value row when:
//! - its last segment is a value (an amount, a date, an identifier, Yes/No), or the
//!   segment before it is a label ending in `:` (then any short text is a value);
//! - it has at least one label segment (three or more letters), and every other
//!   segment is a short code (`B1`, `ia`, `(3)`, `17(1)`); a row with two values is a
//!   table row, not a field, and is left alone;
//! - the label and the value are visibly apart (separate segments).
//!
//! Pairing applies only on form-like pages: at least [`MIN_ROWS`] such rows,
//! covering at least a quarter of the page's lines, so prose pages (two-column papers,
//! bibliographies, contents) are never touched. On those pages, two weaker patterns
//! pair too: a short label and a short text value across a wide gap
//! (`Name    ASHA VERMA`), and a single line `Label: value`.

use std::sync::LazyLock;

use regex::Regex;

use super::document_model::BBox;

/// Fewest strong label–value rows that make a page form-like.
pub const MIN_ROWS: usize = 3;
/// Least share of a page's lines that strong rows must cover for the page to be
/// form-like (a few number-ended lines in a paper's figure or contents are not a form).
const MIN_COVERAGE: f32 = 0.25;
/// Longest label kept, in characters (a longer "label" is a sentence).
const MAX_LABEL_CHARS: usize = 160;
/// Longest value, in characters.
const MAX_VALUE_CHARS: usize = 80;
/// Least vertical overlap, as a share of the smaller line height, for two lines to
/// share a row.
const ROW_OVERLAP: f32 = 0.3;

/// A line of a page as the pairing sees it.
#[derive(Debug, Clone)]
pub struct LayoutLine {
    pub text: String,
    pub bbox: BBox,
    pub size: f32,
    /// The line split at wide gaps, left to right, with boxes.
    pub segments: Vec<(String, BBox)>,
}

/// A label and its value, and the lines (indices into the input) of the row they came
/// from. Several pairs printed side by side share their row's lines.
#[derive(Debug, Clone, PartialEq)]
pub struct FormRow {
    pub lines: Vec<usize>,
    pub label: String,
    pub value: String,
    pub bbox: BBox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    Code,
    Value,
    Label,
    Other,
}

static RE_AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:₹|rs\.?|inr|\$|€|£|usd)?\s*[+\-−–]?\(?\d[\d,]*(?:\.\d+)?\)?\s*%?(?:\s*(?:cr|dr|/-))?$",
    )
    .expect("static regex")
});
static RE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:\d{1,2}[-/. ](?:\d{1,2}|jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*[-/., ]+\d{2,4}|\d{4}[-/.]\d{1,2}[-/.]\d{1,2})(?:\s+\d{1,2}:\d{2}(?::\d{2})?(?:\s*[ap]m)?)?$",
    )
    .expect("static regex")
});
static RE_IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Z0-9][A-Z0-9/\-]{5,39}|\d{1,3}(?:\.\d{1,3}){3})$").expect("static regex")
});
/// A small serial number (`1`, `12`) printed between a label and its value.
static RE_SERIAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,2}\.?$").expect("static regex"));
static RE_FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:yes|no|y|n|na|n/a|nil|true|false)$").expect("static regex")
});
/// Row and line references of forms: `B1`, `ia`, `ivb`, `17(1)`, `(3)`, `2a`.
static RE_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:\(?[A-Za-z]{0,2}\d{1,3}[A-Za-z]{0,3}\)?(?:\(\w{1,3}\))?|\(?[a-z]{1,2}\)?|\(?[ivxl]{1,5}[a-z]?\)?|[IVXL]{1,5})$",
    )
    .expect("static regex")
});
/// An equation number such as `(3)`: never a value.
static RE_EQUATION_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\(\d{1,3}[a-z]?\)$").expect("static regex"));
/// Dot leaders of a table of contents (`. . . .`, `....`, `…`).
static RE_LEADERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\.\s?){4,}|…").expect("static regex"));
/// A numbered section title (`2.1 Overview`, `A Proofs`, `B.1.1 LoRA`).
static RE_NUMBERED_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\d{1,2}|[A-Z])(?:\.\d{1,2})*\.?\s+\S").expect("static regex")
});
/// Characters of mathematics: a row of them is a formula with a subscript, not a field.
const MATH_CHARS: &str = "=∈∉≤≥◦∘∑∏∫×→←⇒∀∃⊂⊆∪∩∞≈≡∼|";
/// `Label: value` on one line.
static RE_INLINE_PAIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<label>[^:]{2,80}?)\s*:\s+(?P<value>\S.{0,79})$").expect("static regex")
});

/// Whether a segment reads as a field value: an amount, a date, an identifier with
/// a digit, or a yes/no flag.
pub fn is_value_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > MAX_VALUE_CHARS || RE_EQUATION_NUMBER.is_match(t) {
        return false;
    }
    if RE_FLAG.is_match(t) {
        return true;
    }
    if !t.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    RE_AMOUNT.is_match(t) || RE_DATE.is_match(t) || RE_IDENTIFIER.is_match(t)
}

fn letters(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphabetic()).count()
}

fn classify(text: &str) -> SegmentKind {
    let t = text.trim();
    if is_value_text(t) && !(t.len() <= 3 && t.chars().all(|c| c.is_ascii_alphabetic())) {
        return SegmentKind::Value;
    }
    if t.chars().count() <= 7 && !t.contains(' ') && RE_CODE.is_match(t) {
        return SegmentKind::Code;
    }
    if letters(t) >= 3 && t.chars().count() <= MAX_LABEL_CHARS {
        return SegmentKind::Label;
    }
    SegmentKind::Other
}

/// Lines grouped into rows: lines whose vertical extent overlaps the row's first line
/// by at least [`ROW_OVERLAP`] of the smaller height (a value printed beside the first
/// line of a two-line label still joins it). Rows come out top to bottom, lines within a
/// row left to right.
fn rows(lines: &[LayoutLine], excluded: &[bool]) -> Vec<Vec<usize>> {
    // Stacked glyphs of vertical text read as one tall "line"; they are never fields.
    let mut order: Vec<usize> = (0..lines.len())
        .filter(|&i| !excluded.get(i).copied().unwrap_or(false))
        .filter(|&i| lines[i].bbox.height() <= 2.2 * lines[i].size.max(1.0))
        .collect();
    order.sort_by(|&a, &b| {
        lines[b]
            .bbox
            .center_y()
            .total_cmp(&lines[a].bbox.center_y())
            .then(lines[a].bbox.x0.total_cmp(&lines[b].bbox.x0))
    });
    let mut out: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let line = &lines[i];
        let joins = out.last().is_some_and(|row| {
            let anchor = &lines[row[0]].bbox;
            let overlap = anchor.y1.min(line.bbox.y1) - anchor.y0.max(line.bbox.y0);
            overlap >= ROW_OVERLAP * anchor.height().min(line.bbox.height()).max(1.0)
        });
        match out.last_mut() {
            Some(row) if joins => row.push(i),
            _ => out.push(vec![i]),
        }
    }
    for row in &mut out {
        row.sort_by(|&a, &b| lines[a].bbox.x0.total_cmp(&lines[b].bbox.x0));
    }
    out
}

/// How sure a row is a label–value pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strength {
    /// The value is an amount, date, identifier or flag, or the label ends in `:`.
    Strong,
    /// A short label and a short text value across a wide gap (`Name    ASHA VERMA`):
    /// accepted only on pages that strong rows already show to be forms.
    Weak,
}

/// Whether a segment can be a text value of a weak row: short, no sentence
/// punctuation, not a label ending in `:`.
fn is_text_value(text: &str) -> bool {
    let t = text.trim();
    let words = t.split_whitespace().count();
    (1..=8).contains(&words)
        && t.chars().count() <= 60
        && letters(t) >= 2
        && !t.ends_with(['.', ':', ';', ','])
        && !t.contains(". ")
}

/// The label–value pairs of one row. A row may hold several pairs side by side
/// (`Status  Individual   Form Number:  ITR-1`): segments are read left to right and a
/// pair closes at each value. A value with no label before it, or two values in a
/// row, make it a table row (no pairs); so does text left over after the last value.
fn pair_row(lines: &[LayoutLine], row: &[usize]) -> Vec<(FormRow, Strength)> {
    let mut segments: Vec<(String, BBox)> = row
        .iter()
        .flat_map(|&i| lines[i].segments.iter().cloned())
        .map(|(t, b)| (t.trim().to_string(), b))
        .filter(|(t, _)| !t.is_empty())
        .collect();
    segments.sort_by(|a, b| a.1.x0.total_cmp(&b.1.x0));
    let size = row.iter().map(|&i| lines[i].size).fold(1.0f32, f32::max);
    let make = |label: &[&(String, BBox)], value: &(String, BBox)| -> Option<FormRow> {
        let text = label
            .iter()
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let text = text.trim().trim_end_matches(':').trim().to_string();
        let value_text = value.0.trim().to_string();
        if letters(&text) < 3
            || value_text.is_empty()
            || text.chars().count() > MAX_LABEL_CHARS
            || RE_LEADERS.is_match(&text)
            || text.chars().any(|c| MATH_CHARS.contains(c))
        {
            return None;
        }
        let bbox = label
            .iter()
            .map(|(_, b)| *b)
            .fold(value.1, |acc, b| acc.union(&b));
        Some(FormRow {
            lines: row.to_vec(),
            label: text,
            value: value_text,
            bbox,
        })
    };
    if segments.len() == 1 {
        // `Label: value` on one line.
        let Some(caps) = RE_INLINE_PAIR.captures(&segments[0].0) else {
            return Vec::new();
        };
        let label = caps["label"].to_string();
        let value = caps["value"].to_string();
        if letters(&label) < 3
            || label.split_whitespace().count() > 5
            || label.contains(['.', ',', ';', '(', '['])
            || value.chars().count() > 60
            || value.ends_with(':')
        {
            return Vec::new();
        }
        let bbox = segments[0].1;
        return vec![(
            FormRow {
                lines: row.to_vec(),
                label,
                value,
                bbox,
            },
            Strength::Weak,
        )];
    }
    // A small serial number right before a value is a line number, not a value.
    let kinds: Vec<SegmentKind> = segments
        .iter()
        .enumerate()
        .map(|(i, (t, _))| match classify(t) {
            SegmentKind::Value
                if RE_SERIAL.is_match(t)
                    && segments
                        .get(i + 1)
                        .is_some_and(|(next, _)| classify(next) == SegmentKind::Value) =>
            {
                SegmentKind::Code
            }
            kind => kind,
        })
        .collect();
    // Closes a pair at `value`. When the label segments before it include an earlier
    // label ending in `:` that is not the first, or text before a final `Label:`, the
    // leading part is tried as a pair of its own (`Status  Individual  Form Number:`).
    let close =
        |pending: &[&(String, BBox)], value: &(String, BBox)| -> Option<Vec<(FormRow, Strength)>> {
            let split = pending
                .iter()
                .rposition(|(t, _)| t.ends_with(':') && classify(t) == SegmentKind::Label)
                .filter(|&k| k > 0);
            if let Some(k) = split {
                let (lead, label) = pending.split_at(k);
                if let Some((lead_value, lead_label)) = lead.split_last() {
                    let weak = is_text_value(&lead_value.0)
                        && lead_label
                            .iter()
                            .any(|(t, _)| classify(t) == SegmentKind::Label);
                    if weak {
                        if let (Some(first), Some(second)) =
                            (make(lead_label, lead_value), make(label, value))
                        {
                            return Some(vec![(first, Strength::Weak), (second, Strength::Strong)]);
                        }
                    }
                }
            }
            make(pending, value).map(|pair| vec![(pair, Strength::Strong)])
        };
    let mut pairs: Vec<(FormRow, Strength)> = Vec::new();
    let mut pending: Vec<&(String, BBox)> = Vec::new();
    for (segment, kind) in segments.iter().zip(&kinds) {
        let has_label = pending
            .iter()
            .any(|(t, _)| classify(t) == SegmentKind::Label);
        let colon_label = pending
            .iter()
            .rev()
            .find(|(t, _)| classify(t) == SegmentKind::Label)
            .is_some_and(|(t, _)| t.ends_with(':'));
        match kind {
            SegmentKind::Value => {
                if !has_label {
                    return Vec::new();
                }
                match close(&pending, segment) {
                    Some(mut closed) => pairs.append(&mut closed),
                    None => return Vec::new(),
                }
                pending.clear();
            }
            SegmentKind::Label | SegmentKind::Other
                if colon_label
                    && segment.0.chars().count() <= MAX_VALUE_CHARS
                    && !segment.0.ends_with(':') =>
            {
                match close(&pending, segment) {
                    Some(mut closed) => pairs.append(&mut closed),
                    None => return Vec::new(),
                }
                pending.clear();
            }
            SegmentKind::Label | SegmentKind::Code => pending.push(segment),
            SegmentKind::Other => return Vec::new(),
        }
    }
    if pending.is_empty() {
        return pairs;
    }
    if !pairs.is_empty() {
        // Text after the last value: not a clean field row.
        return Vec::new();
    }
    // No strong value: a short label and a short text value across a wide gap.
    let Some((value, label)) = pending.split_last() else {
        return Vec::new();
    };
    let last_label = label
        .iter()
        .rev()
        .find(|(t, _)| classify(t) == SegmentKind::Label);
    let weak = is_text_value(&value.0)
        && last_label.is_some_and(|(t, b)| {
            t.split_whitespace().count() <= 5 && value.1.x0 - b.x1 >= 2.0 * size
        });
    match (weak, make(label, value)) {
        (true, Some(pair)) => vec![(pair, Strength::Weak)],
        _ => Vec::new(),
    }
}

/// The label–value rows of a page, top to bottom. `excluded` marks lines that are
/// not candidates (inside a table). Empty unless the page has at least
/// [`MIN_ROWS`] strong rows; weak rows count only on such pages.
pub fn form_rows(lines: &[LayoutLine], excluded: &[bool]) -> Vec<FormRow> {
    let found: Vec<(FormRow, Strength)> = rows(lines, excluded)
        .iter()
        .flat_map(|row| pair_row(lines, row))
        .collect();
    let strong: Vec<&FormRow> = found
        .iter()
        .filter(|(_, s)| *s == Strength::Strong)
        .map(|(row, _)| row)
        .collect();
    let candidates = (0..lines.len())
        .filter(|&i| !excluded.get(i).copied().unwrap_or(false))
        .count();
    let mut covered_lines: Vec<usize> = strong
        .iter()
        .flat_map(|r| r.lines.iter().copied())
        .collect();
    covered_lines.sort_unstable();
    covered_lines.dedup();
    let covered = covered_lines.len();
    if strong.len() < MIN_ROWS || (covered as f32) < MIN_COVERAGE * candidates as f32 {
        return Vec::new();
    }
    // A contents page: numbered titles followed by page numbers.
    let contents = strong
        .iter()
        .filter(|r| {
            RE_NUMBERED_TITLE.is_match(&r.label)
                && r.value.len() <= 3
                && r.value.chars().all(|c| c.is_ascii_digit())
        })
        .count();
    if contents * 2 >= strong.len() {
        return Vec::new();
    }
    found.into_iter().map(|(row, _)| row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of single-word-gap segments at `y` from `(x, text)` cells.
    fn line(y: f32, cells: &[(f32, &str)]) -> LayoutLine {
        let segments: Vec<(String, BBox)> = cells
            .iter()
            .map(|(x, t)| {
                let w = 5.0 * t.chars().count() as f32;
                (t.to_string(), BBox::new(*x, y, x + w, y + 9.0))
            })
            .collect();
        let bbox = segments
            .iter()
            .map(|(_, b)| *b)
            .reduce(|a, b| a.union(&b))
            .unwrap();
        LayoutLine {
            text: cells.iter().map(|(_, t)| *t).collect::<Vec<_>>().join(" "),
            bbox,
            size: 9.0,
            segments,
        }
    }

    #[test]
    fn values_are_amounts_dates_identifiers_and_flags() {
        for v in [
            "8,51,416",
            "75,000",
            "0",
            "₹ 1,200.50",
            "Rs. 500/-",
            "12.5%",
            "31-07-2024",
            "31-Jul-2024",
            "2024-07-31",
            "ABCDE1234F",
            "INV-2024-001",
            "Yes",
            "N/A",
        ] {
            assert!(is_value_text(v), "{v}");
        }
        for v in [
            "Gross Salary",
            "(3)",
            "the model learns",
            "",
            "Total income of the year",
        ] {
            assert!(!is_value_text(v), "{v}");
        }
    }

    #[test]
    fn a_label_column_and_a_value_column_pair_by_row_through_code_cells() {
        // Lines as an XY-cut reading order yields them: codes, then labels, then
        // line references with values, each column separately.
        let lines = vec![
            line(700.0, &[(31.0, "B1")]),
            line(700.0, &[(76.0, "i")]),
            line(680.0, &[(76.0, "a")]),
            line(660.0, &[(76.0, "b")]),
            line(700.0, &[(110.0, "Gross Salary (ia + ib + ic)")]),
            line(680.0, &[(110.0, "Salary as per section 17(1)")]),
            line(660.0, &[(110.0, "Value of perquisites")]),
            line(700.0, &[(444.0, "i"), (532.0, "9,10,000")]),
            line(680.0, &[(384.0, "ia"), (442.0, "9,10,000")]),
            line(660.0, &[(384.0, "ib"), (469.0, "0")]),
        ];
        let rows = form_rows(&lines, &vec![false; lines.len()]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].label, "B1 i Gross Salary (ia + ib + ic) i");
        assert_eq!(rows.iter().filter(|r| r.lines == rows[0].lines).count(), 1);
        assert_eq!(rows[0].value, "9,10,000");
        let mut first = rows[0].lines.clone();
        first.sort_unstable();
        assert_eq!(first, vec![0, 1, 4, 7]);
        assert_eq!(rows[1].label, "a Salary as per section 17(1) ia");
        assert_eq!(rows[2].value, "0");
        assert!(rows[0].bbox.x0 <= 31.0 && rows[0].bbox.x1 >= 532.0);
    }

    #[test]
    fn colon_labels_take_text_values_and_inline_pairs_split() {
        let lines = vec![
            line(
                700.0,
                &[(40.0, "Name of the assessee:"), (220.0, "Asha Verma")],
            ),
            line(680.0, &[(40.0, "Invoice No: INV-0042")]),
            line(660.0, &[(40.0, "Date of filing"), (220.0, "12-08-2024")]),
            line(640.0, &[(40.0, "Amount paid"), (220.0, "4,500.00")]),
        ];
        let rows = form_rows(&lines, &vec![false; lines.len()]);
        assert_eq!(rows.len(), 4);
        assert_eq!(
            (rows[0].label.as_str(), rows[0].value.as_str()),
            ("Name of the assessee", "Asha Verma")
        );
        assert_eq!(
            (rows[1].label.as_str(), rows[1].value.as_str()),
            ("Invoice No", "INV-0042")
        );
        assert_eq!(rows[2].value, "12-08-2024");
    }

    #[test]
    fn table_rows_prose_and_sparse_pages_are_left_alone() {
        // Two values in a row is a table row; prose with a trailing number is one segment.
        let lines = vec![
            line(700.0, &[(40.0, "HNSW"), (200.0, "95.3"), (260.0, "96.1")]),
            line(680.0, &[(40.0, "IVF-PQ"), (200.0, "88.0"), (260.0, "91.4")]),
            line(660.0, &[(40.0, "The index was built in 2024")]),
            line(640.0, &[(40.0, "Total"), (260.0, "12")]),
        ];
        assert!(form_rows(&lines, &vec![false; lines.len()]).is_empty());
        // Two-column prose: the right column is prose, not a value.
        let prose = vec![
            line(
                700.0,
                &[
                    (72.0, "the left column continues here"),
                    (320.0, "while the right column says this"),
                ],
            ),
            line(
                688.0,
                &[
                    (72.0, "another left line of prose"),
                    (320.0, "and another right line of prose"),
                ],
            ),
            line(676.0, &[(72.0, "equation follows"), (320.0, "(3)")]),
            line(
                664.0,
                &[(72.0, "short line"), (320.0, "more text on the right")],
            ),
        ];
        assert!(form_rows(&prose, &vec![false; prose.len()]).is_empty());
    }

    #[test]
    fn text_values_pair_only_on_pages_that_are_forms_and_serials_are_codes() {
        let weak_only = vec![
            line(700.0, &[(40.0, "Name"), (220.0, "ASHA VERMA")]),
            line(680.0, &[(40.0, "Status"), (220.0, "Individual")]),
            line(660.0, &[(40.0, "Residential status"), (220.0, "Resident")]),
        ];
        assert!(form_rows(&weak_only, &vec![false; weak_only.len()]).is_empty());
        let mut form = weak_only.clone();
        form.extend([
            line(
                640.0,
                &[
                    (40.0, "Current year loss, if any"),
                    (300.0, "1"),
                    (420.0, "0"),
                ],
            ),
            line(
                620.0,
                &[(40.0, "Net tax payable"), (300.0, "2"), (420.0, "12,400")],
            ),
            line(
                600.0,
                &[(40.0, "Interest payable"), (300.0, "3"), (420.0, "0")],
            ),
            // A sentence is never a text value.
            line(
                580.0,
                &[
                    (40.0, "Note"),
                    (220.0, "The return was verified. It is final."),
                ],
            ),
        ]);
        let rows = form_rows(&form, &vec![false; form.len()]);
        let pairs: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.label.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("Name", "ASHA VERMA"),
                ("Status", "Individual"),
                ("Residential status", "Resident"),
                ("Current year loss, if any 1", "0"),
                ("Net tax payable 2", "12,400"),
                ("Interest payable 3", "0"),
            ]
        );
    }

    #[test]
    fn a_value_beside_the_first_line_of_a_two_line_label_joins_it() {
        let lines = vec![
            line(700.0, &[(40.0, "Gross receipts"), (300.0, "1,000")]),
            line(680.0, &[(40.0, "Tax paid"), (300.0, "100")]),
            // The label's first line sits 3 pt above the value's baseline.
            LayoutLine {
                text: "Less allowances exempt".into(),
                bbox: BBox::new(40.0, 663.0, 160.0, 672.0),
                size: 9.0,
                segments: vec![(
                    "Less allowances exempt".into(),
                    BBox::new(40.0, 663.0, 160.0, 672.0),
                )],
            },
            line(660.0, &[(300.0, "50")]),
        ];
        let rows = form_rows(&lines, &vec![false; lines.len()]);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[2].label.as_str(), rows[2].value.as_str()),
            ("Less allowances exempt", "50")
        );
    }

    #[test]
    fn pairs_printed_side_by_side_split_and_stacked_glyphs_are_ignored() {
        let mut lines = vec![
            line(
                700.0,
                &[
                    (40.0, "Status"),
                    (120.0, "Individual"),
                    (300.0, "Form Number:"),
                    (380.0, "ITR-1"),
                ],
            ),
            line(
                680.0,
                &[
                    (40.0, "Acknowledgement Number:"),
                    (170.0, "714600000000001"),
                    (300.0, "Date of filing:"),
                    (380.0, "30-Jul-2026"),
                ],
            ),
            line(660.0, &[(40.0, "Taxes Paid"), (300.0, "7"), (380.0, "0")]),
            line(
                640.0,
                &[(40.0, "Net tax payable"), (300.0, "4"), (380.0, "1,200")],
            ),
        ];
        // Vertical text drawn glyph by glyph: one tall "line" beside the rows.
        lines.push(LayoutLine {
            text: "D x a T".into(),
            bbox: BBox::new(20.0, 630.0, 27.0, 690.0),
            size: 9.0,
            segments: vec![("D x a T".into(), BBox::new(20.0, 630.0, 27.0, 690.0))],
        });
        let rows = form_rows(&lines, &vec![false; lines.len()]);
        let pairs: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.label.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("Status", "Individual"),
                ("Form Number", "ITR-1"),
                ("Acknowledgement Number", "714600000000001"),
                ("Date of filing", "30-Jul-2026"),
                ("Taxes Paid 7", "0"),
                ("Net tax payable 4", "1,200"),
            ]
        );
        // The pairs of one row share its line, each with its own box.
        assert_eq!(rows[2].lines, rows[3].lines);
        assert!(rows[2].bbox.x1 < rows[3].bbox.x0);
    }

    #[test]
    fn bibliographies_and_contents_with_colons_are_not_forms() {
        let mut page: Vec<LayoutLine> = (0..20)
            .map(|i| {
                line(
                    700.0 - 12.0 * i as f32,
                    &[(
                        72.0,
                        "N. Author and B. Writer. Adafactor: adaptive learning rates, 2018.",
                    )],
                )
            })
            .collect();
        page.extend([
            line(
                400.0,
                &[(72.0, "6 Conclusion and future work"), (500.0, "37")],
            ),
            line(388.0, &[(72.0, "A Proofs in Section 3"), (500.0, "39")]),
            line(376.0, &[(72.0, "B Hyperparameters"), (500.0, "41")]),
            line(364.0, &[(72.0, "Attention: linear complexity")]),
        ]);
        assert!(form_rows(&page, &vec![false; page.len()]).is_empty());
        // A contents page with dot leaders and page numbers.
        let contents: Vec<LayoutLine> = (0..8)
            .map(|i| {
                line(
                    700.0 - 14.0 * i as f32,
                    &[
                        (
                            72.0,
                            &format!("{} Section title {i} . . . . . . . .", i + 1),
                        ),
                        (520.0, &format!("{}", 3 + 2 * i)),
                    ],
                )
            })
            .collect();
        assert!(form_rows(&contents, &vec![false; contents.len()]).is_empty());
        let numbered: Vec<LayoutLine> = (0..8)
            .map(|i| {
                line(
                    700.0 - 14.0 * i as f32,
                    &[
                        (72.0, &format!("{} Section title", i + 1)),
                        (520.0, &format!("{}", 3 + 2 * i)),
                    ],
                )
            })
            .collect();
        assert!(form_rows(&numbered, &vec![false; numbered.len()]).is_empty());
        // A formula whose subscript sits on its own run.
        let formulas: Vec<LayoutLine> = (0..5)
            .map(|i| {
                line(
                    700.0 - 14.0 * i as f32,
                    &[(72.0, "C = B ◦ A where for any w ∈ Σ"), (400.0, "0")],
                )
            })
            .collect();
        assert!(form_rows(&formulas, &vec![false; formulas.len()]).is_empty());
    }

    #[test]
    fn excluded_lines_are_never_paired() {
        let lines = vec![
            line(700.0, &[(40.0, "Gross receipts"), (300.0, "1,000")]),
            line(680.0, &[(40.0, "Tax paid"), (300.0, "100")]),
            line(660.0, &[(40.0, "Refund due"), (300.0, "50")]),
        ];
        assert_eq!(form_rows(&lines, &[false, false, false]).len(), 3);
        assert!(form_rows(&lines, &[true, false, false]).is_empty());
    }
}
