//! What kind of documents the library holds, so Ask can suggest questions
//! that fit it: research papers get questions about methods, results and
//! comparisons instead of payments and deadlines.
//!
//! Read from the indexed text itself (no model call): a sample of chunks is
//! grouped by file, and a file reads as a paper when its text carries
//! several of the marks papers share (an abstract, a references section,
//! "et al.", an arXiv identifier, figure and table captions, …).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use tauri::State;

use crate::rag_commands::RagState;

/// Chunks read to judge the library (the first ones stored; enough for a
/// few dozen documents, cheap for any library).
const SAMPLE_CHUNKS: usize = 2_000;
/// Distinct paper marks a file needs to read as a paper.
const PAPER_MARKS: usize = 3;
/// Share of the sampled files that must read as papers.
const PAPER_SHARE: f32 = 0.5;

/// The kinds of library Ask suggests questions for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryKind {
    /// Mostly research papers.
    Papers,
    /// Anything else (contracts, invoices, notes, mixed files).
    Documents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryProfile {
    pub kind: LibraryKind,
    /// Files in the sample.
    pub files: usize,
    /// Of those, the ones that read as papers.
    pub papers: usize,
}

/// The paper marks `text` (lower-cased) carries, by name.
fn paper_marks(text: &str, out: &mut BTreeSet<&'static str>) {
    let has_line = |heading: &str| {
        text.lines().any(|line| {
            let line = line
                .trim()
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
            line == heading || line.strip_prefix(heading).is_some_and(|rest| rest.starts_with(['\u{2014}', '-', ':', ' ']) && rest.len() < 60)
        })
    };
    // "Abstract", "ABSTRACT", "Abstract — We present …" opening a line.
    let abstract_opens_a_line = text.lines().any(|line| {
        line.trim()
            .strip_prefix("abstract")
            .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
    });
    if abstract_opens_a_line {
        out.insert("abstract");
    }
    if has_line("references") || has_line("bibliography") {
        out.insert("references");
    }
    if has_line("introduction") {
        out.insert("introduction");
    }
    if has_line("related work") || has_line("conclusion") || has_line("conclusions") {
        out.insert("sections");
    }
    if text.contains(" et al.") || text.contains(" et al,") {
        out.insert("et al");
    }
    if text.contains("arxiv") {
        out.insert("arxiv");
    }
    if text.contains("proceedings of") || text.contains("conference on") || text.contains("journal of") {
        out.insert("venue");
    }
    let caption = |word: &str| {
        text.match_indices(word).any(|(at, _)| {
            text[at + word.len()..]
                .trim_start()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })
    };
    if caption("fig.") || caption("figure") || caption("table") {
        out.insert("captions");
    }
}

/// The profile of a sample of `(file, chunk text)`.
pub fn profile(samples: &[(String, String)]) -> LibraryProfile {
    let mut marks: BTreeMap<&str, BTreeSet<&'static str>> = BTreeMap::new();
    for (file, text) in samples {
        paper_marks(&text.to_lowercase(), marks.entry(file.as_str()).or_default());
    }
    let files = marks.len();
    let papers = marks.values().filter(|m| m.len() >= PAPER_MARKS).count();
    let kind = if files > 0 && papers as f32 >= PAPER_SHARE * files as f32 {
        LibraryKind::Papers
    } else {
        LibraryKind::Documents
    };
    LibraryProfile {
        kind,
        files,
        papers,
    }
}

/// What kind of documents the library holds (see the module docs).
#[tauri::command]
pub async fn library_profile(state: State<'_, RagState>) -> Result<LibraryProfile, String> {
    let rag = state.rag.read().await;
    let chunks = rag
        .list_documents(None, SAMPLE_CHUNKS)
        .await
        .map_err(|e| format!("The index could not be read: {e}"))?;
    drop(rag);
    let samples: Vec<(String, String)> = chunks
        .into_iter()
        .filter(|c| {
            // In-app records (tasks, notes) are not documents of the library.
            !c.metadata
                .get("source_file")
                .is_some_and(|s| s.contains("://"))
        })
        .map(|c| {
            (
                c.metadata.get("source_file").cloned().unwrap_or_default(),
                c.snippet,
            )
        })
        .collect();
    Ok(profile(&samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(file: &str, text: &str) -> (String, String) {
        (file.to_string(), text.to_string())
    }

    #[test]
    fn a_library_of_papers_reads_as_papers() {
        let samples = vec![
            chunk(
                "hnsw.pdf",
                "Abstract \u{2014} We present a new approach for approximate K-nearest neighbor search.",
            ),
            chunk(
                "hnsw.pdf",
                "1 Introduction\nGraph methods [27] ... Malkov et al. showed\nFig. 2. Illustration of the heuristic",
            ),
            chunk(
                "nsg.pdf",
                "ABSTRACT\nApproximate nearest neighbor search (ANNS) is a fundamental problem.",
            ),
            chunk(
                "nsg.pdf",
                "Table 3: Recall at 100. Proceedings of the VLDB Endowment.\nREFERENCES\n[1] A. Andoni et al.",
            ),
            chunk("notes.txt", "Buy milk. Call the plumber on Tuesday."),
        ];
        let p = profile(&samples);
        assert_eq!((p.kind, p.files, p.papers), (LibraryKind::Papers, 3, 2));
    }

    #[test]
    fn contracts_and_invoices_are_documents() {
        let samples = vec![
            chunk(
                "msa.pdf",
                "MASTER SERVICES AGREEMENT\n1. Term. This agreement renews each year unless notice is given.",
            ),
            chunk(
                "invoice-0042.pdf",
                "Invoice INV-2026-0042\nTotal due: INR 7,96,500.00 by 30 June 2026",
            ),
            chunk(
                "policy.docx",
                "Table 1 lists the leave days. Introduction to the policy follows.",
            ),
        ];
        let p = profile(&samples);
        assert_eq!(p.kind, LibraryKind::Documents);
        assert_eq!(p.papers, 0);
        assert_eq!(profile(&[]).kind, LibraryKind::Documents);
    }
}
