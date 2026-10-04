//! Splitting an answer into checkable claims.
//!
//! The answer is Markdown. Claims come from prose only:
//! * paragraphs and block quotes are split into sentences, and a sentence
//!   whose clauses carry their own citations ("A is 5 [1]; B is 6 [2]") into
//!   clauses;
//! * each list item is split the same way;
//! * each table body row is one claim, read as "header: cell; …";
//! * headings, rules, fenced code, display math (`$$…$$`, `\[…\]`) and HTML
//!   comments are not claims.
//!
//! Every claim keeps `source`, the exact text it came from, and `anchor`,
//! the exact text after which the transcript shows its flag (for a table
//! row: the content of its last cell, so the flag stays inside the table).
//! Both are substrings of the message, found by text, never by offset (Rust
//! byte offsets and JavaScript string indices differ).

use std::sync::OnceLock;

use super::citations::{code_spans, find_markers, strip_markers, CitationMarker};
use crate::harness::events::ClaimKind;
use crate::harness::web::relevance::{is_stopword, words};
use regex::Regex;

/// One checkable statement of an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Plain text for scoring: markers removed, emphasis and code ticks
    /// dropped, whitespace collapsed.
    pub text: String,
    /// The exact text of the claim in the message (citations included).
    pub source: String,
    /// The exact text after which a flag is shown.
    pub anchor: String,
    /// Cited passage numbers, in order, without duplicates.
    pub citations: Vec<u32>,
    pub kind: ClaimKind,
    /// States something checkable (not a question, a lead-in such as "Key
    /// points:", a note about the search, a pointer such as "See [3]", or a
    /// statement that something was not found). Cited statements are checked.
    pub statement: bool,
    /// A statement substantial enough to need a source: uncited, it is
    /// flagged in an answer built from sources. Stricter than `statement`,
    /// so glue sentences ("Both differ in scope.") are not flagged.
    pub factual: bool,
}

/// Words before a full stop that do not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "al", "approx", "ca", "cf", "ch", "dr", "eds", "eq", "eqs", "etc", "fig", "figs", "inc", "jr",
    "ltd", "mr", "mrs", "ms", "pp", "prof", "ref", "refs", "resp", "sec", "vol", "vs", "viz",
];

/// Openings of sentences about the answer itself, not about the documents.
const META_OPENINGS: &[&str] = &[
    "below are",
    "below is",
    "do you",
    "feel free",
    "here are",
    "here is",
    "here's",
    "i can",
    "i could",
    "i couldn't",
    "i did not",
    "i didn't",
    "i found",
    "i have",
    "i looked",
    "i read",
    "i searched",
    "i will",
    "i'll",
    "i'm",
    "if you",
    "let me",
    "let's",
    "sure",
    "would you",
];

/// Phrases of a statement that something was not found. Such a sentence is
/// what an answer should say when the documents are silent; it is never an
/// uncited claim.
const ABSENCE_PHRASES: &[&str] = &[
    "could not find",
    "couldn't find",
    "did not find",
    "didn't find",
    "do not contain",
    "do not cover",
    "do not describe",
    "do not discuss",
    "do not mention",
    "do not say",
    "do not specify",
    "do not state",
    "does not contain",
    "does not cover",
    "does not describe",
    "does not discuss",
    "does not mention",
    "does not say",
    "does not specify",
    "does not state",
    "doesn't contain",
    "doesn't mention",
    "doesn't say",
    "don't contain",
    "don't mention",
    "don't say",
    "no information",
    "no mention",
    "no relevant",
    "not covered",
    "not found",
    "not mentioned",
    "not specified",
    "not stated",
    "nothing about",
];

fn cached(cell: &'static OnceLock<Option<Regex>>, pattern: &str) -> Option<&'static Regex> {
    cell.get_or_init(|| Regex::new(pattern).ok()).as_ref()
}

static LIST_ITEM: OnceLock<Option<Regex>> = OnceLock::new();
static TABLE_SEPARATOR: OnceLock<Option<Regex>> = OnceLock::new();
static HEADING: OnceLock<Option<Regex>> = OnceLock::new();
static RULE: OnceLock<Option<Regex>> = OnceLock::new();

fn list_marker_len(line: &str) -> Option<usize> {
    cached(&LIST_ITEM, r"^\s*(?:[-*+]|\d{1,3}[.)])\s+")?
        .find(line)
        .map(|m| m.end())
}

fn is_table_separator(line: &str) -> bool {
    cached(
        &TABLE_SEPARATOR,
        r"^\s*\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)*\|?\s*$",
    )
    .is_some_and(|re| re.is_match(line))
}

fn is_heading(line: &str) -> bool {
    cached(&HEADING, r"^\s{0,3}#{1,6}(?:\s|$)").is_some_and(|re| re.is_match(line))
}

fn is_rule(line: &str) -> bool {
    cached(
        &RULE,
        r"^\s{0,3}(?:(?:-\s*){3,}|(?:\*\s*){3,}|(?:_\s*){3,})$",
    )
    .is_some_and(|re| re.is_match(line.trim_end()))
}

/// A block of prose: pieces of the message (byte ranges) read as one text.
#[derive(Debug, Default)]
struct Block {
    pieces: Vec<(usize, usize)>,
    kind: Option<ClaimKind>,
}

impl Block {
    fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }
}

/// The block's text, with a map from each byte of it to the message.
fn joined(message: &str, block: &Block) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut map = Vec::new();
    for (i, (start, end)) in block.pieces.iter().enumerate() {
        if i > 0 {
            text.push('\n');
            map.push(block.pieces[i - 1].1);
        }
        text.push_str(&message[*start..*end]);
        map.extend(*start..*end);
    }
    (text, map)
}

/// Collapse whitespace, drop emphasis markers and code ticks.
fn plain(text: &str) -> String {
    let stripped = strip_markers(text);
    let cleaned: String = stripped
        .replace("**", "")
        .replace("__", "")
        .chars()
        .filter(|c| *c != '`')
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn content_words(text: &str) -> usize {
    words(text).iter().filter(|w| !is_stopword(w)).count()
}

/// Openings of a sentence that only points at a source.
const POINTER_OPENINGS: &[&str] = &[
    "as described in",
    "as shown in",
    "cf",
    "for details",
    "for more",
    "more in",
    "see",
];

/// Whether a sentence states something checkable.
pub fn is_statement(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.ends_with('?') || trimmed.ends_with(':') {
        return false;
    }
    let lower = trimmed
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    if META_OPENINGS.iter().any(|m| {
        lower
            .strip_prefix(m)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric()))
    }) {
        return false;
    }
    if ABSENCE_PHRASES.iter().any(|p| lower.contains(p)) {
        return false;
    }
    let content = content_words(trimmed);
    if content <= 3
        && POINTER_OPENINGS.iter().any(|m| {
            lower.strip_prefix(m).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric())
            })
        })
    {
        return false;
    }
    content >= 2
}

/// Whether a sentence states a fact substantial enough to need a source.
pub fn is_factual(text: &str) -> bool {
    if !is_statement(text) {
        return false;
    }
    let has_number = !super::numbers::claim_numbers(text).is_empty();
    let content = content_words(text);
    (has_number && content >= 2) || content >= 5
}

/// Whether `word` (the text before a `.`) is an abbreviation.
fn is_abbreviation(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    // Initials ("J."), dotted abbreviations ("e.g", "U.S").
    (w.chars().count() == 1 && w.chars().all(|c| c.is_alphabetic()))
        || w.contains('.')
        || ABBREVIATIONS.contains(&w.as_str())
}

/// Byte spans of `text` a sentence never ends inside: inline code, inline
/// math and citation markers.
fn protected_spans(text: &str, markers: &[CitationMarker]) -> Vec<(usize, usize)> {
    let mut spans = code_spans(text);
    spans.extend(markers.iter().map(|m| (m.start, m.end)));
    // Inline math `$…$` (a lone `$` before a digit is currency, which
    // `$…$` would rarely close on the same line).
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && !(i > 0 && bytes[i - 1] == b'\\') {
            if let Some(close) = text[i + 1..].find('$') {
                let end = i + 1 + close + 1;
                let inner = &text[i + 1..end - 1];
                if !inner.is_empty() && !inner.contains('\n') && !inner.starts_with(' ') {
                    spans.push((i, end));
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    spans
}

fn in_spans(spans: &[(usize, usize)], at: usize) -> bool {
    spans.iter().any(|(s, e)| at >= *s && at < *e)
}

/// Sentence spans (byte ranges) of a block's text.
fn sentence_spans(text: &str) -> Vec<(usize, usize)> {
    let markers = find_markers(text);
    let protected = protected_spans(text, &markers);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut spans = Vec::new();
    let mut start = 0;
    let mut k = 0;
    while k < chars.len() {
        let (at, c) = chars[k];
        if !matches!(c, '.' | '!' | '?' | '。') || in_spans(&protected, at) {
            k += 1;
            continue;
        }
        // Closing quotes, brackets and emphasis belong to the sentence.
        let mut j = k + 1;
        while j < chars.len() && matches!(chars[j].1, '"' | '\'' | ')' | '”' | '’' | '*' | '_')
        {
            j += 1;
        }
        let at_end = j >= chars.len();
        if !at_end && !chars[j].1.is_whitespace() {
            k += 1;
            continue;
        }
        if c == '.' {
            let word_start = chars[..k]
                .iter()
                .rposition(|(_, ch)| !(ch.is_alphanumeric() || *ch == '.'))
                .map(|p| p + 1)
                .unwrap_or(0);
            let word: String = chars[word_start..k].iter().map(|(_, ch)| ch).collect();
            let next = chars[j..]
                .iter()
                .map(|(_, ch)| *ch)
                .find(|ch| !ch.is_whitespace());
            // "Fig. 3", "et al. Smith", "J. Smith"; and "e.g. the": a
            // lower-case word continues the sentence.
            if !at_end && (is_abbreviation(&word) || next.is_some_and(char::is_lowercase)) {
                k += 1;
                continue;
            }
        }
        let mut end = chars.get(j).map(|(p, _)| *p).unwrap_or(text.len());
        // Citations written after the full stop belong to this sentence.
        let mut after = end;
        loop {
            let rest = &text[after..];
            let skipped = rest.len() - rest.trim_start_matches([' ', '\t']).len();
            match markers.iter().find(|m| m.start == after + skipped) {
                Some(m) => {
                    end = m.end;
                    after = m.end;
                }
                None => break,
            }
        }
        if text[start..end].trim().len() > 0 {
            spans.push((start, end));
        }
        start = end;
        k = chars
            .iter()
            .position(|(p, _)| *p >= end)
            .unwrap_or(chars.len());
    }
    if start < text.len() && !text[start..].trim().is_empty() {
        spans.push((start, text.len()));
    }
    // Split clauses that carry their own citations: after a marker that is
    // followed by `;` or `,` when another marker comes later.
    let mut out = Vec::new();
    for (s, e) in spans {
        let inner: Vec<&CitationMarker> = markers
            .iter()
            .filter(|m| m.start >= s && m.end <= e)
            .collect();
        let mut clause_start = s;
        for (idx, m) in inner.iter().enumerate() {
            if idx + 1 == inner.len() {
                break;
            }
            let rest = &text[m.end..e];
            let trimmed = rest.trim_start_matches([' ', '\t']);
            if trimmed.starts_with(';') || trimmed.starts_with(',') {
                let cut = m.end + (rest.len() - trimmed.len()) + 1;
                // Only when the next clause has a citation of its own.
                if inner[idx + 1].start > cut {
                    out.push((clause_start, cut));
                    clause_start = cut;
                }
            }
        }
        out.push((clause_start, e));
    }
    out
}

fn trimmed_span(text: &str, (s, e): (usize, usize)) -> (usize, usize) {
    let slice = &text[s..e];
    let lead = slice.len() - slice.trim_start().len();
    let trail = slice.len() - slice.trim_end().len();
    (s + lead, e - trail)
}

fn citations_of(text: &str) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for m in find_markers(text) {
        for n in m.numbers {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

fn block_claims(message: &str, block: &Block, out: &mut Vec<Claim>) {
    let (text, map) = joined(message, block);
    let kind = block.kind.unwrap_or(ClaimKind::Sentence);
    for span in sentence_spans(&text) {
        let (s, e) = trimmed_span(&text, span);
        if s >= e {
            continue;
        }
        let (os, oe) = (map[s], map[e - 1] + 1);
        let Some(source) = message.get(os..oe) else {
            continue;
        };
        let local = &text[s..e];
        let plain_text = plain(local);
        if plain_text.is_empty() {
            continue;
        }
        out.push(Claim {
            statement: is_statement(&plain_text),
            factual: is_factual(&plain_text),
            text: plain_text,
            source: source.to_string(),
            anchor: source.to_string(),
            citations: citations_of(local),
            kind,
        });
    }
}

/// Cells of a table row, as byte ranges of `line` (outer pipes dropped).
fn row_cells(line: &str) -> Vec<(usize, usize)> {
    let mut cells = Vec::new();
    let mut start = 0;
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'|' && !(i > 0 && bytes[i - 1] == b'\\') {
            cells.push((start, i));
            start = i + 1;
        }
    }
    cells.push((start, line.len()));
    let trimmed = line.trim();
    if trimmed.starts_with('|') && !cells.is_empty() {
        cells.remove(0);
    }
    if trimmed.ends_with('|') && !cells.is_empty() {
        cells.pop();
    }
    cells
}

fn row_claim(line: &str, headers: &[String]) -> Option<Claim> {
    let cells = row_cells(line);
    let mut parts = Vec::new();
    let mut last: Option<&str> = None;
    for (i, (s, e)) in cells.iter().enumerate() {
        let raw = line[*s..*e].trim();
        if raw.is_empty() {
            continue;
        }
        last = Some(raw);
        let value = plain(raw);
        if value.is_empty() {
            continue;
        }
        match headers.get(i).map(|h| plain(h)).filter(|h| !h.is_empty()) {
            Some(header) => parts.push(format!("{header}: {value}")),
            None => parts.push(value),
        }
    }
    let anchor = last?.to_string();
    let text = parts.join("; ");
    let numbers = super::numbers::claim_numbers(&text);
    let factual = !numbers.is_empty() || content_words(&text) >= 4;
    Some(Claim {
        statement: factual || content_words(&text) >= 2,
        factual,
        text,
        source: line.trim().to_string(),
        anchor,
        citations: citations_of(line),
        kind: ClaimKind::TableRow,
    })
}

/// Split one message of an answer into claims, in reading order.
pub fn split_claims(message: &str) -> Vec<Claim> {
    let mut out = Vec::new();
    let mut block = Block::default();
    let mut fence: Option<&str> = None;
    let mut math: Option<&str> = None;
    let mut headers: Option<Vec<String>> = None;
    let mut offset = 0;
    let lines: Vec<&str> = message.split_inclusive('\n').collect();
    let mut idx = 0;
    while idx < lines.len() {
        let raw = lines[idx];
        let line_start = offset;
        offset += raw.len();
        idx += 1;
        let line = raw.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim();

        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(close) = math {
            if trimmed.contains(close) {
                math = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(message, &mut block, &mut out);
            headers = None;
            fence = Some(if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            });
            continue;
        }
        if trimmed.starts_with("$$") {
            flush(message, &mut block, &mut out);
            if !(trimmed.len() > 4 && trimmed[2..].contains("$$")) {
                math = Some("$$");
            }
            continue;
        }
        if trimmed.starts_with("\\[") {
            flush(message, &mut block, &mut out);
            if !trimmed.contains("\\]") {
                math = Some("\\]");
            }
            continue;
        }
        if trimmed.is_empty() {
            flush(message, &mut block, &mut out);
            headers = None;
            continue;
        }
        if is_heading(line) || is_rule(line) || trimmed.starts_with("<!--") {
            flush(message, &mut block, &mut out);
            headers = None;
            continue;
        }
        // Tables: a header row followed by a separator row, then body rows.
        if let Some(h) = &headers {
            if trimmed.contains('|') {
                if let Some(claim) = row_claim(line, h) {
                    out.push(claim);
                }
                continue;
            }
            headers = None;
        }
        if trimmed.contains('|') && lines.get(idx).is_some_and(|next| is_table_separator(next)) {
            flush(message, &mut block, &mut out);
            let cells = row_cells(line);
            headers = Some(
                cells
                    .iter()
                    .map(|(s, e)| line[*s..*e].trim().to_string())
                    .collect(),
            );
            // Skip the separator row.
            offset += lines[idx].len();
            idx += 1;
            continue;
        }
        // Block quotes: read their content.
        let (content_start, content) = {
            let lead = line.len() - line.trim_start().len();
            let rest = &line[lead..];
            if let Some(stripped) = rest.strip_prefix('>') {
                let stripped_lead = stripped.len() - stripped.trim_start().len();
                (lead + 1 + stripped_lead, stripped.trim_start())
            } else {
                (lead, rest)
            }
        };
        if let Some(marker) = list_marker_len(content) {
            flush(message, &mut block, &mut out);
            block.kind = Some(ClaimKind::ListItem);
            let s = line_start + content_start + marker;
            block.pieces.push((s, line_start + line.len()));
            continue;
        }
        if content.is_empty() {
            flush(message, &mut block, &mut out);
            continue;
        }
        let s = line_start + content_start;
        block.pieces.push((s, line_start + line.len()));
    }
    flush(message, &mut block, &mut out);
    out
}

fn flush(message: &str, block: &mut Block, out: &mut Vec<Claim>) {
    if !block.is_empty() {
        block_claims(message, block, out);
    }
    *block = Block::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(claims: &[Claim]) -> Vec<&str> {
        claims.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn sentences_split_with_their_citations() {
        let answer = "The notice period is 60 days [1]. Either party may terminate early [2][3].\nRenewal is automatic. [4]";
        let claims = split_claims(answer);
        assert_eq!(
            texts(&claims),
            vec![
                "The notice period is 60 days.",
                "Either party may terminate early.",
                "Renewal is automatic."
            ]
        );
        assert_eq!(claims[0].citations, vec![1]);
        assert_eq!(claims[1].citations, vec![2, 3]);
        assert_eq!(
            claims[2].citations,
            vec![4],
            "a trailing citation belongs to the sentence before it"
        );
        for c in &claims {
            assert!(answer.contains(&c.source), "{:?}", c.source);
            assert!(c.statement);
        }
        assert!(claims[0].factual);
        assert!(
            !claims[2].factual,
            "too short to be flagged without a source"
        );
    }

    #[test]
    fn abbreviations_and_decimals_do_not_end_sentences() {
        let claims = split_claims(
            "Linear attention (e.g. DeltaNet) scales as O(n) [1]. As shown in Fig. 3 the loss drops by 3.5 points [2]. Smith et al. Report gains too [3]. Dr. Rao agrees, i.e. the effect holds [4].",
        );
        assert_eq!(
            texts(&claims),
            vec![
                "Linear attention (e.g. DeltaNet) scales as O(n).",
                "As shown in Fig. 3 the loss drops by 3.5 points.",
                "Smith et al. Report gains too.",
                "Dr. Rao agrees, i.e. the effect holds."
            ]
        );
    }

    #[test]
    fn clauses_with_their_own_citations_are_separate_claims() {
        let claims = split_claims("KAN uses learnable activations [1]; MLPs use fixed ones [2].");
        assert_eq!(
            texts(&claims),
            vec!["KAN uses learnable activations;", "MLPs use fixed ones."]
        );
        assert_eq!(claims[0].citations, vec![1]);
        assert_eq!(claims[1].citations, vec![2]);
        // One citation for the whole sentence: one claim.
        assert_eq!(split_claims("A holds, and B holds too [1].").len(), 1);
    }

    #[test]
    fn list_items_are_claims_and_lead_ins_are_not_factual() {
        let answer = "Key points:\n\n- The delta rule updates memory with a correction term [1].\n- Training is parallel over chunks of 64 tokens [2]. It uses a WY representation [3].\n1. Third item without a source states a long fact about chunkwise training.";
        let claims = split_claims(answer);
        assert_eq!(claims.len(), 5);
        assert!(!claims[0].factual, "a lead-in ending in a colon");
        assert_eq!(claims[1].kind, ClaimKind::ListItem);
        assert_eq!(
            claims[2].text,
            "Training is parallel over chunks of 64 tokens."
        );
        assert_eq!(claims[3].citations, vec![3]);
        assert!(claims[4].citations.is_empty() && claims[4].factual);
        for c in &claims {
            assert!(answer.contains(&c.source));
        }
    }

    #[test]
    fn table_rows_are_claims_with_an_anchor_in_the_last_cell() {
        let answer = "| Model | Perplexity |\n|---|---:|\n| DeltaNet | 16.87 [2] |\n| Mamba | 17.06 [3] |\n\nAfter the table.";
        let claims = split_claims(answer);
        assert_eq!(claims.len(), 3);
        assert_eq!(claims[0].kind, ClaimKind::TableRow);
        assert_eq!(claims[0].text, "Model: DeltaNet; Perplexity: 16.87");
        assert_eq!(claims[0].citations, vec![2]);
        assert_eq!(claims[0].anchor, "16.87 [2]");
        assert!(claims[0].factual);
        assert_eq!(claims[2].text, "After the table.");
    }

    #[test]
    fn code_math_and_headings_are_not_claims() {
        let answer = "## Results\n\n```python\nx = 1. # not a sentence [1]\n```\n\n$$\nE = mc^2. [2]\n$$\n\nThe energy is $E = mc^2$ for a mass at rest [3].\n\n---\n<!-- sketch -->";
        let claims = split_claims(answer);
        assert_eq!(
            texts(&claims),
            vec!["The energy is $E = mc^2$ for a mass at rest."]
        );
        assert_eq!(claims[0].citations, vec![3]);
    }

    #[test]
    fn pointers_are_not_statements() {
        assert!(!is_statement("See the appendix."));
        assert!(!is_statement("For details, see the table."));
        assert!(is_statement("Renewal is automatic."));
        assert!(is_statement("See-through displays use OLED panels."));
    }

    #[test]
    fn questions_meta_and_absence_statements_are_not_factual() {
        for s in [
            "Would you like me to search the web as well?",
            "I searched the contracts for termination clauses.",
            "Here is what the documents say.",
            "The documents do not mention a renewal fee anywhere in the agreement.",
            "Summary:",
        ] {
            assert!(!is_factual(s), "{s}");
        }
        for s in [
            "The notice period is 60 days.",
            "DeltaNet replaces the additive update with the delta rule update.",
        ] {
            assert!(is_factual(s), "{s}");
        }
    }

    #[test]
    fn multi_line_paragraphs_and_quotes_keep_an_exact_source() {
        let answer = "> The model was trained\n> on 15B tokens [1]. It converged [2].";
        let claims = split_claims(answer);
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].text, "The model was trained on 15B tokens.");
        assert!(answer.contains(&claims[0].source));
        assert!(claims[0].source.contains('\n'));
    }
}
