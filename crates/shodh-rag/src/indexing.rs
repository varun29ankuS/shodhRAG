//! Batch indexing pipeline for files and folders.
//!
//! Provides folder preview, single-file indexing, and batch indexing with
//! pause/resume/cancel support. Progress is emitted via the EventEmitter trait
//! so the caller (Tauri, HTTP server, CLI) can deliver updates to its UI.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use chrono::Utc;
use walkdir::WalkDir;
use futures::FutureExt;

use crate::rag_engine::RAGEngine;
use crate::chat::EventEmitter;

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderPreview {
    pub path: String,
    pub total_files: usize,
    pub files_by_type: HashMap<String, usize>,
    pub estimated_time: f64,
    pub files: Vec<FileInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub name: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub size: u64,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingProgress {
    pub current_file: String,
    pub processed_files: usize,
    pub total_files: usize,
    pub percentage: f32,
    pub current_action: String,
    pub eta_seconds: f32,
    pub speed: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingOptions {
    pub skip_indexed: bool,
    pub watch_changes: bool,
    pub process_subdirs: bool,
    pub priority: String,
    pub file_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingResult {
    pub files_processed: usize,
    pub total_chunks: usize,
    pub failed_files: Vec<String>,
    pub duration: u64,
}

/// Shared state for pause/cancel signalling across async boundaries.
#[derive(Debug)]
pub struct IndexingState {
    pub is_paused: Arc<Mutex<bool>>,
    pub should_cancel: Arc<Mutex<bool>>,
}

impl Default for IndexingState {
    fn default() -> Self {
        Self {
            is_paused: Arc::new(Mutex::new(false)),
            should_cancel: Arc::new(Mutex::new(false)),
        }
    }
}

impl IndexingState {
    pub fn pause(&self) {
        *self.is_paused.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    pub fn resume(&self) {
        *self.is_paused.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }

    pub fn cancel(&self) {
        *self.should_cancel.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    pub fn reset(&self) {
        *self.should_cancel.lock().unwrap_or_else(|e| e.into_inner()) = false;
        *self.is_paused.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }

    pub fn is_cancelled(&self) -> bool {
        *self.should_cancel.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_paused(&self) -> bool {
        *self.is_paused.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Preview a folder before indexing — returns file list + stats.
pub fn preview_folder(folder_path: &str) -> Result<FolderPreview, String> {
    let path = PathBuf::from(folder_path);

    if !path.exists() || !path.is_dir() {
        return Err("Invalid folder path".to_string());
    }

    let mut files = Vec::new();
    let mut files_by_type: HashMap<String, usize> = HashMap::new();

    for entry in WalkDir::new(&path)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("unknown")
            .to_lowercase();

        if is_supported_file_type(&extension) {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

            files.push(FileInfo {
                path: file_path.to_string_lossy().to_string(),
                name: file_path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
                file_type: extension.clone(),
                size,
                selected: true,
            });

            *files_by_type.entry(extension).or_insert(0) += 1;
        }

        if files.len() >= 1000 {
            break;
        }
    }

    let estimated_time = files.len() as f64 * 0.5;

    Ok(FolderPreview {
        path: folder_path.to_string(),
        total_files: files.len(),
        files_by_type,
        estimated_time,
        files: files.into_iter().take(100).collect(),
    })
}

/// Check if a path is a file or directory.
pub fn check_path_type(path: &str) -> Result<(bool, bool), String> {
    let path_buf = PathBuf::from(path);
    if !path_buf.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    Ok((path_buf.is_dir(), path_buf.is_file()))
}

/// Index a single file into a space.
///
/// Returns (chunks_created, duration_ms).
pub async fn index_single_file(
    file_path: &str,
    space_id: &str,
    rag: &mut RAGEngine,
    emitter: Option<&dyn EventEmitter>,
) -> Result<IndexingResult, String> {
    let start_time = Instant::now();
    let path = PathBuf::from(file_path);

    if !path.exists() {
        return Err(format!("File does not exist: {}", file_path));
    }
    if !path.is_file() {
        return Err(format!("Path is not a file: {}", file_path));
    }

    let extension = path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("unknown")
        .to_lowercase();

    if !is_supported_file_type(&extension) {
        return Err(format!("Unsupported file type: {}", extension));
    }

    emit_progress(emitter, file_path, 0, 1, 0.0, "Reading file...");
    emit_progress(emitter, file_path, 0, 1, 50.0, "Indexing...");

    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown");

    let mut metadata = HashMap::new();
    metadata.insert("space_id".to_string(), space_id.to_string());
    metadata.insert("file_path".to_string(), file_path.to_string());
    metadata.insert("file_name".to_string(), file_name.to_string());
    metadata.insert("file_type".to_string(), extension.clone());
    metadata.insert("file_extension".to_string(), extension.clone());
    metadata.insert("filename".to_string(), file_name.to_string());
    metadata.insert("doc_type".to_string(), "document".to_string());
    metadata.insert("indexed_at".to_string(), Utc::now().to_rfc3339());

    let ids = rag.add_document_from_file(&path, metadata)
        .await
        .map_err(|e| format!("Failed to index file: {}", e))?;

    let chunks_created = ids.len();

    emit_progress(emitter, file_path, 1, 1, 100.0, "Complete!");

    let duration = start_time.elapsed().as_millis() as u64;

    Ok(IndexingResult {
        files_processed: 1,
        total_chunks: chunks_created,
        failed_files: vec![],
        duration,
    })
}

/// Batch-index a folder into a space with pause/resume/cancel support.
pub async fn index_folder(
    folder_path: &str,
    space_id: &str,
    options: &IndexingOptions,
    rag: &mut RAGEngine,
    indexing_state: &IndexingState,
    emitter: Option<&dyn EventEmitter>,
) -> Result<IndexingResult, String> {
    let start_time = Instant::now();
    let path = PathBuf::from(folder_path);

    if !path.exists() {
        return Err(format!("Path does not exist: {}", folder_path));
    }
    if !path.is_dir() {
        return Err(format!("Path is not a directory: {}", folder_path));
    }

    indexing_state.reset();

    emit_progress(emitter, "Starting...", 0, 0, 0.0, "Initializing indexing");

    // Collect files to process
    let mut files_to_process = Vec::new();

    for entry in WalkDir::new(&path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("unknown")
            .to_lowercase();

        if options.file_types.contains(&extension) && is_supported_file_type(&extension) {
            files_to_process.push(file_path.to_path_buf());
        }

        if indexing_state.is_cancelled() {
            return Ok(IndexingResult {
                files_processed: 0,
                total_chunks: 0,
                failed_files: vec![],
                duration: start_time.elapsed().as_millis() as u64,
            });
        }
    }

    let total_files = files_to_process.len();

    if total_files == 0 {
        return Ok(IndexingResult {
            files_processed: 0,
            total_chunks: 0,
            failed_files: vec![],
            duration: start_time.elapsed().as_millis() as u64,
        });
    }

    let mut files_processed = 0;
    let mut total_chunks = 0;
    let mut failed_files = Vec::new();
    let mut last_progress_time = Instant::now();

    for (index, file_path) in files_to_process.iter().enumerate() {
        // Pause loop
        while indexing_state.is_paused() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if indexing_state.is_cancelled() {
                break;
            }
        }

        if indexing_state.is_cancelled() {
            break;
        }

        // Throttled progress update
        if last_progress_time.elapsed() > Duration::from_millis(100) {
            let elapsed = start_time.elapsed().as_secs_f32();
            let speed = if elapsed > 0.0 { files_processed as f32 / elapsed } else { 0.0 };
            let eta = if speed > 0.0 { (total_files - files_processed) as f32 / speed } else { 0.0 };

            let current_file = file_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            emit_progress(
                emitter,
                current_file,
                files_processed,
                total_files,
                (files_processed as f32 / total_files as f32) * 100.0,
                &format!("Processing file {} of {}", index + 1, total_files),
            );

            last_progress_time = Instant::now();
        }

        // Process file with panic protection
        let process_result = {
            let result = std::panic::AssertUnwindSafe(
                process_file_with_options(file_path, space_id, rag)
            );
            match result.catch_unwind().await {
                Ok(r) => r,
                Err(panic_info) => {
                    let msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                        s.clone()
                    } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                        s.to_string()
                    } else {
                        "Unknown panic during file processing".to_string()
                    };
                    Err(format!("Panic: {}", msg))
                }
            }
        };

        match process_result {
            Ok(chunks) => {
                files_processed += 1;
                total_chunks += chunks;
            }
            Err(_e) => {
                failed_files.push(file_path.to_string_lossy().to_string());
            }
        }
    }

    emit_progress(emitter, "Completed", files_processed, total_files, 100.0, "Indexing complete");

    Ok(IndexingResult {
        files_processed,
        total_chunks,
        failed_files,
        duration: start_time.elapsed().as_millis() as u64,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

pub fn is_supported_file_type(extension: &str) -> bool {
    matches!(
        extension,
        "txt" | "md" | "pdf" | "html" | "json" | "csv" | "docx" | "xlsx" | "pptx" | "rst" | "tex" |
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "java" | "cpp" | "c" | "h" |
        "hpp" | "cs" | "go" | "rb" | "php" | "swift" | "kt" | "scala" | "r" |
        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" |
        "css" | "scss" | "sass" | "less" | "vue" | "svelte" |
        "toml" | "yaml" | "yml" | "ini" | "conf" | "config" | "env" |
        "xml" | "sql" | "graphql" | "proto" |
        "png" | "jpg" | "jpeg" | "bmp" | "tiff" | "tif"
    )
}

async fn process_file_with_options(
    file_path: &Path,
    space_id: &str,
    rag: &mut RAGEngine,
) -> Result<usize, String> {
    if !file_path.exists() {
        return Err(format!("File does not exist: {}", file_path.display()));
    }

    let mut metadata = HashMap::new();
    metadata.insert("space_id".to_string(), space_id.to_string());
    metadata.insert("file_path".to_string(), file_path.to_string_lossy().to_string());
    metadata.insert("doc_type".to_string(), "document".to_string());
    metadata.insert("title".to_string(), file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string());

    if let Some(extension) = file_path.extension() {
        let ext = extension.to_string_lossy().to_string();
        metadata.insert("file_type".to_string(), ext.clone());
        metadata.insert("file_extension".to_string(), ext.clone());

        if let Some(filename) = file_path.file_name() {
            metadata.insert("filename".to_string(), filename.to_string_lossy().to_string());
        }
    }

    let ids = rag.add_document_from_file(file_path, metadata)
        .await
        .map_err(|e| format!("Failed to process file: {}", e))?;

    Ok(ids.len())
}

fn emit_progress(
    emitter: Option<&dyn EventEmitter>,
    current_file: &str,
    processed: usize,
    total: usize,
    percentage: f32,
    action: &str,
) {
    if let Some(e) = emitter {
        let progress = IndexingProgress {
            current_file: current_file.to_string(),
            processed_files: processed,
            total_files: total,
            percentage,
            current_action: action.to_string(),
            eta_seconds: 0.0,
            speed: 0.0,
        };
        e.emit("indexing-progress", serde_json::to_value(&progress).unwrap_or_default());
    }
}
