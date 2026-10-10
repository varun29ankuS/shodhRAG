//! Database management commands for clearing and resetting data

use crate::rag_commands::RagState;
use std::fs;
use std::path::Path;
use tauri::State;

use crate::audit_commands::AuditState;
use serde_json::json;
use shodh_rag::audit::payload::{source_change, ChangeOrigin};
use shodh_rag::audit::{AuditEventType, AuditRecord};

/// Clear all data from the database and reset to fresh state
async fn reset_database_inner(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("=== Resetting database ===");

    // Step 1: Clear all spaces from memory and disk
    {
        let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
        space_manager
            .clear_all_spaces()
            .map_err(|e| format!("Failed to clear spaces: {}", e))?;
    } // Drop space_manager lock before await

    // Step 2: Delete the spaces.json file from persistent storage
    let spaces_file = state.app_paths.data_dir.join("spaces.json");
    if spaces_file.exists() {
        fs::remove_file(&spaces_file)
            .map_err(|e| format!("Failed to delete spaces.json: {}", e))?;
        tracing::info!("Deleted spaces.json from {:?}", spaces_file);
    }

    // Step 3: Clear the RAG database
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Clear all data using the public method
    if let Err(e) = rag.clear_all_data().await {
        tracing::info!("Failed to clear all data: {}", e);
        return Err(format!("Failed to clear all data: {}", e));
    }

    tracing::info!("All data cleared successfully");

    Ok("Database reset complete. Restart the application for a fresh start.".to_string())
}

/// Clear all documents from the database but keep spaces
async fn clear_all_documents_inner(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("=== Clearing all documents ===");

    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Clear all data from RAG
    rag.clear_all_data()
        .await
        .map_err(|e| format!("Failed to clear data: {}", e))?;
    tracing::info!("Cleared all documents from RAG");
    drop(rag_guard);

    // Clear document associations from spaces
    let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
    let spaces = space_manager
        .get_spaces()
        .map_err(|e| format!("Failed to get spaces: {}", e))?;

    for mut space in spaces {
        space.documents.clear();
        space.document_count = 0;
    }

    // Save the updated spaces
    space_manager
        .save_spaces()
        .map_err(|e| format!("Failed to save spaces: {}", e))?;

    Ok("All documents cleared from database".to_string())
}

/// Get database statistics
#[tauri::command]
pub async fn get_database_stats(state: State<'_, RagState>) -> Result<DatabaseStats, String> {
    let mut stats = DatabaseStats::default();

    // Get space count from state
    {
        let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
        let spaces = space_manager
            .get_spaces()
            .map_err(|e| format!("Failed to get spaces: {}", e))?;
        stats.total_spaces = spaces.len();
        stats.total_documents_in_spaces = spaces.iter().map(|s| s.document_count).sum();
    } // Drop space_manager lock before await

    // Get database size from the actual data directory (not db_path which points elsewhere)
    let data_dir = &state.app_paths.data_dir;
    if data_dir.exists() {
        stats.database_size_mb = get_dir_size(data_dir) as f64 / (1024.0 * 1024.0);
    }

    // Get document count from RAG
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Get stats from RAG engine
    let rag_stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks: usize = rag_stats
        .get("total_chunks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total_docs = rag.count_documents().await.unwrap_or(0);

    stats.total_documents = total_docs;
    stats.total_vectors = total_chunks;

    Ok(stats)
}

/// Clean up orphaned documents (documents not associated with any space)
#[tauri::command]
pub async fn cleanup_orphaned_documents(state: State<'_, RagState>) -> Result<String, String> {
    tracing::info!("=== Cleaning up orphaned documents ===");

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // List all documents by metadata (not search)
    let all_docs = rag
        .list_documents(None, 100000)
        .await
        .map_err(|e| e.to_string())?;

    // Get all valid space IDs
    let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
    let valid_space_ids: Vec<String> = space_manager
        .get_spaces()
        .map_err(|e| format!("Failed to get spaces: {}", e))?
        .iter()
        .map(|s| s.id.clone())
        .collect();

    let mut orphaned_count = 0;
    for doc in all_docs.iter() {
        // Check if document has a space_id that exists
        if let Some(space_id) = doc.metadata.get("space_id") {
            if !valid_space_ids.contains(space_id) {
                orphaned_count += 1;
                // Note: Need delete_document method in ComprehensiveRAG
                tracing::info!(
                    "Found orphaned document with invalid space_id: {}",
                    space_id
                );
            }
        } else {
            orphaned_count += 1;
            tracing::info!("Found orphaned document with no space_id");
        }
    }

    Ok(format!(
        "Found {} orphaned documents. Manual cleanup required for now.",
        orphaned_count
    ))
}

#[derive(serde::Serialize, Default)]
pub struct DatabaseStats {
    pub total_spaces: usize,
    pub total_documents: usize,
    pub total_vectors: usize,
    pub total_documents_in_spaces: usize,
    pub database_size_mb: f64,
}

// Helper function to calculate directory size
fn get_dir_size(path: &Path) -> u64 {
    let mut size = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_dir() {
                    size += get_dir_size(&entry.path());
                } else {
                    size += metadata.len();
                }
            }
        }
    }
    size
}

/// List all distinct indexed source files from the vector store.
/// Returns (doc_id, title, source_path) for each unique document.
#[tauri::command]
pub async fn list_indexed_sources(
    state: State<'_, RagState>,
) -> Result<Vec<IndexedSourceInfo>, String> {
    let rag_guard = state.rag.read().await;
    let docs = rag_guard
        .get_document_info()
        .await
        .map_err(|e| format!("Failed to list sources: {}", e))?;

    let sources: Vec<IndexedSourceInfo> = docs
        .into_iter()
        .map(|(doc_id, title, source)| IndexedSourceInfo {
            doc_id,
            title,
            source,
        })
        .collect();

    tracing::info!("Listed {} indexed sources", sources.len());
    Ok(sources)
}

#[derive(serde::Serialize)]
pub struct IndexedSourceInfo {
    pub doc_id: String,
    pub title: String,
    pub source: String,
}

/// Clear every document from the index (audited).
#[tauri::command]
pub async fn clear_all_documents(
    state: State<'_, RagState>,
    audit: State<'_, AuditState>,
) -> Result<String, String> {
    let result = clear_all_documents_inner(state).await;
    let outcome = match &result {
        Ok(_) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    audit.record(AuditRecord::new(
        AuditEventType::SourceChange,
        source_change("clear_all", ChangeOrigin::Ui, None, None, outcome),
    ));
    result
}

/// Reset the index and spaces (audited).
#[tauri::command]
pub async fn reset_database(
    state: State<'_, RagState>,
    audit: State<'_, AuditState>,
) -> Result<String, String> {
    let result = reset_database_inner(state).await;
    let outcome = match &result {
        Ok(_) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    audit.record(AuditRecord::new(
        AuditEventType::SourceChange,
        source_change("reset", ChangeOrigin::Ui, None, None, outcome),
    ));
    result
}
