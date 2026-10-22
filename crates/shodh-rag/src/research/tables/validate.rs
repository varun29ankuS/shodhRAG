//! The rules a model's answer must pass. Deterministic; the model never has the last word.
//!
//! - The quote must occur, after [`normalize`] (letter case, signs and superscripts kept),
//!   in the passage the model named or in another passage of the same call
//!   (`quote_not_in_passages` otherwise), and be at least [`MIN_QUOTE_WORDS`] words and
//!   [`MIN_QUOTE_CHARS`] characters long (`quote_too_short` otherwise).
//! - The value must pass its column's type rule: `number` — its digits occur in the quote
//!   (nothing computed, rounded or converted) and a stated unit too; `yes_no` — yes or no;
//!   `choice` — one of the options, case-folded; `text` — 1 to 500 characters.
//! - A verified answer the local entailment model reads as contradicted by its quote is
//!   downgraded (`quote_does_not_support`).
//! - A reply with no `answer` key at all is `failed` (`missing_answer`), so the job retries
//!   it; only an explicit `answer: null` is `not_found`, shown as "Not found in the passages searched".

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::normalize::{contains_at_word_bounds, find_quote, normalize};
use super::{collapse, CellStatus, CellValue, ColumnKind, ColumnSpec, Passage, MAX_TEXT_CHARS};
use crate::harness::events::ScoringMethod;
use crate::harness::grounding::verify::{support, ScorerSet};
use crate::research::{ResearchError, ResearchResult};

pub const QUOTE_NOT_IN_PASSAGES: &str = "quote_not_in_passages";
pub const QUOTE_DOES_NOT_SUPPORT: &str = "quote_does_not_support";
pub const MISSING_QUOTE: &str = "missing_quote";
pub const MISSING_ANSWER: &str = "missing_answer";
pub const QUOTE_TOO_SHORT: &str = "quote_too_short";
pub const NOT_A_NUMBER: &str = "not_a_number";
pub const NUMBER_NOT_IN_QUOTE: &str = "number_not_in_quote";
pub const UNIT_NOT_IN_QUOTE: &str = "unit_not_in_quote";
pub const NOT_YES_NO: &str = "not_yes_no";
pub const NOT_AN_OPTION: &str = "not_an_option";
pub const EMPTY_TEXT: &str = "empty_text";
pub const TEXT_TOO_LONG: &str = "text_too_long";
/// Set on an edited cell, which keeps the user's value, when its column's type or options
/// changed and the value no longer fits them (see `TableStore::update_column`).
pub const VALUE_NO_LONGER_FITS: &str = "value_no_longer_fits";

/// Contradiction probability at or above which (and above the entailment probability) a
/// verified cell is downgraded. Uncalibrated; the accuracy eval measures it.
pub const CONTRADICTION_CUTOFF: f32 = 0.5;

/// Fewest words a quote must have to back an answer. A shorter quote ("the") would sit in
/// any paper and prove nothing; the reader must be able to check the answer at a glance.
pub const MIN_QUOTE_WORDS: usize = 3;
/// Fewest characters (after normalisation) a quote must have, whatever its word count.
pub const MIN_QUOTE_CHARS: usize = 15;

/// Reads `answer` so that a missing key stays distinct from `null`: a missing key is
/// `None`, `null` is `Some(None)`, a value is `Some(Some(value))`.
fn answer_present<'de, D>(deserializer: D) -> Result<Option<Option<Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Value>::deserialize(deserializer).map(Some)
}

/// What the model said for one column. `passage` is 1-based, as numbered in the prompt.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawAnswer {
    /// `None`: the reply has no `answer` key. `Some(None)`: `answer: null`, the model
    /// found nothing.
    #[serde(
        default,
        deserialize_with = "answer_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub answer: Option<Option<Value>>,
    pub quote: Option<String>,
    pub passage: Option<usize>,
}

/// The outcome of the checks. `passage` is the 0-based index of the passage the quote
/// was found in.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub status: CellStatus,
    pub value: Option<CellValue>,
    pub quote: Option<String>,
    pub passage: Option<usize>,
    pub error: Option<String>,
}

/// A number as an answer states it: value, then an optional unit without digits.
static ANSWER_NUMBER: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<v>[+\-−–]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[+\-−–]?\.\d+)\s*(?P<u>[^\d\s][^\d]{0,23})?$",
    )
    .ok()
});

/// Numbers printed in a quote, with the sign written directly before them.
static QUOTE_NUMBER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"[+\-]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[+\-]?\.\d+").ok());

fn unsigned_decimal(lexeme: &str) -> String {
    let digits = lexeme
        .trim_start_matches(['+', '-', '−', '–'])
        .replace(',', "");
    match digits.strip_prefix('.') {
        Some(rest) => format!("0.{rest}"),
        None => digits,
    }
}

fn read_number(text: &str) -> Option<CellValue> {
    let caps = ANSWER_NUMBER.as_ref()?.captures(text.trim())?;
    let lexeme = caps.name("v")?.as_str().to_string();
    let negative = lexeme.starts_with(['-', '−', '–']);
    let unsigned = unsigned_decimal(&lexeme);
    let decimal = if negative {
        format!("-{unsigned}")
    } else {
        unsigned
    };
    let unit = caps
        .name("u")
        .map(|u| collapse(u.as_str()))
        .filter(|u| !u.is_empty());
    Some(CellValue::Number {
        lexeme,
        decimal,
        unit,
    })
}

fn scalar_text(answer: &Value) -> Option<String> {
    match answer {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The value an answer gives under `spec`'s type rule, without looking at a quote.
pub fn typed_value(spec: &ColumnSpec, answer: &Value) -> Result<CellValue, &'static str> {
    match spec.kind {
        ColumnKind::Text => {
            let text = collapse(&scalar_text(answer).ok_or(EMPTY_TEXT)?);
            if text.is_empty() {
                Err(EMPTY_TEXT)
            } else if text.chars().count() > MAX_TEXT_CHARS {
                Err(TEXT_TOO_LONG)
            } else {
                Ok(CellValue::Text { text })
            }
        }
        ColumnKind::YesNo => match answer {
            Value::Bool(yes) => Ok(CellValue::YesNo { yes: *yes }),
            Value::String(s) => match s.trim().to_lowercase().as_str() {
                "yes" => Ok(CellValue::YesNo { yes: true }),
                "no" => Ok(CellValue::YesNo { yes: false }),
                _ => Err(NOT_YES_NO),
            },
            _ => Err(NOT_YES_NO),
        },
        ColumnKind::Choice => {
            let folded = collapse(&scalar_text(answer).ok_or(NOT_AN_OPTION)?).to_lowercase();
            spec.options
                .iter()
                .find(|o| o.to_lowercase() == folded)
                .map(|o| CellValue::Choice { option: o.clone() })
                .ok_or(NOT_AN_OPTION)
        }
        ColumnKind::Number => {
            read_number(&scalar_text(answer).ok_or(NOT_A_NUMBER)?).ok_or(NOT_A_NUMBER)
        }
    }
}

/// The number rule: the value's digits and sign occur in the quote as a whole number (not
/// inside a longer one, and not with another sign: "-3.2" is not in "improved by 3.2"),
/// and a stated unit occurs in the quote too, letter case included.
fn number_in_quote(value: &CellValue, quote: &str) -> Result<(), &'static str> {
    let CellValue::Number { decimal, unit, .. } = value else {
        return Ok(());
    };
    let quote = normalize(quote);
    let negative = decimal.starts_with('-');
    let wanted = decimal.trim_start_matches('-');
    let found = QUOTE_NUMBER.as_ref().is_some_and(|re| {
        re.find_iter(&quote).any(|m| {
            let text = m.as_str();
            // A hyphen glued to a word ("Top-1") is not a sign.
            let glued = quote[..m.start()]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
            let quote_negative = text.starts_with('-') && !glued;
            quote_negative == negative && unsigned_decimal(text) == wanted
        })
    });
    if !found {
        return Err(NUMBER_NOT_IN_QUOTE);
    }
    // A unit of letters must occur as a word (`GB`, not inside `GBM`); a symbol (`%`)
    // has no word boundary and is looked up as it is.
    match unit {
        Some(unit) => {
            let unit = normalize(unit);
            let present = if unit.chars().all(char::is_alphanumeric) {
                contains_at_word_bounds(&quote, &unit)
            } else {
                quote.contains(&unit)
            };
            if present {
                Ok(())
            } else {
                Err(UNIT_NOT_IN_QUOTE)
            }
        }
        None => Ok(()),
    }
}

/// Checks one raw answer against the passages of its call.
pub fn check(spec: &ColumnSpec, raw: &RawAnswer, passages: &[Passage]) -> Checked {
    let answer = match raw.answer.as_ref() {
        None => {
            return Checked {
                status: CellStatus::Failed,
                value: None,
                quote: None,
                passage: None,
                error: Some(MISSING_ANSWER.to_string()),
            }
        }
        Some(None) | Some(Some(Value::Null)) => {
            return Checked {
                status: CellStatus::NotFound,
                value: None,
                quote: None,
                passage: None,
                error: None,
            }
        }
        Some(Some(answer)) => answer,
    };
    let quote = raw
        .quote
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_string);
    let typed = typed_value(spec, answer);
    // An unverified cell still shows what the model said: its typed value when the type
    // rule passed, else the raw answer as text.
    let shown = |typed: &Result<CellValue, &'static str>| match typed {
        Ok(value) => Some(value.clone()),
        Err(_) => scalar_text(answer)
            .map(|t| {
                collapse(&t)
                    .chars()
                    .take(MAX_TEXT_CHARS)
                    .collect::<String>()
            })
            .filter(|t| !t.is_empty())
            .map(|text| CellValue::Text { text }),
    };
    let unverified = |error: &str, passage: Option<usize>| Checked {
        status: CellStatus::Unverified,
        value: shown(&typed),
        quote: quote.clone(),
        passage,
        error: Some(error.to_string()),
    };
    let Some(quote_text) = quote.as_deref() else {
        return unverified(MISSING_QUOTE, None);
    };
    let preferred = raw.passage.and_then(|n| n.checked_sub(1));
    let Some(found) = find_quote(quote_text, passages, preferred) else {
        return unverified(QUOTE_NOT_IN_PASSAGES, None);
    };
    let normal = normalize(quote_text);
    if normal.split_whitespace().count() < MIN_QUOTE_WORDS
        || normal.chars().count() < MIN_QUOTE_CHARS
    {
        return unverified(QUOTE_TOO_SHORT, Some(found));
    }
    let value = match typed.as_ref() {
        Ok(value) => value.clone(),
        Err(code) => return unverified(code, Some(found)),
    };
    if let Err(code) = number_in_quote(&value, quote_text) {
        return unverified(code, Some(found));
    }
    Checked {
        status: CellStatus::Verified,
        value: Some(value),
        quote,
        passage: Some(found),
        error: None,
    }
}

/// The statement the entailment model judges against the quote.
pub fn hypothesis(question: &str, value: &CellValue) -> String {
    format!("{}: {}", collapse(question), value.display())
}

/// Downgrades a verified cell whose quote the entailment model reads as contradicting
/// it. Without the entailment model, or when it fails and a fallback scores instead,
/// the cell is unchanged. Runs a local ONNX model: call it from a blocking task.
pub fn ground(checked: Checked, question: &str, scorers: &ScorerSet) -> Checked {
    if checked.status != CellStatus::Verified || scorers.entailment.is_none() {
        return checked;
    }
    let (Some(value), Some(quote)) = (checked.value.as_ref(), checked.quote.as_ref()) else {
        return checked;
    };
    let found = support(
        scorers.scorers(),
        &hypothesis(question, value),
        std::slice::from_ref(quote),
    );
    match found.contradiction {
        Some(c)
            if found.method == ScoringMethod::Entailment
                && c >= CONTRADICTION_CUTOFF
                && c > found.score =>
        {
            Checked {
                status: CellStatus::Unverified,
                error: Some(QUOTE_DOES_NOT_SUPPORT.to_string()),
                ..checked
            }
        }
        _ => checked,
    }
}

/// The value a user typed into a cell, under the column's type rule.
pub fn parse_edit(spec: &ColumnSpec, input: &str) -> ResearchResult<CellValue> {
    typed_value(spec, &Value::String(input.to_string()))
        .map_err(|code| ResearchError::Invalid(reason(code).to_string()))
}

/// A reason code in plain words.
pub fn reason(code: &str) -> &'static str {
    match code {
        QUOTE_NOT_IN_PASSAGES => "The quote does not occur in the passages the model was given.",
        QUOTE_DOES_NOT_SUPPORT => "The quote appears to contradict the answer.",
        MISSING_QUOTE => "The model gave no quote for its answer.",
        MISSING_ANSWER => "The model's reply had no answer for this column.",
        QUOTE_TOO_SHORT => "The quote is too short to back the answer.",
        NOT_A_NUMBER => "The answer is not a single number.",
        NUMBER_NOT_IN_QUOTE => "The number does not occur in the quote.",
        UNIT_NOT_IN_QUOTE => "The unit does not occur in the quote.",
        NOT_YES_NO => "The answer is not yes or no.",
        NOT_AN_OPTION => "The answer is not one of the column's options.",
        EMPTY_TEXT => "The answer is empty.",
        TEXT_TOO_LONG => "The answer is longer than 500 characters.",
        VALUE_NO_LONGER_FITS => "Your value does not fit the column's type or options any more.",
        _ => "The answer could not be checked.",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::harness::grounding::verify::EntailmentScorer;
    use crate::reranking::Entailment;

    fn spec(kind: ColumnKind, options: &[&str]) -> ColumnSpec {
        ColumnSpec {
            question: "What is reported?".to_string(),
            kind,
            options: options.iter().map(|o| o.to_string()).collect(),
            instructions: String::new(),
        }
    }

    fn passages() -> Vec<Passage> {
        [
            "We pre-train on LAION-400M with a contrastive loss.",
            "Top-1 accuracy reaches 91.2% on ImageNet, using 48 GB of memory and 1,234 steps.",
            "Code is available at the project page.",
        ]
        .iter()
        .enumerate()
        .map(|(i, t)| Passage {
            chunk_id: format!("c{i}"),
            text: t.to_string(),
            page: Some(i as u32 + 1),
            bboxes: None,
        })
        .collect()
    }

    fn raw(answer: Value, quote: &str, passage: usize) -> RawAnswer {
        RawAnswer {
            answer: Some(Some(answer)),
            quote: Some(quote.to_string()),
            passage: Some(passage),
        }
    }

    #[test]
    fn the_patterns_compile() {
        assert!(ANSWER_NUMBER.is_some());
        assert!(QUOTE_NUMBER.is_some());
    }

    #[test]
    fn a_correct_answer_with_its_quote_is_verified_at_the_quoted_passage() {
        let out = check(
            &spec(ColumnKind::Text, &[]),
            &raw(
                Value::from("contrastive pre-training on LAION-400M"),
                "We pre-train on LAION-400M",
                1,
            ),
            &passages(),
        );
        assert_eq!(out.status, CellStatus::Verified);
        assert_eq!(out.passage, Some(0));
        assert_eq!(out.error, None);
    }

    #[test]
    fn a_quote_in_another_passage_of_the_call_moves_the_cell_there() {
        let out = check(
            &spec(ColumnKind::YesNo, &[]),
            &raw(Value::from("Yes"), "Code is available", 1),
            &passages(),
        );
        assert_eq!(out.status, CellStatus::Verified);
        assert_eq!(out.passage, Some(2));
        assert_eq!(out.value, Some(CellValue::YesNo { yes: true }));
    }

    #[test]
    fn a_paraphrased_quote_is_unverified_and_keeps_the_answer() {
        let out = check(
            &spec(ColumnKind::Text, &[]),
            &raw(Value::from("LAION"), "They trained using LAION data", 1),
            &passages(),
        );
        assert_eq!(out.status, CellStatus::Unverified);
        assert_eq!(out.error.as_deref(), Some(QUOTE_NOT_IN_PASSAGES));
        assert_eq!(
            out.value,
            Some(CellValue::Text {
                text: "LAION".to_string()
            })
        );
    }

    #[test]
    fn numbers_must_occur_in_their_quote_whole_and_with_their_unit() {
        let s = spec(ColumnKind::Number, &[]);
        let quote =
            "Top-1 accuracy reaches 91.2% on ImageNet, using 48 GB of memory and 1,234 steps";
        let ok = [
            (Value::from("91.2%"), "91.2", Some("%")),
            (Value::from("48 GB"), "48", Some("GB")),
            (Value::from("1234"), "1234", None),
            (Value::from(48), "48", None),
        ];
        for (answer, decimal, unit) in ok {
            let out = check(&s, &raw(answer.clone(), quote, 2), &passages());
            assert_eq!(out.status, CellStatus::Verified, "{answer}");
            let Some(CellValue::Number {
                decimal: d,
                unit: u,
                ..
            }) = out.value
            else {
                panic!("{answer}: not a number");
            };
            assert_eq!((d.as_str(), u.as_deref()), (decimal, unit));
        }
        let bad = [
            (Value::from("91.20"), NUMBER_NOT_IN_QUOTE),
            (Value::from("1.2"), NUMBER_NOT_IN_QUOTE),
            (Value::from("48 TB"), UNIT_NOT_IN_QUOTE),
            (Value::from("48 G"), UNIT_NOT_IN_QUOTE),
            (Value::from("about 48"), NOT_A_NUMBER),
            (Value::from("48 and 91.2"), NOT_A_NUMBER),
        ];
        for (answer, code) in bad {
            let out = check(&s, &raw(answer.clone(), quote, 2), &passages());
            assert_eq!(out.status, CellStatus::Unverified, "{answer}");
            assert_eq!(out.error.as_deref(), Some(code), "{answer}");
        }
    }

    #[test]
    fn choices_and_yes_no_fold_case_and_reject_anything_else() {
        let choice = spec(ColumnKind::Choice, &["RCT", "Cohort"]);
        let quote = "We pre-train on LAION-400M";
        let out = check(&choice, &raw(Value::from(" rct "), quote, 1), &passages());
        assert_eq!(
            out.value,
            Some(CellValue::Choice {
                option: "RCT".to_string()
            })
        );
        let out = check(
            &choice,
            &raw(Value::from("Case study"), quote, 1),
            &passages(),
        );
        assert_eq!(out.error.as_deref(), Some(NOT_AN_OPTION));
        let yes_no = spec(ColumnKind::YesNo, &[]);
        let out = check(&yes_no, &raw(Value::from("maybe"), quote, 1), &passages());
        assert_eq!(out.error.as_deref(), Some(NOT_YES_NO));
        let out = check(&yes_no, &raw(Value::Bool(false), quote, 1), &passages());
        assert_eq!(out.value, Some(CellValue::YesNo { yes: false }));
    }

    #[test]
    fn text_is_one_to_five_hundred_characters() {
        let s = spec(ColumnKind::Text, &[]);
        let quote = "We pre-train on LAION-400M";
        let out = check(&s, &raw(Value::from("  "), quote, 1), &passages());
        assert_eq!(out.error.as_deref(), Some(EMPTY_TEXT));
        let out = check(
            &s,
            &raw(Value::from("x".repeat(501)), quote, 1),
            &passages(),
        );
        assert_eq!(out.error.as_deref(), Some(TEXT_TOO_LONG));
        let out = check(
            &s,
            &raw(Value::from("x".repeat(500)), quote, 1),
            &passages(),
        );
        assert_eq!(out.status, CellStatus::Verified);
    }

    fn one_passage(text: &str) -> Vec<Passage> {
        vec![Passage {
            chunk_id: "c".to_string(),
            text: text.to_string(),
            page: Some(1),
            bboxes: None,
        }]
    }

    #[test]
    fn only_an_explicit_null_is_not_found_and_a_missing_key_fails() {
        let s = spec(ColumnKind::Text, &[]);
        let null = RawAnswer {
            answer: Some(None),
            quote: Some("x".into()),
            passage: Some(1),
        };
        assert_eq!(check(&s, &null, &passages()).status, CellStatus::NotFound);
        let absent = RawAnswer {
            answer: None,
            quote: None,
            passage: None,
        };
        let out = check(&s, &absent, &passages());
        assert_eq!(out.status, CellStatus::Failed);
        assert_eq!(out.error.as_deref(), Some(MISSING_ANSWER));
        let parsed: RawAnswer = serde_json::from_str(r#"{"value":"LAION","quote":"q"}"#).unwrap();
        assert_eq!(parsed.answer, None, "a wrong key is not an answer");
        let parsed: RawAnswer = serde_json::from_str(r#"{"answer":null}"#).unwrap();
        assert_eq!(parsed.answer, Some(None));
        let parsed: RawAnswer = serde_json::from_str(r#"{"answer":"x","passage":2}"#).unwrap();
        assert_eq!(parsed.answer, Some(Some(Value::from("x"))));
        assert_eq!(
            serde_json::to_string(&null).unwrap(),
            r#"{"answer":null,"quote":"x","passage":1}"#
        );
    }

    #[test]
    fn a_missing_quote_is_unverified() {
        let s = spec(ColumnKind::Text, &[]);
        let unquoted = RawAnswer {
            answer: Some(Some(Value::from("LAION"))),
            quote: Some("  ".into()),
            passage: None,
        };
        let out = check(&s, &unquoted, &passages());
        assert_eq!(out.error.as_deref(), Some(MISSING_QUOTE));
    }

    #[test]
    fn a_quote_needs_three_words_and_fifteen_characters() {
        let s = spec(ColumnKind::YesNo, &[]);
        let passages = one_passage("Code is a Co de is avai thing, Co de is avai t here.");
        // One word, two words, three words of nine characters, four words of 13.
        for quote in ["Code", "Code is", "Code is a", "Co de is avai"] {
            let out = check(&s, &raw(Value::from("yes"), quote, 1), &passages);
            assert_eq!(out.status, CellStatus::Unverified, "{quote}");
            assert_eq!(out.error.as_deref(), Some(QUOTE_TOO_SHORT), "{quote}");
        }
        assert_eq!((MIN_QUOTE_WORDS, MIN_QUOTE_CHARS), (3, 15));
        // Four words, exactly fifteen characters.
        let out = check(
            &s,
            &raw(Value::from("yes"), "Co de is avai t", 1),
            &passages,
        );
        assert_eq!(out.status, CellStatus::Verified);
    }

    #[test]
    fn a_number_keeps_its_sign_and_is_not_cut_out_of_a_longer_one() {
        let s = spec(ColumnKind::Number, &[]);
        let passages = one_passage(
            "Validation loss improved by 3.2 points and fell by \u{2212}4.5 points, with Top-1 at 91.2% after 1,234 steps.",
        );
        let cases = [
            (
                "-3.2",
                "loss improved by 3.2 points",
                Some(NUMBER_NOT_IN_QUOTE),
            ),
            ("3.2", "loss improved by 3.2 points", None),
            ("+3.2", "loss improved by 3.2 points", None),
            (
                "4.5",
                "and fell by \u{2212}4.5 points",
                Some(NUMBER_NOT_IN_QUOTE),
            ),
            ("-4.5", "and fell by \u{2212}4.5 points", None),
            ("1", "with Top-1 at 91.2% after", None),
            ("-1", "with Top-1 at 91.2% after", Some(NUMBER_NOT_IN_QUOTE)),
            ("91", "with Top-1 at 91.2% after", Some(NUMBER_NOT_IN_QUOTE)),
            ("91.2%", "with Top-1 at 91.2% after", None),
            (
                "234",
                "at 91.2% after 1,234 steps",
                Some(NUMBER_NOT_IN_QUOTE),
            ),
            ("1,234", "at 91.2% after 1,234 steps", None),
        ];
        for (answer, quote, expected) in cases {
            let out = check(&s, &raw(Value::from(answer), quote, 1), &passages);
            assert_eq!(out.error.as_deref(), expected, "{answer} / {quote}");
        }
        // A quote that stops inside a number is not in the passage at all.
        let out = check(
            &s,
            &raw(Value::from("91"), "with Top-1 at 91", 1),
            &passages,
        );
        assert_eq!(out.error.as_deref(), Some(QUOTE_NOT_IN_PASSAGES));
    }

    #[test]
    fn a_unit_keeps_its_letter_case() {
        let s = spec(ColumnKind::Number, &[]);
        let passages = one_passage("The link sustains 5 Mb/s per node in practice.");
        let quote = "The link sustains 5 Mb/s per node";
        let out = check(&s, &raw(Value::from("5 MB/s"), quote, 1), &passages);
        assert_eq!(out.error.as_deref(), Some(UNIT_NOT_IN_QUOTE));
        let out = check(&s, &raw(Value::from("5 Mb/s"), quote, 1), &passages);
        assert_eq!(out.status, CellStatus::Verified);
    }

    /// Replays recorded NLI probabilities for every premise.
    struct Recorded(Entailment);

    impl EntailmentScorer for Recorded {
        fn entail(
            &self,
            _hypothesis: &str,
            premises: &[String],
        ) -> Result<Vec<Entailment>, String> {
            Ok(premises.iter().map(|_| self.0).collect())
        }
    }

    fn verified() -> Checked {
        Checked {
            status: CellStatus::Verified,
            value: Some(CellValue::YesNo { yes: true }),
            quote: Some("Code will not be released.".to_string()),
            passage: Some(0),
            error: None,
        }
    }

    fn scorers(contradiction: f32, entailment: f32) -> ScorerSet {
        ScorerSet {
            relevance: None,
            entailment: Some(Arc::new(Recorded(Entailment {
                contradiction,
                entailment,
                neutral: (1.0 - contradiction - entailment).max(0.0),
            }))),
        }
    }

    #[test]
    fn a_contradicting_quote_downgrades_and_a_neutral_one_does_not() {
        let question = "Is the code released?";
        let out = ground(verified(), question, &scorers(0.91, 0.02));
        assert_eq!(out.status, CellStatus::Unverified);
        assert_eq!(out.error.as_deref(), Some(QUOTE_DOES_NOT_SUPPORT));
        let out = ground(verified(), question, &scorers(0.30, 0.10));
        assert_eq!(out.status, CellStatus::Verified);
        let out = ground(verified(), question, &scorers(0.55, 0.60));
        assert_eq!(
            out.status,
            CellStatus::Verified,
            "entailment above contradiction"
        );
    }

    #[test]
    fn without_the_entailment_model_nothing_is_downgraded() {
        let out = ground(verified(), "Is the code released?", &ScorerSet::default());
        assert_eq!(out, verified());
        assert_eq!(
            hypothesis(" Is the code\nreleased? ", &CellValue::YesNo { yes: true }),
            "Is the code released?: yes"
        );
    }

    #[test]
    fn edits_follow_the_type_rules_with_plain_reasons() {
        let number = spec(ColumnKind::Number, &[]);
        assert_eq!(
            parse_edit(&number, "48 GB").unwrap(),
            CellValue::Number {
                lexeme: "48".into(),
                decimal: "48".into(),
                unit: Some("GB".into())
            }
        );
        let Err(ResearchError::Invalid(message)) = parse_edit(&number, "lots") else {
            panic!("accepted a non-number");
        };
        assert_eq!(message, reason(NOT_A_NUMBER));
        let choice = spec(ColumnKind::Choice, &["RCT", "Cohort"]);
        assert!(parse_edit(&choice, "cohort").is_ok());
        assert!(parse_edit(&choice, "Survey").is_err());
    }
}
