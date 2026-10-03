# M1: Eval Harness + Current-Pipeline Baseline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `shodh-eval`, a CLI and library that measures retrieval and answer quality of shodh on public datasets and on private local folders. Use it to record the baseline of **today's** pipeline before any of it is replaced.

**Architecture:** A new workspace crate, `crates/shodh-eval`, depends on `shodh-rag` and drives its real code paths:
- **Ingestion:** `indexing::index_folder`.
- **Retrieval:** `RAGEngine::search`.
- **Answers:** `chat::engine::ChatEngine::process_message`.

It works as follows:
- **Datasets** are YAML or JSON files of `EvalCase`s.
- **Corpora** are directories of files. Two generators produce public ones:
  - a converter for the CUAD contract dataset, which is downloaded and checksum-pinned;
  - a deterministic generator for synthetic invoice PDFs.
- **Scoring** is done by pure functions.
- **Reports** are JSON. A baseline stores the mean and standard deviation over repeated runs. The CI gate fails when a metric falls more than 3σ below the baseline.

**Tech Stack:**
- Rust 2021
- `clap` 4
- `tokio`
- `serde_yaml_ng`
- `rust_decimal`
- `lopdf` 0.32 (already in the dependency tree)
- `rand` 0.8.5 + `rand_chacha` 0.3.1 (exact pins for determinism)
- `zip` 2
- `md-5`, `sha2`
- `reqwest` 0.11 (matches `shodh-rag`)
- GitHub Actions (windows-latest)

**Spec:** `docs/superpowers/specs/2026-10-02-folder-sources-grounded-answers-design.md`. This plan implements milestone **M1** (§11) and the evaluation part of §9.

## Global Constraints

**Prerequisites**
- **CI must execute tests.** Sub-project 0's CI fix (the CRT linker mismatch, `--no-run`, `continue-on-error`) must be merged before Task 10's CI job can gate anything. Tasks 1–9 can be implemented and reviewed before that.

**Repository rules (CLAUDE.md)**
- **No local builds.** Executors do not run `cargo build`, `cargo test` or `cargo run` locally unless the user explicitly authorizes it for this plan. Verification commands in each task are run by the user, or by pushing the branch and reading CI. Record the actual output in the task log; never assume a pass.
- **PR workflow:** branch `feat/m1-eval-harness` → commits → push → PR → CI green → merge. No direct pushes to `master`.
- **Commit messages:** no `Co-Authored-By` and no "Generated with Claude Code" lines.
- **Quality bar:** production-grade code only. No TODOs, placeholders, mocks or stubs in `src/`.
- **Test doubles:** the scripted LLM HTTP fixture lives only under `crates/shodh-eval/tests/support/` (spec §9, approved).
- **Gates:** never lower thresholds, weaken assertions, skip tests, or widen tolerances to pass CI. The baseline gate is mean − 3σ measured. Latency is reported, never gated on hosted runners.

**Code rules**
- `crates/shodh-eval/src/**` denies `clippy::unwrap_used` and `clippy::expect_used` outside tests.

**Pinned external artifacts**

| Artifact | Source | Checksum |
|---|---|---|
| CUAD | `https://zenodo.org/records/4595826/files/CUAD_v1.zip`, 105,883,672 bytes | md5 `c38f490a984420b8a62600db401fafd5`; licence CC BY 4.0, attribution required |
| E5 model | `https://huggingface.co/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/onnx/model_O4.onnx` | sha256 `f60256a833caee5c75a3903e589116752ee016ca7bc16f9b96e4db09984c5703` |
| E5 tokenizer | `…/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/onnx/tokenizer.json` | sha256 `62c24cdc13d4c9952d63718d6c9fa4c287974249e16b7ade6d5a85e7bbb75626` |
| Reranker model | `https://huggingface.co/cross-encoder/ms-marco-MiniLM-L-6-v2/resolve/233902d25c440f23af6f7d6e94d2946bac0bee0a/onnx/model_O4.onnx` | sha256 `b232c2eeedd97a593edc177e3ce4cbd1d6c8f6d8f61a5c201cd0cdeb8134da18` |
| Reranker tokenizer | `…/cross-encoder/ms-marco-MiniLM-L-6-v2/resolve/233902d25c440f23af6f7d6e94d2946bac0bee0a/tokenizer.json` | sha256 `d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66` |

The model SHA-256 values are of the file contents, measured by streaming each pinned URL (they match Hugging Face's LFS object ids). Earlier drafts listed Xet storage ids, which are not content hashes and fail verification. The app pins the same files in `crates/shodh-rag/src/embeddings/model_store.rs`.

**Model directory layout** (read by `shodh-rag`):
- `<models>/multilingual-e5-base/{model_O4.onnx,tokenizer.json}`
- `<models>/ms-marco-MiniLM-L6-v2/{model_O4.onnx,tokenizer.json}`

**Document keys**
- A document key is the corpus-relative path with forward slashes, lowercased on every platform. `shodh-rag` lowercases stored sources on Windows; lowercasing everywhere keeps keys identical across OSes.

## Review Focus

1. **Windows paths.** Sources come back from the engine lowercased with `/` separators, possibly with an 8.3 short temp path such as `C:\Users\VARUNS~1\…`. Expected doc keys must still match, and any retrieved source that cannot be mapped must be counted and reported, never silently dropped. Tested in Task 1 and Task 6.
2. **Different CUAD zip layout.** If the zip layout differs from what the converter assumes, the converter must fail with counts. It must never emit an empty or tiny dataset that silently produces a "baseline". Tested in Task 5.
3. **Indian and international money formats** in answers: `₹1,23,456.78`, `Rs. 1,23,456.78`, `INR 123456.78`, `$1,234.50`. These must parse to the same decimal as the expected value. Tested in Task 2.
4. **Multiple chunks per document.** Many chunks from one document in the top results must count once, at its first rank. Otherwise recall@k is inflated and MRR is wrong. Tested in Task 6.
5. **Baseline comparison edge cases:**
   - zero variance plus a tiny drop must fail;
   - an improvement must pass;
   - a metric missing from the current run must fail;
   - a dataset hash different from the baseline's must fail.

   Tested in Task 3.

---

## File Structure

```
Cargo.toml                                   (modify: add workspace member)
.github/workflows/ci.yml                     (modify: eval-retrieval job)
.github/workflows/eval-baseline.yml          (create: manual baseline recording)
eval/README.md                               (create: how to run, metrics, attribution)
eval/baselines/.gitkeep                      (create; baselines committed in Task 10)
crates/shodh-eval/
  Cargo.toml
  src/lib.rs           module list + lint policy
  src/main.rs          CLI wiring only
  src/dataset.rs       EvalCase / Dataset / NumberLiteral, load/save/validate/hash
  src/corpus.rs        document-key normalisation, corpus file listing
  src/scoring.rs       answer scoring (pure functions)
  src/report.rs        RunReport, latency summary, Baseline, compare gate
  src/fetch.rs         checksum-verified downloads (CUAD zip, models)
  src/invoices.rs      deterministic invoice corpus + dataset generator
  src/cuad.rs          CUAD zip → corpus + dataset converter
  src/retrieval.rs     ingest via index_folder + retrieval evaluation
  src/answers.rs       headless ChatEngine answer evaluation
  tests/support/mod.rs scripted OpenAI-compatible HTTP fixture (tests only)
  tests/retrieval_e2e.rs
  tests/answers_e2e.rs
  tests/cli.rs
```

---

### Task 1: Crate scaffold, dataset model, document keys

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/shodh-eval/Cargo.toml`, `crates/shodh-eval/src/lib.rs`, `crates/shodh-eval/src/dataset.rs`, `crates/shodh-eval/src/corpus.rs`

**Interfaces:**
- Produces:
  - `dataset::{QuestionType, NumberLiteral, EvalCase, Dataset, DatasetError}`
  - `Dataset::load(&Path) -> Result<Dataset, DatasetError>`
  - `Dataset::save(&self, &Path) -> Result<(), DatasetError>`
  - `Dataset::validate(&self) -> Result<(), DatasetError>`
  - `Dataset::content_hash(&self) -> String` (sha256 hex)
  - `EvalCase::expected_decimal(&self) -> Result<Option<Decimal>, String>`
  - `corpus::{doc_key_from_relative(&str) -> String, doc_key_from_source(&Path, &str) -> Option<String>, list_corpus_files(&Path, &[&str]) -> std::io::Result<Vec<String>>}`

- [ ] **Step 1: Add the workspace member and crate manifest**

`Cargo.toml` (root):
```toml
[workspace]
resolver = "2"
members = [
    "crates/shodh-rag",
    "crates/shodh-eval",
    "app/src-tauri",
]
```

`crates/shodh-eval/Cargo.toml`:
```toml
[package]
name = "shodh-eval"
version = "0.1.0"
edition = "2021"
publish = false
description = "Evaluation harness for shodh-rag: retrieval and answer quality baselines"

[[bin]]
name = "shodh-eval"
path = "src/main.rs"

[dependencies]
shodh-rag = { path = "../shodh-rag" }
anyhow = "1"
thiserror = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml_ng = "0.10"
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
chrono = { version = "0.4", features = ["serde"] }
rust_decimal = "1.43"
rand = "=0.8.5"
rand_chacha = "=0.3.1"
lopdf = "0.32"
zip = "2"
md-5 = "0.10"
sha2 = "0.10"
hex = "0.4"
reqwest = { version = "0.11", features = ["stream"] }
futures-util = "0.3"
tempfile = "3"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
walkdir = "2"
regex = "1"

[dev-dependencies]
assert_cmd = "2"
```

`crates/shodh-eval/src/lib.rs`:
```rust
//! Evaluation harness for shodh-rag.
//!
//! Measures retrieval and answer quality of the real shodh pipeline on public
//! datasets (CUAD contracts, synthetic invoices) and on private local folders.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod answers;
pub mod corpus;
pub mod cuad;
pub mod dataset;
pub mod fetch;
pub mod invoices;
pub mod report;
pub mod retrieval;
pub mod scoring;
```

Until their tasks land, create each of `answers.rs`, `cuad.rs`, `fetch.rs`, `invoices.rs`, `report.rs`, `retrieval.rs`, `scoring.rs` containing only a `//!` module doc line describing its purpose, taken from the File Structure above. Each later task replaces its file completely. The doc-line files are not stubs: they contain no fake behaviour and are fully replaced within this PR.

- [ ] **Step 2: Write failing tests for the dataset model and document keys**

Append to `crates/shodh-eval/src/dataset.rs`, below the implementation written in Step 4:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    const YAML: &str = r#"
id: private-contracts
description: team contracts folder
cases:
  - id: q1
    question: What is the notice period in the Acme MSA?
    type: lookup
    expected_answer: ["thirty (30) days", "30 days"]
    expected_docs: ["Legal\\Acme_MSA.pdf"]
    expected_pages: [4]
  - id: q2
    question: Total invoiced by Acme in Q3 2026?
    type: aggregate
    expected_number: 123456.78
    expected_docs: ["invoices/a.pdf", "invoices/b.pdf"]
  - id: q3
    question: What is our policy on space travel?
    type: none
"#;

    fn write_tmp(contents: &str, name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn loads_yaml_and_normalises_doc_keys() {
        let (_d, p) = write_tmp(YAML, "set.yaml");
        let ds = Dataset::load(&p).unwrap();
        assert_eq!(ds.cases.len(), 3);
        assert_eq!(ds.cases[0].kind, QuestionType::Lookup);
        assert_eq!(ds.cases[0].expected_docs, vec!["legal/acme_msa.pdf".to_string()]);
        assert_eq!(
            ds.cases[1].expected_decimal().unwrap(),
            Some(Decimal::from_str("123456.78").unwrap())
        );
    }

    #[test]
    fn float_number_literal_keeps_two_decimals() {
        let lit = NumberLiteral::Float(1234.5);
        assert_eq!(lit.to_decimal().unwrap(), Decimal::from_str("1234.5").unwrap());
    }

    #[test]
    fn rejects_duplicate_ids_and_missing_expectations() {
        let bad = r#"
id: bad
description: x
cases:
  - { id: a, question: q, type: aggregate }
  - { id: a, question: q2, type: list }
  - { id: c, question: q3, type: none, expected_docs: ["x.pdf"] }
"#;
        let (_d, p) = write_tmp(bad, "bad.yaml");
        match Dataset::load(&p) {
            Err(DatasetError::Invalid { problems, .. }) => {
                assert!(problems.iter().any(|m| m.contains("duplicate case id 'a'")));
                assert!(problems.iter().any(|m| m.contains("aggregate case 'a' needs expected_number")));
                assert!(problems.iter().any(|m| m.contains("list case 'a' needs expected_answer")));
                assert!(problems.iter().any(|m| m.contains("none case 'c' must not list expected_docs")));
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_dataset() {
        let (_d, p) = write_tmp("id: e\ndescription: x\ncases: []\n", "e.yaml");
        assert!(matches!(Dataset::load(&p), Err(DatasetError::Invalid { .. })));
    }

    #[test]
    fn save_then_load_round_trips_and_hash_is_stable() {
        let (_d, p) = write_tmp(YAML, "set.yaml");
        let ds = Dataset::load(&p).unwrap();
        let out = p.with_file_name("out.yaml");
        ds.save(&out).unwrap();
        let again = Dataset::load(&out).unwrap();
        assert_eq!(ds, again);
        assert_eq!(ds.content_hash(), again.content_hash());
        assert_eq!(ds.content_hash().len(), 64);
    }
}
```

Append to `crates/shodh-eval/src/corpus.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn relative_keys_are_lowercase_forward_slash() {
        assert_eq!(doc_key_from_relative(".\\Legal\\Acme MSA.PDF"), "legal/acme msa.pdf");
        assert_eq!(doc_key_from_relative("/a/B.txt"), "a/b.txt");
    }

    #[test]
    fn source_maps_back_to_key_with_short_and_mixed_case_roots() {
        let root = Path::new("C:\\Users\\VARUNS~1\\AppData\\Local\\Temp\\corpus");
        let src = "c:/users/varuns~1/appdata/local/temp/corpus/invoices/inv-0001.pdf";
        assert_eq!(doc_key_from_source(root, src).as_deref(), Some("invoices/inv-0001.pdf"));
    }

    #[test]
    fn source_outside_root_is_unmapped() {
        let root = Path::new("/data/corpus");
        assert_eq!(doc_key_from_source(root, "/data/other/x.pdf"), None);
        assert_eq!(doc_key_from_source(root, "/data/corpus-2/x.pdf"), None);
    }

    #[test]
    fn lists_supported_files_as_keys_sorted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Sub")).unwrap();
        std::fs::write(dir.path().join("Sub").join("B.PDF"), b"x").unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("skip.exe"), b"x").unwrap();
        let files = list_corpus_files(dir.path(), &["pdf", "txt"]).unwrap();
        assert_eq!(files, vec!["a.txt".to_string(), "sub/b.pdf".to_string()]);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p shodh-eval --lib dataset:: corpus::`
Expected: compile errors (`Dataset`, `doc_key_from_relative` not found).

- [ ] **Step 4: Implement `dataset.rs` and `corpus.rs`**

`crates/shodh-eval/src/dataset.rs` (above the tests module):
```rust
//! Evaluation dataset model: questions with expected answers, numbers and documents.

use std::collections::BTreeSet;
use std::path::Path;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::corpus::doc_key_from_relative;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    /// One fact; any of `expected_answer` (or `expected_number`) is correct.
    Lookup,
    /// Several items; all of `expected_answer` must appear.
    List,
    /// A computed number (sum, count); `expected_number` must appear.
    Aggregate,
    /// Comparison across documents; scored like `Lookup`.
    Compare,
    /// Unanswerable from the corpus; the answer must say so.
    None,
}

/// A number as written in a dataset file: quoted text, integer, or float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberLiteral {
    Text(String),
    Integer(i64),
    Float(f64),
}

impl NumberLiteral {
    pub fn to_decimal(&self) -> Result<Decimal, String> {
        match self {
            Self::Text(s) => Decimal::from_str(s.trim())
                .map_err(|e| format!("'{s}' is not a decimal number: {e}")),
            Self::Integer(i) => Ok(Decimal::from(*i)),
            Self::Float(f) => Decimal::from_str(&f.to_string())
                .map_err(|e| format!("{f} is not representable as a decimal: {e}")),
        }
    }
}

impl From<Decimal> for NumberLiteral {
    fn from(d: Decimal) -> Self {
        Self::Text(d.normalize().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalCase {
    pub id: String,
    pub question: String,
    #[serde(rename = "type")]
    pub kind: QuestionType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_answer: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_number: Option<NumberLiteral>,
    /// Corpus-relative paths; normalised to document keys on load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_docs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_pages: Vec<u32>,
}

impl EvalCase {
    pub fn expected_decimal(&self) -> Result<Option<Decimal>, String> {
        self.expected_number.as_ref().map(NumberLiteral::to_decimal).transpose()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dataset {
    pub id: String,
    pub description: String,
    pub cases: Vec<EvalCase>,
}

#[derive(Debug, thiserror::Error)]
pub enum DatasetError {
    #[error("failed to read or write dataset {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse dataset {path}: {message}")]
    Parse { path: String, message: String },
    #[error("invalid dataset '{id}': {problems:?}")]
    Invalid { id: String, problems: Vec<String> },
}

impl Dataset {
    /// Load from `.yaml`/`.yml` or `.json`, normalise document keys, validate.
    pub fn load(path: &Path) -> Result<Self, DatasetError> {
        let display = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|source| DatasetError::Io {
            path: display.clone(),
            source,
        })?;
        let mut ds: Dataset = match path.extension().and_then(|e| e.to_str()) {
            Some("json") => serde_json::from_str(&text).map_err(|e| DatasetError::Parse {
                path: display.clone(),
                message: e.to_string(),
            })?,
            _ => serde_yaml_ng::from_str(&text).map_err(|e| DatasetError::Parse {
                path: display.clone(),
                message: e.to_string(),
            })?,
        };
        for case in &mut ds.cases {
            case.expected_docs = case
                .expected_docs
                .iter()
                .map(|d| doc_key_from_relative(d))
                .collect();
        }
        ds.validate()?;
        Ok(ds)
    }

    pub fn save(&self, path: &Path) -> Result<(), DatasetError> {
        let display = path.display().to_string();
        let text = serde_yaml_ng::to_string(self).map_err(|e| DatasetError::Parse {
            path: display.clone(),
            message: e.to_string(),
        })?;
        std::fs::write(path, text).map_err(|source| DatasetError::Io { path: display, source })
    }

    pub fn validate(&self) -> Result<(), DatasetError> {
        let mut problems = Vec::new();
        if self.id.trim().is_empty() {
            problems.push("dataset id is empty".to_string());
        }
        if self.cases.is_empty() {
            problems.push("dataset has no cases".to_string());
        }
        let mut seen = BTreeSet::new();
        for c in &self.cases {
            if !seen.insert(c.id.clone()) {
                problems.push(format!("duplicate case id '{}'", c.id));
            }
            if c.question.trim().is_empty() {
                problems.push(format!("case '{}' has an empty question", c.id));
            }
            if let Err(e) = c.expected_decimal() {
                problems.push(format!("case '{}': {e}", c.id));
            }
            let has_answer = !c.expected_answer.is_empty();
            let has_number = c.expected_number.is_some();
            match c.kind {
                QuestionType::Lookup | QuestionType::Compare if !has_answer && !has_number => {
                    problems.push(format!(
                        "{} case '{}' needs expected_answer or expected_number",
                        kind_name(c.kind),
                        c.id
                    ))
                }
                QuestionType::List if !has_answer => {
                    problems.push(format!("list case '{}' needs expected_answer", c.id))
                }
                QuestionType::Aggregate if !has_number => {
                    problems.push(format!("aggregate case '{}' needs expected_number", c.id))
                }
                QuestionType::None if !c.expected_docs.is_empty() => {
                    problems.push(format!("none case '{}' must not list expected_docs", c.id))
                }
                _ => {}
            }
            if c.expected_answer.iter().any(|a| a.trim().is_empty()) {
                problems.push(format!("case '{}' has an empty expected_answer entry", c.id));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(DatasetError::Invalid { id: self.id.clone(), problems })
        }
    }

    /// SHA-256 over canonical JSON; identifies the exact question set a report used.
    pub fn content_hash(&self) -> String {
        let canonical = serde_json::to_vec(self).unwrap_or_default();
        hex::encode(Sha256::digest(&canonical))
    }
}

fn kind_name(kind: QuestionType) -> &'static str {
    match kind {
        QuestionType::Lookup => "lookup",
        QuestionType::List => "list",
        QuestionType::Aggregate => "aggregate",
        QuestionType::Compare => "compare",
        QuestionType::None => "none",
    }
}
```

Note: `serde_json::to_vec` on this type cannot fail (no maps with non-string keys, no custom serializers). `unwrap_or_default` keeps the lint policy without an `expect`.

`crates/shodh-eval/src/corpus.rs` (above the tests module):
```rust
//! Corpus files and document keys.
//!
//! A document key is the corpus-relative path, forward slashes, lowercased.
//! shodh-rag lowercases stored source paths on Windows; lowercasing on every
//! platform keeps keys identical across operating systems.

use std::path::Path;

pub fn doc_key_from_relative(rel: &str) -> String {
    let forward = rel.replace('\\', "/");
    let trimmed = forward.trim_start_matches("./").trim_start_matches('/');
    trimmed.to_lowercase()
}

fn normalise_abs(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Map a source path stored by shodh-rag back to a document key under `root`.
/// Returns `None` when the source is not inside `root`.
pub fn doc_key_from_source(root: &Path, source: &str) -> Option<String> {
    let root_norm = normalise_abs(&root.to_string_lossy());
    let src_norm = normalise_abs(source);
    let rest = src_norm.strip_prefix(&root_norm)?;
    let rest = rest.strip_prefix('/')?;
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

/// All files under `root` whose extension (case-insensitive) is in `extensions`,
/// as sorted document keys.
pub fn list_corpus_files(root: &Path, extensions: &[&str]) -> std::io::Result<Vec<String>> {
    let mut keys = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if !extensions.contains(&ext.as_str()) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_err(std::io::Error::other)?;
        keys.push(doc_key_from_relative(&rel.to_string_lossy()));
    }
    keys.sort();
    Ok(keys)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p shodh-eval --lib dataset:: corpus::`
Expected: 9 tests pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/shodh-eval
git commit -m "Add shodh-eval crate with dataset model and document keys"
```

---

### Task 2: Answer scoring

**Files:**
- Create (replace): `crates/shodh-eval/src/scoring.rs`

**Interfaces:**
- Consumes: `dataset::{EvalCase, QuestionType}`
- Produces:
  - `scoring::{AnswerScore, score_answer(&EvalCase, &str, &[String]) -> Result<AnswerScore, String>}`
  - `scoring::extract_numbers(&str) -> Vec<Decimal>`
  - `scoring::token_recall(&str, &str) -> f64`
  - `scoring::list_recall(&[String], &str) -> f64`
  - `scoring::is_refusal(&str) -> bool`
  - `scoring::LOOKUP_CORRECT_RECALL: f64`

- [ ] **Step 1: Write failing tests**

Append to `crates/shodh-eval/src/scoring.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{EvalCase, NumberLiteral, QuestionType};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn case(kind: QuestionType) -> EvalCase {
        EvalCase {
            id: "c".into(),
            question: "q".into(),
            kind,
            expected_answer: vec![],
            expected_number: None,
            expected_docs: vec!["invoices/a.pdf".into()],
            expected_pages: vec![],
        }
    }

    #[test]
    fn extracts_indian_and_international_amounts() {
        let nums = extract_numbers("Total ₹1,23,456.78; also Rs. 1,23,456.78, INR 123456.78 and $1,234.50");
        assert!(nums.contains(&d("123456.78")));
        assert!(nums.contains(&d("1234.50")));
        assert_eq!(nums.iter().filter(|n| **n == d("123456.78")).count(), 3);
    }

    #[test]
    fn numbers_inside_words_are_not_amounts() {
        let nums = extract_numbers("invoice INV-2026-0042 issued");
        assert!(nums.contains(&d("2026")));
        assert!(nums.contains(&d("42")));
    }

    #[test]
    fn aggregate_matches_at_two_decimals() {
        let mut c = case(QuestionType::Aggregate);
        c.expected_number = Some(NumberLiteral::Text("4200.5".into()));
        let s = score_answer(&c, "The total is Rs. 4,200.50 across 3 invoices [1].", &["invoices/a.pdf".into()]).unwrap();
        assert!(s.correct);
        assert_eq!(s.numeric_match, Some(true));
        assert_eq!(s.citation_hit, Some(true));
        let wrong = score_answer(&c, "The total is 4,200.05.", &[]).unwrap();
        assert!(!wrong.correct);
        assert_eq!(wrong.citation_hit, Some(false));
    }

    #[test]
    fn lookup_uses_best_span_token_recall() {
        let mut c = case(QuestionType::Lookup);
        c.expected_answer = vec!["laws of the State of New York".into(), "New York law".into()];
        let s = score_answer(&c, "It is governed by New York law.", &[]).unwrap();
        assert!((s.score - 1.0).abs() < 1e-9);
        assert!(s.correct);
        let partial = score_answer(&c, "Governed by the laws of the State of Delaware.", &[]).unwrap();
        assert!(partial.score < LOOKUP_CORRECT_RECALL);
        assert!(!partial.correct);
    }

    #[test]
    fn list_requires_every_item() {
        let mut c = case(QuestionType::List);
        c.expected_answer = vec!["INV-2026-0042".into(), "INV-2026-0043".into()];
        let half = score_answer(&c, "Invoices: INV-2026-0042.", &[]).unwrap();
        assert!((half.score - 0.5).abs() < 1e-9);
        assert!(!half.correct);
        let all = score_answer(&c, "inv-2026-0042 and INV 2026 0043", &[]).unwrap();
        assert!(all.correct);
    }

    #[test]
    fn none_requires_refusal() {
        let c = EvalCase { expected_docs: vec![], ..case(QuestionType::None) };
        assert!(score_answer(&c, "I couldn't find this in your documents.", &[]).unwrap().correct);
        assert!(!score_answer(&c, "Our policy allows space travel.", &[]).unwrap().correct);
        assert_eq!(score_answer(&c, "x", &[]).unwrap().citation_hit, None);
    }

    #[test]
    fn token_recall_ignores_case_punctuation_and_stopwords() {
        assert!((token_recall("The Thirty (30) days", "thirty 30 DAYS!") - 1.0).abs() < 1e-9);
        assert_eq!(token_recall("", "anything"), 0.0);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shodh-eval --lib scoring::`
Expected: compile errors (`score_answer` not found).

- [ ] **Step 3: Implement `scoring.rs`**

```rust
//! Answer scoring. Pure functions; no LLM judge.
//!
//! - lookup/compare: best token recall over acceptable spans, or numeric match.
//! - list: fraction of expected items present.
//! - aggregate: expected number present (compared at 2 decimal places).
//! - none: the answer declines (phrase heuristic, reported as such).

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::dataset::{EvalCase, QuestionType};

/// Recall at or above which a lookup answer is reported as correct.
/// Reporting threshold only; gates use the continuous `score`.
pub const LOOKUP_CORRECT_RECALL: f64 = 0.8;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "to", "in", "and", "or", "for", "on", "by", "with", "is", "are",
    "be", "this", "that", "as", "at", "it",
];

const REFUSAL_PHRASES: &[&str] = &[
    "couldn't find", "could not find", "can't find", "cannot find", "unable to find",
    "no information", "not mentioned", "does not contain", "doesn't contain",
    "do not contain", "don't contain", "not found", "no relevant", "not available in",
];

// Constant pattern exercised by every scoring test; a compile failure here is a
// programming error, not a runtime condition.
#[allow(clippy::expect_used)]
static NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9]{1,3}(?:,[0-9]{2,3})+(?:\.[0-9]+)?|[0-9]+(?:\.[0-9]+)?")
        .expect("NUMBER_RE is a valid constant pattern")
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerScore {
    /// Continuous score in [0, 1].
    pub score: f64,
    pub correct: bool,
    pub numeric_match: Option<bool>,
    /// Whether any cited document is an expected document; `None` when the case lists none.
    pub citation_hit: Option<bool>,
}

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect()
}

/// Fraction of `expected` tokens (multiset) present in `answer`.
pub fn token_recall(expected: &str, answer: &str) -> f64 {
    let exp = tokens(expected);
    if exp.is_empty() {
        return 0.0;
    }
    let mut available: HashMap<String, usize> = HashMap::new();
    for t in tokens(answer) {
        *available.entry(t).or_default() += 1;
    }
    let mut hit = 0usize;
    for t in &exp {
        if let Some(n) = available.get_mut(t) {
            if *n > 0 {
                *n -= 1;
                hit += 1;
            }
        }
    }
    hit as f64 / exp.len() as f64
}

fn joined_tokens(text: &str) -> String {
    format!(" {} ", tokens(text).join(" "))
}

/// Fraction of expected items whose token sequence appears in the answer.
pub fn list_recall(items: &[String], answer: &str) -> f64 {
    if items.is_empty() {
        return 0.0;
    }
    let hay = joined_tokens(answer);
    let found = items.iter().filter(|i| hay.contains(&joined_tokens(i))).count();
    found as f64 / items.len() as f64
}

/// All numbers in `text`, with digit-group commas removed (Indian and international grouping).
pub fn extract_numbers(text: &str) -> Vec<Decimal> {
    NUMBER_RE
        .find_iter(text)
        .filter_map(|m| Decimal::from_str(&m.as_str().replace(',', "")).ok())
        .collect()
}

pub fn is_refusal(answer: &str) -> bool {
    let lower = answer.to_lowercase().replace('’', "'");
    REFUSAL_PHRASES.iter().any(|p| lower.contains(p))
}

fn numeric_match(expected: Decimal, answer: &str) -> bool {
    let want = expected.round_dp(2);
    extract_numbers(answer).into_iter().any(|n| n.round_dp(2) == want)
}

pub fn score_answer(case: &EvalCase, answer: &str, cited_doc_keys: &[String]) -> Result<AnswerScore, String> {
    let expected_number = case.expected_decimal()?;
    let citation_hit = if case.expected_docs.is_empty() {
        None
    } else {
        Some(cited_doc_keys.iter().any(|k| case.expected_docs.contains(k)))
    };
    let (score, correct, numeric) = match case.kind {
        QuestionType::Aggregate => {
            let n = expected_number.ok_or_else(|| format!("case '{}' has no expected_number", case.id))?;
            let m = numeric_match(n, answer);
            (if m { 1.0 } else { 0.0 }, m, Some(m))
        }
        QuestionType::Lookup | QuestionType::Compare => match expected_number {
            Some(n) => {
                let m = numeric_match(n, answer);
                (if m { 1.0 } else { 0.0 }, m, Some(m))
            }
            None => {
                let best = case
                    .expected_answer
                    .iter()
                    .map(|span| token_recall(span, answer))
                    .fold(0.0_f64, f64::max);
                (best, best >= LOOKUP_CORRECT_RECALL, None)
            }
        },
        QuestionType::List => {
            let r = list_recall(&case.expected_answer, answer);
            (r, (r - 1.0).abs() < f64::EPSILON, None)
        }
        QuestionType::None => {
            let refused = is_refusal(answer);
            (if refused { 1.0 } else { 0.0 }, refused, None)
        }
    };
    Ok(AnswerScore { score, correct, numeric_match: numeric, citation_hit })
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p shodh-eval --lib scoring::`
Expected: 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/scoring.rs
git commit -m "shodh-eval: add deterministic answer scoring"
```

---

### Task 3: Reports, baselines and the noise gate

**Files:**
- Create (replace): `crates/shodh-eval/src/report.rs`

**Interfaces:**
- Produces:
  - `report::{RunKind, CaseRecord, LatencySummary, IngestSummary, RunReport, MetricStats, Baseline, Regression, CompareOutcome}`
  - `summarize_latencies(&[f64]) -> LatencySummary`
  - `Baseline::from_runs(&[RunReport]) -> Result<Baseline, String>`
  - `compare(&Baseline, &RunReport) -> CompareOutcome`
  - `NOISE_SIGMA: f64 = 3.0`
  - `RunReport::{write, read}`, `Baseline::{write, read}`

- [ ] **Step 1: Write failing tests**

Append to `crates/shodh-eval/src/report.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn run(metrics: &[(&str, f64)], hash: &str) -> RunReport {
        RunReport {
            kind: RunKind::Retrieval,
            dataset_id: "d".into(),
            dataset_hash: hash.into(),
            pipeline: "legacy".into(),
            git_sha: None,
            created_at: chrono::Utc::now(),
            settings: BTreeMap::new(),
            metrics: metrics.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            latency: summarize_latencies(&[1.0]),
            ingest: None,
            cases: vec![],
        }
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let s = summarize_latencies(&[5.0, 1.0, 3.0, 2.0, 4.0]);
        assert_eq!(s.count, 5);
        assert_eq!(s.p50_ms, 3.0);
        assert_eq!(s.p95_ms, 5.0);
        assert_eq!(s.max_ms, 5.0);
        assert_eq!(summarize_latencies(&[]).count, 0);
    }

    #[test]
    fn baseline_mean_and_sample_stddev() {
        let b = Baseline::from_runs(&[run(&[("recall@8", 0.5)], "h"), run(&[("recall@8", 0.7)], "h")]).unwrap();
        let m = &b.metrics["recall@8"];
        assert!((m.mean - 0.6).abs() < 1e-12);
        assert!((m.stddev - 0.141_421_356_237).abs() < 1e-9);
        assert_eq!(m.samples, 2);
    }

    #[test]
    fn baseline_rejects_mixed_datasets() {
        assert!(Baseline::from_runs(&[run(&[("m", 1.0)], "a"), run(&[("m", 1.0)], "b")]).is_err());
        assert!(Baseline::from_runs(&[]).is_err());
    }

    #[test]
    fn zero_variance_any_drop_fails_and_improvement_passes() {
        let b = Baseline::from_runs(&[run(&[("recall@8", 0.62)], "h")]).unwrap();
        assert!(matches!(compare(&b, &run(&[("recall@8", 0.6199)], "h")), CompareOutcome::Fail { .. }));
        assert!(matches!(compare(&b, &run(&[("recall@8", 0.62)], "h")), CompareOutcome::Pass { .. }));
        match compare(&b, &run(&[("recall@8", 0.70)], "h")) {
            CompareOutcome::Pass { improvements } => assert_eq!(improvements.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn drop_within_three_sigma_passes() {
        let b = Baseline::from_runs(&[run(&[("m", 0.50)], "h"), run(&[("m", 0.52)], "h"), run(&[("m", 0.54)], "h")]).unwrap();
        // mean 0.52, stddev 0.02 -> allowed_min 0.46
        assert!(matches!(compare(&b, &run(&[("m", 0.47)], "h")), CompareOutcome::Pass { .. }));
        assert!(matches!(compare(&b, &run(&[("m", 0.45)], "h")), CompareOutcome::Fail { .. }));
    }

    #[test]
    fn missing_metric_and_dataset_mismatch_fail() {
        let b = Baseline::from_runs(&[run(&[("a", 1.0), ("b", 1.0)], "h")]).unwrap();
        match compare(&b, &run(&[("a", 1.0)], "h")) {
            CompareOutcome::Fail { missing, .. } => assert_eq!(missing, vec!["b".to_string()]),
            other => panic!("{other:?}"),
        }
        match compare(&b, &run(&[("a", 1.0), ("b", 1.0)], "other")) {
            CompareOutcome::Fail { dataset_mismatch, .. } => assert!(dataset_mismatch.is_some()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reports_round_trip_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.json");
        let r = run(&[("m", 0.5)], "h");
        r.write(&p).unwrap();
        assert_eq!(RunReport::read(&p).unwrap().metrics, r.metrics);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shodh-eval --lib report::`
Expected: compile errors.

- [ ] **Step 3: Implement `report.rs`**

```rust
//! Run reports, baselines and the regression gate.
//!
//! Gate rule: a higher-is-better metric fails when
//! `current < mean - NOISE_SIGMA * stddev - FLOAT_EPSILON`, where mean/stddev come
//! from repeated baseline runs. Thresholds are measured, never hand-tuned.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const NOISE_SIGMA: f64 = 3.0;
const FLOAT_EPSILON: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Retrieval,
    Answers,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseRecord {
    pub id: String,
    pub question: String,
    pub metrics: BTreeMap<String, f64>,
    pub retrieved_docs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencySummary {
    pub count: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestSummary {
    pub files_seen: usize,
    pub files_indexed: usize,
    pub files_failed: usize,
    pub failed_files: Vec<String>,
    pub chunks: usize,
    pub seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub kind: RunKind,
    pub dataset_id: String,
    pub dataset_hash: String,
    /// Which shodh pipeline produced this run, e.g. "legacy-ragengine-search".
    pub pipeline: String,
    pub git_sha: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Run parameters (k, model, endpoint) recorded for reproducibility.
    pub settings: BTreeMap<String, String>,
    /// Higher-is-better quality metrics; these are gated.
    pub metrics: BTreeMap<String, f64>,
    /// Reported, never gated (shared CI runners are too noisy).
    pub latency: LatencySummary,
    pub ingest: Option<IngestSummary>,
    pub cases: Vec<CaseRecord>,
}

impl RunReport {
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
    }

    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

/// Nearest-rank percentiles over milliseconds.
pub fn summarize_latencies(ms: &[f64]) -> LatencySummary {
    if ms.is_empty() {
        return LatencySummary { count: 0, p50_ms: 0.0, p95_ms: 0.0, max_ms: 0.0 };
    }
    let mut sorted = ms.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = |p: f64| -> f64 {
        let idx = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
        sorted[idx.clamp(1, sorted.len()) - 1]
    };
    LatencySummary {
        count: sorted.len(),
        p50_ms: rank(50.0),
        p95_ms: rank(95.0),
        max_ms: sorted[sorted.len() - 1],
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricStats {
    pub mean: f64,
    /// Sample standard deviation (n-1); 0 for a single run.
    pub stddev: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub kind: RunKind,
    pub dataset_id: String,
    pub dataset_hash: String,
    pub pipeline: String,
    pub recorded_at: DateTime<Utc>,
    pub git_sha: Option<String>,
    pub metrics: BTreeMap<String, MetricStats>,
}

impl Baseline {
    pub fn from_runs(runs: &[RunReport]) -> Result<Self, String> {
        let first = runs.first().ok_or("no runs supplied")?;
        if let Some(other) = runs.iter().find(|r| r.dataset_hash != first.dataset_hash || r.kind != first.kind) {
            return Err(format!(
                "runs disagree on dataset/kind: {} vs {}",
                first.dataset_hash, other.dataset_hash
            ));
        }
        let mut metrics = BTreeMap::new();
        for name in first.metrics.keys() {
            let values: Vec<f64> = runs
                .iter()
                .map(|r| r.metrics.get(name).copied().ok_or_else(|| format!("metric '{name}' missing in a run")))
                .collect::<Result<_, _>>()?;
            let n = values.len() as f64;
            let mean = values.iter().sum::<f64>() / n;
            let stddev = if values.len() > 1 {
                (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
            } else {
                0.0
            };
            metrics.insert(name.clone(), MetricStats { mean, stddev, samples: values.len() });
        }
        Ok(Self {
            kind: first.kind,
            dataset_id: first.dataset_id.clone(),
            dataset_hash: first.dataset_hash.clone(),
            pipeline: first.pipeline.clone(),
            recorded_at: Utc::now(),
            git_sha: first.git_sha.clone(),
            metrics,
        })
    }

    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", path.display()))
    }

    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub metric: String,
    pub baseline_mean: f64,
    pub allowed_min: f64,
    pub current: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompareOutcome {
    Pass { improvements: Vec<(String, f64, f64)> },
    Fail { regressions: Vec<Regression>, missing: Vec<String>, dataset_mismatch: Option<String> },
}

pub fn compare(baseline: &Baseline, current: &RunReport) -> CompareOutcome {
    let dataset_mismatch = (baseline.dataset_hash != current.dataset_hash).then(|| {
        format!(
            "baseline dataset {} ({}) != current {} ({}); re-record the baseline deliberately",
            baseline.dataset_id, baseline.dataset_hash, current.dataset_id, current.dataset_hash
        )
    });
    let mut regressions = Vec::new();
    let mut missing = Vec::new();
    let mut improvements = Vec::new();
    for (name, stats) in &baseline.metrics {
        let Some(&now) = current.metrics.get(name) else {
            missing.push(name.clone());
            continue;
        };
        let allowed_min = stats.mean - NOISE_SIGMA * stats.stddev - FLOAT_EPSILON;
        if now < allowed_min {
            regressions.push(Regression { metric: name.clone(), baseline_mean: stats.mean, allowed_min, current: now });
        } else if now > stats.mean + FLOAT_EPSILON {
            improvements.push((name.clone(), stats.mean, now));
        }
    }
    if dataset_mismatch.is_some() || !regressions.is_empty() || !missing.is_empty() {
        CompareOutcome::Fail { regressions, missing, dataset_mismatch }
    } else {
        CompareOutcome::Pass { improvements }
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p shodh-eval --lib report::`
Expected: 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/report.rs
git commit -m "shodh-eval: add run reports, baselines and 3-sigma regression gate"
```

---

### Task 4: Checksum-verified downloads (CUAD, models)

**Files:**
- Create (replace): `crates/shodh-eval/src/fetch.rs`

**Interfaces:**
- Produces:
  - `fetch::{Checksum, PinnedFile, CUAD_ZIP, MODEL_FILES}`
  - `fetch::verify_file(&Path, &Checksum) -> anyhow::Result<bool>`
  - `fetch::fetch_pinned(&PinnedFile, &Path) -> anyhow::Result<PathBuf>` (async)
  - `fetch::fetch_models(&Path) -> anyhow::Result<()>` (async)

- [ ] **Step 1: Write failing tests**

Append to `crates/shodh-eval/src/fetch.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_sha256_and_md5() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"hello").unwrap();
        let sha = Checksum::Sha256("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        let md5 = Checksum::Md5("5d41402abc4b2a76b9719d911017c592");
        assert!(verify_file(&p, &sha).unwrap());
        assert!(verify_file(&p, &md5).unwrap());
        assert!(!verify_file(&p, &Checksum::Md5("00000000000000000000000000000000")).unwrap());
    }

    #[test]
    fn pinned_table_is_complete() {
        assert_eq!(MODEL_FILES.len(), 4);
        assert!(MODEL_FILES.iter().all(|f| f.url.contains("/resolve/") && !f.url.contains("/resolve/main/")));
        assert!(CUAD_ZIP.url.starts_with("https://zenodo.org/"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shodh-eval --lib fetch::`
Expected: compile errors.

- [ ] **Step 3: Implement `fetch.rs`**

```rust
//! Downloads of pinned external artifacts, verified by checksum before use.
//! Files are written to `<name>.part` and renamed only after verification.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use futures_util::StreamExt;
use md5::Md5;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy)]
pub enum Checksum {
    Sha256(&'static str),
    Md5(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct PinnedFile {
    pub url: &'static str,
    /// Path relative to the destination directory.
    pub relative_path: &'static str,
    pub checksum: Checksum,
}

/// CUAD v1 (Hendrycks et al., 2021), CC BY 4.0.
pub const CUAD_ZIP: PinnedFile = PinnedFile {
    url: "https://zenodo.org/records/4595826/files/CUAD_v1.zip",
    relative_path: "CUAD_v1.zip",
    checksum: Checksum::Md5("c38f490a984420b8a62600db401fafd5"),
};

pub const MODEL_FILES: [PinnedFile; 4] = [
    PinnedFile {
        url: "https://huggingface.co/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/onnx/model_O4.onnx",
        relative_path: "multilingual-e5-base/model_O4.onnx",
        checksum: Checksum::Sha256("f60256a833caee5c75a3903e589116752ee016ca7bc16f9b96e4db09984c5703"),
    },
    PinnedFile {
        url: "https://huggingface.co/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/onnx/tokenizer.json",
        relative_path: "multilingual-e5-base/tokenizer.json",
        checksum: Checksum::Sha256("62c24cdc13d4c9952d63718d6c9fa4c287974249e16b7ade6d5a85e7bbb75626"),
    },
    PinnedFile {
        url: "https://huggingface.co/cross-encoder/ms-marco-MiniLM-L-6-v2/resolve/233902d25c440f23af6f7d6e94d2946bac0bee0a/onnx/model_O4.onnx",
        relative_path: "ms-marco-MiniLM-L6-v2/model_O4.onnx",
        checksum: Checksum::Sha256("b232c2eeedd97a593edc177e3ce4cbd1d6c8f6d8f61a5c201cd0cdeb8134da18"),
    },
    PinnedFile {
        url: "https://huggingface.co/cross-encoder/ms-marco-MiniLM-L-6-v2/resolve/233902d25c440f23af6f7d6e94d2946bac0bee0a/tokenizer.json",
        relative_path: "ms-marco-MiniLM-L6-v2/tokenizer.json",
        checksum: Checksum::Sha256("d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66"),
    },
];

pub fn verify_file(path: &Path, checksum: &Checksum) -> anyhow::Result<bool> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let actual = match checksum {
        Checksum::Sha256(_) => {
            let mut h = Sha256::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            hex::encode(h.finalize())
        }
        Checksum::Md5(_) => {
            let mut h = Md5::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            hex::encode(h.finalize())
        }
    };
    let expected = match checksum {
        Checksum::Sha256(s) | Checksum::Md5(s) => *s,
    };
    Ok(actual.eq_ignore_ascii_case(expected))
}

/// Download `file` into `dest_dir` unless a verified copy already exists.
pub async fn fetch_pinned(file: &PinnedFile, dest_dir: &Path) -> anyhow::Result<PathBuf> {
    let target = dest_dir.join(file.relative_path);
    if target.exists() && verify_file(&target, &file.checksum)? {
        tracing::info!(path = %target.display(), "already present and verified");
        return Ok(target);
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part = target.with_extension("part");
    tracing::info!(url = file.url, "downloading");
    let response = reqwest::get(file.url).await?.error_for_status()?;
    let mut out = tokio::fs::File::create(&part).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        out.write_all(&chunk?).await?;
    }
    out.flush().await?;
    drop(out);
    if !verify_file(&part, &file.checksum)? {
        tokio::fs::remove_file(&part).await.ok();
        bail!("checksum mismatch for {}; refusing to use it", file.url);
    }
    tokio::fs::rename(&part, &target).await?;
    Ok(target)
}

pub async fn fetch_models(models_dir: &Path) -> anyhow::Result<()> {
    for f in &MODEL_FILES {
        fetch_pinned(f, models_dir).await?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p shodh-eval --lib fetch::`
Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/fetch.rs
git commit -m "shodh-eval: add checksum-verified downloads for CUAD and models"
```

---

### Task 5: CUAD corpus and dataset converter

**Files:**
- Create (replace): `crates/shodh-eval/src/cuad.rs`

**Interfaces:**
- Consumes:
  - `dataset::{Dataset, EvalCase, QuestionType}`
  - `corpus::doc_key_from_relative`
- Produces:
  - `cuad::{CuadProfile, ConvertSummary}`
  - `cuad::convert(zip_path: &Path, out_dir: &Path, profile: CuadProfile, seed: u64) -> anyhow::Result<ConvertSummary>`
  - Writes `out_dir/corpus/<contract>.pdf` and `out_dir/dataset.yaml`.

CUAD format, as published:
- `CUAD_v1/CUAD_v1.json` is SQuAD-style: `{"data":[{"title":T,"paragraphs":[{"context":..., "qas":[{"id":"T__Category","question":..., "answers":[{"text":..., "answer_start":N}], "is_impossible":bool}]}]}]}`.
- Contract PDFs are under `CUAD_v1/full_contract_pdf/**/<T>.pdf`.

The converter depends on that layout. When the layout does not match, it **fails with counts**. It never produces a degraded dataset.

- [ ] **Step 1: Write failing tests using a tiny in-test zip**

Append to `crates/shodh-eval/src/cuad.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn qa(title: &str, cat: &str, answers: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "id": format!("{title}__{cat}"),
            "question": format!("Highlight the parts (if any) of this contract related to \"{cat}\""),
            "answers": answers.iter().map(|a| serde_json::json!({"text": a, "answer_start": 0})).collect::<Vec<_>>(),
            "is_impossible": answers.is_empty(),
        })
    }

    fn contract(title: &str) -> serde_json::Value {
        serde_json::json!({"title": title, "paragraphs": [{"context": "text", "qas": [
            qa(title, "Document Name", &["MASTER SERVICES AGREEMENT"]),
            qa(title, "Parties", &["Acme Corp", "Beta LLC", "Acme"]),
            qa(title, "Governing Law", &["laws of the State of New York"]),
            qa(title, "Expiration Date", &[]),
            qa(title, "Agreement Date", &["January 5, 2019"]),
        ]}]})
    }

    fn build_zip(dir: &Path, titles: &[&str], pdfs: &[&str]) -> PathBuf {
        let path = dir.join("CUAD_v1.zip");
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        let data = serde_json::json!({"version": "aok_v1.0", "data": titles.iter().map(|t| contract(t)).collect::<Vec<_>>()});
        zw.start_file("CUAD_v1/CUAD_v1.json", opts).unwrap();
        zw.write_all(data.to_string().as_bytes()).unwrap();
        for t in pdfs {
            zw.start_file(format!("CUAD_v1/full_contract_pdf/Part_I/Services/{t}.pdf"), opts).unwrap();
            zw.write_all(b"%PDF-1.4 test").unwrap();
        }
        zw.finish().unwrap();
        path
    }

    #[test]
    fn converts_answerable_questions_and_copies_pdfs() {
        let dir = tempfile::tempdir().unwrap();
        let titles = ["AcmeCorp_MSA_2019", "Beta_Supply"];
        let zip = build_zip(dir.path(), &titles, &titles);
        let out = dir.path().join("out");
        let s = convert(&zip, &out, CuadProfile::Full, 7).unwrap();
        assert_eq!(s.contracts_used, 2);
        let ds = Dataset::load(&out.join("dataset.yaml")).unwrap();
        // Governing Law + Agreement Date answerable; Expiration Date impossible -> skipped.
        assert_eq!(ds.cases.len(), 4);
        let q = &ds.cases[0];
        assert!(q.question.contains("MASTER SERVICES AGREEMENT"));
        assert!(q.question.contains("Acme Corp and Beta LLC"));
        assert_eq!(q.expected_docs.len(), 1);
        assert!(out.join("corpus").join(format!("{}.pdf", titles[0])).exists());
    }

    #[test]
    fn same_seed_same_subset() {
        let dir = tempfile::tempdir().unwrap();
        let titles: Vec<String> = (0..80).map(|i| format!("C{i:03}")).collect();
        let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
        let zip = build_zip(dir.path(), &refs, &refs);
        let a = convert(&zip, &dir.path().join("a"), CuadProfile::Pr, 42).unwrap();
        let b = convert(&zip, &dir.path().join("b"), CuadProfile::Pr, 42).unwrap();
        assert_eq!(a.contracts_used, PR_CONTRACTS);
        assert_eq!(
            Dataset::load(&dir.path().join("a/dataset.yaml")).unwrap(),
            Dataset::load(&dir.path().join("b/dataset.yaml")).unwrap()
        );
    }

    #[test]
    fn fails_loudly_when_pdfs_do_not_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let zip = build_zip(dir.path(), &["A", "B", "C"], &["A"]);
        let err = convert(&zip, &dir.path().join("o"), CuadProfile::Full, 1).unwrap_err().to_string();
        assert!(err.contains("resolved 1 of 3"), "{err}");
    }

    #[test]
    fn fails_loudly_without_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.zip");
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zw.start_file("README.txt", zip::write::SimpleFileOptions::default()).unwrap();
        zw.finish().unwrap();
        let err = convert(&path, &dir.path().join("o"), CuadProfile::Full, 1).unwrap_err().to_string();
        assert!(err.contains("CUAD_v1.json"), "{err}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shodh-eval --lib cuad::`
Expected: compile errors.

- [ ] **Step 3: Implement `cuad.rs`**

```rust
//! CUAD (Contract Understanding Atticus Dataset) → shodh-eval corpus + dataset.
//!
//! Each question names the contract (from CUAD's "Document Name" and "Parties"
//! annotations), so answering requires finding the right contract in the corpus
//! and the right clause in it. Expected answers are CUAD's expert-annotated spans.
//! Source: Hendrycks et al., "CUAD", 2021. Licence CC BY 4.0.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context};
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::Deserialize;

use crate::corpus::doc_key_from_relative;
use crate::dataset::{Dataset, EvalCase, QuestionType};

/// Contracts sampled for the per-PR profile.
pub const PR_CONTRACTS: usize = 60;
/// Minimum fraction of contracts whose PDF must resolve; below this the zip layout
/// is not what this converter understands, and it refuses to continue.
const MIN_PDF_RESOLUTION: f64 = 0.95;
const MAX_NAME_CHARS: usize = 120;

/// (CUAD category, question phrasing). Order defines case order within a contract.
const CATEGORIES: &[(&str, &str)] = &[
    ("Governing Law", "which jurisdiction's law governs the agreement?"),
    ("Agreement Date", "what is the date of the agreement?"),
    ("Expiration Date", "when does the agreement expire?"),
    ("Renewal Term", "what is the renewal term?"),
    ("Notice Period To Terminate Renewal", "what notice is required to terminate renewal?"),
    ("Cap On Liability", "what is the cap on liability?"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuadProfile {
    /// Seeded sample of `PR_CONTRACTS` contracts for per-PR CI.
    Pr,
    /// Every contract.
    Full,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConvertSummary {
    pub contracts_in_json: usize,
    pub contracts_with_pdf: usize,
    pub contracts_used: usize,
    pub cases: usize,
}

#[derive(Deserialize)]
struct Squad {
    data: Vec<SquadDoc>,
}
#[derive(Deserialize)]
struct SquadDoc {
    title: String,
    paragraphs: Vec<SquadParagraph>,
}
#[derive(Deserialize)]
struct SquadParagraph {
    qas: Vec<SquadQa>,
}
#[derive(Deserialize)]
struct SquadQa {
    id: String,
    answers: Vec<SquadAnswer>,
    #[serde(default)]
    is_impossible: bool,
}
#[derive(Deserialize)]
struct SquadAnswer {
    text: String,
}

fn clean(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(MAX_NAME_CHARS).collect()
}

fn answers_by_category(doc: &SquadDoc) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for p in &doc.paragraphs {
        for q in &p.qas {
            let Some((_, category)) = q.id.rsplit_once("__") else { continue };
            if q.is_impossible {
                continue;
            }
            let entry = map.entry(category.to_string()).or_default();
            for a in &q.answers {
                let t = clean(&a.text);
                if !t.is_empty() && !entry.contains(&t) {
                    entry.push(t);
                }
            }
        }
    }
    map
}

pub fn convert(zip_path: &Path, out_dir: &Path, profile: CuadProfile, seed: u64) -> anyhow::Result<ConvertSummary> {
    let file = std::fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("reading CUAD zip")?;

    let json_name = (0..archive.len())
        .filter_map(|i| archive.name_for_index(i).map(str::to_string))
        .find(|n| n.ends_with("CUAD_v1.json"))
        .ok_or_else(|| anyhow::anyhow!("CUAD_v1.json not found in {}", zip_path.display()))?;
    let mut json = String::new();
    archive.by_name(&json_name)?.read_to_string(&mut json)?;
    let squad: Squad = serde_json::from_str(&json).context("parsing CUAD_v1.json")?;

    // title -> zip entry of its PDF (matched on file stem under full_contract_pdf/).
    let mut pdf_by_title: BTreeMap<String, String> = BTreeMap::new();
    for i in 0..archive.len() {
        let Some(name) = archive.name_for_index(i) else { continue };
        if !name.contains("full_contract_pdf/") || !name.to_lowercase().ends_with(".pdf") {
            continue;
        }
        if let Some(stem) = Path::new(name).file_stem().and_then(|s| s.to_str()) {
            pdf_by_title.insert(stem.to_string(), name.to_string());
        }
    }

    let total = squad.data.len();
    let mut eligible: Vec<&SquadDoc> = squad.data.iter().filter(|d| pdf_by_title.contains_key(&d.title)).collect();
    let resolved = eligible.len();
    if total == 0 || (resolved as f64) < MIN_PDF_RESOLUTION * total as f64 {
        bail!(
            "CUAD layout not recognised: resolved {resolved} of {total} contract PDFs under full_contract_pdf/ (need >= {:.0}%)",
            MIN_PDF_RESOLUTION * 100.0
        );
    }
    eligible.sort_by(|a, b| a.title.cmp(&b.title));
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    eligible.shuffle(&mut rng);
    if profile == CuadProfile::Pr {
        eligible.truncate(PR_CONTRACTS);
    }
    eligible.sort_by(|a, b| a.title.cmp(&b.title));

    let corpus_dir = out_dir.join("corpus");
    std::fs::create_dir_all(&corpus_dir)?;
    let mut cases = Vec::new();
    let mut used = 0usize;
    for doc in &eligible {
        let answers = answers_by_category(doc);
        let Some(name) = answers.get("Document Name").and_then(|v| v.first()) else { continue };
        let parties: Vec<&String> = answers.get("Parties").map(|v| v.iter().take(2).collect()).unwrap_or_default();
        let subject = if parties.is_empty() {
            format!("the {name}")
        } else {
            format!("the {name} between {}", parties.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(" and "))
        };
        let file_name = format!("{}.pdf", doc.title);
        let doc_key = doc_key_from_relative(&file_name);
        let mut added = 0usize;
        for (category, phrasing) in CATEGORIES {
            let Some(spans) = answers.get(*category) else { continue };
            if spans.is_empty() {
                continue;
            }
            cases.push(EvalCase {
                id: format!("cuad-{}-{}", doc_key.trim_end_matches(".pdf"), category.to_lowercase().replace(' ', "-")),
                question: format!("In {subject}, {phrasing}"),
                kind: QuestionType::Lookup,
                expected_answer: spans.clone(),
                expected_number: None,
                expected_docs: vec![doc_key.clone()],
                expected_pages: vec![],
            });
            added += 1;
        }
        if added == 0 {
            continue;
        }
        let entry = pdf_by_title.get(&doc.title).ok_or_else(|| anyhow::anyhow!("pdf vanished for {}", doc.title))?;
        let mut bytes = Vec::new();
        archive.by_name(entry)?.read_to_end(&mut bytes)?;
        std::fs::write(corpus_dir.join(&file_name), bytes)?;
        used += 1;
    }

    let dataset = Dataset {
        id: match profile {
            CuadProfile::Pr => format!("cuad-pr-seed{seed}"),
            CuadProfile::Full => "cuad-full".to_string(),
        },
        description: "CUAD v1 contract clause questions (Hendrycks et al. 2021, CC BY 4.0)".to_string(),
        cases,
    };
    dataset.validate()?;
    dataset.save(&out_dir.join("dataset.yaml"))?;
    Ok(ConvertSummary { contracts_in_json: total, contracts_with_pdf: resolved, contracts_used: used, cases: dataset.cases.len() })
}

```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p shodh-eval --lib cuad::`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/cuad.rs
git commit -m "shodh-eval: add CUAD corpus and dataset converter"
```

---

### Task 6: Deterministic synthetic invoice corpus

**Files:**
- Create (replace): `crates/shodh-eval/src/invoices.rs`

**Interfaces:**
- Consumes:
  - `dataset::{Dataset, EvalCase, QuestionType, NumberLiteral}`
- Produces:
  - `invoices::{InvoiceSpec, generate(out_dir: &Path, seed: u64, count: usize) -> anyhow::Result<Vec<InvoiceSpec>>}`
  - Writes `out_dir/corpus/invoices/<number>.pdf` and `out_dir/dataset.yaml`.

**Design:**
- **Parties:** 8 fictional vendors and 3 buyers, all with valid-format GSTINs.
- **Dates:** invoice dates spread over 2026.
- **Line items:** 1–5 per invoice, with quantity × rate. Tax is 18% GST. Total = subtotal + tax, rounded to 2 dp.
- **Layouts:** four layouts that vary the label wording ("Total" / "Amount Due" / "Grand Total" / "Invoice Total"), the date format (`DD/MM/YYYY`, `DD-Mon-YYYY`, `YYYY-MM-DD`, `D Month YYYY`) and the currency prefix (`INR`, `Rs.`). Text uses PDF base-14 Helvetica (WinAnsi), so the `₹` glyph is not used here; Task 2's scoring tests cover `₹` in answers.
- **Questions:**
  - lookup: total of one invoice
  - lookup: vendor GSTIN on one invoice
  - aggregate: vendor→buyer sum for Q3 (1 Jul–30 Sep 2026)
  - aggregate: vendor invoice count in Q3
  - list: vendor invoice numbers in August 2026

- [ ] **Step 1: Write failing tests**

Append to `crates/shodh-eval/src/invoices.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::Dataset;
    use sha2::{Digest, Sha256};

    fn hash_dir(dir: &Path) -> String {
        let mut files: Vec<_> = walkdir::WalkDir::new(dir).into_iter().filter_map(Result::ok).filter(|e| e.file_type().is_file()).map(|e| e.path().to_path_buf()).collect();
        files.sort();
        let mut h = Sha256::new();
        for f in files {
            h.update(f.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/").as_bytes());
            h.update(std::fs::read(&f).unwrap());
        }
        hex::encode(h.finalize())
    }

    #[test]
    fn generation_is_byte_for_byte_deterministic() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        generate(a.path(), 11, 40).unwrap();
        generate(b.path(), 11, 40).unwrap();
        assert_eq!(hash_dir(a.path()), hash_dir(b.path()));
    }

    #[test]
    fn totals_are_consistent_and_dataset_matches_specs() {
        let dir = tempfile::tempdir().unwrap();
        let specs = generate(dir.path(), 3, 60).unwrap();
        assert_eq!(specs.len(), 60);
        for s in &specs {
            let sub: Decimal = s.lines.iter().map(|l| l.quantity * l.rate).sum();
            assert_eq!(s.subtotal, sub.round_dp(2));
            assert_eq!(s.tax, (s.subtotal * Decimal::new(18, 2)).round_dp(2));
            assert_eq!(s.total, s.subtotal + s.tax);
        }
        let ds = Dataset::load(&dir.path().join("dataset.yaml")).unwrap();
        let agg = ds.cases.iter().find(|c| c.id.starts_with("inv-q3-sum-")).unwrap();
        let vendor_buyer: Vec<&str> = agg.id.trim_start_matches("inv-q3-sum-").splitn(2, "--").collect();
        let expected: Decimal = specs.iter()
            .filter(|s| slug(&s.vendor.name) == vendor_buyer[0] && slug(&s.buyer) == vendor_buyer[1] && in_q3(s.date))
            .map(|s| s.total).sum();
        assert_eq!(agg.expected_decimal().unwrap(), Some(expected));
        assert!(!agg.expected_docs.is_empty());
    }

    #[test]
    fn shodh_parser_reads_back_the_total() {
        let dir = tempfile::tempdir().unwrap();
        let specs = generate(dir.path(), 5, 4).unwrap();
        let path = dir.path().join("corpus").join("invoices").join(format!("{}.pdf", specs[0].number.to_lowercase()));
        let parsed = shodh_rag::processing::parser::DocumentParser::new().parse_file(&path).unwrap();
        assert!(parsed.content.contains(&specs[0].number), "{}", parsed.content);
        let total_text = format_amount(specs[0].total);
        assert!(parsed.content.replace(' ', "").contains(&total_text.replace(' ', "")), "{}", parsed.content);
    }
}
```

The last test checks that the generated PDFs exercise the real `shodh-rag` parser. Before running it, confirm that `shodh_rag::processing::parser::DocumentParser::new()` and `parse_file(&Path) -> Result<ParsedDoc>` (with a public `content: String` field) exist exactly as named, by reading `crates/shodh-rag/src/processing/parser.rs`. If `processing::parser` is not `pub`, make it `pub` in `crates/shodh-rag/src/processing/mod.rs`. That is a visibility-only change, and it goes in this task's commit.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shodh-eval --lib invoices::`
Expected: compile errors.

- [ ] **Step 3: Implement `invoices.rs`**

```rust
//! Deterministic synthetic invoice corpus with exact ground truth.
//!
//! Same seed ⇒ byte-identical PDFs and dataset. PDFs are text PDFs built with lopdf
//! using base-14 Helvetica, rendered in one of four layouts that vary labels,
//! date formats and currency prefixes.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use chrono::{Datelike, NaiveDate};
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rust_decimal::Decimal;

use crate::dataset::{Dataset, EvalCase, NumberLiteral, QuestionType};

#[derive(Debug, Clone, PartialEq)]
pub struct Vendor {
    pub name: String,
    pub gstin: String,
    pub city: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub description: String,
    pub quantity: Decimal,
    pub rate: Decimal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvoiceSpec {
    pub number: String,
    pub date: NaiveDate,
    pub vendor: Vendor,
    pub buyer: String,
    pub lines: Vec<Line>,
    pub subtotal: Decimal,
    pub tax: Decimal,
    pub total: Decimal,
    pub layout: usize,
}

const VENDORS: &[(&str, &str, &str)] = &[
    ("Acme Traders", "27AAACA1234F1Z5", "Mumbai"),
    ("Bharat Steel Works", "24AABCB2345G1Z3", "Ahmedabad"),
    ("Coastal Logistics", "33AACCC3456H1Z1", "Chennai"),
    ("Deccan Software Services", "36AADCD4567J1Z9", "Hyderabad"),
    ("Everest Office Supplies", "07AAECE5678K1Z7", "New Delhi"),
    ("Frontier Packaging", "29AAFCF6789L1Z5", "Bengaluru"),
    ("Ganga Chemicals", "09AAGCG7890M1Z3", "Kanpur"),
    ("Himalaya Facility Management", "06AAHCH8901N1Z1", "Gurugram"),
];
const BUYERS: &[&str] = &["Roshera Technologies", "Northwind Retail", "Saffron Hospitality"];
const ITEMS: &[&str] = &[
    "Consulting hours", "Steel rods (kg)", "Freight charges", "Software licence",
    "A4 paper (ream)", "Corrugated boxes", "Industrial solvent (L)", "Housekeeping services",
];
const TOTAL_LABELS: [&str; 4] = ["Total", "Amount Due", "Grand Total", "Invoice Total"];
const CURRENCY: [&str; 4] = ["INR", "Rs.", "INR", "Rs."];

pub fn slug(s: &str) -> String {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

pub fn in_q3(d: NaiveDate) -> bool {
    d.year() == 2026 && (7..=9).contains(&d.month())
}

/// Indian digit grouping with 2 decimals, e.g. 123456.7 -> "1,23,456.70".
pub fn format_amount(v: Decimal) -> String {
    let s = format!("{:.2}", v.round_dp(2));
    let (int, frac) = s.split_once('.').unwrap_or((s.as_str(), "00"));
    let (sign, digits) = int.strip_prefix('-').map(|d| ("-", d)).unwrap_or(("", int));
    let grouped = if digits.len() <= 3 {
        digits.to_string()
    } else {
        let (head, last3) = digits.split_at(digits.len() - 3);
        let mut parts: Vec<String> = Vec::new();
        let mut rest = head;
        while rest.len() > 2 {
            let (a, b) = rest.split_at(rest.len() - 2);
            parts.push(b.to_string());
            rest = a;
        }
        if !rest.is_empty() {
            parts.push(rest.to_string());
        }
        parts.reverse();
        format!("{},{}", parts.join(","), last3)
    };
    format!("{sign}{grouped}.{frac}")
}

fn format_date(d: NaiveDate, layout: usize) -> String {
    match layout {
        0 => d.format("%d/%m/%Y").to_string(),
        1 => d.format("%d-%b-%Y").to_string(),
        2 => d.format("%Y-%m-%d").to_string(),
        _ => format!("{} {}", d.day(), d.format("%B %Y")),
    }
}

fn spec(rng: &mut ChaCha8Rng, index: usize) -> anyhow::Result<InvoiceSpec> {
    let (vname, gstin, city) = VENDORS[rng.gen_range(0..VENDORS.len())];
    let buyer = BUYERS[rng.gen_range(0..BUYERS.len())].to_string();
    let day_of_year = rng.gen_range(0..365u32);
    let date = NaiveDate::from_yo_opt(2026, day_of_year + 1).context("valid 2026 ordinal")?;
    let n_lines = rng.gen_range(1..=5usize);
    let mut lines = Vec::with_capacity(n_lines);
    for _ in 0..n_lines {
        lines.push(Line {
            description: ITEMS[rng.gen_range(0..ITEMS.len())].to_string(),
            quantity: Decimal::from(rng.gen_range(1..=50u32)),
            rate: Decimal::new(rng.gen_range(5_000..=2_500_000i64), 2),
        });
    }
    let subtotal: Decimal = lines.iter().map(|l| l.quantity * l.rate).sum::<Decimal>().round_dp(2);
    let tax = (subtotal * Decimal::new(18, 2)).round_dp(2);
    Ok(InvoiceSpec {
        number: format!("INV-2026-{:04}", index + 1),
        date,
        vendor: Vendor { name: vname.to_string(), gstin: gstin.to_string(), city: city.to_string() },
        buyer,
        lines,
        subtotal,
        tax,
        total: subtotal + tax,
        layout: rng.gen_range(0..4usize),
    })
}

fn render_lines(s: &InvoiceSpec) -> Vec<String> {
    let cur = CURRENCY[s.layout];
    let mut out = Vec::new();
    let header = match s.layout {
        0 => vec![format!("TAX INVOICE"), s.vendor.name.clone(), format!("GSTIN: {}", s.vendor.gstin), s.vendor.city.clone()],
        1 => vec![s.vendor.name.clone(), format!("{} | GSTIN {}", s.vendor.city, s.vendor.gstin), "INVOICE".to_string()],
        2 => vec![format!("Invoice from {}", s.vendor.name), format!("Supplier GSTIN: {}", s.vendor.gstin)],
        _ => vec![s.vendor.name.to_uppercase(), format!("GST Registration No. {}", s.vendor.gstin), format!("Registered office: {}", s.vendor.city)],
    };
    out.extend(header);
    out.push(String::new());
    out.push(format!("Invoice No: {}", s.number));
    out.push(format!("Invoice Date: {}", format_date(s.date, s.layout)));
    out.push(format!("Bill To: {}", s.buyer));
    out.push(String::new());
    out.push("Description | Qty | Rate | Amount".to_string());
    for l in &s.lines {
        out.push(format!(
            "{} | {} | {} {} | {} {}",
            l.description, l.quantity, cur, format_amount(l.rate), cur, format_amount((l.quantity * l.rate).round_dp(2))
        ));
    }
    out.push(String::new());
    out.push(format!("Subtotal: {} {}", cur, format_amount(s.subtotal)));
    out.push(format!("GST @ 18%: {} {}", cur, format_amount(s.tax)));
    out.push(format!("{}: {} {}", TOTAL_LABELS[s.layout], cur, format_amount(s.total)));
    out
}

fn write_pdf(lines: &[String], path: &Path) -> anyhow::Result<()> {
    let mut doc = Document::with_version("1.5");
    // Fixed trailer ID and no timestamps keep output byte-identical across runs.
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let mut ops = vec![Operation::new("BT", vec![]), Operation::new("Tf", vec!["F1".into(), 11.into()]), Operation::new("TL", vec![15.into()]), Operation::new("Td", vec![50.into(), 790.into()])];
    for line in lines {
        ops.push(Operation::new("Tj", vec![Object::string_literal(line.as_str())]));
        ops.push(Operation::new("T*", vec![]));
    }
    ops.push(Operation::new("ET", vec![]));
    let content = Content { operations: ops };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode()?));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! {
        "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        "Resources" => resources_id, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.compress();
    let mut file = std::fs::File::create(path)?;
    doc.save_to(&mut file)?;
    Ok(())
}

fn doc_key(s: &InvoiceSpec) -> String {
    format!("invoices/{}.pdf", s.number.to_lowercase())
}

fn build_dataset(specs: &[InvoiceSpec], seed: u64) -> Dataset {
    let mut cases = Vec::new();
    for s in specs.iter().step_by(5) {
        cases.push(EvalCase {
            id: format!("inv-total-{}", s.number.to_lowercase()),
            question: format!("What is the total amount on invoice {}?", s.number),
            kind: QuestionType::Lookup,
            expected_answer: vec![],
            expected_number: Some(NumberLiteral::from(s.total)),
            expected_docs: vec![doc_key(s)],
            expected_pages: vec![1],
        });
        cases.push(EvalCase {
            id: format!("inv-gstin-{}", s.number.to_lowercase()),
            question: format!("What GSTIN is printed for the supplier on invoice {}?", s.number),
            kind: QuestionType::Lookup,
            expected_answer: vec![s.vendor.gstin.clone()],
            expected_number: None,
            expected_docs: vec![doc_key(s)],
            expected_pages: vec![1],
        });
    }
    let mut by_pair: BTreeMap<(String, String), Vec<&InvoiceSpec>> = BTreeMap::new();
    let mut by_vendor: BTreeMap<String, Vec<&InvoiceSpec>> = BTreeMap::new();
    for s in specs {
        by_vendor.entry(s.vendor.name.clone()).or_default().push(s);
        if in_q3(s.date) {
            by_pair.entry((s.vendor.name.clone(), s.buyer.clone())).or_default().push(s);
        }
    }
    for ((vendor, buyer), invs) in &by_pair {
        let total: Decimal = invs.iter().map(|s| s.total).sum();
        cases.push(EvalCase {
            id: format!("inv-q3-sum-{}--{}", slug(vendor), slug(buyer)),
            question: format!("What is the total amount invoiced by {vendor} to {buyer} between 1 July 2026 and 30 September 2026?"),
            kind: QuestionType::Aggregate,
            expected_answer: vec![],
            expected_number: Some(NumberLiteral::from(total)),
            expected_docs: invs.iter().map(|s| doc_key(s)).collect(),
            expected_pages: vec![],
        });
    }
    for (vendor, invs) in &by_vendor {
        let q3: Vec<&&InvoiceSpec> = invs.iter().filter(|s| in_q3(s.date)).collect();
        if !q3.is_empty() {
            cases.push(EvalCase {
                id: format!("inv-q3-count-{}", slug(vendor)),
                question: format!("How many invoices did {vendor} issue in Q3 2026 (July to September)?"),
                kind: QuestionType::Aggregate,
                expected_answer: vec![],
                expected_number: Some(NumberLiteral::Integer(q3.len() as i64)),
                expected_docs: q3.iter().map(|s| doc_key(s)).collect(),
                expected_pages: vec![],
            });
        }
        let aug: Vec<&&InvoiceSpec> = invs.iter().filter(|s| s.date.year() == 2026 && s.date.month() == 8).collect();
        if !aug.is_empty() {
            cases.push(EvalCase {
                id: format!("inv-aug-list-{}", slug(vendor)),
                question: format!("List the invoice numbers {vendor} issued in August 2026."),
                kind: QuestionType::List,
                expected_answer: aug.iter().map(|s| s.number.clone()).collect(),
                expected_number: None,
                expected_docs: aug.iter().map(|s| doc_key(s)).collect(),
                expected_pages: vec![],
            });
        }
    }
    Dataset {
        id: format!("invoices-seed{seed}-n{}", specs.len()),
        description: "Synthetic invoices with exact ground truth (generated by shodh-eval)".to_string(),
        cases,
    }
}

pub fn generate(out_dir: &Path, seed: u64, count: usize) -> anyhow::Result<Vec<InvoiceSpec>> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let specs = (0..count).map(|i| spec(&mut rng, i)).collect::<anyhow::Result<Vec<_>>>()?;
    let dir = out_dir.join("corpus").join("invoices");
    std::fs::create_dir_all(&dir)?;
    for s in &specs {
        write_pdf(&render_lines(s), &dir.join(format!("{}.pdf", s.number.to_lowercase())))?;
    }
    let dataset = build_dataset(&specs, seed);
    dataset.validate()?;
    dataset.save(&out_dir.join("dataset.yaml"))?;
    Ok(specs)
}
```

Note: PDF file names are lowercased (`inv-2026-0001.pdf`) so document keys and on-disk names agree on case-sensitive filesystems.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p shodh-eval --lib invoices::`
Expected: 3 tests pass.

If `generation_is_byte_for_byte_deterministic` fails, lopdf is embedding a non-deterministic value. Diff the two outputs (`cmp -l`) to find the field, then set it explicitly; for example, set the trailer `ID` to a fixed array derived from the seed. Do not relax the test.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/invoices.rs crates/shodh-rag/src/processing/mod.rs
git commit -m "shodh-eval: add deterministic synthetic invoice corpus"
```

---

### Task 7: Retrieval evaluation on the real ingest and search path

**Files:**
- Create (replace): `crates/shodh-eval/src/retrieval.rs`
- Test: `crates/shodh-eval/tests/retrieval_e2e.rs`

**Interfaces:**
- Consumes:
  - `shodh_rag::indexing::{index_folder, IndexingOptions, IndexingState}`
  - `shodh_rag::{RAGConfig, RAGEngine}`
  - `shodh_rag::rag::eval::{evaluate, EvalQuery, EvalResult}`
  - `corpus::*`, `dataset::*`, `report::*`
- Produces:
  - `retrieval::{RetrievalSettings, LEGACY_PIPELINE, SUPPORTED_EXTENSIONS, EVAL_SPACE_ID}`
  - `build_index(corpus_root: &Path, data_dir: &Path, models_dir: &Path) -> anyhow::Result<(RAGEngine, IngestSummary)>`
  - `dedupe_ranked(keys: Vec<String>) -> Vec<String>`
  - `evaluate_retrieval(rag: &RAGEngine, corpus_root: &Path, dataset: &Dataset, settings: &RetrievalSettings) -> anyhow::Result<RunReport>`
  - `run(corpus_root, dataset, models_dir, settings, repeats) -> anyhow::Result<Vec<RunReport>>`

Each repeat re-indexes into a fresh temp directory. Measured noise therefore includes any indexing nondeterminism, not only query-time noise.

- [ ] **Step 1: Write failing unit tests (dedupe) and the e2e test**

Append to `crates/shodh-eval/src/retrieval.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_keeps_first_rank_per_document() {
        let ranked = vec!["a.pdf", "a.pdf", "b.pdf", "a.pdf", "c.pdf", "b.pdf"].into_iter().map(String::from).collect();
        assert_eq!(dedupe_ranked(ranked), vec!["a.pdf", "b.pdf", "c.pdf"]);
    }
}
```

`crates/shodh-eval/tests/retrieval_e2e.rs`:
```rust
//! End-to-end: generated invoices -> real index_folder -> real RAGEngine::search.
//! Requires models: set SHODH_EVAL_MODELS to a directory populated by
//! `shodh-eval fetch-models --dir <dir>` (CI does this before running tests).

use std::path::PathBuf;

use shodh_eval::dataset::Dataset;
use shodh_eval::retrieval::{run, RetrievalSettings};

fn models_dir() -> PathBuf {
    let dir = std::env::var("SHODH_EVAL_MODELS")
        .expect("SHODH_EVAL_MODELS must point to a models dir (run `shodh-eval fetch-models`)");
    PathBuf::from(dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn invoices_retrieval_report_is_complete() {
    let work = tempfile::tempdir().unwrap();
    shodh_eval::invoices::generate(work.path(), 99, 20).unwrap();
    let dataset = Dataset::load(&work.path().join("dataset.yaml")).unwrap();
    let corpus = work.path().join("corpus");
    let reports = run(&corpus, &dataset, &models_dir(), &RetrievalSettings::default(), 1).await.unwrap();
    let r = &reports[0];

    let ingest = r.ingest.as_ref().unwrap();
    assert_eq!(ingest.files_seen, 20);
    assert_eq!(ingest.files_indexed + ingest.files_failed, 20);
    assert!(ingest.chunks >= 20);
    assert_eq!(r.metrics["unmapped_sources"], 0.0, "engine sources must map back to corpus keys");
    for k in ["recall@1", "recall@3", "recall@8", "recall@20", "mrr", "ndcg@8", "hit_rate@8"] {
        assert!(r.metrics.contains_key(k), "missing {k}");
    }
    // Lookup questions name the invoice number; the right PDF must be reachable.
    assert!(r.metrics["hit_rate@20"] > 0.0);
    assert_eq!(r.latency.count, r.cases.len());
    assert_eq!(r.dataset_hash, dataset.content_hash());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shodh-eval --lib retrieval:: && SHODH_EVAL_MODELS=<models> cargo test -p shodh-eval --test retrieval_e2e`
Expected: compile errors.

- [ ] **Step 3: Implement `retrieval.rs`**

```rust
//! Retrieval evaluation against the current shodh pipeline:
//! ingest with `indexing::index_folder` (the app's code path), search with
//! `RAGEngine::search`, score document-level ranks with `rag::eval`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Instant;

use anyhow::{anyhow, bail, Context};
use shodh_rag::indexing::{index_folder, IndexingOptions, IndexingState};
use shodh_rag::rag::eval::{evaluate, EvalQuery, EvalResult};
use shodh_rag::{RAGConfig, RAGEngine};

use crate::corpus::{doc_key_from_source, list_corpus_files};
use crate::dataset::{Dataset, QuestionType};
use crate::report::{summarize_latencies, CaseRecord, IngestSummary, RunKind, RunReport};

pub const LEGACY_PIPELINE: &str = "legacy-index_folder+ragengine-search";
pub const EVAL_SPACE_ID: &str = "shodh-eval";
pub const SUPPORTED_EXTENSIONS: &[&str] = &["pdf", "txt", "md", "docx", "xlsx", "csv", "pptx", "html"];

#[derive(Debug, Clone)]
pub struct RetrievalSettings {
    /// Chunks requested from the engine per query (app default for focused queries).
    pub search_k: usize,
    pub k_values: Vec<usize>,
}

impl Default for RetrievalSettings {
    fn default() -> Self {
        Self { search_k: 20, k_values: vec![1, 3, 8, 20] }
    }
}

pub fn dedupe_ranked(keys: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    keys.into_iter().filter(|k| seen.insert(k.clone())).collect()
}

pub async fn build_index(corpus_root: &Path, data_dir: &Path, models_dir: &Path) -> anyhow::Result<(RAGEngine, IngestSummary)> {
    if !models_dir.join("multilingual-e5-base").join("model_O4.onnx").exists() {
        bail!("E5 model missing under {}; run `shodh-eval fetch-models --dir {}`", models_dir.display(), models_dir.display());
    }
    let mut config = RAGConfig::default();
    config.data_dir = data_dir.to_path_buf();
    config.embedding.model_dir = models_dir.to_path_buf();
    config.embedding.use_e5 = true;
    config.embedding.dimension = 768;
    let mut rag = RAGEngine::new(config).await.context("initialising RAGEngine")?;

    let files_seen = list_corpus_files(corpus_root, SUPPORTED_EXTENSIONS)?.len();
    let options = IndexingOptions {
        skip_indexed: false,
        watch_changes: false,
        process_subdirs: true,
        priority: "normal".to_string(),
        file_types: SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
    };
    let state = IndexingState::default();
    let root = corpus_root.to_str().ok_or_else(|| anyhow!("corpus path is not UTF-8"))?;
    let started = Instant::now();
    let result = index_folder(root, EVAL_SPACE_ID, &options, &mut rag, &state, None)
        .await
        .map_err(|e| anyhow!("index_folder failed: {e}"))?;
    let summary = IngestSummary {
        files_seen,
        files_indexed: result.files_processed,
        files_failed: result.failed_files.len(),
        failed_files: result.failed_files,
        chunks: result.total_chunks,
        seconds: started.elapsed().as_secs_f64(),
    };
    Ok((rag, summary))
}

pub async fn evaluate_retrieval(rag: &RAGEngine, corpus_root: &Path, dataset: &Dataset, settings: &RetrievalSettings) -> anyhow::Result<RunReport> {
    let scored: Vec<_> = dataset.cases.iter().filter(|c| c.kind != QuestionType::None && !c.expected_docs.is_empty()).collect();
    let mut ranked_per_case = Vec::with_capacity(scored.len());
    let mut latencies = Vec::with_capacity(scored.len());
    let mut unmapped = 0usize;
    for case in &scored {
        let t = Instant::now();
        let results = rag.search(&case.question, settings.search_k).await.with_context(|| format!("search for case {}", case.id))?;
        latencies.push(t.elapsed().as_secs_f64() * 1000.0);
        let keys: Vec<String> = results
            .iter()
            .map(|r| {
                let raw = r.metadata.get("file_path").cloned().unwrap_or_else(|| r.source.clone());
                doc_key_from_source(corpus_root, &raw).unwrap_or_else(|| {
                    unmapped += 1;
                    format!("<unmapped>:{raw}")
                })
            })
            .collect();
        ranked_per_case.push(dedupe_ranked(keys));
    }

    let queries: Vec<EvalQuery> = scored
        .iter()
        .map(|c| EvalQuery { query: c.question.clone(), relevant_ids: c.expected_docs.iter().cloned().collect(), graded_relevance: Default::default() })
        .collect();
    let mut feed = ranked_per_case.clone().into_iter();
    let m = evaluate(&queries, &settings.k_values, |_| {
        feed.next().unwrap_or_default().into_iter().map(|id| EvalResult { id, score: 0.0 }).collect()
    });

    let mut metrics = BTreeMap::new();
    metrics.insert("mrr".to_string(), m.mrr);
    for &k in &settings.k_values {
        metrics.insert(format!("recall@{k}"), m.recall_at.get(&k).copied().unwrap_or(0.0));
        metrics.insert(format!("ndcg@{k}"), m.ndcg_at.get(&k).copied().unwrap_or(0.0));
        metrics.insert(format!("hit_rate@{k}"), m.hit_rate_at.get(&k).copied().unwrap_or(0.0));
    }
    metrics.insert("unmapped_sources".to_string(), unmapped as f64);

    let cases = scored
        .iter()
        .zip(m.per_query.iter())
        .zip(ranked_per_case)
        .map(|((c, q), ranked)| CaseRecord {
            id: c.id.clone(),
            question: c.question.clone(),
            metrics: q.recall_at_k.iter().map(|(k, v)| (format!("recall@{k}"), *v)).chain([("rr".to_string(), q.reciprocal_rank)]).collect(),
            retrieved_docs: ranked,
            answer: None,
            error: None,
        })
        .collect();

    let mut settings_map = BTreeMap::new();
    settings_map.insert("search_k".to_string(), settings.search_k.to_string());
    settings_map.insert("k_values".to_string(), format!("{:?}", settings.k_values));

    Ok(RunReport {
        kind: RunKind::Retrieval,
        dataset_id: dataset.id.clone(),
        dataset_hash: dataset.content_hash(),
        pipeline: LEGACY_PIPELINE.to_string(),
        git_sha: std::env::var("GITHUB_SHA").ok(),
        created_at: chrono::Utc::now(),
        settings: settings_map,
        metrics,
        latency: summarize_latencies(&latencies),
        ingest: None,
        cases,
    })
}

/// Index the corpus fresh and evaluate, `repeats` times.
pub async fn run(corpus_root: &Path, dataset: &Dataset, models_dir: &Path, settings: &RetrievalSettings, repeats: usize) -> anyhow::Result<Vec<RunReport>> {
    if repeats == 0 {
        bail!("repeats must be at least 1");
    }
    let mut reports = Vec::with_capacity(repeats);
    for i in 0..repeats {
        let data_dir = tempfile::tempdir()?;
        let (rag, ingest) = build_index(corpus_root, data_dir.path(), models_dir).await?;
        tracing::info!(repeat = i + 1, files = ingest.files_indexed, failed = ingest.files_failed, "indexed");
        let mut report = evaluate_retrieval(&rag, corpus_root, dataset, settings).await?;
        report.ingest = Some(ingest);
        reports.push(report);
    }
    Ok(reports)
}
```

Note: confirm that `IndexingOptions` and `IndexingState` are public with the field names used (`crates/shodh-rag/src/indexing.rs:52-80`, verified 2026-10-02), and that `index_folder`'s emitter parameter accepts `None`.

- [ ] **Step 4: Run to verify pass**

Run:
- `cargo test -p shodh-eval --lib retrieval::`
- then `SHODH_EVAL_MODELS=<models> cargo test -p shodh-eval --test retrieval_e2e -- --nocapture`

Expected: both pass.

If `unmapped_sources` is non-zero, print one raw source and fix `doc_key_from_source`. Never mask it by filtering.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/retrieval.rs crates/shodh-eval/tests/retrieval_e2e.rs
git commit -m "shodh-eval: add retrieval evaluation on the real ingest and search path"
```

---

### Task 8: Answer evaluation through the headless ChatEngine

**Files:**
- Create (replace): `crates/shodh-eval/src/answers.rs`
- Create: `crates/shodh-eval/tests/support/mod.rs`, `crates/shodh-eval/tests/answers_e2e.rs`

**Interfaces:**
- Consumes:
  - `shodh_rag::chat::engine::ChatEngine`
  - `shodh_rag::chat::{ChatContext, UserMessage, MessagePlatform}`
  - `shodh_rag::agent::PersonalAssistant`
  - `shodh_rag::memory::{MemorySystem, MemoryConfig}`
  - `shodh_rag::{LLMManager, LLMConfig, LLMMode, ApiProvider}`
  - `retrieval::build_index`, `scoring::score_answer`, `report::*`
- Produces:
  - `answers::{LlmEndpoint, evaluate_answers(corpus_root, dataset, models_dir, &LlmEndpoint) -> anyhow::Result<RunReport>}`

**Isolation:** every case gets a **fresh** `MemorySystem` and `ChatEngine`, so memories from earlier questions cannot leak into later answers. The RAG index is shared. Temperature is 0, and streaming is off.

- [ ] **Step 1: Write the scripted fixture and the failing e2e test**

`crates/shodh-eval/tests/support/mod.rs`:
```rust
//! Test-only scripted OpenAI-compatible chat server. Never compiled into src/.
//! Answers every request with `content`, as JSON or as SSE when `"stream":true`.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub async fn start(content: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let (header_end, content_len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 { return; }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                        break (pos + 4, len);
                    }
                };
                while buf.len() < header_end + content_len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 { break; }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
                let streaming = body.replace(' ', "").contains("\"stream\":true");
                let escaped = serde_json::to_string(content).unwrap();
                let (ctype, payload) = if streaming {
                    ("text/event-stream", format!(
                        "data: {{\"id\":\"f\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{escaped}}},\"finish_reason\":null}}]}}\n\ndata: [DONE]\n\n"
                    ))
                } else {
                    ("application/json", format!(
                        "{{\"id\":\"f\",\"object\":\"chat.completion\",\"created\":0,\"model\":\"fixture\",\"choices\":[{{\"index\":0,\"message\":{{\"role\":\"assistant\",\"content\":{escaped}}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}}}"
                    ))
                };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len());
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/v1/chat/completions")
}
```

`crates/shodh-eval/tests/answers_e2e.rs`:
```rust
mod support;

use std::path::PathBuf;

use shodh_eval::answers::{evaluate_answers, LlmEndpoint};
use shodh_eval::dataset::Dataset;

fn models_dir() -> PathBuf {
    PathBuf::from(std::env::var("SHODH_EVAL_MODELS").expect("SHODH_EVAL_MODELS must point to a models dir"))
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_every_case_through_chat_engine_and_scores_it() {
    let endpoint = support::start("I couldn't find this in your documents.").await;
    let work = tempfile::tempdir().unwrap();
    // 40 invoices: P(no Q3 invoice) = 0.75^40 < 1e-4, so aggregate cases exist.
    shodh_eval::invoices::generate(work.path(), 5, 40).unwrap();
    let dataset = Dataset::load(&work.path().join("dataset.yaml")).unwrap();
    let llm = LlmEndpoint { url: endpoint, model: "fixture".into(), api_key: None };
    let report = evaluate_answers(&work.path().join("corpus"), &dataset, &models_dir(), &llm).await.unwrap();

    assert_eq!(report.cases.len(), dataset.cases.len());
    assert!(report.cases.iter().all(|c| c.error.is_none()), "{:?}", report.cases.iter().find(|c| c.error.is_some()));
    assert!(report.cases.iter().all(|c| c.answer.as_deref() == Some("I couldn't find this in your documents.")));
    // A refusal is wrong for every answerable invoice question.
    assert_eq!(report.metrics["correct_rate"], 0.0);
    assert!(report.metrics.contains_key("aggregate_exact"));
    assert!(report.metrics.contains_key("citation_hit_rate"));
    assert_eq!(report.settings["llm_model"], "fixture");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `SHODH_EVAL_MODELS=<models> cargo test -p shodh-eval --test answers_e2e`
Expected: compile error (`evaluate_answers` not found).

- [ ] **Step 3: Implement `answers.rs`**

```rust
//! Answer evaluation through today's chat pipeline (ChatEngine::process_message),
//! against any OpenAI-compatible chat endpoint (Ollama, vLLM, LM Studio, cloud).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use shodh_rag::agent::PersonalAssistant;
use shodh_rag::chat::engine::ChatEngine;
use shodh_rag::chat::{ChatContext, MessagePlatform, UserMessage};
use shodh_rag::memory::{MemoryConfig, MemorySystem};
use shodh_rag::{ApiProvider, LLMConfig, LLMManager, LLMMode};
use tokio::sync::RwLock;

use crate::corpus::doc_key_from_source;
use crate::dataset::{Dataset, QuestionType};
use crate::report::{summarize_latencies, CaseRecord, RunKind, RunReport};
use crate::retrieval::build_index;
use crate::scoring::score_answer;

pub const LEGACY_CHAT_PIPELINE: &str = "legacy-chatengine-process_message";

#[derive(Debug, Clone)]
pub struct LlmEndpoint {
    /// Full chat-completions URL, e.g. http://localhost:11434/v1/chat/completions
    pub url: String,
    pub model: String,
    pub api_key: Option<String>,
}

async fn llm_manager(endpoint: &LlmEndpoint) -> anyhow::Result<LLMManager> {
    let config = LLMConfig {
        mode: LLMMode::External {
            provider: ApiProvider::Custom { endpoint: endpoint.url.clone() },
            api_key: endpoint.api_key.clone().unwrap_or_default(),
            model: endpoint.model.clone(),
        },
        temperature: 0.0,
        streaming: false,
        ..LLMConfig::default()
    };
    let mut manager = LLMManager::new(config);
    manager.initialize().await.context("initialising LLM endpoint")?;
    Ok(manager)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() { 0.0 } else { values.iter().sum::<f64>() / values.len() as f64 }
}

pub async fn evaluate_answers(corpus_root: &Path, dataset: &Dataset, models_dir: &Path, endpoint: &LlmEndpoint) -> anyhow::Result<RunReport> {
    let data_dir = tempfile::tempdir()?;
    let (rag, ingest) = build_index(corpus_root, data_dir.path(), models_dir).await?;
    let rag = Arc::new(RwLock::new(rag));
    let llm = Arc::new(RwLock::new(Some(llm_manager(endpoint).await?)));

    let mut cases = Vec::with_capacity(dataset.cases.len());
    let mut latencies = Vec::new();
    let (mut scores, mut correct, mut cite_hits) = (Vec::new(), Vec::new(), Vec::new());
    let mut by_kind: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();

    for case in &dataset.cases {
        let memory_dir = tempfile::tempdir()?;
        let memory = Arc::new(RwLock::new(MemorySystem::new(MemoryConfig { storage_path: memory_dir.path().to_path_buf(), ..MemoryConfig::default() })?));
        let assistant = Arc::new(RwLock::new(PersonalAssistant::new(memory.clone()).await?));
        let engine = ChatEngine::new(rag.clone(), Arc::new(RwLock::new(None)), assistant, Some(llm.clone()), memory).await;
        let message = UserMessage { content: case.question.clone(), images: None, platform: MessagePlatform::Desktop, timestamp: chrono::Utc::now() };
        let context = ChatContext { streaming: Some(false), ..ChatContext::default() };

        let started = Instant::now();
        let outcome = engine.process_message(message, context, None).await;
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);

        let record = match outcome {
            Ok(resp) => {
                let cited: Vec<String> = resp.citations.iter().filter_map(|c| doc_key_from_source(corpus_root, &c.source)).collect();
                let s = score_answer(case, &resp.content, &cited).map_err(anyhow::Error::msg)?;
                scores.push(s.score);
                correct.push(if s.correct { 1.0 } else { 0.0 });
                if let Some(hit) = s.citation_hit {
                    cite_hits.push(if hit { 1.0 } else { 0.0 });
                }
                let kind = match case.kind {
                    QuestionType::Lookup => "lookup_score",
                    QuestionType::List => "list_recall",
                    QuestionType::Aggregate => "aggregate_exact",
                    QuestionType::Compare => "compare_score",
                    QuestionType::None => "none_refusal",
                };
                by_kind.entry(kind).or_default().push(s.score);
                let mut m = BTreeMap::new();
                m.insert("score".to_string(), s.score);
                m.insert("correct".to_string(), if s.correct { 1.0 } else { 0.0 });
                CaseRecord { id: case.id.clone(), question: case.question.clone(), metrics: m, retrieved_docs: cited, answer: Some(resp.content), error: None }
            }
            Err(e) => {
                scores.push(0.0);
                correct.push(0.0);
                CaseRecord { id: case.id.clone(), question: case.question.clone(), metrics: BTreeMap::new(), retrieved_docs: vec![], answer: None, error: Some(format!("{e:#}")) }
            }
        };
        cases.push(record);
    }

    let mut metrics = BTreeMap::new();
    metrics.insert("score_mean".to_string(), mean(&scores));
    metrics.insert("correct_rate".to_string(), mean(&correct));
    metrics.insert("citation_hit_rate".to_string(), mean(&cite_hits));
    metrics.insert("error_free_rate".to_string(), cases.iter().filter(|c| c.error.is_none()).count() as f64 / cases.len().max(1) as f64);
    for (k, v) in &by_kind {
        metrics.insert((*k).to_string(), mean(v));
    }

    let mut settings = BTreeMap::new();
    settings.insert("llm_url".to_string(), endpoint.url.clone());
    settings.insert("llm_model".to_string(), endpoint.model.clone());
    settings.insert("temperature".to_string(), "0".to_string());

    Ok(RunReport {
        kind: RunKind::Answers,
        dataset_id: dataset.id.clone(),
        dataset_hash: dataset.content_hash(),
        pipeline: LEGACY_CHAT_PIPELINE.to_string(),
        git_sha: std::env::var("GITHUB_SHA").ok(),
        created_at: chrono::Utc::now(),
        settings,
        metrics,
        latency: summarize_latencies(&latencies),
        ingest: Some(ingest),
        cases,
    })
}
```

Before running, verify these names against the source (all were seen 2026-10-02):
- `shodh_rag::agent::PersonalAssistant` (re-exported at `agent/mod.rs:76`)
- `shodh_rag::memory::{MemorySystem, MemoryConfig}` (`memory/mod.rs:20,52`; `MemorySystem::new` is sync and returns `Result`)
- `PersonalAssistant::new(Arc<tokio::sync::RwLock<MemorySystem>>)` (async)
- `ChatEngine::new(...)` with five arguments (`chat/engine.rs:42`)
- `ChatContext` derives `Default`

If `chat::Citation.source` does not carry the file path, map citations through `resp.search_results[].source_file` instead, and record which field was used in a code comment.

- [ ] **Step 4: Run to verify pass**

Run: `SHODH_EVAL_MODELS=<models> cargo test -p shodh-eval --test answers_e2e -- --nocapture`
Expected: PASS.

If `error_free_rate` < 1, read `cases[].error`. The engine's router falls back on LLM errors (`chat/engine.rs:212`), so an error here is a real defect: report it, don't suppress it.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/answers.rs crates/shodh-eval/tests/support crates/shodh-eval/tests/answers_e2e.rs
git commit -m "shodh-eval: add answer evaluation through headless ChatEngine"
```

---

### Task 9: CLI

**Files:**
- Create: `crates/shodh-eval/src/main.rs`, `crates/shodh-eval/tests/cli.rs`

**Interfaces:**
- Consumes: every module above.
- Produces these subcommands:

| Subcommand | Arguments |
|---|---|
| `fetch-models` | `--dir` |
| `fetch-cuad` | `--dir` |
| `cuad` | `--zip --out --profile pr\|full --seed` |
| `gen-invoices` | `--out --seed --count` |
| `retrieval` | `--corpus --dataset --models --out-dir --repeat --search-k` |
| `answers` | `--corpus --dataset --models --out --llm-url --llm-model --llm-api-key-env` |
| `baseline` | `--runs <files...> --out` |
| `compare` | `--baseline --run`; exits 1 on Fail |

- [ ] **Step 1: Write the failing CLI tests**

`crates/shodh-eval/tests/cli.rs`:
```rust
use assert_cmd::Command;

#[test]
fn gen_invoices_writes_dataset_and_corpus() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("shodh-eval").unwrap()
        .args(["gen-invoices", "--out"]).arg(dir.path()).args(["--seed", "1", "--count", "5"])
        .assert().success();
    assert!(dir.path().join("dataset.yaml").exists());
    assert_eq!(std::fs::read_dir(dir.path().join("corpus/invoices")).unwrap().count(), 5);
}

#[test]
fn compare_exits_nonzero_on_regression() {
    use shodh_eval::report::*;
    let dir = tempfile::tempdir().unwrap();
    let mk = |v: f64| RunReport {
        kind: RunKind::Retrieval, dataset_id: "d".into(), dataset_hash: "h".into(), pipeline: "p".into(),
        git_sha: None, created_at: chrono::Utc::now(), settings: Default::default(),
        metrics: [("recall@8".to_string(), v)].into_iter().collect(),
        latency: summarize_latencies(&[1.0]), ingest: None, cases: vec![],
    };
    mk(0.6).write(&dir.path().join("base-run.json")).unwrap();
    mk(0.5).write(&dir.path().join("worse.json")).unwrap();
    Command::cargo_bin("shodh-eval").unwrap()
        .args(["baseline", "--runs"]).arg(dir.path().join("base-run.json"))
        .arg("--out").arg(dir.path().join("baseline.json")).assert().success();
    Command::cargo_bin("shodh-eval").unwrap()
        .args(["compare", "--baseline"]).arg(dir.path().join("baseline.json"))
        .arg("--run").arg(dir.path().join("worse.json"))
        .assert().failure().stdout(predicates::str::contains("recall@8"));
}
```

Add `predicates = "3"` to `[dev-dependencies]`. Add `chrono` to `[dev-dependencies]` only if the integration test does not resolve it through `[dependencies]`; it does, so no change is needed.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shodh-eval --test cli`
Expected: FAIL (binary has no subcommands).

- [ ] **Step 3: Implement `main.rs`**

```rust
//! shodh-eval CLI. Thin wiring over the library; all logic lives in modules.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use shodh_eval::answers::{evaluate_answers, LlmEndpoint};
use shodh_eval::cuad::{self, CuadProfile};
use shodh_eval::dataset::Dataset;
use shodh_eval::report::{compare, Baseline, CompareOutcome, RunReport};
use shodh_eval::retrieval::{self, RetrievalSettings};
use shodh_eval::{fetch, invoices};

#[derive(Parser)]
#[command(name = "shodh-eval", about = "Measure shodh retrieval and answer quality")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Profile { Pr, Full }

#[derive(Subcommand)]
enum Command {
    /// Download pinned, checksum-verified E5 and reranker models.
    FetchModels { #[arg(long)] dir: PathBuf },
    /// Download the pinned CUAD v1 zip.
    FetchCuad { #[arg(long)] dir: PathBuf },
    /// Convert CUAD into a corpus + dataset.
    Cuad {
        #[arg(long)] zip: PathBuf,
        #[arg(long)] out: PathBuf,
        #[arg(long, value_enum, default_value = "pr")] profile: Profile,
        #[arg(long, default_value_t = 20261002)] seed: u64,
    },
    /// Generate the synthetic invoice corpus + dataset.
    GenInvoices {
        #[arg(long)] out: PathBuf,
        #[arg(long, default_value_t = 20261002)] seed: u64,
        #[arg(long, default_value_t = 120)] count: usize,
    },
    /// Index a corpus with today's pipeline and score retrieval.
    Retrieval {
        #[arg(long)] corpus: PathBuf,
        #[arg(long)] dataset: PathBuf,
        #[arg(long)] models: PathBuf,
        #[arg(long)] out_dir: PathBuf,
        #[arg(long, default_value_t = 1)] repeat: usize,
        #[arg(long, default_value_t = 20)] search_k: usize,
    },
    /// Answer every question through today's chat pipeline and score answers.
    Answers {
        #[arg(long)] corpus: PathBuf,
        #[arg(long)] dataset: PathBuf,
        #[arg(long)] models: PathBuf,
        #[arg(long)] out: PathBuf,
        #[arg(long)] llm_url: String,
        #[arg(long)] llm_model: String,
        /// Name of the environment variable holding the API key (keys never go on the command line).
        #[arg(long)] llm_api_key_env: Option<String>,
    },
    /// Build a baseline (mean, stddev) from repeated run reports.
    Baseline { #[arg(long, num_args = 1..)] runs: Vec<PathBuf>, #[arg(long)] out: PathBuf },
    /// Gate a run against a baseline; exits 1 on regression.
    Compare { #[arg(long)] baseline: PathBuf, #[arg(long)] run: PathBuf },
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).init();
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    match cli.command {
        Command::FetchModels { dir } => fetch::fetch_models(&dir).await?,
        Command::FetchCuad { dir } => {
            let p = fetch::fetch_pinned(&fetch::CUAD_ZIP, &dir).await?;
            println!("{}", p.display());
        }
        Command::Cuad { zip, out, profile, seed } => {
            let profile = match profile { Profile::Pr => CuadProfile::Pr, Profile::Full => CuadProfile::Full };
            let s = cuad::convert(&zip, &out, profile, seed)?;
            println!("{s:?}");
        }
        Command::GenInvoices { out, seed, count } => {
            let specs = invoices::generate(&out, seed, count)?;
            println!("generated {} invoices into {}", specs.len(), out.display());
        }
        Command::Retrieval { corpus, dataset, models, out_dir, repeat, search_k } => {
            let ds = Dataset::load(&dataset)?;
            let settings = RetrievalSettings { search_k, ..RetrievalSettings::default() };
            std::fs::create_dir_all(&out_dir)?;
            for (i, report) in retrieval::run(&corpus, &ds, &models, &settings, repeat).await?.iter().enumerate() {
                let path = out_dir.join(format!("{}-run{}.json", ds.id, i + 1));
                report.write(&path)?;
                println!("{} {:?}", path.display(), report.metrics);
            }
        }
        Command::Answers { corpus, dataset, models, out, llm_url, llm_model, llm_api_key_env } => {
            let api_key = match llm_api_key_env {
                Some(var) => Some(std::env::var(&var).with_context(|| format!("environment variable {var} not set"))?),
                None => None,
            };
            let ds = Dataset::load(&dataset)?;
            let endpoint = LlmEndpoint { url: llm_url, model: llm_model, api_key };
            let report = evaluate_answers(&corpus, &ds, &models, &endpoint).await?;
            report.write(&out)?;
            println!("{} {:?}", out.display(), report.metrics);
        }
        Command::Baseline { runs, out } => {
            let reports = runs.iter().map(|p| RunReport::read(p)).collect::<anyhow::Result<Vec<_>>>()?;
            let b = Baseline::from_runs(&reports).map_err(anyhow::Error::msg)?;
            b.write(&out)?;
            println!("{}", serde_json::to_string_pretty(&b.metrics)?);
        }
        Command::Compare { baseline, run } => {
            let b = Baseline::read(&baseline)?;
            let r = RunReport::read(&run)?;
            match compare(&b, &r) {
                CompareOutcome::Pass { improvements } => {
                    println!("PASS");
                    for (m, before, after) in improvements {
                        println!("  improved {m}: {before:.4} -> {after:.4}");
                    }
                }
                CompareOutcome::Fail { regressions, missing, dataset_mismatch } => {
                    println!("FAIL");
                    if let Some(msg) = dataset_mismatch { println!("  {msg}"); }
                    for m in missing { println!("  missing metric {m}"); }
                    for g in regressions {
                        println!("  {}: {:.4} < allowed {:.4} (baseline mean {:.4})", g.metric, g.current, g.allowed_min, g.baseline_mean);
                    }
                    return Ok(ExitCode::from(1));
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p shodh-eval --test cli`
Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/shodh-eval/src/main.rs crates/shodh-eval/tests/cli.rs crates/shodh-eval/Cargo.toml
git commit -m "shodh-eval: add CLI"
```

---

### Task 10: CI gate, baseline recording, and documentation

**Files:**
- Modify: `.github/workflows/ci.yml` (add `eval-retrieval` job)
- Create: `.github/workflows/eval-baseline.yml`, `eval/README.md`, `eval/baselines/.gitkeep`
- After the baseline workflow runs: commit `eval/baselines/cuad-pr-seed20261002.json` and `eval/baselines/invoices-seed20261002-n120.json`

**Prerequisite:** sub-project 0 has merged a CI where `cargo test` actually executes. If it has not, stop after Step 3 and report that the gate cannot be enabled yet.

- [ ] **Step 1: Add the PR gate job to `ci.yml`**

```yaml
  eval-retrieval:
    name: Eval (retrieval gate)
    runs-on: windows-latest
    needs: check
    env:
      SHODH_EVAL_MODELS: ${{ github.workspace }}\.eval-cache\models
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install Protobuf compiler
        run: choco install protoc -y
      - name: Cache models and CUAD
        uses: actions/cache@v4
        with:
          path: .eval-cache
          key: eval-cache-v1-e5-d1287505-msmarco-233902d2-cuad-c38f490a
      - name: Build shodh-eval
        run: cargo build --release -p shodh-eval
      - name: Unit and e2e tests
        run: cargo test -p shodh-eval
      - name: Fetch pinned artifacts
        run: |
          target\release\shodh-eval fetch-models --dir .eval-cache\models
          target\release\shodh-eval fetch-cuad --dir .eval-cache
      - name: Build datasets
        run: |
          target\release\shodh-eval cuad --zip .eval-cache\CUAD_v1.zip --out eval-work\cuad --profile pr --seed 20261002
          target\release\shodh-eval gen-invoices --out eval-work\invoices --seed 20261002 --count 120
      - name: Retrieval runs
        run: |
          target\release\shodh-eval retrieval --corpus eval-work\cuad\corpus --dataset eval-work\cuad\dataset.yaml --models .eval-cache\models --out-dir eval-out
          target\release\shodh-eval retrieval --corpus eval-work\invoices\corpus --dataset eval-work\invoices\dataset.yaml --models .eval-cache\models --out-dir eval-out
      - name: Gate against committed baselines
        run: |
          target\release\shodh-eval compare --baseline eval\baselines\cuad-pr-seed20261002.json --run eval-out\cuad-pr-seed20261002-run1.json
          target\release\shodh-eval compare --baseline eval\baselines\invoices-seed20261002-n120.json --run eval-out\invoices-seed20261002-n120-run1.json
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: eval-reports
          path: eval-out
```

Add `continue-on-error` nowhere.

- [ ] **Step 2: Add the manual baseline workflow `.github/workflows/eval-baseline.yml`**

```yaml
name: Eval baseline (manual)

on:
  workflow_dispatch:
    inputs:
      repeat:
        description: "Repeated runs used to measure noise (>= 3)"
        default: "5"

jobs:
  record:
    runs-on: windows-latest
    env:
      SHODH_EVAL_MODELS: ${{ github.workspace }}\.eval-cache\models
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: choco install protoc -y
      - uses: actions/cache@v4
        with:
          path: .eval-cache
          key: eval-cache-v1-e5-d1287505-msmarco-233902d2-cuad-c38f490a
      - run: cargo build --release -p shodh-eval
      - run: |
          target\release\shodh-eval fetch-models --dir .eval-cache\models
          target\release\shodh-eval fetch-cuad --dir .eval-cache
          target\release\shodh-eval cuad --zip .eval-cache\CUAD_v1.zip --out eval-work\cuad --profile pr --seed 20261002
          target\release\shodh-eval gen-invoices --out eval-work\invoices --seed 20261002 --count 120
      - shell: pwsh
        run: |
          $n = [int]"${{ github.event.inputs.repeat }}"
          if ($n -lt 3) { throw "repeat must be >= 3 to measure noise" }
          target\release\shodh-eval retrieval --corpus eval-work\cuad\corpus --dataset eval-work\cuad\dataset.yaml --models .eval-cache\models --out-dir eval-runs --repeat $n
          target\release\shodh-eval retrieval --corpus eval-work\invoices\corpus --dataset eval-work\invoices\dataset.yaml --models .eval-cache\models --out-dir eval-runs --repeat $n
          New-Item -ItemType Directory -Force eval-baselines | Out-Null
          target\release\shodh-eval baseline --runs (Get-ChildItem eval-runs\cuad-pr-seed20261002-run*.json).FullName --out eval-baselines\cuad-pr-seed20261002.json
          target\release\shodh-eval baseline --runs (Get-ChildItem eval-runs\invoices-seed20261002-n120-run*.json).FullName --out eval-baselines\invoices-seed20261002-n120.json
      - uses: actions/upload-artifact@v4
        with:
          name: eval-baselines
          path: |
            eval-baselines
            eval-runs
```

- [ ] **Step 3: Write `eval/README.md`**

```markdown
# shodh-eval

Measures shodh's retrieval and answer quality. Every number is reproducible from a pinned dataset, a seed, and pinned models.

## Datasets
- **CUAD v1** — 510 commercial contracts with expert clause annotations.
  Hendrycks, Burns, Chen, Ball. *CUAD: An Expert-Annotated NLP Dataset for Legal Contract Review*, NeurIPS 2021.
  Licence: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). Downloaded from Zenodo record 4595826 and verified by md5.
  Questions name the contract (from its "Document Name" and "Parties" annotations) and ask for one clause.
- **Synthetic invoices** — generated by `shodh-eval gen-invoices` from a seed, with exact totals.
  The same seed always produces byte-identical PDFs.

## Metrics
- Retrieval (document level):
  - recall@k, nDCG@k and hit_rate@k for k ∈ {1, 3, 8, 20}
  - MRR
  - `unmapped_sources`, which must be 0
- Answers:
  - `score_mean`, `correct_rate`, `citation_hit_rate`, `error_free_rate`
  - per question type: `lookup_score`, `list_recall`, `aggregate_exact`, `compare_score`, `none_refusal`
  - Numbers are compared at 2 decimal places. `none_refusal` uses a phrase heuristic.
- Latency (p50/p95/max) is reported but never gated on shared CI runners.

## Gate
The `eval-retrieval` CI job fails when any metric drops below `baseline mean − 3σ`. σ is measured from repeated runs recorded by the manual **Eval baseline** workflow. To change a baseline, re-run that workflow and commit its artifact in a PR that explains why. Never edit numbers by hand.

## Your own folders (local only)
1. Write `my_questions.yaml`:
   ```yaml
   id: my-contracts
   description: Legal folder, Oct 2026
   cases:
     - id: acme-notice
       question: What is the notice period to terminate the Acme MSA?
       type: lookup            # lookup | list | aggregate | compare | none
       expected_answer: ["thirty (30) days", "30 days"]
       expected_docs: ["Acme/Acme_MSA_2024.pdf"]   # relative to the folder
       expected_pages: [4]
     - id: q3-total
       question: What did we pay Acme in Q3 2026?
       type: aggregate
       expected_number: "123456.78"
       expected_docs: ["Invoices/Acme/INV-0042.pdf", "Invoices/Acme/INV-0051.pdf"]
   ```
2. Run:
   ```
   shodh-eval fetch-models --dir D:\shodh-models
   shodh-eval retrieval --corpus D:\Contracts --dataset my_questions.yaml --models D:\shodh-models --out-dir D:\eval-out --repeat 3
   shodh-eval answers --corpus D:\Contracts --dataset my_questions.yaml --models D:\shodh-models --out D:\eval-out\answers.json --llm-url http://localhost:11434/v1/chat/completions --llm-model qwen3:8b
   ```
Results stay on your machine. Do not commit private datasets or reports.
```

- [ ] **Step 4: Push, open the PR, record the baseline**

```bash
git add .github/workflows/ci.yml .github/workflows/eval-baseline.yml eval/README.md eval/baselines/.gitkeep
git commit -m "CI: add retrieval eval gate and manual baseline workflow"
git push -u origin feat/m1-eval-harness
```

Then:
1. Open the PR. The `eval-retrieval` job will fail at the gate step because no baselines are committed yet. That is the expected first state.
2. Ask the user to run **Eval baseline (manual)** on the PR branch with `repeat=5`.
3. Download the `eval-baselines` artifact and commit both JSON files into `eval/baselines/`:

```bash
git add eval/baselines/cuad-pr-seed20261002.json eval/baselines/invoices-seed20261002-n120.json
git commit -m "Record retrieval baselines for the current pipeline"
git push
```

Expected: `eval-retrieval` passes, comparing against the baselines recorded from the same code.

- [ ] **Step 5: Record the answer baseline locally (user)**

The user runs `shodh-eval answers` against both public datasets with their chosen local model (for example via Ollama), and then against their private folders. Commit **only** the public-dataset answer reports to `eval/baselines/answers/` with the model name in the file name. Private reports stay local.

This is the "before" number for every later milestone.
