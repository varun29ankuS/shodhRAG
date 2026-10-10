//! Batch indexing pipeline for files and folders.
//!
//! Provides folder preview, single-file indexing, and batch indexing with
//! pause/resume/cancel support. Progress is emitted via the EventEmitter trait
//! so the caller (Tauri, HTTP server, CLI) can deliver updates to its UI.

use chrono::Utc;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use walkdir::WalkDir;

use crate::chat::EventEmitter;
use crate::embeddings::SearchModelsMissing;
use crate::rag_engine::RAGEngine;

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
    /// The source being indexed, so concurrent jobs can be told apart.
    pub space_id: String,
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

/// A file that could not be indexed, with the reason it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFailure {
    pub file: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingResult {
    pub files_processed: usize,
    pub total_chunks: usize,
    /// Paths of files that failed. Kept for compatibility; see `failures` for reasons.
    pub failed_files: Vec<String>,
    /// Per-file failure reasons, in the same order as `failed_files`.
    #[serde(default)]
    pub failures: Vec<FileFailure>,
    pub duration: u64,
}

impl IndexingResult {
    fn empty(duration: u64) -> Self {
        Self {
            files_processed: 0,
            total_chunks: 0,
            failed_files: Vec::new(),
            failures: Vec::new(),
            duration,
        }
    }
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
        .filter_entry(|e| !crate::folder_sync::is_hidden(e))
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
                name: file_path
                    .file_name()
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
    rag: &RwLock<RAGEngine>,
    emitter: Option<&dyn EventEmitter>,
) -> Result<IndexingResult, String> {
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let start_time = Instant::now();
    let path = crate::rag_engine::canonical_path(Path::new(file_path));

    if !path.exists() {
        return Err(format!("File does not exist: {}", file_path));
    }
    if !path.is_file() {
        return Err(format!("Path is not a file: {}", file_path));
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("unknown")
        .to_lowercase();

    if !is_supported_file_type(&extension) {
        return Err(format!("Unsupported file type: {}", extension));
    }

    emit_progress(emitter, space_id, file_path, 0, 1, 0.0, "Reading file...");
    emit_progress(emitter, space_id, file_path, 0, 1, 50.0, "Indexing...");

    let file_name = path
        .file_name()
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

    let ids = index_file(rag, &path, metadata)
        .await
        .map_err(|e| format!("Failed to index file: {e}"))?;

    let chunks_created = ids.len();

    emit_progress(emitter, space_id, file_path, 1, 1, 100.0, "Complete!");

    let duration = start_time.elapsed().as_millis() as u64;

    Ok(IndexingResult {
        files_processed: 1,
        total_chunks: chunks_created,
        failed_files: Vec::new(),
        failures: Vec::new(),
        duration,
    })
}

/// Batch-index a folder into a space with pause/resume/cancel support.
pub async fn index_folder(
    folder_path: &str,
    space_id: &str,
    options: &IndexingOptions,
    rag: &RwLock<RAGEngine>,
    indexing_state: &IndexingState,
    emitter: Option<&dyn EventEmitter>,
) -> Result<IndexingResult, String> {
    // Without the embedding model every file would be parsed and then fail.
    if !rag.read().await.has_search_models() {
        return Err(SearchModelsMissing.to_string());
    }
    let start_time = Instant::now();
    // One spelling for every path that leaves this function (progress,
    // failures, metadata), however the folder was typed.
    let path = crate::rag_engine::canonical_path(Path::new(folder_path));

    if !path.exists() {
        return Err(format!("Path does not exist: {}", folder_path));
    }
    if !path.is_dir() {
        return Err(format!("Path is not a directory: {}", folder_path));
    }

    indexing_state.reset();

    emit_progress(
        emitter,
        space_id,
        "Starting...",
        0,
        0,
        0.0,
        "Initializing indexing",
    );

    // Collect files to process
    let mut files_to_process = Vec::new();

    for entry in WalkDir::new(&path)
        .into_iter()
        .filter_entry(|e| !crate::folder_sync::is_hidden(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("unknown")
            .to_lowercase();

        if is_selected_file_type(options, &extension)
            && !crate::folder_sync::is_temporary_file(file_path)
        {
            files_to_process.push(file_path.to_path_buf());
        }

        if indexing_state.is_cancelled() {
            return Ok(IndexingResult::empty(
                start_time.elapsed().as_millis() as u64
            ));
        }
    }

    let total_files = files_to_process.len();

    if total_files == 0 {
        return Ok(IndexingResult::empty(
            start_time.elapsed().as_millis() as u64
        ));
    }

    let mut files_processed = 0;
    let mut total_chunks = 0;
    let mut failures: Vec<FileFailure> = Vec::new();
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
            let speed = if elapsed > 0.0 {
                files_processed as f32 / elapsed
            } else {
                0.0
            };
            let eta = if speed > 0.0 {
                (total_files - files_processed) as f32 / speed
            } else {
                0.0
            };

            let current_file = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            emit_progress(
                emitter,
                space_id,
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
            let result =
                std::panic::AssertUnwindSafe(process_file_with_options(file_path, space_id, rag));
            match result.catch_unwind().await {
                Ok(r) => r,
                Err(panic_info) => Err(format!("Panic: {}", panic_message(panic_info.as_ref()))),
            }
        };

        match process_result {
            Ok(chunks) => {
                files_processed += 1;
                total_chunks += chunks;
            }
            Err(reason) => {
                let file = file_path.to_string_lossy().to_string();
                tracing::warn!(file = %file, reason = %reason, "Failed to index file");
                failures.push(FileFailure { file, reason });
            }
        }
    }

    emit_progress(
        emitter,
        space_id,
        "Completed",
        files_processed,
        total_files,
        100.0,
        "Indexing complete",
    );

    if !failures.is_empty() {
        tracing::warn!(
            failed = failures.len(),
            processed = files_processed,
            total = total_files,
            "Folder indexing finished with failures"
        );
    }

    Ok(IndexingResult {
        files_processed,
        total_chunks,
        failed_files: failures.iter().map(|f| f.file.clone()).collect(),
        failures,
        duration: start_time.elapsed().as_millis() as u64,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Every file extension (lowercase, without the dot) the ingest pipeline can
/// index. Shared with the app so allow-lists are never duplicated.
pub const SUPPORTED_FILE_EXTENSIONS: &[&str] = &[
    // Documents
    "txt", "md", "pdf", "html", "json", "docx", "pptx", "rst", "tex", // Tabular
    "csv", "tsv", "xlsx", "xls", "xlsm", "xlsb", "ods", // Code
    "rs", "py", "js", "ts", "jsx", "tsx", "java", "cpp", "c", "h", "hpp", "cs", "go", "rb", "php",
    "swift", "kt", "scala", "r", "sh", "bash", "zsh", "ps1", "bat", "cmd", // Web
    "css", "scss", "sass", "less", "vue", "svelte", // Config / data
    "toml", "yaml", "yml", "ini", "conf", "config", "env", "xml", "sql", "graphql", "proto",
    // Images (OCR)
    "png", "jpg", "jpeg", "bmp", "tiff", "tif",
];

/// Whether `extension` (lowercase, without the dot) can be indexed.
pub fn is_supported_file_type(extension: &str) -> bool {
    SUPPORTED_FILE_EXTENSIONS.contains(&extension)
}

/// Whether a file with `extension` should be indexed under `options`.
/// `options.file_types` is an optional restriction: an empty list means
/// "every supported type"; a non-empty list narrows the supported set.
fn is_selected_file_type(options: &IndexingOptions, extension: &str) -> bool {
    is_supported_file_type(extension)
        && (options.file_types.is_empty()
            || options
                .file_types
                .iter()
                .any(|t| t.trim_start_matches('.').eq_ignore_ascii_case(extension)))
}

/// Index one file of a folder source (the metadata every folder file gets).
pub(crate) async fn process_file_with_options(
    file_path: &Path,
    space_id: &str,
    rag: &RwLock<RAGEngine>,
) -> Result<usize, String> {
    if !file_path.exists() {
        return Err(format!("File does not exist: {}", file_path.display()));
    }

    let mut metadata = HashMap::new();
    metadata.insert("space_id".to_string(), space_id.to_string());
    metadata.insert(
        "file_path".to_string(),
        file_path.to_string_lossy().to_string(),
    );
    metadata.insert("doc_type".to_string(), "document".to_string());
    metadata.insert(
        "title".to_string(),
        file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string(),
    );

    if let Some(extension) = file_path.extension() {
        let ext = extension.to_string_lossy().to_string();
        metadata.insert("file_type".to_string(), ext.clone());
        metadata.insert("file_extension".to_string(), ext.clone());

        if let Some(filename) = file_path.file_name() {
            metadata.insert(
                "filename".to_string(),
                filename.to_string_lossy().to_string(),
            );
        }
    }

    let ids = index_file(rag, file_path, metadata)
        .await
        .map_err(|e| format!("Failed to process file: {e}"))?;

    Ok(ids.len())
}

/// Indexes one file into the shared engine. The file is parsed, chunked and embedded on
/// a blocking thread without the engine lock; only storing it takes the write lock, so
/// searches run while files are indexed.
pub async fn index_file(
    rag: &RwLock<RAGEngine>,
    path: &Path,
    metadata: HashMap<String, String>,
) -> Result<Vec<uuid::Uuid>, String> {
    let preparer = rag
        .read()
        .await
        .file_preparer()
        .map_err(|e| format!("{e:#}"))?;
    let owned = path.to_path_buf();
    let prepared = tokio::task::spawn_blocking(move || preparer.prepare(&owned, metadata))
        .await
        .map_err(|e| match e.try_into_panic() {
            Ok(panic) => format!("Panic: {}", panic_message(panic.as_ref())),
            Err(e) => format!("Indexing task failed: {e}"),
        })?
        .map_err(|e| format!("{e:#}"))?;
    rag.write()
        .await
        .commit_file(prepared)
        .await
        .map_err(|e| format!("{e:#}"))
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "Unknown panic during file processing".to_string()
    }
}

fn emit_progress(
    emitter: Option<&dyn EventEmitter>,
    space_id: &str,
    current_file: &str,
    processed: usize,
    total: usize,
    percentage: f32,
    action: &str,
) {
    if let Some(e) = emitter {
        let progress = IndexingProgress {
            space_id: space_id.to_string(),
            current_file: current_file.to_string(),
            processed_files: processed,
            total_files: total,
            percentage,
            current_action: action.to_string(),
            eta_seconds: 0.0,
            speed: 0.0,
        };
        e.emit(
            "indexing-progress",
            serde_json::to_value(&progress).unwrap_or_default(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreadsheet_and_delimited_extensions_are_supported() {
        for ext in ["xlsx", "xls", "xlsm", "xlsb", "ods", "csv", "tsv"] {
            assert!(is_supported_file_type(ext), "{} should be supported", ext);
        }
        assert!(!is_supported_file_type("exe"));
    }

    fn options(file_types: &[&str]) -> IndexingOptions {
        IndexingOptions {
            skip_indexed: false,
            watch_changes: false,
            process_subdirs: true,
            priority: "normal".to_string(),
            file_types: file_types.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn file_types_is_an_optional_restriction() {
        let all = options(&[]);
        assert!(is_selected_file_type(&all, "ods"));
        assert!(is_selected_file_type(&all, "tsv"));
        assert!(!is_selected_file_type(&all, "exe"));

        let narrowed = options(&["PDF", ".csv", "exe"]);
        assert!(is_selected_file_type(&narrowed, "pdf"));
        assert!(is_selected_file_type(&narrowed, "csv"));
        assert!(!is_selected_file_type(&narrowed, "xlsx"));
        // Requesting an unsupported type never makes it indexable.
        assert!(!is_selected_file_type(&narrowed, "exe"));
    }

    #[test]
    fn indexing_result_failures_serialize_and_default_when_absent() {
        let result = IndexingResult {
            files_processed: 1,
            total_chunks: 4,
            failed_files: vec!["a.xlsx".to_string()],
            failures: vec![FileFailure {
                file: "a.xlsx".to_string(),
                reason: "Failed to process file: failed to open spreadsheet a.xlsx".to_string(),
            }],
            duration: 10,
        };
        let json = serde_json::to_value(&result).expect("serializable");
        assert_eq!(json["failures"][0]["file"], "a.xlsx");
        assert!(json["failures"][0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("failed to open spreadsheet")));

        let legacy: IndexingResult = serde_json::from_str(
            r#"{"files_processed":0,"total_chunks":0,"failed_files":[],"duration":0}"#,
        )
        .expect("legacy payload deserializes");
        assert!(legacy.failures.is_empty());
    }

    #[test]
    fn poisoned_state_lock_does_not_panic() {
        let state = IndexingState::default();
        let flag = Arc::clone(&state.should_cancel);
        let _ = std::thread::spawn(move || {
            let _guard = flag.lock().unwrap_or_else(|e| e.into_inner());
            panic!("poison the lock");
        })
        .join();
        state.cancel();
        assert!(state.is_cancelled());
    }

    /// Embeds like the word embedder, but a document batch containing `HOLD` waits
    /// until the test releases it: an index job stuck in embedding.
    struct GatedEmbedder {
        words: crate::statements::testing::WordEmbedder,
        entered: std::sync::Mutex<std::sync::mpsc::Sender<()>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl crate::embeddings::EmbeddingModel for GatedEmbedder {
        fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            self.words.embed_query(text)
        }
        fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            if text.contains("HOLD") {
                let _ = self
                    .entered
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(());
                let _ = self
                    .release
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .recv();
            }
            self.words.embed_document(text)
        }
        fn dimension(&self) -> usize {
            crate::statements::testing::DIM
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn searches_run_while_a_folder_is_indexed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        config.search.min_score_threshold = 0.0;
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut engine = RAGEngine::new(config).await.expect("engine");
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
                GatedEmbedder {
                    words: Default::default(),
                    entered: std::sync::Mutex::new(entered_tx),
                    release: std::sync::Mutex::new(release_rx),
                },
            )))
            .expect("models");
        engine
            .add_document(
                "The lease notice period is sixty days. Rent is reviewed every spring, and the landlord repairs the roof and the heating when they fail.",
                crate::types::DocumentFormat::TXT,
                HashMap::from([("title".to_string(), "lease".to_string())]),
                crate::types::Citation::default(),
            )
            .await
            .expect("indexed");
        let rag = Arc::new(RwLock::new(engine));

        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(
            folder.join("slow.txt"),
            "HOLD this file in embedding. It describes the parking rules of the building:              visitors park on the street, residents in the garage below the courtyard.",
        )
        .expect("file");
        let job = {
            let rag = rag.clone();
            let folder = folder.to_string_lossy().to_string();
            tokio::spawn(async move {
                index_folder(
                    &folder,
                    "space-1",
                    &options(&[]),
                    &rag,
                    &IndexingState::default(),
                    None,
                )
                .await
            })
        };
        // The job is embedding the file now.
        tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(60)))
            .await
            .expect("join")
            .expect("the index job reached embedding");

        let search = async { rag.read().await.search("lease notice period", 3).await };
        let found = tokio::time::timeout(Duration::from_secs(10), search)
            .await
            .expect("search waited for the index job")
            .expect("search");
        assert!(found.iter().any(|r| r.text.contains("sixty days")));

        release_tx.send(()).expect("release");
        let result = job.await.expect("join").expect("indexed");
        assert_eq!(result.files_processed, 1, "{:?}", result.failures);
        let after = rag
            .read()
            .await
            .search("HOLD file embedding", 3)
            .await
            .expect("search");
        assert!(after.iter().any(|r| r.text.contains("HOLD")));
    }
}
