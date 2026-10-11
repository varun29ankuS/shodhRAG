//! Evaluation datasets: questions, the source files (and pages and passages)
//! that answer them, and the short facts a correct answer states.
//!
//! Stored as JSON:
//!
//! ```json
//! {
//!   "id": "synthetic-v1",
//!   "description": "...",
//!   "cases": [
//!     {
//!       "id": "msa-notice",
//!       "question": "How much notice ends the agreement for convenience?",
//!       "sources": [{ "file": "contracts/msa.pdf", "pages": [3], "passage": "ninety (90) days written notice" }],
//!       "facts": ["90 days"]
//!     },
//!     { "id": "none-ceo", "question": "Who is the CEO?", "answerable": false }
//!   ]
//! }
//! ```
//!
//! `keep: false` drops a case from every run (used when reviewing generated
//! questions); `context` holds the passage a generated question was written from.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// A file that answers a case, optionally narrowed to pages and a passage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedSource {
    /// Path relative to the corpus folder (either separator).
    pub file: String,
    /// 1-based pages that hold the answer (paged formats only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<u32>,
    /// A short span of the source's text that answers the question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    pub id: String,
    pub question: String,
    /// `false`: the corpus does not answer the question; the answer must say so.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub answerable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<ExpectedSource>,
    /// Short strings a correct answer states (matched on normalised tokens).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
    /// Review decision; `false` excludes the case.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub keep: bool,
    /// For generated cases: the passage the question was written from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub id: String,
    pub description: String,
    pub cases: Vec<EvalCase>,
}

impl Dataset {
    /// Read and validate a dataset file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading dataset {}", path.display()))?;
        let dataset: Dataset = serde_json::from_str(&text)
            .with_context(|| format!("parsing dataset {}", path.display()))?;
        dataset
            .validate()
            .with_context(|| format!("invalid dataset {}", path.display()))?;
        Ok(dataset)
    }

    /// Write as pretty JSON with a trailing newline.
    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_json()?)
            .with_context(|| format!("writing dataset {}", path.display()))
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(format!("{}\n", serde_json::to_string_pretty(self)?))
    }

    /// Cases that take part in runs (`keep` is true).
    pub fn active_cases(&self) -> impl Iterator<Item = &EvalCase> {
        self.cases.iter().filter(|c| c.keep)
    }

    /// Every problem with the dataset; an error lists them all.
    pub fn validate(&self) -> Result<()> {
        let mut problems = Vec::new();
        if self.id.trim().is_empty() {
            problems.push("dataset id is empty".to_string());
        }
        if self.active_cases().next().is_none() {
            problems.push("dataset has no cases to run".to_string());
        }
        let mut seen = BTreeSet::new();
        for case in &self.cases {
            let id = &case.id;
            if id.trim().is_empty() {
                problems.push("a case has an empty id".to_string());
            }
            if !seen.insert(id.clone()) {
                problems.push(format!("duplicate case id '{id}'"));
            }
            if case.question.trim().is_empty() {
                problems.push(format!("case '{id}' has an empty question"));
            }
            if case.answerable && case.sources.is_empty() {
                problems.push(format!("answerable case '{id}' lists no sources"));
            }
            if !case.answerable && (!case.sources.is_empty() || !case.facts.is_empty()) {
                problems.push(format!(
                    "unanswerable case '{id}' must not list sources or facts"
                ));
            }
            if case.facts.iter().any(|f| f.trim().is_empty()) {
                problems.push(format!("case '{id}' has an empty fact"));
            }
            for source in &case.sources {
                if let Err(e) = check_relative(&source.file) {
                    problems.push(format!("case '{id}': {e}"));
                }
                if source.pages.contains(&0) {
                    problems.push(format!("case '{id}': pages are 1-based"));
                }
                if source
                    .passage
                    .as_deref()
                    .is_some_and(|p| p.trim().is_empty())
                {
                    problems.push(format!("case '{id}' has an empty passage"));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            bail!("{}", problems.join("; "))
        }
    }

    /// SHA-256 of the dataset id and its active cases (canonical JSON):
    /// identifies the exact question set a report was measured on.
    pub fn content_hash(&self) -> String {
        let active: Vec<&EvalCase> = self.active_cases().collect();
        let canonical = serde_json::to_vec(&(&self.id, active)).unwrap_or_default();
        hex::encode(Sha256::digest(&canonical))
    }
}

/// A corpus-relative path: not empty, not absolute, no `..`.
fn check_relative(file: &str) -> Result<(), String> {
    if file.trim().is_empty() {
        return Err("a source has an empty file".to_string());
    }
    let unified = file.replace('\\', "/");
    let path = Path::new(&unified);
    let escapes = path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if escapes || unified.starts_with('/') || unified.contains(':') {
        return Err(format!(
            "source file '{file}' must be relative to the corpus"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str) -> EvalCase {
        EvalCase {
            id: id.into(),
            question: "q?".into(),
            answerable: true,
            sources: vec![ExpectedSource {
                file: "a/b.pdf".into(),
                pages: vec![2],
                passage: Some("x".into()),
            }],
            facts: vec!["f".into()],
            keep: true,
            context: None,
        }
    }

    fn dataset(cases: Vec<EvalCase>) -> Dataset {
        Dataset {
            id: "d".into(),
            description: "test".into(),
            cases,
        }
    }

    #[test]
    fn defaults_are_omitted_and_round_trip() {
        let mut none = case("n");
        none.answerable = false;
        none.sources.clear();
        none.facts.clear();
        let ds = dataset(vec![case("a"), none]);
        let json = ds.to_json().unwrap();
        assert!(!json.contains("\"keep\""));
        assert_eq!(json.matches("\"answerable\": false").count(), 1);
        let back: Dataset = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ds);
        back.validate().unwrap();
    }

    #[test]
    fn every_problem_is_reported() {
        let mut dup = case("a");
        dup.sources.clear();
        let mut bad = case("b");
        bad.answerable = false;
        bad.sources[0].file = "../escape.pdf".into();
        bad.sources[0].pages = vec![0];
        let err = dataset(vec![case("a"), dup, bad])
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate case id 'a'"), "{err}");
        assert!(
            err.contains("answerable case 'a' lists no sources"),
            "{err}"
        );
        assert!(err.contains("unanswerable case 'b'"), "{err}");
        assert!(err.contains("must be relative"), "{err}");
        assert!(err.contains("1-based"), "{err}");
        assert!(dataset(vec![]).validate().is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let json =
            r#"{"id":"d","description":"","cases":[{"id":"a","question":"q","answer":"x"}]}"#;
        assert!(serde_json::from_str::<Dataset>(json).is_err());
    }

    #[test]
    fn pruned_cases_change_the_hash_and_leave_runs() {
        let full = dataset(vec![case("a"), case("b")]);
        let mut pruned = full.clone();
        pruned.cases[1].keep = false;
        assert_ne!(full.content_hash(), pruned.content_hash());
        assert_eq!(pruned.active_cases().count(), 1);
        assert_eq!(full.content_hash().len(), 64);
        assert_eq!(full.content_hash(), full.clone().content_hash());
    }
}
