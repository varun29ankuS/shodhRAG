//! Thin Tauri wrappers for indexing commands.
//! Business logic (folder preview, batch indexing, file processing) lives in shodh_rag::indexing.

use crate::chat_engine::TauriEventEmitter;
use crate::rag_commands::RagState;
use tauri::{AppHandle, State};

// Re-export backend types so existing callers don't break
pub use shodh_rag::indexing::{FolderPreview, IndexingOptions, IndexingResult, IndexingState};

#[tauri::command]
pub async fn preview_folder(folder_path: String) -> Result<FolderPreview, String> {
    shodh_rag::indexing::preview_folder(&folder_path)
}

#[tauri::command]
pub async fn link_folder_enhanced(
    app: AppHandle,
    folder_path: String,
    space_id: String,
    options: IndexingOptions,
    state: State<'_, RagState>,
    indexing_state: State<'_, IndexingState>,
) -> Result<IndexingResult, String> {
    let emitter = TauriEventEmitter::new(app);
    let mut rag_guard = state.rag.write().await;

    shodh_rag::indexing::index_folder(
        &folder_path,
        &space_id,
        &options,
        &mut *rag_guard,
        &indexing_state,
        Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
    )
    .await
}

#[tauri::command]
pub async fn test_indexing() -> Result<String, String> {
    Ok("Backend is responding".to_string())
}

#[tauri::command]
pub async fn pause_indexing(indexing_state: State<'_, IndexingState>) -> Result<(), String> {
    indexing_state.pause();
    Ok(())
}

#[tauri::command]
pub async fn resume_indexing(indexing_state: State<'_, IndexingState>) -> Result<(), String> {
    indexing_state.resume();
    Ok(())
}

#[tauri::command]
pub async fn cancel_indexing(indexing_state: State<'_, IndexingState>) -> Result<(), String> {
    indexing_state.cancel();
    Ok(())
}

#[tauri::command]
pub async fn check_path_type(path: String) -> Result<serde_json::Value, String> {
    let (is_dir, is_file) = shodh_rag::indexing::check_path_type(&path)?;
    Ok(serde_json::json!({
        "isDirectory": is_dir,
        "isFile": is_file,
        "exists": true
    }))
}

#[tauri::command]
pub async fn index_single_file(
    app: AppHandle,
    file_path: String,
    space_id: String,
    state: State<'_, RagState>,
) -> Result<IndexingResult, String> {
    let emitter = TauriEventEmitter::new(app);
    let mut rag_guard = state.rag.write().await;

    shodh_rag::indexing::index_single_file(
        &file_path,
        &space_id,
        &mut *rag_guard,
        Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
    )
    .await
}
