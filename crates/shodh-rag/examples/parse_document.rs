//! Parse files with the indexing parser and write one JSON file per input
//! describing its blocks (kind, text, page, bbox, level, section path) and the
//! parse time. Used to evaluate the parser against a reference extraction.
//!
//! ```text
//! cargo run -p shodh-rag --example parse_document -- [--table-model <model_root>] <out_dir> <file>...
//! ```
//!
//! With `--table-model`, tables on table-candidate pages are structured by the table
//! model installed under `<model_root>` (as the background refinement does). With
//! `--chunks`, the structure chunker's chunks are written too (token counts estimated
//! from words, about 4 tokens per 3 words).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde_json::json;
use shodh_rag::processing::document_model::BlockKind;
use shodh_rag::processing::structure_chunker::StructureChunker;
use shodh_rag::processing::table_model::{model_dir, TableModel};
use shodh_rag::processing::DocumentParser;

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let model_root = match args.iter().position(|a| a == "--table-model") {
        Some(at) if at + 1 < args.len() => {
            let root = args.remove(at + 1);
            args.remove(at);
            Some(root)
        }
        Some(_) => bail!("--table-model needs the model root"),
        None => None,
    };
    let with_chunks = match args.iter().position(|a| a == "--chunks") {
        Some(at) => {
            args.remove(at);
            true
        }
        None => false,
    };
    if args.len() < 2 {
        bail!("usage: parse_document [--table-model <model_root>] [--chunks] <out_dir> <file>...");
    }
    let out_dir = Path::new(&args[0]);
    std::fs::create_dir_all(out_dir)?;
    let parser = match &model_root {
        Some(root) => {
            let model = TableModel::load(&model_dir(Path::new(root)), 4)
                .with_context(|| format!("loading the table model under {root}"))?;
            DocumentParser::with_table_model(Arc::new(model))
        }
        None => DocumentParser::new(),
    };
    let mode = if model_root.is_some() {
        "model"
    } else {
        "shodh"
    };
    for file in &args[1..] {
        let path = Path::new(file);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let start = Instant::now();
        let parsed = parser.parse_file(path);
        let ms = start.elapsed().as_millis();
        let value = match parsed {
            Ok(doc) => {
                let blocks: Vec<_> = doc
                    .document
                    .as_ref()
                    .map(|d| {
                        d.blocks
                            .iter()
                            .map(|b| {
                                let level = match b.kind {
                                    BlockKind::Heading { level } => level,
                                    _ => 0,
                                };
                                json!({
                                    "kind": b.kind.name(),
                                    "text": b.render(),
                                    "page": b.page.unwrap_or(0),
                                    "bbox": b.bbox.map(|x| [x.x0, x.y0, x.x1, x.y1]),
                                    "level": level,
                                    "section": b.section_path.join(" > "),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let pages = doc.document.as_ref().map(|d| d.pages.len()).unwrap_or(0);
                let estimate = |text: &str| (text.split_whitespace().count() * 4).div_ceil(3);
                let chunks: Vec<_> = match (&doc.document, with_chunks) {
                    (Some(structured), true) => StructureChunker::default()
                        .chunk(structured, &doc.title, &estimate)
                        .into_iter()
                        .map(|c| {
                            let layout = c.layout.as_ref();
                            json!({
                                "text": c.text,
                                "page": layout.and_then(|l| l.page_start).unwrap_or(0),
                                "unit": layout.map(|l| l.unit).unwrap_or(""),
                            })
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                json!({"file": name, "mode": mode, "ms": ms, "pages": pages, "status": "ok",
                       "structured": doc.document.is_some(), "blocks": blocks,
                       "content_chars": doc.content.chars().count(),
                       "sections": doc.structured_sections.len(),
                       "chunks": chunks,
                       "table_candidate_pages": doc.metadata.get("table_candidate_pages"),
                       "model_tables": doc.metadata.get("model_tables")})
            }
            Err(e) => json!({"file": name, "mode": mode, "ms": ms, "pages": 0,
                             "status": format!("{e:#}"), "blocks": []}),
        };
        let blocks = value["blocks"].as_array().map(Vec::len).unwrap_or(0);
        eprintln!("{ms:7} ms {blocks:6} blocks  {name}  {}", value["status"]);
        std::fs::write(
            out_dir.join(format!("{mode}__{name}.json")),
            serde_json::to_string(&value)?,
        )
        .with_context(|| format!("writing output for {name}"))?;
    }
    Ok(())
}
