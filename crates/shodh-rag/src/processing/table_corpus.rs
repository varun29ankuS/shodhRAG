//! Corpus harnesses of the table model (ignored; they need the model files and a folder
//! of PDFs). `SHODH_MODEL_ROOT` is the model root (the table model is installed under
//! it when missing) and `SHODH_RESEARCH_CORPUS` a folder of PDFs searched recursively.

use std::path::{Path, PathBuf};

use super::table_model::{model_dir, TableModel};
use crate::embeddings::model_store::{HttpByteSource, ModelStore};

pub(crate) fn pdfs_under(dir: &Path, out: &mut Vec<PathBuf>) {
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

pub(crate) fn installed_model() -> Option<TableModel> {
    let root = PathBuf::from(std::env::var("SHODH_MODEL_ROOT").ok()?);
    let store = ModelStore::table_model(root.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let source = HttpByteSource::new().unwrap();
    runtime
        .block_on(store.install(&source, &|_| {}))
        .expect("table model install");
    Some(TableModel::load(&model_dir(&root), 4).expect("table model load"))
}

#[test]
#[ignore = "needs SHODH_MODEL_ROOT and SHODH_TABLE_PROBE (pdf path) and SHODH_TABLE_PROBE_PAGE"]
fn probe_one_page() {
    let Some(model) = installed_model() else {
        return;
    };
    let (Ok(path), Ok(page)) = (
        std::env::var("SHODH_TABLE_PROBE"),
        std::env::var("SHODH_TABLE_PROBE_PAGE"),
    ) else {
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let page: u32 = page.parse().unwrap();
    for _ in 0..2 {
        let (pages, errors) = model.structure_pages(&bytes, &[page]);
        println!("errors: {errors:?}");
        for p in &pages {
            println!("page {} regions {} in {:?}", p.number, p.regions, p.elapsed);
            for t in &p.tables {
                println!("table {:?}", t.bbox);
                for c in &t.cells {
                    println!(
                        "  r{} c{} {}x{} h{} rh{} s{} {:?} {:?}",
                        c.row,
                        c.col,
                        c.row_span,
                        c.col_span,
                        c.column_header,
                        c.row_header,
                        c.row_section,
                        c.text,
                        c.bbox
                    );
                }
            }
        }
    }
}
