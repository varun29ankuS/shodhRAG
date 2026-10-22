//! Evidence tables: papers of a workspace (rows) asked the user's questions (columns).
//! Every filled cell holds an answer, the quote it rests on, checked word for word
//! against the passages the model was given, and the page and boxes of that passage.
//!
//! - [`store`]: the tables in `shodh.db` (schema 14) and the fill cache.
//! - [`normalize`]: the normal form quotes are compared in.
//! - [`validate`]: the deterministic rules a model's answer must pass to be `verified`.
//!
//! The model never has the last word: an answer whose quote is not in the passages, whose
//! value fails its column's type rule, or that the local entailment model reads as
//! contradicted by its quote is stored as `unverified` with the reason.

pub mod normalize;
pub mod parse;
pub mod store;
pub mod validate;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ResearchError, ResearchResult};

pub use store::{CacheEntry, TableStore, TableStoreHandle};

/// Longest `text` value, in characters.
pub const MAX_TEXT_CHARS: usize = 500;
/// Fewest options of a pick-list column.
pub const MIN_OPTIONS: usize = 2;
/// Most options of a pick-list column.
pub const MAX_OPTIONS: usize = 12;
/// Longest column question, in characters.
pub const MAX_QUESTION_CHARS: usize = 300;
/// Longest column instructions, in characters.
pub const MAX_INSTRUCTIONS_CHARS: usize = 1_000;
/// Longest pick-list option, in characters.
pub const MAX_OPTION_CHARS: usize = 80;
/// Longest table name, in characters.
pub const MAX_NAME_CHARS: usize = 120;

/// What a column's values are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    Text,
    Number,
    YesNo,
    Choice,
}

impl ColumnKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::YesNo => "yes_no",
            Self::Choice => "choice",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "text" => Some(Self::Text),
            "number" => Some(Self::Number),
            "yes_no" => Some(Self::YesNo),
            "choice" => Some(Self::Choice),
            _ => None,
        }
    }
}

/// Where a cell stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellStatus {
    /// Never filled.
    Empty,
    /// Waiting in a fill job.
    Queued,
    /// The quote occurs in the paper and the value passed its type check.
    Verified,
    /// The model answered, but the quote or the type check failed (`error` says which).
    Unverified,
    /// The model reported no answer in the passages it was given.
    NotFound,
    /// The user's value; never overwritten by a fill.
    Edited,
    /// The call failed (network, quota, parse); retried by the job.
    Failed,
}

impl CellStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Queued => "queued",
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::NotFound => "not_found",
            Self::Edited => "edited",
            Self::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "empty" => Some(Self::Empty),
            "queued" => Some(Self::Queued),
            "verified" => Some(Self::Verified),
            "unverified" => Some(Self::Unverified),
            "not_found" => Some(Self::NotFound),
            "edited" => Some(Self::Edited),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// A cell's value, typed by its column's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CellValue {
    Text {
        text: String,
    },
    /// `lexeme` is the number as printed; `decimal` is derived from it only by dropping
    /// thousands separators and writing a Unicode minus as `-`. Nothing is rounded.
    Number {
        lexeme: String,
        decimal: String,
        unit: Option<String>,
    },
    YesNo {
        yes: bool,
    },
    Choice {
        option: String,
    },
}

impl CellValue {
    /// The value as one line of text.
    pub fn display(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::Number { lexeme, unit, .. } => match unit {
                Some(unit) => format!("{lexeme} {unit}"),
                None => lexeme.clone(),
            },
            Self::YesNo { yes } => if *yes { "yes" } else { "no" }.to_string(),
            Self::Choice { option } => option.clone(),
        }
    }
}

/// Whitespace runs as one space, trimmed.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A column as the user defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnSpec {
    pub question: String,
    pub kind: ColumnKind,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub instructions: String,
}

impl ColumnSpec {
    /// The spec with whitespace collapsed, checked: a question, lengths, and 2–12 distinct
    /// options (compared case-folded) for a pick-list and none for other kinds.
    pub fn cleaned(&self) -> ResearchResult<ColumnSpec> {
        let invalid = |message: String| Err(ResearchError::Invalid(message));
        let question = collapse(&self.question);
        if question.is_empty() {
            return invalid("A column needs a question.".to_string());
        }
        if question.chars().count() > MAX_QUESTION_CHARS {
            return invalid(format!(
                "A column question is at most {MAX_QUESTION_CHARS} characters."
            ));
        }
        let instructions = collapse(&self.instructions);
        if instructions.chars().count() > MAX_INSTRUCTIONS_CHARS {
            return invalid(format!(
                "Column instructions are at most {MAX_INSTRUCTIONS_CHARS} characters."
            ));
        }
        let options: Vec<String> = self
            .options
            .iter()
            .map(|o| collapse(o))
            .filter(|o| !o.is_empty())
            .collect();
        if self.kind == ColumnKind::Choice {
            if options.len() < MIN_OPTIONS || options.len() > MAX_OPTIONS {
                return invalid(format!(
                    "A pick-list column needs {MIN_OPTIONS} to {MAX_OPTIONS} options."
                ));
            }
            let mut seen = HashSet::new();
            for option in &options {
                if option.chars().count() > MAX_OPTION_CHARS {
                    return invalid(format!(
                        "An option is at most {MAX_OPTION_CHARS} characters."
                    ));
                }
                if !seen.insert(option.to_lowercase()) {
                    return invalid(format!("The option \"{option}\" is listed twice."));
                }
            }
        } else if !options.is_empty() {
            return invalid("Only a pick-list column has options.".to_string());
        }
        Ok(ColumnSpec {
            question,
            kind: self.kind,
            options,
            instructions,
        })
    }

    /// `column_key`: the SHA-256 (hex) of the normalised question, kind, options and
    /// instructions. Two specs that differ only in whitespace share a key; any other edit
    /// changes it, so the column's cells refill ([`TableStore::update_column`] removes
    /// the cache entries only the old key's cells used).
    pub fn key(&self) -> String {
        let canonical = serde_json::json!({
            "v": 1,
            "question": collapse(&self.question),
            "kind": self.kind.as_str(),
            "options": self.options.iter().map(|o| collapse(o)).collect::<Vec<_>>(),
            "instructions": collapse(&self.instructions),
        });
        hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
    }
}

/// One evidence table of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceTable {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}

/// One column. `revision` grows by one each time the spec's key changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub id: String,
    pub table_id: String,
    pub position: i64,
    #[serde(flatten)]
    pub spec: ColumnSpec,
    pub key: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// One row: a paper, by its library identity ([`crate::research::path_key`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub id: String,
    pub table_id: String,
    pub position: i64,
    pub document_id: String,
    pub added_at: String,
}

/// One cell. `column_revision` older than the column's `revision`, or a `paper_hash` that
/// is not the file's, marks a stale cell: still shown, refilled on the next run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub table_id: String,
    pub row_id: String,
    pub column_id: String,
    pub status: CellStatus,
    pub value: Option<CellValue>,
    pub quote: Option<String>,
    pub chunk_id: Option<String>,
    pub page: Option<u32>,
    /// The matched passage's boxes, as the index stores them
    /// (`[{"page":3,"x0":..,"y0":..,"x1":..,"y1":..}]`, PDF points, bottom-left origin).
    pub bboxes: Option<String>,
    pub model: Option<String>,
    pub column_revision: Option<i64>,
    pub paper_hash: Option<String>,
    pub filled_at: Option<String>,
    pub edited_at: Option<String>,
    pub error: Option<String>,
}

/// A passage of a paper as the model is shown it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
    pub chunk_id: String,
    pub text: String,
    pub page: Option<u32>,
    pub bboxes: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(question: &str, kind: ColumnKind, options: &[&str], instructions: &str) -> ColumnSpec {
        ColumnSpec {
            question: question.to_string(),
            kind,
            options: options.iter().map(|o| o.to_string()).collect(),
            instructions: instructions.to_string(),
        }
    }

    #[test]
    fn kinds_and_statuses_round_trip_their_stored_names() {
        for kind in [
            ColumnKind::Text,
            ColumnKind::Number,
            ColumnKind::YesNo,
            ColumnKind::Choice,
        ] {
            assert_eq!(ColumnKind::parse(kind.as_str()), Some(kind));
        }
        for status in [
            CellStatus::Empty,
            CellStatus::Queued,
            CellStatus::Verified,
            CellStatus::Unverified,
            CellStatus::NotFound,
            CellStatus::Edited,
            CellStatus::Failed,
        ] {
            assert_eq!(CellStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(ColumnKind::parse("yes-no"), None);
        assert_eq!(CellStatus::parse("done"), None);
    }

    #[test]
    fn the_column_key_ignores_whitespace_and_changes_with_every_field() {
        let base = spec("Which dataset?", ColumnKind::Text, &[], "largest only");
        let spaced = spec(
            "  Which   dataset? ",
            ColumnKind::Text,
            &[],
            "largest\nonly",
        );
        assert_eq!(base.key(), spaced.key());
        assert_eq!(base.key().len(), 64);
        let changed = [
            spec("Which datasets?", ColumnKind::Text, &[], "largest only"),
            spec("Which dataset?", ColumnKind::Number, &[], "largest only"),
            spec("Which dataset?", ColumnKind::Text, &[], "smallest only"),
        ];
        for other in changed {
            assert_ne!(base.key(), other.key(), "{other:?}");
        }
        let a = spec("Design?", ColumnKind::Choice, &["RCT", "Cohort"], "");
        let b = spec("Design?", ColumnKind::Choice, &["Cohort", "RCT"], "");
        assert_ne!(a.key(), b.key(), "option order is part of the column");
    }

    #[test]
    fn cleaning_checks_questions_and_options() {
        let ok = spec(
            " Study design? ",
            ColumnKind::Choice,
            &["RCT", " Cohort "],
            "",
        )
        .cleaned()
        .unwrap();
        assert_eq!(ok.question, "Study design?");
        assert_eq!(ok.options, vec!["RCT", "Cohort"]);

        let cases = [
            spec("   ", ColumnKind::Text, &[], ""),
            spec("Design?", ColumnKind::Choice, &["RCT"], ""),
            spec("Design?", ColumnKind::Choice, &["RCT", "rct"], ""),
            spec("Design?", ColumnKind::Text, &["RCT", "Cohort"], ""),
            spec(
                &"q".repeat(MAX_QUESTION_CHARS + 1),
                ColumnKind::Text,
                &[],
                "",
            ),
        ];
        for case in cases {
            assert!(
                matches!(case.cleaned(), Err(ResearchError::Invalid(_))),
                "{case:?}"
            );
        }
        let thirteen: Vec<String> = (0..13).map(|i| format!("o{i}")).collect();
        let many = ColumnSpec {
            question: "Which?".to_string(),
            kind: ColumnKind::Choice,
            options: thirteen,
            instructions: String::new(),
        };
        assert!(many.cleaned().is_err());
    }

    #[test]
    fn values_serialise_with_their_kind_and_display_as_one_line() {
        let number = CellValue::Number {
            lexeme: "48".to_string(),
            decimal: "48".to_string(),
            unit: Some("GB".to_string()),
        };
        let json = serde_json::to_string(&number).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"number","lexeme":"48","decimal":"48","unit":"GB"}"#
        );
        assert_eq!(serde_json::from_str::<CellValue>(&json).unwrap(), number);
        assert_eq!(number.display(), "48 GB");
        assert_eq!(CellValue::YesNo { yes: false }.display(), "no");
    }
}
