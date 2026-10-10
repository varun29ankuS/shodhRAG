//! Tauri commands for RAG operations

use crate::space_manager::SpaceManager;
use serde::{Deserialize, Serialize};
use shodh_rag::comprehensive_system::{Citation, ComprehensiveRAG};
use shodh_rag::types::MetadataFilter;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::State;

use crate::audit_commands::AuditState;
use serde_json::json;
use shodh_rag::audit::payload::{source_change, ChangeOrigin};
use shodh_rag::audit::{AuditEventType, AuditRecord};
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::RwLock as TokioRwLock;

// Windows-specific import to hide console windows
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Application paths for persistent storage
#[derive(Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
}

/// Application state
pub struct RagState {
    pub rag: Arc<TokioRwLock<ComprehensiveRAG>>,
    pub space_manager: Mutex<SpaceManager>,
    pub app_paths: AppPaths,
    pub rag_initialized: Arc<TokioRwLock<bool>>,
    pub initialization_lock: Arc<TokioMutex<()>>, // Mutex to prevent concurrent initialization
}

/// Search request from frontend
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub max_results: usize,
    pub space_id: Option<String>,
    pub filters: Option<HashMap<String, String>>,
}

/// Search result to frontend with enhanced citation tracking
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub snippet: String,
    pub citation: Citation,
    pub metadata: HashMap<String, String>,
    // Citation tracking enhancements
    pub source_file: String,
    pub page_number: Option<u32>,
    pub line_range: Option<(u32, u32)>,
    pub surrounding_context: String,
}

/// Decision metadata from query analysis
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionMetadata {
    pub intent: String,
    pub should_retrieve: bool,
    pub strategy: String,
    pub reasoning: String,
    pub confidence: f32,
}

/// Search response with results and decision metadata
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub decision: DecisionMetadata,
}

/// Mark the RAG system ready. The engine itself is created at startup
/// (lib.rs); this only records that the frontend finished initialising.
#[tauri::command]
pub async fn initialize_rag(state: State<'_, RagState>) -> Result<String, String> {
    let _init_lock = state.initialization_lock.lock().await;
    let mut initialized = state.rag_initialized.write().await;
    if !*initialized {
        *initialized = true;
        tracing::info!(db = ?state.app_paths.db_path, "RAG ready");
    }
    Ok(format!(
        "RAG initialized with persistent storage at {:?}",
        state.app_paths.db_path
    ))
}

/// Search documents
#[tauri::command]
pub async fn search_documents(
    request: SearchRequest,
    state: State<'_, RagState>,
) -> Result<SearchResponse, String> {
    tracing::info!(
        "Search documents called with query: '{}', max_results: {}",
        request.query,
        request.max_results
    );

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Build metadata filter from request filters
    let filter = if let Some(filters) = request.filters {
        let mut metadata_filter = MetadataFilter {
            space_id: None,
            source_type: None,
            source_path: None,
            date_from: None,
            date_to: None,
            custom: None,
            any_of: None,
        };

        let mut custom_fields: HashMap<String, String> = HashMap::new();

        // Process each filter key-value pair
        for (key, value) in filters {
            match key.as_str() {
                "space_id" => {
                    metadata_filter.space_id = Some(value);
                }
                "source_type" => {
                    metadata_filter.source_type = Some(value);
                }
                "source_path" => {
                    metadata_filter.source_path = Some(value);
                }
                "date_from" => {
                    metadata_filter.date_from = value.parse::<i64>().ok();
                }
                "date_to" => {
                    metadata_filter.date_to = value.parse::<i64>().ok();
                }
                _ => {
                    custom_fields.insert(key, value);
                }
            }
        }

        if !custom_fields.is_empty() {
            metadata_filter.custom = Some(custom_fields);
        }

        Some(metadata_filter)
    } else {
        None
    };

    // Perform comprehensive search
    tracing::info!("Performing local document search...");
    let results = rag
        .search_comprehensive(&request.query, request.max_results, filter)
        .await
        .map_err(|e| {
            tracing::info!("Search failed with error: {}", e);
            format!("Search failed: {}", e)
        })?;

    tracing::info!(
        "Search completed successfully, found {} results",
        results.len()
    );

    // Filter by space_id if provided (additional client-side filter)
    let filtered_results = if let Some(ref space) = request.space_id {
        tracing::info!("Filtering results by space_id: '{}'", space);
        let total = results.len();
        let filtered: Vec<_> = results
            .into_iter()
            .filter(|r| {
                r.metadata
                    .get("space_id")
                    .map(|s| s == space)
                    .unwrap_or(false)
            })
            .collect();
        tracing::info!("Filtered from {} to {} results", total, filtered.len());
        filtered
    } else {
        results
    };

    // Convert to frontend format with enhanced citation tracking
    let frontend_results: Vec<SearchResult> = filtered_results
        .into_iter()
        .map(|r| {
            // Debug logging
            tracing::info!("Converting result:");
            tracing::info!("  Citation title: '{}'", r.citation.title);
            tracing::info!("  Snippet length: {}", r.snippet.len());
            tracing::info!(
                "  Metadata keys: {:?}",
                r.metadata.keys().collect::<Vec<_>>()
            );

            // Extract source file from metadata
            let source_file = r
                .metadata
                .get("file_path")
                .or_else(|| r.metadata.get("source"))
                .cloned()
                .unwrap_or_else(|| r.citation.source.clone());

            // Extract page number
            let page_number = r
                .metadata
                .get("page_number")
                .or_else(|| r.metadata.get("page"))
                .and_then(|p| p.parse::<u32>().ok());

            // Extract line range
            let line_range = r
                .metadata
                .get("line_start")
                .and_then(|start| start.parse::<u32>().ok())
                .zip(
                    r.metadata
                        .get("line_end")
                        .and_then(|end| end.parse::<u32>().ok()),
                );

            // Get surrounding context (200 chars before/after)
            let full_text = r
                .metadata
                .get("full_text")
                .or_else(|| r.metadata.get("content"))
                .cloned()
                .unwrap_or_else(|| r.snippet.clone());

            let snippet_pos = full_text.find(&r.snippet).unwrap_or(0);
            let context_start = snippet_pos.saturating_sub(200);
            let context_end = (snippet_pos + r.snippet.len() + 200).min(full_text.len());
            let surrounding_context = full_text[context_start..context_end].to_string();

            SearchResult {
                id: r.id.to_string(),
                score: r.score,
                snippet: r.snippet.clone(),
                citation: r.citation.clone(),
                metadata: r.metadata.clone(),
                source_file,
                page_number,
                line_range,
                surrounding_context,
            }
        })
        .collect();

    tracing::info!("Returning {} results to frontend", frontend_results.len());
    if let Some(first) = frontend_results.first() {
        tracing::info!("First result citation title: '{}'", first.citation.title);
        tracing::info!(
            "First result snippet: '{}'",
            &first.snippet[..first.snippet.len().min(50)]
        );
    }

    // Create decision metadata
    let decision = DecisionMetadata {
        intent: "search".to_string(),
        should_retrieve: true,
        strategy: "comprehensive".to_string(),
        reasoning: "Local document search with optional metadata filtering".to_string(),
        confidence: if frontend_results.is_empty() {
            0.0
        } else {
            frontend_results.iter().map(|r| r.score).sum::<f32>() / frontend_results.len() as f32
        },
    };

    Ok(SearchResponse {
        results: frontend_results,
        decision,
    })
}

/// Get system statistics
#[tauri::command]
pub async fn get_statistics(state: State<'_, RagState>) -> Result<HashMap<String, String>, String> {
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    let stats = rag
        .get_statistics()
        .await
        .map_err(|e| format!("Failed to get statistics: {}", e))?;

    let mut result = HashMap::new();

    // Copy all stats from the HashMap
    let total_chunks = stats
        .get("total_chunks")
        .cloned()
        .unwrap_or_else(|| "0".to_string());
    let fts_indexed = stats
        .get("fts_indexed")
        .cloned()
        .unwrap_or_else(|| "0".to_string());
    let embedding_dimension = stats
        .get("embedding_dimension")
        .cloned()
        .unwrap_or_else(|| "0".to_string());
    let data_dir = stats
        .get("data_dir")
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    // Get actual document count (distinct doc_ids), not just chunk count
    let total_docs = rag.count_documents().await.unwrap_or(0);

    result.insert("total_chunks".to_string(), total_chunks.clone());
    result.insert("fts_indexed".to_string(), fts_indexed);
    result.insert("embedding_dimension".to_string(), embedding_dimension);
    result.insert("data_dir".to_string(), data_dir.clone());

    // Frontend aliases — total_documents = actual document count, chunks = chunk count
    result.insert("total_documents".to_string(), total_docs.to_string());
    result.insert("documents".to_string(), total_docs.to_string());
    result.insert("chunks".to_string(), total_chunks);

    // Calculate actual database size
    if !data_dir.is_empty() && std::path::Path::new(&data_dir).exists() {
        let lance_path = std::path::Path::new(&data_dir).join("lance_data");
        let tantivy_path = std::path::Path::new(&data_dir).join("tantivy_index");
        let mut total_bytes: u64 = 0;
        if lance_path.exists() {
            total_bytes += dir_size_recursive(&lance_path);
        }
        if tantivy_path.exists() {
            total_bytes += dir_size_recursive(&tantivy_path);
        }
        let size_mb = total_bytes as f64 / (1024.0 * 1024.0);
        result.insert("index_size_mb".to_string(), format!("{:.2}", size_mb));
    }

    // Copy any additional keys from the stats
    for (key, value) in &stats {
        if !result.contains_key(key) {
            result.insert(key.clone(), value.clone());
        }
    }

    Ok(result)
}

/// Delete all documents from a specific folder path
async fn delete_folder_source_inner(
    folder_path: String,
    state: State<'_, RagState>,
) -> Result<String, String> {
    tracing::info!("\n=== Deleting source folder: {} ===", folder_path);

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Use prefix-based deletion — folder path matches all files stored under it.
    // The RAG engine normalizes paths internally (lowercase, forward slashes on Windows)
    // so this will match regardless of how the original path was formatted.
    let deleted_count = rag
        .delete_by_source_prefix(&folder_path)
        .await
        .map_err(|e| format!("Failed to delete documents from folder: {}", e))?;

    // If nothing matched with the normalized path, also try the original raw path
    // to clean up documents indexed before the normalization fix.
    let deleted_count = if deleted_count == 0 {
        let raw = folder_path.clone();
        let fallback = rag
            .delete_by_source(&raw)
            .await
            .map_err(|e| format!("Failed to delete: {}", e))?;
        if fallback == 0 {
            // Also try with backslashes replaced but preserving case
            let with_fwd_slashes = raw.replace('\\', "/");
            rag.delete_by_source(&with_fwd_slashes).await.unwrap_or(0)
        } else {
            fallback
        }
    } else {
        deleted_count
    };

    tracing::info!("Deletion complete: {} documents deleted", deleted_count);

    if deleted_count == 0 {
        tracing::info!("No documents found for folder: {}", folder_path);
        return Ok(format!("No documents found for folder: {}", folder_path));
    }

    Ok(format!(
        "Successfully deleted {} documents from folder: {}",
        deleted_count, folder_path
    ))
}

// Notes persistence file path

/// File information structure
#[derive(Debug, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub file_path: String,
    pub file_type: String,
    pub status: String,
}

/// Get list of files for a specific source/space
#[tauri::command]
pub async fn get_source_files(
    source_id: String,
    state: State<'_, RagState>,
) -> Result<Vec<FileInfo>, String> {
    tracing::info!("\n=== Getting files for source: '{}' ===", source_id);

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Get statistics first
    if let Ok(stats) = rag.get_statistics().await {
        tracing::info!(
            "Database has total_chunks={}, fts_indexed={}",
            stats.get("total_chunks").unwrap_or(&"0".to_string()),
            stats.get("fts_indexed").unwrap_or(&"0".to_string())
        );
    }

    // List all chunks by metadata filter (not search — list)
    let all_results = rag.list_documents(None, 100000).await.unwrap_or_default();

    tracing::info!("📊 Search returned {} results", all_results.len());

    // Use HashMap to collect unique files by file_path
    let mut files_map: HashMap<String, FileInfo> = HashMap::new();
    let mut matched_count = 0;

    for (idx, result) in all_results.iter().enumerate() {
        // Debug: Print first few documents metadata
        if idx < 3 {
            tracing::info!(
                "📄 Sample doc {} metadata keys: {:?}",
                idx + 1,
                result.metadata.keys().collect::<Vec<_>>()
            );
            if let Some(space_id) = result.metadata.get("space_id") {
                tracing::info!("   space_id: '{}'", space_id);
            }
        }

        // Check if this document belongs to the requested source
        // Try multiple metadata field names
        let doc_source_id = result
            .metadata
            .get("space_id")
            .or_else(|| result.metadata.get("source_id"))
            .or_else(|| result.metadata.get("Space ID")); // Try capitalized version

        if let Some(doc_space_id) = doc_source_id {
            // Smart matching:
            // 1. Exact match (for new sources with unique IDs)
            // 2. Legacy match (for old sources indexed with 'default')
            let is_match = doc_space_id == &source_id
                || (doc_space_id == "default" && source_id.chars().all(|c| c.is_numeric()));

            if is_match {
                matched_count += 1;
                if let Some(file_path) = result.metadata.get("file_path") {
                    // Only add each unique file once
                    if !files_map.contains_key::<str>(file_path) {
                        // Extract file name from metadata or path
                        let file_name = result
                            .metadata
                            .get("file_name")
                            .or_else(|| result.metadata.get("title"))
                            .cloned()
                            .unwrap_or_else(|| {
                                file_path
                                    .rsplit(&['/', '\\'][..])
                                    .next()
                                    .unwrap_or("unknown")
                                    .to_string()
                            });

                        // Get file type/extension
                        let file_type = result
                            .metadata
                            .get("file_extension")
                            .or_else(|| result.metadata.get("file_type"))
                            .cloned()
                            .unwrap_or_else(|| {
                                // Extract extension from file path
                                file_path.rsplit('.').next().unwrap_or("txt").to_lowercase()
                            });

                        // All documents in the index are considered successfully indexed
                        let status = "indexed".to_string();

                        tracing::info!("✓ Found file: {} (type: {})", file_name, file_type);

                        files_map.insert(
                            file_path.clone(),
                            FileInfo {
                                name: file_name,
                                file_path: file_path.clone(),
                                file_type,
                                status,
                            },
                        );
                    }
                }
            }
        }
    }

    // Convert to vector and sort by name
    let mut files: Vec<FileInfo> = files_map.into_values().collect();
    files.sort_by_key(|a| a.name.to_lowercase());

    tracing::info!(
        "📊 Matched {} chunks, found {} unique files for source '{}'",
        matched_count,
        files.len(),
        source_id
    );
    Ok(files)
}

// Helper function to detect code files
pub(crate) fn is_code_file(path: &str) -> bool {
    let code_extensions = [
        "rs", "py", "js", "ts", "tsx", "jsx", "java", "cpp", "c", "h", "hpp", "go", "rb", "php",
        "cs", "swift", "kt", "scala", "r", "m", "mm", "vue", "svelte", "sol", "zig", "nim", "cr",
        "ex", "exs", "erl", "hrl", "clj", "lisp", "scm", "rkt", "hs", "ml", "fs", "fsx", "erl",
        "elm", "dart", "lua", "pl", "sh", "bash", "zsh", "fish", "yaml", "yml", "toml", "json",
        "xml", "sql",
    ];

    path.rsplit('.')
        .next()
        .map(|ext| code_extensions.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Jump to source - Open file and scroll to specific location with highlighting
#[tauri::command]
pub async fn jump_to_source(
    app_handle: tauri::AppHandle,
    file_path: String,
    line_number: Option<u32>,
    page_number: Option<u32>,
    search_text: Option<String>,
) -> Result<String, String> {
    tracing::info!(
        file = %file_path,
        line = ?line_number,
        page = ?page_number,
        search_len = search_text.as_ref().map(|s| s.len()),
        "jump_to_source invoked"
    );

    use std::process::Command;
    use tauri_plugin_opener::OpenerExt;

    // Check if file exists
    let path = std::path::Path::new(&file_path);
    if !path.exists() {
        tracing::warn!(path = %file_path, "jump_to_source: file not found on disk");
        return Err(format!("File not found: {}", file_path));
    }

    // Open file with system default application
    // For code files with line numbers, try to use VS Code or other code editors
    if line_number.is_some() && is_code_file(&file_path) {
        let line = line_number.unwrap_or(1);
        let line_arg = format!("{}:{}", file_path, line);

        // Try VS Code first (supports line numbers)
        #[cfg(target_os = "windows")]
        {
            let mut code_cmd = Command::new("code");
            code_cmd.arg("--goto").arg(&line_arg);
            code_cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            if code_cmd.spawn().is_ok() {
                return Ok(format!("Opened in VS Code: {}", line_arg));
            }
        }

        #[cfg(target_os = "macos")]
        {
            if Command::new("code")
                .arg("--goto")
                .arg(&line_arg)
                .spawn()
                .is_ok()
            {
                return Ok(format!("Opened in VS Code: {}", line_arg));
            }
        }

        #[cfg(target_os = "linux")]
        {
            if Command::new("code")
                .arg("--goto")
                .arg(&line_arg)
                .spawn()
                .is_ok()
            {
                return Ok(format!("Opened in VS Code: {}", line_arg));
            }
        }

        // Fallback: open with system default via Tauri opener plugin
        app_handle
            .opener()
            .open_path(&file_path, None::<&str>)
            .map_err(|e| format!("Failed to open file: {}", e))?;

        Ok(format!("Opened file: {} (line {})", file_path, line))
    } else {
        // For PDFs on Windows, try Adobe Reader with page/search parameters
        #[cfg(target_os = "windows")]
        if file_path.to_lowercase().ends_with(".pdf") {
            let page = page_number.unwrap_or(1);

            let adobe_params = if let Some(ref search) = search_text {
                let encoded = search
                    .chars()
                    .take(80)
                    .collect::<String>()
                    .replace(' ', "%20")
                    .replace('"', "%22");
                format!("/A \"page={}&search={}\"", page, encoded)
            } else {
                format!("/A \"page={}\"", page)
            };

            let adobe_paths = [
                "C:\\Program Files\\Adobe\\Acrobat DC\\Acrobat\\Acrobat.exe",
                "C:\\Program Files (x86)\\Adobe\\Acrobat Reader DC\\Reader\\AcroRd32.exe",
                "C:\\Program Files\\Adobe\\Acrobat Reader DC\\Reader\\AcroRd32.exe",
            ];

            for adobe_path in &adobe_paths {
                if std::path::Path::new(adobe_path).exists() {
                    let mut adobe_cmd = Command::new(adobe_path);
                    adobe_cmd.arg(&adobe_params).arg(&file_path);
                    adobe_cmd.creation_flags(0x08000000);
                    if adobe_cmd.spawn().is_ok() {
                        return Ok(format!("Opened PDF in Adobe Reader at page {}", page));
                    }
                }
            }
        }

        // Open with system default via Tauri opener plugin (reliable on all platforms)
        app_handle
            .opener()
            .open_path(&file_path, None::<&str>)
            .map_err(|e| format!("Failed to open file: {}", e))?;

        Ok(format!("Opened: {}", file_path))
    }
}

/// Recursively calculate directory size in bytes
fn dir_size_recursive(path: &std::path::Path) -> u64 {
    let mut size = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    size += dir_size_recursive(&entry.path());
                } else {
                    size += meta.len();
                }
            }
        }
    }
    size
}

/// `source_change` record with the command's outcome.
fn source_change_record(
    action: &str,
    source_id: Option<&str>,
    path: Option<&str>,
    result: &Result<String, String>,
) -> AuditRecord {
    let outcome = match result {
        Ok(message) => json!({"ok": true, "result": message}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    AuditRecord::new(
        AuditEventType::SourceChange,
        source_change(action, ChangeOrigin::Ui, source_id, path, outcome),
    )
}

/// Delete all documents from a folder (audited).
#[tauri::command]
pub async fn delete_folder_source(
    folder_path: String,
    state: State<'_, RagState>,
    audit: State<'_, AuditState>,
) -> Result<String, String> {
    let path = folder_path.clone();
    let result = delete_folder_source_inner(folder_path, state).await;
    audit.record(source_change_record("remove", None, Some(&path), &result));
    result
}
