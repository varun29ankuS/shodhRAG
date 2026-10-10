//! Numbers in claims and evidence (spec §7.4: every number in the answer
//! must appear in its cited text).
//!
//! A claim's numbers are its digit tokens, normalised (`1,000` → `1000`,
//! `3.50` → `3.5`, `60%` → `60`). Left out are numbers that name something
//! rather than state a quantity: part of a word (`GPT-4`, `3D`, `L6`), or a
//! reference such as "Table 2", "Eq. 3", "Section 4.1". Number words in a
//! claim are not checked (a passage rarely spells out "two approaches").
//!
//! Evidence numbers are its digit tokens plus spelled-out numbers
//! ("sixty" → 60, "sixty-five" → 65), so "sixty days" supports "60 days".

use std::collections::HashSet;

/// Words that turn the number after them into a reference, not a quantity.
const REFERENCE_WORDS: &[&str] = &[
    "alg",
    "algorithm",
    "appendix",
    "ch",
    "chapter",
    "col",
    "column",
    "corollary",
    "definition",
    "eq",
    "eqn",
    "eqs",
    "equation",
    "equations",
    "fig",
    "figs",
    "figure",
    "figures",
    "footnote",
    "item",
    "lemma",
    "line",
    "lines",
    "listing",
    "p",
    "page",
    "pages",
    "pp",
    "proposition",
    "ref",
    "row",
    "sec",
    "section",
    "sections",
    "step",
    "tab",
    "table",
    "tables",
    "theorem",
];

/// One number token: its normalised value and the word before it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    value: String,
    previous_word: String,
    attached_to_word: bool,
}

/// `"003.500"` → `"3.5"`, `"1,000"` → `"1000"`.
fn normalise(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| *c != ',').collect();
    let (int, frac) = match digits.split_once('.') {
        Some((i, f)) => (i, f.trim_end_matches('0')),
        None => (digits.as_str(), ""),
    };
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    if frac.is_empty() {
        int.to_string()
    } else {
        format!("{int}.{frac}")
    }
}

fn tokens(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len()
            && (chars[i].is_ascii_digit()
                || ((chars[i] == '.' || chars[i] == ',')
                    && i + 1 < chars.len()
                    && chars[i + 1].is_ascii_digit()
                    && i > start))
        {
            i += 1;
        }
        let mut raw: String = chars[start..i].iter().collect();
        // A comma is a thousands separator only before exactly three digits
        // ("1,000"); otherwise it separates numbers ("1,2"), which are read
        // one at a time.
        // Western grouping ("1,000,000") or Indian grouping ("2,97,390.43":
        // pairs, then a final group of three).
        let groups: Vec<usize> = raw
            .split(',')
            .skip(1)
            .map(|group| group.split('.').next().unwrap_or_default().len())
            .collect();
        let thousands = groups.iter().all(|len| *len == 3)
            || (groups.last() == Some(&3)
                && groups[..groups.len() - 1].iter().all(|len| *len == 2));
        if !thousands {
            if let Some(comma) = raw.find(',') {
                raw.truncate(comma);
                i = start + raw.chars().count();
            }
        }
        let before = start.checked_sub(1).map(|p| chars[p]);
        let after = chars.get(i).copied();
        // A magnitude suffix keeps a quantity a quantity: "340M", "32k", "1.3B", "4x".
        let magnitude = after.is_some_and(|c| matches!(c, 'k' | 'K' | 'M' | 'B' | 'G' | 'T' | 'x'))
            && !chars.get(i + 1).is_some_and(|c| c.is_alphabetic());
        let attached_to_word = before.is_some_and(char::is_alphabetic)
            || (after.is_some_and(char::is_alphabetic) && !magnitude)
            || (before == Some('-')
                && start
                    .checked_sub(2)
                    .and_then(|p| chars.get(p))
                    .is_some_and(|c| c.is_alphabetic()));
        let previous_word: String = {
            let mut j = start;
            while j > 0 && !chars[j - 1].is_alphanumeric() {
                j -= 1;
            }
            // The end of a range ("pages 10-12", "equations (1)–(3)") takes
            // the word before its start.
            if j > 0 && chars[j - 1].is_ascii_digit() && start - j <= 3 {
                while j > 0 && chars[j - 1].is_ascii_digit() {
                    j -= 1;
                }
                while j > 0 && !chars[j - 1].is_alphanumeric() {
                    j -= 1;
                }
            }
            let end = j;
            while j > 0 && chars[j - 1].is_alphabetic() {
                j -= 1;
            }
            chars[j..end].iter().collect::<String>().to_lowercase()
        };
        out.push(Token {
            value: normalise(&raw),
            previous_word,
            attached_to_word,
        });
    }
    out
}

/// `text` without inline math (`$…$`): its numbers are notation ("$C=10$",
/// "$M_{ic}=1$"), not quantities a passage states in words.
fn without_inline_math(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('$') {
        let after = &rest[open + 1..];
        match after.find('$') {
            Some(close) if close > 0 => {
                out.push_str(&rest[..open]);
                out.push(' ');
                rest = &after[close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Quantities a claim states, normalised, in order, without duplicates.
pub fn claim_numbers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in tokens(&without_inline_math(text)) {
        if token.attached_to_word || REFERENCE_WORDS.contains(&token.previous_word.as_str()) {
            continue;
        }
        if !out.contains(&token.value) {
            out.push(token.value);
        }
    }
    out
}

const UNITS: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 8] = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn word_value(word: &str) -> Option<u64> {
    if let Some(i) = UNITS.iter().position(|w| *w == word) {
        return u64::try_from(i).ok();
    }
    if let Some(i) = TENS.iter().position(|w| *w == word) {
        return u64::try_from(i).ok().map(|i| (i + 2) * 10);
    }
    match word {
        "hundred" => Some(100),
        "thousand" => Some(1_000),
        "million" => Some(1_000_000),
        "billion" => Some(1_000_000_000),
        "dozen" => Some(12),
        _ => None,
    }
}

/// Every number `text` contains, as digits or spelled out.
pub fn evidence_numbers(text: &str) -> HashSet<String> {
    let mut out: HashSet<String> = tokens(text).into_iter().map(|t| t.value).collect();
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic() && c != '-')
        .filter(|w| !w.is_empty())
        .collect();
    for word in words {
        // "sixty-five" → 65; "sixty" → 60.
        let parts: Vec<&str> = word.split('-').collect();
        match parts.as_slice() {
            [tens, unit] => {
                if let (Some(t), Some(u)) = (word_value(tens), word_value(unit)) {
                    if (20..=90).contains(&t) && u < 10 {
                        out.insert((t + u).to_string());
                    }
                }
                for part in parts {
                    if let Some(v) = word_value(part) {
                        out.insert(v.to_string());
                    }
                }
            }
            _ => {
                for part in parts {
                    if let Some(v) = word_value(part) {
                        out.insert(v.to_string());
                    }
                }
            }
        }
    }
    out
}

/// The claim's numbers that no evidence text contains.
pub fn missing_numbers(claim: &str, evidence: &[&str]) -> Vec<String> {
    let numbers = claim_numbers(claim);
    if numbers.is_empty() {
        return Vec::new();
    }
    let mut found: HashSet<String> = HashSet::new();
    for text in evidence {
        found.extend(evidence_numbers(text));
    }
    numbers.into_iter().filter(|n| !found.contains(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_numbers_are_normalised_quantities() {
        assert_eq!(
            claim_numbers("Revenue grew 12.50% to $1,000 in 2023, 3.0 times more."),
            vec!["12.5", "1000", "2023", "3"]
        );
        assert_eq!(claim_numbers("Sixty days."), Vec::<String>::new());
    }

    #[test]
    fn names_and_references_are_not_quantities() {
        assert_eq!(
            claim_numbers("GPT-4 and MiniLM-L6 in 3D, see Table 2, Eq. 3 and Section 4.1."),
            Vec::<String>::new()
        );
        assert_eq!(claim_numbers("Layer 12 has 64 heads."), vec!["12", "64"]);
        assert_eq!(
            claim_numbers(
                "See equations (1)–(3) and pages 10-12; with $C=10$ classes it reached 61.7%."
            ),
            vec!["61.7"]
        );
        assert_eq!(
            claim_numbers("Models with 340M and 1.3B parameters, 32k context, 4x faster."),
            vec!["340", "1.3", "32", "4"]
        );
    }

    #[test]
    fn evidence_numbers_include_spelled_out_numbers() {
        let found = evidence_numbers(
            "Either party may terminate with sixty days notice, or sixty-five in Q4.",
        );
        assert!(found.contains("60"));
        assert!(found.contains("65"));
        assert!(found.contains("4"), "Q4 still counts as evidence");
        assert!(evidence_numbers("1,024 tokens").contains("1024"));
        assert_eq!(
            claim_numbers("Total ₹2,97,390.43 and 1,00,000"),
            vec!["297390.43", "100000"]
        );
        assert!(evidence_numbers("value 0.50").contains("0.5"));
    }

    #[test]
    fn missing_numbers_are_reported() {
        let evidence = ["The notice period is sixty days and the fee is 1,200 EUR."];
        assert!(missing_numbers("Notice is 60 days.", &evidence).is_empty());
        assert_eq!(
            missing_numbers("Notice is 90 days and the fee 1200 EUR.", &evidence),
            vec!["90"]
        );
    }

    #[test]
    fn commas_between_numbers_are_not_thousands_separators() {
        assert_eq!(claim_numbers("Values 1,2 and 3"), vec!["1", "2", "3"]);
    }
}
