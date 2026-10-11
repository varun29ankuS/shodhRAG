//! OCR text quality, time and memory on a scanned corpus, engine by engine
//! (`shodh-eval ocr`). Retrieval metrics say whether misreadings cost
//! answers; this says how much text each engine gets right, so engines,
//! model tiers and render resolutions can be compared directly. Reported
//! only, never gated.
//!
//! Per page: `line_recall` (share of the ground truth's lines found whole,
//! as the retrieval passages must be), `word_recall` (share of its words
//! found, as a multiset) and `cer` (character error rate: edit distance over
//! the ground truth's length), all on normalised text ([`normalize_text`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::text::{contains_span, normalize_text};

/// Which engine reads the pages.
#[derive(Debug, Clone, PartialEq)]
pub enum Engine {
    /// PP-OCRv6 from a model directory, run with `settings` (detector band
    /// size, threads).
    PpOcr {
        dir: PathBuf,
        settings: shodh_rag::processing::ocr::OcrSettings,
        /// Read in the OCR child process, as the app does (this executable
        /// started as the child; it loads the model with the default
        /// settings, and memory is the child's, sampled by the caller).
        worker: bool,
    },
    /// Windows.Media.Ocr; pages rendered at `dpi` (`None`: 96 dpi page size).
    System,
}

pub struct OcrOptions {
    /// Absolute corpus folder.
    pub corpus: PathBuf,
    /// Scanned file -> text by page (`ground-truth.json`).
    pub truth: BTreeMap<String, Vec<String>>,
    pub engine: Engine,
    /// Render resolution for PDF pages; for the system OCR `None` keeps its
    /// default (96 dpi).
    pub dpi: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageScore {
    pub file: String,
    pub page: usize,
    pub line_recall: f64,
    pub word_recall: f64,
    pub cer: f64,
    /// Render plus recognition, milliseconds (for the system OCR on a PDF,
    /// the file's time divided by its pages).
    pub ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OcrReport {
    pub engine: String,
    pub dpi: String,
    pub pages: Vec<PageScore>,
    pub mean_line_recall: f64,
    pub mean_word_recall: f64,
    pub mean_cer: f64,
    pub mean_ms_per_page: f64,
    /// Model load time (PP-OCRv6), milliseconds.
    pub load_ms: Option<f64>,
    /// Process working set before loading the engine, after loading it and
    /// the peak over the run, in MB (Windows only).
    pub memory_mb: Option<MemoryUse>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MemoryUse {
    pub before: f64,
    pub loaded: f64,
    pub peak: f64,
}

/// Levenshtein distance over characters.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(ca != cb);
            row[j + 1] = substitute.min(prev[j + 1] + 1).min(row[j] + 1);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

/// Scores OCR text against the ground truth of one page: (line recall,
/// word recall, character error rate).
pub fn score_page(truth: &str, ocr: &str) -> (f64, f64, f64) {
    let lines: Vec<&str> = truth.lines().filter(|l| !l.trim().is_empty()).collect();
    let found = lines.iter().filter(|l| contains_span(ocr, l)).count();
    let line_recall = if lines.is_empty() {
        1.0
    } else {
        found as f64 / lines.len() as f64
    };
    let (t, o) = (normalize_text(truth), normalize_text(ocr));
    let mut available: BTreeMap<&str, usize> = BTreeMap::new();
    for w in o.split(' ').filter(|w| !w.is_empty()) {
        *available.entry(w).or_default() += 1;
    }
    let words: Vec<&str> = t.split(' ').filter(|w| !w.is_empty()).collect();
    let mut hits = 0usize;
    for w in &words {
        if let Some(n) = available.get_mut(w) {
            if *n > 0 {
                *n -= 1;
                hits += 1;
            }
        }
    }
    let word_recall = if words.is_empty() {
        1.0
    } else {
        hits as f64 / words.len() as f64
    };
    let cer = if t.is_empty() {
        0.0
    } else {
        edit_distance(&t, &o) as f64 / t.chars().count() as f64
    };
    (line_recall, word_recall, cer)
}

/// Working set and peak working set of this process, in MB.
#[cfg(windows)]
pub fn process_memory_mb() -> Option<(f64, f64)> {
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: the pseudo handle of the current process is always valid and
    // `counters` is a correctly sized PROCESS_MEMORY_COUNTERS.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    let mb = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
    (ok != 0).then(|| {
        (
            mb(counters.working_set_size),
            mb(counters.peak_working_set_size),
        )
    })
}

#[cfg(not(windows))]
pub fn process_memory_mb() -> Option<(f64, f64)> {
    None
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

/// The text of every page of `file` with the chosen engine, and the time per page.
fn read_file(engine: &EngineHandle, path: &Path, dpi: Option<u32>) -> Result<Vec<(String, f64)>> {
    let is_pdf = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    match engine {
        EngineHandle::Worker(worker) => {
            use shodh_rag::processing::ocr::worker::{FileKind, WorkerEngine};
            let kind = if is_pdf {
                FileKind::Pdf
            } else {
                FileKind::Image
            };
            let started = Instant::now();
            let texts = worker.read(
                kind,
                path,
                WorkerEngine::PpOcr,
                dpi.unwrap_or(shodh_rag::processing::ocr::raster::RENDER_DPI),
                None,
                &|_, _| {},
            )?;
            let ms = started.elapsed().as_secs_f64() * 1000.0 / texts.len().max(1) as f64;
            Ok(texts.into_iter().map(|t| (t, ms)).collect())
        }
        EngineHandle::PpOcr(model) => {
            use shodh_rag::processing::ocr::raster;
            if !is_pdf {
                let started = Instant::now();
                let img = raster::load_image(path)?;
                let text = model.read_page(&img)?.text();
                return Ok(vec![(text, started.elapsed().as_secs_f64() * 1000.0)]);
            }
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let pages = raster::PdfPages::open(&bytes)?;
            let dpi = dpi.unwrap_or(raster::RENDER_DPI);
            (0..pages.count())
                .map(|i| {
                    let started = Instant::now();
                    let img = pages.render(i, dpi)?;
                    let text = model.read_page(&img)?.text();
                    Ok((text, started.elapsed().as_secs_f64() * 1000.0))
                })
                .collect()
        }
        #[cfg(windows)]
        EngineHandle::System => {
            use shodh_rag::processing::windows_ocr;
            let started = Instant::now();
            let texts = if is_pdf {
                windows_ocr::ocr_pdf_pages(path, dpi, &|_, _| {})?
            } else {
                vec![windows_ocr::ocr_image(path)?]
            };
            let ms = started.elapsed().as_secs_f64() * 1000.0 / texts.len().max(1) as f64;
            Ok(texts.into_iter().map(|t| (t, ms)).collect())
        }
    }
}

enum EngineHandle {
    PpOcr(Box<shodh_rag::processing::ocr::PpOcr>),
    Worker(Box<shodh_rag::processing::ocr::worker::OcrWorker>),
    #[cfg(windows)]
    System,
}

pub fn run(opts: &OcrOptions) -> Result<OcrReport> {
    let before = process_memory_mb();
    let started = Instant::now();
    let (engine, name, load_ms) = match &opts.engine {
        Engine::PpOcr {
            dir, worker: true, ..
        } => {
            use shodh_rag::processing::ocr::worker::{OcrWorker, WorkerCommand};
            let worker = OcrWorker::new(WorkerCommand::current_exe()?);
            worker.set_model_dir(Some(dir.clone()));
            (
                EngineHandle::Worker(Box::new(worker)),
                format!("ppocr in the OCR child process ({})", dir.display()),
                None,
            )
        }
        Engine::PpOcr { dir, settings, .. } => {
            let model = shodh_rag::processing::ocr::PpOcr::load_with(dir, *settings)?;
            (
                EngineHandle::PpOcr(Box::new(model)),
                format!(
                    "ppocr ({}, detector bands <= {} px, {} threads, below-normal priority)",
                    dir.display(),
                    settings.det_band_pixels,
                    settings.threads
                ),
                Some(started.elapsed().as_secs_f64() * 1000.0),
            )
        }
        #[cfg(windows)]
        Engine::System => (EngineHandle::System, "windows-ocr".to_string(), None),
        #[cfg(not(windows))]
        Engine::System => bail!("the system OCR exists only on Windows"),
    };
    let loaded = process_memory_mb();
    // Read as the app reads scans: this thread below normal priority.
    let _below_normal = shodh_rag::processing::ocr::priority::BelowNormal::new();
    let mut pages = Vec::new();
    for (file, truth_pages) in &opts.truth {
        let path = file.split('/').fold(opts.corpus.clone(), |p, s| p.join(s));
        let read =
            read_file(&engine, &path, opts.dpi).with_context(|| format!("reading {file}"))?;
        if read.len() != truth_pages.len() {
            bail!(
                "{file}: OCR returned {} pages, the ground truth has {}",
                read.len(),
                truth_pages.len()
            );
        }
        for (i, ((text, ms), truth)) in read.iter().zip(truth_pages).enumerate() {
            let (line_recall, word_recall, cer) = score_page(truth, text);
            eprintln!(
                "{file} p{}: lines {line_recall:.3} words {word_recall:.3} cer {cer:.4} {ms:.0} ms",
                i + 1
            );
            pages.push(PageScore {
                file: file.clone(),
                page: i + 1,
                line_recall,
                word_recall,
                cer,
                ms: *ms,
            });
        }
    }
    let after = process_memory_mb();
    let memory_mb = match (before, loaded, after) {
        (Some(b), Some(l), Some(a)) => Some(MemoryUse {
            before: b.0,
            loaded: l.0,
            peak: a.1,
        }),
        _ => None,
    };
    Ok(OcrReport {
        engine: name,
        dpi: opts
            .dpi
            .map_or_else(|| "default".to_string(), |d| d.to_string()),
        mean_line_recall: mean(pages.iter().map(|p| p.line_recall)),
        mean_word_recall: mean(pages.iter().map(|p| p.word_recall)),
        mean_cer: mean(pages.iter().map(|p| p.cer)),
        mean_ms_per_page: mean(pages.iter().map(|p| p.ms)),
        pages,
        load_ms,
        memory_mb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_counts_insertions_deletions_and_substitutions() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("3.Fees", "3. Fees"), 1);
    }

    #[test]
    fn pages_are_scored_on_normalised_text() {
        let truth = "TAX INVOICE\nInvoice No: INV-2025-0117\nTotal amount due: INR 7,96,500.00";
        let (lines, words, cer) = score_page(truth, truth);
        assert_eq!((lines, words, cer), (1.0, 1.0, 0.0));
        // Punctuation and grouping do not count; a lost line and a misread word do.
        let ocr = "tax invoice\nInvoice No INV-2025-0117\nTotal amount due: INR 7,96,5OO";
        let (lines, words, cer) = score_page(truth, ocr);
        assert!((lines - 2.0 / 3.0).abs() < 1e-9, "{lines}");
        assert!(words < 1.0 && words > 0.8, "{words}");
        assert!(cer > 0.0 && cer < 0.1, "{cer}");
    }
}
