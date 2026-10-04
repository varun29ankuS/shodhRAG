//! Citation markers in answer text.
//!
//! One grammar, shared with the transcript renderer
//! (`app/src/features/agent/grounding.ts`); both are tested against
//! `harness/fixtures/citations.json`, so a pill and a verifier flag can never
//! disagree about what `[…]` means.
//!
//! A marker is
//! * `[n]`, `[n, m]`, `[Document n]`: run-wide passage numbers, 1-based;
//! * `[n-m]` / `[n–m]`: a range, expanded when it spans at most
//!   [`MAX_RANGE_SPAN`] numbers;
//! * `【n†…】`: the lenticular form some models write.
//!
//! Not markers: a group containing `0`, a range wider than
//! [`MAX_RANGE_SPAN`] or running backwards (intervals such as `[0, 1]` or
//! `[1-100]`), a group whose numbers do not increase (`[4, 9, 1]` is a shape
//! or a vector), a markdown link `[3](…)`, and anything inside fenced or
//! inline code.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;

/// Widest range expanded into citations (`[1-10]`); wider ones are prose.
pub const MAX_RANGE_SPAN: u32 = 10;

/// One citation marker: its byte span in the text and the numbers it cites,
/// in order, without duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationMarker {
    pub start: usize,
    pub end: usize,
    pub numbers: Vec<u32>,
}

fn cached(cell: &'static OnceLock<Option<Regex>>, pattern: &str) -> Option<&'static Regex> {
    cell.get_or_init(|| Regex::new(pattern).ok()).as_ref()
}

static BRACKET: OnceLock<Option<Regex>> = OnceLock::new();
static LENTICULAR: OnceLock<Option<Regex>> = OnceLock::new();

const BRACKET_PATTERN: &str = r"(?i)\[(?:document\s+)?(\d{1,6}(?:\s*[-–]\s*\d{1,6})?(?:\s*,\s*(?:document\s+)?\d{1,6}(?:\s*[-–]\s*\d{1,6})?)*)\]";
const LENTICULAR_PATTERN: &str = r"【(\d{1,6})†[^】]*】";

/// The numbers of a bracket group's inside (`"2, 4-6"`), or `None` when the
/// group is not a citation.
fn group_numbers(inner: &str) -> Option<Vec<u32>> {
    let mut out: Vec<u32> = Vec::new();
    for part in inner.split(',') {
        let part = part.trim();
        let part = part
            .get(..9)
            .filter(|p| p.eq_ignore_ascii_case("document "))
            .map(|_| part[9..].trim_start())
            .unwrap_or(part);
        let (from, to) = match part.split_once(['-', '–']) {
            Some((a, b)) => (a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?),
            None => {
                let n = part.parse::<u32>().ok()?;
                (n, n)
            }
        };
        if from == 0 || to < from || to - from + 1 > MAX_RANGE_SPAN {
            return None;
        }
        // Citations are written in increasing order; "[2, 5, 1]" is a
        // tuple (a network shape, a vector), not three citations.
        if out.last().is_some_and(|last| from <= *last) {
            return None;
        }
        out.extend(from..=to);
    }
    (!out.is_empty()).then_some(out)
}

/// Byte ranges of fenced code blocks (```` ``` ```` or `~~~`) and inline code
/// spans, where brackets are code, not citations.
pub fn code_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    let mut fence: Option<(usize, &str)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        match fence {
            Some((start, marker)) => {
                if trimmed.starts_with(marker) {
                    spans.push((start, offset + line.len()));
                    fence = None;
                }
            }
            None => {
                if trimmed.starts_with("```") {
                    fence = Some((offset, "```"));
                } else if trimmed.starts_with("~~~") {
                    fence = Some((offset, "~~~"));
                }
            }
        }
        offset += line.len();
    }
    if let Some((start, _)) = fence {
        spans.push((start, text.len()));
    }
    // Inline code outside the fences: a run of backticks closed by the same run.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(&(_, end)) = spans.iter().find(|(s, e)| i >= *s && i < *e) {
            i = end;
            continue;
        }
        if bytes[i] == b'`' {
            let run_start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            let ticks = &text[run_start..i];
            if let Some(close) = text[i..].find(ticks) {
                let end = i + close + ticks.len();
                spans.push((run_start, end));
                i = end;
            }
            continue;
        }
        i += 1;
    }
    spans.sort_unstable();
    spans
}

fn inside(spans: &[(usize, usize)], at: usize) -> bool {
    spans.iter().any(|(s, e)| at >= *s && at < *e)
}

/// Every citation marker of `text` outside code, in order.
pub fn find_markers(text: &str) -> Vec<CitationMarker> {
    let code = code_spans(text);
    let mut out = Vec::new();
    if let Some(re) = cached(&BRACKET, BRACKET_PATTERN) {
        for caps in re.captures_iter(text) {
            let (Some(whole), Some(inner)) = (caps.get(0), caps.get(1)) else {
                continue;
            };
            if inside(&code, whole.start()) || text[whole.end()..].starts_with('(') {
                continue;
            }
            if let Some(numbers) = group_numbers(inner.as_str()) {
                out.push(CitationMarker {
                    start: whole.start(),
                    end: whole.end(),
                    numbers,
                });
            }
        }
    }
    if let Some(re) = cached(&LENTICULAR, LENTICULAR_PATTERN) {
        for caps in re.captures_iter(text) {
            let (Some(whole), Some(n)) = (caps.get(0), caps.get(1)) else {
                continue;
            };
            if inside(&code, whole.start()) {
                continue;
            }
            if let Some(n) = n.as_str().parse::<u32>().ok().filter(|n| *n > 0) {
                out.push(CitationMarker {
                    start: whole.start(),
                    end: whole.end(),
                    numbers: vec![n],
                });
            }
        }
    }
    out.sort_by_key(|m| m.start);
    out
}

/// Every number `text` cites, outside code.
pub fn cited_numbers(text: &str) -> BTreeSet<u32> {
    find_markers(text)
        .into_iter()
        .flat_map(|m| m.numbers)
        .collect()
}

/// `text` with every citation marker removed (spaces before a removed
/// marker are dropped too, so "days [1]." becomes "days.").
pub fn strip_markers(text: &str) -> String {
    let markers = find_markers(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in &markers {
        let kept = &text[last..m.start];
        out.push_str(kept.trim_end_matches([' ', '\t']));
        last = m.end;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Shared with `app/tests/grounding.test.ts`.
    const SHARED: &str = include_str!("../fixtures/citations.json");

    #[test]
    fn shared_fixture_cases_parse_identically() {
        let cases: Value = serde_json::from_str(SHARED).unwrap();
        let cases = cases["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let text = case["text"].as_str().unwrap();
            let expected: Vec<Vec<u32>> = case["markers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| {
                    m.as_array()
                        .unwrap()
                        .iter()
                        .map(|n| n.as_u64().unwrap() as u32)
                        .collect()
                })
                .collect();
            let got: Vec<Vec<u32>> = find_markers(text).into_iter().map(|m| m.numbers).collect();
            assert_eq!(got, expected, "{text}");
        }
    }

    #[test]
    fn marker_spans_cover_the_brackets() {
        let text = "Sixty days [1]. Also [2, 4-5].";
        let markers = find_markers(text);
        assert_eq!(&text[markers[0].start..markers[0].end], "[1]");
        assert_eq!(&text[markers[1].start..markers[1].end], "[2, 4-5]");
        assert_eq!(markers[1].numbers, vec![2, 4, 5]);
    }

    #[test]
    fn markers_are_stripped_with_their_leading_space() {
        assert_eq!(
            strip_markers("Notice is 60 days [1][2]. Renewal [3]."),
            "Notice is 60 days. Renewal."
        );
        assert_eq!(strip_markers("No markers."), "No markers.");
    }

    #[test]
    fn code_is_never_cited() {
        let text = "Use `a[1]` here [2].\n```\nlet x = b[3];\n```\nDone [4].";
        assert_eq!(
            cited_numbers(text).into_iter().collect::<Vec<_>>(),
            vec![2, 4]
        );
        let unclosed = "Text [1].\n```\nx[2]";
        assert_eq!(
            cited_numbers(unclosed).into_iter().collect::<Vec<_>>(),
            vec![1]
        );
    }
}
