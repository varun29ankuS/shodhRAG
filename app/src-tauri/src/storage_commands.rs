//! Storage management commands for proper document lifecycle
//! Ensures consistency between spaces and documents

use tauri::State;
use crate::rag_commands::RagState;
use crate::space_manager::SpaceManager;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    pub total_documents: usize,
    pub total_spaces: usize,
    pub database_size_mb: f64,
    pub index_size_mb: f64,
    pub cache_size_mb: f64,
    pub last_backup: Option<String>,
    pub health: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailedDocument {
    pub id: String,
    pub title: String,
    pub space_id: String,
    pub space_name: String,
    pub size_kb: f64,
    pub chunks: usize,
    pub added_at: String,
}

/// Get comprehensive storage statistics
#[tauri::command]
pub async fn get_storage_stats(
    state: State<'_, RagState>,
    space_manager: State<'_, SpaceManager>,
) -> Result<StorageStats, String> {
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;
    let stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks: usize = stats.get("total_chunks").and_then(|s| s.parse().ok()).unwrap_or(0);

    let spaces = space_manager.get_spaces()
        .map_err(|e| e.to_string())?;

    let total_docs = rag.count_documents().await.unwrap_or(0);

    // Calculate actual database size from the data directory
    let data_dir = stats.get("data_dir").cloned().unwrap_or_default();
    let db_size = if !data_dir.is_empty() && Path::new(&data_dir).exists() {
        get_dir_size(Path::new(&data_dir)) as f64 / (1024.0 * 1024.0)
    } else {
        0.0
    };

    Ok(StorageStats {
        total_documents: total_docs,
        total_spaces: spaces.len(),
        database_size_mb: db_size,
        index_size_mb: 0.0,
        cache_size_mb: 0.0,
        last_backup: None,
        health: "healthy".to_string(),
    })
}

/// Get detailed documents for a space
#[tauri::command]
pub async fn get_space_documents_detailed(
    state: State<'_, RagState>,
    space_manager: State<'_, SpaceManager>,
    space_id: String,
) -> Result<Vec<DetailedDocument>, String> {
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // List documents in this space by metadata filter (not search)
    let filter = shodh_rag::types::MetadataFilter {
        space_id: Some(space_id.clone()),
        ..Default::default()
    };
    let documents = rag.list_documents(Some(filter), 10000)
        .await
        .map_err(|e| e.to_string())?;

    // Get space name
    let spaces = space_manager.get_spaces().map_err(|e| e.to_string())?;
    let space_name = spaces.iter()
        .find(|s| s.id == space_id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let detailed_docs: Vec<DetailedDocument> = documents.into_iter()
        .map(|doc| {
            DetailedDocument {
                id: doc.id.to_string(),
                title: doc.metadata.get("title")
                    .or_else(|| doc.metadata.get("file_name"))
                    .unwrap_or(&"Untitled".to_string())
                    .clone(),
                space_id: space_id.clone(),
                space_name: space_name.clone(),
                size_kb: doc.snippet.len() as f64 / 1024.0,
                chunks: doc.metadata.get("chunk_count")
                    .and_then(|c| c.parse::<usize>().ok())
                    .unwrap_or(1),
                added_at: doc.metadata.get("indexed_at")
                    .unwrap_or(&chrono::Utc::now().to_rfc3339())
                    .clone(),
            }
        })
        .collect();

    Ok(detailed_docs)
}

/// Delete multiple documents in batch
#[tauri::command]
pub async fn delete_documents_batch(
    state: State<'_, RagState>,
    space_manager: State<'_, SpaceManager>,
    document_ids: Vec<String>,
) -> Result<usize, String> {
    // Note: ComprehensiveRAG (RAGEngine) only supports delete_by_source, not individual doc deletion.
    // This is a limitation of the new API. We log the request and return 0 deleted.
    tracing::info!("delete_documents_batch: Requested deletion of {} documents (not supported in current API)", document_ids.len());

    // Update space document counts
    update_space_counts(&state, &space_manager).await?;

    Ok(0)
}

/// Clear all documents from a space
#[tauri::command]
pub async fn clear_space_documents(
    state: State<'_, RagState>,
    space_manager: State<'_, SpaceManager>,
    space_id: String,
) -> Result<usize, String> {
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    // Use delete_by_source with the space_id as source identifier
    let deleted = rag.delete_by_source(&space_id)
        .await
        .map_err(|e| format!("Failed to delete documents: {}", e))?;

    // Update space to have 0 documents
    if let Ok(mut spaces) = space_manager.spaces.lock() {
        if let Some(space) = spaces.iter_mut().find(|s| s.id == space_id) {
            space.document_count = 0;
            space.documents.clear();
        }
    }

    // Save spaces
    space_manager.save_spaces()
        .map_err(|e| format!("Failed to save spaces: {}", e))?;

    Ok(deleted)
}

/// Optimize storage by triggering index creation if needed
#[tauri::command]
pub async fn optimize_storage(
    state: State<'_, RagState>,
) -> Result<String, String> {
    tracing::info!("Optimizing storage...");

    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // Trigger index optimization
    rag.optimize()
        .await
        .map_err(|e| format!("Failed to optimize: {}", e))?;

    let stats = rag.get_statistics().await.unwrap_or_default();
    let total_chunks = stats.get("total_chunks").cloned().unwrap_or_default();

    Ok(format!("Storage optimized. Total chunks: {}", total_chunks))
}

/// Create a backup of the current database
#[tauri::command]
pub async fn create_backup(
    state: State<'_, RagState>,
) -> Result<String, String> {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let backup_name = format!("backup_{}", timestamp);

    // Create backups directory
    let backup_dir = Path::new("./backups");
    if !backup_dir.exists() {
        fs::create_dir_all(backup_dir)
            .map_err(|e| format!("Failed to create backup directory: {}", e))?;
    }

    // Copy kalki_data to backup directory
    let source = Path::new("./kalki_data");
    let dest = backup_dir.join(&backup_name);

    if source.exists() {
        copy_dir_all(source, &dest)
            .map_err(|e| format!("Failed to create backup: {}", e))?;
    }

    // Also backup spaces.json
    let spaces_file = Path::new("./data/spaces.json");
    if spaces_file.exists() {
        let spaces_backup = dest.join("spaces.json");
        fs::copy(spaces_file, spaces_backup)
            .map_err(|e| format!("Failed to backup spaces: {}", e))?;
    }

    Ok(backup_name)
}

/// Restore from a backup
#[tauri::command]
pub async fn restore_backup(
    backup_name: String,
) -> Result<String, String> {
    let backup_path = Path::new("./backups").join(&backup_name);

    if !backup_path.exists() {
        return Err(format!("Backup {} not found", backup_name));
    }

    // Stop using current database first
    // This would need proper shutdown of ComprehensiveRAG

    // Restore kalki_data
    let source = &backup_path;
    let dest = Path::new("./kalki_data");

    // Remove current data
    if dest.exists() {
        fs::remove_dir_all(dest)
            .map_err(|e| format!("Failed to remove current data: {}", e))?;
    }

    // Copy backup
    copy_dir_all(source, dest)
        .map_err(|e| format!("Failed to restore backup: {}", e))?;

    // Restore spaces.json
    let spaces_backup = backup_path.join("spaces.json");
    if spaces_backup.exists() {
        let spaces_dest = Path::new("./data/spaces.json");
        fs::copy(spaces_backup, spaces_dest)
            .map_err(|e| format!("Failed to restore spaces: {}", e))?;
    }

    Ok("Backup restored successfully. Please restart the application.".to_string())
}

// Helper functions

fn get_dir_size(path: &Path) -> u64 {
    let mut size = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    size += metadata.len();
                } else if metadata.is_dir() {
                    size += get_dir_size(&entry.path());
                }
            }
        }
    }
    size
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

async fn update_space_counts(
    state: &State<'_, RagState>,
    space_manager: &State<'_, SpaceManager>,
) -> Result<(), String> {
    let rag_guard = state.rag.read().await;
    let rag = &*rag_guard;

    // List all chunks to count per space (not search)
    let all_results = rag.list_documents(None, 100000)
        .await
        .unwrap_or_default();

    let mut spaces = space_manager.spaces.lock().map_err(|e| e.to_string())?;

    for space in spaces.iter_mut() {
        let count = all_results.iter()
            .filter(|r| r.metadata.get("space_id").map(|s| s == &space.id).unwrap_or(false))
            .count();
        space.document_count = count;
    }

    drop(spaces);
    space_manager.save_spaces()
}