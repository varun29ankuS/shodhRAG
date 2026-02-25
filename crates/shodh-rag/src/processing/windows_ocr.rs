use anyhow::{Context, Result};
use std::path::Path;

use windows::core::HSTRING;
use windows::Data::Pdf::PdfDocument;
use windows::Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::StorageFile;
use windows::Storage::Streams::InMemoryRandomAccessStream;

/// OCR a scanned PDF by rendering each page and running Windows OCR.
pub fn ocr_pdf(path: &Path) -> Result<String> {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let path_str = HSTRING::from(abs_path.to_string_lossy().as_ref());
    let file = StorageFile::GetFileFromPathAsync(&path_str)
        .context("Failed to create StorageFile async op")?
        .get()
        .context("Failed to open file for PDF OCR")?;

    let pdf = PdfDocument::LoadFromFileAsync(&file)
        .context("Failed to create PdfDocument async op")?
        .get()
        .context("Failed to load PDF document")?;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .context("Windows OCR engine not available — install a language pack")?;

    let page_count = pdf.PageCount().context("Failed to get PDF page count")?;
    let mut all_text = String::new();

    for i in 0..page_count {
        let page = pdf
            .GetPage(i)
            .with_context(|| format!("Failed to get PDF page {}", i))?;

        let stream =
            InMemoryRandomAccessStream::new().context("Failed to create in-memory stream")?;

        page.RenderToStreamAsync(&stream)
            .context("Failed to create render async op")?
            .get()
            .with_context(|| format!("Failed to render PDF page {} to stream", i))?;

        stream.Seek(0).context("Failed to seek stream")?;

        let decoder = BitmapDecoder::CreateAsync(&stream)
            .context("Failed to create bitmap decoder async op")?
            .get()
            .with_context(|| format!("Failed to decode rendered page {}", i))?;

        let bitmap = decoder
            .GetSoftwareBitmapAsync()
            .context("Failed to create bitmap async op")?
            .get()
            .with_context(|| format!("Failed to get bitmap for page {}", i))?;

        let converted = SoftwareBitmap::Convert(&bitmap, BitmapPixelFormat::Bgra8)
            .with_context(|| format!("Failed to convert bitmap for page {}", i))?;

        let result = engine
            .RecognizeAsync(&converted)
            .context("Failed to create OCR async op")?
            .get()
            .with_context(|| format!("OCR failed on page {}", i))?;

        let page_text = result
            .Text()
            .with_context(|| format!("Failed to get OCR text for page {}", i))?
            .to_string();

        if !page_text.is_empty() {
            if !all_text.is_empty() {
                all_text.push('\n');
            }
            all_text.push_str(&page_text);
        }
    }

    if all_text.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "OCR produced no text for PDF: {}",
            path.display()
        ));
    }

    Ok(all_text)
}

/// OCR an image file (PNG, JPG, BMP, TIFF) using Windows OCR API.
pub fn ocr_image(path: &Path) -> Result<String> {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let path_str = HSTRING::from(abs_path.to_string_lossy().as_ref());
    let file = StorageFile::GetFileFromPathAsync(&path_str)
        .context("Failed to create StorageFile async op")?
        .get()
        .context("Failed to open image file for OCR")?;

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

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .context("Windows OCR engine not available — install a language pack")?;

    let result = engine
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
