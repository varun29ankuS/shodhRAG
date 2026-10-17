//! Keeps the Library's folders in sync with the index.
//!
//! The frontend lists its folder sources ([`sync_folder_sources`], on start
//! and whenever the list changes). Each listed folder is synced at once (the
//! startup check: changes made while the app was closed) and then watched:
//! changes are debounced and the folder is synced again. The sync itself is
//! [`shodh_rag::folder_sync::sync_folder`]; this module schedules it, one sync
//! at a time and never while background work is paused, and reports each
//! outcome to the Library as a [`FOLDER_SYNC_EVENT`].
//!
//! Listed sources that are single files are not watched; each is indexed
//! again once if it was indexed in an older index format
//! ([`upgrade_file_source`]), as a folder's files are on its sync. Once per
//! run, documents of an older format that no listed source owns (files added
//! in a chat) are brought up to date too ([`upgrade_unowned`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use shodh_rag::folder_sync::{
    is_temporary_file, sync_folder, upgrade_file_source, upgrade_unowned, ManifestStore, SyncReport,
};
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use shodh_rag::indexing::{FileFailure, IndexingState};
use shodh_rag::RAGEngine;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, RwLock};

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
    /// The documents no Library source owns were checked for an older index
    /// format in this run.
    unowned_checked: std::sync::atomic::AtomicBool,
    /// Scans read without the scan reader were looked for in this run, once
    /// the Library's folders are watched (see [crate::scan_reader::reread_on_start]).
    scans_checked: std::sync::atomic::AtomicBool,
    /// Sources removed from the Library ([`Self::remove_source`]), in this
    /// run and before: never watched again, even when a list that still
    /// names them arrives (a saved Library list older than the removal).
    removed: Mutex<HashSet<String>>,
    /// Where [`Self::removed`] is kept ([`REMOVED_SOURCES_FILE`]).
    removed_file: std::path::PathBuf,
}

/// The file, in the folder manifests' directory, that lists the sources
/// removed from the Library. Not a manifest: manifests are `<id>.json`
/// directly in the directory, this is in a folder of its own.
const REMOVED_SOURCES_FILE: &str = "removed/sources.json";

/// The removed sources listed in `file`; none when it does not exist. A
/// file that cannot be read is reported and read as none.
fn load_removed(file: &Path) -> HashSet<String> {
    match std::fs::read_to_string(file) {
        Ok(text) => match serde_json::from_str::<Vec<String>>(&text) {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                tracing::warn!(file = %file.display(), error = %e, "removed sources unreadable");
                HashSet::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
        Err(e) => {
            tracing::warn!(file = %file.display(), error = %e, "removed sources unreadable");
            HashSet::new()
        }
    }
}

/// Saves `removed` to `file`: written to a temporary file, flushed, and
/// renamed over the previous list, so a kill leaves one list or the other.
fn save_removed(file: &Path, removed: &HashSet<String>) -> Result<(), String> {
    use std::io::Write;
    let mut ids: Vec<&String> = removed.iter().collect();
    ids.sort();
    let text = serde_json::to_string(&ids).map_err(|e| e.to_string())?;
    let io = |e: std::io::Error| format!("{} could not be saved: {e}", file.display());
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let tmp = file.with_extension("json.tmp");
    let written = (|| {
        let mut out = std::fs::File::create(&tmp)?;
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&tmp, file)
    })();
    written.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })
}

impl FolderSyncState {
    pub fn new(manifest_dir: impl Into<std::path::PathBuf>) -> Self {
        let manifest_dir = manifest_dir.into();
        let removed_file = manifest_dir.join(REMOVED_SOURCES_FILE);
        Self {
            store: ManifestStore::in_dir(&manifest_dir),
            watched: Mutex::new(HashMap::new()),
            running: tokio::sync::Mutex::new(()),
            unowned_checked: std::sync::atomic::AtomicBool::new(false),
            scans_checked: std::sync::atomic::AtomicBool::new(false),
            removed: Mutex::new(load_removed(&removed_file)),
            removed_file,
        }
    }

    /// Forgets the removals nothing can bring back any more, at start: a
    /// removed source the saved Library list (`listed`, the list the Library
    /// restores from) no longer names and that has no manifest. The list
    /// names it until the Library saved itself without it; the manifest is
    /// deleted only once the source's chunks are, so a removal cut short
    /// (the app killed while it deleted) keeps its record. Returns the ids
    /// forgotten.
    pub fn prune_removed(&self, listed: &HashSet<String>) -> Vec<String> {
        let mut removed = self.removed.lock().unwrap_or_else(|e| e.into_inner());
        let mut pruned: Vec<String> = removed
            .iter()
            .filter(|id| !listed.contains(*id) && !self.store.has_manifest(id))
            .cloned()
            .collect();
        if pruned.is_empty() {
            return pruned;
        }
        for id in &pruned {
            removed.remove(id);
        }
        if let Err(error) = save_removed(&self.removed_file, &removed) {
            // Kept on disk: forgotten next time instead.
            tracing::warn!(%error, "removed sources not pruned");
            removed.extend(pruned.drain(..));
        }
        pruned.sort();
        pruned
    }

    /// The sources removed from the Library, in this run and before: the
    /// Library drops them from a saved list that still names them, and their
    /// index runs are refused ([`IndexingState::mark_removed`]).
    pub fn removed_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .removed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// Drops what the manifest of `source_id` records about the file `key` (an
    /// index identity), so the source's next sync reads it as new. Waits for a
    /// running sync, which would otherwise write its own copy back. Returns
    /// whether the file was recorded.
    pub async fn forget_file(&self, source_id: &str, key: &str) -> bool {
        let _turn = self.running.lock().await;
        match self.store.forget_file(source_id, key) {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(source_id, error = %e, "folder manifest not updated");
                false
            }
        }
    }

    /// Drops the manifest of `source_id`, a source the Library no longer
    /// lists. Waits for a running sync, which would otherwise write it back;
    /// a source listed and watched again meanwhile keeps its manifest.
    pub async fn forget_source(&self, source_id: &str) {
        let _turn = self.running.lock().await;
        if self.watched_ids().contains(source_id) {
            return;
        }
        self.store.forget(source_id);
    }

    /// Removes the Library source `source_id` (the folder or file `path`):
    /// the one way a source is removed, from the Library and by the agent.
    ///
    /// In order: the source stops being watched (its sync stops before its
    /// next file) and is never watched again; its index runs stop before
    /// their next file and are waited for; then, in the syncs' turn, its
    /// chunks are deleted ([`RAGEngine::delete_library_source`], by source
    /// id, never by path prefix) and its manifest with them. Nothing of the
    /// source runs after that, so nothing writes its chunks or its manifest
    /// back. The removal is recorded on disk first, so a saved Library list
    /// that still names the source never brings it back, in this run or the
    /// next; a removal that cannot be recorded does not start. Returns the
    /// chunks deleted. When the deletion fails the source may be watched and
    /// indexed again; its manifest is kept.
    pub async fn remove_source(
        &self,
        rag: &RwLock<RAGEngine>,
        indexing: &IndexingState,
        source_id: &str,
        path: Option<&str>,
    ) -> Result<usize, String> {
        {
            let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
            let mut removed = self.removed.lock().unwrap_or_else(|e| e.into_inner());
            if removed.insert(source_id.to_string()) {
                if let Err(error) = save_removed(&self.removed_file, &removed) {
                    removed.remove(source_id);
                    return Err(format!("The removal could not be recorded: {error}"));
                }
            }
            // Dropping the watcher ends the source's sync task.
            watched.remove(source_id);
        }
        indexing.stop_source(source_id).await;
        let _turn = self.running.lock().await;
        let deleted = rag
            .write()
            .await
            .delete_library_source(source_id, path)
            .await
            .map_err(|e| format!("Failed to delete the documents of the source: {e:#}"));
        match deleted {
            Ok(deleted) => {
                self.store.forget(source_id);
                Ok(deleted)
            }
            Err(error) => {
                {
                    let mut removed = self.removed.lock().unwrap_or_else(|e| e.into_inner());
                    removed.remove(source_id);
                    if let Err(e) = save_removed(&self.removed_file, &removed) {
                        tracing::warn!(source_id, error = %e, "removed sources not updated");
                    }
                }
                indexing.restore_source(source_id);
                Err(error)
            }
        }
    }

    /// Where the folder manifests are kept.
    pub fn store(&self) -> &ManifestStore {
        &self.store
    }

    /// The folder sources kept in sync: the Library's listed folders whose
    /// index is complete.
    pub fn watched_ids(&self) -> std::collections::BTreeSet<String> {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// The folder the Library lists for `source_id`, while it is kept in sync.
    pub fn library_path(&self, source_id: &str) -> Option<String> {
        self.watched_source(source_id).map(|source| source.path)
    }

    /// The watched folder of `source_id`.
    fn watched_source(&self, source_id: &str) -> Option<FolderSourceRef> {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(source_id)
            .map(|w| FolderSourceRef {
                id: source_id.to_string(),
                path: w.path.clone(),
            })
    }

    /// Watches `source` without a watcher task (tests).
    #[cfg(test)]
    pub(crate) fn watch_quietly(&self, source: &FolderSourceRef) {
        let watcher =
            notify::recommended_watcher(|_: notify::Result<Event>| {}).expect("a file watcher");
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                source.id.clone(),
                Watched {
                    path: source.path.clone(),
                    _watcher: watcher,
                },
            );
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
            if !report.unreadable.is_empty() {
                parts.push(format!(
                    "{} folders or files could not be opened",
                    report.unreadable.len()
                ));
            }
            let status = if report.failures.is_empty() && report.unreadable.is_empty() {
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

/// Why a sync's card says something could not be read: the folders and
/// files the sync could not open, whose index entries it left as they were.
fn unreadable_message(unreadable: &[FileFailure]) -> Option<String> {
    let first = unreadable.first()?;
    let reason = if first.reason.is_empty() {
        String::new()
    } else {
        format!(" ({})", first.reason)
    };
    Some(match unreadable.len() {
        1 => format!(
            "{}{reason} could not be read; what the index holds of it was left as it was",
            first.file
        ),
        n => format!(
            "{n} folders or files could not be read, among them {}{reason}; what the index holds of them was left as it was",
            first.file
        ),
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
    // A source removed or no longer listed meanwhile stops before its next file.
    let result = sync_folder(&source.path, &source.id, &state.store, &rag, || {
        !state.is_watched(source)
    })
    .await;
    if let Err(e) = &result {
        tracing::warn!(source_id = %source.id, error = %e, "folder sync failed");
    }
    if let Some(item) = inbox_item(source, &result) {
        crate::inbox_commands::post(app, item).await;
    }
    let (report, error) = match result {
        Ok(report) => {
            let error = unreadable_message(&report.unreadable);
            (Some(report), error)
        }
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

/// Indexes the single-file source `source` again if it was indexed in an older
/// index format. Takes its turn with the folder syncs.
async fn upgrade_file_once(app: &AppHandle, source: &FolderSourceRef) {
    app.state::<BackgroundState>().wait_until_resumed().await;
    let state = app.state::<FolderSyncState>();
    let _turn = state.running.lock().await;
    let rag = app.state::<RagState>().rag.clone();
    if let Err(e) = upgrade_file_source(&source.path, &source.id, &rag).await {
        tracing::warn!(source_id = %source.id, error = %e, "file source not indexed again");
    }
}

/// Brings the documents of an older index format that none of `owned` (the
/// Library's source ids) owns up to date, once the files an earlier build
/// filed under one owner for every chat are the main Ask's. Takes its turn
/// with the syncs.
async fn upgrade_unowned_once(app: &AppHandle, owned: std::collections::BTreeSet<String>) {
    app.state::<BackgroundState>().wait_until_resumed().await;
    let state = app.state::<FolderSyncState>();
    let _turn = state.running.lock().await;
    let rag = app.state::<RagState>().rag.clone();
    // Files an earlier build filed under one owner for every chat become the
    // main Ask's, before they are brought up to date with the others.
    if let Err(e) = rag.write().await.adopt_legacy_chat_uploads().await {
        tracing::warn!(error = %format!("{e:#}"), "files added in chats not given to the main Ask");
    }
    if let Err(e) = upgrade_unowned(&rag, &owned).await {
        tracing::warn!(error = %e, "documents outside the Library not brought up to date");
    }
}

/// Scans indexed before the OCR engine was recorded with their chunks
/// ([`shodh_rag::folder_sync::legacy_ocr_scans`]), looked for in turn with the
/// syncs and while background work is not paused: the look opens the PDFs
/// that may be scans. The turn is released before the scans are read again.
pub async fn legacy_ocr_scans_in_turn(
    app: &AppHandle,
) -> Result<Vec<shodh_rag::folder_sync::LegacyScan>, String> {
    app.state::<BackgroundState>().wait_until_resumed().await;
    let state = app.state::<FolderSyncState>();
    let _turn = state.running.lock().await;
    let rag = app.state::<RagState>().rag.clone();
    shodh_rag::folder_sync::legacy_ocr_scans(&rag).await
}

/// Syncs the folder source `source_id` now, if it is watched (for files whose
/// manifest entries were dropped with [`FolderSyncState::forget_file`]).
pub async fn sync_source_now(app: &AppHandle, source_id: &str) {
    let source = app.state::<FolderSyncState>().watched_source(source_id);
    if let Some(source) = source {
        sync_once(app, &source).await;
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

/// The source ids a saved Library list (`library_sources` of the UI state)
/// names; `None` when it is not a list of sources.
pub fn listed_source_ids(saved: &serde_json::Value) -> Option<HashSet<String>> {
    let entries = saved.as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|entry| entry.get("id")?.as_str().map(str::to_string))
            .collect(),
    )
}

/// The sources removed from the Library (in this run or before), for the
/// Library to drop from a saved list that still names them.
#[tauri::command]
pub async fn removed_library_sources(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(app.state::<FolderSyncState>().removed_ids())
}

/// The Library's folder sources: watch and sync these, stop watching the
/// rest (a removed source's manifest is deleted). Paths that are single
/// files are not watched; each is brought to the current index format.
#[tauri::command]
pub async fn sync_folder_sources(
    app: AppHandle,
    sources: Vec<FolderSourceRef>,
) -> Result<(), String> {
    let state = app.state::<FolderSyncState>();
    if !state
        .unowned_checked
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        let owned = sources.iter().map(|s| s.id.clone()).collect();
        let app = app.clone();
        tauri::async_runtime::spawn(async move { upgrade_unowned_once(&app, owned).await });
    }
    let mut watched = state.watched.lock().unwrap_or_else(|e| e.into_inner());
    let removed = state.removed.lock().unwrap_or_else(|e| e.into_inner());
    let stale: Vec<String> = watched
        .iter()
        .filter(|(id, w)| !sources.iter().any(|s| &s.id == *id && s.path == w.path))
        .map(|(id, _)| id.clone())
        .collect();
    let mut unlisted = Vec::new();
    for id in stale {
        watched.remove(&id);
        if !sources.iter().any(|s| s.id == id) {
            unlisted.push(id);
        }
    }
    for source in sources {
        // Removed: a list sent before the Library dropped it.
        if removed.contains(&source.id) {
            continue;
        }
        if Path::new(&source.path).is_file() {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { upgrade_file_once(&app, &source).await });
            continue;
        }
        if watched.contains_key(&source.id) {
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
    drop(removed);
    drop(watched);
    // In the syncs' turn: a sync still running would write the manifest back.
    for id in unlisted {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            app.state::<FolderSyncState>().forget_source(&id).await;
        });
    }
    // Once the folders are watched, so scans in them can go to their syncs.
    if !state
        .scans_checked
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        let rag = app.state::<RagState>().rag.clone();
        tauri::async_runtime::spawn(crate::scan_reader::reread_on_start(app.clone(), rag));
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
                unreadable: Vec::new(),
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

    #[test]
    fn an_unreadable_folder_is_named_on_the_card() {
        assert_eq!(unreadable_message(&[]), None);
        let private = FileFailure {
            file: "C:/papers/private".into(),
            reason: "Access is denied.".into(),
        };
        let one = unreadable_message(std::slice::from_ref(&private)).unwrap();
        assert!(
            one.starts_with("C:/papers/private (Access is denied.) could not be read"),
            "{one}"
        );
        let two = unreadable_message(&[
            private.clone(),
            FileFailure {
                file: "C:/papers/old".into(),
                reason: String::new(),
            },
        ])
        .unwrap();
        assert!(
            two.starts_with("2 folders or files could not be read, among them C:/papers/private"),
            "{two}"
        );
    }

    fn file_count(rows: &[shodh_rag::storage::DocumentSourceRow], source_id: &str) -> usize {
        rows.iter().filter(|row| row.space_id == source_id).count()
    }

    /// A source removed while its folder syncs: the sync stops before its
    /// next file, and once the removal is done nothing of the source is left
    /// (no chunks, no manifest, no watch) and nothing writes any back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_source_removed_during_its_sync_stops_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let rag =
            RwLock::new(crate::agent_tools::testing::engine_with_search_models(dir.path()).await);
        let state = FolderSyncState::new(dir.path().join("sync"));
        let indexing = IndexingState::default();
        let folder = dir.path().join("papers");
        std::fs::create_dir_all(&folder).unwrap();
        for (name, text) in [
            (
                "a.txt",
                "The lease notice period is sixty days for either party.",
            ),
            (
                "b.txt",
                "Visitors park on the street next to the courtyard.",
            ),
            ("c.txt", "Quiet hours begin at ten in the evening."),
        ] {
            std::fs::write(folder.join(name), text).unwrap();
        }
        let source = FolderSourceRef {
            id: "folder-papers".into(),
            path: folder.display().to_string(),
        };
        state.watch_quietly(&source);

        // The sync asks before each file; the removal starts after the first,
        // as `remove_source` begins: the source is no longer watched.
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let report = sync_folder(&source.path, &source.id, state.store(), &rag, || {
            if asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                state
                    .watched
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&source.id);
            }
            !state.is_watched(&source)
        })
        .await
        .unwrap();
        assert_eq!(
            report.indexed, 1,
            "stopped before the next file: {report:?}"
        );
        let rows = rag.read().await.document_sources().await.unwrap();
        assert_eq!(file_count(&rows, &source.id), 1);

        let deleted = state
            .remove_source(&rag, &indexing, &source.id, Some(source.path.as_str()))
            .await
            .unwrap();
        assert!(deleted > 0);
        let rows = rag.read().await.document_sources().await.unwrap();
        assert_eq!(file_count(&rows, &source.id), 0, "no chunks left");
        assert!(
            !dir.path().join("sync").join("folder-papers.json").exists(),
            "no manifest left"
        );
        assert!(!state.watched_ids().contains(&source.id));
        // A list that still names it (sent before the Library dropped it)
        // does not bring it back.
        assert!(state
            .removed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&source.id));
        // Its index runs cannot start again.
        assert!(indexing.begin_run(&source.id).is_none());
    }

    /// A removal is kept on disk: after a restart, a saved Library list that
    /// still names the source does not bring it back. It is not watched, its
    /// index runs are refused, and the Library is told it was removed.
    #[tokio::test]
    async fn a_removal_outlives_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let rag =
            RwLock::new(crate::agent_tools::testing::engine_with_search_models(dir.path()).await);
        {
            let state = FolderSyncState::new(dir.path().join("sync"));
            state
                .remove_source(&rag, &IndexingState::default(), "folder-papers", None)
                .await
                .unwrap();
        }
        // The next run.
        let state = FolderSyncState::new(dir.path().join("sync"));
        assert_eq!(state.removed_ids(), ["folder-papers"]);
        assert!(state
            .removed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains("folder-papers"));
        let indexing = IndexingState::default();
        for id in state.removed_ids() {
            indexing.mark_removed(&id);
        }
        assert!(
            indexing.begin_run("folder-papers").is_none(),
            "not indexed again"
        );
        assert!(indexing.begin_run("folder-other").is_some());
    }

    /// A removal record is forgotten once nothing can bring the source back:
    /// the saved Library list no longer names it and it has no manifest.
    #[tokio::test]
    async fn removal_records_are_pruned_once_nothing_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let rag =
            RwLock::new(crate::agent_tools::testing::engine_with_search_models(dir.path()).await);
        let state = FolderSyncState::new(dir.path().join("sync"));
        for id in ["gone", "still-listed", "cut-short"] {
            state
                .remove_source(&rag, &IndexingState::default(), id, None)
                .await
                .unwrap();
        }
        // A removal cut short: its manifest is still there.
        std::fs::write(
            dir.path().join("sync").join("cut-short.json"),
            r#"{"folder":"papers","files":{}}"#,
        )
        .unwrap();
        let saved =
            serde_json::json!([{ "id": "still-listed", "path": "C:/papers" }, { "path": "x" }]);
        let listed = listed_source_ids(&saved).unwrap();
        assert_eq!(listed_source_ids(&serde_json::json!({})), None);

        assert_eq!(state.prune_removed(&listed), ["gone"]);
        assert_eq!(state.removed_ids(), ["cut-short", "still-listed"]);
        // Kept on disk too.
        let next = FolderSyncState::new(dir.path().join("sync"));
        assert_eq!(next.removed_ids(), ["cut-short", "still-listed"]);
        assert!(next.prune_removed(&listed).is_empty());
    }

    /// The removal waits for the sync that holds the turn, so that sync
    /// cannot write the manifest back after the removal deleted it.
    #[tokio::test]
    async fn a_removal_waits_for_the_running_sync() {
        let dir = tempfile::tempdir().unwrap();
        let rag =
            RwLock::new(crate::agent_tools::testing::engine_with_search_models(dir.path()).await);
        let state = FolderSyncState::new(dir.path().join("sync"));
        let indexing = IndexingState::default();
        let manifest = dir.path().join("sync").join("folder-papers.json");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();

        let turn = state.running.lock().await;
        let removal = state.remove_source(&rag, &indexing, "folder-papers", None);
        tokio::pin!(removal);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut removal)
                .await
                .is_err(),
            "the removal cannot finish while a sync holds the turn"
        );
        // The sync saves its progress as it ends.
        std::fs::write(&manifest, r#"{"folder":"papers","files":{}}"#).unwrap();
        drop(turn);
        removal.await.unwrap();
        assert!(!manifest.exists(), "deleted after the sync, not before");
    }
}
