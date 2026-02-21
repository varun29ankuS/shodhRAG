//! Image upload, OCR (Windows OCR API), and form export commands

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;
use shodh_rag::rag::{FormField, export_form_as_html, export_form_as_json_schema};

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
        use windows::Storage::{StorageFile, FileAccessMode};
        use windows::Graphics::Imaging::BitmapDecoder;
        use windows::Media::Ocr::OcrEngine;
        use windows::core::HSTRING;

        let abs_path = std::fs::canonicalize(&path)
            .map_err(|e| format!("Path error: {}", e))?;
        let path_str = abs_path.to_string_lossy().to_string();
        // Strip \\?\ prefix that canonicalize adds on Windows
        let clean_path = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);

        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(clean_path))
            .map_err(|e| format!("GetFileFromPath failed: {}", e))?
            .get()
            .map_err(|e| format!("GetFileFromPath get failed: {}", e))?;

        let stream = file.OpenAsync(FileAccessMode::Read)
            .map_err(|e| format!("OpenAsync failed: {}", e))?
            .get()
            .map_err(|e| format!("OpenAsync get failed: {}", e))?;

        let decoder = BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| format!("BitmapDecoder failed: {}", e))?
            .get()
            .map_err(|e| format!("BitmapDecoder get failed: {}", e))?;

        let bitmap = decoder.GetSoftwareBitmapAsync()
            .map_err(|e| format!("GetSoftwareBitmap failed: {}", e))?
            .get()
            .map_err(|e| format!("GetSoftwareBitmap get failed: {}", e))?;

        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|e| format!("OCR engine creation failed: {}", e))?;

        let ocr_result = engine.RecognizeAsync(&bitmap)
            .map_err(|e| format!("RecognizeAsync failed: {}", e))?
            .get()
            .map_err(|e| format!("RecognizeAsync get failed: {}", e))?;

        let text = ocr_result.Text()
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

    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        raw_b64,
    ).map_err(|e| format!("Invalid base64: {}", e))?;

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

    let bytes = std::fs::read(&file_path)
        .map_err(|e| format!("Failed to read file: {}", e))?;

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

    let results = rag.search(&query, limit.unwrap_or(10))
        .await
        .map_err(|e| format!("Search failed: {}", e))?;

    let image_results: Vec<_> = results
        .into_iter()
        .filter(|r| r.metadata.values().any(|v| v.contains("image")))
        .map(|r| serde_json::json!({
            "id": r.doc_id.to_string(),
            "text": r.text,
            "score": r.score,
            "source": r.source,
        }))
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
