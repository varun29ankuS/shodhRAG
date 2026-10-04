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

/// Whether a corpus file may appear in checked-in fixtures: published arXiv papers
/// only (an arXiv id in the name, or KAN, arXiv:2404.19756). The user's own folder is
/// never included.
pub(crate) fn fixture_allowed(path: &Path) -> bool {
    if path
        .components()
        .any(|c| c.as_os_str().to_string_lossy() == "My Papers")
    {
        return false;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let arxiv_id = name.len() > 10
        && name[..4].chars().all(|c| c.is_ascii_digit())
        && name.as_bytes()[4] == b'.'
        && name[5..10].chars().all(|c| c.is_ascii_digit());
    arxiv_id || name.starts_with("KAN - Kolmogorov-Arnold Networks")
}

fn reading_summary(
    tables: &[crate::research::results::interpret::TableInput],
) -> (usize, std::collections::BTreeMap<String, usize>) {
    use crate::research::results::{ambiguous, interpret::read_table};
    let mut candidates = Vec::new();
    let mut reasons = std::collections::BTreeMap::new();
    for table in tables {
        let reading = read_table(table, None);
        candidates.extend(reading.candidates);
        if let Some(reason) = reading.skipped {
            *reasons.entry(reason).or_default() += 1;
        }
    }
    // What extraction would store: ambiguous duplicates are dropped.
    let values = candidates.len() - ambiguous(candidates.iter()).len();
    (values, reasons)
}

/// Survey of the corpus: tables before (heuristic) and after (model on candidate
/// pages), Result values the rules read from each, candidate recall against the layout
/// model run on every page, and timings. Writes `SHODH_TABLE_SURVEY` (JSON) and, for
/// arXiv papers only, the extractor inputs to `SHODH_TABLE_DUMP`.
#[test]
#[ignore = "needs SHODH_MODEL_ROOT, SHODH_RESEARCH_CORPUS, SHODH_TABLE_SURVEY"]
fn corpus_table_survey() {
    use super::pdf_layout::{parse_pdf_layout_with, TableMode};
    use crate::research::results::tables_of;
    use serde_json::{json, Value};
    use std::time::Instant;

    let (Ok(corpus), Ok(survey)) = (
        std::env::var("SHODH_RESEARCH_CORPUS"),
        std::env::var("SHODH_TABLE_SURVEY"),
    ) else {
        return;
    };
    let Some(model) = installed_model() else {
        return;
    };
    let every_page = std::env::var("SHODH_TABLE_RECALL").is_ok();
    let mut files = Vec::new();
    pdfs_under(Path::new(&corpus), &mut files);
    files.sort();
    let mut papers = Vec::new();
    let mut dump = serde_json::Map::new();
    for file in files {
        let bytes = std::fs::read(&file).unwrap();
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let started = Instant::now();
        let Ok(before) = parse_pdf_layout_with(&bytes, TableMode::Candidates) else {
            continue;
        };
        let heuristic_ms = started.elapsed().as_millis();
        let started = Instant::now();
        let after = parse_pdf_layout_with(&bytes, TableMode::Model(&model)).unwrap();
        let model_ms = started.elapsed().as_millis();
        let before_tables = tables_of(&before.document);
        let after_tables = tables_of(&after.document);
        let (before_values, before_reasons) = reading_summary(&before_tables);
        let (after_values, after_reasons) = reading_summary(&after_tables);
        let candidates: Vec<u32> = before.tables.candidates.iter().map(|c| c.page).collect();
        let mut region_pages: Vec<u32> = Vec::new();
        if every_page {
            for page in 1..=before.document.pages.len() as u32 {
                if model.table_regions_on(&bytes, page).unwrap_or(0) > 0 {
                    region_pages.push(page);
                }
            }
        }
        if let (true, Ok(dir)) = (fixture_allowed(&file), std::env::var("SHODH_SPOT_DIR")) {
            spot_check_crops(&bytes, &name, &after_tables, Path::new(&dir));
        }
        if fixture_allowed(&file) {
            dump.insert(
                name.clone(),
                Value::Array(
                    after_tables
                        .iter()
                        .filter_map(|t| serde_json::to_value(t).ok())
                        .collect(),
                ),
            );
        }
        let paper = json!({
            "file": name,
            "fixture": fixture_allowed(&file),
            "pages": before.document.pages.len(),
            "heuristicMs": heuristic_ms,
            "modelMs": model_ms,
            "candidates": before.tables.candidates,
            "regionPages": region_pages,
            "missedRegionPages": region_pages.iter().filter(|p| !candidates.contains(p)).collect::<Vec<_>>(),
            "before": { "tables": before.tables.tables, "values": before_values, "reasons": before_reasons },
            "after": {
                "tables": after.tables.tables,
                "modelTables": after.tables.model_tables,
                "values": after_values,
                "reasons": after_reasons,
                "modelPages": after.tables.model_pages,
                "modelErrors": after.tables.model_errors,
            },
        });
        println!("{}", serde_json::to_string(&paper).unwrap());
        papers.push(paper);
    }
    std::fs::write(&survey, serde_json::to_string_pretty(&papers).unwrap()).unwrap();
    if let Ok(target) = std::env::var("SHODH_TABLE_DUMP") {
        std::fs::write(
            target,
            serde_json::to_string_pretty(&Value::Object(dump)).unwrap(),
        )
        .unwrap();
    }
}

/// Writes every value the rules read from `tables` to `<dir>/values.jsonl` and, for
/// each table that gave values, a crop of the table with every value cell outlined
/// (`<dir>/<paper>-p<page>-t<index>.png`), for checking values against the page.
fn spot_check_crops(
    bytes: &[u8],
    paper: &str,
    tables: &[crate::research::results::interpret::TableInput],
    dir: &Path,
) {
    use crate::research::results::interpret::read_table;
    use std::io::Write;
    std::fs::create_dir_all(dir).unwrap();
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("values.jsonl"))
        .unwrap();
    let stem: String = paper.chars().take(10).collect();
    let readings: Vec<_> = tables.iter().map(|t| read_table(t, None)).collect();
    let dropped =
        crate::research::results::ambiguous(readings.iter().flat_map(|r| r.candidates.iter()));
    let mut ordinal = 0usize;
    for (index, (table, reading)) in tables.iter().zip(&readings).enumerate() {
        if reading.candidates.is_empty() {
            continue;
        }
        let image_name = format!("{stem}-p{}-t{index}.png", table.page);
        for c in &reading.candidates {
            let stored = !dropped.contains(&ordinal);
            ordinal += 1;
            let line = serde_json::json!({
                "stored": stored,
                "paper": paper, "image": image_name, "page": c.page, "method": c.method,
                "dataset": c.dataset, "datasetEvidence": c.dataset_evidence, "metric": c.metric,
                "value": c.number.decimal, "cell": c.cell_text, "spread": c.number.spread,
                "unit": c.unit, "setting": c.setting, "confidence": c.confidence,
                "box": c.cell_box, "caption": table.caption.clone().or(table.nearby_caption.clone()),
            });
            writeln!(log, "{line}").unwrap();
        }
        let Some((image, height)) = TableModel::render_page(bytes, table.page) else {
            continue;
        };
        let Some(bbox) = table.bbox else { continue };
        let scale = image.height() as f32 / height;
        let px = |x: f32| (x * scale).max(0.0) as u32;
        let (x0, x1) = (px(bbox.x0 - 6.0), px(bbox.x1 + 6.0).min(image.width()));
        let (y0, y1) = (
            px(height - bbox.y1 - 30.0),
            px(height - bbox.y0 + 6.0).min(image.height()),
        );
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let mut crop = image::imageops::crop_imm(&image, x0, y0, x1 - x0, y1 - y0).to_image();
        for c in &reading.candidates {
            let Some(b) = c.cell_box else { continue };
            let (bx0, bx1) = (px(b.x0).saturating_sub(x0), px(b.x1).saturating_sub(x0));
            let (by0, by1) = (
                px(height - b.y1).saturating_sub(y0),
                px(height - b.y0).saturating_sub(y0),
            );
            for x in bx0..=bx1.min(crop.width().saturating_sub(1)) {
                for y in [by0, by1] {
                    if y < crop.height() {
                        crop.put_pixel(x, y, image::Rgb([230, 30, 30]));
                    }
                }
            }
        }
        crop.save(dir.join(&image_name)).unwrap();
    }
}
