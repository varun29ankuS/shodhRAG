//! Simple document upload commands with progress tracking
//! For drag-drop PDF/DOCX/XLSX into chat

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use tauri::State;
use crate::rag_commands::RagState;
use shodh_rag::comprehensive_system::Citation;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadProgress {
    pub stage: String,          // "parsing", "chunking", "embedding", "indexing", "complete"
    pub progress: f32,          // 0.0 to 1.0
    pub message: String,
    pub chunks_processed: usize,
    pub total_chunks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub success: bool,
    pub file_name: String,
    pub file_type: String,
    pub chunks_created: usize,
    pub file_size_mb: f64,
    pub processing_time_ms: u64,
    pub error: Option<String>,
}

/// Upload and index a document file (PDF, DOCX, XLSX, etc.)
#[tauri::command]
pub async fn upload_document_file(
    state: State<'_, RagState>,
    file_path: String,
    space_id: Option<String>,
) -> Result<UploadResult, String> {
    let start_time = std::time::Instant::now();

    let path = Path::new(&file_path);

    // Get file metadata
    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let extension = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let file_size = std::fs::metadata(path)
        .map(|m| m.len() as f64 / 1_048_576.0)  // Convert to MB
        .unwrap_or(0.0);

    // Determine file type
    let file_type = match extension.as_str() {
        "pdf" => "PDF",
        "docx" | "doc" => "Word Document",
        "xlsx" | "xls" => "Excel Spreadsheet",
        "pptx" | "ppt" => "PowerPoint",
        "txt" => "Text File",
        "md" => "Markdown",
        _ => "Document",
    };

    tracing::info!("📄 Uploading: {} ({:.2} MB)", file_name, file_size);

    // Prepare metadata
    let mut metadata = HashMap::new();
    metadata.insert("file_name".to_string(), file_name.clone());
    metadata.insert("file_type".to_string(), file_type.to_string());
    metadata.insert("file_size_mb".to_string(), format!("{:.2}", file_size));
    metadata.insert("uploaded_at".to_string(), chrono::Utc::now().to_rfc3339());

    if let Some(sid) = space_id.clone() {
        metadata.insert("space_id".to_string(), sid);
    }

    // Create citation
    let _citation = Citation {
        title: file_name.clone(),
        authors: vec![],
        source: file_path.clone(),
        year: chrono::Utc::now().format("%Y").to_string(),
        url: None,
        doi: None,
        page_numbers: None,
    };

    // Process the file
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    match rag.add_document_from_file(path, metadata).await {
        Ok(chunk_ids) => {
            let processing_time = start_time.elapsed().as_millis() as u64;

            tracing::info!("✅ Indexed {} chunks in {}ms", chunk_ids.len(), processing_time);

            // Update space activity if provided
            if let Some(sid) = space_id {
                if let Ok(space_manager) = state.space_manager.lock() {
                    // Add document to space (uses first chunk ID as document ID)
                    if let Some(first_chunk) = chunk_ids.first() {
                        let _ = space_manager.add_document_to_space(&sid, first_chunk.to_string());
                    }
                }
            }

            Ok(UploadResult {
                success: true,
                file_name,
                file_type: file_type.to_string(),
                chunks_created: chunk_ids.len(),
                file_size_mb: file_size,
                processing_time_ms: processing_time,
                error: None,
            })
        }
        Err(e) => {
            let error_msg = e.to_string();
            tracing::info!("❌ Upload failed: {}", error_msg);

            Ok(UploadResult {
                success: false,
                file_name,
                file_type: file_type.to_string(),
                chunks_created: 0,
                file_size_mb: file_size,
                processing_time_ms: start_time.elapsed().as_millis() as u64,
                error: Some(error_msg),
            })
        }
    }
}

/// Save dropped file bytes to a temp directory and return the file path
#[tauri::command]
pub fn save_temp_file(
    file_name: String,
    file_data: Vec<u8>,
) -> Result<String, String> {
    // Create temp directory for uploaded files
    let temp_dir = std::env::temp_dir().join("kalki_uploads");
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;

    // Create unique file path
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
    let file_path = temp_dir.join(format!("{}_{}", timestamp, file_name));

    // Write file data
    let mut file = fs::File::create(&file_path)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;

    file.write_all(&file_data)
        .map_err(|e| format!("Failed to write file data: {}", e))?;

    // Return absolute path as string
    file_path.to_str()
        .ok_or_else(|| "Failed to convert path to string".to_string())
        .map(|s| s.to_string())
}
