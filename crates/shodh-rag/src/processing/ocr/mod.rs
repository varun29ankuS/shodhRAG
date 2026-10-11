//! OCR for scanned PDFs and images with PaddleOCR's PP-OCRv6 models (Apache-2.0),
//! run with ONNX Runtime on every platform.
//!
//! A page is rendered ([`raster`]), text lines are detected with the DB
//! detector ([`det`]), each line is cut out straightened ([`geometry`]) and
//! recognised with the CTC recogniser ([`rec`]), and the lines are put in
//! reading order ([`order`]). Pre- and post-processing follow the models'
//! `inference.yml`; the deviations are documented where they are made.
//!
//! The models are pinned in [`crate::embeddings::model_store::ocr_model_artifacts`],
//! installed with the search models, loaded on the first scanned page and
//! dropped when idle ([`SharedOcrModel`]). Without them, Windows falls back to
//! the system OCR ([`super::windows_ocr`]) and other platforms have no OCR
//! ([`ScanReaderMissing`]).
//!
//! OCR runs behind the user's work: half the logical cores, at below-normal
//! priority ([`priority`]). The app runs it in a child process ([`worker`]).

pub mod cache;
pub mod det;
pub mod geometry;
pub mod order;
pub mod priority;
pub mod raster;
pub mod rec;
pub mod worker;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use image::RgbImage;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Value;

use crate::embeddings::model_store::OCR_MODEL_DIR;
use det::{DetBox, DetParams};
use geometry::Quad;
use rec::{Charset, Recognized};

/// Id of the model set, recorded in the metadata of documents it read.
pub const OCR_MODEL_ID: &str = "pp-ocrv6-small";

/// Files of the model set, relative to the model directory.
pub const DET_MODEL_FILE: &str = "det.onnx";
pub const DET_CONFIG_FILE: &str = "det.yml";
pub const REC_MODEL_FILE: &str = "rec.onnx";
pub const REC_CONFIG_FILE: &str = "rec.yml";

/// Most pixels the detector reads at once (see [`det::bands`]): a Letter page
/// at 300 dpi is detected in bands of 2550 x 768. Chosen by measurement on the
/// scanned evaluation set (`shodh-eval ocr`).
pub const DET_BAND_PIXELS: u32 = 2_000_000;

/// Lines recognised with a lower mean character probability are dropped
/// (PaddleOCR's `drop_score`): specks and smudges read as stray glyphs.
pub const MIN_LINE_SCORE: f32 = 0.5;

/// Most crops recognised in one batch (PaddleOCR's `rec_batch_num`).
const REC_BATCH: usize = 6;
/// Most padded input width per recognition batch, in pixels: the output is
/// `batch x width/8 x classes` floats, about 30 MB at this budget with the
/// 18,710-class dictionary.
const REC_WIDTH_BUDGET: u32 = 3200;

/// A scan was not read because the scan reader (this OCR model) is not
/// installed and no other engine could read it. The indexer reports such
/// files so they are read once it is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("this file is a scan, and the scan reader (31 MB) that reads scans is not installed")]
pub struct ScanReaderMissing;

/// The installed OCR model, shared by the parsers. Loaded on first use and
/// dropped when idle.
pub type SharedOcrModel = std::sync::Arc<crate::lazy_model::LazyModel<PpOcr>>;

/// A handle for an OCR model that is not installed yet.
pub fn shared_ocr_model() -> SharedOcrModel {
    std::sync::Arc::new(crate::lazy_model::LazyModel::new("OCR model"))
}

/// The model directory under a model root.
pub fn model_dir(root: &Path) -> PathBuf {
    root.join(OCR_MODEL_DIR)
}

/// Whether every file of the model set is in `dir`.
pub fn is_installed(dir: &Path) -> bool {
    [
        DET_MODEL_FILE,
        DET_CONFIG_FILE,
        REC_MODEL_FILE,
        REC_CONFIG_FILE,
    ]
    .iter()
    .all(|f| dir.join(f).is_file())
}

/// One recognised line: its box in page pixels, text and confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub quad: Quad,
    pub text: String,
    pub score: f32,
}

/// What OCR read on one page image.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrPage {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<OcrLine>,
}

impl OcrPage {
    /// Blocks of lines in reading order (see [`order`]).
    pub fn blocks(&self) -> Vec<Vec<String>> {
        let lines: Vec<(Quad, &str)> = self
            .lines
            .iter()
            .map(|l| (l.quad, l.text.as_str()))
            .collect();
        order::reading_order(&lines)
    }

    /// The page's text in reading order: blocks separated by blank lines.
    pub fn text(&self) -> String {
        order::page_text(&self.blocks())
    }
}

/// `PostProcess` values of a detector's `inference.yml` (`thresh`,
/// `box_thresh`, `unclip_ratio`, `max_candidates`), as `key: number` lines
/// indented under `PostProcess:`.
fn det_params(yml: &str, band_pixels: u32) -> Result<DetParams> {
    let mut in_post = false;
    let mut values = std::collections::HashMap::new();
    for line in yml.lines() {
        if !line.starts_with(' ') {
            in_post = line.trim_end() == "PostProcess:";
            continue;
        }
        if !in_post {
            continue;
        }
        if let Some((key, value)) = line.trim().split_once(':') {
            values.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    let number = |key: &str| -> Result<f32> {
        values
            .get(key)
            .with_context(|| format!("det.yml has no PostProcess.{key}"))?
            .parse::<f32>()
            .with_context(|| format!("det.yml PostProcess.{key} is not a number"))
    };
    if values.get("name").map(String::as_str) != Some("DBPostProcess") {
        bail!("det.yml does not describe a DB detector");
    }
    Ok(DetParams {
        thresh: number("thresh")?,
        box_thresh: number("box_thresh")?,
        unclip_ratio: number("unclip_ratio")?,
        max_candidates: number("max_candidates")? as usize,
        band_pixels,
    })
}

/// How the model set runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrSettings {
    /// Most pixels the detector reads at once (see [`DET_BAND_PIXELS`]).
    pub det_band_pixels: u32,
    /// Intra-op threads of each session, the calling thread included.
    pub threads: usize,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            det_band_pixels: DET_BAND_PIXELS,
            threads: priority::ocr_threads(),
        }
    }
}

/// An ONNX session on `threads` intra-op threads. The threads ONNX Runtime
/// starts run below normal priority ([`priority::BackgroundThreads`]) and
/// sleep between operators instead of spinning, so a page being read does
/// not keep idle cores busy.
fn session(bytes: &[u8], threads: usize) -> Result<Session> {
    Session::builder()
        .map_err(|e| anyhow!("ONNX session builder: {e:?}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow!("ONNX optimization level: {e:?}"))?
        .with_intra_threads(threads)
        .map_err(|e| anyhow!("ONNX intra threads: {e:?}"))?
        .with_inter_threads(1)
        .map_err(|e| anyhow!("ONNX inter threads: {e:?}"))?
        .with_intra_op_spinning(false)
        .map_err(|e| anyhow!("ONNX intra-op spinning: {e:?}"))?
        .with_thread_manager(priority::BackgroundThreads)
        .map_err(|e| anyhow!("ONNX thread manager: {e:?}"))?
        // No arena and no memory pattern (as for the embedder): activations
        // are freed after each page instead of staying resident at the size
        // of the largest page seen.
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| anyhow!("ONNX CPU execution provider: {e:?}"))?
        .with_memory_pattern(false)
        .map_err(|e| anyhow!("ONNX memory pattern: {e:?}"))?
        .commit_from_memory(bytes)
        .map_err(|e| anyhow!("loading the ONNX model: {e:?}"))
}

/// The loaded detector and recogniser. Inference takes `&mut` on a session,
/// so each sits behind a mutex; pages are read one at a time.
pub struct PpOcr {
    det: Mutex<Session>,
    rec: Mutex<Session>,
    charset: Charset,
    params: DetParams,
    dir: PathBuf,
}

impl std::fmt::Debug for PpOcr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PpOcr")
            .field("dir", &self.dir)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl PpOcr {
    /// Loads the model set from `dir` (the `pp-ocrv6` directory of the model
    /// root) with the default settings.
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_with(dir, OcrSettings::default())
    }

    /// Loads the model set from `dir` with `settings`.
    pub fn load_with(dir: &Path, settings: OcrSettings) -> Result<Self> {
        if !is_installed(dir) {
            bail!("the OCR model is not installed in {}", dir.display());
        }
        let read = |file: &str| {
            std::fs::read(dir.join(file))
                .with_context(|| format!("reading {}", dir.join(file).display()))
        };
        let text = |file: &str| -> Result<String> {
            String::from_utf8(read(file)?).with_context(|| format!("{file} is not UTF-8"))
        };
        let params = det_params(&text(DET_CONFIG_FILE)?, settings.det_band_pixels)?;
        let charset = Charset::from_inference_yml(&text(REC_CONFIG_FILE)?)?;
        let threads = settings.threads.max(1);
        let det = session(&read(DET_MODEL_FILE)?, threads).context("loading the OCR detector")?;
        let rec = session(&read(REC_MODEL_FILE)?, threads).context("loading the OCR recogniser")?;
        Ok(Self {
            det: Mutex::new(det),
            rec: Mutex::new(rec),
            charset,
            params,
            dir: dir.to_path_buf(),
        })
    }

    /// Text line boxes on `img`, in page pixels: the page detected band by
    /// band (see [`det::bands`]), at most [`det::MAX_WIDTH`] wide.
    pub fn detect(&self, img: &RgbImage) -> Result<Vec<DetBox>> {
        let scaled;
        let (page, scale) = if img.width() > det::MAX_WIDTH {
            let s = det::MAX_WIDTH as f32 / img.width() as f32;
            let h = ((img.height() as f32 * s).round() as u32).max(1);
            scaled = image::imageops::resize(
                img,
                det::MAX_WIDTH,
                h,
                image::imageops::FilterType::Triangle,
            );
            (&scaled, s)
        } else {
            (img, 1.0)
        };
        let mut boxes = Vec::new();
        for band in det::bands(page.width(), page.height(), self.params.band_pixels) {
            let view =
                image::imageops::crop_imm(page, 0, band.top, page.width(), band.height).to_image();
            for b in self.detect_band(&view)? {
                let quad = Quad(
                    b.quad
                        .0
                        .map(|p| geometry::Point::new(p.x, p.y + band.top as f32)),
                );
                if band.owns(quad.center().y) {
                    boxes.push(DetBox {
                        quad: quad.scaled(1.0 / scale, 1.0 / scale),
                        score: b.score,
                    });
                }
            }
        }
        Ok(boxes)
    }

    /// Text line boxes on one band, in band pixels.
    fn detect_band(&self, band: &RgbImage) -> Result<Vec<DetBox>> {
        let (data, w, h) = det::prepare(band);
        let input = Value::from_array((vec![1usize, 3, h as usize, w as usize], data))
            .map_err(|e| anyhow!("detector input: {e:?}"))?;
        let mut session = self.det.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session
            .run(ort::inputs!["x" => input])
            .map_err(|e| anyhow!("text detection failed: {e:?}"))?;
        let (shape, prob) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("detector output: {e:?}"))?;
        if shape.len() != 4 || shape[2] as u32 != h || shape[3] as u32 != w {
            bail!("detector output has shape {shape:?} for a {w} x {h} input");
        }
        Ok(det::postprocess(
            prob,
            w as usize,
            h as usize,
            band.dimensions(),
            &self.params,
        ))
    }

    /// Recognises `crops` (straightened lines), in order.
    pub fn recognize(&self, crops: &[RgbImage]) -> Result<Vec<Recognized>> {
        let widths: Vec<u32> = crops.iter().map(rec::scaled_width).collect();
        let mut out: Vec<Option<Recognized>> = vec![None; crops.len()];
        let mut session = self.rec.lock().unwrap_or_else(|e| e.into_inner());
        for batch in rec::batches(&widths, REC_BATCH, REC_WIDTH_BUDGET) {
            let refs: Vec<&RgbImage> = batch.iter().map(|&i| &crops[i]).collect();
            let (data, width) = rec::batch_input(&refs);
            let input = Value::from_array((
                vec![refs.len(), 3, rec::REC_HEIGHT as usize, width as usize],
                data,
            ))
            .map_err(|e| anyhow!("recogniser input: {e:?}"))?;
            let outputs = session
                .run(ort::inputs!["x" => input])
                .map_err(|e| anyhow!("text recognition failed: {e:?}"))?;
            let (shape, probs) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("recogniser output: {e:?}"))?;
            if shape.len() != 3 || shape[0] as usize != refs.len() {
                bail!(
                    "recogniser output has shape {shape:?} for {} lines",
                    refs.len()
                );
            }
            if shape[2] as usize != self.charset.len() {
                bail!(
                    "the recogniser has {} classes but its dictionary gives {}",
                    shape[2],
                    self.charset.len()
                );
            }
            let steps = shape[1] as usize;
            let per_line = steps * self.charset.len();
            for (k, &i) in batch.iter().enumerate() {
                let line = &probs[k * per_line..(k + 1) * per_line];
                out[i] = Some(rec::ctc_decode(line, steps, &self.charset));
            }
        }
        Ok(out.into_iter().flatten().collect())
    }

    /// Reads one page image: detection, crops, recognition; lines below
    /// [`MIN_LINE_SCORE`] or without text are dropped.
    pub fn read_page(&self, img: &RgbImage) -> Result<OcrPage> {
        let boxes = self.detect(img)?;
        let (quads, crops): (Vec<Quad>, Vec<RgbImage>) = boxes
            .iter()
            .filter_map(|b| geometry::crop_quad(img, &b.quad).map(|c| (b.quad, c)))
            .unzip();
        let texts = self.recognize(&crops)?;
        let lines = quads
            .into_iter()
            .zip(texts)
            .filter(|(_, r)| r.score >= MIN_LINE_SCORE && !r.text.trim().is_empty())
            .map(|(quad, r)| OcrLine {
                quad,
                text: r.text.trim().to_string(),
                score: r.score,
            })
            .collect();
        Ok(OcrPage {
            width: img.width(),
            height: img.height(),
            lines,
        })
    }
}

/// Each page of the PDF in `bytes` rendered at `dpi` and read by `model`,
/// in order. `on_page(page, pages)` is called before each page (1-based). A
/// page that cannot be rendered or read is logged and reads as empty.
pub fn read_pdf(
    model: &PpOcr,
    bytes: &[u8],
    dpi: u32,
    path: &Path,
    on_page: &dyn Fn(usize, usize),
) -> Result<Vec<String>> {
    let pages = raster::PdfPages::open(bytes)?;
    let mut texts = Vec::with_capacity(pages.count());
    for index in 0..pages.count() {
        on_page(index + 1, pages.count());
        match pages
            .render(index, dpi)
            .and_then(|img| model.read_page(&img))
        {
            Ok(page) => texts.push(page.text()),
            Err(e) => {
                tracing::warn!(
                    page = index + 1,
                    error = %format!("{e:#}"),
                    "OCR skipped a page: {}",
                    path.display()
                );
                texts.push(String::new());
            }
        }
    }
    Ok(texts)
}

/// The image file at `path` read by `model`.
pub fn read_image(model: &PpOcr, path: &Path) -> Result<String> {
    let img = raster::load_image(path)?;
    Ok(model.read_page(&img)?.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DET_YML: &str = "Global:\n  model_name: PP-OCRv6_small_det\nPostProcess:\n  box_thresh: 0.45\n  max_candidates: 3000\n  name: DBPostProcess\n  thresh: 0.2\n  unclip_ratio: 1.4\nPreProcess:\n  transform_ops:\n  - DecodeImage:\n      channel_first: false\n";

    #[test]
    fn detector_settings_come_from_its_inference_yml() {
        let p = det_params(DET_YML, 1600).unwrap();
        assert_eq!(
            p,
            DetParams {
                thresh: 0.2,
                box_thresh: 0.45,
                unclip_ratio: 1.4,
                max_candidates: 3000,
                band_pixels: 1600,
            }
        );
        assert!(det_params("PostProcess:\n  name: CTCLabelDecode\n", 1600).is_err());
        assert!(det_params("PostProcess:\n  name: DBPostProcess\n  thresh: x\n", 1600).is_err());
    }

    #[test]
    fn a_missing_file_means_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        for f in [DET_MODEL_FILE, DET_CONFIG_FILE, REC_MODEL_FILE] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        assert!(!is_installed(dir.path()));
        assert!(PpOcr::load(dir.path()).is_err());
        std::fs::write(dir.path().join(REC_CONFIG_FILE), b"x").unwrap();
        assert!(is_installed(dir.path()));
    }
}
