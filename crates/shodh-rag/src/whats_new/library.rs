//! The papers already in the user's library, as a paper alert compares against them, and
//! the titles topic suggestions are drawn from. Read on this computer only.
//!
//! - **Identities** come from the paper graph's scans of each library PDF (`citation_scans`:
//!   its title, DOI and arXiv id, read from the document), so they exist once the graph
//!   has been built.
//! - **File names** add what the scans do not cover: an arXiv id in a PDF's name
//!   (`2406.06484 - Title.pdf`) and, for topic suggestions, a name that reads as a title
//!   ([`title_from_file_name`]); ids, DOIs, hashes, numbers and names like
//!   `paper_final(1)` are not titles.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::harness::web::papers::{normalize_arxiv_id, normalize_doi, Paper};
use crate::harness::web::relevance::{is_stopword, words};
use crate::research::citations::identity::filename_arxiv_id;

/// A library paper known by its identifiers and title.
pub fn known_paper(title: &str, doi: Option<&str>, arxiv_id: Option<&str>) -> Paper {
    Paper {
        title: title.to_string(),
        authors: Vec::new(),
        year: None,
        venue: None,
        abstract_snippet: None,
        doi: doi.and_then(normalize_doi),
        arxiv_id: arxiv_id.and_then(normalize_arxiv_id),
        landing_url: None,
        pdf_url: None,
        sources: Vec::new(),
        relevance: None,
        title_match: false,
        source_score: None,
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The identity of one stored scan (`PaperScan` JSON), as a paper; `None` when it
/// names nothing.
pub fn scan_paper(scan_json: &str) -> Option<Paper> {
    let scan: Value = serde_json::from_str(scan_json).ok()?;
    let identity = scan.get("identity")?;
    let title = text(identity, "title").unwrap_or("");
    let doi = text(identity, "doi");
    let arxiv = text(identity, "arxivId").or_else(|| text(identity, "filenameArxivId"));
    if title.is_empty() && doi.is_none() && arxiv.is_none() {
        return None;
    }
    Some(known_paper(title, doi, arxiv))
}

/// The library's papers: the identities of the stored scans, and the arXiv ids in the
/// names of the indexed files.
pub fn library_papers(scan_jsons: &[String], indexed_files: &[String]) -> Vec<Paper> {
    let mut papers: Vec<Paper> = scan_jsons.iter().filter_map(|s| scan_paper(s)).collect();
    for file in indexed_files {
        let Some(name) = Path::new(file).file_name().map(|n| n.to_string_lossy()) else {
            continue;
        };
        if let Some(id) = filename_arxiv_id(&name) {
            papers.push(known_paper("", None, Some(&id)));
        }
    }
    papers
}

/// The titles of stored scans (for topic suggestions).
pub fn scan_titles(scan_jsons: &[String]) -> Vec<String> {
    scan_jsons
        .iter()
        .filter_map(|s| {
            let scan: Value = serde_json::from_str(s).ok()?;
            text(scan.get("identity")?, "title").map(str::to_string)
        })
        .collect()
}

static ARXIV_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(arxiv)?\d{4}\.\d{4,5}(v\d+)?\b").expect("arXiv id"));
static DOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b10\.\d{4,9}[/_][^\s]+").expect("DOI"));
static COPY_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*\d+\s*\)|\[\s*\d+\s*\]").expect("copy mark"));

/// Words of a file name that say what kind of file it is, never what it is about.
const FILE_WORDS: [&str; 16] = [
    "download",
    "downloads",
    "final",
    "draft",
    "copy",
    "paper",
    "papers",
    "untitled",
    "document",
    "scan",
    "file",
    "new",
    "old",
    "version",
    "rev",
    "preprint",
];

fn is_junk_word(word: &str) -> bool {
    let has_digit = word.chars().any(|c| c.is_ascii_digit());
    let hex = word.len() >= 8 && word.chars().all(|c| c.is_ascii_hexdigit());
    let version =
        word.len() <= 3 && word.starts_with('v') && word[1..].chars().all(|c| c.is_ascii_digit());
    hex || version || has_digit || FILE_WORDS.contains(&word)
}

/// The title a PDF's file name spells, or `None` when the name is an identifier or a
/// placeholder: an arXiv id or DOI, a hash, a number, `paper_final(1)`, `download`.
/// What is left after dropping those must hold at least two words of three or more
/// letters that are not stopwords.
pub fn title_from_file_name(file_name: &str) -> Option<String> {
    let stem = Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = ARXIV_ID.replace_all(&stem, " ");
    let stem = DOI.replace_all(&stem, " ");
    let stem = COPY_MARK.replace_all(&stem, " ");
    let kept: Vec<String> = words(&stem)
        .into_iter()
        .filter(|w| !is_junk_word(w))
        .collect();
    let meaningful = kept
        .iter()
        .filter(|w| w.chars().count() >= 3 && !is_stopword(w))
        .count();
    (meaningful >= 2).then(|| kept.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_give_their_identities() {
        let scan = r#"{"identity": {"title": "Gated Linear Attention", "doi": "https://doi.org/10.1234/ABC", "arxivId": "2312.06635v2"}}"#;
        let paper = scan_paper(scan).unwrap();
        assert_eq!(paper.title, "Gated Linear Attention");
        assert_eq!(paper.doi.as_deref(), Some("10.1234/abc"));
        assert_eq!(paper.arxiv_id.as_deref(), Some("2312.06635"));
        assert!(scan_paper(r#"{"identity": {}}"#).is_none());
        assert!(scan_paper("not json").is_none());
    }

    #[test]
    fn library_papers_without_a_graph_use_file_names() {
        let papers = library_papers(
            &[],
            &[
                "C:/library/Papers/2406.06484 - Parallelizing Linear Transformers.pdf".into(),
                "C:/library/Papers/notes.md".into(),
            ],
        );
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].arxiv_id.as_deref(), Some("2406.06484"));
    }

    #[test]
    fn file_names_that_are_ids_or_junk_are_not_titles() {
        for name in [
            "2301.12345v2.pdf",
            "arXiv2301.12345.pdf",
            "10.1145_3292500.3330701.pdf",
            "paper_final(1).pdf",
            "download.pdf",
            "download (3).pdf",
            "3f9a2c7be81d4e60a1b2.pdf",
            "20231104.pdf",
            "12345.pdf",
            "untitled document.pdf",
            "draft v2.pdf",
            "scan_0001.pdf",
        ] {
            assert_eq!(title_from_file_name(name), None, "{name}");
        }
    }

    #[test]
    fn file_names_that_read_as_titles_are_kept() {
        assert_eq!(
            title_from_file_name("2406.06484 - Parallelizing Linear Transformers.pdf").as_deref(),
            Some("parallelizing linear transformers")
        );
        assert_eq!(
            title_from_file_name("kv_cache_compression_survey (1).pdf").as_deref(),
            Some("kv cache compression survey")
        );
        assert_eq!(
            title_from_file_name("Linear-Attention-final.pdf").as_deref(),
            Some("linear attention")
        );
    }
}
