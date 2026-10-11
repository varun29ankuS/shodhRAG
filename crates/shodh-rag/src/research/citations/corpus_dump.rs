//! Builds the reference fixtures of the citation parser from a real paper corpus.
//!
//! `cargo test -p shodh-rag --lib research::citations::corpus_dump -- --ignored` with
//! `SHODH_RESEARCH_CORPUS` set to a folder of PDFs (searched recursively) and
//! `SHODH_REFERENCE_DUMP` set to the JSON file to write. Only files whose name starts with
//! an arXiv id are read (public preprints). The output holds, per file, the Title block,
//! the first-page text and every bibliography block the layout parser found (page and
//! text); the parser tests read the checked-in copy (`fixtures/corpus_references.json`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::processing::document_model::BlockKind;
use crate::processing::pdf_layout::parse_pdf_layout;

fn pdfs_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            pdfs_under(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            out.push(path);
        }
    }
}

fn starts_with_arxiv_id(name: &str) -> bool {
    let head: String = name.chars().take(10).collect();
    head.len() == 10
        && head.as_bytes()[4] == b'.'
        && head
            .chars()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_digit())
}

#[test]
#[ignore = "needs SHODH_RESEARCH_CORPUS and SHODH_REFERENCE_DUMP"]
fn dump_corpus_references() {
    let (Ok(corpus), Ok(target)) = (
        std::env::var("SHODH_RESEARCH_CORPUS"),
        std::env::var("SHODH_REFERENCE_DUMP"),
    ) else {
        return;
    };
    let mut files = Vec::new();
    pdfs_under(Path::new(&corpus), &mut files);
    files.sort();
    let mut papers = serde_json::Map::new();
    for file in files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !starts_with_arxiv_id(&name) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let started = std::time::Instant::now();
        let Ok(doc) = parse_pdf_layout(&bytes) else {
            continue;
        };
        let parse_ms = started.elapsed().as_millis();
        let title: Vec<&str> = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Title))
            .map(|b| b.text.as_str())
            .collect();
        let first_page: Vec<&str> = doc
            .blocks
            .iter()
            .filter(|b| b.page == Some(1))
            .map(|b| b.text.as_str())
            .collect();
        let references: Vec<Value> = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::ReferenceEntry))
            .map(|b| json!({ "page": b.page, "text": b.text, "section": b.section_path }))
            .collect();
        papers.insert(
            name,
            json!({
                "parseMs": parse_ms,
                "title": title,
                "firstPage": first_page,
                "references": references,
            }),
        );
    }
    let text = serde_json::to_string_pretty(&Value::Object(papers)).unwrap();
    std::fs::write(target, text).unwrap();
}
