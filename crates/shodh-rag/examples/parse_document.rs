//! Parse files with the indexing parser and write one JSON file per input
//! describing its blocks (kind, text, page, bbox, level, section path) and the
//! parse time. Used to evaluate the parser against a reference extraction.
//!
//! ```text
//! cargo run -p shodh-rag --example parse_document -- <out_dir> <file>...
//! ```

use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde_json::json;
use shodh_rag::processing::document_model::BlockKind;
use shodh_rag::processing::DocumentParser;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        bail!("usage: parse_document <out_dir> <file>...");
    }
    let out_dir = Path::new(&args[0]);
    std::fs::create_dir_all(out_dir)?;
    let parser = DocumentParser::new();
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
                json!({"file": name, "mode": "shodh", "ms": ms, "pages": pages, "status": "ok",
                       "structured": doc.document.is_some(), "blocks": blocks,
                       "content_chars": doc.content.chars().count()})
            }
            Err(e) => json!({"file": name, "mode": "shodh", "ms": ms, "pages": 0,
                             "status": format!("{e:#}"), "blocks": []}),
        };
        let blocks = value["blocks"].as_array().map(Vec::len).unwrap_or(0);
        eprintln!("{ms:7} ms {blocks:6} blocks  {name}  {}", value["status"]);
        std::fs::write(
            out_dir.join(format!("shodh__{name}.json")),
            serde_json::to_string(&value)?,
        )
        .with_context(|| format!("writing output for {name}"))?;
    }
    Ok(())
}
