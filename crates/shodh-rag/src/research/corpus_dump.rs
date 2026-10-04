//! Builds the table fixtures of the Result extractor from a real paper corpus.
//!
//! `cargo test -p shodh-rag --lib research::corpus_dump -- --ignored` with
//! `SHODH_RESEARCH_CORPUS` set to a folder of PDFs (searched recursively) and
//! `SHODH_TABLE_DUMP` set to the JSON file to write. The output holds every table
//! block the layout parser finds (page, page height, box, caption, header, rows,
//! cell boxes) keyed by file name; the extractor tests read the checked-in copy, so
//! CI needs no corpus.

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

/// The table blocks of one parsed PDF as fixture JSON.
pub(crate) fn tables_json(bytes: &[u8]) -> Vec<Value> {
    let Ok(doc) = parse_pdf_layout(bytes) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for block in &doc.blocks {
        if let BlockKind::Table {
            header,
            rows,
            caption,
            cell_boxes,
        } = &block.kind
        {
            let page_height = block
                .page
                .and_then(|p| doc.pages.iter().find(|i| i.number == p))
                .map(|i| i.height);
            out.push(json!({
                "page": block.page,
                "pageHeight": page_height,
                "bbox": block.bbox,
                "caption": caption,
                "sectionPath": block.section_path,
                "header": header,
                "rows": rows,
                "cellBoxes": cell_boxes,
            }));
        }
    }
    out
}

#[test]
#[ignore = "needs SHODH_RESEARCH_CORPUS and SHODH_TABLE_DUMP"]
fn dump_corpus_tables() {
    let (Ok(corpus), Ok(target)) = (
        std::env::var("SHODH_RESEARCH_CORPUS"),
        std::env::var("SHODH_TABLE_DUMP"),
    ) else {
        return;
    };
    let mut files = Vec::new();
    pdfs_under(Path::new(&corpus), &mut files);
    files.sort();
    let mut papers = serde_json::Map::new();
    for file in files {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        papers.insert(name, Value::Array(tables_json(&bytes)));
    }
    let text = serde_json::to_string_pretty(&Value::Object(papers)).unwrap();
    std::fs::write(target, text).unwrap();
}
