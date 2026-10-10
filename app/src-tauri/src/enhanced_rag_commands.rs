//! Thin Tauri wrappers for indexing commands.
//! Business logic (folder preview, batch indexing, file processing) lives in shodh_rag::indexing.

use crate::audit_commands::AuditState;
use crate::event_emitter::TauriEventEmitter;
use crate::inbox_commands;
use crate::rag_commands::RagState;
use shodh_rag::audit::payload::{indexing_outcome, source_change, ChangeOrigin};
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus};
use tauri::{AppHandle, State};

// Re-export backend types so existing callers don't break
pub use shodh_rag::indexing::{IndexingOptions, IndexingResult, IndexingState};

#[tauri::command]
pub async fn link_folder_enhanced(
    app: AppHandle,
    folder_path: String,
    space_id: String,
    options: IndexingOptions,
    state: State<'_, RagState>,
    indexing_state: State<'_, IndexingState>,
    audit: State<'_, AuditState>,
) -> Result<IndexingResult, String> {
    let name = std::path::Path::new(&folder_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder_path.clone());
    let inbox_id = format!("indexing:{space_id}");
    let link = Some(InboxLink::new(
        "library",
        Some(serde_json::json!({ "kind": "source", "sourceId": space_id })),
    ));
    inbox_commands::post(
        &app,
        inbox_commands::work_item(
            inbox_id.clone(),
            InboxKind::Indexing,
            InboxStatus::Working,
            format!("Indexing {name}"),
            None,
            link.clone(),
        ),
    )
    .await;
    let emitter = TauriEventEmitter::new(app.clone());
    let result = shodh_rag::indexing::index_folder(
        &folder_path,
        &space_id,
        &options,
        &state.rag,
        &indexing_state,
        Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
    )
    .await;
    audit.record(AuditRecord::new(
        AuditEventType::SourceChange,
        source_change(
            "index_folder",
            ChangeOrigin::Ui,
            Some(&space_id),
            Some(&folder_path),
            indexing_outcome(&result),
        ),
    ));
    let (status, title, detail) = match &result {
        Ok(done) if done.failures.is_empty() => (
            InboxStatus::Done,
            format!("{name} is indexed"),
            format!("{} files indexed", done.files_processed),
        ),
        Ok(done) => (
            InboxStatus::Failed,
            format!("{name} is indexed, with failures"),
            format!(
                "{} files indexed, {} could not be read",
                done.files_processed,
                done.failures.len()
            ),
        ),
        Err(e) => (
            InboxStatus::Failed,
            format!("{name} was not indexed"),
            e.clone(),
        ),
    };
    inbox_commands::post(
        &app,
        inbox_commands::work_item(
            inbox_id,
            InboxKind::Indexing,
            status,
            title,
            Some(detail),
            link,
        ),
    )
    .await;
    result
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
    audit: State<'_, AuditState>,
) -> Result<IndexingResult, String> {
    let emitter = TauriEventEmitter::new(app);
    let result = shodh_rag::indexing::index_single_file(
        &file_path,
        &space_id,
        &state.rag,
        Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
    )
    .await;
    audit.record(AuditRecord::new(
        AuditEventType::SourceChange,
        source_change(
            "add_file",
            ChangeOrigin::Ui,
            Some(&space_id),
            Some(&file_path),
            indexing_outcome(&result),
        ),
    ));
    result
}
