//! Image upload, OCR (Windows OCR API), and form export commands

use serde::{Deserialize, Serialize};
use shodh_rag::rag::{export_form_as_html, export_form_as_json_schema, FormField};
use tauri::State;
use uuid::Uuid;

use crate::rag_commands::RagState;

/// Result of image processing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageProcessResult {
    pub id: String,
    pub extracted_text: String,
    pub file_path: String,
    pub confidence: f32,
    pub word_count: usize,
    pub image_data: String,
}

// ─── Windows OCR via temp file + WinRT ──────────────────────────────────────

#[cfg(target_os = "windows")]
async fn run_ocr(image_bytes: &[u8]) -> Result<(String, f32), String> {
    // Save to a temp PNG file (Windows OCR needs a file stream)
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("shodh_ocr_{}.png", Uuid::new_v4()));

    // Decode and re-encode as PNG to ensure valid format
    let img = image::load_from_memory(image_bytes)
        .map_err(|e| format!("Failed to decode image: {}", e))?;
    img.save_with_format(&tmp_path, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to save temp image: {}", e))?;

    let result = run_ocr_on_file(tmp_path.clone()).await;
    let _ = std::fs::remove_file(&tmp_path);
    result
}

#[cfg(target_os = "windows")]
async fn run_ocr_on_file(path: std::path::PathBuf) -> Result<(String, f32), String> {
    // WinRT IAsyncOperation does not implement Rust Future — run blocking on a separate thread
    tokio::task::spawn_blocking(move || {
        use windows::core::HSTRING;
        use windows::Graphics::Imaging::BitmapDecoder;
        use windows::Media::Ocr::OcrEngine;
        use windows::Storage::{FileAccessMode, StorageFile};

        let abs_path = std::fs::canonicalize(&path).map_err(|e| format!("Path error: {}", e))?;
        let path_str = abs_path.to_string_lossy().to_string();
        // Strip \\?\ prefix that canonicalize adds on Windows
        let clean_path = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);

        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(clean_path))
            .map_err(|e| format!("GetFileFromPath failed: {}", e))?
            .get()
            .map_err(|e| format!("GetFileFromPath get failed: {}", e))?;

        let stream = file
            .OpenAsync(FileAccessMode::Read)
            .map_err(|e| format!("OpenAsync failed: {}", e))?
            .get()
            .map_err(|e| format!("OpenAsync get failed: {}", e))?;

        let decoder = BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| format!("BitmapDecoder failed: {}", e))?
            .get()
            .map_err(|e| format!("BitmapDecoder get failed: {}", e))?;

        let bitmap = decoder
            .GetSoftwareBitmapAsync()
            .map_err(|e| format!("GetSoftwareBitmap failed: {}", e))?
            .get()
            .map_err(|e| format!("GetSoftwareBitmap get failed: {}", e))?;

        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|e| format!("OCR engine creation failed: {}", e))?;

        let ocr_result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| format!("RecognizeAsync failed: {}", e))?
            .get()
            .map_err(|e| format!("RecognizeAsync get failed: {}", e))?;

        let text = ocr_result
            .Text()
            .map_err(|e| format!("Text() failed: {}", e))?
            .to_string();

        let word_count = text.split_whitespace().count();
        let confidence: f32 = if word_count > 0 { 0.9 } else { 0.0 };

        Ok((text, confidence))
    })
    .await
    .map_err(|e| format!("OCR task panicked: {}", e))?
}

#[cfg(not(target_os = "windows"))]
async fn run_ocr(_image_bytes: &[u8]) -> Result<(String, f32), String> {
    Err("OCR is only available on Windows".to_string())
}

// ─── Commands ───────────────────────────────────────────────────────────────

/// Process an image from base64 data (paste/screenshot)
/// Encode raw RGBA pixels as a `data:image/png;base64,...` URI.
fn rgba_to_png_data_uri(rgba: Vec<u8>, width: u32, height: u32) -> Result<String, String> {
    use base64::Engine as _;

    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| format!("Clipboard image data does not match {width}x{height}"))?;
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to encode clipboard image: {e}"))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
    ))
}

/// The image on the system clipboard as a PNG data URI, or `None` when the
/// clipboard holds no image (e.g. text). Used by the Ctrl+V image paste.
#[tauri::command]
pub async fn read_clipboard_image(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    let (rgba, width, height) = match app.clipboard().read_image() {
        Ok(image) => (image.rgba().to_vec(), image.width(), image.height()),
        Err(e) => {
            // Also the normal result for a text paste: nothing to process.
            tracing::debug!("No image on the clipboard: {}", e);
            return Ok(None);
        }
    };
    if width == 0 || height == 0 {
        return Ok(None);
    }
    tokio::task::spawn_blocking(move || rgba_to_png_data_uri(rgba, width, height))
        .await
        .map_err(|e| format!("Clipboard image task failed: {e}"))?
        .map(Some)
}

#[tauri::command]
pub async fn process_image_from_base64(
    image_data: String,
    _state: State<'_, RagState>,
) -> Result<ImageProcessResult, String> {
    let image_id = Uuid::new_v4().to_string();

    // Strip data URI prefix
    let raw_b64 = if let Some(idx) = image_data.find(',') {
        &image_data[idx + 1..]
    } else {
        &image_data
    };

    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, raw_b64)
        .map_err(|e| format!("Invalid base64: {}", e))?;

    let (extracted_text, confidence) = match run_ocr(&bytes).await {
        Ok((text, conf)) => (text, conf),
        Err(e) => {
            tracing::warn!("OCR failed, returning empty text: {}", e);
            (String::new(), 0.0)
        }
    };

    let word_count = extracted_text.split_whitespace().count();

    Ok(ImageProcessResult {
        id: image_id,
        extracted_text,
        file_path: String::new(),
        confidence,
        word_count,
        image_data: if image_data.starts_with("data:image/") {
            image_data
        } else {
            format!("data:image/png;base64,{}", image_data)
        },
    })
}

/// Process an image from file path (drag-drop)
#[tauri::command]
pub async fn process_image_from_file(
    file_path: String,
    _state: State<'_, RagState>,
) -> Result<ImageProcessResult, String> {
    let image_id = Uuid::new_v4().to_string();

    let bytes = std::fs::read(&file_path).map_err(|e| format!("Failed to read file: {}", e))?;

    let (extracted_text, confidence) = match run_ocr(&bytes).await {
        Ok((text, conf)) => (text, conf),
        Err(e) => {
            tracing::warn!("OCR failed for {}: {}", file_path, e);
            (String::new(), 0.0)
        }
    };

    let word_count = extracted_text.split_whitespace().count();

    // Generate base64 data URI for display
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    let ext = std::path::Path::new(&file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    };

    Ok(ImageProcessResult {
        id: image_id,
        extracted_text,
        file_path,
        confidence,
        word_count,
        image_data: format!("data:{};base64,{}", mime, b64),
    })
}

/// Search indexed images by text query
#[tauri::command]
pub async fn search_images(
    query: String,
    limit: Option<usize>,
    state: State<'_, RagState>,
) -> Result<Vec<serde_json::Value>, String> {
    let rag = state.rag.read().await;

    let results = rag
        .search(&query, limit.unwrap_or(10))
        .await
        .map_err(|e| format!("Search failed: {}", e))?;

    let image_results: Vec<_> = results
        .into_iter()
        .filter(|r| r.metadata.values().any(|v| v.contains("image")))
        .map(|r| {
            serde_json::json!({
                "id": r.doc_id.to_string(),
                "text": r.text,
                "score": r.score,
                "source": r.source,
            })
        })
        .collect();

    Ok(image_results)
}

/// Export form as HTML file
#[tauri::command]
pub async fn export_form_html(
    title: String,
    description: Option<String>,
    fields: Vec<FormField>,
) -> Result<String, String> {
    export_form_as_html(&title, description.as_deref(), &fields)
        .map_err(|e| format!("Failed to export form as HTML: {}", e))
}

/// Export form as JSON Schema
#[tauri::command]
pub async fn export_form_json(
    title: String,
    description: Option<String>,
    fields: Vec<FormField>,
) -> Result<String, String> {
    export_form_as_json_schema(&title, description.as_deref(), &fields)
        .map_err(|e| format!("Failed to export form as JSON: {}", e))
}

#[cfg(test)]
mod tests {
    use super::rgba_to_png_data_uri;
    use base64::Engine as _;

    #[test]
    fn clipboard_pixels_become_a_png_data_uri() {
        let rgba = vec![255u8, 0, 0, 255, 0, 255, 0, 255];
        let uri = rgba_to_png_data_uri(rgba, 2, 1).unwrap();
        let b64 = uri.strip_prefix("data:image/png;base64,").unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.get_pixel(1, 0).0, [0, 255, 0, 255]);
    }

    #[test]
    fn mismatched_dimensions_are_an_error() {
        assert!(rgba_to_png_data_uri(vec![0; 4], 2, 2).is_err());
    }
}
