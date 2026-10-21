# Evidence Table Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A researcher picks papers in a workspace and asks questions as columns. Shodh fills one row per paper, and every cell holds an answer, the quote it rests on (checked word for word against the paper), and the page.

**Architecture:**
- **Backend:** a new module `crates/shodh-rag/src/research/tables/`, with its tables in `shodh.db` at schema version 14.
- **Validation:** deterministic and local. The model never has the last word.
  1. a verbatim quote match after normalisation;
  2. a type rule per column kind;
  3. a contradiction check with the local NLI model.
- **Filling:**
  1. retrieval restricted to one paper;
  2. a token budget, giving one call per paper, or two at most;
  3. a tolerant JSON parse;
  4. a cache keyed by paper hash, column key and model.
- **Scope of this plan:** PR 1 and PR 2 are fully specified. PR 3 to PR 5 are outlined here, and get their detailed plan after PR 2's extraction quality has been checked on real papers.

**Tech Stack:**
- Rust: rusqlite/SQLCipher (`shodh.db`), `serde_json`, `regex`, `sha2`, `unicode-normalization` (new direct dependency), `async-trait`, `tokio`.
- Tauri v2 commands (PR 3).
- React/TypeScript under `app/src/features/tables/` (PR 4). Tests run with `node --test`.

**Spec:** `docs/superpowers/specs/2026-10-20-evidence-table-design.md` (approved). Read it together with this plan. §11 sets the order of work.

## Global Constraints

**Code quality**
- Production-grade code only: no TODO, stub, placeholder or mock in production code. Test doubles that implement a real trait (`TextModel`, `PaperIndex`, `EntailmentScorer`) live only in `#[cfg(test)]` modules.
- No `unwrap()`/`expect()` outside tests. `research/mod.rs` has `#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]`, and it covers `research::tables`.
  - Regexes are `static X: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(..).ok());`, as in `research/results/numbers.rs`.
  - A test asserts that each one compiles.
- Mutex locks are taken with `.lock().unwrap_or_else(|e| e.into_inner())`.

**No local builds: CI verifies**
- Run no local `cargo` or `npm` builds.
- The "Run" lines below are the commands CI runs (`.github/workflows/ci.yml`: `cargo test --package shodh-rag --lib`, `cargo test --package shodh-rag-app --lib`, `cargo clippy --workspace -- -D clippy::correctness -D clippy::suspicious -D clippy::perf`).
- Red/green is checked once per PR: push the branch, then read `gh pr checks <n>` / `gh run view <id> --log-failed`.

**Models, quota and privacy**
- Tests never call a real model. The free quota is about 50 requests a day.
- The eval (PR 5) is resumable across days.
- Never send the user's email or identity to any service.

**Spec values (copied verbatim)**

| Area | Rule |
|---|---|
| Column kinds | `text` (a string of at most 500 chars), `number` (a decimal string plus an optional unit, verbatim from the quote), `yes_no` (`yes` or `no`), `choice` (one of `options_json`, 2–12 options, set by the user) |
| Cell statuses | `empty`, `queued`, `verified`, `unverified`, `not_found`, `edited`, `failed` |
| `column_key` | the SHA-256 of the normalised `question`, `kind`, `options_json` and `instructions` |
| Retrieval | top 4 passages per column; union, dedup by chunk id; always include the paper's first chunk |
| Budget | passages are added in rank order, round-robin across columns, until about 6,000 tokens; at most 2 calls per paper; beyond that the remaining columns get the passages that fit and are marked as such |
| Reply envelope | `{"cells":[{"column":"<id>","answer":<value\|null>,"quote":"<verbatim>","passage":<n>}]}` |
| Retry | one retry, and only if the envelope can't be parsed at all; the retry counts against the job's calls |
| Quote normalisation | Unicode NFKC, collapsed whitespace, hyphenation at line breaks joined |
| Not found wording | "Not found in the passages searched", never "not mentioned" |
| Grounding error codes | `quote_not_in_passages`, `quote_does_not_support` |
| Daily cap (PR 3) | a per-day table cap in Settings, 30 calls by default |
| Quota (PR 3) | an HTTP 429 or the provider's daily-limit error pauses the job until the next day |

**Repo rules**
- `app/tsconfig.json` has `"strict": false`. Narrow unions with `'in'` checks or string discriminants, never by truthiness alone.
- Commits use plain messages: no `Co-Authored-By`, no generated footers.
- Each PR goes branch → commit → push → PR → CI green → merge. Merging is the user's call, or follows the "merge when green" rule.
- **Migration number:** `14` is the next free entry in `MIGRATIONS` on origin/master (13 = `run_memories`, from #83).
  - Before committing PR 1, re-run `grep -nE "^        [0-9]+,$" crates/shodh-rag/src/audit/store.rs | tail -1`.
  - Also check that no open PR adds 14 (`gh pr list`, then diff `audit/store.rs`).
  - If one does, take the next free number, and change it everywhere this plan says 14.

## Review Focus

These are input classes that the spec implies but does not spell out. Each has a pinned test in the task named.

1. **A reply whose quote holds braces or escaped quotes** (LaTeX `\{x\}`, set notation, `"…"` inside the quote).
   - The envelope must still parse. A naive brace counter that ignores JSON string state cuts it short.
   - Pinned in Task 5: `braces_and_quotes_inside_strings_do_not_end_the_envelope`.
2. **A paper that is not indexed yet, or search models that are missing.**
   - Retrieval returns nothing.
   - The fill must make no model call, and must mark the cells `failed` with `paper_not_indexed`, not `not_found`.
   - Pinned in Task 8: `an_unindexed_paper_costs_no_call_and_fails_its_cells`.
3. **Quotes that differ from the passage only typographically:**
   - an `ﬁ` ligature, curly quotes, an en dash;
   - a word split by a line-break hyphen;
   - a short quote that occurs only inside a longer word ("on" in "contrastive").

   The first two kinds must match, and the last must not. Pinned in Task 2.
4. **The user edits a cell or a column while a fill for that row is in flight.**
   - An `edited` cell is never overwritten.
   - A result for an older column revision is not stored.
   - No cache entry is left behind that no cell references.
   - Pinned in Task 4: `a_fill_never_overwrites_an_edit_or_a_newer_column`.
5. **A reply that leaves out a column, or repeats one.**
   - A left-out column becomes `failed` with `missing_in_reply`, so the job retries it. It is never `not_found`.
   - For a repeated column, the first entry wins.
   - Pinned in Task 8: `a_column_missing_from_the_reply_fails_and_a_repeat_is_ignored`.

---

## File Structure

```
crates/shodh-rag/
  Cargo.toml                         MODIFY  + unicode-normalization = "0.1"
  src/audit/store.rs                 MODIFY  migration 14 + doc comment
  src/research/mod.rs                MODIFY  pub mod tables;
  src/workspaces/store.rs            MODIFY  delete() also deletes the workspace's tables
  src/rag_engine.rs                  MODIFY  RAGEngine::first_chunk (PR 2) + tests
  src/research/tables/
    mod.rs        PR1  kinds, statuses, values, ColumnSpec (clean + key), Passage, row types
    normalize.rs  PR1  quote normalisation and verbatim location
    validate.rs   PR1  type rules, quote check, grounding downgrade, edit parsing, reasons
    store.rs      PR1  TableStore: tables, columns, rows, cells, fill cache, forget paths
    parse.rs      PR2  tolerant reply parse
    budget.rs     PR2  passage budget and the 1/2-call split
    prompt.rs     PR2  the fill prompt
    passages.rs   PR2  PaperIndex trait, RAGEngine impl, Result passages
    fill.rs       PR2  paper hash, staleness, the fill of one paper
    fixtures/     PR2  recorded replies (*.txt)
Cargo.lock                           MODIFY  shodh-rag gains "unicode-normalization"
```

`TableStore` opens its own connection to `shodh.db` (`open_shared_connection`), as `ResearchDb` and `WorkspaceStore` do. Migrations run once, and every component sees schema 14.

---

# PR 1 — Schema, store, model and validation rules

Branch: `feat/evidence-table-schema` from origin/master.

### Task 1: Migration 14 and the table model

**Files:**
- Modify: `crates/shodh-rag/src/audit/store.rs` (doc comment above `MIGRATIONS`; append entry 14 after entry 13)
- Modify: `crates/shodh-rag/src/research/mod.rs` (add `pub mod tables;` after `pub mod snippets;`)
- Create: `crates/shodh-rag/src/research/tables/mod.rs`

**Interfaces:**
- Produces, all in `crate::research::tables`:
  - `ColumnKind { Text, Number, YesNo, Choice }`, with `as_str()` / `parse(&str)`.
  - `CellStatus { Empty, Queued, Verified, Unverified, NotFound, Edited, Failed }`, with `as_str()` / `parse(&str)`.
  - `CellValue`, tagged `kind`:
    - `Text{text}`
    - `Number{lexeme, decimal, unit}`
    - `YesNo{yes}`
    - `Choice{option}`

    It has `display()`.
  - `ColumnSpec { question, kind, options, instructions }`, with `cleaned() -> ResearchResult<ColumnSpec>` and `key() -> String`.
  - Row types: `EvidenceTable`, `Column`, `TableRow`, `Cell`.
  - `Passage { chunk_id, text, page: Option<u32>, bboxes: Option<String> }`.
  - `collapse(&str) -> String`.
  - Constants: `MAX_TEXT_CHARS = 500`, `MIN_OPTIONS = 2`, `MAX_OPTIONS = 12`.

- [ ] **Step 1: Append migration 14** to `MIGRATIONS` in `crates/shodh-rag/src/audit/store.rs`, after the `13` entry:

```rust
    (
        14,
        // Evidence tables: a workspace's tables of papers (rows) × questions (columns).
        // A cell holds the answer, the verbatim quote it rests on and where it is; the
        // fill cache holds the model's raw answer per (paper content, column, model) so an
        // unchanged column of an unchanged paper never costs a second call. Deleting a
        // table, column or row removes its cells and the cache entries no other cell
        // uses; removing a paper from the library removes its rows.
        "CREATE TABLE evidence_table (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX evidence_table_workspace ON evidence_table(workspace_id, created_at);
        CREATE TABLE table_column (
            id TEXT PRIMARY KEY,
            table_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            question TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('text', 'number', 'yes_no', 'choice')),
            options_json TEXT NOT NULL,
            instructions TEXT NOT NULL,
            column_key TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision >= 1)
        );
        CREATE INDEX table_column_table ON table_column(table_id, position);
        CREATE TABLE table_row (
            id TEXT PRIMARY KEY,
            table_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            document_id TEXT NOT NULL,
            added_at TEXT NOT NULL,
            UNIQUE (table_id, document_id)
        );
        CREATE INDEX table_row_document ON table_row(document_id);
        CREATE TABLE table_cell (
            table_id TEXT NOT NULL,
            row_id TEXT NOT NULL,
            column_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN
                ('empty', 'queued', 'verified', 'unverified', 'not_found', 'edited', 'failed')),
            value_json TEXT NULL,
            quote TEXT NULL,
            chunk_id TEXT NULL,
            page INTEGER NULL,
            bbox_json TEXT NULL,
            model TEXT NULL,
            column_revision INTEGER NULL,
            paper_hash TEXT NULL,
            filled_at TEXT NULL,
            edited_at TEXT NULL,
            error TEXT NULL,
            PRIMARY KEY (row_id, column_id)
        );
        CREATE INDEX table_cell_table ON table_cell(table_id);
        CREATE INDEX table_cell_column ON table_cell(column_id);
        CREATE INDEX table_cell_paper ON table_cell(paper_hash);
        CREATE TABLE table_fill_job (
            id TEXT PRIMARY KEY,
            table_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN
                ('queued', 'running', 'paused', 'done', 'cancelled', 'failed')),
            planned_calls INTEGER NOT NULL CHECK (planned_calls >= 0),
            done_calls INTEGER NOT NULL CHECK (done_calls >= 0),
            next_attempt_at TEXT NULL,
            last_error TEXT NULL
        );
        CREATE INDEX table_fill_job_table ON table_fill_job(table_id, created_at);
        CREATE TABLE fill_cache (
            paper_hash TEXT NOT NULL,
            column_key TEXT NOT NULL,
            model TEXT NOT NULL,
            response_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (paper_hash, column_key, model)
        );",
    ),
```

`column_key` is stored on `table_column` and is not in the spec's column list. Two things need it in SQL:
- the cache purges;
- the "is another table still using this cache entry" check.

Recomputing it in Rust inside a transaction would mean loading every column of every table.

Then edit the doc comment above `MIGRATIONS`. Replace `and what happened to memories in each answer (13) use one` with `what happened to memories in each answer (13) and evidence tables (14) use one`.

- [ ] **Step 2: Write `research/tables/mod.rs`, with its tests at the bottom.**

```rust
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
pub mod store;
pub mod validate;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ResearchError, ResearchResult};

pub use store::{CacheEntry, TableStore};

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
        for kind in [ColumnKind::Text, ColumnKind::Number, ColumnKind::YesNo, ColumnKind::Choice] {
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
        let spaced = spec("  Which   dataset? ", ColumnKind::Text, &[], "largest\nonly");
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
        let ok = spec(" Study design? ", ColumnKind::Choice, &["RCT", " Cohort "], "")
            .cleaned()
            .unwrap();
        assert_eq!(ok.question, "Study design?");
        assert_eq!(ok.options, vec!["RCT", "Cohort"]);

        let cases = [
            spec("   ", ColumnKind::Text, &[], ""),
            spec("Design?", ColumnKind::Choice, &["RCT"], ""),
            spec("Design?", ColumnKind::Choice, &["RCT", "rct"], ""),
            spec("Design?", ColumnKind::Text, &["RCT", "Cohort"], ""),
            spec(&"q".repeat(MAX_QUESTION_CHARS + 1), ColumnKind::Text, &[], ""),
        ];
        for case in cases {
            assert!(matches!(case.cleaned(), Err(ResearchError::Invalid(_))), "{case:?}");
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
        assert_eq!(json, r#"{"kind":"number","lexeme":"48","decimal":"48","unit":"GB"}"#);
        assert_eq!(serde_json::from_str::<CellValue>(&json).unwrap(), number);
        assert_eq!(number.display(), "48 GB");
        assert_eq!(CellValue::YesNo { yes: false }.display(), "no");
    }
}
```

`mod.rs` declares `normalize`, `store` and `validate`, so Tasks 2–4 must land in the same PR before CI can compile it. Commit after each task anyway. PR 1's CI run is the gate.

- [ ] **Step 3: Commit**

```bash
git add crates/shodh-rag/src/audit/store.rs crates/shodh-rag/src/research/mod.rs crates/shodh-rag/src/research/tables/mod.rs
git commit -m "Evidence tables: schema 14 and the table model"
```

### Task 2: Quote normalisation and location

**Files:**
- Modify: `crates/shodh-rag/Cargo.toml`. After `regex = "1"`, add `unicode-normalization = "0.1"`.
- Modify: `Cargo.lock`. In the `[[package]] name = "shodh-rag"` dependency list, insert ` "unicode-normalization",` between ` "tracing-subscriber",` and ` "url",`. The lock already has `unicode-normalization 0.1.25` as a transitive dependency, so no new package entry is needed.
- Create: `crates/shodh-rag/src/research/tables/normalize.rs`

**Interfaces:**
- Produces:
  - `normalize(text: &str) -> String`
  - `find_quote(quote: &str, passages: &[Passage], preferred: Option<usize>) -> Option<usize>`. It returns a 0-based index into `passages`.
  - `contains_at_word_bounds(haystack: &str, needle: &str) -> bool`
  - `join_line_breaks(text: &str) -> String`

- [ ] **Step 1: Write `normalize.rs`, with its tests.**

```rust
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
static LINE_BREAK_HYPHEN: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(\p{L})[-\u{2010}\u{00AD}][ \t]*\r?\n[ \t]*(\p{L})").ok()
});

fn fold(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
        other => other,
    }
}

fn finish(text: &str) -> String {
    let folded: String = text
        .nfkc()
        .filter(|c| *c != '\u{00AD}')
        .map(fold)
        .collect();
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
        assert_eq!(join_line_breaks("contras-\ntive pre-\n  training"), "contrastive pretraining");
        assert_eq!(join_line_breaks("LAION-400M\nnext line"), "LAION-400M\nnext line");
    }

    #[test]
    fn a_hyphenated_compound_split_at_a_line_end_matches_with_its_hyphen() {
        let passages = [passage("a", "a state-of-the-\nart retriever")];
        assert_eq!(find_quote("a state-of-the-art retriever", &passages, None), Some(0));
    }

    #[test]
    fn a_quote_never_matches_inside_a_longer_word() {
        let passages = [passage("a", "contrastive pre-training on LAION-400M")];
        assert_eq!(find_quote("on", &passages, None), Some(0), "the word itself");
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
```

- [ ] **Step 2: Commit**

```bash
git add crates/shodh-rag/Cargo.toml Cargo.lock crates/shodh-rag/src/research/tables/normalize.rs
git commit -m "Evidence tables: quote normalisation and verbatim location"
```

### Task 3: Validation rules and the grounding downgrade

**Files:**
- Create: `crates/shodh-rag/src/research/tables/validate.rs`

**Interfaces:**
- Consumes:
  - `normalize`, `find_quote` and `contains_at_word_bounds` (Task 2)
  - `ColumnSpec`, `CellValue`, `CellStatus` and `Passage` (Task 1)
  - `crate::harness::grounding::verify::{support, ScorerSet, EntailmentScorer}`
  - `crate::harness::events::ScoringMethod`
- Produces:
  - `RawAnswer { answer: Option<serde_json::Value>, quote: Option<String>, passage: Option<usize> }` (Serialize + Deserialize; `passage` is 1-based, as the model writes it)
  - `Checked { status, value, quote, passage: Option<usize> (0-based), error: Option<String> }`
  - `check(spec, raw, passages) -> Checked`
  - `typed_value(spec, answer) -> Result<CellValue, &'static str>`
  - `ground(checked, question, scorers) -> Checked`
  - `hypothesis(question, value) -> String`
  - `parse_edit(spec, input) -> ResearchResult<CellValue>`
  - `reason(code) -> &'static str`
  - The error-code constants below
  - `CONTRADICTION_CUTOFF = 0.5`

**Two decisions the spec leaves open, fixed here:**
- **The NLI hypothesis** is `"{question}: {value}"`. A value alone, such as `yes` or `0.912`, is not a statement NLI can judge.
- **The cutoff:** a cell is downgraded only when the entailment model ran and the quote's highest contradiction probability is at least `CONTRADICTION_CUTOFF` and above its entailment probability. The cross-encoder and lexical fallbacks never downgrade, because relevance is not contradiction.
- No contradiction cutoff exists in the code base yet (`harness/events.rs` and `features/agent/grounding.ts` only display it). This one is uncalibrated, and PR 5's eval measures it (see Risks).

- [ ] **Step 1: Write `validate.rs`, with its tests.**

```rust
//! The rules a model's answer must pass. Deterministic; the model never has the last word.
//!
//! - The quote must occur, after [`normalize`], in the passage the model named or in
//!   another passage of the same call (`quote_not_in_passages` otherwise).
//! - The value must pass its column's type rule: `number` — its digits occur in the quote
//!   (nothing computed, rounded or converted) and a stated unit too; `yes_no` — yes or no;
//!   `choice` — one of the options, case-folded; `text` — 1 to 500 characters.
//! - A verified answer the local entailment model reads as contradicted by its quote is
//!   downgraded (`quote_does_not_support`).
//! - `answer: null` is `not_found`, shown as "Not found in the passages searched".

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
pub const NOT_A_NUMBER: &str = "not_a_number";
pub const NUMBER_NOT_IN_QUOTE: &str = "number_not_in_quote";
pub const UNIT_NOT_IN_QUOTE: &str = "unit_not_in_quote";
pub const NOT_YES_NO: &str = "not_yes_no";
pub const NOT_AN_OPTION: &str = "not_an_option";
pub const EMPTY_TEXT: &str = "empty_text";
pub const TEXT_TOO_LONG: &str = "text_too_long";

/// Contradiction probability at or above which (and above the entailment probability) a
/// verified cell is downgraded. Uncalibrated; the accuracy eval measures it.
pub const CONTRADICTION_CUTOFF: f32 = 0.5;

/// What the model said for one column. `passage` is 1-based, as numbered in the prompt.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawAnswer {
    pub answer: Option<Value>,
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

/// Numbers printed in a quote (signs are not digits and are compared without).
static QUOTE_NUMBER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|\.\d+").ok());

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
    let decimal = if negative { format!("-{unsigned}") } else { unsigned };
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

/// The number rule: the value's digits occur in the quote as a whole number (not inside
/// a longer one), and a stated unit occurs in the quote too.
fn number_in_quote(value: &CellValue, quote: &str) -> Result<(), &'static str> {
    let CellValue::Number { decimal, unit, .. } = value else {
        return Ok(());
    };
    let quote = normalize(quote);
    let wanted = decimal.trim_start_matches('-');
    let found = QUOTE_NUMBER
        .as_ref()
        .is_some_and(|re| re.find_iter(&quote).any(|m| unsigned_decimal(m.as_str()) == wanted));
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
    let Some(answer) = raw.answer.as_ref().filter(|a| !a.is_null()) else {
        return Checked {
            status: CellStatus::NotFound,
            value: None,
            quote: None,
            passage: None,
            error: None,
        };
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
            .map(|t| collapse(&t).chars().take(MAX_TEXT_CHARS).collect::<String>())
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
        NOT_A_NUMBER => "The answer is not a single number.",
        NUMBER_NOT_IN_QUOTE => "The number does not occur in the quote.",
        UNIT_NOT_IN_QUOTE => "The unit does not occur in the quote.",
        NOT_YES_NO => "The answer is not yes or no.",
        NOT_AN_OPTION => "The answer is not one of the column's options.",
        EMPTY_TEXT => "The answer is empty.",
        TEXT_TOO_LONG => "The answer is longer than 500 characters.",
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
            answer: Some(answer),
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
            &raw(Value::from("contrastive pre-training on LAION-400M"), "We pre-train on LAION-400M", 1),
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
        assert_eq!(out.value, Some(CellValue::Text { text: "LAION".to_string() }));
    }

    #[test]
    fn numbers_must_occur_in_their_quote_whole_and_with_their_unit() {
        let s = spec(ColumnKind::Number, &[]);
        let quote = "Top-1 accuracy reaches 91.2% on ImageNet, using 48 GB of memory and 1,234 steps";
        let ok = [
            (Value::from("91.2%"), "91.2", Some("%")),
            (Value::from("48 GB"), "48", Some("GB")),
            (Value::from("1234"), "1234", None),
            (Value::from(48), "48", None),
        ];
        for (answer, decimal, unit) in ok {
            let out = check(&s, &raw(answer.clone(), quote, 2), &passages());
            assert_eq!(out.status, CellStatus::Verified, "{answer}");
            let Some(CellValue::Number { decimal: d, unit: u, .. }) = out.value else {
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
        assert_eq!(out.value, Some(CellValue::Choice { option: "RCT".to_string() }));
        let out = check(&choice, &raw(Value::from("Case study"), quote, 1), &passages());
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
        let out = check(&s, &raw(Value::from("x".repeat(501)), quote, 1), &passages());
        assert_eq!(out.error.as_deref(), Some(TEXT_TOO_LONG));
        let out = check(&s, &raw(Value::from("x".repeat(500)), quote, 1), &passages());
        assert_eq!(out.status, CellStatus::Verified);
    }

    #[test]
    fn a_null_answer_is_not_found_and_a_missing_quote_is_unverified() {
        let s = spec(ColumnKind::Text, &[]);
        let none = RawAnswer { answer: Some(Value::Null), quote: Some("x".into()), passage: Some(1) };
        assert_eq!(check(&s, &none, &passages()).status, CellStatus::NotFound);
        let absent = RawAnswer { answer: None, quote: None, passage: None };
        assert_eq!(check(&s, &absent, &passages()).status, CellStatus::NotFound);
        let unquoted = RawAnswer { answer: Some(Value::from("LAION")), quote: Some("  ".into()), passage: None };
        let out = check(&s, &unquoted, &passages());
        assert_eq!(out.error.as_deref(), Some(MISSING_QUOTE));
    }

    /// Replays recorded NLI probabilities for every premise.
    struct Recorded(Entailment);

    impl EntailmentScorer for Recorded {
        fn entail(&self, _hypothesis: &str, premises: &[String]) -> Result<Vec<Entailment>, String> {
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
        assert_eq!(out.status, CellStatus::Verified, "entailment above contradiction");
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
            CellValue::Number { lexeme: "48".into(), decimal: "48".into(), unit: Some("GB".into()) }
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
```

**- [ ] **Step 2: Commit**

```bash
git add crates/shodh-rag/src/research/tables/validate.rs
git commit -m "Evidence tables: type rules, quote check and contradiction downgrade"
```

### Task 4: TableStore, its forget paths and the workspace hook

**Files:**
- Create: `crates/shodh-rag/src/research/tables/store.rs`
- Modify: `crates/shodh-rag/src/workspaces/store.rs`, `delete()`. Before `DELETE FROM workspaces`, add `crate::research::tables::store::delete_workspace_tables(&tx, id)?;`.

**Interfaces:**
- Consumes:
  - from Task 1: `ColumnSpec::{cleaned, key}`, `Column`, `Cell`, `CellValue`, `CellStatus`, `EvidenceTable`, `TableRow`
  - from Task 3: `parse_edit`
  - `crate::research::path_key`
  - `crate::audit::{open_shared_connection, AuditKey}`
- Produces `TableStore`. Each method is listed below with what it returns and what it does.

**Opening the store**

| Method | Returns |
|---|---|
| `open(path: &Path, key: Option<&AuditKey>)` | `ResearchResult<Self>` |

**Tables**

| Method | Returns | Behaviour |
|---|---|---|
| `create_table(workspace_id, name)` | `ResearchResult<EvidenceTable>` | `NotFound` if the workspace is unknown |
| `tables(workspace_id)` | `ResearchResult<Vec<EvidenceTable>>` | |
| `table(id)` | `ResearchResult<Option<EvidenceTable>>` | |
| `rename_table(id, name)` | `ResearchResult<EvidenceTable>` | |
| `delete_table(id)` | `ResearchResult<bool>` | |

**Columns**

| Method | Returns | Behaviour |
|---|---|---|
| `add_column(table_id, &ColumnSpec)` | `ResearchResult<Column>` | |
| `update_column(column_id, &ColumnSpec)` | `ResearchResult<Column>` | `revision + 1` only when the key changes |
| `delete_column(column_id)` | `ResearchResult<bool>` | |
| `columns(table_id)` | `ResearchResult<Vec<Column>>` | ordered by position |
| `column(column_id)` | `ResearchResult<Option<Column>>` | |

**Rows**

| Method | Returns | Behaviour |
|---|---|---|
| `add_rows(table_id, &[String])` | `ResearchResult<Vec<TableRow>>` | Paths are stored as `path_key`; duplicates are skipped |
| `remove_row(row_id)` | `ResearchResult<bool>` | |
| `rows(table_id)` | `ResearchResult<Vec<TableRow>>` | |

**Cells**

| Method | Returns | Behaviour |
|---|---|---|
| `cells(table_id)` | `ResearchResult<Vec<Cell>>` | |
| `cell(row_id, column_id)` | `ResearchResult<Option<Cell>>` | |
| `store_fill(&Cell, Option<&CacheEntry>)` | `ResearchResult<bool>` | Writes the cell, and the cache entry with it, only when:<br>• the row and column still exist in the cell's table;<br>• the column's revision is still `cell.column_revision`;<br>• the stored cell is not `edited`. |
| `edit_cell(row_id, column_id, input: &str)` | `ResearchResult<Cell>` | |
| `clear_cell(row_id, column_id)` | `ResearchResult<bool>` | |

**Cache and forgetting**

| Method | Returns | Behaviour |
|---|---|---|
| `cached(paper_hash, column_key, model)` | `ResearchResult<Option<String>>` | |
| `forget_documents(&[String])` | `ResearchResult<usize>` | Removes the rows of these papers (by `path_key`), their cells, and every cache entry of their paper hashes that no remaining cell uses. Returns the rows removed. |

**Other items Task 4 produces**
- `CacheEntry { paper_hash, column_key, model, response_json }`
- `pub(crate) fn delete_workspace_tables(conn: &Connection, workspace_id: &str) -> rusqlite::Result<usize>`

**Removing a paper from the library.** The spec says this "removes its rows' cells". This plan removes the rows too: a row whose paper is gone from the library has nothing left to show. The PR 3 hook calls `forget_documents` with the paper paths of the removed source.

- [ ] **Step 1: Write `store.rs`, with its tests.**

```rust
//! The evidence tables of `shodh.db` (schema 14): tables, columns, rows, cells and the
//! fill cache. One connection, like [`crate::research::ResearchDb`]; every write that
//! touches several tables runs in one immediate transaction.
//!
//! The fill cache is not owned by a table: two tables asking the same question of the
//! same paper with the same model share an entry. A cache entry is removed when the last
//! cell that uses it goes (its table, column or row deleted, its column's key changed, or
//! its paper removed from the library), so nothing of a paper's text outlives the cells
//! that showed it.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

use super::validate::parse_edit;
use super::{
    collapse, Cell, CellStatus, CellValue, Column, ColumnKind, ColumnSpec, EvidenceTable,
    TableRow, MAX_NAME_CHARS,
};
use crate::audit::{open_shared_connection, AuditKey};
use crate::research::{path_key, ResearchError, ResearchResult};

/// A model's raw answer for one column of one paper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub paper_hash: String,
    pub column_key: String,
    pub model: String,
    pub response_json: String,
}

/// The evidence tables of `shodh.db`.
pub struct TableStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for TableStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableStore").finish_non_exhaustive()
    }
}

fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn bad_column(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, message.into())
}

fn clean_name(name: &str) -> ResearchResult<String> {
    let name = collapse(name);
    if name.is_empty() {
        return Err(ResearchError::Invalid("A table needs a name.".to_string()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ResearchError::Invalid(format!(
            "A table name is at most {MAX_NAME_CHARS} characters."
        )));
    }
    Ok(name)
}

const TABLE_COLS: &str = "id, workspace_id, name, created_at, updated_at";

fn table_from(r: &Row<'_>) -> rusqlite::Result<EvidenceTable> {
    Ok(EvidenceTable {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        name: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

const COLUMN_COLS: &str = "id, table_id, position, question, kind, options_json, instructions, \
     column_key, revision, created_at, updated_at";

fn column_from(r: &Row<'_>) -> rusqlite::Result<Column> {
    let kind: String = r.get(4)?;
    let kind = ColumnKind::parse(&kind).ok_or_else(|| bad_column(4, format!("unknown kind {kind}")))?;
    let options_json: String = r.get(5)?;
    let options: Vec<String> =
        serde_json::from_str(&options_json).map_err(|e| bad_column(5, e.to_string()))?;
    Ok(Column {
        id: r.get(0)?,
        table_id: r.get(1)?,
        position: r.get(2)?,
        spec: ColumnSpec {
            question: r.get(3)?,
            kind,
            options,
            instructions: r.get(6)?,
        },
        key: r.get(7)?,
        revision: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

const ROW_COLS: &str = "id, table_id, position, document_id, added_at";

fn row_from(r: &Row<'_>) -> rusqlite::Result<TableRow> {
    Ok(TableRow {
        id: r.get(0)?,
        table_id: r.get(1)?,
        position: r.get(2)?,
        document_id: r.get(3)?,
        added_at: r.get(4)?,
    })
}

const CELL_COLS: &str = "table_id, row_id, column_id, status, value_json, quote, chunk_id, page, \
     bbox_json, model, column_revision, paper_hash, filled_at, edited_at, error";

fn cell_from(r: &Row<'_>) -> rusqlite::Result<Cell> {
    let status: String = r.get(3)?;
    let status =
        CellStatus::parse(&status).ok_or_else(|| bad_column(3, format!("unknown status {status}")))?;
    let value_json: Option<String> = r.get(4)?;
    let value = value_json
        .map(|json| serde_json::from_str::<CellValue>(&json))
        .transpose()
        .map_err(|e| bad_column(4, e.to_string()))?;
    Ok(Cell {
        table_id: r.get(0)?,
        row_id: r.get(1)?,
        column_id: r.get(2)?,
        status,
        value,
        quote: r.get(5)?,
        chunk_id: r.get(6)?,
        page: r.get(7)?,
        bboxes: r.get(8)?,
        model: r.get(9)?,
        column_revision: r.get(10)?,
        paper_hash: r.get(11)?,
        filled_at: r.get(12)?,
        edited_at: r.get(13)?,
        error: r.get(14)?,
    })
}

fn value_json(value: &Option<CellValue>) -> ResearchResult<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| ResearchError::Invalid(e.to_string()))
}

/// Removes the cache entries used by the cells `scope` selects (a condition on `c`, the
/// cell, with `?1` bound to `arg`) that no cell outside `scope` uses. `scope` is one of
/// this module's constants, never user text.
fn purge_cache(conn: &Connection, scope: &str, arg: &str) -> rusqlite::Result<usize> {
    let sql = format!(
        "DELETE FROM fill_cache WHERE EXISTS (
            SELECT 1 FROM table_cell c JOIN table_column k ON k.id = c.column_id
             WHERE ({scope}) AND c.paper_hash = fill_cache.paper_hash
               AND k.column_key = fill_cache.column_key)
         AND NOT EXISTS (
            SELECT 1 FROM table_cell c JOIN table_column k ON k.id = c.column_id
             WHERE NOT ({scope}) AND c.paper_hash = fill_cache.paper_hash
               AND k.column_key = fill_cache.column_key)"
    );
    conn.execute(&sql, params![arg])
}

const BY_TABLE: &str = "c.table_id = ?1";
const BY_COLUMN: &str = "c.column_id = ?1";
const BY_ROW: &str = "c.row_id = ?1";

/// Deletes one table with its columns, rows, cells, jobs and unshared cache entries.
fn delete_table_in(conn: &Connection, table_id: &str) -> rusqlite::Result<usize> {
    purge_cache(conn, BY_TABLE, table_id)?;
    conn.execute("DELETE FROM table_cell WHERE table_id = ?1", params![table_id])?;
    conn.execute("DELETE FROM table_row WHERE table_id = ?1", params![table_id])?;
    conn.execute("DELETE FROM table_column WHERE table_id = ?1", params![table_id])?;
    conn.execute("DELETE FROM table_fill_job WHERE table_id = ?1", params![table_id])?;
    conn.execute("DELETE FROM evidence_table WHERE id = ?1", params![table_id])
}

/// Deletes one row with its cells and the cache entries only it used.
fn delete_row_in(conn: &Connection, row_id: &str) -> rusqlite::Result<usize> {
    purge_cache(conn, BY_ROW, row_id)?;
    conn.execute("DELETE FROM table_cell WHERE row_id = ?1", params![row_id])?;
    conn.execute("DELETE FROM table_row WHERE id = ?1", params![row_id])
}

/// Deletes every table of a workspace (called inside the workspace's own delete
/// transaction, so a workspace never goes without its tables or the other way round).
pub(crate) fn delete_workspace_tables(conn: &Connection, workspace_id: &str) -> rusqlite::Result<usize> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM evidence_table WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace_id], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in &ids {
        delete_table_in(conn, id)?;
    }
    Ok(ids.len())
}

impl TableStore {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> ResearchResult<Self> {
        Ok(Self {
            conn: Mutex::new(open_shared_connection(path, key)?),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn create_table(&self, workspace_id: &str, name: &str) -> ResearchResult<EvidenceTable> {
        let name = clean_name(name)?;
        let conn = self.lock();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
            params![workspace_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(ResearchError::NotFound(format!("No workspace {workspace_id}.")));
        }
        let now = now_text();
        let table = EvidenceTable {
            id: new_id(),
            workspace_id: workspace_id.to_string(),
            name,
            created_at: now.clone(),
            updated_at: now,
        };
        conn.execute(
            "INSERT INTO evidence_table(id, workspace_id, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![table.id, table.workspace_id, table.name, table.created_at, table.updated_at],
        )?;
        Ok(table)
    }

    pub fn tables(&self, workspace_id: &str) -> ResearchResult<Vec<EvidenceTable>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TABLE_COLS} FROM evidence_table WHERE workspace_id = ?1 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![workspace_id], table_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn table(&self, id: &str) -> ResearchResult<Option<EvidenceTable>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {TABLE_COLS} FROM evidence_table WHERE id = ?1"),
                params![id],
                table_from,
            )
            .optional()?)
    }

    pub fn rename_table(&self, id: &str, name: &str) -> ResearchResult<EvidenceTable> {
        let name = clean_name(name)?;
        let changed = self.lock().execute(
            "UPDATE evidence_table SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, now_text()],
        )?;
        if changed == 0 {
            return Err(ResearchError::NotFound(format!("No table {id}.")));
        }
        self.table(id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No table {id}.")))
    }

    pub fn delete_table(&self, id: &str) -> ResearchResult<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = delete_table_in(&tx, id)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    pub fn add_column(&self, table_id: &str, spec: &ColumnSpec) -> ResearchResult<Column> {
        let spec = spec.cleaned()?;
        let options_json =
            serde_json::to_string(&spec.options).map_err(|e| ResearchError::Invalid(e.to_string()))?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence_table WHERE id = ?1)",
            params![table_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(ResearchError::NotFound(format!("No table {table_id}.")));
        }
        let position: i64 = tx.query_row(
            "SELECT IFNULL(MAX(position), -1) + 1 FROM table_column WHERE table_id = ?1",
            params![table_id],
            |r| r.get(0),
        )?;
        let now = now_text();
        let column = Column {
            id: new_id(),
            table_id: table_id.to_string(),
            position,
            key: spec.key(),
            spec,
            revision: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        tx.execute(
            &format!("INSERT INTO table_column({COLUMN_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
            params![
                column.id,
                column.table_id,
                column.position,
                column.spec.question,
                column.spec.kind.as_str(),
                options_json,
                column.spec.instructions,
                column.key,
                column.revision,
                column.created_at,
                column.updated_at
            ],
        )?;
        tx.execute(
            "UPDATE evidence_table SET updated_at = ?2 WHERE id = ?1",
            params![table_id, now],
        )?;
        tx.commit()?;
        Ok(column)
    }

    pub fn column(&self, column_id: &str) -> ResearchResult<Option<Column>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {COLUMN_COLS} FROM table_column WHERE id = ?1"),
                params![column_id],
                column_from,
            )
            .optional()?)
    }

    pub fn columns(&self, table_id: &str) -> ResearchResult<Vec<Column>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMN_COLS} FROM table_column WHERE table_id = ?1 ORDER BY position, created_at"
        ))?;
        let rows = stmt.query_map(params![table_id], column_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Changes a column. A change of its key (question, kind, options or instructions,
    /// whitespace aside) raises its revision, so its cells are stale, and removes the
    /// cache entries only its cells used.
    pub fn update_column(&self, column_id: &str, spec: &ColumnSpec) -> ResearchResult<Column> {
        let spec = spec.cleaned()?;
        let key = spec.key();
        let options_json =
            serde_json::to_string(&spec.options).map_err(|e| ResearchError::Invalid(e.to_string()))?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = tx
            .query_row(
                &format!("SELECT {COLUMN_COLS} FROM table_column WHERE id = ?1"),
                params![column_id],
                column_from,
            )
            .optional()?
            .ok_or_else(|| ResearchError::NotFound(format!("No column {column_id}.")))?;
        if current.key == key {
            return Ok(current);
        }
        purge_cache(&tx, BY_COLUMN, column_id)?;
        let now = now_text();
        tx.execute(
            "UPDATE table_column SET question = ?2, kind = ?3, options_json = ?4,
               instructions = ?5, column_key = ?6, revision = revision + 1, updated_at = ?7
             WHERE id = ?1",
            params![column_id, spec.question, spec.kind.as_str(), options_json, spec.instructions, key, now],
        )?;
        let updated = tx.query_row(
            &format!("SELECT {COLUMN_COLS} FROM table_column WHERE id = ?1"),
            params![column_id],
            column_from,
        )?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn delete_column(&self, column_id: &str) -> ResearchResult<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        purge_cache(&tx, BY_COLUMN, column_id)?;
        tx.execute("DELETE FROM table_cell WHERE column_id = ?1", params![column_id])?;
        let removed = tx.execute("DELETE FROM table_column WHERE id = ?1", params![column_id])?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Adds papers (file paths, stored by their library identity) as rows; a paper the
    /// table already has is skipped. Returns the rows added.
    pub fn add_rows(&self, table_id: &str, documents: &[String]) -> ResearchResult<Vec<TableRow>> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence_table WHERE id = ?1)",
            params![table_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(ResearchError::NotFound(format!("No table {table_id}.")));
        }
        let mut position: i64 = tx.query_row(
            "SELECT IFNULL(MAX(position), -1) + 1 FROM table_row WHERE table_id = ?1",
            params![table_id],
            |r| r.get(0),
        )?;
        let now = now_text();
        let mut added = Vec::new();
        for document in documents {
            let document_id = path_key(document);
            if document_id.is_empty() {
                continue;
            }
            let row = TableRow {
                id: new_id(),
                table_id: table_id.to_string(),
                position,
                document_id,
                added_at: now.clone(),
            };
            let inserted = tx.execute(
                &format!("INSERT OR IGNORE INTO table_row({ROW_COLS}) VALUES (?1, ?2, ?3, ?4, ?5)"),
                params![row.id, row.table_id, row.position, row.document_id, row.added_at],
            )?;
            if inserted > 0 {
                position += 1;
                added.push(row);
            }
        }
        tx.commit()?;
        Ok(added)
    }

    pub fn rows(&self, table_id: &str) -> ResearchResult<Vec<TableRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ROW_COLS} FROM table_row WHERE table_id = ?1 ORDER BY position, added_at"
        ))?;
        let rows = stmt.query_map(params![table_id], row_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn remove_row(&self, row_id: &str) -> ResearchResult<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = delete_row_in(&tx, row_id)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    pub fn cells(&self, table_id: &str) -> ResearchResult<Vec<Cell>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("SELECT {CELL_COLS} FROM table_cell WHERE table_id = ?1"))?;
        let rows = stmt.query_map(params![table_id], cell_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn cell(&self, row_id: &str, column_id: &str) -> ResearchResult<Option<Cell>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {CELL_COLS} FROM table_cell WHERE row_id = ?1 AND column_id = ?2"),
                params![row_id, column_id],
                cell_from,
            )
            .optional()?)
    }

    /// Stores a filled cell and, with it, its cache entry. Nothing is written (false) when
    /// the row or column is gone, the column changed since the fill started, or the user
    /// edited the cell meanwhile.
    pub fn store_fill(&self, cell: &Cell, cache: Option<&CacheEntry>) -> ResearchResult<bool> {
        let value = value_json(&cell.value)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT k.revision FROM table_column k JOIN table_row r ON r.table_id = k.table_id
                 WHERE k.id = ?1 AND r.id = ?2 AND k.table_id = ?3",
                params![cell.column_id, cell.row_id, cell.table_id],
                |r| r.get(0),
            )
            .optional()?;
        if current.is_none() || current != cell.column_revision {
            return Ok(false);
        }
        let written = tx.execute(
            &format!(
                "INSERT INTO table_cell({CELL_COLS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, NULL, ?14)
                 ON CONFLICT(row_id, column_id) DO UPDATE SET
                   status = excluded.status, value_json = excluded.value_json,
                   quote = excluded.quote, chunk_id = excluded.chunk_id, page = excluded.page,
                   bbox_json = excluded.bbox_json, model = excluded.model,
                   column_revision = excluded.column_revision, paper_hash = excluded.paper_hash,
                   filled_at = excluded.filled_at, error = excluded.error
                 WHERE table_cell.status <> 'edited'"
            ),
            params![
                cell.table_id,
                cell.row_id,
                cell.column_id,
                cell.status.as_str(),
                value,
                cell.quote,
                cell.chunk_id,
                cell.page,
                cell.bboxes,
                cell.model,
                cell.column_revision,
                cell.paper_hash,
                cell.filled_at,
                cell.error
            ],
        )?;
        if written == 0 {
            return Ok(false);
        }
        if let Some(entry) = cache {
            tx.execute(
                "INSERT OR REPLACE INTO fill_cache(paper_hash, column_key, model, response_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![entry.paper_hash, entry.column_key, entry.model, entry.response_json, now_text()],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// The user's value for a cell, under its column's type rule. The model's quote and
    /// passage no longer back it and are cleared; the paper hash stays.
    pub fn edit_cell(&self, row_id: &str, column_id: &str, input: &str) -> ResearchResult<Cell> {
        let column = self
            .column(column_id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No column {column_id}.")))?;
        let value = parse_edit(&column.spec, input)?;
        let value = value_json(&Some(value))?;
        let conn = self.lock();
        let row_table: Option<String> = conn
            .query_row("SELECT table_id FROM table_row WHERE id = ?1", params![row_id], |r| r.get(0))
            .optional()?;
        if row_table.as_deref() != Some(column.table_id.as_str()) {
            return Err(ResearchError::NotFound(format!("No row {row_id} in this table.")));
        }
        conn.execute(
            "INSERT INTO table_cell(table_id, row_id, column_id, status, value_json, column_revision, edited_at)
             VALUES (?1, ?2, ?3, 'edited', ?4, ?5, ?6)
             ON CONFLICT(row_id, column_id) DO UPDATE SET status = 'edited',
               value_json = excluded.value_json, quote = NULL, chunk_id = NULL, page = NULL,
               bbox_json = NULL, model = NULL, column_revision = excluded.column_revision,
               edited_at = excluded.edited_at, error = NULL",
            params![column.table_id, row_id, column_id, value, column.revision, now_text()],
        )?;
        drop(conn);
        self.cell(row_id, column_id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No cell {row_id}/{column_id}.")))
    }

    /// Empties a cell (an edit included); the next fill fills it.
    pub fn clear_cell(&self, row_id: &str, column_id: &str) -> ResearchResult<bool> {
        Ok(self.lock().execute(
            "DELETE FROM table_cell WHERE row_id = ?1 AND column_id = ?2",
            params![row_id, column_id],
        )? > 0)
    }

    pub fn cached(&self, paper_hash: &str, column_key: &str, model: &str) -> ResearchResult<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT response_json FROM fill_cache
                 WHERE paper_hash = ?1 AND column_key = ?2 AND model = ?3",
                params![paper_hash, column_key, model],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Forgets papers removed from the library: their rows in every table, their cells,
    /// and the cache entries no remaining cell uses. Returns the rows removed.
    pub fn forget_documents(&self, documents: &[String]) -> ResearchResult<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed = 0;
        for document in documents {
            let row_ids: Vec<String> = {
                let mut stmt = tx.prepare("SELECT id FROM table_row WHERE document_id = ?1")?;
                let rows = stmt.query_map(params![path_key(document)], |r| r.get(0))?;
                rows.collect::<Result<_, _>>()?
            };
            let hashes: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT DISTINCT c.paper_hash FROM table_cell c JOIN table_row r ON r.id = c.row_id
                     WHERE r.document_id = ?1 AND c.paper_hash IS NOT NULL",
                )?;
                let rows = stmt.query_map(params![path_key(document)], |r| r.get(0))?;
                rows.collect::<Result<_, _>>()?
            };
            for id in &row_ids {
                removed += delete_row_in(&tx, id)?;
            }
            // Every cache entry of the paper's content, whichever column it answered,
            // unless a remaining cell (another path to the same file) still shows it.
            for hash in &hashes {
                tx.execute(
                    "DELETE FROM fill_cache WHERE paper_hash = ?1
                       AND NOT EXISTS (SELECT 1 FROM table_cell WHERE paper_hash = ?1)",
                    params![hash],
                )?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        store: TableStore,
        workspace: String,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let workspace = WorkspaceStore::open(&path, None)
            .unwrap()
            .create(
                &NewWorkspace { name: "Survey".to_string(), ..Default::default() },
                WorkspaceAuthor::User,
            )
            .unwrap()
            .id;
        let store = TableStore::open(&path, None).unwrap();
        Fixture { _dir: dir, path, store, workspace }
    }

    fn spec(question: &str, kind: ColumnKind) -> ColumnSpec {
        ColumnSpec {
            question: question.to_string(),
            kind,
            options: Vec::new(),
            instructions: String::new(),
        }
    }

    fn filled(column: &Column, row: &TableRow, hash: &str) -> Cell {
        Cell {
            table_id: column.table_id.clone(),
            row_id: row.id.clone(),
            column_id: column.id.clone(),
            status: CellStatus::Verified,
            value: Some(CellValue::Text { text: "LAION-400M".to_string() }),
            quote: Some("We pre-train on LAION-400M".to_string()),
            chunk_id: Some("chunk-1".to_string()),
            page: Some(3),
            bboxes: Some(r#"[{"page":3,"x0":1.0,"y0":2.0,"x1":3.0,"y1":4.0}]"#.to_string()),
            model: Some("test-model".to_string()),
            column_revision: Some(column.revision),
            paper_hash: Some(hash.to_string()),
            filled_at: Some(now_text()),
            edited_at: None,
            error: None,
        }
    }

    fn entry(column: &Column, hash: &str) -> CacheEntry {
        CacheEntry {
            paper_hash: hash.to_string(),
            column_key: column.key.clone(),
            model: "test-model".to_string(),
            response_json: r#"{"answer":"LAION-400M","quote":"We pre-train on LAION-400M","passage":1}"#.to_string(),
        }
    }

    #[test]
    fn tables_columns_and_rows_round_trip_in_order() {
        let f = fixture();
        assert!(matches!(f.store.create_table("ws-missing", "T"), Err(ResearchError::NotFound(_))));
        let table = f.store.create_table(&f.workspace, "  Retrieval  papers ").unwrap();
        assert_eq!(table.name, "Retrieval papers");
        let a = f.store.add_column(&table.id, &spec("Method?", ColumnKind::Text)).unwrap();
        let b = f.store.add_column(&table.id, &spec("Sample size?", ColumnKind::Number)).unwrap();
        assert_eq!((a.position, b.position), (0, 1));
        assert_eq!(f.store.columns(&table.id).unwrap(), vec![a.clone(), b]);
        let rows = f
            .store
            .add_rows(&table.id, &["/papers/A.pdf".to_string(), "/papers/B.pdf".to_string(), "/papers/A.pdf".to_string()])
            .unwrap();
        assert_eq!(rows.len(), 2, "a paper is a row once");
        assert_eq!(rows[0].document_id, path_key("/papers/A.pdf"));
        let again = f.store.add_rows(&table.id, &["/papers/A.pdf".to_string()]).unwrap();
        assert!(again.is_empty());
        assert_eq!(f.store.rows(&table.id).unwrap(), rows);
        let renamed = f.store.rename_table(&table.id, "Dense retrieval").unwrap();
        assert_eq!(renamed.name, "Dense retrieval");
        assert_eq!(f.store.tables(&f.workspace).unwrap(), vec![renamed]);
    }

    #[test]
    fn editing_a_column_bumps_its_revision_only_when_its_key_changes() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f.store.add_column(&table.id, &spec("Method?", ColumnKind::Text)).unwrap();
        let same = f.store.update_column(&column.id, &spec("  Method? ", ColumnKind::Text)).unwrap();
        assert_eq!(same.revision, 1);
        let changed = f.store.update_column(&column.id, &spec("Main method?", ColumnKind::Text)).unwrap();
        assert_eq!(changed.revision, 2);
        assert_ne!(changed.key, column.key);
    }

    #[test]
    fn a_fill_never_overwrites_an_edit_or_a_newer_column() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f.store.add_column(&table.id, &spec("Dataset?", ColumnKind::Text)).unwrap();
        let row = f.store.add_rows(&table.id, &["/p/a.pdf".to_string()]).unwrap().remove(0);

        assert!(f.store.store_fill(&filled(&column, &row, "h1"), Some(&entry(&column, "h1"))).unwrap());
        assert!(f.store.cached("h1", &column.key, "test-model").unwrap().is_some());

        let edited = f.store.edit_cell(&row.id, &column.id, "  my value ").unwrap();
        assert_eq!(edited.status, CellStatus::Edited);
        assert_eq!(edited.value, Some(CellValue::Text { text: "my value".to_string() }));
        assert_eq!(edited.quote, None);
        assert_eq!(edited.paper_hash.as_deref(), Some("h1"));

        // A fill that finishes after the edit leaves the edit and writes no cache entry.
        let late = filled(&column, &row, "h2");
        assert!(!f.store.store_fill(&late, Some(&entry(&column, "h2"))).unwrap());
        assert_eq!(f.store.cell(&row.id, &column.id).unwrap().unwrap().status, CellStatus::Edited);
        assert!(f.store.cached("h2", &column.key, "test-model").unwrap().is_none());

        // A fill for an older revision of the column is not stored.
        f.store.clear_cell(&row.id, &column.id).unwrap();
        let newer = f.store.update_column(&column.id, &spec("Largest dataset?", ColumnKind::Text)).unwrap();
        assert!(!f.store.store_fill(&filled(&column, &row, "h1"), Some(&entry(&column, "h1"))).unwrap());
        assert!(f.store.cell(&row.id, &column.id).unwrap().is_none());
        assert!(f.store.store_fill(&filled(&newer, &row, "h1"), Some(&entry(&newer, "h1"))).unwrap());

        // A cell of a deleted row is not stored.
        f.store.remove_row(&row.id).unwrap();
        assert!(!f.store.store_fill(&filled(&newer, &row, "h1"), None).unwrap());
    }

    #[test]
    fn edits_follow_the_column_type() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f.store.add_column(&table.id, &spec("Released?", ColumnKind::YesNo)).unwrap();
        let row = f.store.add_rows(&table.id, &["/p/a.pdf".to_string()]).unwrap().remove(0);
        assert!(matches!(f.store.edit_cell(&row.id, &column.id, "perhaps"), Err(ResearchError::Invalid(_))));
        let cell = f.store.edit_cell(&row.id, &column.id, "No").unwrap();
        assert_eq!(cell.value, Some(CellValue::YesNo { yes: false }));
        let other = f.store.create_table(&f.workspace, "U").unwrap();
        let foreign = f.store.add_rows(&other.id, &["/p/b.pdf".to_string()]).unwrap().remove(0);
        assert!(matches!(f.store.edit_cell(&foreign.id, &column.id, "yes"), Err(ResearchError::NotFound(_))));
    }

    #[test]
    fn deleting_a_table_removes_everything_but_cache_entries_another_table_uses() {
        let f = fixture();
        let first = f.store.create_table(&f.workspace, "A").unwrap();
        let second = f.store.create_table(&f.workspace, "B").unwrap();
        let q = spec("Dataset?", ColumnKind::Text);
        let only = spec("Licence?", ColumnKind::Text);
        let c1 = f.store.add_column(&first.id, &q).unwrap();
        let c1b = f.store.add_column(&first.id, &only).unwrap();
        let c2 = f.store.add_column(&second.id, &q).unwrap();
        let r1 = f.store.add_rows(&first.id, &["/p/a.pdf".to_string()]).unwrap().remove(0);
        let r2 = f.store.add_rows(&second.id, &["/p/a.pdf".to_string()]).unwrap().remove(0);
        f.store.store_fill(&filled(&c1, &r1, "h"), Some(&entry(&c1, "h"))).unwrap();
        f.store.store_fill(&filled(&c1b, &r1, "h"), Some(&entry(&c1b, "h"))).unwrap();
        f.store.store_fill(&filled(&c2, &r2, "h"), Some(&entry(&c2, "h"))).unwrap();

        assert!(f.store.delete_table(&first.id).unwrap());
        assert!(f.store.table(&first.id).unwrap().is_none());
        assert!(f.store.columns(&first.id).unwrap().is_empty());
        assert!(f.store.rows(&first.id).unwrap().is_empty());
        assert!(f.store.cells(&first.id).unwrap().is_empty());
        assert!(f.store.cached("h", &c1.key, "test-model").unwrap().is_some(), "table B still uses it");
        assert!(f.store.cached("h", &c1b.key, "test-model").unwrap().is_none());
        assert!(!f.store.delete_table(&first.id).unwrap());
    }

    #[test]
    fn changing_or_deleting_a_column_removes_its_cache_entries() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f.store.add_column(&table.id, &spec("Dataset?", ColumnKind::Text)).unwrap();
        let row = f.store.add_rows(&table.id, &["/p/a.pdf".to_string()]).unwrap().remove(0);
        f.store.store_fill(&filled(&column, &row, "h"), Some(&entry(&column, "h"))).unwrap();
        let changed = f.store.update_column(&column.id, &spec("Datasets?", ColumnKind::Text)).unwrap();
        assert!(f.store.cached("h", &column.key, "test-model").unwrap().is_none());
        f.store.store_fill(&filled(&changed, &row, "h"), Some(&entry(&changed, "h"))).unwrap();
        assert!(f.store.delete_column(&changed.id).unwrap());
        assert!(f.store.cached("h", &changed.key, "test-model").unwrap().is_none());
        assert!(f.store.cells(&table.id).unwrap().is_empty());
    }

    #[test]
    fn forgetting_a_paper_removes_its_rows_cells_and_cache_in_every_table() {
        let f = fixture();
        let one = f.store.create_table(&f.workspace, "A").unwrap();
        let two = f.store.create_table(&f.workspace, "B").unwrap();
        let c1 = f.store.add_column(&one.id, &spec("Dataset?", ColumnKind::Text)).unwrap();
        let c2 = f.store.add_column(&two.id, &spec("Method?", ColumnKind::Text)).unwrap();
        let gone = vec!["/p/gone.pdf".to_string()];
        let kept = vec!["/p/kept.pdf".to_string()];
        let r1 = f.store.add_rows(&one.id, &gone).unwrap().remove(0);
        let r2 = f.store.add_rows(&two.id, &gone).unwrap().remove(0);
        let r3 = f.store.add_rows(&two.id, &kept).unwrap().remove(0);
        f.store.store_fill(&filled(&c1, &r1, "hg"), Some(&entry(&c1, "hg"))).unwrap();
        f.store.store_fill(&filled(&c2, &r2, "hg"), Some(&entry(&c2, "hg"))).unwrap();
        f.store.store_fill(&filled(&c2, &r3, "hk"), Some(&entry(&c2, "hk"))).unwrap();

        assert_eq!(f.store.forget_documents(&gone).unwrap(), 2);
        assert!(f.store.rows(&one.id).unwrap().is_empty());
        assert_eq!(f.store.rows(&two.id).unwrap(), vec![r3.clone()]);
        assert!(f.store.cached("hg", &c1.key, "test-model").unwrap().is_none());
        assert!(f.store.cached("hg", &c2.key, "test-model").unwrap().is_none());
        assert!(f.store.cached("hk", &c2.key, "test-model").unwrap().is_some());
        assert_eq!(f.store.cells(&two.id).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_workspace_deletes_its_tables() {
        let f = fixture();
        let table = f.store.create_table(&f.workspace, "T").unwrap();
        let column = f.store.add_column(&table.id, &spec("Dataset?", ColumnKind::Text)).unwrap();
        let row = f.store.add_rows(&table.id, &["/p/a.pdf".to_string()]).unwrap().remove(0);
        f.store.store_fill(&filled(&column, &row, "h"), Some(&entry(&column, "h"))).unwrap();
        let workspaces = WorkspaceStore::open(&f.path, None).unwrap();
        assert!(workspaces.delete(&f.workspace).unwrap());
        assert!(f.store.tables(&f.workspace).unwrap().is_empty());
        assert!(f.store.cells(&table.id).unwrap().is_empty());
        assert!(f.store.cached("h", &column.key, "test-model").unwrap().is_none());
    }
}
```

- [ ] **Step 2: Wire the workspace hook.** In `crates/shodh-rag/src/workspaces/store.rs`, find the line in `delete` that runs `DELETE FROM workspace_instructions …`. Directly after its `)?;`, insert:

```rust
        crate::research::tables::store::delete_workspace_tables(&tx, id)?;
```

`WorkspaceError` already converts from `rusqlite::Error` (the existing `tx.execute(..)?` calls rely on it). Update the doc comment: `Deletes a workspace, its instruction history, its source list and its evidence tables (never the sources themselves).`

- [ ] **Step 3: Commit**

```bash
git add crates/shodh-rag/src/research/tables/store.rs crates/shodh-rag/src/workspaces/store.rs
git commit -m "Evidence tables: store, fill cache and forget paths"
```

### PR 1 end: push, open the PR and get CI green

- [ ] Run `git push -u origin feat/evidence-table-schema`.
- [ ] Run `gh pr create --title "Evidence tables: schema, store and validation rules" --body "<summary of Tasks 1–4>"`.
- [ ] Watch `gh pr checks <n> --watch`. If a check fails:
  - read `gh run view <run> --log-failed`;
  - fix the root cause;
  - push again.

  Never weaken a test.
- [ ] CI runs `cargo test --package shodh-rag --lib research::tables` (inside the full `--lib` run) and the clippy job.
- [ ] **PR 1 is independently mergeable:** it adds no command and changes no behaviour, apart from deleting a workspace's tables.

---

# PR 2 — The fill pipeline for one paper × one column set

Branch: `feat/evidence-table-fill`, from origin/master after PR 1 has merged.

PR 2 adds the library function that fills one paper. It needs:
- a `TextModel`;
- a `PaperIndex`;
- a `ScorerSet`.

There is no Tauri command yet. The job queue and the commands are PR 3. Every test runs on recorded replies and in-memory doubles, so CI makes no model call and needs no search model.

The module declarations: in `research/tables/mod.rs`, after `pub mod normalize;`, add:

```rust
pub mod budget;
pub mod fill;
pub mod parse;
pub mod passages;
pub mod prompt;
```

Also extend the module doc list with:
- [`fill`]: retrieval, budget, call, parse, validation and cache for one paper.
- [`passages`]: where passages come from.

### Task 5: Tolerant reply parse

**Files:**
- Create: `crates/shodh-rag/src/research/tables/parse.rs`

**Interfaces:**
- Consumes: `RawAnswer` (Task 3).
- Produces:
  - `ParsedCell { column: String, raw: RawAnswer }`. `column` is trimmed and lower-cased.
  - `parse_reply(reply: &str) -> Option<Vec<ParsedCell>>`. It returns `None` only when no `{"cells":[…]}` envelope can be found. That one case is what earns the single retry.

- [ ] **Step 1: Write `parse.rs`, with its tests.**

```rust
//! Reading a free model's reply. No provider path supports structured output, so the
//! reply is searched for the first balanced `{…}` that parses as JSON and has a `cells`
//! array: reasoning blocks (`<think>…</think>`), code fences and prose around it are
//! skipped, braces and quotes inside JSON strings do not count, and unknown keys are
//! ignored. A cell without a `column` or without an `answer` key is dropped; the fill
//! then marks its column `missing_in_reply`.

use serde_json::Value;

use super::validate::RawAnswer;

/// One cell of a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCell {
    /// The column tag as the model wrote it, trimmed and lower-cased.
    pub column: String,
    pub raw: RawAnswer,
}

const THINK_END: &str = "</think>";

/// The reply after its last closed reasoning block.
fn after_thinking(reply: &str) -> &str {
    // ASCII lower-casing keeps byte offsets.
    match reply.to_ascii_lowercase().rfind(THINK_END) {
        Some(at) => &reply[at + THINK_END.len()..],
        None => reply,
    }
}

/// Byte length of the balanced object `text` starts with (`text` starts with `{`),
/// skipping braces inside JSON strings.
fn balanced_len(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn passage_number(value: Option<&Value>) -> Option<usize> {
    match value? {
        Value::Number(n) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        Value::String(s) => s.trim().trim_start_matches('[').trim_end_matches(']').trim().parse().ok(),
        _ => None,
    }
}

fn cell_of(value: &Value) -> Option<ParsedCell> {
    let object = value.as_object()?;
    let column = match object.get("column")? {
        Value::String(s) => s.trim().to_lowercase(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let answer = match object.get("answer")? {
        Value::Null => None,
        other => Some(other.clone()),
    };
    Some(ParsedCell {
        column,
        raw: RawAnswer {
            answer,
            quote: object.get("quote").and_then(Value::as_str).map(str::to_string),
            passage: passage_number(object.get("passage")),
        },
    })
}

/// The cells of a reply, or `None` when it holds no `{"cells":[…]}` envelope.
pub fn parse_reply(reply: &str) -> Option<Vec<ParsedCell>> {
    let text = after_thinking(reply);
    let mut from = 0;
    while let Some(offset) = text[from..].find('{') {
        let start = from + offset;
        if let Some(len) = balanced_len(&text[start..]) {
            if let Ok(Value::Object(envelope)) = serde_json::from_str::<Value>(&text[start..start + len]) {
                if let Some(Value::Array(cells)) = envelope.get("cells") {
                    return Some(cells.iter().filter_map(cell_of).collect());
                }
            }
        }
        from = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_thinking_and_prose_around_the_envelope_are_skipped() {
        let reply = "<think>\nPassage 2 has {the number}.\n</think>\nSure! Here it is:\n```json\n\
            {\"cells\":[{\"column\":\" C1 \",\"answer\":\"0.912\",\"quote\":\"Recall@10 of 0.912\",\"passage\":2,\"confidence\":0.8}]}\n\
            ```\nHope this helps {really}.";
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].column, "c1");
        assert_eq!(cells[0].raw.answer, Some(Value::from("0.912")));
        assert_eq!(cells[0].raw.passage, Some(2));
    }

    #[test]
    fn braces_and_quotes_inside_strings_do_not_end_the_envelope() {
        let reply = r#"{"cells":[{"column":"c1","answer":"the set \\{x : x > 0\\}","quote":"We define \"positive\" as {x | x > 0}.","passage":"[3]"}]}"#;
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells[0].raw.quote.as_deref(), Some("We define \"positive\" as {x | x > 0}."));
        assert_eq!(cells[0].raw.passage, Some(3));
    }

    #[test]
    fn an_earlier_object_without_cells_is_passed_over() {
        let reply = r#"Columns: {"c1": "dataset"}. Answer: {"cells":[{"column":"c1","answer":null,"quote":"","passage":null}]}"#;
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells[0].raw.answer, None);
        assert_eq!(cells[0].raw.passage, None);
    }

    #[test]
    fn cells_without_a_column_or_an_answer_key_are_dropped() {
        let reply = r#"{"cells":[{"answer":"x"},{"column":"c2","quote":"q"},{"column":3,"answer":true,"quote":"q"}]}"#;
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].column, "3");
        assert_eq!(cells[0].raw.answer, Some(Value::Bool(true)));
    }

    #[test]
    fn no_envelope_is_none() {
        for reply in ["", "I could not find it.", "{\"cells\": \"none\"}", "{\"cells\":[", "</think>"] {
            assert_eq!(parse_reply(reply), None, "{reply}");
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/shodh-rag/src/research/tables/parse.rs crates/shodh-rag/src/research/tables/mod.rs
git commit -m "Evidence tables: tolerant parse of model replies"
```

### Task 6: Budget, the 1/2-call split and the prompt

**Files:**
- Create: `crates/shodh-rag/src/research/tables/budget.rs`
- Create: `crates/shodh-rag/src/research/tables/prompt.rs`

**Interfaces:**
- Consumes:
  - `Passage` and `ColumnSpec` (Task 1);
  - `join_line_breaks` (Task 2);
  - `crate::embeddings::estimate_tokens`.
- Produces:
  - `PASSAGES_PER_COLUMN = 4`, `CALL_TOKEN_BUDGET = 6_000`, `MAX_CALLS_PER_PAPER = 2`.
  - `passage_tokens(&Passage) -> usize`.
  - `CallPlan { columns: Vec<usize>, passages: Vec<Passage>, truncated: Vec<usize> }`. Its indices point into the `ranked` input.
  - `plan_calls(first: Option<&Passage>, ranked: &[Vec<Passage>]) -> Vec<CallPlan>`.
  - `fill_prompt(passages: &[Passage], columns: &[(&str, &ColumnSpec)]) -> String`.
  - `reply_budget(columns: usize) -> usize`.

**Token counting.** Tokens are counted with `estimate_tokens` (3 characters per token, plus 2). The chunker's tokenizer lives with the embedder, and this layer does not hold it. The estimate is conservative, so a call carries slightly fewer passages than 6,000 real tokens would allow. It never carries more.

- [ ] **Step 1: Write `budget.rs`, with its tests.**

```rust
//! Which passages go into which call. The paper's first chunk (title and abstract) goes
//! first; then passages are added in rank order, round-robin across columns, until the
//! call holds about [`CALL_TOKEN_BUDGET`] tokens, a passage two columns share counted
//! once. When a column's top passages do not all fit, the columns are split into two
//! calls (the first half, the second half, in column order); a column whose passages
//! still do not all fit gets those that do and is listed as `truncated`.

use std::collections::{BTreeSet, HashSet};

use super::Passage;
use crate::embeddings::estimate_tokens;

/// Passages retrieved per column.
pub const PASSAGES_PER_COLUMN: usize = 4;
/// Passage tokens per call.
pub const CALL_TOKEN_BUDGET: usize = 6_000;
/// Most calls for one paper (a retry of an unparsable reply aside).
pub const MAX_CALLS_PER_PAPER: usize = 2;
/// The number and page label of a passage in the prompt.
const PASSAGE_OVERHEAD_TOKENS: usize = 8;

/// Tokens a passage costs in the prompt.
pub fn passage_tokens(passage: &Passage) -> usize {
    estimate_tokens(&passage.text) + PASSAGE_OVERHEAD_TOKENS
}

/// One model call: its columns and passages (indices into the `ranked` input).
#[derive(Debug, Clone, PartialEq)]
pub struct CallPlan {
    pub columns: Vec<usize>,
    pub passages: Vec<Passage>,
    pub truncated: Vec<usize>,
}

fn plan_one(first: Option<&Passage>, ranked: &[Vec<Passage>], columns: &[usize]) -> CallPlan {
    let mut passages = Vec::new();
    let mut seen = HashSet::new();
    let mut used = 0;
    let mut truncated = BTreeSet::new();
    if let Some(p) = first {
        seen.insert(p.chunk_id.clone());
        used += passage_tokens(p);
        passages.push(p.clone());
    }
    let depth = columns.iter().map(|&c| ranked[c].len()).max().unwrap_or(0);
    for rank in 0..depth {
        for &c in columns {
            let Some(p) = ranked[c].get(rank) else {
                continue;
            };
            if seen.contains(&p.chunk_id) {
                continue;
            }
            let cost = passage_tokens(p);
            if used + cost > CALL_TOKEN_BUDGET {
                truncated.insert(c);
                continue;
            }
            seen.insert(p.chunk_id.clone());
            used += cost;
            passages.push(p.clone());
        }
    }
    CallPlan {
        columns: columns.to_vec(),
        passages,
        truncated: truncated.into_iter().collect(),
    }
}

/// The calls for one paper: one when every column's passages fit, else two.
pub fn plan_calls(first: Option<&Passage>, ranked: &[Vec<Passage>]) -> Vec<CallPlan> {
    if ranked.is_empty() {
        return Vec::new();
    }
    let all: Vec<usize> = (0..ranked.len()).collect();
    let single = plan_one(first, ranked, &all);
    if single.truncated.is_empty() || ranked.len() < MAX_CALLS_PER_PAPER {
        return vec![single];
    }
    let mid = ranked.len().div_ceil(MAX_CALLS_PER_PAPER);
    vec![
        plan_one(first, ranked, &all[..mid]),
        plan_one(first, ranked, &all[mid..]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A passage costing exactly `tokens` (estimate: chars / 3 + 2, plus 8 overhead).
    fn passage(id: &str, tokens: usize) -> Passage {
        Passage {
            chunk_id: id.to_string(),
            text: "x".repeat((tokens - 10) * 3),
            page: None,
            bboxes: None,
        }
    }

    fn ranked(columns: usize, per_column: usize, tokens: usize) -> Vec<Vec<Passage>> {
        (0..columns)
            .map(|c| (0..per_column).map(|r| passage(&format!("{c}-{r}"), tokens)).collect())
            .collect()
    }

    #[test]
    fn the_test_passage_costs_what_it_says() {
        assert_eq!(passage_tokens(&passage("a", 1_000)), 1_000);
    }

    #[test]
    fn columns_that_fit_share_one_call_with_the_first_chunk_first() {
        let first = passage("first", 500);
        let plans = plan_calls(Some(&first), &ranked(2, 2, 1_000));
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].columns, vec![0, 1]);
        assert!(plans[0].truncated.is_empty());
        let ids: Vec<&str> = plans[0].passages.iter().map(|p| p.chunk_id.as_str()).collect();
        assert_eq!(ids, vec!["first", "0-0", "1-0", "0-1", "1-1"], "round-robin by rank");
    }

    #[test]
    fn a_shared_passage_is_sent_once() {
        let shared = passage("shared", 1_000);
        let lists = vec![vec![shared.clone(), passage("a", 1_000)], vec![shared, passage("b", 1_000)]];
        let plans = plan_calls(None, &lists);
        assert_eq!(plans[0].passages.len(), 3);
    }

    #[test]
    fn columns_that_do_not_fit_are_split_into_two_calls() {
        let plans = plan_calls(None, &ranked(4, 2, 1_000));
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].columns, vec![0, 1]);
        assert_eq!(plans[1].columns, vec![2, 3]);
        assert!(plans.iter().all(|p| p.truncated.is_empty() && p.passages.len() == 4));
    }

    #[test]
    fn beyond_two_calls_columns_get_what_fits_and_are_marked() {
        let plans = plan_calls(None, &ranked(2, 4, 2_000));
        assert_eq!(plans.len(), 2);
        for (i, plan) in plans.iter().enumerate() {
            assert_eq!(plan.truncated, vec![i]);
            assert_eq!(plan.passages.len(), 3);
        }
        let single = plan_calls(None, &ranked(1, 4, 2_000));
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].truncated, vec![0]);
        assert!(plan_calls(None, &[]).is_empty());
    }
}
```

- [ ] **Step 2: Write `prompt.rs`, with its tests.**

```rust
//! The fill prompt: the rules, the reply envelope, the numbered passages (with pages)
//! and the columns as a JSON list. Columns are tagged `c1`, `c2`, … in the prompt, not by
//! their ids, so the model has short, unambiguous tags to copy back. Passages are shown
//! with line-break hyphenation joined, so the words the model copies are the words the
//! quote check looks for.

use serde_json::{json, Value};

use super::normalize::join_line_breaks;
use super::{collapse, ColumnKind, ColumnSpec, Passage};

const RULES: &str = "You fill one row of an evidence table from passages of one research paper.\n\
Answer each column's question from the passages only.\n\
Rules:\n\
- \"quote\": the words that give the answer, copied exactly as written in one passage: word for word, at most 300 characters, no ellipsis, no changes.\n\
- \"passage\": the number of the passage the quote is from.\n\
- \"answer\": null when the passages do not answer the question. Never guess.\n\
- kind \"number\": the number exactly as printed in the quote, as a string, with its unit when the quote states one (\"0.912\", \"48 GB\"). Never compute, round or convert.\n\
- kind \"yes_no\": \"yes\" or \"no\".\n\
- kind \"choice\": exactly one of the column's options.\n\
- kind \"text\": a short answer of at most 500 characters.\n\
Reply with one JSON object and nothing else:\n\
{\"cells\":[{\"column\":\"<column id>\",\"answer\":<value or null>,\"quote\":\"<verbatim quote>\",\"passage\":<passage number>}]}\n";

/// Output tokens allowed for a reply covering `columns` columns.
pub fn reply_budget(columns: usize) -> usize {
    (200 + 160 * columns).min(2_000)
}

fn column_json(tag: &str, spec: &ColumnSpec) -> Value {
    let mut column = json!({
        "id": tag,
        "question": spec.question,
        "kind": spec.kind.as_str(),
    });
    if spec.kind == ColumnKind::Choice {
        column["options"] = json!(spec.options);
    }
    if !spec.instructions.is_empty() {
        column["instructions"] = json!(spec.instructions);
    }
    column
}

/// The prompt for one call. `columns` pairs each tag with its column.
pub fn fill_prompt(passages: &[Passage], columns: &[(&str, &ColumnSpec)]) -> String {
    let mut prompt = String::from(RULES);
    prompt.push_str("\nPassages:\n");
    for (i, passage) in passages.iter().enumerate() {
        let text = collapse(&join_line_breaks(&passage.text));
        match passage.page {
            Some(page) => prompt.push_str(&format!("[{}] (page {page}) {text}\n", i + 1)),
            None => prompt.push_str(&format!("[{}] {text}\n", i + 1)),
        }
    }
    let list: Vec<Value> = columns.iter().map(|(tag, spec)| column_json(tag, spec)).collect();
    prompt.push_str("\nColumns:\n");
    prompt.push_str(&Value::Array(list).to_string());
    prompt.push('\n');
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_numbers_passages_with_pages_and_lists_columns_as_json() {
        let passages = vec![
            Passage { chunk_id: "a".into(), text: "Dense Retrieval.\nAbstract: contras-\ntive.".into(), page: Some(1), bboxes: None },
            Passage { chunk_id: "b".into(), text: "No page here.".into(), page: None, bboxes: None },
        ];
        let choice = ColumnSpec {
            question: "Study kind?".into(),
            kind: ColumnKind::Choice,
            options: vec!["Benchmark".into(), "Survey".into()],
            instructions: "pick the main one".into(),
        };
        let text = ColumnSpec { question: "Dataset?".into(), kind: ColumnKind::Text, options: vec![], instructions: String::new() };
        let prompt = fill_prompt(&passages, &[("c1", &choice), ("c2", &text)]);
        assert!(prompt.contains("[1] (page 1) Dense Retrieval. Abstract: contrastive.\n"));
        assert!(prompt.contains("[2] No page here.\n"));
        // Key order inside a JSON object depends on serde_json's `preserve_order`
        // feature, so each field is checked on its own.
        for field in [r#""id":"c1""#, r#""kind":"choice""#, r#""question":"Study kind?""#, r#""instructions":"pick the main one""#] {
            assert!(prompt.contains(field), "{field}");
        }
        assert!(prompt.contains(r#""options":["Benchmark","Survey"]"#));
        assert_eq!(prompt.matches(r#""options""#).count(), 1, "a text column has no options");
        assert!(prompt.contains("{\"cells\":[{\"column\""));
        assert_eq!(reply_budget(1), 360);
        assert_eq!(reply_budget(20), 2_000);
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/shodh-rag/src/research/tables/budget.rs crates/shodh-rag/src/research/tables/prompt.rs
git commit -m "Evidence tables: passage budget, call split and fill prompt"
```

### Task 7: Passages: the PaperIndex trait, the RAGEngine implementation and Result passages

**Files:**
- Create: `crates/shodh-rag/src/research/tables/passages.rs`
- Modify: `crates/shodh-rag/src/rag_engine.rs`:
  - add `RAGEngine::first_chunk`, next to `document_metadata` (around line 2822);
  - add tests in its `#[cfg(test)] mod tests`, using `test_engine` (around line 3759).

**Interfaces:**
- Consumes:
  - `RAGEngine::search_comprehensive(query, k, Option<MetadataFilter>)`, where `MetadataFilter` derives `Default` and the `source_path` filter compares to the stored `source`;
  - `crate::research::path_key`;
  - `ResultRecord` and `ResultStatus` from `research::results`.
- Produces:
  - The trait:
    - `#[async_trait] trait PaperIndex: Send + Sync`
    - `async fn search_paper(&self, source: &str, query: &str, k: usize) -> Result<Vec<Passage>, String>`
    - `async fn first_passage(&self, source: &str) -> Result<Option<Passage>, String>`
  - `impl PaperIndex for tokio::sync::RwLock<RAGEngine>`.
  - `passage_from(chunk_id, text, &HashMap<String, String>) -> Passage`.
  - `result_passages(&[ResultRecord]) -> Vec<Passage>`.
  - `RAGEngine::first_chunk(&self, source: &str) -> anyhow::Result<Option<SearchHit>>`.

**Two traps:**
- **Path form.** The stored `source` is `normalize_source_path`, which `path_key` gives. Pass a raw path to the filter and you get zero passages, with no error.
- **The first chunk.** `list_chunks(pred, 1)` returns some chunk, not chunk 0. The predicate must say `chunk_index = 0`.

- [ ] **Step 1: Add `first_chunk` to `RAGEngine`** (after `document_metadata`):

```rust
    /// The first chunk of `source` (a paper's title and abstract), or `None` when it has
    /// no chunks. `source` is normalised as the index stores it.
    pub async fn first_chunk(&self, source: &str) -> Result<Option<crate::storage::SearchHit>> {
        let source = normalize_source_path(Path::new(source));
        let predicate = format!("{} AND chunk_index = 0", source_predicate(&source));
        Ok(self.store.list_chunks(Some(&predicate), 1).await?.into_iter().next())
    }
```

- [ ] **Step 2: Write `passages.rs`.**

```rust
//! Where a paper's passages come from: the hybrid search restricted to the paper (its
//! stored `source`), the paper's first chunk, and for `number` columns the accepted
//! Result statements already read from its tables, whose cell boxes become the highlight.

use std::collections::HashMap;

use serde_json::json;

use super::Passage;
use crate::rag_engine::RAGEngine;
use crate::research::path_key;
use crate::research::results::{ResultRecord, ResultStatus};
use crate::types::MetadataFilter;

/// Search over one paper's chunks. Implemented by the RAG engine; tests use doubles.
#[async_trait::async_trait]
pub trait PaperIndex: Send + Sync {
    /// The `k` passages of `source` that best match `query`, best first.
    async fn search_paper(&self, source: &str, query: &str, k: usize) -> Result<Vec<Passage>, String>;
    /// The first chunk of `source`, or `None` when the paper has no chunks.
    async fn first_passage(&self, source: &str) -> Result<Option<Passage>, String>;
}

/// A passage from a chunk's id, text and stored metadata (`page`, `bboxes`).
pub fn passage_from(chunk_id: String, text: String, metadata: &HashMap<String, String>) -> Passage {
    Passage {
        chunk_id,
        text,
        page: metadata.get("page").and_then(|p| p.trim().parse().ok()),
        bboxes: metadata.get("bboxes").filter(|b| !b.trim().is_empty()).cloned(),
    }
}

/// Chunk ids in one spelling (search results carry parsed UUIDs, listed chunks the
/// stored string), so the first chunk and a search hit of it are one passage.
fn chunk_id(id: &str) -> String {
    uuid::Uuid::parse_str(id).map(|u| u.to_string()).unwrap_or_else(|_| id.to_string())
}

#[async_trait::async_trait]
impl PaperIndex for tokio::sync::RwLock<RAGEngine> {
    async fn search_paper(&self, source: &str, query: &str, k: usize) -> Result<Vec<Passage>, String> {
        let filter = MetadataFilter {
            source_path: Some(path_key(source)),
            ..Default::default()
        };
        let engine = self.read().await;
        let hits = engine
            .search_comprehensive(query, k, Some(filter))
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(hits
            .into_iter()
            .map(|hit| passage_from(chunk_id(&hit.id.to_string()), hit.snippet, &hit.metadata))
            .collect())
    }

    async fn first_passage(&self, source: &str) -> Result<Option<Passage>, String> {
        let engine = self.read().await;
        let hit = engine
            .first_chunk(&path_key(source))
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(hit.map(|hit| {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&hit.metadata_json).unwrap_or_default();
            passage_from(chunk_id(&hit.id), hit.text, &metadata)
        }))
    }
}

/// The accepted Results of a paper as passages: one line per Result
/// ("BM25 on MS MARCO: Recall@10 = 0.355 (k1 = 0.9). Table: …"), its page and its cell box.
pub fn result_passages(records: &[ResultRecord]) -> Vec<Passage> {
    records
        .iter()
        .filter(|r| r.status == ResultStatus::Accepted)
        .map(|r| {
            let mut text = format!("{} on {}: {} = {}", r.method, r.dataset, r.metric, r.value_text);
            if let Some(setting) = r.setting.as_deref().filter(|s| !s.trim().is_empty()) {
                text.push_str(&format!(" ({setting})"));
            }
            if let Some(caption) = r.table_caption.as_deref().filter(|c| !c.trim().is_empty()) {
                text.push_str(&format!(". Table: {caption}"));
            }
            Passage {
                chunk_id: format!("result:{}", r.id),
                text,
                page: Some(r.page),
                bboxes: r.region.map(|g| {
                    json!([{ "page": g.page, "x0": g.x0, "y0": g.y0, "x1": g.x1, "y1": g.y1 }]).to_string()
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use shodh_ontology::ExtractorKind;

    use super::*;
    use crate::research::results::Region;
    use crate::statements::Scope;

    fn record(id: &str, status: ResultStatus) -> ResultRecord {
        ResultRecord {
            id: id.to_string(),
            method: "BM25".to_string(),
            dataset: "MS MARCO".to_string(),
            metric: "Recall@10".to_string(),
            method_id: "method:bm25".to_string(),
            dataset_id: "dataset:ms-marco".to_string(),
            metric_id: "metric:recall-10".to_string(),
            value: "0.355".to_string(),
            value_text: "0.355".to_string(),
            unit: None,
            spread: None,
            setting: Some("k1 = 0.9".to_string()),
            file_path: "/p/a.pdf".to_string(),
            file_name: "a.pdf".to_string(),
            page: 7,
            region: Some(Region { page: 7, x0: 10.0, y0: 20.0, x1: 30.0, y1: 40.0 }),
            table_caption: Some("Table 2: Retrieval results.".to_string()),
            extractor: ExtractorKind::User,
            confidence: 1.0,
            status,
            scope: Scope::Global,
            extracted_at: Utc::now(),
        }
    }

    #[test]
    fn accepted_results_become_passages_with_their_cell_box() {
        let passages = result_passages(&[record("r1", ResultStatus::Accepted), record("r2", ResultStatus::Review)]);
        assert_eq!(passages.len(), 1);
        let p = &passages[0];
        assert_eq!(p.chunk_id, "result:r1");
        assert_eq!(p.text, "BM25 on MS MARCO: Recall@10 = 0.355 (k1 = 0.9). Table: Table 2: Retrieval results.");
        assert_eq!(p.page, Some(7));
        let boxes: serde_json::Value = serde_json::from_str(p.bboxes.as_deref().unwrap()).unwrap();
        assert_eq!(boxes[0]["page"], 7);
        assert_eq!(boxes[0]["x1"], 30.0);
    }

    #[test]
    fn chunk_metadata_gives_the_page_and_boxes() {
        let mut metadata = HashMap::new();
        metadata.insert("page".to_string(), "4".to_string());
        metadata.insert("bboxes".to_string(), r#"[{"page":4,"x0":1.0,"y0":2.0,"x1":3.0,"y1":4.0}]"#.to_string());
        let p = passage_from("id".into(), "text".into(), &metadata);
        assert_eq!(p.page, Some(4));
        assert!(p.bboxes.is_some());
        let bare = passage_from("id".into(), "text".into(), &HashMap::new());
        assert_eq!((bare.page, bare.bboxes), (None, None));
        let id = uuid::Uuid::new_v4();
        assert_eq!(chunk_id(&id.to_string().to_uppercase()), id.to_string());
    }
}
```

- [ ] **Step 3: Engine tests,** in `rag_engine.rs` `mod tests`. They use `test_engine`, which runs with the word embedder and no model download.

```rust
    /// A table fill searches one paper, by any spelling of its path, and starts from
    /// its first chunk.
    #[tokio::test]
    async fn a_paper_index_searches_only_that_paper_and_finds_its_first_chunk() {
        use crate::research::tables::passages::PaperIndex;

        let dir = tempfile::tempdir().unwrap();
        let mut engine = test_engine(dir.path()).await;
        let a = dir.path().join("alpha.md");
        let b = dir.path().join("beta.md");
        let mut alpha = String::from("# Alpha retrieval\n\nAlpha abstract: dense retrieval with a dataset of papers.\n");
        for i in 0..40 {
            alpha.push_str(&format!("\n## Section {i}\n\nAlpha section {i} reports the dataset size and the method.\n"));
        }
        std::fs::write(&a, alpha).unwrap();
        std::fs::write(&b, "# Beta\n\nBeta also reports a dataset and a method.\n").unwrap();
        engine.add_document_from_file(&a, HashMap::new()).await.unwrap();
        engine.add_document_from_file(&b, HashMap::new()).await.unwrap();

        // Several chunks, so the test proves chunk 0 is chosen, not merely the only one.
        let chunks = engine
            .store
            .list_chunks(Some(&source_predicate(&normalize_source_path(&a))), 1_000)
            .await
            .unwrap();
        assert!(chunks.len() > 1, "{} chunks", chunks.len());
        let first = engine.first_chunk(&a.to_string_lossy()).await.unwrap().unwrap();
        assert_eq!(first.chunk_index, 0);
        assert!(first.text.contains("Alpha"), "{}", first.text);

        let index = tokio::sync::RwLock::new(engine);
        let passages = index
            .search_paper(&a.to_string_lossy(), "dataset method", 4)
            .await
            .unwrap();
        assert!(!passages.is_empty());
        assert!(passages.iter().all(|p| p.text.contains("Alpha") || p.text.contains("alpha")), "{passages:?}");
        let opened = index.first_passage(&a.to_string_lossy()).await.unwrap().unwrap();
        assert!(opened.text.contains("Alpha"));
        let missing = dir.path().join("never-indexed.md");
        assert!(index.first_passage(&missing.to_string_lossy()).await.unwrap().is_none());
        assert!(index.search_paper(&missing.to_string_lossy(), "dataset", 4).await.unwrap().is_empty());
    }
```

No existing `rag_engine.rs` test runs `search_comprehensive` on `test_engine`, so this test is the first to do it. If the word embedder cannot drive the full pipeline, fix the test setup: attach what `search_comprehensive` requires. Never skip the search assertions.

The 40 sections are units of the structure chunker (see `short_and_whitespace_free_files_are_indexed_with_their_text`), so the file is several chunks. If CI shows a single chunk, lengthen each section's text; never drop the `chunks.len() > 1` assertion.

- [ ] **Step 4: Commit**

```bash
git add crates/shodh-rag/src/research/tables/passages.rs crates/shodh-rag/src/rag_engine.rs
git commit -m "Evidence tables: per-paper passages from the index and from Results"
```

### Task 8: Filling one paper, tested on recorded replies

**Files:**
- Create: `crates/shodh-rag/src/research/tables/fill.rs`
- Create, in `crates/shodh-rag/src/research/tables/fixtures/`:
  - `reply_correct.txt`
  - `reply_flawed.txt`
  - `reply_unparsable.txt`

**Interfaces:**
- Consumes:
  - from Tasks 1–7: `TableStore::{store_fill, cached}`, `CacheEntry`, `check`, `ground`, `RawAnswer`, `parse_reply`, `plan_calls`, `PASSAGES_PER_COLUMN`, `fill_prompt`, `reply_budget`, `PaperIndex`;
  - `TextModel` (`research/results/mod.rs`);
  - `ScorerSet`;
  - `crate::research::blocking`.
- Produces:
  - error codes: `PAPER_UNREADABLE`, `PAPER_NOT_INDEXED`, `SEARCH_FAILED`, `MODEL_FAILED`, `UNPARSABLE_RESPONSE`, `MISSING_IN_REPLY`, `PASSAGES_TRUNCATED`;
  - `paper_hash(path: &str) -> ResearchResult<String>`;
  - `needs_fill(cell: Option<&Cell>, column: &Column, paper_hash: &str) -> bool`;
  - `PaperRequest { row: TableRow, columns: Vec<Column>, cells: Vec<Cell>, result_passages: Vec<Passage> }`;
  - `PaperFill { cells: Vec<Cell>, calls: usize, cache_hits: usize, model_error: Option<String> }`. `calls` counts the retry. `model_error` carries the provider's message, for PR 3's quota classification.
  - `Filler { store: Arc<TableStore>, index: Arc<dyn PaperIndex>, model: Arc<dyn TextModel>, scorers: ScorerSet }`, with `async fn fill_paper(&self, request: &PaperRequest) -> ResearchResult<PaperFill>`.

**`paper_hash` departs from the spec.** The spec calls it "the library's content hash of the file", but the library has no content hash: citation scans use size and mtime. So `paper_hash` is the SHA-256 of the file's bytes, computed when the paper is filled. A 20 MB PDF hashes in tens of milliseconds, which is negligible next to a model call.

**Cells written for the cases outside the main flow:**
- **`passages_truncated`:** set only on a `not_found` cell of a truncated column. That is the case where the missing passages matter: the answer may have been in the passages left out. A `verified` cell carries no error.
- **Cache hits:** validated again against that column's passages. The local retrieval is re-run, and the first chunk plus the column's ranked passages form the pool. The cached `passage` number is ignored, because numbering differs between calls.
- **A model error (any `Err` from `complete`):** the columns of that call become `failed`, with `model_failed: <detail>`. Any later call for the paper is not made, and its columns fail the same way. `PaperFill.model_error` is set.

- [ ] **Step 1: Write the fixtures.** They are written in the style of free-model replies:
  - a reasoning block;
  - a fence;
  - prose;
  - extra keys.

  Before PR 5's eval, PR 3 replaces them with replies captured from a real free model (see Open questions).

`fixtures/reply_correct.txt`:

````text
<think>
The dataset is LAION-400M (passage 1). Recall@10 is 0.912, passage 2 {as printed}. The code is available (passage 3). It is a benchmark.
</think>
```json
{"cells":[
 {"column":"c1","answer":"LAION-400M","quote":"We pre-train a bi-encoder on LAION-400M","passage":1},
 {"column":"c2","answer":"0.912","quote":"Recall@10 of 0.912 on MS MARCO","passage":2},
 {"column":"c3","answer":"yes","quote":"Our code is available at the project page","passage":3},
 {"column":"c4","answer":"benchmark","quote":"The study is a benchmark evaluation","passage":3,"confidence":0.9}
]}
```
````

`fixtures/reply_flawed.txt`:

```text
Here is the row:
{"cells":[
 {"column":"c1","answer":"LAION","quote":"They trained on the LAION dataset","passage":1},
 {"column":"c2","answer":"0.92","quote":"Recall@10 of 0.912 on MS MARCO","passage":2},
 {"column":"c3","answer":null,"quote":"","passage":null},
 {"column":"c4","answer":"Case study","quote":"The study is a benchmark evaluation","passage":3}
]}
Let me know if you need anything else.
```

`fixtures/reply_unparsable.txt`:

```text
I looked at the passages. c1 = LAION-400M, c2 = 0.912, c3 = yes, c4 = Benchmark.
```

- [ ] **Step 2: Write `fill.rs`, with its tests.**

```rust
//! Filling one paper's cells: the cells whose column needs it (never filled, failed,
//! queued, stale by column revision or paper hash; never edited) are answered from the
//! cache when the column, paper and model are unchanged, else by at most two model calls
//! over passages retrieved locally from the paper. Every answer goes through
//! [`super::validate`]; every stored cell says which passage, page and boxes back it.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};

use super::budget::{plan_calls, PASSAGES_PER_COLUMN};
use super::parse::parse_reply;
use super::passages::PaperIndex;
use super::prompt::{fill_prompt, reply_budget};
use super::store::{CacheEntry, TableStore};
use super::validate::{check, ground, Checked, RawAnswer};
use super::{collapse, Cell, CellStatus, Column, ColumnKind, ColumnSpec, Passage, TableRow};
use crate::harness::grounding::verify::ScorerSet;
use crate::research::results::TextModel;
use crate::research::{blocking, ResearchError, ResearchResult};

pub const PAPER_UNREADABLE: &str = "paper_unreadable";
pub const PAPER_NOT_INDEXED: &str = "paper_not_indexed";
pub const SEARCH_FAILED: &str = "search_failed";
pub const MODEL_FAILED: &str = "model_failed";
pub const UNPARSABLE_RESPONSE: &str = "unparsable_response";
pub const MISSING_IN_REPLY: &str = "missing_in_reply";
pub const PASSAGES_TRUNCATED: &str = "passages_truncated";

/// Longest provider or I/O detail kept in a cell's error.
const MAX_DETAIL_CHARS: usize = 300;

fn with_detail(code: &str, detail: &str) -> String {
    let detail: String = collapse(detail).chars().take(MAX_DETAIL_CHARS).collect();
    format!("{code}: {detail}")
}

fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// SHA-256 (hex) of the file's bytes: a changed PDF refills its row.
pub fn paper_hash(path: &str) -> ResearchResult<String> {
    let unreadable = |e: std::io::Error| ResearchError::Pdf(format!("{path} could not be read: {e}"));
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(unreadable)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Whether a fill should (re)fill this cell.
pub fn needs_fill(cell: Option<&Cell>, column: &Column, paper_hash: &str) -> bool {
    let Some(cell) = cell else {
        return true;
    };
    match cell.status {
        CellStatus::Edited => false,
        CellStatus::Empty | CellStatus::Queued | CellStatus::Failed => true,
        CellStatus::Verified | CellStatus::Unverified | CellStatus::NotFound => {
            cell.column_revision != Some(column.revision) || cell.paper_hash.as_deref() != Some(paper_hash)
        }
    }
}

/// One paper to fill: its row, the table's columns, the row's current cells and, for
/// `number` columns, its accepted Results as passages.
#[derive(Debug, Clone)]
pub struct PaperRequest {
    pub row: TableRow,
    pub columns: Vec<Column>,
    pub cells: Vec<Cell>,
    pub result_passages: Vec<Passage>,
}

/// What a fill did: the cells it stored, the model calls it made (a retry included),
/// the columns answered from the cache, and the model's error when a call failed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaperFill {
    pub cells: Vec<Cell>,
    pub calls: usize,
    pub cache_hits: usize,
    pub model_error: Option<String>,
}

/// Fills papers with one model, index and set of local scorers.
#[derive(Clone)]
pub struct Filler {
    pub store: Arc<TableStore>,
    pub index: Arc<dyn PaperIndex>,
    pub model: Arc<dyn TextModel>,
    pub scorers: ScorerSet,
}

/// The first chunk then the column's passages, a chunk once.
fn pool_of(first: Option<&Passage>, ranked: &[Passage]) -> Vec<Passage> {
    let mut pool: Vec<Passage> = Vec::with_capacity(ranked.len() + 1);
    for p in first.into_iter().chain(ranked.iter()) {
        if !pool.iter().any(|q| q.chunk_id == p.chunk_id) {
            pool.push(p.clone());
        }
    }
    pool
}

impl Filler {
    pub async fn fill_paper(&self, request: &PaperRequest) -> ResearchResult<PaperFill> {
        let row = &request.row;
        let path = row.document_id.clone();
        let model_id = self.model.model_id();
        let existing: HashMap<&str, &Cell> = request
            .cells
            .iter()
            .filter(|c| c.row_id == row.id)
            .map(|c| (c.column_id.as_str(), c))
            .collect();
        let mut fill = PaperFill::default();

        let hash = {
            let p = path.clone();
            blocking(move || paper_hash(&p)).await
        };
        let hash = match hash {
            Ok(hash) => hash,
            Err(e) => {
                let columns: Vec<&Column> = request
                    .columns
                    .iter()
                    .filter(|c| existing.get(c.id.as_str()).is_none_or(|cell| cell.status != CellStatus::Edited))
                    .collect();
                let error = with_detail(PAPER_UNREADABLE, &e.to_string());
                self.fail(row, &columns, None, &model_id, &error, &mut fill).await?;
                return Ok(fill);
            }
        };
        let columns: Vec<&Column> = request
            .columns
            .iter()
            .filter(|c| needs_fill(existing.get(c.id.as_str()).copied(), c, &hash))
            .collect();
        if columns.is_empty() {
            return Ok(fill);
        }

        // 1. Local retrieval: no model call.
        let first = match self.index.first_passage(&path).await {
            Ok(first) => first,
            Err(e) => {
                self.fail(row, &columns, Some(&hash), &model_id, &with_detail(SEARCH_FAILED, &e), &mut fill).await?;
                return Ok(fill);
            }
        };
        let mut ranked: Vec<Vec<Passage>> = Vec::with_capacity(columns.len());
        for column in &columns {
            let query = collapse(&format!("{} {}", column.spec.question, column.spec.instructions));
            let mut passages = match self.index.search_paper(&path, &query, PASSAGES_PER_COLUMN).await {
                Ok(passages) => passages,
                Err(e) => {
                    self.fail(row, &columns, Some(&hash), &model_id, &with_detail(SEARCH_FAILED, &e), &mut fill).await?;
                    return Ok(fill);
                }
            };
            passages.truncate(PASSAGES_PER_COLUMN);
            if column.spec.kind == ColumnKind::Number {
                passages.extend(request.result_passages.iter().cloned());
            }
            ranked.push(passages);
        }
        if first.is_none() && ranked.iter().all(Vec::is_empty) {
            self.fail(row, &columns, Some(&hash), &model_id, PAPER_NOT_INDEXED, &mut fill).await?;
            return Ok(fill);
        }

        // 2. The cache: an unchanged column of an unchanged paper costs no call.
        let mut misses: Vec<usize> = Vec::new();
        for (i, column) in columns.iter().enumerate() {
            let store = self.store.clone();
            let (h, k, m) = (hash.clone(), column.key.clone(), model_id.clone());
            let cached = blocking(move || store.cached(&h, &k, &m)).await?;
            match cached.and_then(|json| serde_json::from_str::<RawAnswer>(&json).ok()) {
                Some(raw) => {
                    let pool = pool_of(first.as_ref(), &ranked[i]);
                    let raw = RawAnswer { passage: None, ..raw };
                    let checked = check(&column.spec, &raw, &pool);
                    fill.cache_hits += 1;
                    self.finish(row, column, checked, &pool, &model_id, &hash, None, &mut fill).await?;
                }
                None => misses.push(i),
            }
        }

        // 3. At most two calls over the missed columns.
        let miss_ranked: Vec<Vec<Passage>> = misses.iter().map(|&i| ranked[i].clone()).collect();
        for plan in plan_calls(first.as_ref(), &miss_ranked) {
            let call_columns: Vec<&Column> = plan.columns.iter().map(|&m| columns[misses[m]]).collect();
            if let Some(error) = fill.model_error.clone() {
                self.fail(row, &call_columns, Some(&hash), &model_id, &with_detail(MODEL_FAILED, &error), &mut fill).await?;
                continue;
            }
            let tags: Vec<String> = (1..=call_columns.len()).map(|n| format!("c{n}")).collect();
            let specs: Vec<(&str, &ColumnSpec)> = tags
                .iter()
                .map(String::as_str)
                .zip(call_columns.iter().map(|c| &c.spec))
                .collect();
            let prompt = fill_prompt(&plan.passages, &specs);
            let mut parsed = None;
            // One retry, only for a reply with no envelope at all.
            for _ in 0..2 {
                fill.calls += 1;
                match self.model.complete(&prompt, reply_budget(call_columns.len())).await {
                    Ok(reply) => {
                        if let Some(cells) = parse_reply(&reply) {
                            parsed = Some(cells);
                            break;
                        }
                    }
                    Err(e) => {
                        fill.model_error = Some(e);
                        break;
                    }
                }
            }
            let Some(parsed) = parsed else {
                let error = match fill.model_error.as_deref() {
                    Some(e) => with_detail(MODEL_FAILED, e),
                    None => UNPARSABLE_RESPONSE.to_string(),
                };
                self.fail(row, &call_columns, Some(&hash), &model_id, &error, &mut fill).await?;
                continue;
            };
            let mut by_tag: HashMap<String, RawAnswer> = HashMap::new();
            for cell in parsed {
                by_tag.entry(cell.column).or_insert(cell.raw);
            }
            for (n, column) in call_columns.iter().enumerate() {
                let Some(raw) = by_tag.remove(&tags[n]) else {
                    self.fail(row, &[*column], Some(&hash), &model_id, MISSING_IN_REPLY, &mut fill).await?;
                    continue;
                };
                let mut checked = check(&column.spec, &raw, &plan.passages);
                if checked.status == CellStatus::NotFound && plan.truncated.contains(&plan.columns[n]) {
                    checked.error = Some(PASSAGES_TRUNCATED.to_string());
                }
                let cache = serde_json::to_string(&raw).ok().map(|response_json| CacheEntry {
                    paper_hash: hash.clone(),
                    column_key: column.key.clone(),
                    model: model_id.clone(),
                    response_json,
                });
                self.finish(row, column, checked, &plan.passages, &model_id, &hash, cache, &mut fill).await?;
            }
        }
        Ok(fill)
    }

    /// Runs the grounding check (a local model, on a blocking thread) and stores the cell.
    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        row: &TableRow,
        column: &Column,
        checked: Checked,
        passages: &[Passage],
        model: &str,
        hash: &str,
        cache: Option<CacheEntry>,
        fill: &mut PaperFill,
    ) -> ResearchResult<()> {
        let question = column.spec.question.clone();
        let scorers = self.scorers.clone();
        let checked = tokio::task::spawn_blocking(move || ground(checked, &question, &scorers))
            .await
            .map_err(|e| ResearchError::Task(e.to_string()))?;
        let matched = checked.passage.and_then(|i| passages.get(i));
        let cell = Cell {
            table_id: row.table_id.clone(),
            row_id: row.id.clone(),
            column_id: column.id.clone(),
            status: checked.status,
            value: checked.value,
            quote: checked.quote,
            chunk_id: matched.map(|p| p.chunk_id.clone()),
            page: matched.and_then(|p| p.page),
            bboxes: matched.and_then(|p| p.bboxes.clone()),
            model: Some(model.to_string()),
            column_revision: Some(column.revision),
            paper_hash: Some(hash.to_string()),
            filled_at: Some(now_text()),
            edited_at: None,
            error: checked.error,
        };
        self.store_cell(cell, cache, fill).await
    }

    /// Stores `failed` cells for `columns` with `error`.
    async fn fail(
        &self,
        row: &TableRow,
        columns: &[&Column],
        hash: Option<&str>,
        model: &str,
        error: &str,
        fill: &mut PaperFill,
    ) -> ResearchResult<()> {
        for column in columns {
            let cell = Cell {
                table_id: row.table_id.clone(),
                row_id: row.id.clone(),
                column_id: column.id.clone(),
                status: CellStatus::Failed,
                value: None,
                quote: None,
                chunk_id: None,
                page: None,
                bboxes: None,
                model: Some(model.to_string()),
                column_revision: Some(column.revision),
                paper_hash: hash.map(str::to_string),
                filled_at: Some(now_text()),
                edited_at: None,
                error: Some(error.to_string()),
            };
            self.store_cell(cell, None, fill).await?;
        }
        Ok(())
    }

    async fn store_cell(&self, cell: Cell, cache: Option<CacheEntry>, fill: &mut PaperFill) -> ResearchResult<()> {
        let store = self.store.clone();
        let stored = cell.clone();
        let written = blocking(move || store.store_fill(&stored, cache.as_ref())).await?;
        if written {
            fill.cells.push(cell);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::harness::grounding::verify::EntailmentScorer;
    use crate::research::path_key;
    use crate::reranking::Entailment;
    use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

    const CORRECT: &str = include_str!("fixtures/reply_correct.txt");
    const FLAWED: &str = include_str!("fixtures/reply_flawed.txt");
    const UNPARSABLE: &str = include_str!("fixtures/reply_unparsable.txt");

    fn passage(id: &str, page: u32, text: &str) -> Passage {
        Passage {
            chunk_id: id.to_string(),
            text: text.to_string(),
            page: Some(page),
            bboxes: Some(format!(r#"[{{"page":{page},"x0":1.0,"y0":2.0,"x1":3.0,"y1":4.0}}]"#)),
        }
    }

    /// The same passages for every query; records the sources it was asked about.
    struct Index {
        first: Option<Passage>,
        passages: Vec<Passage>,
        sources: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl PaperIndex for Index {
        async fn search_paper(&self, source: &str, _query: &str, k: usize) -> Result<Vec<Passage>, String> {
            self.sources.lock().unwrap_or_else(|e| e.into_inner()).push(source.to_string());
            Ok(self.passages.iter().take(k).cloned().collect())
        }
        async fn first_passage(&self, _source: &str) -> Result<Option<Passage>, String> {
            Ok(self.first.clone())
        }
    }

    fn paper_index() -> Index {
        Index {
            first: Some(passage(
                "p0",
                1,
                "Dense Retrieval at Scale. Abstract: We pre-train a bi-encoder on LAION-400M with a contrastive loss.",
            )),
            passages: vec![
                passage("p1", 4, "Table 2 reports Recall@10 of 0.912 on MS MARCO with 48 GB of GPU memory."),
                passage("p2", 6, "Our code is available at the project page. The study is a benchmark evaluation."),
            ],
            sources: Mutex::new(Vec::new()),
        }
    }

    /// Replays recorded replies in order and keeps the prompts it was sent.
    struct Replies {
        replies: Mutex<VecDeque<Result<String, String>>>,
        prompts: Mutex<Vec<String>>,
    }

    impl Replies {
        fn new(replies: Vec<Result<&str, &str>>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().map(|r| r.map(str::to_string).map_err(str::to_string)).collect()),
                prompts: Mutex::new(Vec::new()),
            }
        }
        fn prompts(&self) -> Vec<String> {
            self.prompts.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    #[async_trait::async_trait]
    impl TextModel for Replies {
        fn model_id(&self) -> String {
            "recorded-model".to_string()
        }
        async fn complete(&self, prompt: &str, _max_tokens: usize) -> Result<String, String> {
            self.prompts.lock().unwrap_or_else(|e| e.into_inner()).push(prompt.to_string());
            self.replies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .unwrap_or_else(|| Err("no recorded reply left".to_string()))
        }
    }

    struct Recorded(Entailment);

    impl EntailmentScorer for Recorded {
        fn entail(&self, _hypothesis: &str, premises: &[String]) -> Result<Vec<Entailment>, String> {
            Ok(premises.iter().map(|_| self.0).collect())
        }
    }

    struct Setup {
        _dir: tempfile::TempDir,
        paper: PathBuf,
        store: Arc<TableStore>,
        table: String,
        columns: Vec<Column>,
        row: TableRow,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("shodh.db");
        let workspace = WorkspaceStore::open(&db, None)
            .unwrap()
            .create(&NewWorkspace { name: "Survey".to_string(), ..Default::default() }, WorkspaceAuthor::User)
            .unwrap()
            .id;
        let store = Arc::new(TableStore::open(&db, None).unwrap());
        let table = store.create_table(&workspace, "Retrieval").unwrap().id;
        let specs = [
            ("Which pre-training dataset is used?", ColumnKind::Text, vec![]),
            ("What Recall@10 is reported?", ColumnKind::Number, vec![]),
            ("Is the code released?", ColumnKind::YesNo, vec![]),
            ("What kind of study is it?", ColumnKind::Choice, vec!["Benchmark".to_string(), "User study".to_string(), "Survey".to_string()]),
        ];
        let columns = specs
            .into_iter()
            .map(|(question, kind, options)| {
                store
                    .add_column(&table, &ColumnSpec { question: question.to_string(), kind, options, instructions: String::new() })
                    .unwrap()
            })
            .collect();
        let paper = dir.path().join("paper.pdf");
        std::fs::write(&paper, b"%PDF-1.4 dense retrieval at scale, version 1").unwrap();
        let row = store.add_rows(&table, &[paper.to_string_lossy().into_owned()]).unwrap().remove(0);
        Setup { _dir: dir, paper, store, table, columns, row }
    }

    fn filler(s: &Setup, index: Index, model: Arc<Replies>, scorers: ScorerSet) -> Filler {
        Filler { store: s.store.clone(), index: Arc::new(index), model, scorers }
    }

    fn request(s: &Setup) -> PaperRequest {
        PaperRequest {
            row: s.row.clone(),
            columns: s.store.columns(&s.table).unwrap(),
            cells: s.store.cells(&s.table).unwrap(),
            result_passages: Vec::new(),
        }
    }

    fn status_of(s: &Setup, column: usize) -> (CellStatus, Option<String>) {
        let cell = s.store.cell(&s.row.id, &s.columns[column].id).unwrap().unwrap();
        (cell.status, cell.error)
    }

    #[tokio::test]
    async fn a_correct_reply_verifies_every_cell_at_its_passage_in_one_call() {
        let s = setup();
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let index = paper_index();
        let f = filler(&s, index, model.clone(), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 1);
        assert_eq!(fill.cells.len(), 4);
        assert!(fill.cells.iter().all(|c| c.status == CellStatus::Verified), "{:?}", fill.cells);
        let recall = s.store.cell(&s.row.id, &s.columns[1].id).unwrap().unwrap();
        assert_eq!(recall.chunk_id.as_deref(), Some("p1"));
        assert_eq!(recall.page, Some(4));
        assert!(recall.bboxes.is_some());
        assert_eq!(recall.paper_hash.as_deref(), Some(paper_hash(&s.paper.to_string_lossy()).unwrap().as_str()));
        let prompt = &model.prompts()[0];
        assert!(prompt.contains("[1] (page 1) Dense Retrieval at Scale."));
        assert!(prompt.contains("[2] (page 4) Table 2 reports"));
        assert!(prompt.contains(r#""id":"c4""#));
        for column in &s.columns {
            assert!(s.store.cached(recall.paper_hash.as_deref().unwrap(), &column.key, "recorded-model").unwrap().is_some());
        }
    }

    #[tokio::test]
    async fn paraphrases_wrong_numbers_outside_choices_and_nulls_are_not_verified() {
        let s = setup();
        let f = filler(&s, paper_index(), Arc::new(Replies::new(vec![Ok(FLAWED)])), ScorerSet::default());
        f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(status_of(&s, 0), (CellStatus::Unverified, Some("quote_not_in_passages".into())));
        assert_eq!(status_of(&s, 1), (CellStatus::Unverified, Some("number_not_in_quote".into())));
        assert_eq!(status_of(&s, 2), (CellStatus::NotFound, None));
        assert_eq!(status_of(&s, 3), (CellStatus::Unverified, Some("not_an_option".into())));
    }

    #[tokio::test]
    async fn an_unparsable_reply_is_retried_once_and_then_fails_its_cells() {
        let s = setup();
        let f = filler(&s, paper_index(), Arc::new(Replies::new(vec![Ok(UNPARSABLE), Ok(UNPARSABLE)])), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 2);
        for column in 0..4 {
            assert_eq!(status_of(&s, column), (CellStatus::Failed, Some(UNPARSABLE_RESPONSE.into())));
        }
        let s = setup();
        let f = filler(&s, paper_index(), Arc::new(Replies::new(vec![Ok(UNPARSABLE), Ok(CORRECT)])), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 2, "the retry counts");
        assert!(fill.cells.iter().all(|c| c.status == CellStatus::Verified));
    }

    #[tokio::test]
    async fn an_unchanged_paper_and_column_cost_no_call_and_a_change_costs_only_itself() {
        let s = setup();
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        f.fill_paper(&request(&s)).await.unwrap();

        // Filled cells are not refilled.
        let again = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!((again.calls, again.cells.len()), (0, 0));

        // Cleared cells come back from the cache, checked again, with no call.
        for column in &s.columns {
            s.store.clear_cell(&s.row.id, &column.id).unwrap();
        }
        let cached = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!((cached.calls, cached.cache_hits), (0, 4));
        assert!(cached.cells.iter().all(|c| c.status == CellStatus::Verified));

        // An edited column misses the cache alone: one call, asking only that column.
        let edited = s.store.update_column(
            &s.columns[0].id,
            &ColumnSpec { question: "Which dataset is largest?".into(), kind: ColumnKind::Text, options: vec![], instructions: String::new() },
        ).unwrap();
        let one = r#"{"cells":[{"column":"c1","answer":"LAION-400M","quote":"LAION-400M","passage":1}]}"#;
        let model = Arc::new(Replies::new(vec![Ok(one)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        let refill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!((refill.calls, refill.cells.len()), (1, 1));
        assert_eq!(refill.cells[0].column_id, edited.id);
        assert!(model.prompts()[0].contains("Which dataset is largest?"));
        assert!(!model.prompts()[0].contains("Is the code released?"));

        // A changed file refills its row.
        std::fs::write(&s.paper, b"%PDF-1.4 dense retrieval at scale, version 2").unwrap();
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        let changed = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(changed.calls, 1);
        assert_eq!(changed.cells.len(), 4);
    }

    #[tokio::test]
    async fn an_unindexed_paper_costs_no_call_and_fails_its_cells() {
        let s = setup();
        let index = Index { first: None, passages: Vec::new(), sources: Mutex::new(Vec::new()) };
        let model = Arc::new(Replies::new(vec![]));
        let f = filler(&s, index, model.clone(), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 0);
        assert!(model.prompts().is_empty());
        for column in 0..4 {
            assert_eq!(status_of(&s, column), (CellStatus::Failed, Some(PAPER_NOT_INDEXED.into())));
        }
    }

    #[tokio::test]
    async fn a_column_missing_from_the_reply_fails_and_a_repeat_is_ignored() {
        let s = setup();
        let reply = r#"{"cells":[
            {"column":"c1","answer":"LAION-400M","quote":"We pre-train a bi-encoder on LAION-400M","passage":1},
            {"column":"c1","answer":"ImageNet","quote":"ImageNet","passage":1}]}"#;
        let f = filler(&s, paper_index(), Arc::new(Replies::new(vec![Ok(reply)])), ScorerSet::default());
        f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(status_of(&s, 0), (CellStatus::Verified, None));
        for column in 1..4 {
            assert_eq!(status_of(&s, column), (CellStatus::Failed, Some(MISSING_IN_REPLY.into())));
        }
    }

    #[tokio::test]
    async fn a_provider_error_fails_the_cells_and_is_reported() {
        let s = setup();
        let f = filler(&s, paper_index(), Arc::new(Replies::new(vec![Err("HTTP 429 Too Many Requests: daily limit")])), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 1);
        assert_eq!(fill.model_error.as_deref(), Some("HTTP 429 Too Many Requests: daily limit"));
        let (status, error) = status_of(&s, 0);
        assert_eq!(status, CellStatus::Failed);
        assert_eq!(error.as_deref(), Some("model_failed: HTTP 429 Too Many Requests: daily limit"));
    }

    #[tokio::test]
    async fn edited_cells_are_never_asked_about() {
        let s = setup();
        s.store.edit_cell(&s.row.id, &s.columns[2].id, "no").unwrap();
        let model = Arc::new(Replies::new(vec![Ok(r#"{"cells":[]}"#)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        f.fill_paper(&request(&s)).await.unwrap();
        assert!(!model.prompts()[0].contains("Is the code released?"));
        assert_eq!(status_of(&s, 2).0, CellStatus::Edited);
    }

    #[tokio::test]
    async fn a_contradicting_quote_downgrades_verified_cells() {
        let s = setup();
        let scorers = ScorerSet {
            relevance: None,
            entailment: Some(Arc::new(Recorded(Entailment { contradiction: 0.9, entailment: 0.05, neutral: 0.05 }))),
        };
        let f = filler(&s, paper_index(), Arc::new(Replies::new(vec![Ok(CORRECT)])), scorers);
        f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(status_of(&s, 2), (CellStatus::Unverified, Some("quote_does_not_support".into())));
    }

    #[tokio::test]
    async fn number_columns_see_the_papers_results_and_the_search_gets_the_row_path() {
        let s = setup();
        let index = Arc::new(paper_index());
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = Filler {
            store: s.store.clone(),
            index: index.clone() as Arc<dyn PaperIndex>,
            model: model.clone(),
            scorers: ScorerSet::default(),
        };
        let mut req = request(&s);
        req.result_passages = vec![passage("result:r1", 7, "BM25 on MS MARCO: Recall@10 = 0.355")];
        f.fill_paper(&req).await.unwrap();
        assert!(model.prompts()[0].contains("(page 7) BM25 on MS MARCO: Recall@10 = 0.355"));
        assert_eq!(s.row.document_id, path_key(&s.paper.to_string_lossy()));
        let sources = index.sources.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(sources.len(), 4, "one search per column");
        assert!(sources.iter().all(|source| source == &s.row.document_id));
    }

    #[test]
    fn staleness_follows_revision_and_hash_and_never_touches_edits() {
        let s = setup();
        let column = &s.columns[0];
        let cell = |status, revision, hash: &str| Cell {
            table_id: s.table.clone(),
            row_id: s.row.id.clone(),
            column_id: column.id.clone(),
            status,
            value: None,
            quote: None,
            chunk_id: None,
            page: None,
            bboxes: None,
            model: None,
            column_revision: Some(revision),
            paper_hash: Some(hash.to_string()),
            filled_at: None,
            edited_at: None,
            error: None,
        };
        assert!(needs_fill(None, column, "h"));
        assert!(!needs_fill(Some(&cell(CellStatus::Verified, 1, "h")), column, "h"));
        assert!(needs_fill(Some(&cell(CellStatus::Verified, 1, "other")), column, "h"));
        assert!(needs_fill(Some(&cell(CellStatus::NotFound, 0, "h")), column, "h"));
        assert!(needs_fill(Some(&cell(CellStatus::Failed, 1, "h")), column, "h"));
        assert!(!needs_fill(Some(&cell(CellStatus::Edited, 0, "other")), column, "h"));
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/shodh-rag/src/research/tables/fill.rs crates/shodh-rag/src/research/tables/fixtures
git commit -m "Evidence tables: fill one paper from retrieval, budget, call, parse, validation and cache"
```

### PR 2 end: push, open the PR and get CI green

- [ ] Run `git push -u origin feat/evidence-table-fill`.
- [ ] Run `gh pr create --title "Evidence tables: fill pipeline for one paper"`.
- [ ] Watch `gh pr checks <n> --watch` until it is green. Fix root causes only.
- [ ] **PR 2 is independently mergeable.** It is library code with tests, and nothing calls it yet.
- [ ] **Quality gate (before PR 4 and PR 5 are planned in detail):**
  - PR 2 has no command that can run a real model, and the user does not run cargo. The first real run therefore comes with PR 3's `table_fill_paper` command.
  - Fill 3 of the user's papers × 5 columns with a free model: about 3–6 calls.
  - Record four counts in the PR 4 plan: verified-and-correct, verified-but-wrong, unverified and not-found.
  - If verified-but-wrong is above 1 of 15, or unverified above 5 of 15, fix the prompt or normalisation before the UI is built.

---

# PR 3 — Job queue, daily cap, quota pause, resume and notifications (outline)

Branch: `feat/evidence-table-jobs`. This PR is planned in detail after the PR 2 quality gate. The outline below fixes:
- the files;
- the interfaces;
- the tests that must exist.

### Task 9: Shared per-day call counter (migration 15)
- **Files:**
  - `crates/shodh-rag/src/audit/store.rs`: migration 15, `model_calls(day TEXT PRIMARY KEY, calls INTEGER NOT NULL CHECK (calls >= 0))`. `day` is the local date as `YYYY-MM-DD`.
  - `crates/shodh-rag/src/research/tables/calls.rs`.
- **Interfaces:**
  - `CallCounter::open(path, key)`
  - `record(day: &str, n: usize) -> ResearchResult<usize>`, which returns the day's total
  - `today(day: &str) -> ResearchResult<usize>`
- **Tests:**
  - counts add up per day;
  - a new day starts at 0;
  - two connections see the same count.
- **Decide in the detail plan:**
  - whether chat and agent calls also call `record`;
  - otherwise the dialog wording becomes "Tables have made 12 calls today".

### Task 10: The job runner
- **Files:** `crates/shodh-rag/src/research/tables/jobs.rs`.
- **Interfaces:**
  - `JobState { Queued, Running, Paused, Done, Cancelled, Failed }`, stored in `table_fill_job.state`.
  - `estimate(store, table_id, cache) -> FillEstimate { papers, columns, cells, calls, cached }`:
    - one call per paper with at least one miss;
    - two calls when its columns' passages cannot fit (the estimate runs `plan_calls` on retrieval, which is local).
  - `JobRunner { filler: Filler, store, counter, cap: usize, cancel: Arc<AtomicBool> }`
    - `start(table_id) -> job_id`;
    - `run(job_id) -> ResearchResult<JobOutcome>`. It sets cells to `queued`, fills rows in `position` order, and adds each `PaperFill.calls` to `done_calls` and to the counter.
  - **Before each paper:**
    - stop with `Paused` when `today + 2 > cap`;
    - set `next_attempt_at` to the next local midnight.
  - **Quota:**
    - `harness::provider_error::classify(model_error, None, now_ms)` is `RateLimited` or `QuotaExhausted` → `Paused`;
    - `next_attempt_at` = `limit_reset_at_ms`, or the next midnight;
    - the paper's `failed` cells are retried on resume.
  - **Cancel:** checked between calls. Finished cells stay. Queued cells go back to `empty`.
  - **Resume:** `resume_due(now) -> Vec<job_id>` returns `Queued`/`Running` jobs, and `Paused` jobs whose `next_attempt_at <= now`.
- **Tests** (all with `Replies`/`Index` doubles like Task 8):
  - `a_restarted_job_resumes_where_it_stopped_and_never_refills_done_cells`;
  - `a_quota_error_pauses_until_the_reset_and_resumes_after_it`;
  - `cancel_stops_after_the_current_call_and_keeps_finished_cells`;
  - `the_daily_cap_is_never_exceeded`;
  - `estimate_counts_only_cache_misses`.

### Task 11: App state, commands, notifications and the library-removal hook
- **Files:**
  - `app/src-tauri/src/table_commands.rs` (new);
  - `app/src-tauri/src/lib.rs` (manage the state and register the commands);
  - `app/src-tauri/src/notifications.rs` (`table_outcome`);
  - `app/src-tauri/src/file_watcher.rs` or its command caller (hook);
  - the settings store (`tables.daily_call_cap`, default 30).
- **State:** `TableState` holds:
  - `Arc<TableStore>`;
  - the runner, built lazily with `crate::memory_learn::configured_model(&dir, &llm)`, or the table's model when one is set;
  - `Arc<RwLock<RAGEngine>>` as the `PaperIndex`;
  - the app's `ScorerSet`;
  - `ResultService::list` for `result_passages`.
- **Commands:**
  - tables: `tables_list`, `table_create`, `table_rename`, `table_delete`, `table_get` (table, columns, rows with paper titles, cells);
  - columns: `column_add`, `column_update`, `column_delete`, `column_move`;
  - rows: `rows_add`, `row_remove`;
  - cells: `cell_edit`, `cell_clear`, `cell_refill`, which widens to the top 8 passages (spec §10);
  - fill: `fill_estimate`, `fill_start`, `fill_cancel`, `fill_status`;
  - one debugging command for the quality gate: `table_fill_paper(table_id, row_id)`.
- **Events and resume:**
  - The commands emit `table_changed` and `table_fill_progress`.
  - On launch, `resume_due` jobs restart.
- **Notifications:** `notifications::announce(&app, table_outcome(...))` on finish, and on "needs you" (paused for quota or cap).
- **Library-removal hook:** before `remove_source`, collect the source's indexed documents (`RAGEngine::document_sources()` where `space_id == source_id`; confirm this field's meaning in the detail plan). After a successful removal, call `TableStore::forget_documents`.
- **Fixtures:** replace Task 8's fixtures with replies captured from one free model through `table_fill_paper`, at 3 calls. Commit them byte for byte as the model returned them.
- **Tests** (Rust, `shodh-rag-app --lib`):
  - command argument validation;
  - `table_outcome` wording;
  - the hook removes rows only for the removed source's files.

**PR 3 end:** push, open the PR, get CI green. Then the quality gate runs on the user's papers via `table_fill_paper`.

# PR 4 — Tables tab, grid, cell panel, column and budget dialogs (outline)

Branch: `feat/evidence-table-ui`. The files are under `app/src/features/tables/`, and the tests under `app/tests/` (run with `node --test`).

- **`types.ts`:** mirrors the camelCase Rust types. `CellValue` is a union discriminated by `kind`, which matters because `strict: false`. `CellStatus` is a string union.
- **`api.ts`:** typed `invoke` wrappers for the PR 3 commands.
- **`model.ts`** (pure, tested):
  - `sortRows(rows, cells, column, dir)`: numbers sort numerically and ignore units; empty cells go last.
  - `filterRows(rows, cells, filters)`: by `yes_no`/`choice` values and by status.
  - `cellMark(cell)`: one of `'unverified' | 'not_found' | 'edited' | 'failed' | null`.
  - `isStale(cell, column)`.
  - `reasonText(code)`: mirrors `validate::reason`, plus the fill codes.
  - `nextCell(pos, key, dims)`: arrow-key movement.
  - `PRESET_COLUMNS`: Title, Year, Method, Dataset, Main result, Limitations.
- **Components:**
  - `TablesTab.tsx`: the list and "New table" with presets.
  - `TableGrid.tsx`: a real `<table>` with `<th scope>`, roving tabindex, arrow keys, and stale cells dimmed.
  - `CellPanel.tsx`:
    - the value, and the quote with its matched words marked;
    - the page, and **Open in PDF**, which uses the existing viewer's open-at-highlight path with `bboxes`;
    - the model and date, and the reason;
    - Edit, Refill and Clear.
  - `ColumnDialog.tsx`: question, kind, options (2–12), instructions.
  - `FillDialog.tsx`: "Filling N papers × M columns needs about K model calls. Shodh has made T calls today."
- **Workspace wiring:** in `app/src/features/workspaces/model.ts`, add `'tables'` to `WORKSPACE_TABS` and `WORKSPACE_TAB_LABELS`. `WorkspacePage.tsx` renders `TablesTab`.
- **Tests:**
  - `app/tests/tablesModel.test.ts`: sorting with units and empties, filters, marks, staleness, arrow movement at the edges.
  - `app/tests/tablesFormat.test.ts`: the reason wording, and "Not found in the passages searched".

**PR 4 end:** CI green, with the TypeScript check and `npm test`.

# PR 5 — Export and the accuracy eval (outline)

Branch: `feat/evidence-table-export-eval`.

- **Export, in `crates/shodh-rag/src/research/tables/export.rs`:**
  - `to_csv(view, with_quotes)` follows RFC 4180 quoting. Any cell that starts with `=`, `+`, `-`, `@`, a tab or a CR is prefixed with `'`, which guards against spreadsheet formula injection from paper text. It adds the optional `<column> quote` and `<column> page`.
  - `to_markdown(view)` writes a table, with footnotes holding the quotes and pages. It escapes `|`.
  - The commands are `table_export_csv` and `table_export_md`, through the existing save dialog.
  - **Tests:** quoting, the injection guard, footnote numbering, and empty or not-found cells written as "Not found in the passages searched".
- **Eval set:**
  - Place it at `crates/shodh-eval/data/evidence_table/`: 10 open-access papers × 5 columns (text, text, number, yes_no, choice), with values and quotes checked by hand (`gold.json`).
  - The committed `shodh-eval` corpus is synthetic, so the 10 papers must be chosen and their licences checked (see Open questions).
- **Runner:**
  - It is the `shodh-eval` subcommand `evidence-table --model <id> --max-calls <n>`.
  - It appends one JSON line per paper to `results/evidence_table_<model>.jsonl`, skips papers already present, and stops at `--max-calls`. It is resumable across days on the free quota.
  - It reports four rates:
    - verified-and-correct;
    - verified-but-wrong;
    - unverified;
    - not found when an answer exists.

    It also reports the `quote_does_not_support` hit rate, which calibrates `CONTRADICTION_CUTOFF`.
- **README:** a section with the numbers, the model name and the date.

---

## Risks

1. **Uncalibrated contradiction cutoff (0.5).** It sits on top of an NLI hypothesis form (`question: value`) that no test has calibrated.
   - Too low, and correct cells are downgraded.
   - Too high, and it never fires.

   PR 5 measures it. Until then it errs towards keeping cells `verified` (it needs contradiction ≥ 0.5 *and* above entailment).
2. **Free-model quotes with an ellipsis or small rewording** are `unverified` by design. If the PR 2 gate shows this dominates, the fix is in the prompt or in a documented ellipsis rule. The match must never be loosened.
3. **The token estimate (3 characters per token) overcounts English text.** Calls carry fewer passages than 6,000 real tokens would allow, which raises `not_found`. PR 3 can pass the embedder's tokenizer instead, if the gate shows misses.
4. **Neighbour-expanded search snippets.** Window chunks are joined with their neighbours, so a quote can match text from the neighbour while the cell takes the centre chunk's page and boxes. The highlight can then be one chunk off. Structure chunks (PDFs) are not expanded.
5. **`paper_hash` reads the whole file on every fill of the paper.** It is negligible for PDFs. Re-check if tables of hundreds of papers appear.

## Open questions

1. **Recorded fixtures.** The spec asks for real responses captured once. PR 2 ships hand-written replies in free-model style, because nothing can call a model before PR 3. Capturing the real ones in PR 3 costs about 3 calls.
2. **The eval corpus.** The spec says "10 open-access papers from the existing retrieval-eval corpus", but the committed `shodh-eval` corpus is synthetic. Someone has to choose the papers, check their licences and write the gold set by hand.
3. **The per-day counter's scope.** Does it count chat calls too (the spec's "Shodh has made 12 calls today")? That means hooking the chat path, or the dialog says "Tables have made…".
4. **Where the eval runs.** The user runs no local cargo. It runs either from a scheduled CI workflow with a free-key secret, with its state kept between runs, or on the user's machine.

5. **Spec deviations, each with its reason:**
   - **`paper_hash`** is the SHA-256 of the file's bytes. The library has no content hash.
   - **Removing a paper from the library** deletes its rows, not only its cells.
   - **Editing or deleting a column** purges the cache entries only its cells used. §4 says the old entries "stay unused", but then deleting a table could never find them, and quotes would outlive their cells.
   - **`column_key`** is stored on `table_column`, so the SQL purges can match cache entries.

## Self-review

**Spec coverage**

| Spec section | Where it is covered |
|---|---|
| §3 | storage (Tasks 1 and 4), commands (Task 11), UI (PR 4) |
| §4 | Tasks 1 and 4, including forget/delete and the workspace hook |
| §5.1–5.6 | Tasks 5–8 |
| §5, Results as passages | Task 7 and Task 8's last test |
| §6 | PR 3 |
| §7 | PR 4 |
| §7, export | PR 5 |
| §9, Rust unit tests | Tasks 2–8 |
| §9, recorded responses | Task 8 |
| §9, frontend tests | PR 4 |
| §9, eval | PR 5 |
| §8 (v2) | No schema change needed: `yes_no`/`choice` columns and statuses are already typed and filterable |

**Placeholder scan**
- No TBD or TODO.
- The PR 3–5 outlines are intentionally without code, as the brief requires.

**Type consistency**
- Names are checked across tasks:
  - `Column.key`, `Column.revision`, `Cell.column_revision`;
  - `Passage`, `RawAnswer`, `Checked`;
  - `CacheEntry`, `PaperRequest`, `PaperFill`, `Filler`.
- Indices:
  - `RawAnswer.passage` is 1-based.
  - `Checked.passage` is 0-based.
  - `CallPlan.columns` and `CallPlan.truncated` index the `ranked` input.

**Review Focus**
- All five items have tests in Tasks 2, 4, 5 and 8.
