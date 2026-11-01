//! Thin Tauri wrappers for indexing commands.
//! Business logic (folder preview, batch indexing, file processing) lives in shodh_rag::indexing.

use crate::audit_commands::AuditState;
use crate::event_emitter::TauriEventEmitter;
use crate::file_watcher::FolderSyncState;
use crate::inbox_commands;
use crate::rag_commands::RagState;
use shodh_rag::audit::payload::{indexing_outcome, source_change, ChangeOrigin};
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::folder_sync::Adoption;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
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
    folder_sync: State<'_, FolderSyncState>,
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
    // With `skip_indexed`, files already indexed under a source that is not
    // kept in sync are adopted; the folders kept in sync keep theirs.
    let listed = folder_sync.watched_ids();
    let result = shodh_rag::indexing::index_folder_adopting(
        &folder_path,
        &space_id,
        &options,
        &state.rag,
        &indexing_state,
        Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
        Some(Adoption {
            store: folder_sync.store(),
            listed: &listed,
        }),
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
    let removed = indexing_state.is_removed(&space_id);
    inbox_commands::post(&app, finished_item(inbox_id, &name, link, removed, &result)).await;
    result
}

/// The Inbox item of a folder's indexing that ended. A run cut short, or
/// refused, because its source was removed (`removed`) did not index the
/// folder and is `quiet`: the user removed it, so nothing is announced.
fn finished_item(
    inbox_id: String,
    name: &str,
    link: Option<InboxLink>,
    removed: bool,
    result: &Result<IndexingResult, String>,
) -> NewInboxItem {
    let (status, title, detail) = match result {
        _ if removed => (
            InboxStatus::Done,
            format!("{name} was removed from the Library"),
            "Indexing stopped when the folder was removed.".to_string(),
        ),
        Ok(done) if done.failures.is_empty() => (
            InboxStatus::Done,
            format!("{name} is indexed"),
            indexed_detail(done),
        ),
        Ok(done) => (
            InboxStatus::Failed,
            format!("{name} is indexed, with failures"),
            indexed_detail(done),
        ),
        Err(e) => (
            InboxStatus::Failed,
            format!("{name} was not indexed"),
            e.clone(),
        ),
    };
    let mut item = inbox_commands::work_item(
        inbox_id,
        InboxKind::Indexing,
        status,
        title,
        Some(detail),
        link,
    );
    if removed {
        item.data = serde_json::json!({ "quiet": true });
    }
    item
}

/// What a folder's indexing did, for its Inbox item: "3 files indexed,
/// 9 already indexed, 1 could not be read". Files adopted from another
/// source count as already indexed.
fn indexed_detail(done: &IndexingResult) -> String {
    let mut parts = Vec::new();
    if done.files_processed > 0 || done.files_unchanged == 0 {
        parts.push(format!("{} files indexed", done.files_processed));
    }
    if done.files_unchanged > 0 {
        parts.push(if parts.is_empty() {
            format!("{} files already indexed", done.files_unchanged)
        } else {
            format!("{} already indexed", done.files_unchanged)
        });
    }
    if !done.failures.is_empty() {
        parts.push(format!("{} could not be read", done.failures.len()));
    }
    parts.join(", ")
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
    indexing_state: State<'_, IndexingState>,
    audit: State<'_, AuditState>,
) -> Result<IndexingResult, String> {
    let emitter = TauriEventEmitter::new(app);
    // Registered while it runs: removing the source waits for it, and a
    // removed source is not indexed again.
    let result = match indexing_state.begin_run_at(&space_id, &file_path) {
        Some(_run) => {
            shodh_rag::indexing::index_single_file(
                &file_path,
                &space_id,
                &state.rag,
                Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
            )
            .await
        }
        None => Err(format!(
            "{file_path} was removed from the Library and is not indexed"
        )),
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::indexing::FileFailure;

    fn done(processed: usize, unchanged: usize, failed: usize) -> IndexingResult {
        let failures: Vec<FileFailure> = (0..failed)
            .map(|i| FileFailure {
                file: format!("f{i}.pdf"),
                reason: "unreadable".to_string(),
            })
            .collect();
        IndexingResult {
            files_processed: processed,
            total_chunks: 0,
            failed_files: failures.iter().map(|f| f.file.clone()).collect(),
            failures,
            files_unchanged: unchanged,
            duration: 0,
            stopped: false,
        }
    }

    #[test]
    fn indexing_its_source_removal_cut_short_is_not_announced() {
        let mut memory = crate::notifications::IndexingMemory::default();
        let removed = |result: Result<IndexingResult, String>| {
            finished_item("indexing:s1".to_string(), "Papers", None, true, &result)
        };
        // Cut short part way, or refused before it started: the Inbox says the
        // folder was removed, and nothing is announced.
        for item in [
            removed(Ok(done(3, 0, 0))),
            removed(Err(
                "Papers was removed from the Library and is not indexed".to_string(),
            )),
        ] {
            assert_eq!(item.status, InboxStatus::Done);
            assert_eq!(item.title, "Papers was removed from the Library");
            assert_eq!(item.data["quiet"], true);
            assert_eq!(memory.outcome(&item), None);
        }
        // Indexing that ran to its end is announced.
        let finished = finished_item(
            "indexing:s1".to_string(),
            "Papers",
            None,
            false,
            &Ok(done(3, 0, 0)),
        );
        assert_eq!(finished.title, "Papers is indexed");
        assert!(memory.outcome(&finished).is_some());
        let failed = finished_item(
            "indexing:s1".to_string(),
            "Papers",
            None,
            false,
            &Err("disk full".to_string()),
        );
        assert_eq!(failed.status, InboxStatus::Failed);
        assert!(memory.outcome(&failed).is_some());
    }

    #[test]
    fn the_inbox_counts_files_already_indexed() {
        assert_eq!(indexed_detail(&done(4, 0, 0)), "4 files indexed");
        assert_eq!(indexed_detail(&done(0, 0, 0)), "0 files indexed");
        assert_eq!(indexed_detail(&done(0, 12, 0)), "12 files already indexed");
        assert_eq!(
            indexed_detail(&done(3, 9, 1)),
            "3 files indexed, 9 already indexed, 1 could not be read"
        );
        assert_eq!(
            indexed_detail(&done(0, 2, 1)),
            "2 files already indexed, 1 could not be read"
        );
    }
}
