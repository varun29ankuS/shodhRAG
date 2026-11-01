//! Windows system OCR (Windows.Media.Ocr) of images and of PDF pages
//! rendered by Windows.Data.Pdf: the fallback when the PP-OCRv6 model is not
//! installed ([`super::ocr`]), and the reader of scans whose images the
//! pure-Rust renderer cannot decode (JBIG2, JPEG 2000).

use anyhow::{Context, Result};
use std::path::Path;

use super::ocr::OcrPages;

use windows::core::HSTRING;
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::StorageFile;
use windows::Storage::Streams::InMemoryRandomAccessStream;

/// Device-independent pixels per inch: Windows.Data.Pdf page sizes are in DIPs.
const DIPS_PER_INCH: f64 = 96.0;

fn absolute(path: &Path) -> Result<std::path::PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

fn storage_file(path: &Path) -> Result<StorageFile> {
    let path_str = HSTRING::from(absolute(path)?.to_string_lossy().as_ref());
    StorageFile::GetFileFromPathAsync(&path_str)
        .context("Failed to create StorageFile async op")?
        .get()
        .with_context(|| format!("Failed to open {}", path.display()))
}

/// A PDF opened with Windows.Data.Pdf.
struct WindowsPdf {
    doc: PdfDocument,
}

impl WindowsPdf {
    fn open(path: &Path) -> Result<Self> {
        let file = storage_file(path)?;
        let doc = PdfDocument::LoadFromFileAsync(&file)
            .context("Failed to create PdfDocument async op")?
            .get()
            .context("Failed to load PDF document")?;
        Ok(Self { doc })
    }

    fn page_count(&self) -> Result<usize> {
        Ok(self
            .doc
            .PageCount()
            .context("Failed to get PDF page count")? as usize)
    }

    /// The longer side of page `index` (0-based), in inches.
    fn longer_side_inches(&self, index: usize) -> Result<f64> {
        let size = self
            .doc
            .GetPage(index as u32)
            .and_then(|p| p.Size())
            .context("Failed to get the PDF page size")?;
        Ok(size.Width.max(size.Height) as f64 / DIPS_PER_INCH)
    }

    /// Renders page `index` (0-based) into a stream: at `dpi` when given, else
    /// at the page's size in DIPs (96 dpi). The stream holds a PNG and is
    /// positioned at its start.
    fn render_stream(&self, index: usize, dpi: Option<u32>) -> Result<InMemoryRandomAccessStream> {
        let page = self
            .doc
            .GetPage(index as u32)
            .with_context(|| format!("Failed to get PDF page {}", index + 1))?;
        let stream =
            InMemoryRandomAccessStream::new().context("Failed to create in-memory stream")?;
        match dpi {
            Some(dpi) => {
                let size = page.Size().context("Failed to get the PDF page size")?;
                let scale = dpi as f64 / DIPS_PER_INCH;
                let options =
                    PdfPageRenderOptions::new().context("Failed to create render options")?;
                options
                    .SetDestinationWidth(((size.Width as f64 * scale).round() as u32).max(1))
                    .context("Failed to set the render width")?;
                options
                    .SetDestinationHeight(((size.Height as f64 * scale).round() as u32).max(1))
                    .context("Failed to set the render height")?;
                page.RenderWithOptionsToStreamAsync(&stream, &options)
                    .context("Failed to create render async op")?
                    .get()
                    .with_context(|| format!("Failed to render PDF page {}", index + 1))?;
            }
            None => {
                page.RenderToStreamAsync(&stream)
                    .context("Failed to create render async op")?
                    .get()
                    .with_context(|| format!("Failed to render PDF page {}", index + 1))?;
            }
        }
        stream.Seek(0).context("Failed to seek stream")?;
        Ok(stream)
    }
}

fn engine() -> Result<OcrEngine> {
    OcrEngine::TryCreateFromUserProfileLanguages()
        .context("Windows OCR engine not available — install a language pack")
}

/// OCR a scanned PDF by rendering each page and running Windows OCR: at
/// `dpi` when given (lowered where the page would exceed the engine's largest
/// image size), else at the page size in DIPs (96 dpi). Returns one string per
/// page, in page order (empty for blank pages). `on_page(page, pages)` is
/// called before each page is read (1-based). A page that cannot be rendered
/// or read is logged, reads as empty and is listed as failed; the other
/// pages are kept ([`super::ocr::read_each_page`]).
pub fn ocr_pdf_pages(
    path: &Path,
    dpi: Option<u32>,
    on_page: &dyn Fn(usize, usize),
) -> Result<OcrPages> {
    let pdf = WindowsPdf::open(path)?;
    let engine = engine()?;
    let max_side = OcrEngine::MaxImageDimension().context("Failed to get the OCR image limit")?;
    let page_count = pdf.page_count()?;
    Ok(super::ocr::read_each_page(
        page_count,
        path,
        on_page,
        |index| read_pdf_page(&pdf, &engine, max_side, index, dpi),
    )?)
}

/// Page `index` (0-based) of `pdf` rendered and read by `engine` (see
/// [`ocr_pdf_pages`]).
fn read_pdf_page(
    pdf: &WindowsPdf,
    engine: &OcrEngine,
    max_side: u32,
    index: usize,
    dpi: Option<u32>,
) -> Result<String> {
    let page = index + 1;
    let dpi = match dpi {
        Some(dpi) => {
            let cap = (max_side as f64 / pdf.longer_side_inches(index)?).floor() as u32;
            Some(dpi.min(cap.max(1)))
        }
        None => None,
    };
    let stream = pdf.render_stream(index, dpi)?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .context("Failed to create bitmap decoder async op")?
        .get()
        .with_context(|| format!("Failed to decode rendered page {page}"))?;

    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .context("Failed to create bitmap async op")?
        .get()
        .with_context(|| format!("Failed to get bitmap for page {page}"))?;

    let converted = SoftwareBitmap::Convert(&bitmap, BitmapPixelFormat::Bgra8)
        .with_context(|| format!("Failed to convert bitmap for page {page}"))?;

    let result = engine
        .RecognizeAsync(&converted)
        .context("Failed to create OCR async op")?
        .get()
        .with_context(|| format!("OCR failed on page {page}"))?;

    Ok(result
        .Text()
        .with_context(|| format!("Failed to get OCR text for page {page}"))?
        .to_string())
}

/// OCR an image file (PNG, JPG, BMP, TIFF) using Windows OCR API.
pub fn ocr_image(path: &Path) -> Result<String> {
    let file = storage_file(path)?;

    let stream = file
        .OpenReadAsync()
        .context("Failed to create open async op")?
        .get()
        .context("Failed to open image stream")?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .context("Failed to create bitmap decoder async op")?
        .get()
        .context("Failed to decode image")?;

    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .context("Failed to create bitmap async op")?
        .get()
        .context("Failed to get software bitmap")?;

    let converted = SoftwareBitmap::Convert(&bitmap, BitmapPixelFormat::Bgra8)
        .context("Failed to convert bitmap format")?;

    let result = engine()?
        .RecognizeAsync(&converted)
        .context("Failed to create OCR async op")?
        .get()
        .context("OCR recognition failed")?;

    let text = result.Text().context("Failed to get OCR text")?.to_string();

    if text.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "OCR produced no text for image: {}",
            path.display()
        ));
    }

    Ok(text)
}

/// Check if Windows OCR is available on this system.
pub fn is_ocr_available() -> bool {
    OcrEngine::TryCreateFromUserProfileLanguages().is_ok()
}
