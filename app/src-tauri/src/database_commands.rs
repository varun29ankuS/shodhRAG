//! Database management commands for clearing and resetting data

use crate::rag_commands::RagState;
use std::fs;
use std::io::Write;
use std::path::Path;
use tauri::State;

/// Clear all data from the database and reset to fresh state
#[tauri::command]
pub async fn reset_database(state: State<'_, RagState>) -> Result<String, String> {
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
#[tauri::command]
pub async fn clear_all_documents(state: State<'_, RagState>) -> Result<String, String> {
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

/// Delete a specific space and all its documents permanently
#[tauri::command]
pub async fn delete_space_permanently(
    state: State<'_, RagState>,
    space_id: String,
) -> Result<String, String> {
    tracing::info!("=== Permanently deleting space: {} ===", space_id);

    // Delete all chunks belonging to this space from vector store + text index
    let mut rag_guard = state.rag.write().await;
    let rag = &mut *rag_guard;

    let deleted_count = rag
        .delete_by_space_id(&space_id)
        .await
        .map_err(|e| format!("Failed to delete documents: {}", e))?;

    tracing::info!("Deleted {} chunks from space {}", deleted_count, space_id);
    drop(rag_guard);

    // Delete the space from SpaceManager and save to disk
    let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
    space_manager
        .delete_space(&space_id)
        .map_err(|e| format!("Failed to delete space: {}", e))?;

    Ok(format!("Space {} permanently deleted", space_id))
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

/// Create a backup file with custom data
#[tauri::command]
pub async fn save_backup_file(
    state: State<'_, RagState>,
    file_name: String,
    data: String,
) -> Result<String, String> {
    tracing::info!("=== Saving backup file: {} ===", file_name);

    let backup_dir = state.app_paths.data_dir.join("backups");

    fs::create_dir_all(&backup_dir)
        .map_err(|e| format!("Failed to create backup directory: {}", e))?;

    let backup_path = backup_dir.join(&file_name);

    let mut file = fs::File::create(&backup_path)
        .map_err(|e| format!("Failed to create backup file: {}", e))?;

    file.write_all(data.as_bytes())
        .map_err(|e| format!("Failed to write backup data: {}", e))?;

    tracing::info!("Backup file saved at: {:?}", backup_path);

    Ok(backup_path.to_string_lossy().to_string())
}

/// Read a backup file
#[tauri::command]
pub async fn read_backup_file(
    state: State<'_, RagState>,
    backup_path: String,
) -> Result<String, String> {
    tracing::info!("=== Reading backup file: {} ===", backup_path);

    let path = Path::new(&backup_path);

    if !path.exists() {
        return Err(format!("Backup file not found: {}", backup_path));
    }

    fs::read_to_string(path).map_err(|e| format!("Failed to read backup file: {}", e))
}

/// Restore a space from backup data
#[tauri::command]
pub async fn restore_space_from_backup(
    state: State<'_, RagState>,
    space_data: serde_json::Value,
) -> Result<String, String> {
    tracing::info!("=== Restoring space from backup ===");

    let space: crate::space_manager::Space =
        serde_json::from_value(space_data).map_err(|e| format!("Invalid space data: {}", e))?;

    let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;

    // Directly add the space to the spaces vector
    let mut spaces = space_manager.spaces.lock().map_err(|e| e.to_string())?;

    // Check if space with this ID already exists
    if spaces.iter().any(|s| s.id == space.id) {
        return Err(format!("Space with ID {} already exists", space.id));
    }

    spaces.push(space.clone());
    drop(spaces); // Release lock

    space_manager
        .save_spaces()
        .map_err(|e| format!("Failed to save restored space: {}", e))?;

    Ok(format!("Space '{}' restored successfully", space.name))
}

/// List all available backup files
#[tauri::command]
pub async fn list_backup_files(state: State<'_, RagState>) -> Result<Vec<BackupFileInfo>, String> {
    let backup_dir = state.app_paths.data_dir.join("backups");

    if !backup_dir.exists() {
        return Ok(vec![]);
    }

    let mut backups = Vec::new();

    if let Ok(entries) = fs::read_dir(&backup_dir) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file()
                    && entry.path().extension().and_then(|s| s.to_str()) == Some("json")
                {
                    if let Some(file_name) = entry.file_name().to_str() {
                        backups.push(BackupFileInfo {
                            file_name: file_name.to_string(),
                            file_path: entry.path().to_string_lossy().to_string(),
                            size_bytes: metadata.len(),
                            created_at: metadata
                                .modified()
                                .ok()
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs())
                                .unwrap_or(0),
                        });
                    }
                }
            }
        }
    }

    backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(backups)
}

/// Update space metadata for versioning
#[tauri::command]
pub async fn update_space_metadata(
    state: State<'_, RagState>,
    space_id: String,
    metadata: serde_json::Value,
) -> Result<String, String> {
    tracing::info!("=== Updating metadata for space: {} ===", space_id);

    use std::collections::HashMap;

    // Convert JSON value to HashMap<String, String>
    let metadata_map: HashMap<String, String> = if let serde_json::Value::Object(map) = metadata {
        map.into_iter()
            .filter_map(|(k, v)| {
                if let serde_json::Value::String(s) = v {
                    Some((k, s))
                } else {
                    Some((k, v.to_string()))
                }
            })
            .collect()
    } else {
        return Err("Invalid metadata format".to_string());
    };

    let space_manager = state.space_manager.lock().map_err(|e| e.to_string())?;
    let mut spaces = space_manager.spaces.lock().map_err(|e| e.to_string())?;

    if let Some(space) = spaces.iter_mut().find(|s| s.id == space_id) {
        space.metadata = metadata_map;
        drop(spaces); // Release lock

        space_manager
            .save_spaces()
            .map_err(|e| format!("Failed to save space metadata: {}", e))?;

        Ok("Metadata updated successfully".to_string())
    } else {
        Err(format!("Space not found: {}", space_id))
    }
}

#[derive(serde::Serialize)]
pub struct BackupFileInfo {
    pub file_name: String,
    pub file_path: String,
    pub size_bytes: u64,
    pub created_at: u64,
}
