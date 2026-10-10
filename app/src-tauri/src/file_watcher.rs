//! Keeps the Library's folders in sync with the index.
//!
//! The frontend lists its folder sources ([`sync_folder_sources`], on start
//! and whenever the list changes). Each listed folder is synced at once (the
//! startup check: changes made while the app was closed) and then watched:
//! changes are debounced and the folder is synced again. The sync itself is
//! [`shodh_rag::folder_sync::sync_folder`]; this module schedules it, one sync
//! at a time and never while background work is paused, and reports each
//! outcome to the Library as a [`FOLDER_SYNC_EVENT`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use shodh_rag::folder_sync::{is_temporary_file, sync_folder, ManifestStore, SyncReport};
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use crate::background::BackgroundState;
use crate::rag_commands::RagState;

/// Emitted after every sync of a folder source.
pub const FOLDER_SYNC_EVENT: &str = "folder-sync";

/// A folder is synced once its changes have stopped for this long.
const QUIET: Duration = Duration::from_secs(2);

/// A folder source of the Library.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FolderSourceRef {
    pub id: String,
    pub path: String,
}

/// The outcome of one sync, for the source's card.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderSyncOutcome {
    source_id: String,
    at: String,
    #[serde(flatten)]
    report: Option<SyncReport>,
    error: Option<String>,
}

struct Watched {
    path: String,
    /// Dropping the watcher closes the source's change channel, which ends
    /// its sync task.
    _watcher: RecommendedWatcher,
}

/// Managed by Tauri.
pub struct FolderSyncState {
    store: ManifestStore,
    watched: Mutex<HashMap<String, Watched>>,
    /// Held for the duration of a sync: one folder at a time.
    running: tokio::sync::Mutex<()>,
}

impl FolderSyncState {
    pub fn new(manifest_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            store: ManifestStore::in_dir(manifest_dir),
            watched: Mutex::new(HashMap::new()),
            running: tokio::sync::Mutex::new(()),
        }
    }

    fn is_watched(&self, source: &FolderSourceRef) -> bool {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&source.id)
            .is_some_and(|w| w.path == source.path)
    }
}

/// Whether an event can change what a folder source indexes.
fn is_relevant(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    ) && event.paths.iter().any(|p| !is_temporary_file(p))
}

/// Sync `source` now, then after every burst of changes, until it is no
/// longer watched.
async fn keep_in_sync(
    app: AppHandle,
    source: FolderSourceRef,
    mut changes: mpsc::UnboundedReceiver<()>,
) {
    loop {
        sync_once(&app, &source).await;
        if changes.recv().await.is_none() {
            return;
        }
        loop {
            match tokio::time::timeout(QUIET, changes.recv()).await {
                Ok(Some(())) => {}
                Ok(None) => return,
                Err(_) => break,
            }
        }
    }
}

/// The Inbox item of a sync that changed the index or failed; a sync that found
/// nothing to do adds none.
fn inbox_item(
    source: &FolderSourceRef,
    result: &Result<SyncReport, String>,
) -> Option<NewInboxItem> {
    let name = Path::new(&source.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.path.clone());
    let (status, title, detail) = match result {
        Ok(report) if report.indexed + report.removed == 0 => return None,
        Ok(report) => {
            let mut parts = Vec::new();
            if report.indexed > 0 {
                parts.push(format!("{} indexed", report.indexed));
            }
            if report.removed > 0 {
                parts.push(format!("{} removed", report.removed));
            }
            if !report.failures.is_empty() {
                parts.push(format!("{} could not be read", report.failures.len()));
            }
            let status = if report.failures.is_empty() {
                InboxStatus::Done
            } else {
                InboxStatus::Failed
            };
            (status, format!("{name} is up to date"), parts.join(", "))
        }
        Err(e) => (
            InboxStatus::Failed,
            format!("{name} was not synced"),
            e.clone(),
        ),
    };
    Some(NewInboxItem {
        id: format!("indexing:{}", source.id),
        kind: InboxKind::Indexing,
        status,
        title,
        detail: Some(detail),
        link: Some(InboxLink::new(
            "library",
            Some(serde_json::json!({ "kind": "source", "sourceId": source.id })),
        )),
        data: serde_json::json!({ "sourceId": source.id }),
    })
}

async fn sync_once(app: &AppHandle, source: &FolderSourceRef) {
    app.state::<BackgroundState>().wait_until_resumed().await;
    let state = app.state::<FolderSyncState>();
    let _turn = state.running.lock().await;
    if !state.is_watched(source) {
        return;
    }
    let rag = app.state::<RagState>().rag.clone();
    let result = sync_folder(&source.path, &source.id, &state.store, &rag, || false).await;
    if let Err(e) = &result {
        tracing::warn!(source_id = %source.id, error = %e, "folder sync failed");
    }
    if let Some(item) = inbox_item(source, &result) {
        crate::inbox_commands::post(app, item).await;
    }
    let (report, error) = match result {
        Ok(report) => (Some(report), None),
        Err(e) => (None, Some(e)),
    };
    let outcome = FolderSyncOutcome {
        source_id: source.id.clone(),
        at: chrono::Utc::now().to_rfc3339(),
        report,
        error,
    };
    if let Err(e) = app.emit(FOLDER_SYNC_EVENT, &outcome) {
        tracing::warn!("Failed to emit {FOLDER_SYNC_EVENT}: {e}");
    }
}

fn watch(app: &AppHandle, source: &FolderSourceRef) -> Result<Watched, String> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let changed = match result {
            Ok(event) => is_relevant(&event),
            // Events may have been lost: look at the whole folder again.
            Err(_) => true,
        };
        if changed {
            // The task is gone only when the source stopped being watched.
            let _ = tx.send(());
        }
    })
    .map_err(|e| format!("Cannot watch {}: {e}", source.path))?;
    watcher
        .watch(Path::new(&source.path), RecursiveMode::Recursive)
        .map_err(|e| format!("Cannot watch {}: {e}", source.path))?;
    tauri::async_runtime::spawn(keep_in_sync(app.clone(), source.clone(), rx));
    Ok(Watched {
        path: source.path.clone(),
        _watcher: watcher,
    })
}

/// The Library's folder sources: watch and sync these, stop watching the
/// rest (a removed source's manifest is deleted). Paths that are single
/// files are not folders and are skipped.
#[tauri::command]
pub async fn sync_folder_sources(
    app: AppHandle,
    sources: Vec<FolderSourceRef>,
) -> Result<(), String> {
    let state = app.state::<FolderSyncState>();
    let mut watched = state.watched.lock().unwrap_or_else(|e| e.into_inner());
    let stale: Vec<String> = watched
        .iter()
        .filter(|(id, w)| !sources.iter().any(|s| &s.id == *id && s.path == w.path))
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        watched.remove(&id);
        if !sources.iter().any(|s| s.id == id) {
            state.store.forget(&id);
        }
    }
    for source in sources {
        if watched.contains_key(&source.id) || Path::new(&source.path).is_file() {
            continue;
        }
        match watch(&app, &source) {
            Ok(w) => {
                watched.insert(source.id.clone(), w);
            }
            Err(error) => {
                tracing::warn!(source_id = %source.id, %error, "folder not watched");
                let outcome = FolderSyncOutcome {
                    source_id: source.id,
                    at: chrono::Utc::now().to_rfc3339(),
                    report: None,
                    error: Some(error),
                };
                if let Err(e) = app.emit(FOLDER_SYNC_EVENT, &outcome) {
                    tracing::warn!("Failed to emit {FOLDER_SYNC_EVENT}: {e}");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RemoveKind};
    use std::path::PathBuf;

    fn event(kind: EventKind, paths: &[&str]) -> Event {
        Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    #[test]
    fn only_changes_to_real_files_start_a_sync() {
        assert!(is_relevant(&event(
            EventKind::Create(CreateKind::File),
            &["C:/papers/a.pdf"]
        )));
        assert!(is_relevant(&event(
            EventKind::Modify(ModifyKind::Any),
            &["C:/papers/~$a.docx", "C:/papers/a.docx"]
        )));
        // A removed folder has no extension: still relevant.
        assert!(is_relevant(&event(
            EventKind::Remove(RemoveKind::Folder),
            &["C:/papers/old"]
        )));
        assert!(!is_relevant(&event(
            EventKind::Create(CreateKind::File),
            &["C:/papers/~$a.docx", "C:/papers/b.pdf.crdownload"]
        )));
        assert!(!is_relevant(&event(
            EventKind::Access(notify::event::AccessKind::Any),
            &["C:/papers/a.pdf"]
        )));
    }

    #[test]
    fn outcomes_serialize_for_the_library() {
        let ok = FolderSyncOutcome {
            source_id: "s1".into(),
            at: "2026-10-07T10:00:00Z".into(),
            report: Some(SyncReport {
                indexed: 2,
                removed: 1,
                files: 5,
                failures: Vec::new(),
            }),
            error: None,
        };
        let json = serde_json::to_value(&ok).unwrap();
        assert_eq!(json["sourceId"], "s1");
        assert_eq!(json["indexed"], 2);
        assert_eq!(json["removed"], 1);
        assert_eq!(json["files"], 5);
        assert_eq!(json["error"], serde_json::Value::Null);
        let failed = FolderSyncOutcome {
            source_id: "s1".into(),
            at: "2026-10-07T10:00:00Z".into(),
            report: None,
            error: Some("D:/papers is not available".into()),
        };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["error"], "D:/papers is not available");
        assert!(json.get("files").is_none());
    }
}
