//! Tauri commands for RAG operations

use shodh_rag::comprehensive_system::{
    ComprehensiveRAG, Citation, DocumentFormat
};
use shodh_rag::types::MetadataFilter;
use shodh_rag::agent::ConversationManager;
use shodh_rag::memory::MemorySystem;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;
use tauri::State;
use chrono::{self, Local};
use crate::space_manager::SpaceManager;
use tokio::sync::RwLock as TokioRwLock;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

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
    pub notes: Mutex<Vec<Note>>,
    pub space_manager: Mutex<SpaceManager>,
    pub conversation_manager: Arc<TokioRwLock<Option<ConversationManager>>>,
    pub memory_system: Arc<TokioRwLock<Option<Arc<TokioRwLock<MemorySystem>>>>>,
    pub personal_assistant: Arc<TokioRwLock<Option<Arc<TokioRwLock<shodh_rag::agent::PersonalAssistant>>>>>,
    pub app_paths: AppPaths,
    pub rag_initialized: Arc<TokioRwLock<bool>>,
    pub initialization_lock: Arc<TokioMutex<()>>, // Mutex to prevent concurrent initialization

    // Unified chat system
    pub artifact_store: Arc<TokioRwLock<crate::artifact_store::ArtifactStore>>,
    pub conversation_id: Arc<TokioRwLock<Option<String>>>,
    pub agent_system: Arc<TokioRwLock<Option<Arc<TokioRwLock<shodh_rag::agent::AgentSystem>>>>>, // Changed to match PersonalAssistant's type
    pub llm_manager: Arc<TokioRwLock<Option<shodh_rag::llm::LLMManager>>>,
}

/// Note structure for persistent memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
}

/// Note update structure
#[derive(Debug, Deserialize)]
pub struct NoteUpdate {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub pinned: Option<bool>,
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

/// Document upload request
#[derive(Debug, Deserialize)]
pub struct DocumentUpload {
    pub content: String,
    pub title: String,
    pub authors: Vec<String>,
    pub source: String,
    pub year: String,
    pub document_type: String,
    pub metadata: HashMap<String, String>,
}

/// Initialize the RAG system with persistent storage
#[tauri::command]
pub async fn initialize_rag(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("\n===== RAG INITIALIZATION CALLED =====");
    tracing::info!("Timestamp: {}", chrono::Utc::now().to_rfc3339());

    // Quick check before acquiring lock (optimization)
    {
        let initialized = *state.rag_initialized.read().await;
        if initialized {
            let assistant_guard = state.personal_assistant.read().await;
            let has_assistant = assistant_guard.is_some();
            drop(assistant_guard);

            if has_assistant {
                tracing::info!("✓ Already fully initialized - returning immediately");
                return Ok("RAG and PersonalAssistant already initialized".to_string());
            }
        }
    }

    tracing::info!("Attempting to acquire initialization lock...");
    tracing::info!("Lock address: {:p}", &*state.initialization_lock);

    // Acquire initialization lock to prevent concurrent calls (blocks until available)
    let _init_lock = state.initialization_lock.lock().await;

    tracing::info!("✓ Initialization lock acquired");
    tracing::info!("Using persistent database path: {:?}", state.app_paths.db_path);

    // Double-check after acquiring lock (another thread might have initialized while we waited)
    let initialized = *state.rag_initialized.read().await;
    if initialized {
        tracing::info!("RAG already initialized by another thread - checking PersonalAssistant status...");

        // Check if PersonalAssistant is initialized
        let assistant_guard = state.personal_assistant.read().await;
        let has_assistant = assistant_guard.is_some();
        drop(assistant_guard);

        if has_assistant {
            tracing::info!("✓ PersonalAssistant is initialized and ready");
            return Ok("RAG and PersonalAssistant already initialized".to_string());
        } else {
            tracing::info!("⚠ RAG initialized but PersonalAssistant is missing - will reinitialize PersonalAssistant only");
            // Skip RAG initialization, go straight to PersonalAssistant
            // Jump to PersonalAssistant initialization section
        }
    } else {
        // Mark RAG as initialized (it was already created at startup in lib.rs)
        tracing::info!("✓ RAG instance already exists from startup - marking as initialized");
        *state.rag_initialized.write().await = true;
    }

    // Now initialize PersonalAssistant with the RAG reference
    tracing::info!("\n===== PERSONAL ASSISTANT INITIALIZATION =====");
    tracing::info!("Checking Memory System availability...");

    // Wait up to 10 seconds for Memory System to initialize
    let mut retry_count = 0;
    let max_retries = 20; // 20 * 500ms = 10 seconds

    loop {
        tracing::info!("🔍 Retry attempt {}/{}", retry_count + 1, max_retries);
        let memory_system_guard = state.memory_system.read().await;

        if let Some(ref memory_arc) = *memory_system_guard {
            tracing::info!("✓ Memory System is available!");

            // Memory System is ready, clone the Arc before dropping the guard
            let memory_arc_clone = memory_arc.clone();
            drop(memory_system_guard); // Release lock before async operation

            tracing::info!("Creating PersonalAssistant from agent framework...");

            tracing::info!("Calling PersonalAssistant::new()...");
            match shodh_rag::agent::PersonalAssistant::new(memory_arc_clone).await {
                Ok(assistant) => {
                    tracing::info!("✓ PersonalAssistant created successfully");

                    // Load agents from YAML directory
                    tracing::info!("Loading agents from backend YAML directory...");
                    let agents_dir = std::env::current_dir()
                        .map(|p| p.join("agents"))
                        .unwrap_or_else(|_| std::path::PathBuf::from("agents"));

                    tracing::info!("  Agents directory: {:?}", agents_dir);

                    if agents_dir.exists() {
                        let agent_system = assistant.get_agent_system();
                        let system = agent_system.write().await;
                        match system.load_agents_from_directory(agents_dir.to_str().unwrap()).await {
                            Ok(loaded_ids) => {
                                tracing::info!("✓ Loaded {} agents: {:?}", loaded_ids.len(), loaded_ids);
                            },
                            Err(e) => {
                                tracing::info!("⚠ Failed to load agents from directory: {}", e);
                                tracing::info!("  Agents will need to be loaded manually");
                            }
                        }
                        drop(system);
                    } else {
                        tracing::info!("⚠ Agents directory not found at {:?}", agents_dir);
                        tracing::info!("  Agents will need to be loaded manually");
                    }

                    // Get the agent_system from PersonalAssistant and sync it to RagState
                    // Share the same Arc reference instead of cloning
                    let agent_system_arc = assistant.get_agent_system();

                    // Store the shared Arc reference in RagState
                    // This synchronizes the agents between PersonalAssistant and unified chat
                    // Both will point to the SAME AgentSystem instance (no duplication!)
                    *state.agent_system.write().await = Some(agent_system_arc.clone());
                    tracing::info!("✓ AgentSystem synchronized to RagState (shared reference)");

                    *state.personal_assistant.write().await = Some(Arc::new(TokioRwLock::new(assistant)));
                    tracing::info!("✓ PersonalAssistant stored in state");
                    tracing::info!("===== PERSONAL ASSISTANT READY =====\n");
                    break;
                },
                Err(e) => {
                    tracing::info!("✗ Failed to initialize PersonalAssistant: {}", e);
                    tracing::info!("   Error details: {:?}", e);
                    tracing::info!("   Agent system will be unavailable");
                    tracing::info!("===== PERSONAL ASSISTANT FAILED =====\n");
                    break;
                }
            }
        } else {
            tracing::info!("⏳ Memory system not ready yet...");
            drop(memory_system_guard); // Release lock before sleep

            retry_count += 1;
            if retry_count >= max_retries {
                tracing::info!("✗ Memory system not initialized after {} retries ({} seconds)", max_retries, max_retries / 2);
                tracing::info!("   PersonalAssistant will not be available");
                tracing::info!("   This usually means:");
                tracing::info!("   1. Memory System initialization is taking too long");
                tracing::info!("   2. Memory System initialization failed (check earlier logs)");
                tracing::info!("   3. There's a deadlock or async task issue");
                tracing::info!("   You can reinitialize it later by calling initialize_rag again");
                tracing::info!("===== PERSONAL ASSISTANT UNAVAILABLE =====\n");
                break;
            }

            tracing::info!("   Waiting 500ms before retry...");
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    tracing::info!("RAG initialization completed successfully with persistent storage");
    tracing::info!("Database location: {:?}", state.app_paths.db_path);
    Ok(format!("RAG initialized successfully with persistent storage at {:?}", state.app_paths.db_path))
}

/// Check initialization status - Diagnostic command
#[tauri::command]
pub async fn check_initialization_status(state: State<'_, RagState>) -> Result<serde_json::Value, String> {
    use serde_json::json;

    tracing::info!("\n===== INITIALIZATION STATUS CHECK =====");

    // Check RAG initialization
    let rag_initialized = *state.rag_initialized.read().await;
    tracing::info!("RAG Initialized: {}", rag_initialized);

    // Check Memory System
    let memory_guard = state.memory_system.read().await;
    let memory_initialized = memory_guard.is_some();
    drop(memory_guard);
    tracing::info!("Memory System Initialized: {}", memory_initialized);

    // Check PersonalAssistant
    let assistant_guard = state.personal_assistant.read().await;
    let assistant_initialized = assistant_guard.is_some();
    drop(assistant_guard);
    tracing::info!("PersonalAssistant Initialized: {}", assistant_initialized);

    // Check ConversationManager
    let conversation_guard = state.conversation_manager.read().await;
    let conversation_initialized = conversation_guard.is_some();
    drop(conversation_guard);
    tracing::info!("ConversationManager Initialized: {}", conversation_initialized);

    tracing::info!("=====================================\n");

    Ok(json!({
        "rag_initialized": rag_initialized,
        "memory_system_initialized": memory_initialized,
        "personal_assistant_initialized": assistant_initialized,
        "conversation_manager_initialized": conversation_initialized,
        "all_systems_ready": rag_initialized && memory_initialized && assistant_initialized,
        "message": if rag_initialized && memory_initialized && assistant_initialized {
            "All systems initialized and ready"
        } else if !rag_initialized {
            "RAG not initialized - call initialize_rag command"
        } else if !memory_initialized {
            "Memory System not initialized - this is a background task that should complete automatically"
        } else {
            "PersonalAssistant not initialized - this means initialize_rag was called but PersonalAssistant initialization failed. Check console logs."
        }
    }))
}

/// Search documents
#[tauri::command]
pub async fn search_documents(
    request: SearchRequest,
    state: State<'_, RagState>,
) -> Result<SearchResponse, String> {
    tracing::info!("Search documents called with query: '{}', max_results: {}", request.query, request.max_results);

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
        };

        let mut custom_fields: HashMap<String, String> = HashMap::new();

        // Process each filter key-value pair
        for (key, value) in filters {
            match key.as_str() {
                "space_id" => {
                    metadata_filter.space_id = Some(value);
                },
                "source_type" => {
                    metadata_filter.source_type = Some(value);
                },
                "source_path" => {
                    metadata_filter.source_path = Some(value);
                },
                "date_from" => {
                    metadata_filter.date_from = value.parse::<i64>().ok();
                },
                "date_to" => {
                    metadata_filter.date_to = value.parse::<i64>().ok();
                },
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
    let results = rag.search_comprehensive(&request.query, request.max_results, filter)
        .await
        .map_err(|e| {
            tracing::info!("Search failed with error: {}", e);
            format!("Search failed: {}", e)
        })?;

    tracing::info!("Search completed successfully, found {} results", results.len());

    // Filter by space_id if provided (additional client-side filter)
    let filtered_results = if let Some(ref space) = request.space_id {
        tracing::info!("Filtering results by space_id: '{}'", space);
        let total = results.len();
        let filtered: Vec<_> = results.into_iter()
            .filter(|r| {
                r.metadata.get("space_id")
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
            tracing::info!("  Metadata keys: {:?}", r.metadata.keys().collect::<Vec<_>>());

            // Extract source file from metadata
            let source_file = r.metadata.get("file_path")
                .or_else(|| r.metadata.get("source"))
                .cloned()
                .unwrap_or_else(|| r.citation.source.clone());

            // Extract page number
            let page_number = r.metadata.get("page_number")
                .or_else(|| r.metadata.get("page"))
                .and_then(|p| p.parse::<u32>().ok());

            // Extract line range
            let line_range = r.metadata.get("line_start")
                .and_then(|start| start.parse::<u32>().ok())
                .and_then(|start| {
                    r.metadata.get("line_end")
                        .and_then(|end| end.parse::<u32>().ok())
                        .map(|end| (start, end))
                });

            // Get surrounding context (200 chars before/after)
            let full_text = r.metadata.get("full_text")
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
        tracing::info!("First result snippet: '{}'", &first.snippet[..first.snippet.len().min(50)]);
    }

    // Create decision metadata
    let decision = DecisionMetadata {
        intent: "search".to_string(),
        should_retrieve: true,
        strategy: "comprehensive".to_string(),
        reasoning: "Local document search with optional metadata filtering".to_string(),
        confidence: if frontend_results.is_empty() { 0.0 } else {
            frontend_results.iter().map(|r| r.score).sum::<f32>() / frontend_results.len() as f32
        },
    };

    Ok(SearchResponse {
        results: frontend_results,
        decision,
    })
}

/// Add a document
#[tauri::command]
pub async fn add_document(
    document: DocumentUpload,
    state: State<'_, RagState>,
) -> Result<String, String> {
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;
    
    // Create citation
    let citation = Citation {
        title: document.title,
        authors: document.authors,
        source: document.source,
        year: document.year,
        url: None,
        doi: None,
        page_numbers: None,
    };
    
    // Determine document format
    let format = match document.document_type.as_str() {
        "pdf" => DocumentFormat::PDF,
        "html" => DocumentFormat::HTML,
        "markdown" | "md" => DocumentFormat::MD,
        "json" => DocumentFormat::JSON,
        _ => DocumentFormat::TXT,
    };
    
    // Add document
    let ids = rag
        .add_document(&document.content, format, document.metadata, citation)
        .await
        .map_err(|e| format!("Failed to add document: {}", e))?;
    
    Ok(format!("Document added successfully with {} chunks", ids.len()))
}

/// Upload a file
#[tauri::command]
pub async fn upload_file(
    file_path: String,
    metadata: HashMap<String, String>,
    state: State<'_, RagState>,
) -> Result<String, String> {
    tracing::info!("Upload file called with path: {}", file_path);

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    let path = PathBuf::from(&file_path);

    // Check if file exists
    if !path.exists() {
        return Err(format!("File does not exist: {}", file_path));
    }

    // Check if it's a file (not directory)
    if !path.is_file() {
        return Err(format!("Path is not a file: {}", file_path));
    }

    tracing::info!("File validated: {}", file_path);

    // Ensure file_path is in metadata
    let mut enhanced_metadata = metadata.clone();
    enhanced_metadata.insert("file_path".to_string(), file_path.clone());
    enhanced_metadata.insert("original_path".to_string(), path.to_string_lossy().to_string());
    enhanced_metadata.insert("doc_type".to_string(), "document".to_string());

    // Debug: Log the space_id to make sure it's being passed
    if let Some(space_id) = enhanced_metadata.get("space_id") {
        tracing::info!("Document will be added to space: '{}'", space_id);
    } else {
        tracing::info!("Warning: No space_id in metadata, document won't be associated with any space!");
    }

    // Add document from file
    tracing::info!("Attempting to add document from file...");
    tracing::info!("Metadata being passed: {:?}", enhanced_metadata);
    let ids = match rag.add_document_from_file(path.as_path(), enhanced_metadata).await {
        Ok(ids) => {
            tracing::info!("Successfully processed file with {} chunks", ids.len());
            ids
        },
        Err(e) => {
            tracing::info!("Error adding document: {}", e);
            return Err(format!("Failed to process file: {}", e));
        }
    };
    
    // Update space document count if space_id is provided
    if let Some(space_id) = metadata.get("space_id") {
        if let Ok(space_manager) = state.space_manager.lock() {
            // Generate a document ID (using the first chunk ID as document ID)
            if let Some(first_id) = ids.first() {
                let doc_id = first_id.to_string();
                if let Err(e) = space_manager.add_document_to_space(space_id, doc_id) {
                    tracing::info!("Warning: Failed to update space document count: {}", e);
                } else {
                    tracing::info!("Successfully added document to space {}", space_id);
                }
            }
        }
    } else {
        tracing::info!("No space_id in metadata, document not linked to any space");
    }

    tracing::info!("Successfully processed file with {} chunks", ids.len());
    Ok(format!("File uploaded successfully with {} chunks", ids.len()))
}

/// Get system statistics
#[tauri::command]
pub async fn get_statistics(state: State<'_, RagState>) -> Result<HashMap<String, String>, String> {
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    let stats = rag.get_statistics()
        .await
        .map_err(|e| format!("Failed to get statistics: {}", e))?;

    let mut result = HashMap::new();

    // Copy all stats from the HashMap
    let total_chunks = stats.get("total_chunks").cloned().unwrap_or_else(|| "0".to_string());
    let fts_indexed = stats.get("fts_indexed").cloned().unwrap_or_else(|| "0".to_string());
    let embedding_dimension = stats.get("embedding_dimension").cloned().unwrap_or_else(|| "0".to_string());
    let data_dir = stats.get("data_dir").cloned().unwrap_or_else(|| "unknown".to_string());

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

/// Clear all data
#[tauri::command]
pub async fn clear_all_data(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("\n\n=== CRITICAL: clear_all_data() was called ===");
    tracing::info!("Stack trace would be helpful here to find caller");
    tracing::info!("This should ONLY be called when user explicitly deletes a space!");
    tracing::info!("================================================\n");
    
    // Log a backtrace if possible
    if let Ok(bt) = std::env::var("RUST_BACKTRACE") {
        if bt == "1" || bt == "full" {
            tracing::info!("Backtrace: {:?}", std::backtrace::Backtrace::force_capture());
        }
    }

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Clear data using the existing RAG instance
    rag.clear_all_data()
        .await
        .map_err(|e| format!("Failed to clear data: {}", e))?;
    tracing::info!("Successfully cleared all indexed data");

    // Clear notes as well
    if let Ok(mut notes) = state.notes.lock() {
        notes.clear();
        tracing::info!("Cleared all notes");
    }

    // Clear all spaces from space manager
    if let Ok(space_manager) = state.space_manager.lock() {
        space_manager.clear_all_spaces()
            .map_err(|e| format!("Failed to clear spaces: {}", e))?;
        tracing::info!("Cleared all spaces from space manager");
    }

    Ok("All data cleared successfully. Please re-index your documents.".to_string())
}

/// Delete all documents from a specific folder path
#[tauri::command]
pub async fn delete_folder_source(
    folder_path: String,
    state: State<'_, RagState>
) -> Result<String, String> {
    tracing::info!("\n=== Deleting source folder: {} ===", folder_path);

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Use prefix-based deletion — folder path matches all files stored under it.
    // The RAG engine normalizes paths internally (lowercase, forward slashes on Windows)
    // so this will match regardless of how the original path was formatted.
    let deleted_count = rag.delete_by_source_prefix(&folder_path)
        .await
        .map_err(|e| format!("Failed to delete documents from folder: {}", e))?;

    // If nothing matched with the normalized path, also try the original raw path
    // to clean up documents indexed before the normalization fix.
    let deleted_count = if deleted_count == 0 {
        let raw = folder_path.clone();
        let fallback = rag.delete_by_source(&raw)
            .await
            .map_err(|e| format!("Failed to delete: {}", e))?;
        if fallback == 0 {
            // Also try with backslashes replaced but preserving case
            let with_fwd_slashes = raw.replace('\\', "/");
            rag.delete_by_source(&with_fwd_slashes)
                .await
                .unwrap_or(0)
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

    Ok(format!("Successfully deleted {} documents from folder: {}", deleted_count, folder_path))
}

// Notes persistence file path
fn get_notes_file_path() -> Result<PathBuf, String> {
    // Use local kalki-v2 data directory
    let data_dir = std::path::PathBuf::from("./data");
    if !data_dir.exists() {
        fs::create_dir_all(&data_dir).map_err(|e| format!("Failed to create data directory: {}", e))?;
    }
    Ok(data_dir.join("notes.json"))
}

/// Load notes from persistent storage
fn load_notes_from_disk() -> Result<Vec<Note>, String> {
    let notes_file = get_notes_file_path()?;
    if notes_file.exists() {
        let content = fs::read_to_string(notes_file)
            .map_err(|e| format!("Failed to read notes file: {}", e))?;
        let notes: Vec<Note> = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse notes file: {}", e))?;
        Ok(notes)
    } else {
        Ok(Vec::new())
    }
}

/// Save notes to persistent storage
fn save_notes_to_disk(notes: &[Note]) -> Result<(), String> {
    let notes_file = get_notes_file_path()?;
    let content = serde_json::to_string_pretty(notes)
        .map_err(|e| format!("Failed to serialize notes: {}", e))?;
    fs::write(notes_file, content)
        .map_err(|e| format!("Failed to write notes file: {}", e))?;
    Ok(())
}

/// Get all notes
#[tauri::command]
pub async fn get_notes(state: State<'_, RagState>) -> Result<Vec<Note>, String> {
    // Try to load from memory first
    let notes_guard = state.notes.lock().map_err(|e| e.to_string())?;
    if !notes_guard.is_empty() {
        return Ok(notes_guard.clone());
    }
    drop(notes_guard);
    
    // Load from disk if memory is empty
    let notes = load_notes_from_disk()?;
    let mut notes_guard = state.notes.lock().map_err(|e| e.to_string())?;
    *notes_guard = notes.clone();
    Ok(notes)
}

/// Save a new note
#[tauri::command]
pub async fn save_note(note: Note, state: State<'_, RagState>) -> Result<String, String> {
    let mut notes_guard = state.notes.lock().map_err(|e| e.to_string())?;
    notes_guard.push(note.clone());
    save_notes_to_disk(&notes_guard)?;
    Ok(format!("Note '{}' saved successfully", note.title))
}

/// Update an existing note
#[tauri::command]
pub async fn update_note(
    note_id: String,
    updates: NoteUpdate,
    state: State<'_, RagState>,
) -> Result<String, String> {
    let mut notes_guard = state.notes.lock().map_err(|e| e.to_string())?;
    
    if let Some(note) = notes_guard.iter_mut().find(|n| n.id == note_id) {
        let mut title_for_response = note.title.clone();
        
        if let Some(title) = updates.title {
            note.title = title.clone();
            title_for_response = title;
        }
        if let Some(content) = updates.content {
            note.content = content;
        }
        if let Some(tags) = updates.tags {
            note.tags = tags;
        }
        if let Some(pinned) = updates.pinned {
            note.pinned = pinned;
        }
        note.updated_at = chrono::Utc::now().to_rfc3339();
        
        // Save to disk with the updated notes
        save_notes_to_disk(&*notes_guard)?;
        Ok(format!("Note '{}' updated successfully", title_for_response))
    } else {
        Err("Note not found".to_string())
    }
}

/// Delete a note
#[tauri::command]
pub async fn delete_note(note_id: String, state: State<'_, RagState>) -> Result<String, String> {
    let mut notes_guard = state.notes.lock().map_err(|e| e.to_string())?;
    
    if let Some(pos) = notes_guard.iter().position(|n| n.id == note_id) {
        let note = notes_guard.remove(pos);
        save_notes_to_disk(&notes_guard)?;
        Ok(format!("Note '{}' deleted successfully", note.title))
    } else {
        Err("Note not found".to_string())
    }
}

/// Add note content to RAG system for searchability
#[tauri::command]
pub async fn add_note_to_rag(
    note_id: String,
    content: String,
    tags: Vec<String>,
    state: State<'_, RagState>,
) -> Result<String, String> {
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Create metadata for the note
        let mut metadata = HashMap::new();
        metadata.insert("type".to_string(), "note".to_string());
        metadata.insert("note_id".to_string(), note_id.clone());
        metadata.insert("tags".to_string(), tags.join(", "));
        metadata.insert("source".to_string(), "user_notes".to_string());
        
        // Create citation for the note
        let citation = Citation {
            title: format!("User Note: {}", note_id),
            authors: vec!["User".to_string()],
            source: "Personal Notes".to_string(),
            year: chrono::Utc::now().format("%Y").to_string(),
            url: None,
            doi: None,
            page_numbers: None,
        };
        
        // Add to RAG system
        let _ids = rag.add_document(
            &content,
            DocumentFormat::TXT,
            metadata,
            citation,
        ).await.map_err(|e| format!("Failed to add note to RAG: {}", e))?;

    Ok("Note added to RAG system for searchability".to_string())
}

/// Remove note from RAG system
#[tauri::command]
pub async fn remove_note_from_rag(
    note_id: String,
    _state: State<'_, RagState>,
) -> Result<String, String> {
    // Note: For full implementation, you'd need to track document IDs per note
    // and remove them from the storage. For now, we'll just acknowledge the request.
    Ok(format!("Note {} removal from RAG acknowledged", note_id))
}

/// Get list of documents in a space
#[tauri::command]
pub async fn list_space_documents(
    space_id: String,
    folder_path: Option<String>,
    state: State<'_, RagState>,
) -> Result<Vec<String>, String> {
    tracing::info!("Listing documents for space: {}", space_id);

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;
    
    // Get list of indexed documents
    let mut file_list = Vec::new();
    
    // If folder_path is provided, list files from that folder
    if let Some(path) = folder_path {
        let path = std::path::Path::new(&path);
        if path.exists() && path.is_dir() {
            // Read all files from the directory
            match std::fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if let Ok(metadata) = entry.metadata() {
                            if metadata.is_file() {
                                if let Some(name) = entry.file_name().to_str() {
                                    // Check if this is a supported file type
                                    if is_supported_file(name) {
                                        file_list.push(name.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::info!("Error reading directory: {}", e);
                }
            }
        }
    }

    // If no files found from folder, try to get from RAG statistics
    if file_list.is_empty() {
        // Get document count from stats as a fallback
        if let Ok(stats) = rag.get_statistics().await {
            let doc_count: usize = stats.get("total_chunks")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            // Return generic list if we have documents but can't get names
            if doc_count > 0 {
                tracing::info!("Found {} documents in RAG system but couldn't retrieve names", doc_count);
            }
        }
    }

    Ok(file_list)
}

/// Helper function to check if a file is supported
fn is_supported_file(filename: &str) -> bool {
    let supported_extensions = vec![
        "txt", "md", "pdf", "docx", "doc", "rtf",
        "py", "js", "rs", "java", "cpp", "c", "h",
        "json", "xml", "yaml", "yml", "toml",
        "png", "jpg", "jpeg", "gif", "bmp", "svg", "webp", "tiff", "tif"
    ];
    
    if let Some(ext) = filename.split('.').last() {
        supported_extensions.contains(&ext.to_lowercase().as_str())
    } else {
        false
    }
}

/// Link a folder and index all files within it
#[tauri::command]
pub async fn link_folder(
    folder_path: String,
    metadata: HashMap<String, String>,
    state: State<'_, RagState>,
) -> Result<String, String> {
    tracing::info!("\n=== LINK_FOLDER START ===");
    tracing::info!("Folder path: {}", folder_path);
    tracing::info!("Metadata: {:?}", metadata);

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    tracing::info!("RAG system is initialized");
    
    let folder_path = PathBuf::from(&folder_path);
    
    // Check if folder exists
    if !folder_path.exists() {
        return Err(format!("Folder does not exist: {}", folder_path.display()));
    }

    // Check if it's a directory
    if !folder_path.is_dir() {
        return Err(format!("Path is not a directory: {}", folder_path.display()));
    }
    
    tracing::info!("Folder validated: {}", folder_path.display());
    
    // Get all files in the folder recursively
    let mut files_processed = 0;
    let mut total_chunks = 0;
    
    fn collect_files(dir: &PathBuf) -> Result<Vec<PathBuf>, std::io::Error> {
        let mut files = Vec::new();
        let entries = fs::read_dir(dir)?;
        
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_dir() {
                // Recursively collect files from subdirectories
                let mut subfiles = collect_files(&path)?;
                files.append(&mut subfiles);
            } else if path.is_file() {
                // Check if the file has a supported extension
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    // Include code files and documentation
                    if matches!(ext_str.as_str(), 
                        // Documents
                        "txt" | "md" | "pdf" | "html" | "json" | "csv" | "docx" | "rst" | "tex" |
                        // Code files
                        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "java" | "cpp" | "c" | "h" | 
                        "hpp" | "cs" | "go" | "rb" | "php" | "swift" | "kt" | "scala" | "r" |
                        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" |
                        // Web files
                        "css" | "scss" | "sass" | "less" | "vue" | "svelte" |
                        // Config files
                        "toml" | "yaml" | "yml" | "ini" | "conf" | "config" | "env" |
                        // Data files
                        "xml" | "sql" | "graphql" | "proto"
                    ) {
                        files.push(path);
                    }
                }
            }
        }

        Ok(files)
    }
    
    let files = collect_files(&folder_path)
        .map_err(|e| format!("Failed to scan folder: {}", e))?;
    
    tracing::info!("Found {} files to process", files.len());

    let total_files = files.len();

    // Process each file with detailed logging
    for (idx, file_path) in files.iter().enumerate() {
        let progress = ((idx + 1) as f32 / total_files as f32 * 100.0) as u32;
        tracing::info!("Processing [{}/{}] ({}%): {}", idx + 1, total_files, progress, file_path.display());

        match process_single_file(&file_path, &metadata, rag).await {
            Ok(chunk_count) => {
                files_processed += 1;
                total_chunks += chunk_count;
                tracing::info!("✓ Processed: {} ({} chunks)", file_path.display(), chunk_count);
            },
            Err(e) => {
                tracing::info!("✗ Skipped: {} - Error: {}", file_path.display(), e);
                // Continue with other files even if one fails
            }
        }

        // Flush output every 10 files
        if idx % 10 == 0 {
            tracing::info!("Progress: {}/{} files processed ({} chunks total)", files_processed, total_files, total_chunks);
        }
    }

    tracing::info!("=== INDEXING COMPLETE ===");
    tracing::info!("Total files found: {}", total_files);
    tracing::info!("Files successfully processed: {}", files_processed);
    tracing::info!("Files skipped: {}", total_files - files_processed);
    tracing::info!("Total chunks created: {}", total_chunks);

    // Update space document count if space_id is provided
    if let Some(space_id) = metadata.get("space_id") {
        tracing::info!("Updating space {} with {} documents", space_id, files_processed);
        if let Ok(space_manager) = state.space_manager.lock() {
            // Note: space_manager tracks individual documents, but we're adding in bulk
            // The count will be updated when documents are queried
            tracing::info!("✓ Documents associated with space: {}", space_id);
        }
    } else {
        tracing::info!("⚠️  No space_id in metadata - documents won't be associated with any space");
    }

    Ok(format!(
        "Indexed {}/{} files with {} chunks ({}% success rate)",
        files_processed, total_files, total_chunks,
        (files_processed as f32 / total_files as f32 * 100.0) as u32
    ))
}

/// Helper function to process a single file
async fn process_single_file(
    file_path: &PathBuf,
    base_metadata: &HashMap<String, String>,
    rag: &mut ComprehensiveRAG,
) -> Result<usize, String> {
    tracing::info!("  → Processing file: {}", file_path.display());

    // Check file size first
    if let Ok(metadata) = fs::metadata(file_path) {
        let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
        tracing::info!("     File size: {:.2} MB", size_mb);

        if size_mb > 10.0 {
            return Err(format!("File too large: {:.2} MB (max 10 MB)", size_mb));
        }
    } else {
        return Err("Failed to read file metadata".to_string());
    }

    // Add file-specific metadata
    let mut file_metadata = base_metadata.clone();
    file_metadata.insert("file_path".to_string(), file_path.to_string_lossy().to_string());
    file_metadata.insert("file_name".to_string(), file_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string());
    
    if let Some(ext) = file_path.extension() {
        let ext_str = ext.to_string_lossy().to_string();
        file_metadata.insert("file_extension".to_string(), ext_str.clone());

        // Add language/type classification for better code search
        let file_type = match ext_str.as_str() {
            "rs" => "rust_code",
            "py" => "python_code",
            "js" | "jsx" => "javascript_code",
            "ts" | "tsx" => "typescript_code",
            "java" => "java_code",
            "cpp" | "cc" | "cxx" | "c" | "h" | "hpp" => "cpp_code",
            "cs" => "csharp_code",
            "go" => "go_code",
            "rb" => "ruby_code",
            "php" => "php_code",
            "swift" => "swift_code",
            "kt" => "kotlin_code",
            "md" | "markdown" => "documentation",
            "json" | "yaml" | "yml" | "toml" => "configuration",
            _ => "code",
        };
        file_metadata.insert("file_type".to_string(), file_type.to_string());
        file_metadata.insert("content_category".to_string(), "codebase".to_string());
    }

    tracing::info!("     Metadata prepared: {} keys", file_metadata.len());

    // Add document from file
    tracing::info!("     Calling add_document_from_file...");
    let ids = rag
        .add_document_from_file(file_path, file_metadata)
        .await
        .map_err(|e| {
            let err_msg = format!("Parsing failed: {}", e);
            tracing::info!("     ✗ ERROR: {}", err_msg);

            // Try to identify the issue
            if e.to_string().contains("UTF-8") || e.to_string().contains("encoding") {
                tracing::info!("     ℹ️  Likely cause: Binary file or non-UTF8 encoding");
            } else if e.to_string().contains("syntax") || e.to_string().contains("parse") {
                tracing::info!("     ℹ️  Likely cause: Syntax error or malformed file");
            } else if e.to_string().contains("size") || e.to_string().contains("large") {
                tracing::info!("     ℹ️  Likely cause: File too large");
            }

            err_msg
        })?;

    tracing::info!("     ✓ Successfully created {} chunks", ids.len());
    Ok(ids.len())
}

/// Get folder statistics
#[tauri::command]
pub async fn get_folder_stats(
    folder_path: String,
) -> Result<HashMap<String, String>, String> {
    tracing::info!("Getting folder stats for: {}", folder_path);
    
    let folder_path = PathBuf::from(&folder_path);
    
    if !folder_path.exists() {
        tracing::info!("Folder does not exist: {}", folder_path.display());
        return Err(format!("Folder does not exist: {}", folder_path.display()));
    }

    if !folder_path.is_dir() {
        tracing::info!("Path is not a directory: {}", folder_path.display());
        return Err(format!("Path is not a directory: {}", folder_path.display()));
    }
    
    fn count_files(dir: &PathBuf) -> Result<(usize, usize), std::io::Error> {
        let mut file_count = 0;
        let mut supported_count = 0;
        let entries = fs::read_dir(dir)?;
        
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_dir() {
                let (sub_files, sub_supported) = count_files(&path)?;
                file_count += sub_files;
                supported_count += sub_supported;
            } else if path.is_file() {
                file_count += 1;
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if matches!(ext_str.as_str(), "txt" | "md" | "pdf" | "html" | "json" | "csv" | "docx") {
                        supported_count += 1;
                    }
                }
            }
        }

        Ok((file_count, supported_count))
    }
    
    let (total_files, supported_files) = count_files(&folder_path)
        .map_err(|e| format!("Failed to scan folder: {}", e))?;
    
    let mut stats = HashMap::new();
    stats.insert("total_files".to_string(), total_files.to_string());
    stats.insert("supported_files".to_string(), supported_files.to_string());
    stats.insert("folder_path".to_string(), folder_path.to_string_lossy().to_string());

    Ok(stats)
}

#[tauri::command]
pub async fn get_daily_brief(
    state: tauri::State<'_, RagState>,
) -> Result<serde_json::Value, String> {
    use serde_json::json;
    use std::collections::HashMap;

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;
    let now = Local::now();

    // Pull real statistics from the RAG engine
    let stats: HashMap<String, String> = rag
        .get_statistics()
        .await
        .unwrap_or_default();

    let total_chunks: usize = stats
        .get("total_chunks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let fts_indexed: usize = stats
        .get("fts_indexed")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let doc_count: usize = rag.count_documents().await.unwrap_or(0);

    // Get real document info for type breakdown and recently added
    let doc_info = rag.get_document_info().await.unwrap_or_default();

    let mut type_counts: HashMap<String, usize> = HashMap::new();
    let mut recent_docs: Vec<serde_json::Value> = Vec::new();

    for (_doc_id, title, source) in &doc_info {
        let ext = std::path::Path::new(source)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("other")
            .to_lowercase();
        let doc_type = match ext.as_str() {
            "pdf" => "PDF",
            "md" | "markdown" => "Markdown",
            "txt" => "Text",
            "docx" | "doc" => "Word",
            "xlsx" | "xls" | "csv" => "Spreadsheet",
            "pptx" | "ppt" => "Presentation",
            "html" | "htm" => "HTML",
            "json" | "yaml" | "toml" => "Data",
            "rs" | "py" | "js" | "ts" | "go" | "java" | "c" | "cpp" => "Code",
            _ => "Other",
        };
        *type_counts.entry(doc_type.to_string()).or_insert(0) += 1;

        if recent_docs.len() < 5 {
            let file_name = std::path::Path::new(source)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(title.as_str());
            recent_docs.push(json!({
                "name": file_name,
                "source": source,
            }));
        }
    }

    let daily_brief = json!({
        "date": now.format("%A, %B %d, %Y").to_string(),
        "documentsIndexed": doc_count,
        "totalChunks": total_chunks,
        "ftsIndexed": fts_indexed,
        "documentStats": {
            "byType": type_counts,
            "recentlyAdded": recent_docs,
        },
        "suggestions": if doc_count == 0 {
            vec![
                "Get started by indexing a folder of documents",
                "Drag and drop files into the chat to index them",
            ]
        } else {
            vec![
                "Try asking a question about your indexed documents",
                "Use specific terms for more precise results",
            ]
        }
    });

    Ok(daily_brief)
}

#[tauri::command]
pub async fn get_knowledge_map(
    state: tauri::State<'_, RagState>,
) -> Result<serde_json::Value, String> {
    use serde_json::json;
    use std::collections::HashMap;

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Build knowledge graph from real indexed documents
    let doc_info = rag.get_document_info().await.unwrap_or_default();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut type_groups: HashMap<String, Vec<String>> = HashMap::new();

    for (idx, (doc_id, title, source)) in doc_info.iter().enumerate().take(30) {
        let file_name = std::path::Path::new(source)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(title.as_str())
            .to_string();

        let ext = std::path::Path::new(source)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("other")
            .to_lowercase();

        let (color, doc_type) = match ext.as_str() {
            "pdf" => ("#ef4444", "PDF"),
            "md" | "markdown" => ("#8b5cf6", "Markdown"),
            "txt" => ("#6b7280", "Text"),
            "docx" | "doc" => ("#3b82f6", "Word"),
            "xlsx" | "xls" | "csv" => ("#10b981", "Spreadsheet"),
            "pptx" | "ppt" => ("#f97316", "Presentation"),
            "html" | "htm" => ("#06b6d4", "HTML"),
            _ => ("#fbbf24", "Other"),
        };

        let node_id = format!("doc-{}", idx);
        nodes.push(json!({
            "id": node_id,
            "label": file_name,
            "type": "document",
            "size": 20,
            "color": color,
            "connections": 0,
            "docId": doc_id,
        }));

        type_groups
            .entry(doc_type.to_string())
            .or_default()
            .push(node_id);
    }

    // Create edges between documents of the same type
    for (_doc_type, node_ids) in &type_groups {
        for i in 0..node_ids.len() {
            for j in (i + 1)..node_ids.len().min(i + 3) {
                edges.push(json!({
                    "source": node_ids[i],
                    "target": node_ids[j],
                    "weight": 0.6,
                    "type": "same_type",
                }));
            }
        }
    }

    // Build clusters from type groups
    let clusters: Vec<serde_json::Value> = type_groups
        .iter()
        .enumerate()
        .map(|(idx, (name, ids))| {
            json!({
                "id": format!("c{}", idx),
                "name": name,
                "nodeIds": ids,
            })
        })
        .collect();

    let knowledge_map = json!({
        "nodes": nodes,
        "edges": edges,
        "clusters": clusters,
    });

    Ok(knowledge_map)
}

/// Open original document file with system default application
#[tauri::command]
pub async fn open_original_document(file_path: String) -> Result<String, String> {
    use std::process::Command;
    
    tracing::info!("Opening original document: {}", file_path);
    
    // Check if file exists
    let path = std::path::Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    // Open file with system default application
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(&["/C", "start", "", &file_path]);
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        cmd.spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&file_path)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(&file_path)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    Ok(format!("Opened: {}", file_path))
}

/// Open file at specific location (line number or page) with appropriate editor
#[tauri::command]
pub async fn open_file_at_location(
    file_path: String,
    line_number: Option<u32>,
    page_number: Option<u32>,
) -> Result<String, String> {
    use std::process::Command;

    tracing::info!("Opening file at location: {} (line: {:?}, page: {:?})", file_path, line_number, page_number);

    // Check if file exists
    let path = std::path::Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    // Try to open with VS Code if available (supports line numbers)
    let vscode_result = try_open_with_vscode(&file_path, line_number);
    if vscode_result.is_ok() {
        return Ok(format!("Opened in VS Code: {} at line {}", file_path, line_number.unwrap_or(1)));
    }

    // Fallback to system default (doesn't support line numbers)
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(&["/C", "start", "", &file_path]);

        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        cmd.spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&file_path)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(&file_path)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    Ok(format!("Opened: {} (line numbers not supported with default app)", file_path))
}

/// Helper function to try opening file with VS Code at specific line
fn try_open_with_vscode(file_path: &str, line_number: Option<u32>) -> Result<(), String> {
    use std::process::Command;

    // Try "code" command (VS Code CLI)
    let mut cmd = Command::new("code");

    if let Some(line) = line_number {
        // VS Code syntax: code --goto file:line:column
        cmd.arg("--goto");
        cmd.arg(format!("{}:{}", file_path, line));
    } else {
        cmd.arg(file_path);
    }

    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

    cmd.spawn()
        .map_err(|e| format!("VS Code not available: {}", e))?;

    Ok(())
}

/// Read original file content for display in app
/// Uses parsed content from index for PDFs/DOCX, falls back to raw file for code
#[tauri::command]
pub async fn read_original_file(
    file_path: String,
    state: State<'_, RagState>,
) -> Result<String, String> {
    use std::fs;

    tracing::info!("Reading file (using parsed content if available): {}", file_path);

    // First, try to get parsed content from RAG index
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // List documents with this file_path using a metadata filter (not search)
    let filter = MetadataFilter {
        space_id: None,
        source_type: None,
        source_path: Some(file_path.clone()),
        date_from: None,
        date_to: None,
        custom: None,
    };

    let results = rag.list_documents(Some(filter), 10000)
        .await
        .map_err(|e| format!("Failed to query index: {}", e))?;

    if !results.is_empty() {
        tracing::info!("Found {} chunks in index, reconstructing document", results.len());

        // Reconstruct full document from chunks (sorted by chunk_index)
        let mut chunks: Vec<_> = results.into_iter().collect();
        chunks.sort_by_key(|r| {
            r.metadata.get("chunk_index")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0)
        });

        let full_text: String = chunks.iter()
            .map(|r| r.snippet.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        if !full_text.is_empty() {
            tracing::info!("Returning parsed content from index ({} chars)", full_text.len());
            return Ok(full_text);
        }
    }

    tracing::info!("No indexed content found, reading raw file from disk");

    // Fallback: read raw file from disk
    let path = std::path::Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    // Try to read as UTF-8 first
    match fs::read_to_string(&file_path) {
        Ok(content) => Ok(content),
        Err(e) => {
            // If UTF-8 fails, try reading as bytes and convert with lossy UTF-8
            tracing::info!("Failed to read as UTF-8, trying lossy conversion: {}", e);
            match fs::read(&file_path) {
                Ok(bytes) => {
                    let content = String::from_utf8_lossy(&bytes).to_string();
                    Ok(content)
                }
                Err(read_err) => {
                    Err(format!("Failed to read file: {}", read_err))
                }
            }
        }
    }
}

/// Get document metadata including file path
#[tauri::command]
pub async fn get_document_metadata(
    document_title: String,
    state: State<'_, RagState>,
) -> Result<HashMap<String, String>, String> {
    tracing::info!("Getting metadata for document: {}", document_title);

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;
    
    // Search for the document to get its metadata
    let search_results = rag.search(&document_title, 1)
        .await
        .map_err(|e| format!("Failed to search for document: {}", e))?;
    
    if let Some(result) = search_results.first() {
        // Return the actual metadata
        Ok(result.metadata.clone())
    } else {
        // Document not found - return empty metadata
        let mut metadata = HashMap::new();
        metadata.insert("error".to_string(), "Document not found in index".to_string());
        Ok(metadata)
    }
}

/// Add test documents to a space for debugging
#[tauri::command]
pub async fn add_test_documents(
    state: State<'_, RagState>,
    space_id: String,
) -> Result<String, String> {
    tracing::info!("\n=== Adding test documents to space '{}' ===", space_id);

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Create test documents
    let test_docs = vec![
        ("Test Document 1", "This is a test document to verify the system is working correctly."),
        ("Test Document 2", "Another test document with different content for testing."),
        ("Test Document 3", "Third test document to ensure multiple documents work."),
    ];

    let mut total_chunks = 0;

    for (title, content) in test_docs {
        let mut metadata = HashMap::new();
        metadata.insert("space_id".to_string(), space_id.clone());
        metadata.insert("title".to_string(), title.to_string());
        metadata.insert("doc_type".to_string(), "document".to_string());
        metadata.insert("created_at".to_string(), chrono::Utc::now().to_rfc3339());
        metadata.insert("test_document".to_string(), "true".to_string());

        tracing::info!("Adding test document: '{}' with metadata: {:?}", title, metadata);

        let citation = Citation {
            title: title.to_string(),
            authors: vec!["Test Author".to_string()],
            source: "Test Source".to_string(),
            year: "2024".to_string(),
            url: None,
            doi: None,
            page_numbers: None,
        };

        let ids = rag.add_document(
            content,
            DocumentFormat::TXT,
            metadata,
            citation,
        ).await.map_err(|e| format!("Failed to add test document: {}", e))?;

        total_chunks += ids.len();
        tracing::info!("✓ Added test document '{}' with {} chunks", title, ids.len());
    }

    Ok(format!("Added 3 test documents with {} total chunks to space '{}'", total_chunks, space_id))
}

/// Get all documents (for debugging and space listing)
#[tauri::command]
pub async fn get_all_documents(
    state: State<'_, RagState>,
    max_results: usize,
) -> Result<Vec<HashMap<String, String>>, String> {
    tracing::info!("\n=== Getting all documents (max: {}) ===", max_results);

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Try to get statistics first
    if let Ok(stats) = rag.get_statistics().await {
        tracing::info!("Database statistics: total_chunks={}, fts_indexed={}",
                 stats.get("total_chunks").unwrap_or(&"0".to_string()),
                 stats.get("fts_indexed").unwrap_or(&"0".to_string()));
    }

    // List all chunks by metadata (not search)
    let comprehensive_results = rag.list_documents(None, max_results)
        .await
        .unwrap_or_default();

    tracing::info!("Found {} results", comprehensive_results.len());

    // Convert to simple format
    Ok(comprehensive_results.into_iter().map(|r| {
        let mut doc = HashMap::new();
        doc.insert("id".to_string(), r.id.to_string());
        doc.insert("score".to_string(), r.score.to_string());
        doc.insert("text".to_string(), r.snippet.clone());
        doc.insert("title".to_string(), r.citation.title.clone());
        // Copy all metadata
        for (key, value) in r.metadata {
            doc.insert(format!("metadata_{}", key), value);
        }
        doc
    }).collect())
}

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
        tracing::info!("Database has total_chunks={}, fts_indexed={}",
                 stats.get("total_chunks").unwrap_or(&"0".to_string()),
                 stats.get("fts_indexed").unwrap_or(&"0".to_string()));
    }

    // List all chunks by metadata filter (not search — list)
    let all_results = rag.list_documents(None, 100000)
        .await
        .unwrap_or_default();

    tracing::info!("📊 Search returned {} results", all_results.len());

    // Use HashMap to collect unique files by file_path
    let mut files_map: HashMap<String, FileInfo> = HashMap::new();
    let mut matched_count = 0;

    for (idx, result) in all_results.iter().enumerate() {
        // Debug: Print first few documents metadata
        if idx < 3 {
            tracing::info!("📄 Sample doc {} metadata keys: {:?}", idx + 1, result.metadata.keys().collect::<Vec<_>>());
            if let Some(space_id) = result.metadata.get("space_id") {
                tracing::info!("   space_id: '{}'", space_id);
            }
        }

        // Check if this document belongs to the requested source
        // Try multiple metadata field names
        let doc_source_id = result.metadata.get("space_id")
            .or_else(|| result.metadata.get("source_id"))
            .or_else(|| result.metadata.get("Space ID")); // Try capitalized version

        if let Some(doc_space_id) = doc_source_id {
            // Smart matching:
            // 1. Exact match (for new sources with unique IDs)
            // 2. Legacy match (for old sources indexed with 'default')
            let is_match = doc_space_id == &source_id ||
                           (doc_space_id == "default" && source_id.chars().all(|c| c.is_numeric()));

            if is_match {
                matched_count += 1;
                if let Some(file_path) = result.metadata.get("file_path") {
                    // Only add each unique file once
                    if !files_map.contains_key::<str>(file_path) {
                        // Extract file name from metadata or path
                        let file_name = result.metadata.get("file_name")
                            .or_else(|| result.metadata.get("title"))
                            .cloned()
                            .unwrap_or_else(|| {
                                file_path.split(&['/', '\\'][..])
                                    .last()
                                    .unwrap_or("unknown")
                                    .to_string()
                            });

                        // Get file type/extension
                        let file_type = result.metadata.get("file_extension")
                            .or_else(|| result.metadata.get("file_type"))
                            .cloned()
                            .unwrap_or_else(|| {
                                // Extract extension from file path
                                file_path.split('.')
                                    .last()
                                    .unwrap_or("txt")
                                    .to_lowercase()
                            });

                        // All documents in the index are considered successfully indexed
                        let status = "indexed".to_string();

                        tracing::info!("✓ Found file: {} (type: {})", file_name, file_type);

                        files_map.insert(file_path.clone(), FileInfo {
                            name: file_name,
                            file_path: file_path.clone(),
                            file_type,
                            status,
                        });
                    }
                }
            }
        }
    }

    // Convert to vector and sort by name
    let mut files: Vec<FileInfo> = files_map.into_values().collect();
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    tracing::info!("📊 Matched {} chunks, found {} unique files for source '{}'", matched_count, files.len(), source_id);
    Ok(files)
}

/// Get full text content of an indexed document
#[tauri::command]
pub async fn get_document_full_text(
    document_title: String,
    state: State<'_, RagState>,
) -> Result<String, String> {
    tracing::info!("Getting full text for document: {}", document_title);

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;
    
    // Search for all chunks of this document
    let search_results = rag.search(&document_title, 50) // Get many results to find all chunks
        .await
        .map_err(|e| format!("Failed to search for document: {}", e))?;
    
    // Filter results to only include chunks from this document
    let mut document_chunks: Vec<_> = search_results
        .into_iter()
        .filter(|r| {
            // Check if metadata contains the document title
            r.metadata.get("document_title")
                .map(|title| title == &document_title)
                .unwrap_or(false) ||
            r.metadata.get("title")
                .map(|title| title == &document_title)
                .unwrap_or(false)
        })
        .collect();
    
    // Sort by chunk index if available
    document_chunks.sort_by_key(|r| {
        r.metadata.get("chunk_index")
            .and_then(|idx| idx.parse::<usize>().ok())
            .unwrap_or(0)
    });
    
    // Check if this is a PDF with page images
    let has_page_images = document_chunks
        .first()
        .and_then(|chunk| chunk.metadata.get("has_visual_content"))
        .map(|v| v == "true")
        .unwrap_or(false);
    
    if has_page_images {
        // Return JSON with page images for visual display
        let mut result = serde_json::json!({
            "type": "multimodal",
            "pages": []
        });
        
        // Collect page images from metadata
        let mut pages = Vec::new();
        let mut current_page = 1;
        
        while let Some(page_image) = document_chunks
            .first()
            .and_then(|chunk| chunk.metadata.get(&format!("page_image_{}", current_page))) {
            
            pages.push(serde_json::json!({
                "page_number": current_page,
                "image": page_image,
                "text": document_chunks
                    .iter()
                    .filter(|chunk| {
                        chunk.metadata.get("page_number")
                            .and_then(|p| p.parse::<usize>().ok())
                            .map(|p| p == current_page)
                            .unwrap_or(false)
                    })
                    .map(|chunk| {
                        chunk.metadata.get("full_text")
                            .or_else(|| chunk.metadata.get("content"))
                            .or_else(|| chunk.metadata.get("text"))
                            .map(|s| s.as_str())
                            .unwrap_or(&chunk.text)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }));
            
            current_page += 1;
        }

        result["pages"] = serde_json::json!(pages);
        
        // Also include visual elements if present
        if let Some(visual_elements) = document_chunks
            .first()
            .and_then(|chunk| chunk.metadata.get("visual_elements")) {
            result["visual_elements"] = serde_json::from_str(visual_elements).unwrap_or(serde_json::json!([]));
        }

        Ok(result.to_string())
    } else {
        // Return plain text for non-PDF documents
        let full_text = if !document_chunks.is_empty() {
            document_chunks
                .iter()
                .map(|chunk| {
                    // Try to get full text from metadata, otherwise use snippet
                    chunk.metadata.get("full_text")
                        .or_else(|| chunk.metadata.get("content"))
                        .or_else(|| chunk.metadata.get("text"))
                        .map(|s| s.as_str())
                        .unwrap_or(&chunk.text)
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            "Document content not found in index".to_string()
        };

        Ok(full_text)
    }
}


// Helper function to detect code files
fn is_code_file(path: &str) -> bool {
    let code_extensions = [
        "rs", "py", "js", "ts", "tsx", "jsx", "java", "cpp", "c", "h",
        "hpp", "go", "rb", "php", "cs", "swift", "kt", "scala", "r",
        "m", "mm", "vue", "svelte", "sol", "zig", "nim", "cr", "ex",
        "exs", "erl", "hrl", "clj", "lisp", "scm", "rkt", "hs", "ml",
        "fs", "fsx", "erl", "elm", "dart", "lua", "pl", "sh", "bash",
        "zsh", "fish", "yaml", "yml", "toml", "json", "xml", "sql"
    ];

    path.split('.').last()
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
            if Command::new("code").arg("--goto").arg(&line_arg).spawn().is_ok() {
                return Ok(format!("Opened in VS Code: {}", line_arg));
            }
        }

        #[cfg(target_os = "linux")]
        {
            if Command::new("code").arg("--goto").arg(&line_arg).spawn().is_ok() {
                return Ok(format!("Opened in VS Code: {}", line_arg));
            }
        }

        // Fallback: open with system default via Tauri opener plugin
        app_handle.opener().open_path(&file_path, None::<&str>)
            .map_err(|e| format!("Failed to open file: {}", e))?;

        Ok(format!("Opened file: {} (line {})", file_path, line))
    } else {
        // For PDFs on Windows, try Adobe Reader with page/search parameters
        #[cfg(target_os = "windows")]
        if file_path.to_lowercase().ends_with(".pdf") {
            let page = page_number.unwrap_or(1);

            let adobe_params = if let Some(ref search) = search_text {
                let encoded = search.chars().take(80).collect::<String>()
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
        app_handle.opener().open_path(&file_path, None::<&str>)
            .map_err(|e| format!("Failed to open file: {}", e))?;

        Ok(format!("Opened: {}", file_path))
    }
}

/// Get document preview for hover tooltip
#[tauri::command]
pub async fn get_document_preview(
    file_path: String,
    page_number: Option<u32>,
    line_range: Option<(u32, u32)>,
) -> Result<serde_json::Value, String> {
    tracing::info!("📄 Getting preview for: {}", file_path);

    use std::path::Path;
    use std::fs;
    use std::io::{BufRead, BufReader};

    // Check if file exists
    if !Path::new(&file_path).exists() {
        return Err(format!("File not found: {}", file_path));
    }

    let path = Path::new(&file_path);
    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let extension = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let metadata = fs::metadata(&file_path)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;

    let file_size = format_file_size(metadata.len());

    // Extract excerpt based on file type
    let excerpt = if is_code_file(&file_path) || extension == "txt" || extension == "md" {
        // Extract text excerpt
        let file = fs::File::open(&file_path)
            .map_err(|e| format!("Failed to open file: {}", e))?;

        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines()
            .filter_map(|l| l.ok())
            .collect();

        if let Some((start, end)) = line_range {
            let start_idx = (start.saturating_sub(1)) as usize;
            let end_idx = (end as usize).min(lines.len());

            if start_idx < lines.len() {
                let excerpt = lines[start_idx..end_idx].join("\n");
                if excerpt.len() > 300 {
                    format!("{}...", &excerpt[..300])
                } else {
                    excerpt
                }
            } else {
                "Line range out of bounds".to_string()
            }
        } else {
            // Show first few lines
            let preview_lines = lines.iter().take(5).cloned().collect::<Vec<_>>().join("\n");
            if preview_lines.len() > 300 {
                format!("{}...", &preview_lines[..300])
            } else {
                preview_lines
            }
        }
    } else if extension == "pdf" {
        format!("PDF Document (Page {})", page_number.unwrap_or(1))
    } else if extension == "docx" || extension == "doc" {
        "Word Document - Click to open".to_string()
    } else if extension == "xlsx" || extension == "xls" {
        "Excel Spreadsheet - Click to open".to_string()
    } else {
        "Click to open file".to_string()
    };

    Ok(serde_json::json!({
        "file_name": file_name,
        "file_type": extension.to_uppercase(),
        "excerpt": excerpt,
        "total_pages": None::<u32>,
        "file_size": file_size,
        "file_path": file_path,
    }))
}

/// Format file size in human-readable format
fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Parse LLM response into structured outputs (tables, charts, forms, system actions)
#[tauri::command]
pub async fn parse_llm_response(
    response: String,
) -> Result<Vec<shodh_rag::rag::StructuredOutput>, String> {
    use shodh_rag::rag::parse_llm_response;

    let outputs = parse_llm_response(&response);
    Ok(outputs)
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

