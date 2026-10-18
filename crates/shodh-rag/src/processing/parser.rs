use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::document_model::{Block, BlockKind, StructuredDocument};
use super::ocr::cache::{CachedScan, FileStamp, OcrCache};
use super::ocr::worker::{FileKind, OcrWorker, WorkerEngine};
use super::ocr::{ScanReaderMissing, SharedOcrModel};
use super::pdf_layout::{TableMode, TableReport};
use super::table_model::{TableModel, TABLE_MODEL_ID};
use super::{limits, office_xml, pdf_forms, pdf_layout, tabular, text_structure};
use crate::config::{OcrConfig, OcrEngineChoice};
use crate::types::{DocumentFormat, DocumentSection};

#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub content: String,
    pub title: String,
    pub metadata: HashMap<String, String>,
    pub format: DocumentFormat,
    /// Structured sections for PDFs with forms/tables. Empty for plain text formats.
    pub structured_sections: Vec<DocumentSection>,
    /// Semantic blocks (headings, paragraphs, tables, theorems, ...) with
    /// pages and bounding boxes, for formats the structure parsers handle
    /// (PDF, LaTeX, Markdown). `None` for other formats and for PDFs that
    /// only the unpaged fallback extractor could read.
    pub document: Option<StructuredDocument>,
}

/// Metadata key listing a PDF's table-candidate pages (comma-separated, 1-based).
pub const TABLE_CANDIDATES_KEY: &str = "table_candidate_pages";
/// Metadata key naming the table model whose structure a PDF's tables carry.
pub const TABLE_MODEL_KEY: &str = "table_model";
/// Metadata key counting the tables the model structured.
pub const MODEL_TABLES_KEY: &str = "model_tables";
/// Metadata key naming the OCR engine that read a scanned PDF or an image
/// ([`OCR_ENGINE_PPOCR`] or [`OCR_ENGINE_SYSTEM`]).
pub const OCR_ENGINE_KEY: &str = "ocr_engine";
/// [`OCR_ENGINE_KEY`] value: the PP-OCRv6 model ([`super::ocr::OCR_MODEL_ID`]).
pub const OCR_ENGINE_PPOCR: &str = super::ocr::OCR_MODEL_ID;
/// [`OCR_ENGINE_KEY`] value: Windows.Media.Ocr.
pub const OCR_ENGINE_SYSTEM: &str = "windows-ocr";
/// Metadata key set (`"true"`) on a scan the system OCR read only because the
/// scan reader (the PP-OCRv6 model) is not installed: it is read again once
/// the reader is installed.
pub const OCR_READER_MISSING_KEY: &str = "ocr_reader_missing";
/// Metadata key listing the pages (1-based, comma-separated) of a scan that
/// OCR could not read: they read as empty ([`ocr_failed_pages_notice`]).
pub const OCR_FAILED_PAGES_KEY: &str = "ocr_failed_pages";

/// A PDF that needs a password to open: its text cannot be read, by OCR
/// either, so it is not treated as a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("this PDF is protected with a password, so its text cannot be read")]
pub struct PdfEncrypted;

/// A PDF that no reader here could open and OCR could not read: damaged,
/// incomplete (a partial download) or not a PDF at all. Carries what the
/// PDF reader said.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("this PDF is damaged or incomplete ({0})")]
pub struct PdfDamaged(pub String);

/// Called with (page being read, pages) while OCR reads a scanned PDF, before
/// each page (1-based).
pub type OcrProgress<'a> = &'a dyn Fn(usize, usize);

/// A parsed PDF: text, structured blocks when the layout parser or OCR read
/// it, the table detection, and how OCR read a scan.
struct PdfParse {
    text: String,
    document: Option<StructuredDocument>,
    tables: Option<TableReport>,
    ocr: Option<OcrUse>,
    /// Pages (1-based) of a scan that OCR could not read.
    ocr_failed_pages: Vec<usize>,
}

/// A scanned PDF as OCR read it: its pages as a document, the engine that
/// read it, its page count, and the pages (1-based) OCR could not read.
struct ScanRead {
    doc: StructuredDocument,
    engine: &'static str,
    pages: usize,
    failed: Vec<usize>,
}

impl ScanRead {
    fn of(pages: super::ocr::OcrPages, engine: &'static str) -> Self {
        Self {
            doc: ocr_pages_to_document(&pages.texts),
            engine,
            pages: pages.texts.len(),
            failed: pages.failed,
        }
    }
}

/// How a scan was read: the engine, and whether the scan reader was missing
/// (so the system OCR read it instead).
#[derive(Debug, Clone, Copy)]
struct OcrUse {
    engine: &'static str,
    reader_missing: bool,
}

/// The OCR engine a scanned page goes to.
enum OcrRoute {
    /// PP-OCRv6 in this process.
    PpOcr(Arc<super::ocr::PpOcr>),
    /// The system OCR in this process.
    System,
    /// An engine in the OCR child process ([`OcrWorker`]).
    Worker(WorkerEngine),
    None,
}

/// The [`OCR_ENGINE_KEY`] value of an engine in the OCR child.
fn worker_engine_name(engine: WorkerEngine) -> &'static str {
    match engine {
        WorkerEngine::PpOcr => OCR_ENGINE_PPOCR,
        WorkerEngine::System => OCR_ENGINE_SYSTEM,
    }
}

/// Which engine reads a scan, decided before any model is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OcrEngine {
    PpOcr,
    System,
    None,
}

/// The engine for a scan and whether the scan reader is missing for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OcrDecision {
    engine: OcrEngine,
    /// The scan reader would read this scan but is not installed.
    reader_missing: bool,
}

/// Chooses the engine for a scan (see [`OcrEngineChoice`]). On `Auto` the scan
/// reader (PP-OCRv6) reads scans when it is installed, on every platform;
/// without it Windows reads with the system OCR and other platforms cannot
/// read. Scans whose images are JBIG2 or JPEG 2000 (`unrendered_codecs`),
/// which the pure-Rust renderer draws blank, go to the system OCR where there
/// is one, whatever the choice. `system_available` is asked only when the
/// answer matters.
fn decide_ocr(
    choice: OcrEngineChoice,
    reader_installed: bool,
    system_available: impl FnOnce() -> bool,
    unrendered_codecs: bool,
) -> OcrDecision {
    let decision = |engine, reader_missing| OcrDecision {
        engine,
        reader_missing,
    };
    let system = |available: bool| {
        if available {
            OcrEngine::System
        } else {
            OcrEngine::None
        }
    };
    match choice {
        OcrEngineChoice::System => decision(system(system_available()), false),
        OcrEngineChoice::PpOcr | OcrEngineChoice::Auto => {
            let system_available = std::cell::LazyCell::new(system_available);
            if unrendered_codecs && *system_available {
                decision(OcrEngine::System, false)
            } else if reader_installed {
                decision(OcrEngine::PpOcr, false)
            } else if choice == OcrEngineChoice::Auto {
                decision(system(*system_available), true)
            } else {
                decision(OcrEngine::None, true)
            }
        }
    }
}

/// Parses files into text, sections and structured documents.
///
/// PDF tables come from the layout heuristics; a parser built
/// [`with_table_model`](Self::with_table_model) structures the tables of the
/// table-candidate pages with the model instead. The fast path records the
/// candidate pages in the metadata ([`TABLE_CANDIDATES_KEY`]) so a caller can
/// refine those files later.
#[derive(Debug, Clone, Default)]
pub struct DocumentParser {
    table_model: Option<Arc<TableModel>>,
    ocr_model: Option<SharedOcrModel>,
    /// When set, OCR runs in the child process instead of this one (the
    /// in-process model is then not used).
    ocr_worker: Option<Arc<OcrWorker>>,
    /// What OCR read from scans, reused while a file is unchanged.
    ocr_cache: Option<Arc<OcrCache>>,
    ocr: OcrConfig,
}

impl DocumentParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// A parser that structures PDF tables with `model` on candidate pages.
    pub fn with_table_model(model: Arc<TableModel>) -> Self {
        Self {
            table_model: Some(model),
            ..Self::default()
        }
    }

    /// This parser reading scans and images with `model` (when installed)
    /// and the system OCR as `config` chooses.
    pub fn with_ocr(mut self, model: SharedOcrModel, config: OcrConfig) -> Self {
        self.ocr_model = Some(model);
        self.ocr = config;
        self
    }

    /// This parser reading scans and images in the OCR child process
    /// `worker` (see [`super::ocr::worker`]); the model it reads with is the
    /// worker's ([`OcrWorker::model_dir`]).
    pub fn with_ocr_worker(mut self, worker: Arc<OcrWorker>) -> Self {
        self.ocr_worker = Some(worker);
        self
    }

    /// This parser keeping what OCR reads in `cache` and reading scans from
    /// it while they are unchanged (see [`super::ocr::cache`]).
    pub fn with_ocr_cache(mut self, cache: Arc<OcrCache>) -> Self {
        self.ocr_cache = Some(cache);
        self
    }

    /// Whether the scan reader (the PP-OCRv6 model) is installed.
    fn reader_installed(&self) -> bool {
        match &self.ocr_worker {
            Some(worker) => worker.model_dir().is_some(),
            None => self.ocr_model.as_ref().is_some_and(|m| m.available()),
        }
    }

    /// The engine for a scan ([`decide_ocr`]) and whether the scan reader is
    /// missing for it. The model is loaded (seconds, on first use) only when
    /// it reads; one that is installed but fails to load is logged and the
    /// scan goes where it would go without it.
    fn ocr_route(&self, unrendered_codecs: bool) -> (OcrRoute, bool) {
        if let Some(worker) = &self.ocr_worker {
            // Windows OCR is asked for in the child only (no WinRT in this
            // process); without an OCR language the child says so.
            let decision = decide_ocr(
                self.ocr.engine,
                worker.model_dir().is_some(),
                || cfg!(windows),
                unrendered_codecs,
            );
            let route = match decision.engine {
                OcrEngine::PpOcr => OcrRoute::Worker(WorkerEngine::PpOcr),
                OcrEngine::System => OcrRoute::Worker(WorkerEngine::System),
                OcrEngine::None => OcrRoute::None,
            };
            return (route, decision.reader_missing);
        }
        let installed = self.ocr_model.as_ref().is_some_and(|m| m.available());
        let decision = decide_ocr(
            self.ocr.engine,
            installed,
            system_ocr_available,
            unrendered_codecs,
        );
        let route = |engine| match engine {
            OcrEngine::System => OcrRoute::System,
            _ => OcrRoute::None,
        };
        match decision.engine {
            OcrEngine::PpOcr => match self.ocr_model.as_ref().and_then(|m| m.get()) {
                Some(model) => (OcrRoute::PpOcr(model), false),
                None => {
                    let fallback = decide_ocr(
                        self.ocr.engine,
                        false,
                        system_ocr_available,
                        unrendered_codecs,
                    );
                    (route(fallback.engine), false)
                }
            },
            engine => (route(engine), decision.reader_missing),
        }
    }

    /// A scanned PDF read by OCR page by page (at below-normal priority),
    /// `None` when no engine could read it; and whether the scan reader was
    /// missing for it.
    fn ocr_pdf(
        &self,
        path: &Path,
        bytes: &[u8],
        progress: OcrProgress<'_>,
    ) -> (Result<Option<ScanRead>>, bool) {
        let unrendered = super::ocr::raster::uses_unrendered_codecs(bytes);
        let (route, reader_missing) = self.ocr_route(unrendered);
        let _below_normal = super::ocr::priority::BelowNormal::new();
        (
            self.read_scan(route, path, bytes, unrendered, progress),
            reader_missing,
        )
    }

    fn read_scan(
        &self,
        route: OcrRoute,
        path: &Path,
        bytes: &[u8],
        unrendered: bool,
        progress: OcrProgress<'_>,
    ) -> Result<Option<ScanRead>> {
        let read = match route {
            OcrRoute::PpOcr(model) => {
                if unrendered {
                    tracing::warn!(
                        "Scan uses JBIG2 or JPEG 2000 images, which cannot be rendered here; \
                         those pages read as empty: {}",
                        path.display()
                    );
                }
                let pages =
                    super::ocr::read_pdf(&model, bytes, self.ocr.render_dpi, path, progress)?;
                Some(ScanRead::of(pages, OCR_ENGINE_PPOCR))
            }
            #[cfg(windows)]
            OcrRoute::System => {
                if unrendered {
                    tracing::info!(
                        "Scan uses JBIG2 or JPEG 2000 images; reading it with the system OCR: {}",
                        path.display()
                    );
                }
                let pages =
                    super::windows_ocr::ocr_pdf_pages(path, self.ocr.system_render_dpi, progress)?;
                Some(ScanRead::of(pages, OCR_ENGINE_SYSTEM))
            }
            #[cfg(not(windows))]
            OcrRoute::System => None,
            OcrRoute::Worker(engine) => {
                let worker = self
                    .ocr_worker
                    .as_ref()
                    .context("no OCR process is configured")?;
                let pages = worker.read_pages(
                    FileKind::Pdf,
                    path,
                    engine,
                    self.ocr.render_dpi,
                    self.ocr.system_render_dpi,
                    progress,
                )?;
                Some(ScanRead::of(pages, worker_engine_name(engine)))
            }
            OcrRoute::None => None,
        };
        Ok(read)
    }

    pub fn parse_file(&self, path: &Path) -> Result<ParsedDocument> {
        self.parse_file_with_progress(path, &|_, _| {})
    }

    /// [`Self::parse_file`], reporting OCR's progress through a scanned PDF.
    /// With a cache ([`Self::with_ocr_cache`]), a scan read before and
    /// unchanged since is taken from it without OCR, and a scan read now is
    /// put in it.
    pub fn parse_file_with_progress(
        &self,
        path: &Path,
        progress: OcrProgress<'_>,
    ) -> Result<ParsedDocument> {
        let cached = self
            .ocr_cache
            .as_ref()
            .filter(|_| is_ocr_candidate(path))
            .and_then(|cache| {
                let stamp = FileStamp::of(path).ok()?;
                Some((cache, crate::rag_engine::normalize_source_path(path), stamp))
            });
        if let Some((cache, source, stamp)) = &cached {
            if let Some(entry) = cache.get(source) {
                let system_only = self.ocr.engine == OcrEngineChoice::System;
                if entry.is_current_for(*stamp, system_only, self.reader_installed()) {
                    tracing::debug!("Scan read from the OCR cache: {}", path.display());
                    return Ok(entry.into_parsed());
                }
            }
        }
        let parsed = self.parse_uncached(path, progress)?;
        if let Some((cache, source, stamp)) = cached {
            if let Some(entry) = CachedScan::of(&source, stamp, &parsed) {
                if let Err(e) = cache.put(&entry) {
                    tracing::warn!(error = %format!("{e:#}"), "OCR result not cached: {}", path.display());
                }
            }
        }
        Ok(parsed)
    }

    fn parse_uncached(&self, path: &Path, progress: OcrProgress<'_>) -> Result<ParsedDocument> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt")
            .to_lowercase();

        let format = DocumentFormat::from_extension(&extension);
        // Use file stem (without extension) for a cleaner display title
        let stem = path.file_stem().and_then(|n| n.to_str());
        let mut title = stem.unwrap_or("untitled").to_string();

        let mut metadata = HashMap::new();
        metadata.insert("file_path".to_string(), path.display().to_string());
        metadata.insert("file_extension".to_string(), extension.clone());

        if let Ok(meta) = std::fs::metadata(path) {
            // Refused before it is read: see [`limits`].
            limits::check_file_size(&extension, meta.len())
                .map_err(|e| anyhow::Error::new(e).context(path.display().to_string()))?;
            metadata.insert("file_size".to_string(), meta.len().to_string());
        }

        // Tabular formats produce typed tables first; their flat text is derived
        // from the same tables so the file is read exactly once.
        let tables = if tabular::is_spreadsheet_extension(&extension) {
            Some(tabular::read_spreadsheet(path)?)
        } else if tabular::is_delimited_extension(&extension) {
            Some(vec![tabular::read_delimited_file(path, &extension)?])
        } else {
            None
        };

        let mut document: Option<StructuredDocument> = None;
        let mut docx_sections = Vec::new();
        let content = match &tables {
            Some(tables) => tabular::tables_to_text(tables),
            None => match extension.as_str() {
                "pdf" => {
                    let parsed = self.parse_pdf(path, progress)?;
                    document = parsed.document;
                    if let Some(report) = parsed.tables {
                        record_table_report(&report, self.table_model.is_some(), &mut metadata);
                    }
                    if let Some(ocr) = parsed.ocr {
                        record_ocr(ocr, &mut metadata);
                    }
                    if !parsed.ocr_failed_pages.is_empty() {
                        let pages: Vec<String> = parsed
                            .ocr_failed_pages
                            .iter()
                            .map(|p| p.to_string())
                            .collect();
                        metadata.insert(OCR_FAILED_PAGES_KEY.to_string(), pages.join(","));
                    }
                    parsed.text
                }
                "tex" | "latex" => {
                    let raw = tabular::read_text_file(path)?;
                    let doc = text_structure::parse_latex(&raw);
                    let text = doc.plain_text();
                    document = Some(doc);
                    text
                }
                "md" | "markdown" => {
                    let raw = tabular::read_text_file(path)?;
                    let doc = text_structure::parse_markdown(&raw);
                    document = Some(doc);
                    raw
                }
                "docx" => {
                    let (text, sections) = self.parse_docx(path)?;
                    docx_sections = sections;
                    text
                }
                "pptx" => self.parse_pptx(path)?,
                "html" | "htm" => self.parse_html(path)?,
                "png" | "jpg" | "jpeg" | "bmp" | "tiff" | "tif" => {
                    let (doc, ocr) = self.parse_image(path)?;
                    record_ocr(ocr, &mut metadata);
                    let text = doc.plain_text();
                    document = Some(doc);
                    text
                }
                _ => tabular::read_text_file(path)?,
            },
        };
        if let Some(doc) = &document {
            if !doc.pages.is_empty() {
                metadata.insert("page_count".to_string(), doc.pages.len().to_string());
            }
            // A file named by an id ("1706.03762v7.pdf") is titled by the
            // title printed in it.
            if let Some(printed) = document_title(doc) {
                if stem.is_none_or(is_identifier_stem) {
                    title = printed;
                }
            }
        }

        // Extract structured sections for formats with tabular/form data. A PDF
        // with a structured document carries its form fields as blocks; only a PDF
        // the layout parser could not read gets sections (page text and fields).
        let structured_sections = match tables {
            Some(tables) => {
                record_table_metadata(&tables, &mut metadata);
                tabular::tables_to_sections(tables)
            }
            None if format == DocumentFormat::PDF && document.is_none() => {
                self.extract_pdf_structure(path, &content)
            }
            None => docx_sections,
        };

        if !structured_sections.is_empty() {
            let field_count = structured_sections
                .iter()
                .filter(|s| matches!(s, DocumentSection::FormFields { .. }))
                .count();
            let table_count = structured_sections
                .iter()
                .filter(|s| matches!(s, DocumentSection::Table { .. }))
                .count();
            tracing::info!(
                sections = structured_sections.len(),
                form_field_groups = field_count,
                tables = table_count,
                "Structured extraction complete: {}",
                path.display()
            );
        }

        Ok(ParsedDocument {
            content,
            title,
            metadata,
            format,
            structured_sections,
            document,
        })
    }

    /// Parse a PDF into text plus, when the layout parser succeeds, its
    /// structured blocks.
    ///
    /// Order of attempts:
    /// 1. The layout parser ([`pdf_layout`]): paged, positioned blocks.
    /// 2. When the layout parser cannot open the file, `pdf_extract` text
    ///    (unpaged).
    /// 3. When the PDF has no text layer (a scan), OCR page by page
    ///    ([`Self::ocr_route`]).
    ///
    /// A PDF is treated as scanned only when its text layer is essentially
    /// empty across all pages ([`is_effectively_scanned`]), never because one
    /// extractor failed.
    fn parse_pdf(&self, path: &Path, progress: OcrProgress<'_>) -> Result<PdfParse> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read PDF: {}", path.display()))?;

        // Form fields and comments live outside the text layer; they are placed into
        // the structured document as blocks with their pages and boxes.
        // The form reader also says whether the PDF opens at all and whether
        // it needs a password, which decides the error when nothing reads it.
        let (forms, form_error) = match pdf_forms::extract_forms(&bytes) {
            Ok(forms) => (forms, None),
            Err(e) => {
                tracing::debug!(error = %e, "PDF form fields not read: {}", path.display());
                (pdf_forms::FormExtraction::default(), Some(e))
            }
        };
        let too_many_pages = |pages: usize| {
            limits::check_pdf_pages(pages)
                .map_err(|e| anyhow::Error::new(e).context(path.display().to_string()))
        };
        too_many_pages(forms.pages)?;
        if !forms.fields.is_empty() {
            tracing::info!(
                fields = forms.fields.len(),
                stats = ?forms.stats,
                "PDF form fields read: {}",
                path.display()
            );
        }
        let mode = match &self.table_model {
            Some(model) => TableMode::Model(model),
            None => TableMode::Candidates,
        };
        let mut layout_failed = false;
        // Pages the layout parser found (none when it could not read the file).
        let mut layout_pages = 0;
        match pdf_layout::parse_pdf_layout_with(&bytes, mode) {
            Ok(parsed) => {
                let mut doc = parsed.document;
                too_many_pages(doc.pages.len())?;
                for error in &parsed.tables.model_errors {
                    tracing::warn!(error = %error, "table model page skipped: {}", path.display());
                }
                let chars = doc.text_chars();
                layout_pages = doc.pages.len();
                if !is_effectively_scanned(chars, doc.pages.len()) {
                    insert_form_fields(&mut doc, &forms.fields);
                    return Ok(PdfParse {
                        text: doc.plain_text(),
                        document: Some(doc),
                        tables: Some(parsed.tables),
                        ocr: None,
                        ocr_failed_pages: Vec::new(),
                    });
                }
                tracing::info!(
                    pages = doc.pages.len(),
                    text_chars = chars,
                    "PDF has no text layer; trying OCR: {}",
                    path.display()
                );
            }
            Err(e) => {
                layout_failed = true;
                tracing::warn!(error = %e, "PDF layout parsing failed: {}", path.display());
            }
        }

        if layout_failed {
            if let Some(text) = pdf_extract_text(&bytes) {
                return Ok(PdfParse {
                    text,
                    document: None,
                    tables: None,
                    ocr: None,
                    ocr_failed_pages: Vec::new(),
                });
            }
        }
        // No text was read. A PDF locked with a password is not a scan: OCR
        // would render blank pages, and the scan reader would not help.
        if matches!(form_error, Some(pdf_forms::FormError::Encrypted)) {
            return Err(anyhow::Error::new(PdfEncrypted).context(path.display().to_string()));
        }
        // Neither PDF reader could open it: OCR is still tried (its renderer
        // may manage), and if it reads nothing the file is reported damaged
        // rather than as a scan.
        let damaged = match &form_error {
            Some(pdf_forms::FormError::Open(reason)) if layout_pages == 0 => Some(reason.clone()),
            _ => None,
        };

        let (read, reader_missing) = self.ocr_pdf(path, &bytes, progress);
        // Why OCR read nothing: its error, or the pages it could not read.
        let mut ocr_error = None;
        let mut unread = Vec::new();
        match read {
            Ok(Some(mut scan)) => {
                if scan.doc.text_chars() > 0 || !forms.fields.is_empty() {
                    insert_form_fields(&mut scan.doc, &forms.fields);
                    tracing::info!(
                        pages = scan.pages,
                        engine = scan.engine,
                        "Using OCR text: {}",
                        path.display()
                    );
                    if !scan.failed.is_empty() {
                        tracing::warn!(
                            "OCR could not read {}: {}",
                            page_list(&scan.failed),
                            path.display()
                        );
                    }
                    return Ok(PdfParse {
                        text: scan.doc.plain_text(),
                        document: Some(scan.doc),
                        tables: None,
                        ocr: Some(OcrUse {
                            engine: scan.engine,
                            reader_missing,
                        }),
                        ocr_failed_pages: scan.failed,
                    });
                }
                unread = scan.failed;
            }
            Ok(None) => {
                tracing::warn!("No OCR engine is available for {}", path.display());
            }
            Err(e) => {
                tracing::warn!("OCR failed for {}: {:#}", path.display(), e);
                ocr_error = Some(e);
            }
        }
        if let Some(reason) = damaged {
            return Err(anyhow::Error::new(PdfDamaged(reason)).context(path.display().to_string()));
        }
        // The scan reader may read what nothing here could.
        if reader_missing {
            return Err(anyhow::Error::new(ScanReaderMissing).context(path.display().to_string()));
        }
        if let Some(e) = ocr_error {
            return Err(e.context(format!(
                "OCR could not read this scanned PDF: {}",
                path.display()
            )));
        }
        if !unread.is_empty() {
            return Err(anyhow::anyhow!(
                "OCR could not read {} of this scanned PDF: {}",
                page_list(&unread),
                path.display()
            ));
        }

        Err(anyhow::anyhow!(
            "PDF contains no extractable text (scanned/image-based and OCR found no text): {}",
            path.display()
        ))
    }

    /// Sections of a PDF the layout parser could not read: its form fields and
    /// comments grouped by page, then its text from the fallback readers
    /// ([`fallback_text_sections`]); `fallback_content` is `pdf_extract`'s
    /// text of the whole document.
    fn extract_pdf_structure(&self, path: &Path, fallback_content: &str) -> Vec<DocumentSection> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::debug!("PDF re-read failed for {}: {}", path.display(), e);
                return Vec::new();
            }
        };
        let mut sections = Vec::new();
        if let Ok(forms) = pdf_forms::extract_forms(&bytes) {
            let mut by_page: std::collections::BTreeMap<usize, Vec<(String, String)>> =
                std::collections::BTreeMap::new();
            for field in forms.fields.iter().filter(|f| !f.value.trim().is_empty()) {
                by_page
                    .entry(field.page.map(|p| p as usize).unwrap_or(0))
                    .or_default()
                    .push((field.label.clone(), field.value.clone()));
            }
            for (page, fields) in by_page {
                sections.push(DocumentSection::FormFields { fields, page });
            }
        }
        let (lopdf_pages, page_count) = match super::lopdf_parser::LoPdfParser::parse_bytes(&bytes)
        {
            Ok(doc) => {
                let count = doc.pages.len();
                let pages: Vec<(usize, String)> = doc
                    .pages
                    .into_iter()
                    .map(|page| (page.page_number, page.text))
                    .collect();
                (pages, Some(count))
            }
            Err(e) => {
                tracing::debug!("lopdf extraction failed for {}: {}", path.display(), e);
                (Vec::new(), None)
            }
        };
        sections.extend(fallback_text_sections(
            &lopdf_pages,
            fallback_content,
            || page_count.and_then(|count| pdf_extract_pages(&bytes, count)),
        ));
        sections
    }

    /// The text of a DOCX and, when it has tables, its sections in reading
    /// order: the text between tables and each table (its first row as the
    /// header), so the tables are chunked as tables. A DOCX without tables
    /// has no sections; its text is chunked as it is.
    fn parse_docx(&self, path: &Path) -> Result<(String, Vec<DocumentSection>)> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open DOCX: {}", path.display()))?;

        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("Failed to read DOCX as ZIP: {}", path.display()))?;

        let xml_content = {
            let mut document_xml = archive
                .by_name("word/document.xml")
                .with_context(|| format!("DOCX missing word/document.xml: {}", path.display()))?;
            let declared = document_xml.size();
            read_office_part(&mut document_xml, "word/document.xml", declared).with_context(
                || format!("Failed to read document.xml from DOCX: {}", path.display()),
            )?
        };

        let blocks = office_xml::docx_blocks(&xml_content).with_context(|| {
            format!(
                "DOCX word/document.xml is not valid XML: {}",
                path.display()
            )
        })?;
        let text = office_xml::docx_text(&blocks);

        if text.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "DOCX contains no extractable text: {}",
                path.display()
            ));
        }

        Ok((text, docx_sections(blocks)))
    }

    /// An image file read by OCR (at below-normal priority) as page 1, and
    /// how it was read.
    fn parse_image(&self, path: &Path) -> Result<(StructuredDocument, OcrUse)> {
        let (route, reader_missing) = self.ocr_route(false);
        let _below_normal = super::ocr::priority::BelowNormal::new();
        let missing =
            || Err(anyhow::Error::new(ScanReaderMissing).context(path.display().to_string()));
        let read = |engine: &'static str, text: Result<String>| match text {
            Ok(text) if !text.trim().is_empty() => Ok((
                ocr_pages_to_document(std::slice::from_ref(&text)),
                OcrUse {
                    engine,
                    reader_missing,
                },
            )),
            // The scan reader may read what the system OCR could not.
            _ if reader_missing => missing(),
            Ok(_) => Err(anyhow::anyhow!(
                "OCR produced no text for image: {}",
                path.display()
            )),
            Err(e) => Err(e),
        };
        match route {
            OcrRoute::PpOcr(model) => read(OCR_ENGINE_PPOCR, super::ocr::read_image(&model, path)),
            OcrRoute::Worker(engine) => {
                let text = match &self.ocr_worker {
                    Some(worker) => worker
                        .read(
                            FileKind::Image,
                            path,
                            engine,
                            self.ocr.render_dpi,
                            self.ocr.system_render_dpi,
                            &|_, _| {},
                        )
                        .map(|pages| pages.concat()),
                    None => Err(anyhow::anyhow!("no OCR process is configured")),
                };
                read(worker_engine_name(engine), text)
            }
            #[cfg(windows)]
            OcrRoute::System => read(OCR_ENGINE_SYSTEM, super::windows_ocr::ocr_image(path)),
            #[cfg(not(windows))]
            OcrRoute::System => anyhow::bail!(
                "Image OCR not available on this platform: {}",
                path.display()
            ),
            OcrRoute::None if reader_missing => missing(),
            OcrRoute::None => anyhow::bail!("No OCR engine is available for {}", path.display()),
        }
    }

    /// Parse PPTX by extracting text from each slide's XML.
    fn parse_pptx(&self, path: &Path) -> Result<String> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open PPTX: {}", path.display()))?;

        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("Failed to read PPTX as ZIP: {}", path.display()))?;

        let mut slides: Vec<(usize, String)> = Vec::new();

        for i in 0..archive.len() {
            let mut entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.name().to_string();
            // Slide XML files: ppt/slides/slide1.xml, slide2.xml, ...
            if !name.starts_with("ppt/slides/slide") || !name.ends_with(".xml") {
                continue;
            }

            // Extract slide number from filename
            let slide_num = name
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse::<usize>()
                .unwrap_or(0);

            let declared = entry.size();
            let xml = read_office_part(&mut entry, &name, declared)
                .with_context(|| format!("Failed to read {name} from PPTX: {}", path.display()))?;
            let text = office_xml::pptx_slide_text(&xml)
                .with_context(|| format!("PPTX {name} is not valid XML: {}", path.display()))?;
            if !text.trim().is_empty() {
                slides.push((slide_num, text));
            }
        }

        if slides.is_empty() {
            return Err(anyhow::anyhow!(
                "PPTX contains no extractable text: {}",
                path.display()
            ));
        }

        slides.sort_by_key(|(num, _)| *num);

        let text = slides
            .into_iter()
            .map(|(num, text)| format!("--- Slide {} ---\n{}", num, text))
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(text)
    }

    /// Parse HTML by stripping tags and extracting visible text.
    fn parse_html(&self, path: &Path) -> Result<String> {
        let raw = tabular::read_text_file(path)
            .with_context(|| format!("Failed to read HTML: {}", path.display()))?;

        Ok(strip_html_tags(&raw))
    }

    pub fn parse_content(
        &self,
        content: &str,
        format: DocumentFormat,
        title: &str,
    ) -> ParsedDocument {
        ParsedDocument {
            content: content.to_string(),
            title: title.to_string(),
            metadata: HashMap::new(),
            format,
            structured_sections: Vec::new(),
            document: None,
        }
    }
}

/// Whether `path` is a file OCR may read (a PDF or an image).
fn is_ocr_candidate(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["pdf", "png", "jpg", "jpeg", "bmp", "tiff", "tif"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

/// Whether the file at `path` is one the parser reads with OCR: an image,
/// or a PDF without a text layer (the decision [`DocumentParser::parse_file`]
/// makes before it reads a PDF with OCR). False when the file cannot be read.
pub fn needs_ocr(path: &Path) -> bool {
    if !is_ocr_candidate(path) {
        return false;
    }
    let is_pdf = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return true;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    // A look at the first pages first: one that already holds the text no
    // scan has is not a scan, so the full layout parse below runs only for
    // PDFs that open with almost no text.
    if opening_has_text(&bytes) {
        return false;
    }
    match pdf_layout::parse_pdf_layout_with(&bytes, TableMode::Candidates) {
        Ok(parsed) => {
            is_effectively_scanned(parsed.document.text_chars(), parsed.document.pages.len())
        }
        Err(_) => pdf_extract_text(&bytes).is_none(),
    }
}

/// Whether the first two pages of a PDF hold 200 characters of text or more,
/// which no scan has in all its pages ([`is_effectively_scanned`]). False
/// when that cannot be told (the reader fails, or the PDF is encrypted).
fn opening_has_text(bytes: &[u8]) -> bool {
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = lopdf::Document::load_mem(pdf_forms::strip_leading_junk(bytes)).ok()?;
        if doc.is_encrypted() {
            return None;
        }
        let pages: Vec<u32> = doc.get_pages().keys().copied().take(2).collect();
        doc.extract_text(&pages).ok()
    }));
    matches!(read, Ok(Some(text)) if text.chars().filter(|c| !c.is_whitespace()).count() >= 200)
}

/// Whether a PDF's text layer is too thin to be its real content: under 200
/// characters in total and under 8 per page. Pages that are figures only are
/// normal in papers, so this looks at the whole document, not single pages.
pub fn is_effectively_scanned(text_chars: usize, pages: usize) -> bool {
    text_chars < 200 && text_chars < 8 * pages.max(1)
}

/// Whole-document text via `pdf_extract`, or `None` when it fails, panics
/// (it can on malformed CMaps) or yields nothing.
fn pdf_extract_text(bytes: &[u8]) -> Option<String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }));
    let text = match result {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            tracing::debug!("pdf_extract failed: {:?}", e);
            return None;
        }
        Err(_) => {
            tracing::warn!("pdf_extract panicked");
            return None;
        }
    };
    let cleaned = clean_extracted(&text);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// `pdf_extract` text without blank lines and with each line trimmed.
fn clean_extracted(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

/// `pdf_extract`'s text of each page (cleaned as [`pdf_extract_text`]'s),
/// or `None` when it fails, panics, reads nothing, or reads other than
/// `pages` pages: it stops at the first page it cannot read, so a short list
/// would put the text of later pages on no page at all.
fn pdf_extract_pages(bytes: &[u8], pages: usize) -> Option<Vec<String>> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem_by_pages(bytes)
    }));
    let texts = match result {
        Ok(Ok(texts)) => texts,
        Ok(Err(e)) => {
            tracing::debug!("pdf_extract failed by page: {:?}", e);
            return None;
        }
        Err(_) => {
            tracing::warn!("pdf_extract panicked by page");
            return None;
        }
    };
    if texts.len() != pages {
        return None;
    }
    let texts: Vec<String> = texts.iter().map(|text| clean_extracted(text)).collect();
    texts.iter().any(|text| !text.is_empty()).then_some(texts)
}

/// Characters of `text` other than whitespace.
fn visible_chars(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// The text sections of a PDF the layout parser could not read, from one of
/// the two fallback readers: lopdf's pages (`lopdf_pages`: page number and
/// text), or `pdf_extract`'s text (`extract_text`, the whole document).
/// `pdf_extract` decodes fonts, hex strings and ToUnicode maps that lopdf's
/// reader does not, so its text is used unless lopdf's reads about as much
/// (nine tenths of its characters other than whitespace, or more). Its text
/// is put on pages by `extract_pages` (asked only then) when those hold
/// about all of it, else it is one unpaged section (page 0).
fn fallback_text_sections(
    lopdf_pages: &[(usize, String)],
    extract_text: &str,
    extract_pages: impl FnOnce() -> Option<Vec<String>>,
) -> Vec<DocumentSection> {
    let section = |content: &str, page: usize| DocumentSection::Text {
        content: content.to_string(),
        page,
        heading: None,
    };
    let lopdf_chars: usize = lopdf_pages
        .iter()
        .map(|(_, text)| visible_chars(text))
        .sum();
    let extract_chars = visible_chars(extract_text);
    if lopdf_chars > 0 && lopdf_chars * 10 >= extract_chars * 9 {
        return lopdf_pages
            .iter()
            .map(|(page, text)| (*page, text.trim()))
            .filter(|(_, text)| !text.is_empty())
            .map(|(page, text)| section(text, page))
            .collect();
    }
    if extract_chars == 0 {
        return Vec::new();
    }
    if let Some(pages) = extract_pages() {
        let paged: usize = pages.iter().map(|text| visible_chars(text)).sum();
        if paged * 10 >= extract_chars * 9 {
            return pages
                .iter()
                .enumerate()
                .map(|(index, text)| (index + 1, text.trim()))
                .filter(|(_, text)| !text.is_empty())
                .map(|(page, text)| section(text, page))
                .collect();
        }
    }
    vec![section(extract_text, 0)]
}

/// Places form fields into a structured document as [`BlockKind::FormField`]
/// blocks: on their page, before the first block that lies wholly below the field
/// (else after the page's last block), so they read in page order and inherit the
/// section they sit in. Fields without a page go to the end. Empty text and choice
/// fields are left out; checkboxes and signatures always count (`No`, `Not signed`).
pub fn insert_form_fields(doc: &mut StructuredDocument, fields: &[pdf_forms::FormField]) {
    use pdf_forms::FieldKind;
    let mut inserted = 0usize;
    for field in fields {
        let keep = !field.value.trim().is_empty()
            || matches!(field.kind, FieldKind::Checkbox | FieldKind::Signature);
        if !keep {
            continue;
        }
        let mut block = Block::form_field(&field.label, &field.value, field.kind);
        block.page = field.page;
        block.bbox = field.bbox;
        let at = match field.page {
            Some(page) => {
                let centre = field.bbox.map(|b| b.center_y());
                let below = doc.blocks.iter().position(|b| {
                    b.page == Some(page)
                        && centre.is_some_and(|y| b.bbox.is_some_and(|bb| bb.y1 < y))
                });
                below.unwrap_or_else(|| {
                    doc.blocks
                        .iter()
                        .rposition(|b| b.page.is_some_and(|p| p <= page))
                        .map(|i| i + 1)
                        .unwrap_or(0)
                })
            }
            None => doc.blocks.len(),
        };
        doc.blocks.insert(at, block);
        inserted += 1;
    }
    if inserted > 0 {
        doc.finalize();
    }
}

/// Whether the system OCR can run here (Windows with an OCR language).
fn system_ocr_available() -> bool {
    #[cfg(windows)]
    {
        super::windows_ocr::is_ocr_available()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The title printed in `doc`: its title block unless that reads as a
/// heading, else the first line of its first page that reads as a title (as
/// the citation graph reads it, [`crate::research::citations::identity`]).
pub fn document_title(doc: &StructuredDocument) -> Option<String> {
    crate::research::citations::identity::title_from_block(doc)
}

/// Whether a file name (its stem) is an identifier rather than a title: an
/// arXiv id ("1706.03762v7"), a number or date ("20231015", "2023-10-15"),
/// a hash or UUID, or a code such as "PMC1234567" or "IMG_0042": four
/// digits or more and no run of more than three letters.
pub fn is_identifier_stem(stem: &str) -> bool {
    let stem = stem.trim();
    if stem.is_empty() {
        return true;
    }
    let digits = stem.chars().filter(|c| c.is_ascii_digit()).count();
    let hash = stem.chars().filter(|c| *c != '-').count() >= 8
        && digits > 0
        && stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    let mut longest_letters = 0;
    let mut letters = 0;
    for c in stem.chars() {
        if c.is_alphabetic() {
            letters += 1;
            longest_letters = longest_letters.max(letters);
        } else {
            letters = 0;
        }
    }
    hash || (digits >= 4 && longest_letters <= 3)
}

/// Pages shown in a list of pages before the rest are counted.
const PAGES_LISTED: usize = 10;

/// `pages` (1-based) in words: "page 4", "pages 4, 9"; past
/// [`PAGES_LISTED`] pages, the rest are counted ("… 10 and 5 more").
fn page_list(pages: &[usize]) -> String {
    let listed: Vec<String> = pages
        .iter()
        .take(PAGES_LISTED)
        .map(|p| p.to_string())
        .collect();
    let mut list = format!(
        "{} {}",
        if pages.len() == 1 { "page" } else { "pages" },
        listed.join(", ")
    );
    if pages.len() > PAGES_LISTED {
        list.push_str(&format!(" and {} more", pages.len() - PAGES_LISTED));
    }
    list
}

/// What to tell the user about a file whose `metadata` lists pages OCR
/// could not read ([`OCR_FAILED_PAGES_KEY`]): "pages 4, 9 could not be
/// read". `None` when every page was read.
pub fn ocr_failed_pages_notice(metadata: &HashMap<String, String>) -> Option<String> {
    let pages: Vec<usize> = metadata
        .get(OCR_FAILED_PAGES_KEY)?
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    if pages.is_empty() {
        return None;
    }
    Some(format!("{} could not be read", page_list(&pages)))
}

/// Records how a scan was read: the engine, and that the scan reader was
/// missing ([`OCR_READER_MISSING_KEY`]).
fn record_ocr(ocr: OcrUse, metadata: &mut HashMap<String, String>) {
    metadata.insert(OCR_ENGINE_KEY.to_string(), ocr.engine.to_string());
    if ocr.reader_missing {
        metadata.insert(OCR_READER_MISSING_KEY.to_string(), "true".to_string());
    }
}

/// One paragraph block per non-empty OCR'd page, numbered from 1. Pages are
/// not split into headings and paragraphs: on the scanned evaluation set,
/// section-sized chunks of OCR'd pages ranked the right page first less
/// often than page-sized ones (`page_hit@1` 0.56 against 0.88).
fn ocr_pages_to_document(pages: &[String]) -> StructuredDocument {
    let blocks = pages
        .iter()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(i, text)| Block::new(BlockKind::Paragraph, text.trim()).on_page(i as u32 + 1, None))
        .collect();
    let mut doc = StructuredDocument {
        pages: Vec::new(),
        blocks,
    };
    doc.finalize();
    doc
}

/// Records the table detection of a PDF parse: its candidate pages, and with the
/// model, the model's id and how many tables it structured.
fn record_table_report(
    report: &TableReport,
    with_model: bool,
    metadata: &mut HashMap<String, String>,
) {
    if !report.candidates.is_empty() {
        let pages: Vec<String> = report
            .candidates
            .iter()
            .map(|c| c.page.to_string())
            .collect();
        metadata.insert(TABLE_CANDIDATES_KEY.to_string(), pages.join(","));
    }
    if with_model {
        metadata.insert(TABLE_MODEL_KEY.to_string(), TABLE_MODEL_ID.to_string());
        metadata.insert(
            MODEL_TABLES_KEY.to_string(),
            report.model_tables.to_string(),
        );
    }
}

/// Record per-table statistics (sheet count, row count, numeric columns) in
/// document metadata. Numeric column hints are consumed by chart generation.
fn record_table_metadata(
    tables: &[tabular::ExtractedTable],
    metadata: &mut HashMap<String, String>,
) {
    metadata.insert("sheet_count".to_string(), tables.len().to_string());
    let total_rows: usize = tables.iter().map(|t| t.rows.len()).sum();
    metadata.insert("total_data_rows".to_string(), total_rows.to_string());
    for (idx, table) in tables.iter().enumerate() {
        metadata.insert(format!("sheet_{}_name", idx), table.name.clone());
        let numeric = tabular::numeric_columns(table);
        if !numeric.is_empty() {
            metadata.insert(format!("sheet_{}_numeric_columns", idx), numeric.join(","));
        }
    }
}

/// An XML part of a Word or PowerPoint file (`name`, `declared` bytes once
/// decompressed by its archive entry), refused when it is larger than
/// [`limits::MAX_OFFICE_PART_BYTES`] decompressed whatever its entry says.
fn read_office_part(part: impl std::io::Read, name: &str, declared: u64) -> Result<String> {
    use std::io::Read;
    limits::check_office_part(name, declared)?;
    let mut bytes = Vec::new();
    part.take(limits::MAX_OFFICE_PART_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("Failed to decompress {name}"))?;
    limits::check_office_part(name, bytes.len() as u64)?;
    String::from_utf8(bytes).with_context(|| format!("{name} is not UTF-8 text"))
}

/// The sections of a DOCX with tables ([`DocumentParser::parse_docx`]):
/// the paragraphs between tables as one text section each, and every table
/// of two rows or more with its first row as the header. A table of one row
/// is text. No sections when there is no such table.
fn docx_sections(blocks: Vec<office_xml::DocxBlock>) -> Vec<DocumentSection> {
    use office_xml::DocxBlock;
    let has_table = blocks
        .iter()
        .any(|b| matches!(b, DocxBlock::Table(rows) if rows.len() >= 2));
    if !has_table {
        return Vec::new();
    }
    let mut sections = Vec::new();
    let mut text: Vec<String> = Vec::new();
    let flush = |text: &mut Vec<String>, sections: &mut Vec<DocumentSection>| {
        if !text.is_empty() {
            sections.push(DocumentSection::Text {
                content: std::mem::take(text).join("\n"),
                page: 0,
                heading: None,
            });
        }
    };
    for block in blocks {
        match block {
            DocxBlock::Paragraph(line) => text.push(line),
            DocxBlock::Table(rows) if rows.len() < 2 => {
                text.extend(rows.iter().map(|row| row.join("\t")));
            }
            DocxBlock::Table(mut rows) => {
                flush(&mut text, &mut sections);
                let headers = rows.remove(0);
                sections.push(DocumentSection::Table {
                    headers,
                    rows,
                    page: 0,
                    caption: None,
                });
            }
        }
    }
    flush(&mut text, &mut sections);
    sections
}

/// Elements that start a new line in the text: their opening and closing tags.
const HTML_BLOCK_ELEMENTS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "tr",
    "ul",
];

/// Elements whose content is never text: everything up to their closing tag
/// is skipped.
const HTML_RAW_TEXT_ELEMENTS: &[&str] = &["script", "style", "template"];

/// What numeric references to 0x80-0x9F stand for: the Windows-1252
/// characters of those bytes, as the HTML standard reads them. The five bytes
/// Windows-1252 leaves undefined keep their code.
const WINDOWS_1252_C1: [u32; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, // 0x80
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F, // 0x88
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, // 0x90
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178, // 0x98
];

/// Opens and closes a CDATA section.
const CDATA_OPEN: &str = "<![CDATA[";
const CDATA_CLOSE: &str = "]]>";

/// Strip HTML tags and decode common entities, returning visible text content.
///
/// Works on byte offsets into `html`, which only ever stop at ASCII bytes
/// (`<`, `>`, `&`, `;`, quotes) or after a whole character, so every slice
/// is on a char boundary whatever the script of the page. Tag and entity
/// names are compared ASCII-case-insensitively in place; the page is never
/// lowercased (which can change its byte length).
fn strip_html_tags(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut result = String::with_capacity(html.len() / 2);
    let mut last_was_whitespace = false;
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'<' if html[i..].starts_with(CDATA_OPEN) => {
                // Character data (XHTML, SVG, MathML): its text as written,
                // up to `]]>`.
                let body = i + CDATA_OPEN.len();
                let (text_end, end) = html[body..]
                    .find(CDATA_CLOSE)
                    .map_or((html.len(), html.len()), |p| {
                        (body + p, body + p + CDATA_CLOSE.len())
                    });
                for ch in html[body..text_end].chars() {
                    push_html_text(&mut result, &mut last_was_whitespace, ch);
                }
                i = end;
            }
            b'<' => {
                let Some(tag) = html_tag_at(html, i) else {
                    // A `<` that opens no tag ("p < 0.05") is text.
                    push_html_text(&mut result, &mut last_was_whitespace, '<');
                    i += 1;
                    continue;
                };
                if !tag.closing
                    && HTML_RAW_TEXT_ELEMENTS
                        .iter()
                        .any(|name| tag.name.eq_ignore_ascii_case(name))
                {
                    i = html_raw_text_end(html, tag.end, tag.name);
                    continue;
                }
                if HTML_BLOCK_ELEMENTS
                    .iter()
                    .any(|name| tag.name.eq_ignore_ascii_case(name))
                {
                    if !result.is_empty() && !result.ends_with('\n') {
                        result.push('\n');
                    }
                    last_was_whitespace = true;
                } else if !tag.closing
                    && (tag.name.eq_ignore_ascii_case("td") || tag.name.eq_ignore_ascii_case("th"))
                    && !result.is_empty()
                    && !result.ends_with('\n')
                    && !result.ends_with('\t')
                {
                    // Table cells are separated by a tab.
                    result.push('\t');
                    last_was_whitespace = true;
                }
                i = tag.end;
            }
            b'&' => match decode_html_entity(&html[i..]) {
                Some((ch, len)) => {
                    push_html_text(&mut result, &mut last_was_whitespace, ch);
                    i += len;
                }
                None => {
                    push_html_text(&mut result, &mut last_was_whitespace, '&');
                    i += 1;
                }
            },
            _ => {
                let Some(ch) = html[i..].chars().next() else {
                    break;
                };
                push_html_text(&mut result, &mut last_was_whitespace, ch);
                i += ch.len_utf8();
            }
        }
    }

    // Clean up excessive blank lines
    let mut cleaned = String::with_capacity(result.len());
    let mut blank_lines = 0;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 {
                cleaned.push('\n');
            }
        } else {
            blank_lines = 0;
            if !cleaned.is_empty() && !cleaned.ends_with('\n') {
                cleaned.push('\n');
            }
            cleaned.push_str(trimmed);
        }
    }

    cleaned
}

/// Append one character of page text, collapsing runs of whitespace (a
/// non-breaking space included) to one space or line break.
fn push_html_text(result: &mut String, last_was_whitespace: &mut bool, ch: char) {
    if ch.is_whitespace() {
        if !*last_was_whitespace && !result.is_empty() {
            result.push(if ch == '\n' { '\n' } else { ' ' });
            *last_was_whitespace = true;
        }
    } else {
        result.push(ch);
        *last_was_whitespace = false;
    }
}

/// A tag, comment, doctype or processing instruction in a page.
struct HtmlTag<'a> {
    /// The element name (ASCII letters and digits); empty for a comment,
    /// doctype or processing instruction.
    name: &'a str,
    closing: bool,
    /// Byte offset just past the tag (past its `>`, or the end of the page
    /// for a tag never closed).
    end: usize,
}

/// The tag starting at the `<` at byte `start`, or `None` when that `<` is
/// text: a tag begins with a letter, `/` and a letter, `!` or `?`.
fn html_tag_at(html: &str, start: usize) -> Option<HtmlTag<'_>> {
    let bytes = html.as_bytes();
    let after = start + 1;
    if html
        .get(after..after + 3)
        .is_some_and(|s| s.as_bytes() == b"!--")
    {
        let body = after + 3;
        let rest = &html[body..];
        // `<!-->` and `<!--->` are empty comments, closed there.
        let end = if rest.starts_with('>') {
            body + 1
        } else if rest.starts_with("->") {
            body + 2
        } else {
            rest.find("-->").map_or(html.len(), |p| body + p + 3)
        };
        return Some(HtmlTag {
            name: "",
            closing: false,
            end,
        });
    }
    if matches!(bytes.get(after), Some(b'!' | b'?')) {
        let end = html[after..]
            .find('>')
            .map_or(html.len(), |p| after + p + 1);
        return Some(HtmlTag {
            name: "",
            closing: false,
            end,
        });
    }
    let closing = bytes.get(after) == Some(&b'/');
    let name_start = if closing { after + 1 } else { after };
    if !bytes.get(name_start).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let name_len = bytes[name_start..]
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    let name_end = name_start + name_len;
    Some(HtmlTag {
        name: &html[name_start..name_end],
        closing,
        end: html_tag_end(bytes, name_end),
    })
}

/// Byte offset just past the `>` that closes a tag whose attributes start at
/// `from`; a `>` inside a quoted attribute value does not close it. A quote
/// opens a value only right after `=` (spaces between allowed): elsewhere,
/// as in the stray tag of `x<y and it's`, it is an ordinary character.
fn html_tag_end(bytes: &[u8], from: usize) -> usize {
    let mut quote: Option<u8> = None;
    // The last byte outside quotes that is not whitespace.
    let mut last = 0u8;
    for (offset, &b) in bytes[from..].iter().enumerate() {
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                    last = b;
                }
            }
            None => match b {
                b'>' => return from + offset + 1,
                b'"' | b'\'' if last == b'=' => quote = Some(b),
                _ if b.is_ascii_whitespace() => {}
                _ => last = b,
            },
        }
    }
    bytes.len()
}

/// Byte offset just past the closing tag of the raw-text element `name`
/// whose content starts at `from` (the end of the page when it is never
/// closed). `</SCRIPT >` closes `<script>`.
fn html_raw_text_end(html: &str, from: usize, name: &str) -> usize {
    let bytes = html.as_bytes();
    let mut at = from;
    while let Some(offset) = html[at..].find("</") {
        let name_start = at + offset + 2;
        let closes = html
            .get(name_start..name_start + name.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
            && !bytes
                .get(name_start + name.len())
                .is_some_and(u8::is_ascii_alphanumeric);
        if closes {
            return html_tag_end(bytes, name_start + name.len());
        }
        at = name_start;
    }
    html.len()
}

/// The character an entity at the start of `text` (which starts with `&`)
/// stands for, and the entity's length in bytes: `&amp;`, `&lt;`, `&gt;`,
/// `&quot;`, `&apos;`, `&nbsp;`, a few typographic names, and numeric
/// `&#39;` / `&#x27;`. `None` for anything else, which stays as written.
fn decode_html_entity(text: &str) -> Option<(char, usize)> {
    // The longest entity decoded: `&#x10FFFF;` or `&#1114111;`.
    const MAX_ENTITY_LEN: usize = 10;
    let semicolon = text
        .bytes()
        .take(MAX_ENTITY_LEN + 1)
        .position(|b| b == b';')?;
    let name = text.get(1..semicolon)?;
    let ch = if let Some(number) = name.strip_prefix('#') {
        let (digits, radix) = match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16),
            None => (number, 10),
        };
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        let code = u32::from_str_radix(digits, radix).ok()?;
        // Pages written in Windows-1252 refer to its characters 0x80-0x9F
        // by their byte (`&#150;` for an en dash); browsers read them so.
        let code = match code {
            0x80..=0x9F => WINDOWS_1252_C1[(code - 0x80) as usize],
            _ => code,
        };
        match char::from_u32(code) {
            Some('\0') | None => char::REPLACEMENT_CHARACTER,
            Some(c) => c,
        }
    } else {
        match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => '\u{a0}',
            "ndash" => '\u{2013}',
            "mdash" => '\u{2014}',
            "hellip" => '\u{2026}',
            "lsquo" => '\u{2018}',
            "rsquo" => '\u{2019}',
            "ldquo" => '\u{201c}',
            "rdquo" => '\u{201d}',
            "copy" => '\u{a9}',
            "reg" => '\u{ae}',
            "trade" => '\u{2122}',
            "euro" => '\u{20ac}',
            _ => return None,
        }
    };
    Some((ch, semicolon + 1))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::processing::pdf_fixtures::{build_pdf, column};

    /// A ToUnicode CMap with a 6-byte bfrange destination. Valid enough for
    /// PDF viewers, but pdf_extract's CMap parser panics on it ("bad length
    /// of hexstring") — the failure that made a text PDF look scanned.
    const ODD_CMAP: &str = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /Odd def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n1 beginbfrange\n<20> <7E> <0020>\nendbfrange\n1 beginbfrange\n<80> <81> <000000000041>\nendbfrange\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

    #[test]
    fn text_pdf_that_crashes_pdf_extract_is_not_treated_as_scanned() {
        let bytes = build_pdf(&[column(72.0, 700.0, "robot", 6)], Some(ODD_CMAP));
        let pdf_extract_result =
            std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes));
        assert!(
            pdf_extract_result.is_err(),
            "fixture must reproduce the pdf_extract panic"
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("robottt-like.pdf");
        std::fs::write(&path, &bytes).expect("write pdf");
        let parsed = DocumentParser::new().parse_file(&path).expect("parsed");
        assert!(
            parsed.content.contains("robot line 0 of body text"),
            "{}",
            parsed.content
        );
        let doc = parsed.document.expect("structured document");
        assert!(!doc.blocks.is_empty());
        assert!(doc.blocks.iter().all(|b| b.page == Some(1)));
        assert_eq!(
            parsed.metadata.get("page_count").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn acroform_fields_become_citable_blocks_on_their_pages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("return-form.pdf");
        std::fs::write(&path, crate::processing::pdf_forms::tests::form_pdf()).expect("write");
        let parsed = DocumentParser::new().parse_file(&path).expect("parsed");
        // Fields are blocks of the document, not a page-less section.
        assert!(parsed.structured_sections.is_empty());
        let doc = parsed.document.expect("structured document");
        let fields: Vec<&Block> = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::FormField { .. }))
            .collect();
        let texts: Vec<&str> = fields.iter().map(|b| b.text.as_str()).collect();
        assert!(
            texts.contains(&"Full name of the applicant: Asha Verma"),
            "{texts:?}"
        );
        assert!(texts.contains(&"Resident of India: Yes (Resident)"));
        assert!(texts.contains(&"Declaration accepted: No"));
        assert!(texts.contains(&"Tax regime: New regime"));
        assert!(texts.contains(&"States of income: Maharashtra, Karnataka"));
        assert!(texts.contains(&"Comment: Check the totals"));
        // The empty field is left out.
        assert!(!texts.iter().any(|t| t.starts_with("notes")));
        let name = fields
            .iter()
            .find(|b| b.text.contains("Asha Verma"))
            .expect("name field");
        assert_eq!(name.page, Some(1));
        assert_eq!(
            name.bbox,
            Some(crate::processing::document_model::BBox::new(
                250.0, 696.0, 450.0, 712.0
            ))
        );
        let employer = fields
            .iter()
            .find(|b| b.text.contains("Acme Tools Ltd"))
            .expect("employer field");
        assert_eq!(employer.page, Some(2));
        // Placed in reading order on its page: after the page-1 heading text above it.
        let heading_at = doc
            .blocks
            .iter()
            .position(|b| b.text.contains("Applicant details"))
            .expect("heading text");
        let name_at = doc
            .blocks
            .iter()
            .position(|b| b.text.contains("Asha Verma"))
            .expect("name");
        assert!(heading_at < name_at);
        // A value drawn by its widget's appearance stream is not read twice.
        assert_eq!(
            parsed.content.matches("Asha Verma").count(),
            1,
            "{}",
            parsed.content
        );

        // The chunker packs the fields into form chunks citing each widget's box.
        let chunks = crate::processing::structure_chunker::StructureChunker::new(300).chunk(
            &doc,
            &crate::processing::chunker::DocumentContext::titled("Return"),
            &|t: &str| t.split_whitespace().count(),
        );
        let form = chunks
            .iter()
            .find(|c| c.text.contains("Asha Verma"))
            .expect("form chunk");
        let layout = form.layout.as_ref().expect("layout");
        assert_eq!(layout.unit, "form");
        assert_eq!(layout.page_start, Some(1));
        assert!(layout
            .regions
            .iter()
            .any(|r| r.page == 1 && r.bbox.x0 == 250.0 && r.bbox.y1 == 712.0));
    }

    #[test]
    fn scanned_only_when_text_is_essentially_absent_across_pages() {
        assert!(is_effectively_scanned(0, 22));
        assert!(is_effectively_scanned(60, 22));
        assert!(!is_effectively_scanned(64_190, 22));
        // A one-page memo with a short text layer is real text.
        assert!(!is_effectively_scanned(40, 1));
        assert!(!is_effectively_scanned(250, 40));
    }

    #[test]
    fn latex_and_markdown_files_get_structured_documents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tex = dir.path().join("paper.tex");
        std::fs::write(
            &tex,
            "\\begin{document}\n\\section{Intro}\nPrimes spiral.\n\\begin{lemma}\nEvery prime is odd or two.\n\\end{lemma}\n\\end{document}\n",
        )
        .expect("write tex");
        let parsed = DocumentParser::new().parse_file(&tex).expect("parsed tex");
        let doc = parsed.document.expect("tex structure");
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(&b.kind, BlockKind::Theorem { label } if label == "Lemma")));
        assert!(parsed.content.contains("Primes spiral."));

        let md = dir.path().join("notes.md");
        std::fs::write(&md, "# Notes\n\nSome text.\n").expect("write md");
        let parsed = DocumentParser::new().parse_file(&md).expect("parsed md");
        assert_eq!(parsed.document.expect("md structure").blocks.len(), 2);
    }

    /// The committed scanned corpus of the evaluation harness (synthetic,
    /// image-only pages: see `crates/shodh-eval/src/scan.rs`).
    fn scanned(file: &str) -> std::path::PathBuf {
        file.split('/').fold(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("shodh-eval")
                .join("data")
                .join("scanned")
                .join("corpus"),
            |p, part| p.join(part),
        )
    }

    /// A parser that reads scans with the installed PP-OCRv6 model only.
    fn ppocr_parser() -> DocumentParser {
        let root = std::env::var_os("SHODH_TEST_MODELS")
            .expect("SHODH_TEST_MODELS must point at the models directory");
        let dir = super::super::ocr::model_dir(std::path::Path::new(&root));
        assert!(
            super::super::ocr::is_installed(&dir),
            "the OCR model is not installed in {}",
            dir.display()
        );
        let model = super::super::ocr::shared_ocr_model();
        model.set_loader(move || super::super::ocr::PpOcr::load(&dir));
        DocumentParser::new().with_ocr(
            model,
            OcrConfig {
                engine: OcrEngineChoice::PpOcr,
                ..OcrConfig::default()
            },
        )
    }

    /// A scanned invoice (300 dpi, black and white, no text layer) reads
    /// through PP-OCRv6 into a paged document whose table rows read across.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing pp-ocrv6/"
    )]
    fn scanned_pdf_reads_through_ppocr_into_pages() {
        let parsed = ppocr_parser()
            .parse_file(&scanned("invoices/inv-2025-0164-scan.pdf"))
            .expect("parsed scan");
        assert_eq!(
            parsed.metadata.get(OCR_ENGINE_KEY).map(String::as_str),
            Some(OCR_ENGINE_PPOCR)
        );
        let doc = parsed.document.expect("OCR document");
        let page = |n: u32| {
            doc.blocks
                .iter()
                .filter(|b| b.page == Some(n))
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let first = page(1);
        assert!(first.contains("Invoice No: INV-2025-0164"), "{first}");
        assert!(
            first.contains("Dashboard migration (hours) 40 2,500.00 1,00,000.00"),
            "table row not read across: {first}"
        );
        assert!(
            page(2).contains("Total amount due: INR 4,63,740.00"),
            "{}",
            page(2)
        );
    }

    /// A scanned page saved as an image reads through PP-OCRv6 as page 1.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing pp-ocrv6/"
    )]
    fn scanned_image_reads_through_ppocr_as_one_page() {
        let parsed = ppocr_parser()
            .parse_file(&scanned("contracts/nda-orchid-kestrel-p2-scan.png"))
            .expect("parsed image");
        assert_eq!(
            parsed.metadata.get(OCR_ENGINE_KEY).map(String::as_str),
            Some(OCR_ENGINE_PPOCR)
        );
        let doc = parsed.document.expect("OCR document");
        assert!(doc.blocks.iter().all(|b| b.page == Some(1)));
        assert!(
            parsed
                .content
                .contains("The recipient keeps Confidential Information secret for five (5) years"),
            "{}",
            parsed.content
        );
    }

    /// A scan as the indexer read it, cached for `path`.
    pub(crate) fn cached_scan(path: &Path, text: &str) -> CachedScan {
        let mut doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![Block::new(BlockKind::Paragraph, text).on_page(1, None)],
        };
        doc.finalize();
        let parsed = ParsedDocument {
            content: text.to_string(),
            title: "scan".to_string(),
            metadata: HashMap::from([(OCR_ENGINE_KEY.to_string(), OCR_ENGINE_PPOCR.to_string())]),
            format: DocumentFormat::PDF,
            structured_sections: Vec::new(),
            document: Some(doc),
        };
        CachedScan::of(
            &crate::rag_engine::normalize_source_path(path),
            FileStamp::of(path).unwrap(),
            &parsed,
        )
        .expect("an OCR parse is cacheable")
    }

    /// A cached scan is read without starting OCR; once the file changes,
    /// the cache is missed and OCR is asked for.
    #[test]
    fn a_cached_scan_is_read_without_ocr_until_the_file_changes() {
        use super::super::ocr::worker::WorkerCommand;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lease-scan.pdf");
        std::fs::copy(scanned("contracts/lease-saffron-tidewater-scan.pdf"), &path).unwrap();
        let cache = Arc::new(OcrCache::new(dir.path().join("ocr_cache")));
        cache
            .put(&cached_scan(&path, "Notice period: ninety (90) days"))
            .unwrap();
        // A worker that cannot start: any OCR would fail.
        let worker = Arc::new(OcrWorker::new(WorkerCommand {
            program: dir.path().join("no-ocr-worker.exe"),
            args: Vec::new(),
            env: Vec::new(),
        }));
        worker.set_model_dir(Some(dir.path().join("pp-ocrv6")));
        let parser = DocumentParser::new()
            .with_ocr_worker(worker.clone())
            .with_ocr_cache(cache.clone());

        let parsed = parser.parse_file(&path).expect("read from the cache");
        assert_eq!(parsed.content, "Notice period: ninety (90) days");
        assert_eq!(
            parsed.metadata.get(OCR_ENGINE_KEY).map(String::as_str),
            Some(OCR_ENGINE_PPOCR)
        );
        assert!(parsed.document.is_some_and(|d| d.blocks[0].page == Some(1)));
        assert!(!worker.is_running(), "a hit starts no OCR");

        // A stale modification time misses: OCR is asked for (and fails here).
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400),
        )
        .unwrap();
        drop(file);
        assert!(
            parser.parse_file(&path).is_err(),
            "a changed file is read again"
        );
        assert!(!worker.is_running());
    }

    /// Without the model, choosing PP-OCRv6 only reads nothing rather than
    /// falling back silently, and says the scan reader is missing.
    #[test]
    fn ppocr_only_without_the_model_reads_no_scan() {
        let parser = DocumentParser::new().with_ocr(
            super::super::ocr::shared_ocr_model(),
            OcrConfig {
                engine: OcrEngineChoice::PpOcr,
                ..OcrConfig::default()
            },
        );
        for file in [
            "contracts/nda-orchid-kestrel-p2-scan.png",
            "invoices/inv-2025-0117-scan.pdf",
        ] {
            let err = parser
                .parse_file(&scanned(file))
                .expect_err("no OCR engine");
            assert!(err.is::<ScanReaderMissing>(), "{file}: {err:#}");
            assert!(
                format!("{err:#}").contains("the scan reader (31 MB)"),
                "{err:#}"
            );
        }
    }

    /// A PDF that needs a password, and one no reader can open, say so and
    /// are not taken for scans (the scan reader would not help). The Library
    /// turns these reasons into plain words by the phrases asserted here
    /// (`app/src/features/library/indexingAdvice.ts`).
    #[test]
    fn locked_and_damaged_pdfs_are_not_reported_as_scans() {
        use crate::processing::pdf_fixtures::{build_pdf, lock_with_password};
        let parser = DocumentParser::new().with_ocr(
            super::super::ocr::shared_ocr_model(),
            OcrConfig {
                engine: OcrEngineChoice::PpOcr,
                ..OcrConfig::default()
            },
        );
        let dir = tempfile::tempdir().unwrap();

        let locked = dir.path().join("statement.pdf");
        std::fs::write(&locked, lock_with_password(&build_pdf(&[vec![]], None))).unwrap();
        let err = parser.parse_file(&locked).expect_err("a locked PDF");
        assert!(err.is::<PdfEncrypted>(), "{err:#}");
        assert!(!err.is::<ScanReaderMissing>(), "{err:#}");
        assert!(
            format!("{err:#}").contains("protected with a password"),
            "{err:#}"
        );

        for (name, bytes) in [
            (
                "partial-download.pdf",
                &b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog"[..],
            ),
            ("not-a-pdf.pdf", &b"PK\x03\x04 this is a zip, not a PDF"[..]),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let err = parser.parse_file(&path).expect_err("a damaged PDF");
            assert!(err.is::<PdfDamaged>(), "{name}: {err:#}");
            assert!(
                format!("{err:#}").contains("damaged or incomplete"),
                "{name}: {err:#}"
            );
        }

        // A PDF whose empty password opens it is read as usual.
        let open = dir.path().join("open.pdf");
        std::fs::write(
            &open,
            build_pdf(
                &[vec![crate::processing::pdf_fixtures::text(
                    72.0,
                    700.0,
                    11.0,
                    "The deposit is returned within thirty days of the end of the lease.",
                )]],
                None,
            ),
        )
        .unwrap();
        assert!(parser.parse_file(&open).is_ok());
    }

    /// Scans are told apart from text PDFs without OCR, as `parse_file` tells
    /// them apart.
    #[test]
    fn needs_ocr_finds_scans_and_images_only() {
        assert!(needs_ocr(&scanned("invoices/inv-2025-0117-scan.pdf")));
        assert!(needs_ocr(&scanned(
            "contracts/nda-orchid-kestrel-p2-scan.png"
        )));
        let dir = tempfile::tempdir().unwrap();
        let text_pdf = dir.path().join("lease.pdf");
        let lines: Vec<_> = (0..30)
            .map(|i| {
                crate::processing::pdf_fixtures::text(
                    72.0,
                    740.0 - 20.0 * i as f32,
                    11.0,
                    "The tenant pays the rent on the first working day of each month.",
                )
            })
            .collect();
        std::fs::write(
            &text_pdf,
            crate::processing::pdf_fixtures::build_pdf(&[lines], None),
        )
        .unwrap();
        assert!(!needs_ocr(&text_pdf));
        // Told from its first page, without the layout parse.
        assert!(opening_has_text(&std::fs::read(&text_pdf).unwrap()));
        assert!(!opening_has_text(
            &std::fs::read(scanned("invoices/inv-2025-0117-scan.pdf")).unwrap()
        ));
        let notes = dir.path().join("notes.txt");
        std::fs::write(&notes, "plain text").unwrap();
        assert!(!needs_ocr(&notes));
        assert!(!needs_ocr(&dir.path().join("gone.pdf")), "unreadable");
    }

    /// The other reasons the Library reads: a damaged Office file and an
    /// unreadable spreadsheet.
    #[test]
    fn damaged_office_files_say_what_failed() {
        let parser = DocumentParser::new();
        let dir = tempfile::tempdir().unwrap();
        let docx = dir.path().join("report.docx");
        std::fs::write(&docx, b"not a zip archive").unwrap();
        let err = format!("{:#}", parser.parse_file(&docx).expect_err("damaged docx"));
        assert!(err.contains("as ZIP"), "{err}");
        let xlsx = dir.path().join("budget.xlsx");
        std::fs::write(&xlsx, b"not a zip archive").unwrap();
        let err = format!("{:#}", parser.parse_file(&xlsx).expect_err("damaged xlsx"));
        assert!(err.contains("failed to open spreadsheet"), "{err}");
    }

    /// The system OCR is choosable, but `Auto` uses it only where the scan
    /// reader is missing or cannot render the scan.
    #[test]
    fn ocr_route_prefers_the_scan_reader_on_auto() {
        use OcrEngine::{None as NoEngine, PpOcr, System};
        let d = |engine, reader_missing| OcrDecision {
            engine,
            reader_missing,
        };
        let auto = OcrEngineChoice::Auto;
        // (reader installed, system OCR available, JBIG2/JPX) -> decision
        let cases = [
            ((true, true, false), d(PpOcr, false)),
            ((true, false, false), d(PpOcr, false)),
            ((false, true, false), d(System, true)),
            ((false, false, false), d(NoEngine, true)),
            // Blank with the pure-Rust renderer: the system OCR reads it, by design.
            ((true, true, true), d(System, false)),
            ((false, true, true), d(System, false)),
            ((true, false, true), d(PpOcr, false)),
            ((false, false, true), d(NoEngine, true)),
        ];
        for ((installed, system, unrendered), expected) in cases {
            assert_eq!(
                decide_ocr(auto, installed, || system, unrendered),
                expected,
                "auto: installed {installed}, system {system}, unrendered {unrendered}"
            );
        }
        assert_eq!(
            decide_ocr(OcrEngineChoice::PpOcr, false, || true, false),
            d(NoEngine, true)
        );
        assert_eq!(
            decide_ocr(OcrEngineChoice::PpOcr, true, || true, true),
            d(System, false)
        );
        assert_eq!(
            decide_ocr(OcrEngineChoice::System, true, || true, false),
            d(System, false)
        );
        assert_eq!(
            decide_ocr(OcrEngineChoice::System, false, || false, false),
            d(NoEngine, false)
        );
    }

    /// The system OCR is not consulted when the scan reader reads.
    #[test]
    fn ocr_route_does_not_ask_for_the_system_ocr_when_the_reader_reads() {
        let decision = decide_ocr(
            OcrEngineChoice::Auto,
            true,
            || panic!("the system OCR was consulted"),
            false,
        );
        assert_eq!(decision.engine, OcrEngine::PpOcr);
    }

    /// A scan read by the system OCR because the reader is missing is marked
    /// for reading again; one it reads by design is not.
    #[test]
    fn ocr_metadata_marks_scans_read_without_the_reader() {
        let mut metadata = HashMap::new();
        record_ocr(
            OcrUse {
                engine: OCR_ENGINE_SYSTEM,
                reader_missing: true,
            },
            &mut metadata,
        );
        assert_eq!(metadata[OCR_ENGINE_KEY], OCR_ENGINE_SYSTEM);
        assert_eq!(metadata[OCR_READER_MISSING_KEY], "true");
        let mut metadata = HashMap::new();
        record_ocr(
            OcrUse {
                engine: OCR_ENGINE_SYSTEM,
                reader_missing: false,
            },
            &mut metadata,
        );
        assert!(!metadata.contains_key(OCR_READER_MISSING_KEY));
    }

    /// OCR reports each page of a scanned PDF before reading it.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing pp-ocrv6/"
    )]
    fn ocr_reports_scanned_pages_as_it_reads() {
        let seen = std::sync::Mutex::new(Vec::new());
        let parsed = ppocr_parser()
            .parse_file_with_progress(
                &scanned("invoices/inv-2025-0164-scan.pdf"),
                &|page, pages| seen.lock().unwrap().push((page, pages)),
            )
            .expect("parsed scan");
        assert_eq!(
            parsed.metadata.get(OCR_ENGINE_KEY).map(String::as_str),
            Some(OCR_ENGINE_PPOCR)
        );
        assert_eq!(seen.into_inner().unwrap(), [(1, 2), (2, 2)]);
    }

    /// Non-ASCII text before a script or style used to put char and byte
    /// offsets out of step: the page panicked or its scripts became text.
    #[test]
    fn html_text_survives_non_ascii_before_scripts_and_styles() {
        let html = "<p>café</p><script>var x = '<p>hidden</p>';</script>\
                    <p>naïve</p><STYLE>p { color: red }</STYLE ><p>Größe</p>";
        assert_eq!(strip_html_tags(html), "café\nnaïve\nGröße");
    }

    #[test]
    fn html_tags_match_in_any_case_and_scripts_stay_out() {
        let html = "<DIV>One</DIV><Div>Two<BR>Three</div>\
                    <SCRIPT type=\"text/javascript\">steal(document.cookie)</SCRIPT>";
        assert_eq!(strip_html_tags(html), "One\nTwo\nThree");
    }

    #[test]
    fn html_entities_are_decoded_once() {
        let html = "<p>R&amp;D &lt;b&gt; &quot;q&quot; it&#39;s it&apos;s&nbsp;&nbsp;x \
                    &#x41;&#66; &unknown; &#xZZ; AT&T &amp;lt;</p>";
        assert_eq!(
            strip_html_tags(html),
            "R&D <b> \"q\" it's it's x AB &unknown; &#xZZ; AT&T &lt;"
        );
    }

    #[test]
    fn cjk_and_emoji_pages_keep_their_text() {
        let html = "<html><head><title>ページ</title></head><body>\
                    <h1>日本語のページ</h1><p>絵文字 😀 &amp; 記号</p>\
                    <ul><li>牛乳</li><li>🇮🇳 भारत</li></ul></body></html>";
        assert_eq!(
            strip_html_tags(html),
            "ページ\n日本語のページ\n絵文字 😀 & 記号\n牛乳\n🇮🇳 भारत"
        );
    }

    #[test]
    fn stray_brackets_comments_and_unclosed_tags() {
        let html = "<!DOCTYPE html><!-- <p>hidden</p> --><p>p < 0.05 and 3 > 2</p>\
                    <?xml version=\"1.0\"?><p>Open <b class=\"x";
        assert_eq!(strip_html_tags(html), "p < 0.05 and 3 > 2\nOpen");
        assert_eq!(strip_html_tags("<p>日本<script>never closed"), "日本");
        assert_eq!(strip_html_tags("text then <"), "text then <");
        assert_eq!(strip_html_tags("<!-- never closed"), "");
        // A quote outside an attribute value opens nothing: the page goes on.
        assert_eq!(
            strip_html_tags("<p>it's x<y and it's here</p><p>Next</p>"),
            "it's x\nNext"
        );
        assert_eq!(
            strip_html_tags("<a title = 'a > b' href=x>Link</a> after"),
            "Link after"
        );
    }

    #[test]
    fn html_table_cells_are_tab_separated_rows() {
        let html = "<table><tr><th>Name</th><th>Qty</th></tr>\
                    <tr><td>Milk</td><td>2</td></tr></table>";
        assert_eq!(strip_html_tags(html), "Name\tQty\nMilk\t2");
    }

    /// `<!-->` and `<!--->` close where they open; a CDATA section is its
    /// text as written, up to `]]>`.
    #[test]
    fn empty_comments_close_at_once_and_cdata_is_text() {
        assert_eq!(strip_html_tags("a<!-->b<!--->c<!-- x -->d"), "abcd");
        assert_eq!(
            strip_html_tags("<p>x<![CDATA[ 1 < 2 && a > b ]]>y</p>"),
            "x 1 < 2 && a > b y"
        );
        assert_eq!(strip_html_tags("<p>日本<![CDATA[語"), "日本語");
    }

    /// Numeric references to 0x80-0x9F mean the Windows-1252 characters of
    /// those bytes; the five bytes it leaves undefined keep their code.
    #[test]
    fn c1_references_are_read_as_windows_1252() {
        assert_eq!(strip_html_tags("&#150;"), "\u{2013}");
        assert_eq!(strip_html_tags("it&#146;s"), "it\u{2019}s");
        assert_eq!(
            strip_html_tags("&#x80;&#147;q&#148;&#159;"),
            "\u{20ac}\u{201c}q\u{201d}\u{178}"
        );
        assert_eq!(strip_html_tags("&#157;"), "\u{9d}");
        assert_eq!(strip_html_tags("&#160;x&#8211;"), "x\u{2013}");
    }

    /// A ZIP archive with `parts` (name, content), as Word and PowerPoint
    /// files are.
    fn office_file(path: &Path, parts: &[(&str, &str)]) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in parts {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        std::fs::write(path, bytes).unwrap();
    }

    const W_DOC: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
                         xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" \
                         xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" \
                         xmlns:v=\"urn:schemas-microsoft-com:vml\"";

    fn docx_paragraph(text: &str) -> String {
        format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
    }

    fn docx_cell(text: &str) -> String {
        format!("<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>")
    }

    /// DOCX text holds no XML: tab stops in paragraph properties, text
    /// boxes stored twice and entities are read as Word shows them, and a
    /// table is chunked as a table between the text around it.
    #[test]
    fn docx_text_is_the_document_text_and_its_tables_are_tables() {
        let text_box = "<w:txbxContent><w:p><w:r><w:t>Callout: budget approved</w:t></w:r></w:p></w:txbxContent>";
        let body = format!(
            "<w:p><w:pPr><w:tabs><w:tab w:val=\"left\" w:pos=\"720\"/></w:tabs></w:pPr>\
             <w:r><w:t>Owner</w:t></w:r><w:r><w:tab/><w:t>R&amp;D department</w:t></w:r></w:p>\
             {}\
             <w:p><w:r><mc:AlternateContent>\
             <mc:Choice Requires=\"wps\"><w:drawing><wps:wsp><wps:txbx>{text_box}</wps:txbx></wps:wsp></w:drawing></mc:Choice>\
             <mc:Fallback><w:pict><v:shape><v:textbox>{text_box}</v:textbox></v:shape></w:pict></mc:Fallback>\
             </mc:AlternateContent></w:r></w:p>\
             <w:tbl><w:tblPr><w:tblW w:w=\"0\"/></w:tblPr><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl>\
             {}",
            docx_paragraph("The research budget for the coming year covers the laboratory, two new hires and the travel to three conferences."),
            docx_cell("Item"),
            docx_cell("Cost"),
            docx_cell("Laboratory"),
            docx_cell("120,000"),
            docx_cell("Travel &amp; stay"),
            docx_cell("18,500"),
            docx_paragraph("The board reviews these figures again in the spring, once the grant decisions are known."),
        );
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document {W_DOC}><w:body>{body}</w:body></w:document>"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget.docx");
        office_file(&path, &[("word/document.xml", &xml)]);

        let parsed = DocumentParser::new().parse_file(&path).expect("parsed");
        let content = &parsed.content;
        assert!(
            !content.contains('<') && !content.contains("w:"),
            "{content}"
        );
        assert!(content.starts_with("Owner\tR&D department\n"), "{content}");
        assert_eq!(
            content.matches("Callout: budget approved").count(),
            1,
            "{content}"
        );
        assert!(content.contains("Travel & stay\t18,500"), "{content}");

        let tables: Vec<(&Vec<String>, &Vec<Vec<String>>)> = parsed
            .structured_sections
            .iter()
            .filter_map(|s| match s {
                DocumentSection::Table { headers, rows, .. } => Some((headers, rows)),
                _ => None,
            })
            .collect();
        assert_eq!(tables.len(), 1, "{:?}", parsed.structured_sections);
        assert_eq!(tables[0].0, &["Item", "Cost"]);
        assert_eq!(tables[0].1[1], ["Travel & stay", "18,500"]);
        let texts: Vec<(usize, &str)> = parsed
            .structured_sections
            .iter()
            .filter_map(|s| match s {
                DocumentSection::Text { content, page, .. } => Some((*page, content.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts.iter().all(|(page, _)| *page == 0));
        assert!(texts[0].1.contains("research budget") && texts[0].1.contains("Callout"));
        assert!(texts[1].1.contains("board reviews"));

        // Without a table the text is all there is.
        let plain = dir.path().join("plain.docx");
        let prose = docx_paragraph(
            "Only prose here, long enough to be a document of its own with nothing tabular in it at all.",
        );
        office_file(
            &plain,
            &[(
                "word/document.xml",
                &format!("<w:document {W_DOC}><w:body>{prose}</w:body></w:document>"),
            )],
        );
        let parsed = DocumentParser::new().parse_file(&plain).expect("parsed");
        assert!(parsed.structured_sections.is_empty());
        assert!(parsed.content.starts_with("Only prose here"));

        // Broken XML is reported, not read as no text.
        let broken = dir.path().join("broken.docx");
        office_file(
            &broken,
            &[("word/document.xml", "<w:document><w:body><w:p>")],
        );
        let err = format!(
            "{:#}",
            DocumentParser::new()
                .parse_file(&broken)
                .expect_err("broken")
        );
        assert!(err.contains("not valid XML"), "{err}");
    }

    /// PPTX text holds no tab lists or tags, and entities are decoded.
    #[test]
    fn pptx_text_is_the_slide_text() {
        let slide = |text: &str| {
            format!(
                "<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
                 xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree><p:sp><p:txBody>\
                 <a:bodyPr/><a:p><a:pPr><a:tabLst><a:tab pos=\"914400\" algn=\"l\"/></a:tabLst></a:pPr>\
                 <a:r><a:rPr lang=\"en-US\"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
            )
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("talk.pptx");
        let second = slide("Results &amp; next steps for the field trial in the coastal districts");
        let first = slide("Why we measured rainfall &lt;daily&gt; across forty stations this year");
        office_file(
            &path,
            &[
                ("ppt/slides/slide2.xml", &second),
                ("ppt/slides/slide1.xml", &first),
            ],
        );
        let parsed = DocumentParser::new().parse_file(&path).expect("parsed");
        assert_eq!(
            parsed.content,
            "--- Slide 1 ---\nWhy we measured rainfall <daily> across forty stations this year\n\n\
             --- Slide 2 ---\nResults & next steps for the field trial in the coastal districts"
        );
    }

    /// A file over its kind's limit is refused before it is read.
    #[test]
    fn a_file_over_its_limit_is_too_large_to_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(limits::MAX_TEXT_BYTES + 1).unwrap();
        drop(file);
        let err = DocumentParser::new()
            .parse_file(&path)
            .expect_err("too large");
        assert!(err.is::<limits::TooLarge>(), "{err:#}");
        assert!(
            format!("{err:#}").contains("too large to index (limit 50 MB for text files)"),
            "{err:#}"
        );
    }

    fn text_pages(sections: &[DocumentSection]) -> Vec<(usize, String)> {
        sections
            .iter()
            .filter_map(|s| match s {
                DocumentSection::Text { content, page, .. } => Some((*page, content.clone())),
                _ => None,
            })
            .collect()
    }

    /// When the layout parser cannot read a PDF, `pdf_extract`'s text is
    /// used unless lopdf's reads about as much.
    #[test]
    fn the_fallback_prefers_the_reader_that_reads_more() {
        let lopdf: Vec<(usize, String)> = vec![(1, "Inv 1".to_string()), (2, String::new())];
        let extract = "Invoice 1 for the supply of steel rods, payable within thirty days.";
        let paged = || {
            Some(vec![
                "Invoice 1 for the supply of steel rods,".to_string(),
                "payable within thirty days.".to_string(),
            ])
        };
        // pdf_extract reads more: its pages.
        assert_eq!(
            text_pages(&fallback_text_sections(&lopdf, extract, paged)),
            [
                (1, "Invoice 1 for the supply of steel rods,".to_string()),
                (2, "payable within thirty days.".to_string())
            ]
        );
        // Without pages, or with pages that miss text: unpaged.
        assert_eq!(
            text_pages(&fallback_text_sections(&lopdf, extract, || None)),
            [(0, extract.to_string())]
        );
        assert_eq!(
            text_pages(&fallback_text_sections(&lopdf, extract, || {
                Some(vec!["Invoice 1".to_string()])
            })),
            [(0, extract.to_string())]
        );
        // lopdf reads about as much: its pages; pdf_extract's are not asked for.
        let full: Vec<(usize, String)> = vec![(1, extract.to_string())];
        assert_eq!(
            text_pages(&fallback_text_sections(&full, extract, || {
                panic!("pdf_extract pages are not needed")
            })),
            [(1, extract.to_string())]
        );
        assert_eq!(
            text_pages(&fallback_text_sections(&lopdf, "", || None)),
            [(1, "Inv 1".to_string())]
        );
        assert!(fallback_text_sections(&[], "", || None).is_empty());
    }

    #[test]
    fn pdf_extract_reads_pages_only_when_it_reads_them_all() {
        use crate::processing::pdf_fixtures::{build_pdf, column};
        let bytes = build_pdf(
            &[
                column(72.0, 700.0, "first", 2),
                column(72.0, 700.0, "second", 2),
            ],
            None,
        );
        let pages = pdf_extract_pages(&bytes, 2).expect("two pages");
        assert!(
            pages[0].contains("first") && !pages[0].contains("second"),
            "{pages:?}"
        );
        assert!(
            pages[1].contains("second") && !pages[1].contains("first"),
            "{pages:?}"
        );
        assert_eq!(pdf_extract_pages(&bytes, 3), None);
    }

    #[test]
    fn file_names_that_are_ids_are_told_from_titles() {
        for id in [
            "1706.03762v7",
            "2310.06825",
            "20231015",
            "2023-10-15",
            "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
            "a3f9c2e1b4d5e6f7",
            "PMC1234567",
            "IMG_0042",
            "",
        ] {
            assert!(is_identifier_stem(id), "{id:?}");
        }
        for title in [
            "attention",
            "report2023",
            "My thesis draft",
            "notes",
            "Q3 results 2023",
            "deadbeef",
            "lease-scan",
        ] {
            assert!(!is_identifier_stem(title), "{title:?}");
        }
    }

    /// A PDF named by its arXiv id is titled by the title printed on it; a
    /// PDF with a name keeps it.
    #[test]
    fn a_pdf_named_by_an_id_is_titled_by_its_printed_title() {
        use crate::processing::pdf_fixtures::{bold, build_pdf, column};
        let mut page = vec![bold(72.0, 740.0, 18.0, "Attention Is All You Need")];
        page.push(bold(72.0, 700.0, 12.0, "1 Introduction"));
        page.extend(column(72.0, 684.0, "intro", 8));
        let bytes = build_pdf(&[page], None);
        let dir = tempfile::tempdir().unwrap();
        let arxiv = dir.path().join("1706.03762v7.pdf");
        std::fs::write(&arxiv, &bytes).unwrap();
        let parsed = DocumentParser::new().parse_file(&arxiv).expect("parsed");
        assert_eq!(parsed.title, "Attention Is All You Need");
        let named = dir.path().join("transformer-paper.pdf");
        std::fs::write(&named, &bytes).unwrap();
        let parsed = DocumentParser::new().parse_file(&named).expect("parsed");
        assert_eq!(parsed.title, "transformer-paper");
    }

    /// OCR that fails says why, instead of reporting a scan with no text.
    #[test]
    fn an_ocr_failure_is_reported_with_its_reason() {
        use crate::processing::ocr::worker::WorkerCommand;
        use crate::processing::pdf_fixtures::build_pdf;
        let worker = OcrWorker::new(WorkerCommand {
            program: std::path::PathBuf::from("this-ocr-worker-does-not-exist"),
            args: Vec::new(),
            env: Vec::new(),
        });
        let dir = tempfile::tempdir().unwrap();
        worker.set_model_dir(Some(dir.path().join("pp-ocrv6")));
        let parser = DocumentParser::new().with_ocr_worker(Arc::new(worker));
        let scan = dir.path().join("scan.pdf");
        std::fs::write(&scan, build_pdf(&[vec![]], None)).unwrap();
        let err = format!("{:#}", parser.parse_file(&scan).expect_err("OCR failed"));
        assert!(err.contains("OCR could not read this scanned PDF"), "{err}");
        assert!(err.contains("starting the OCR process"), "{err}");
        assert!(!err.contains("no extractable text"), "{err}");
    }

    #[test]
    fn unread_pages_are_named() {
        assert_eq!(page_list(&[4]), "page 4");
        assert_eq!(page_list(&[4, 9]), "pages 4, 9");
        assert_eq!(
            page_list(&(1..=15).collect::<Vec<_>>()),
            "pages 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 and 5 more"
        );
        let metadata = HashMap::from([(OCR_FAILED_PAGES_KEY.to_string(), "4,9".to_string())]);
        assert_eq!(
            ocr_failed_pages_notice(&metadata).as_deref(),
            Some("pages 4, 9 could not be read")
        );
        assert_eq!(ocr_failed_pages_notice(&HashMap::new()), None);
    }
}
