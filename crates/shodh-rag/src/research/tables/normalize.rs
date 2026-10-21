//! The normal form quotes and passages are compared in: line-break hyphenation joined,
//! Unicode NFKC (ligatures such as `ﬁ` become `fi`, full-width digits become ASCII),
//! typographic quotes and dashes folded to ASCII, soft hyphens dropped, whitespace
//! collapsed, case folded. A quote is located only at word boundaries, so a two-letter
//! quote never matches inside a longer word.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use super::Passage;

/// A letter, a hyphen at the end of a line, and the letter that continues the word.
static LINE_BREAK_HYPHEN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(\p{L})[-\u{2010}\u{00AD}][ \t]*\r?\n[ \t]*(\p{L})").ok());

fn fold(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
        other => other,
    }
}

fn finish(text: &str) -> String {
    let folded: String = text.nfkc().filter(|c| *c != '\u{00AD}').map(fold).collect();
    folded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `text` with every word split by a line-break hyphen joined ("contras-\ntive" →
/// "contrastive"). The fill prompt shows passages in this form, so the model quotes the
/// joined words.
pub fn join_line_breaks(text: &str) -> String {
    match LINE_BREAK_HYPHEN.as_ref() {
        Some(re) => re.replace_all(text, "$1$2").into_owned(),
        None => text.to_string(),
    }
}

/// `text` in the normal form, with a word split by a line-break hyphen joined.
pub fn normalize(text: &str) -> String {
    finish(&join_line_breaks(text))
}

/// The forms a passage is searched in: hyphenation joined ("contras-\ntive" →
/// "contrastive"), and the hyphen kept ("state-of-the-\nart" → "state-of-the-art").
fn passage_forms(text: &str) -> [String; 2] {
    let kept = match LINE_BREAK_HYPHEN.as_ref() {
        Some(re) => finish(&re.replace_all(text, "$1-$2")),
        None => finish(text),
    };
    [normalize(text), kept]
}

/// Whether `needle` occurs in `haystack` with no letter or digit right before or after it.
pub fn contains_at_word_bounds(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.match_indices(needle).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + needle.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// The passage `quote` occurs in, word for word after normalisation: `preferred` (the
/// one the model named, 0-based) when it is there, else the first other passage holding
/// it. `None` when no passage does or the quote is empty.
pub fn find_quote(quote: &str, passages: &[Passage], preferred: Option<usize>) -> Option<usize> {
    let needle = normalize(quote);
    if needle.is_empty() {
        return None;
    }
    let holds = |p: &Passage| {
        passage_forms(&p.text)
            .iter()
            .any(|form| contains_at_word_bounds(form, &needle))
    };
    if let Some(i) = preferred {
        if passages.get(i).is_some_and(holds) {
            return Some(i);
        }
    }
    passages.iter().position(holds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passage(id: &str, text: &str) -> Passage {
        Passage {
            chunk_id: id.to_string(),
            text: text.to_string(),
            page: Some(1),
            bboxes: None,
        }
    }

    #[test]
    fn the_pattern_compiles() {
        assert!(LINE_BREAK_HYPHEN.is_some());
    }

    #[test]
    fn ligatures_quotes_dashes_and_hyphenation_are_not_differences() {
        let passages = [passage(
            "a",
            "We train a classi\u{fb01}er with contras-\ntive loss; the model\u{2019}s accuracy\nis 91.2 % \u{2013} see Table 3.",
        )];
        for quote in [
            "We train a classifier with contrastive loss",
            "the model's accuracy is 91.2 % - see Table 3",
            "WE TRAIN A CLASSIFIER",
        ] {
            assert_eq!(find_quote(quote, &passages, None), Some(0), "{quote}");
        }
    }

    #[test]
    fn joining_line_breaks_keeps_other_hyphens_and_newlines() {
        assert_eq!(
            join_line_breaks("contras-\ntive pre-\n  training"),
            "contrastive pretraining"
        );
        assert_eq!(
            join_line_breaks("LAION-400M\nnext line"),
            "LAION-400M\nnext line"
        );
    }

    #[test]
    fn a_hyphenated_compound_split_at_a_line_end_matches_with_its_hyphen() {
        let passages = [passage("a", "a state-of-the-\nart retriever")];
        assert_eq!(
            find_quote("a state-of-the-art retriever", &passages, None),
            Some(0)
        );
    }

    #[test]
    fn a_quote_never_matches_inside_a_longer_word() {
        let passages = [passage("a", "contrastive pre-training on LAION-400M")];
        assert_eq!(
            find_quote("on", &passages, None),
            Some(0),
            "the word itself"
        );
        assert_eq!(find_quote("trast", &passages, None), None);
        assert_eq!(find_quote("LAION-40", &passages, None), None);
        assert_eq!(find_quote("   ", &passages, None), None);
    }

    #[test]
    fn the_named_passage_wins_and_another_given_passage_is_used_when_it_holds_the_quote() {
        let passages = [
            passage("a", "Results on ImageNet."),
            passage("b", "We evaluate on ImageNet and COCO."),
        ];
        assert_eq!(find_quote("ImageNet", &passages, Some(1)), Some(1));
        assert_eq!(find_quote("and COCO", &passages, Some(0)), Some(1));
        assert_eq!(find_quote("ImageNet", &passages, Some(7)), Some(0));
        assert_eq!(find_quote("CIFAR-10", &passages, Some(0)), None);
    }
}
