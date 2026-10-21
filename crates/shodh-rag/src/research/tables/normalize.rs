//! The normal form quotes and passages are compared in: a word split by a line-break
//! hyphen joined, the ligatures `\u{fb00}` `\u{fb01}` `\u{fb02}` `\u{fb03}` `\u{fb04}` spelled out,
//! typographic quotes and dashes (and the minus sign) folded to ASCII, soft hyphens
//! dropped, whitespace collapsed. Letter case, signs, and superscript or subscript digits
//! are kept: "5 MB/s" is not "5 Mb/s", "-3.2" is not "3.2" and "10\u{2076}" is not "106".
//! A quote is located only at token boundaries: a letter or digit cannot continue it on
//! either side, and a number cannot be cut out of a longer one ("91" is not in "91.2",
//! "234" is not in "1,234").

use std::sync::LazyLock;

use regex::Regex;

use super::Passage;

/// A letter, a hyphen at the end of a line, and the letter that continues the word.
static LINE_BREAK_HYPHEN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(\p{L})[-\u{2010}\u{00AD}][ \t]*\r?\n[ \t]*(\p{L})").ok());

fn finish(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{00AD}' => {}
            '\u{FB00}' => folded.push_str("ff"),
            '\u{FB01}' => folded.push_str("fi"),
            '\u{FB02}' => folded.push_str("fl"),
            '\u{FB03}' => folded.push_str("ffi"),
            '\u{FB04}' => folded.push_str("ffl"),
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => folded.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => folded.push('"'),
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => {
                folded.push('-')
            }
            other => folded.push(other),
        }
    }
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// Whether `needle` occurs in `haystack` at token boundaries: no letter or digit right
/// before or after it, and a needle that starts (ends) with a digit does not begin
/// (end) inside a longer number, that is right after `<digit>.` or `<digit>,` (right
/// before `.<digit>` or `,<digit>`).
pub fn contains_at_word_bounds(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let starts_digit = needle.chars().next().is_some_and(|c| c.is_ascii_digit());
    let ends_digit = needle
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_digit());
    let separator = |c: Option<char>| matches!(c, Some('.') | Some(','));
    haystack.match_indices(needle).any(|(at, _)| {
        let mut before = haystack[..at].chars().rev();
        let (b1, b2) = (before.next(), before.next());
        let mut after = haystack[at + needle.len()..].chars();
        let (a1, a2) = (after.next(), after.next());
        let continues_before = b1.is_some_and(char::is_alphanumeric)
            || (starts_digit && separator(b1) && b2.is_some_and(|c| c.is_ascii_digit()));
        let continues_after = a1.is_some_and(char::is_alphanumeric)
            || (ends_digit && separator(a1) && a2.is_some_and(|c| c.is_ascii_digit()));
        !continues_before && !continues_after
    })
}

/// The passage `quote` occurs in, word for word (case included) after normalisation: `preferred` (the
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
            "We train a classifier",
        ] {
            assert_eq!(find_quote(quote, &passages, None), Some(0), "{quote}");
        }
    }

    #[test]
    fn letter_case_and_superscripts_are_part_of_the_text() {
        let passages = [passage(
            "a",
            "Throughput is 5 Mb/s at a scale of 10\u{2076} images.",
        )];
        assert_eq!(
            find_quote("Throughput is 5 Mb/s at", &passages, None),
            Some(0)
        );
        assert_eq!(find_quote("Throughput is 5 MB/s at", &passages, None), None);
        assert_eq!(
            find_quote(
                "WE TRAIN A CLASSIFIER",
                &[passage("b", "We train a classifier")],
                None
            ),
            None
        );
        assert_eq!(
            find_quote("a scale of 10\u{2076} images", &passages, None),
            Some(0)
        );
        assert_eq!(find_quote("a scale of 106 images", &passages, None), None);
        assert_eq!(find_quote("a scale of 10 images", &passages, None), None);
    }

    #[test]
    fn a_number_is_never_cut_out_of_a_longer_one() {
        let passages = [passage(
            "a",
            "Top-1 accuracy reaches 91.2% on ImageNet, with 1,234 steps and \u{2212}3.5 loss.",
        )];
        assert_eq!(find_quote("accuracy reaches 91", &passages, None), None);
        assert_eq!(
            find_quote("accuracy reaches 91.2", &passages, None),
            Some(0)
        );
        assert_eq!(find_quote("234 steps", &passages, None), None);
        assert_eq!(find_quote("with 1,234 steps", &passages, None), Some(0));
        assert_eq!(find_quote("and -3.5 loss", &passages, None), Some(0));
        assert_eq!(find_quote("and 3.5 loss", &passages, None), None);
        assert!(
            contains_at_word_bounds("see 91.2.", "91.2"),
            "a full stop ends the number"
        );
        assert!(!contains_at_word_bounds("a 0.5 b", "5 b"));
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
