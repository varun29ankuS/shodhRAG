//! PDF export through the WebView's own engine.
//!
//! An answer, a side-discussion summary or a gallery visual is printed by the
//! same renderer that shows it in the app, so KaTeX math, mermaid diagrams,
//! charts, sketches, paper figures, tables and every script (Devanagari,
//! CJK, Arabic) come out exactly as on screen.
//!
//! How:
//! 1. The document (title, date, markdown, sources) is registered as a print
//!    job under the label of a new window, which loads the app's
//!    `/print-view` page. That page asks for its job by its window label
//!    ([`print_job`]), renders it with the answer renderer and a print
//!    stylesheet, waits for fonts, figures and diagrams, and reports
//!    [`print_job_ready`].
//! 2. On Windows the window sits off-screen (a hidden WebView2 throttles
//!    rendering) and WebView2's native `PrintToPdf` writes the file
//!    ([`WebViewPdfPrinter`]). Elsewhere the window is shown and the system
//!    print dialog opens on it; the user saves the PDF from there.
//!
//! Files: the destination is reserved with an exclusive create before
//! printing, so an existing file is never replaced (` (2)`, ` (3)` … are
//! tried instead); a failed or timed-out print removes the reservation and
//! the window is always closed.
//!
//! The WebView2 call is behind [`PdfPrinter`], so the export logic (paths,
//! reservation, clean-up, the agent tool) is tested with a fake printer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;

use crate::agent_tools::files::{create_unique, sanitize_name};

/// The app page that renders a print job.
pub const PRINT_VIEW_PATH: &str = "print-view";
/// Labels of print windows start with this.
pub const PRINT_WINDOW_PREFIX: &str = "print-";
const MAX_TITLE_CHARS: usize = 300;
/// Longest markdown printed (the agent's `export_document` allows 200 000).
pub const MAX_MARKDOWN_CHARS: usize = 400_000;
const MAX_SOURCES: usize = 1_000;
const MAX_SOURCE_FIELD_CHARS: usize = 2_048;
/// How long the print view may take to render (paper figures are read from
/// disk and rasterised; mermaid and charts render asynchronously).
const RENDER_TIMEOUT: Duration = Duration::from_secs(90);
/// How long WebView2 may take to write the PDF.
const PRINT_TIMEOUT: Duration = Duration::from_secs(120);
/// Print window width in CSS pixels: a Letter or A4 page minus margins at
/// 96 dpi, so charts and sketches sized to the window fit the page.
const PRINT_WIDTH: f64 = 700.0;
const PRINT_HEIGHT: f64 = 1_000.0;
/// Page margins of the PDF, in inches.
pub const PAGE_MARGIN_INCHES: f64 = 0.6;

/// One entry of the Sources section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintSource {
    pub n: u32,
    /// File name, record title or web page title.
    pub title: String,
    /// "page 4", or a path for unpaged files.
    #[serde(default)]
    pub location: Option<String>,
    /// For web sources.
    #[serde(default)]
    pub url: Option<String>,
}

/// What a print job renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintDocument {
    pub title: String,
    /// A line under the title, e.g. "Side discussion about Table 2".
    #[serde(default)]
    pub subtitle: Option<String>,
    /// When the content was written (RFC 3339), shown as the date.
    pub created_at: String,
    /// The answer's markdown, citations as `[n]`.
    pub markdown: String,
    #[serde(default)]
    pub sources: Vec<PrintSource>,
}

impl PrintDocument {
    /// Bounds on what a print job may carry.
    pub fn validate(&self) -> Result<(), PdfExportError> {
        let invalid = |m: String| Err(PdfExportError::InvalidDocument(m));
        if self.title.trim().is_empty() {
            return invalid("the title is empty".into());
        }
        if self.title.chars().count() > MAX_TITLE_CHARS {
            return invalid(format!(
                "the title is longer than {MAX_TITLE_CHARS} characters"
            ));
        }
        if self
            .subtitle
            .as_ref()
            .is_some_and(|s| s.chars().count() > MAX_TITLE_CHARS)
        {
            return invalid(format!(
                "the subtitle is longer than {MAX_TITLE_CHARS} characters"
            ));
        }
        if chrono::DateTime::parse_from_rfc3339(&self.created_at).is_err() {
            return invalid("the date is not an RFC 3339 timestamp".into());
        }
        if self.markdown.trim().is_empty() {
            return invalid("there is nothing to print".into());
        }
        if self.markdown.chars().count() > MAX_MARKDOWN_CHARS {
            return invalid(format!(
                "the content is longer than {MAX_MARKDOWN_CHARS} characters"
            ));
        }
        if self.sources.len() > MAX_SOURCES {
            return invalid(format!("more than {MAX_SOURCES} sources"));
        }
        for s in &self.sources {
            let too_long = [Some(&s.title), s.location.as_ref(), s.url.as_ref()]
                .into_iter()
                .flatten()
                .any(|f| f.chars().count() > MAX_SOURCE_FIELD_CHARS);
            if s.n == 0 || s.title.trim().is_empty() || too_long {
                return invalid(format!("source [{}] is incomplete or too long", s.n));
            }
            if let Some(url) = &s.url {
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return invalid(format!("source [{}] has a URL that is not http(s)", s.n));
                }
            }
        }
        Ok(())
    }
}

/// Why a PDF export failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PdfExportError {
    #[error("Choose a full path for the PDF")]
    RelativePath,
    #[error("The folder {0} does not exist")]
    MissingFolder(String),
    #[error("The PDF cannot be printed: {0}")]
    InvalidDocument(String),
    #[error("{path} could not be created: {reason}")]
    Reserve { path: String, reason: String },
    #[error(
        "This system cannot write PDF files directly; use the print dialog and choose Save as PDF"
    )]
    Unsupported,
    #[error("The print view could not be opened: {0}")]
    Window(String),
    #[error("The print view did not finish rendering within {0} seconds")]
    RenderTimeout(u64),
    #[error("The print view could not render the content: {0}")]
    Render(String),
    #[error("Printing to PDF failed: {0}")]
    Print(String),
    #[error("Printing to PDF did not finish within {0} seconds")]
    PrintTimeout(u64),
}

/// Prints a document to a PDF file. Implemented with WebView2 on Windows;
/// tests use a fake.
#[async_trait]
pub trait PdfPrinter: Send + Sync {
    /// Whether this system writes PDF files itself. When false, only the
    /// print dialog is available ([`WebViewPdfPrinter::open_print_dialog`]).
    fn writes_files(&self) -> bool;
    /// Render `document` and write it as PDF to `path`, which exists (an
    /// empty file reserved for it) and may be overwritten.
    async fn print(&self, document: &PrintDocument, path: &Path) -> Result<(), PdfExportError>;
}

/// The folder and file stem of a path the user picked in a save dialog: it
/// must be absolute and its folder must exist; the extension is always
/// `.pdf` and the name is made safe.
pub fn split_destination(requested: &str) -> Result<(PathBuf, String), PdfExportError> {
    let path = Path::new(requested.trim());
    if !path.is_absolute() {
        return Err(PdfExportError::RelativePath);
    }
    let folder = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(PdfExportError::RelativePath)?;
    if !folder.is_dir() {
        return Err(PdfExportError::MissingFolder(folder.display().to_string()));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name
        .strip_suffix(".pdf")
        .or_else(|| name.strip_suffix(".PDF"))
        .unwrap_or(&name);
    Ok((folder.to_path_buf(), sanitize_name(stem, "Shodh export")))
}

/// Print into an already reserved (empty, exclusively created) `path`. On
/// any failure the reservation is removed, so no empty or partial file is
/// left behind. Returns the size of the written PDF.
pub async fn print_reserved(
    printer: &dyn PdfPrinter,
    document: &PrintDocument,
    path: &Path,
) -> Result<u64, PdfExportError> {
    let outcome = match printer.print(document, path).await {
        Ok(()) => match std::fs::metadata(path) {
            Ok(meta) if meta.len() > 0 => Ok(meta.len()),
            Ok(_) => Err(PdfExportError::Print("the PDF is empty".to_string())),
            Err(e) => Err(PdfExportError::Print(format!(
                "the PDF was not written ({e})"
            ))),
        },
        Err(e) => Err(e),
    };
    if outcome.is_err() {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "removing a failed PDF export failed");
            }
        }
    }
    outcome
}

/// Reserve the first free `stem.pdf`, `stem (2).pdf`, … in `folder` and
/// print there. Returns the path written and its size.
pub async fn write_pdf(
    printer: &dyn PdfPrinter,
    document: &PrintDocument,
    folder: &Path,
    stem: &str,
) -> Result<(PathBuf, u64), PdfExportError> {
    document.validate()?;
    if !printer.writes_files() {
        return Err(PdfExportError::Unsupported);
    }
    let (path, file) = create_unique(folder, stem, "pdf").map_err(|e| PdfExportError::Reserve {
        path: folder.join(format!("{stem}.pdf")).display().to_string(),
        reason: e.to_string(),
    })?;
    // The printer opens the file itself; a handle kept open here would make
    // its write fail with a sharing violation.
    drop(file);
    let bytes = print_reserved(printer, document, &path).await?;
    Ok((path, bytes))
}

// ── Print jobs ─────────────────────────────────────────────────────────────

/// What the print view receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintJobView {
    pub document: PrintDocument,
    /// True when the PDF is written by the app (no dialog); false when the
    /// system print dialog will open on the view.
    pub native: bool,
}

struct PrintJob {
    document: PrintDocument,
    native: bool,
    ready: Option<oneshot::Sender<Result<(), String>>>,
}

/// Open print jobs, by the label of the window rendering them.
#[derive(Default)]
pub struct PrintJobs {
    jobs: Mutex<HashMap<String, PrintJob>>,
}

impl PrintJobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PrintJob>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a job for the window `label`; the receiver resolves when its
    /// view reports it rendered (or failed).
    pub fn open(
        &self,
        label: &str,
        document: PrintDocument,
        native: bool,
    ) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        self.lock().insert(
            label.to_string(),
            PrintJob {
                document,
                native,
                ready: Some(tx),
            },
        );
        rx
    }

    /// The job of window `label`.
    pub fn view(&self, label: &str) -> Option<PrintJobView> {
        self.lock().get(label).map(|j| PrintJobView {
            document: j.document.clone(),
            native: j.native,
        })
    }

    /// The view of window `label` finished (or failed) rendering. False when
    /// there is no such job or it already reported.
    pub fn ready(&self, label: &str, result: Result<(), String>) -> bool {
        let sender = self.lock().get_mut(label).and_then(|j| j.ready.take());
        sender.is_some_and(|tx| tx.send(result).is_ok())
    }

    pub fn close(&self, label: &str) {
        self.lock().remove(label);
    }
}

/// Wait for a print view to render.
async fn wait_rendered(rx: oneshot::Receiver<Result<(), String>>) -> Result<(), PdfExportError> {
    match tokio::time::timeout(RENDER_TIMEOUT, rx).await {
        Err(_) => Err(PdfExportError::RenderTimeout(RENDER_TIMEOUT.as_secs())),
        Ok(Err(_)) => Err(PdfExportError::Render(
            "the print view closed before it finished".to_string(),
        )),
        Ok(Ok(Err(message))) => Err(PdfExportError::Render(message)),
        Ok(Ok(Ok(()))) => Ok(()),
    }
}

// ── The WebView printer ────────────────────────────────────────────────────

/// Prints with the app's own WebView: WebView2 `PrintToPdf` on Windows, the
/// system print dialog elsewhere.
pub struct WebViewPdfPrinter {
    app: AppHandle,
    jobs: Arc<PrintJobs>,
}

impl WebViewPdfPrinter {
    pub fn new(app: AppHandle, jobs: Arc<PrintJobs>) -> Self {
        Self { app, jobs }
    }

    /// Open a window on the print view for `document` and wait until it has
    /// rendered. `offscreen` windows are placed outside every screen.
    async fn open_view(
        &self,
        document: &PrintDocument,
        native: bool,
        offscreen: bool,
    ) -> Result<(WebviewWindow, String), PdfExportError> {
        let label = format!("{PRINT_WINDOW_PREFIX}{}", uuid::Uuid::new_v4().simple());
        let rx = self.jobs.open(&label, document.clone(), native);
        let builder = crate::profile::webview_storage(WebviewWindowBuilder::new(
            &self.app,
            &label,
            WebviewUrl::App(PRINT_VIEW_PATH.into()),
        ))
        .title(format!("Print — {}", document.title))
        .inner_size(PRINT_WIDTH, PRINT_HEIGHT);
        // Off every screen (not -32000, which Windows uses for minimized
        // windows), visible so WebView2 keeps rendering.
        let builder = if offscreen {
            builder
                .position(-16_000.0, -16_000.0)
                .visible(true)
                .focused(false)
                .skip_taskbar(true)
                .decorations(false)
                .resizable(false)
        } else {
            builder.center().visible(true)
        };
        let window = match builder.build() {
            Ok(w) => w,
            Err(e) => {
                self.jobs.close(&label);
                return Err(PdfExportError::Window(e.to_string()));
            }
        };
        if let Err(e) = wait_rendered(rx).await {
            self.jobs.close(&label);
            close_window(&window);
            return Err(e);
        }
        Ok((window, label))
    }

    /// Show the print view and open the system print dialog on it (systems
    /// without a native PDF writer). The window stays open for the dialog;
    /// the user closes it.
    pub async fn open_print_dialog(&self, document: &PrintDocument) -> Result<(), PdfExportError> {
        document.validate()?;
        let (window, label) = self.open_view(document, false, false).await?;
        self.jobs.close(&label);
        window
            .print()
            .map_err(|e| PdfExportError::Print(e.to_string()))
    }
}

fn close_window(window: &WebviewWindow) {
    if let Err(e) = window.destroy() {
        tracing::warn!(label = %window.label(), error = %e, "closing a print window failed");
    }
}

#[async_trait]
impl PdfPrinter for WebViewPdfPrinter {
    fn writes_files(&self) -> bool {
        cfg!(windows)
    }

    async fn print(&self, document: &PrintDocument, path: &Path) -> Result<(), PdfExportError> {
        if !self.writes_files() {
            return Err(PdfExportError::Unsupported);
        }
        let (window, label) = self.open_view(document, true, true).await?;
        let result = print_window_to_pdf(&window, path).await;
        self.jobs.close(&label);
        close_window(&window);
        result
    }
}

/// WebView2 `ICoreWebView2_7::PrintToPdf` on the window's webview.
#[cfg(windows)]
async fn print_window_to_pdf(window: &WebviewWindow, path: &Path) -> Result<(), PdfExportError> {
    let (tx, rx) = oneshot::channel::<Result<(), String>>();
    let done = Arc::new(Mutex::new(Some(tx)));
    let target = path.to_path_buf();
    window
        .with_webview(move |webview| {
            if let Err(e) = webview2::start_print_to_pdf(&webview, &target, done.clone()) {
                webview2::finish(&done, Err(e.to_string()));
            }
        })
        .map_err(|e| PdfExportError::Print(e.to_string()))?;
    match tokio::time::timeout(PRINT_TIMEOUT, rx).await {
        Err(_) => Err(PdfExportError::PrintTimeout(PRINT_TIMEOUT.as_secs())),
        Ok(Err(_)) => Err(PdfExportError::Print(
            "the print view closed while printing".to_string(),
        )),
        Ok(Ok(result)) => result.map_err(PdfExportError::Print),
    }
}

#[cfg(not(windows))]
async fn print_window_to_pdf(_window: &WebviewWindow, _path: &Path) -> Result<(), PdfExportError> {
    Err(PdfExportError::Unsupported)
}

#[cfg(windows)]
mod webview2 {
    //! The one unsafe call: WebView2's PrintToPdf with print settings.

    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use tauri::webview::PlatformWebview;
    use tokio::sync::oneshot;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment6, ICoreWebView2_7,
    };
    use webview2_com::PrintToPdfCompletedHandler;
    use windows_core::{Interface, PCWSTR};

    use super::PAGE_MARGIN_INCHES;

    pub type Done = Arc<Mutex<Option<oneshot::Sender<Result<(), String>>>>>;

    /// Report the outcome once (later reports are ignored).
    pub fn finish(done: &Done, result: Result<(), String>) {
        let sender = done.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(tx) = sender {
            // The caller may have timed out and gone; nothing to tell then.
            let _ = tx.send(result);
        }
    }

    /// Start printing the webview to `path`; completion arrives on `done`.
    /// Runs on the UI thread (inside `with_webview`).
    pub fn start_print_to_pdf(
        webview: &PlatformWebview,
        path: &Path,
        done: Done,
    ) -> windows_core::Result<()> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: COM calls on the webview's own UI thread (with_webview runs
        // the closure there); `wide` is NUL-terminated and outlives the call,
        // which copies the path before returning.
        unsafe {
            let core: ICoreWebView2_7 = webview.controller().CoreWebView2()?.cast()?;
            let environment: ICoreWebView2Environment6 = webview.environment().cast()?;
            let settings = environment.CreatePrintSettings()?;
            settings.SetShouldPrintBackgrounds(true)?;
            settings.SetShouldPrintHeaderAndFooter(false)?;
            settings.SetMarginTop(PAGE_MARGIN_INCHES)?;
            settings.SetMarginBottom(PAGE_MARGIN_INCHES)?;
            settings.SetMarginLeft(PAGE_MARGIN_INCHES)?;
            settings.SetMarginRight(PAGE_MARGIN_INCHES)?;
            let handler = PrintToPdfCompletedHandler::create(Box::new(move |result, ok| {
                let outcome = match result {
                    Err(e) => Err(e.to_string()),
                    Ok(()) if !ok => {
                        Err("WebView2 reported that the PDF was not written".to_string())
                    }
                    Ok(()) => Ok(()),
                };
                finish(&done, outcome);
                Ok(())
            }));
            core.PrintToPdf(PCWSTR::from_raw(wide.as_ptr()), &settings, &handler)
        }
    }
}

// ── Commands ───────────────────────────────────────────────────────────────

/// Print jobs and the printer, shared by the commands and the agent.
pub struct PdfExportState {
    pub jobs: Arc<PrintJobs>,
    pub printer: Arc<WebViewPdfPrinter>,
}

impl PdfExportState {
    pub fn new(app: AppHandle) -> Self {
        let jobs = Arc::new(PrintJobs::default());
        let printer = Arc::new(WebViewPdfPrinter::new(app, jobs.clone()));
        Self { jobs, printer }
    }
}

/// What an export did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfExportOutcome {
    /// The file written (never one that existed before).
    pub path: Option<String>,
    pub bytes: Option<u64>,
    /// False when the system print dialog was opened instead.
    pub written: bool,
}

/// Export → PDF from the UI: write `document` to `path` (picked in a save
/// dialog; an existing file there is kept and the PDF gets a suffixed name),
/// or open the system print dialog where the app cannot write PDFs itself.
#[tauri::command]
pub async fn export_pdf(
    app: tauri::AppHandle,
    state: tauri::State<'_, PdfExportState>,
    document: PrintDocument,
    path: Option<String>,
) -> Result<PdfExportOutcome, String> {
    document.validate().map_err(|e| e.to_string())?;
    let printer = state.printer.clone();
    if !printer.writes_files() {
        printer
            .open_print_dialog(&document)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(PdfExportOutcome {
            path: None,
            bytes: None,
            written: false,
        });
    }
    let requested = path.ok_or_else(|| PdfExportError::RelativePath.to_string())?;
    let (folder, stem) = split_destination(&requested).map_err(|e| e.to_string())?;
    let (written, bytes) = write_pdf(printer.as_ref(), &document, &folder, &stem)
        .await
        .map_err(|e| e.to_string())?;
    let shown = written.display().to_string();
    crate::inbox_commands::post(
        &app,
        crate::inbox_commands::export_item(&shown, true, "Exported", Some(&shown), ""),
    )
    .await;
    Ok(PdfExportOutcome {
        path: Some(written.display().to_string()),
        bytes: Some(bytes),
        written: true,
    })
}

/// The print job of the calling print window.
#[tauri::command]
pub fn print_job(
    window: WebviewWindow,
    state: tauri::State<'_, PdfExportState>,
) -> Result<PrintJobView, String> {
    let label = window.label();
    if !label.starts_with(PRINT_WINDOW_PREFIX) {
        return Err("Only a print window has a print job".to_string());
    }
    state
        .jobs
        .view(label)
        .ok_or_else(|| "This print job is no longer open".to_string())
}

/// The calling print window finished rendering (`error` when it could not).
#[tauri::command]
pub fn print_job_ready(
    window: WebviewWindow,
    state: tauri::State<'_, PdfExportState>,
    error: Option<String>,
) -> Result<(), String> {
    let label = window.label();
    if !label.starts_with(PRINT_WINDOW_PREFIX) {
        return Err("Only a print window can report a print job".to_string());
    }
    let result = match error {
        Some(message) => Err(message.chars().take(500).collect()),
        None => Ok(()),
    };
    if state.jobs.ready(label, result) {
        Ok(())
    } else {
        Err("This print job is no longer open".to_string())
    }
}

/// Managed state for the PDF commands.
pub fn manage(app: &AppHandle) {
    app.manage(PdfExportState::new(app.clone()));
}

#[cfg(test)]
pub(crate) mod testing {
    //! A printer that writes a small PDF-like file, or fails.

    use super::*;

    pub struct FakePrinter {
        pub native: bool,
        pub fail: Option<PdfExportError>,
        pub printed: Mutex<Vec<(PrintDocument, PathBuf)>>,
    }

    impl FakePrinter {
        pub fn writing() -> Self {
            Self {
                native: true,
                fail: None,
                printed: Mutex::new(Vec::new()),
            }
        }
        pub fn failing(error: PdfExportError) -> Self {
            Self {
                fail: Some(error),
                ..Self::writing()
            }
        }
        pub fn dialog_only() -> Self {
            Self {
                native: false,
                ..Self::writing()
            }
        }
        pub fn printed(&self) -> Vec<(PrintDocument, PathBuf)> {
            self.printed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl PdfPrinter for FakePrinter {
        fn writes_files(&self) -> bool {
            self.native
        }
        async fn print(&self, document: &PrintDocument, path: &Path) -> Result<(), PdfExportError> {
            self.printed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((document.clone(), path.to_path_buf()));
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            std::fs::write(path, b"%PDF-1.7\n% fake\n%%EOF\n")
                .map_err(|e| PdfExportError::Print(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakePrinter;
    use super::*;

    fn document() -> PrintDocument {
        PrintDocument {
            title: "Notice periods".into(),
            subtitle: None,
            created_at: "2026-10-04T10:00:00Z".into(),
            markdown: "The notice period is 60 days [1].".into(),
            sources: vec![PrintSource {
                n: 1,
                title: "lease.pdf".into(),
                location: Some("page 4".into()),
                url: None,
            }],
        }
    }

    #[test]
    fn documents_are_validated() {
        assert!(document().validate().is_ok());
        let mut d = document();
        d.title = "  ".into();
        assert!(d.validate().is_err());
        let mut d = document();
        d.created_at = "yesterday".into();
        assert!(d.validate().is_err());
        let mut d = document();
        d.markdown = "x".repeat(MAX_MARKDOWN_CHARS + 1);
        assert!(d.validate().is_err());
        let mut d = document();
        d.sources[0].url = Some("javascript:alert(1)".into());
        assert!(d.validate().is_err());
        let mut d = document();
        d.sources[0].n = 0;
        assert!(d.validate().is_err());
    }

    #[test]
    fn destinations_need_an_absolute_path_in_an_existing_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            split_destination("report.pdf").unwrap_err(),
            PdfExportError::RelativePath
        );
        let missing = dir.path().join("nope").join("a.pdf");
        assert!(matches!(
            split_destination(&missing.display().to_string()).unwrap_err(),
            PdfExportError::MissingFolder(_)
        ));
        let (folder, stem) =
            split_destination(&dir.path().join("Q3: results.pdf").display().to_string()).unwrap();
        assert_eq!(folder, dir.path());
        assert_eq!(stem, "Q3_ results");
        let (_, stem) = split_destination(&dir.path().join("notes").display().to_string()).unwrap();
        assert_eq!(stem, "notes");
    }

    #[tokio::test]
    async fn an_existing_file_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Notice.pdf"), b"keep me").unwrap();
        let printer = FakePrinter::writing();
        let (path, bytes) = write_pdf(&printer, &document(), dir.path(), "Notice")
            .await
            .unwrap();
        assert_eq!(path, dir.path().join("Notice (2).pdf"));
        assert!(bytes > 0);
        assert_eq!(
            std::fs::read(dir.path().join("Notice.pdf")).unwrap(),
            b"keep me"
        );
        assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF"));
        assert_eq!(printer.printed()[0].0.title, "Notice periods");
    }

    #[tokio::test]
    async fn a_failed_print_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let printer = FakePrinter::failing(PdfExportError::RenderTimeout(90));
        let err = write_pdf(&printer, &document(), dir.path(), "Notice")
            .await
            .unwrap_err();
        assert_eq!(err, PdfExportError::RenderTimeout(90));
        assert!(!dir.path().join("Notice.pdf").exists());

        // A reservation made by the caller (an approved exact path) is removed too.
        let exact = dir.path().join("Exact.pdf");
        drop(crate::agent_tools::files::create_exact(&exact).unwrap());
        let err = print_reserved(&printer, &document(), &exact)
            .await
            .unwrap_err();
        assert_eq!(err, PdfExportError::RenderTimeout(90));
        assert!(!exact.exists());
    }

    #[tokio::test]
    async fn dialog_only_systems_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let printer = FakePrinter::dialog_only();
        let err = write_pdf(&printer, &document(), dir.path(), "Notice")
            .await
            .unwrap_err();
        assert_eq!(err, PdfExportError::Unsupported);
        assert!(printer.printed().is_empty());
        assert!(!dir.path().join("Notice.pdf").exists());
    }

    #[test]
    fn print_jobs_are_found_by_window_and_report_once() {
        let jobs = PrintJobs::default();
        let mut rx = jobs.open("print-1", document(), true);
        assert_eq!(
            jobs.view("print-1").unwrap().document.title,
            "Notice periods"
        );
        assert!(jobs.view("print-2").is_none());
        assert!(jobs.ready("print-1", Ok(())));
        assert!(!jobs.ready("print-1", Ok(())), "a job reports once");
        assert_eq!(rx.try_recv().unwrap(), Ok(()));
        jobs.close("print-1");
        assert!(jobs.view("print-1").is_none());
        assert!(!jobs.ready("print-1", Err("late".into())));
    }

    #[tokio::test]
    async fn a_failed_render_is_reported() {
        let jobs = PrintJobs::default();
        let rx = jobs.open("print-1", document(), true);
        jobs.ready("print-1", Err("mermaid failed".into()));
        assert_eq!(
            wait_rendered(rx).await.unwrap_err(),
            PdfExportError::Render("mermaid failed".into())
        );
        let rx = jobs.open("print-2", document(), true);
        jobs.close("print-2");
        assert!(matches!(
            wait_rendered(rx).await.unwrap_err(),
            PdfExportError::Render(_)
        ));
    }
}
