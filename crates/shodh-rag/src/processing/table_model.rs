//! The optional table model: table regions and table structure for the pages the
//! layout heuristics flag as table candidates (`pdf_layout::table_candidate_pages`).
//!
//! On each candidate page the page is rendered, docling.rs's Heron layout detector
//! finds the table regions, and its TableFormer port predicts each table's grid: rows,
//! columns, row and column spans, header cells and a box per cell, with the page's
//! words matched into the cells. Everything else of the document keeps coming from
//! the fast heuristic parser; see `docs/adr/0002-document-parser.md` ("Table model").
//!
//! The model files are pinned in
//! [`crate::embeddings::model_store::table_model_artifacts`] and downloaded only when
//! the user asks. Without them (or on a platform where the model cannot be loaded
//! safely, see [`TableModel::load`]) tables come from the heuristics alone.
//!
//! Coordinates: docling reports page points with a top-left origin; everything here is
//! converted to the document model's bottom-left origin ([`BBox`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::document_model::BBox;
use crate::embeddings::model_store::TABLE_MODEL_DIR;

/// Why the table model could not run.
#[derive(Debug, thiserror::Error)]
pub enum TableModelError {
    #[error("the table model is not installed (missing {0})")]
    NotInstalled(String),
    #[error("the table model is only loaded on Windows; tables use the layout heuristics")]
    UnsupportedPlatform,
    #[error("the table model could not be loaded: {0}")]
    Load(String),
    #[error("page {page} could not be prepared for the table model: {message}")]
    Page { page: u32, message: String },
    #[error("the table model failed on page {page}: {message}")]
    Inference { page: u32, message: String },
    #[error("the table model panicked on page {0}")]
    Panicked(u32),
}

/// Id of the model set, recorded on the chunks and Results whose tables it structured.
pub const TABLE_MODEL_ID: &str = "docling-heron-int8+tableformer/models-v1";

/// The table model when it is installed, shared by the parsers and the background
/// refinement. Loaded on first use and dropped when idle.
pub type SharedTableModel = std::sync::Arc<crate::lazy_model::LazyModel<TableModel>>;

/// A handle for a table model that is not installed yet.
pub fn shared_table_model() -> SharedTableModel {
    std::sync::Arc::new(crate::lazy_model::LazyModel::new("table model"))
}

/// Files of the model set, relative to the model directory (`docling-tables/`).
const LAYOUT_FILE: &str = "layout_heron_int8.onnx";
const ENCODER_FILE: &str = "encoder_fp16.onnx";
const DECODER_FILE: &str = "decoder_int8.onnx";
const BBOX_FILE: &str = "bbox.onnx";
const BBOX_DATA_FILE: &str = "bbox.onnx.data";

/// Layout label of table regions.
const TABLE_LABEL: &str = "table";
/// A table region overlapping a higher-scoring one by more than this share of its
/// own area is the same table detected twice.
const DUPLICATE_OVERLAP: f32 = 0.5;

/// One cell of a predicted table, in page points (bottom-left origin).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCell {
    pub text: String,
    pub bbox: Option<BBox>,
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    /// TableFormer's column-header tag (`ched`).
    pub column_header: bool,
    /// TableFormer's row-header tag (`rhed`).
    pub row_header: bool,
    /// TableFormer's section-row tag (`srow`): a full-width group label.
    pub row_section: bool,
}

/// One predicted table: its region and its cells (anchors only; spans say what a cell
/// covers).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTable {
    pub bbox: BBox,
    pub score: f32,
    pub cells: Vec<ModelCell>,
}

/// What the model found on one page.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPage {
    /// 1-based page number.
    pub number: u32,
    /// Table regions the layout detector found.
    pub regions: usize,
    /// Regions TableFormer gave a structure.
    pub tables: Vec<ModelTable>,
    /// Render + layout + structure time for the page.
    pub elapsed: Duration,
}

struct Models {
    layout: docling_pdf::layout::LayoutModel,
    structure: docling_pdf::tableformer::TableFormer,
}

/// The loaded layout and table-structure models. Inference takes `&mut` on both
/// models, so pages are processed one at a time behind a mutex.
pub struct TableModel {
    models: Mutex<Models>,
    dir: PathBuf,
}

impl std::fmt::Debug for TableModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableModel")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// Serializes every change of the process environment made to load the models.
static MODEL_ENV: Mutex<()> = Mutex::new(());

/// Sets environment variables for the lifetime of the guard and restores the previous
/// values (or their absence) when it is dropped, also on unwinding.
struct ScopedEnv {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl ScopedEnv {
    fn set(vars: &[(&'static str, std::ffi::OsString)]) -> ScopedEnv {
        let saved = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
        ScopedEnv { saved }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

/// The model directory under a model root.
pub fn model_dir(root: &Path) -> PathBuf {
    root.join(TABLE_MODEL_DIR)
}

impl TableModel {
    /// Loads the models from `dir` (the `docling-tables` directory of the model root)
    /// with `threads` intra-op threads for the layout detector and the structure
    /// encoder (the structure decoder always runs on one).
    ///
    /// docling.rs takes model paths only from process environment variables
    /// (`DOCLING_LAYOUT_ONNX`, `DOCLING_TABLEFORMER_{ENCODER,DECODER,BBOX}`), read
    /// when a model is loaded. They are set for the duration of the load only, under
    /// a process-wide lock, and restored afterwards; `DOCLING_RS_NO_GRAPH_CACHE` keeps
    /// docling from writing optimized copies of the graphs into the user's home
    /// directory. Changing the environment while other threads run is sound on
    /// Windows, where the variables live behind the OS's own lock
    /// (`SetEnvironmentVariableW`); on other platforms a C library reading the
    /// environment at the same moment could observe a torn update, so the model is
    /// not loaded there ([`TableModelError::UnsupportedPlatform`]) and tables keep
    /// coming from the heuristics.
    pub fn load(dir: &Path, threads: usize) -> Result<TableModel, TableModelError> {
        for file in [
            LAYOUT_FILE,
            ENCODER_FILE,
            DECODER_FILE,
            BBOX_FILE,
            BBOX_DATA_FILE,
        ] {
            if !dir.join(file).is_file() {
                return Err(TableModelError::NotInstalled(file.to_string()));
            }
        }
        if !cfg!(windows) {
            return Err(TableModelError::UnsupportedPlatform);
        }
        let threads = threads.max(1);
        let path = |file: &str| dir.join(file).into_os_string();
        let _lock = MODEL_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnv::set(&[
            ("DOCLING_LAYOUT_ONNX", path(LAYOUT_FILE)),
            ("DOCLING_TABLEFORMER_ENCODER", path(ENCODER_FILE)),
            ("DOCLING_TABLEFORMER_DECODER", path(DECODER_FILE)),
            ("DOCLING_TABLEFORMER_BBOX", path(BBOX_FILE)),
            ("DOCLING_RS_NO_GRAPH_CACHE", "1".into()),
        ]);
        let loaded = std::panic::catch_unwind(|| {
            let layout = docling_pdf::layout::LayoutModel::load_with(threads)
                .map_err(TableModelError::Load)?;
            let structure =
                docling_pdf::tableformer::TableFormer::load_with(threads).ok_or_else(|| {
                    TableModelError::Load("the table structure graphs did not load".to_string())
                })?;
            Ok(Models { layout, structure })
        })
        .unwrap_or_else(|_| {
            Err(TableModelError::Load(
                "loading the models panicked".to_string(),
            ))
        })?;
        Ok(TableModel {
            models: Mutex::new(loaded),
            dir: dir.to_path_buf(),
        })
    }

    /// Finds and structures the tables of `pages` (1-based) of the PDF in `bytes`.
    /// A page that fails is reported in the error list and skipped; the others are
    /// returned in the order given.
    pub fn structure_pages(
        &self,
        bytes: &[u8],
        pages: &[u32],
    ) -> (Vec<ModelPage>, Vec<TableModelError>) {
        let mut out = Vec::with_capacity(pages.len());
        let mut errors = Vec::new();
        for &number in pages {
            match self.structure_page(bytes, number) {
                Ok(page) => out.push(page),
                Err(e) => errors.push(e),
            }
        }
        (out, errors)
    }

    /// Table regions the layout detector finds on page `number`, without structuring
    /// them (the candidate-recall survey runs it on every page).
    #[cfg(test)]
    pub(crate) fn table_regions_on(&self, bytes: &[u8], number: u32) -> Option<usize> {
        let index = number.saturating_sub(1) as usize;
        let mut rendered = None;
        docling_pdf::pdfium_backend::for_each_page::<docling_pdf::PdfError, _>(
            bytes,
            None,
            true,
            true,
            Some((index, index)),
            |_, _, page| {
                rendered = Some(page);
                Ok(())
            },
        )
        .ok()?;
        let page = rendered?;
        let mut models = self.models.lock().unwrap_or_else(|e| e.into_inner());
        let regions = models
            .layout
            .predict(docling_pdf::layout_src(&page), page.width, page.height)
            .ok()?;
        Some(table_regions(&regions).len())
    }

    /// The page rendered as the models see it (2 px per point) with its height in
    /// points, for the spot-check harness.
    #[cfg(test)]
    pub(crate) fn render_page(bytes: &[u8], number: u32) -> Option<(image::RgbImage, f32)> {
        let index = number.saturating_sub(1) as usize;
        let mut rendered = None;
        docling_pdf::pdfium_backend::for_each_page::<docling_pdf::PdfError, _>(
            bytes,
            None,
            true,
            false,
            Some((index, index)),
            |_, _, page| {
                rendered = Some((page.image, page.height));
                Ok(())
            },
        )
        .ok()?;
        rendered
    }

    fn structure_page(&self, bytes: &[u8], number: u32) -> Result<ModelPage, TableModelError> {
        let started = Instant::now();
        let index = number.saturating_sub(1) as usize;
        let mut rendered: Option<docling_pdf::PdfPage> = None;
        let walk = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            docling_pdf::pdfium_backend::for_each_page::<docling_pdf::PdfError, _>(
                bytes,
                None,
                true,
                true,
                Some((index, index)),
                |_, _, page| {
                    rendered = Some(page);
                    Ok(())
                },
            )
        }));
        match walk {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return Err(TableModelError::Page {
                    page: number,
                    message: e.to_string(),
                })
            }
            Err(_) => return Err(TableModelError::Panicked(number)),
        }
        let page = rendered.ok_or_else(|| TableModelError::Page {
            page: number,
            message: "the page was not rendered".to_string(),
        })?;
        let mut models = self.models.lock().unwrap_or_else(|e| e.into_inner());
        let models = &mut *models;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let regions = models
                .layout
                .predict(docling_pdf::layout_src(&page), page.width, page.height)
                .map_err(|message| TableModelError::Inference {
                    page: number,
                    message,
                })?;
            let regions = table_regions(&regions);
            if regions.is_empty() {
                return Ok((0, Vec::new()));
            }
            let frames: Vec<[f32; 4]> = regions.iter().map(|(_, r)| *r).collect();
            let page1024 = docling_pdf::tableformer::TableFormer::page_1024(&page.image);
            let grids = models.structure.predict_tables_on(
                page.image.height(),
                &page1024,
                &frames,
                &page.word_cells,
            );
            let tables = regions
                .iter()
                .zip(grids)
                .filter_map(|((score, region), grid)| {
                    let grid = grid?;
                    let cells: Vec<ModelCell> = grid
                        .cells
                        .iter()
                        .map(|c| ModelCell {
                            text: c.text.split_whitespace().collect::<Vec<_>>().join(" "),
                            bbox: c.bbox.map(|b| to_bottom_left(b, page.height)),
                            row: c.start_row,
                            col: c.start_col,
                            row_span: c.row_span.max(1),
                            col_span: c.col_span.max(1),
                            column_header: c.column_header,
                            row_header: c.row_header,
                            row_section: c.row_section,
                        })
                        .collect();
                    (!cells.is_empty()).then(|| ModelTable {
                        bbox: to_bottom_left(*region, page.height),
                        score: *score,
                        cells,
                    })
                })
                .collect();
            Ok((frames.len(), tables))
        }));
        let (regions, tables) = match result {
            Ok(r) => r?,
            Err(_) => return Err(TableModelError::Panicked(number)),
        };
        Ok(ModelPage {
            number,
            regions,
            tables,
            elapsed: started.elapsed(),
        })
    }
}

/// Converts a top-left `[l, t, r, b]` box in page points to the bottom-left origin.
pub fn to_bottom_left(b: [f32; 4], page_height: f32) -> BBox {
    BBox::new(b[0], page_height - b[3], b[2], page_height - b[1])
}

/// The table regions of a page's layout detections above docling's table threshold,
/// best first, with duplicates (a region mostly inside a better one) removed. Boxes stay
/// top-left `[l, t, r, b]`.
fn table_regions(regions: &[docling_pdf::layout::Region]) -> Vec<(f32, [f32; 4])> {
    let mut tables: Vec<(f32, [f32; 4])> = regions
        .iter()
        .filter(|r| {
            r.label == TABLE_LABEL && r.score >= docling_pdf::layout::label_threshold(r.label)
        })
        .map(|r| (r.score, [r.l, r.t, r.r, r.b]))
        .collect();
    tables.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut kept: Vec<(f32, [f32; 4])> = Vec::new();
    for (score, b) in tables {
        let area = ((b[2] - b[0]) * (b[3] - b[1])).max(1.0);
        let duplicate = kept.iter().any(|(_, k)| {
            let w = (b[2].min(k[2]) - b[0].max(k[0])).max(0.0);
            let h = (b[3].min(k[3]) - b[1].max(k[1])).max(0.0);
            w * h / area > DUPLICATE_OVERLAP
        });
        if !duplicate {
            kept.push((score, b));
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_flip_to_a_bottom_left_origin() {
        let b = to_bottom_left([205.7, 109.8, 228.1, 118.9], 792.0);
        assert!((b.x0 - 205.7).abs() < 1e-3 && (b.x1 - 228.1).abs() < 1e-3);
        assert!((b.y0 - 673.1).abs() < 1e-3 && (b.y1 - 682.2).abs() < 1e-3);
    }

    #[test]
    fn duplicate_and_weak_table_regions_are_dropped() {
        let region = |label: &'static str, score: f32, l: f32, t: f32, r: f32, b: f32| {
            docling_pdf::layout::Region {
                label,
                score,
                l,
                t,
                r,
                b,
            }
        };
        let kept = table_regions(&[
            region("table", 0.8, 100.0, 100.0, 300.0, 200.0),
            // Mostly inside the first: the same table again.
            region("table", 0.6, 110.0, 110.0, 290.0, 210.0),
            region("table", 0.9, 100.0, 400.0, 300.0, 500.0),
            region("table", 0.3, 100.0, 600.0, 300.0, 700.0),
            region("picture", 0.95, 100.0, 600.0, 300.0, 700.0),
        ]);
        assert_eq!(
            kept,
            vec![
                (0.9, [100.0, 400.0, 300.0, 500.0]),
                (0.8, [100.0, 100.0, 300.0, 200.0])
            ]
        );
    }

    #[test]
    fn scoped_environment_is_restored() {
        let key = "SHODH_TABLE_MODEL_ENV_TEST";
        std::env::remove_var(key);
        {
            let _env = ScopedEnv::set(&[(key, "set".into())]);
            assert_eq!(std::env::var(key).as_deref(), Ok("set"));
        }
        assert!(std::env::var_os(key).is_none());
    }

    #[test]
    fn a_missing_file_means_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        match TableModel::load(dir.path(), 1) {
            Err(TableModelError::NotInstalled(file)) => assert_eq!(file, LAYOUT_FILE),
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }
}
