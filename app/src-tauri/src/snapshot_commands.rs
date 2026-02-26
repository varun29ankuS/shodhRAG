//! Tauri commands for snapshot operations

use crate::snapshot_manager::{SnapshotManager, SnapshotType, RestoreMode, Snapshot};
use crate::rag_commands::RagState;
use tauri::State;
use serde::{Serialize, Deserialize};
use std::path::PathBuf;

/// Create snapshot request
#[derive(Debug, Deserialize)]
pub struct CreateSnapshotRequest {
    pub space_id: String,
    pub space_name: String,
    pub name: String,
    pub description: Option<String>,
    pub snapshot_type: String, // "manual", "automatic", "pre_update", "export"
}

/// Restore snapshot request
#[derive(Debug, Deserialize)]
pub struct RestoreSnapshotRequest {
    pub snapshot_id: String,
    pub restore_mode: String, // "replace", "merge", "create_new"
}

/// Create a snapshot of a space
#[tauri::command]
pub async fn create_snapshot(
    state: State<'_, RagState>,
    request: CreateSnapshotRequest,
) -> Result<Snapshot, String> {
    let rag_guard = state.rag.lock().map_err(|e| e.to_string())?;
    
    if let Some(ref rag) = *rag_guard {
        let snapshot_type = match request.snapshot_type.as_str() {
            "automatic" => SnapshotType::Automatic,
            "pre_update" => SnapshotType::PreUpdate,
            "export" => SnapshotType::Export,
            _ => SnapshotType::Manual,
        };

        let manager = SnapshotManager::new(None);
        let snapshot = manager
            .create_snapshot(
                rag,
                &request.space_id,
                &request.space_name,
                request.name,
                request.description,
                snapshot_type,
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(snapshot)
    } else {
        Err("RAG system not initialized".to_string())
    }
}

/// Restore a space from snapshot
#[tauri::command]
pub async fn restore_snapshot(
    state: State<'_, RagState>,
    request: RestoreSnapshotRequest,
) -> Result<RestoreResultResponse, String> {
    let rag_guard = state.rag.lock().map_err(|e| e.to_string())?;
    
    if let Some(ref rag) = *rag_guard {
        let restore_mode = match request.restore_mode.as_str() {
            "merge" => RestoreMode::Merge,
            "create_new" => RestoreMode::CreateNew,
            _ => RestoreMode::Replace,
        };

        let manager = SnapshotManager::new(None);
        let result = manager
            .restore_snapshot(rag, &request.snapshot_id, restore_mode)
            .await
            .map_err(|e| e.to_string())?;

        Ok(RestoreResultResponse {
            restored_documents: result.restored_documents,
            snapshot_id: result.snapshot_id,
            space_id: result.space_id,
            message: format!("Successfully restored {} documents", result.restored_documents),
        })
    } else {
        Err("RAG system not initialized".to_string())
    }
}

/// List all snapshots
#[tauri::command]
pub async fn list_snapshots(
    state: State<'_, RagState>,
    space_id: Option<String>,
) -> Result<Vec<SnapshotInfo>, String> {
    let rag_guard = state.rag.lock().map_err(|e| e.to_string())?;
    
    if let Some(ref rag) = *rag_guard {
        let manager = SnapshotManager::new(None);
        let snapshots = manager
            .list_snapshots(rag, space_id.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        Ok(snapshots.into_iter().map(|s| SnapshotInfo {
            id: s.id,
            space_id: s.space_id,
            space_name: s.space_name,
            name: s.name,
            description: s.description,
            created_at: s.created_at.to_rfc3339(),
            document_count: s.document_count,
            size_bytes: s.size_bytes,
            snapshot_type: format!("{:?}", s.snapshot_type),
        }).collect())
    } else {
        Ok(Vec::new())
    }
}

/// Delete a snapshot
#[tauri::command]
pub async fn delete_snapshot(
    state: State<'_, RagState>,
    snapshot_id: String,
) -> Result<String, String> {
    let rag_guard = state.rag.lock().map_err(|e| e.to_string())?;
    
    if let Some(ref rag) = *rag_guard {
        let manager = SnapshotManager::new(None);
        manager
            .delete_snapshot(rag, &snapshot_id)
            .await
            .map_err(|e| e.to_string())?;

        Ok(format!("Snapshot {} deleted successfully", snapshot_id))
    } else {
        Err("RAG system not initialized".to_string())
    }
}

/// Clean up old snapshots
#[tauri::command]
pub async fn cleanup_snapshots(
    state: State<'_, RagState>,
) -> Result<CleanupResponse, String> {
    let rag_guard = state.rag.lock().map_err(|e| e.to_string())?;
    
    if let Some(ref rag) = *rag_guard {
        let manager = SnapshotManager::new(None);
        let result = manager
            .cleanup_old_snapshots(rag)
            .await
            .map_err(|e| e.to_string())?;

        Ok(CleanupResponse {
            deleted_count: result.deleted_snapshots,
            remaining_count: result.remaining_snapshots,
            message: format!("Cleaned up {} old snapshots", result.deleted_snapshots),
        })
    } else {
        Err("RAG system not initialized".to_string())
    }
}

/// Export snapshot to file
#[tauri::command]
pub async fn export_snapshot(
    state: State<'_, RagState>,
    snapshot_id: String,
    export_path: String,
) -> Result<String, String> {
    let rag_guard = state.rag.lock().map_err(|e| e.to_string())?;
    
    if let Some(ref rag) = *rag_guard {
        let manager = SnapshotManager::new(None);
        let path = PathBuf::from(export_path);
        
        manager
            .export_snapshot(rag, &snapshot_id, &path)
            .await
            .map_err(|e| e.to_string())?;

        Ok(format!("Snapshot exported to {:?}", path))
    } else {
        Err("RAG system not initialized".to_string())
    }
}

/// Create automatic snapshot before major operations
#[tauri::command]
pub async fn create_auto_snapshot(
    state: State<'_, RagState>,
    space_id: String,
    space_name: String,
    operation: String,
) -> Result<String, String> {
    let rag_guard = state.rag.lock().map_err(|e| e.to_string())?;
    
    if let Some(ref rag) = *rag_guard {
        let manager = SnapshotManager::new(None);
        let snapshot = manager
            .create_snapshot(
                rag,
                &space_id,
                &space_name,
                format!("Auto: Before {}", operation),
                Some(format!("Automatic snapshot created before {}", operation)),
                SnapshotType::Automatic,
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(snapshot.id)
    } else {
        Err("RAG system not initialized".to_string())
    }
}

// Response structures
#[derive(Debug, Serialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub space_id: String,
    pub space_name: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub document_count: usize,
    pub size_bytes: usize,
    pub snapshot_type: String,
}

#[derive(Debug, Serialize)]
pub struct RestoreResultResponse {
    pub restored_documents: usize,
    pub snapshot_id: String,
    pub space_id: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct CleanupResponse {
    pub deleted_count: usize,
    pub remaining_count: usize,
    pub message: String,
}