//! Text matching used by every metric: facts and passages are compared on
//! normalised tokens, so punctuation, case, digit grouping (`7,96,500` and
//! `796,500` and `796500`) and trailing decimal zeros (`18,450.00`) do not
//! decide whether a fact was found.

/// Lower-case word tokens joined by single spaces. Keeps `.` inside numbers
/// and `%`; drops digit-group separators; trims trailing zeros of decimals.
pub fn normalize_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let next_digit = chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if c.is_alphanumeric() || c == '%' {
            out.extend(c.to_lowercase());
        } else if matches!(c, ',' | '\u{2009}' | '\u{202f}' | '\'') && prev_digit && next_digit {
            // Digit grouping: 1,23,456 / 123 456 (thin spaces) / 1'234.
        } else if c == '.' && prev_digit && next_digit {
            out.push('.');
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace()
        .map(canonical_number)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `796500.00` -> `796500`, `1.50%` -> `1.5%`; other tokens unchanged.
fn canonical_number(token: &str) -> String {
    let (number, percent) = match token.strip_suffix('%') {
        Some(n) => (n, "%"),
        None => (token, ""),
    };
    let is_decimal = number.contains('.')
        && number.chars().all(|c| c.is_ascii_digit() || c == '.')
        && number.matches('.').count() == 1;
    if !is_decimal {
        return token.to_string();
    }
    let trimmed = number.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed}{percent}")
}

/// Whether `span` occurs in `text` as a whole-token sequence, after
/// normalisation. An empty span never matches.
pub fn contains_span(text: &str, span: &str) -> bool {
    let span = normalize_text(span);
    if span.is_empty() {
        return false;
    }
    format!(" {} ", normalize_text(text)).contains(&format!(" {span} "))
}

/// Phrases with which an answer says the documents do not contain what was
/// asked. A heuristic, reported as such: an answer can decline in words this
/// list does not know.
const REFUSAL_PHRASES: &[&str] = &[
    "couldn't find",
    "could not find",
    "can't find",
    "cannot find",
    "unable to find",
    "didn't find",
    "did not find",
    "found no",
    "no information",
    "no mention",
    "doesn't mention",
    "does not mention",
    "don't mention",
    "do not mention",
    "not mentioned",
    "isn't mentioned",
    "is not mentioned",
    "not specified",
    "doesn't specify",
    "does not specify",
    "don't specify",
    "do not specify",
    "doesn't contain",
    "does not contain",
    "don't contain",
    "do not contain",
    "doesn't say",
    "does not say",
    "don't say",
    "do not say",
    "not stated",
    "doesn't state",
    "does not state",
    "not found",
    "no relevant",
    "not in your documents",
    "not in the documents",
    "none of the documents",
    "none of your documents",
    "not available in",
    "no record of",
];

/// Whether `answer` says the documents do not answer the question.
pub fn is_refusal(answer: &str) -> bool {
    let lower = answer.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    REFUSAL_PHRASES.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_grouping_and_decimal_zeros_do_not_matter() {
        assert_eq!(normalize_text("INR 7,96,500.00"), "inr 796500");
        assert_eq!(normalize_text("Rs. 796,500"), "rs 796500");
        assert_eq!(normalize_text("EUR 18,450.00 (net)"), "eur 18450 net");
        assert_eq!(normalize_text("1.50% per month."), "1.5% per month");
        assert_eq!(normalize_text("recall@10 (0.78)."), "recall 10 0.78");
    }

    #[test]
    fn spans_match_whole_tokens_only() {
        assert!(contains_span(
            "Total amount due: INR 7,96,500.00",
            "7,96,500"
        ));
        assert!(contains_span(
            "an initial term of thirty-six (36) months",
            "36 months"
        ));
        assert!(contains_span(
            "Late payments accrue interest at 1.5% per month",
            "1.5%"
        ));
        assert!(!contains_span("within forty-five (45) days", "5 days"));
        assert!(!contains_span("anything", ""));
        assert!(contains_span("PR-200 arm", "pr 200"));
    }

    #[test]
    fn list_commas_are_not_digit_grouping() {
        assert_eq!(normalize_text("pages 3, 4"), "pages 3 4");
    }

    #[test]
    fn refusals_are_recognised_with_either_apostrophe() {
        assert!(is_refusal(
            "The documents don\u{2019}t contain a parental leave policy."
        ));
        assert!(is_refusal("I couldn't find this in your documents."));
        assert!(!is_refusal("The notice period is 90 days [1]."));
    }
}
