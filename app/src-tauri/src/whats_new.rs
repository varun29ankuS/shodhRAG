//! What's new: files new in the Library's watched folders, shown in Library → What's new
//! and told of by one Inbox item per folder and day (a `folder_watch` engine job).
//!
//! - **Rows first, then the item.** A folder sync reports the files it indexed for the
//!   first time ([`shodh_rag::folder_sync::SyncReport::added`]; a folder's first sync is a
//!   baseline and reports none). They are written as feed rows at once, and the folder's
//!   job is sent an event. Its run reads the rows no item has told of yet and posts (or
//!   updates) the folder's item of the day, then marks them: a crash between the two posts
//!   the same item again, never loses it, and bursts of syncs fold into the runs the
//!   engine has pending.
//! - **On by default.** A folder without a job shows its new files; the job is made with
//!   the first ones. Turning it off ([`WhatsNew::set_folder_watch`]) keeps the job,
//!   switched off, so nothing is recorded until it is turned on again.
//! - **In the app only.** Items are Info; nothing here moves the screen or raises a
//!   desktop notification.
//! - A feed that cannot be written is retried on the engine's ladder; after three failed
//!   runs in a row the folder's job is put on hold with an Attention item (its fix:
//!   Resume, which turns the folder's watch on again).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use shodh_rag::engine::clock::resolve_local;
use shodh_rag::engine::runner::{Engine, Handled, JobHandler};
use shodh_rag::engine::store::{
    CatchUp, CreatedBy, EngineStore, HoldReason, Job, JobKind, JobPatch, NewJob, Overlap, Run,
};
use shodh_rag::folder_sync::SyncReport;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use shodh_rag::rag_engine::normalize_source_path;
use shodh_rag::whats_new::store::{FeedItem, FeedKind, FeedStore, NewFeedItem};
use tauri::{AppHandle, Emitter, Manager};

use crate::file_watcher::{FolderSourceRef, FolderSyncState};

/// Emitted when What's new changed (rows found, seen, dismissed, a watch switched).
pub const WHATS_NEW_CHANGED_EVENT: &str = "whats-new-changed";
/// The Inbox id of a folder's item of the day: `<prefix><source id>:<yyyymmdd>`.
pub const FOLDER_DIGEST_PREFIX: &str = "whats_new:files:";
/// The Inbox id of a folder's job on hold: `<prefix><source id>`.
pub const FOLDER_HOLD_PREFIX: &str = "whats-new-hold:";
/// The error code of a run that could not read or write the feed.
pub const FEED_UNAVAILABLE: &str = "feed_unavailable";
/// Most rows the What's new section lists.
pub const MAX_LISTED: usize = 200;
/// File names a folder's item names before "and N more".
const NAMED_IN_DIGEST: usize = 3;

/// What What's new tells the app.
#[async_trait]
pub trait WhatsNewSink: Send + Sync {
    async fn post(&self, item: NewInboxItem);
    async fn resolve(&self, id: &str);
    async fn resolve_prefixed(&self, kind: InboxKind, prefix: &str);
    /// The feed or a folder's watch changed: the section reads it again.
    fn changed(&self);
}

/// The title of a folder's item: "1 new file in Papers", "3 new files in Papers".
pub fn digest_title(count: usize, folder: &str) -> String {
    if count == 1 {
        format!("1 new file in {folder}")
    } else {
        format!("{count} new files in {folder}")
    }
}

/// The detail of a folder's item: the first file names, then how many more.
fn digest_detail(names: &[String], count: usize) -> String {
    let shown: Vec<&str> = names
        .iter()
        .take(NAMED_IN_DIGEST)
        .map(String::as_str)
        .collect();
    let rest = count.saturating_sub(shown.len());
    if rest == 0 {
        shown.join(", ")
    } else {
        format!("{} and {rest} more", shown.join(", "))
    }
}

/// The last part of a folder's path, as the Library names it.
pub fn folder_name(folder: &str) -> String {
    Path::new(folder)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| folder.to_string())
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// The Library's What's new section, as a navigation target.
fn section_link() -> InboxLink {
    InboxLink::new(
        "library",
        Some(json!({ "kind": "library", "section": "whats_new" })),
    )
}

/// The Inbox id of a folder's item on the local day of `at`.
pub fn digest_item_id(source_id: &str, at: DateTime<Utc>) -> String {
    let day = at.with_timezone(&Local).format("%Y%m%d");
    format!("{FOLDER_DIGEST_PREFIX}{source_id}:{day}")
}

/// The start of the local day of `at`.
fn local_day_start(at: DateTime<Utc>) -> DateTime<Utc> {
    let day = at.with_timezone(&Local).date_naive();
    resolve_local(&Local, day.and_time(NaiveTime::MIN)).unwrap_or(at)
}

fn source_of(job: &Job) -> &str {
    &job.subject
}

fn folder_of(job: &Job) -> String {
    job.input
        .get("folder")
        .and_then(Value::as_str)
        .map(folder_name)
        .unwrap_or_else(|| job.title.clone())
}

/// One folder of the Library and whether its new files show in What's new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderWatchView {
    pub source_id: String,
    pub folder: String,
    pub on: bool,
    /// Its job is on hold after failing again and again.
    pub held: bool,
}

/// What the What's new section shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsNewView {
    pub items: Vec<FeedItem>,
    pub folders: Vec<FolderWatchView>,
}

struct Folders {
    engine: Arc<Engine>,
    feed: Arc<FeedStore>,
    sink: Arc<dyn WhatsNewSink>,
}

impl Folders {
    fn store(&self) -> &Arc<EngineStore> {
        self.engine.store()
    }

    /// Posts the folder's item of the day for what `job` has not told of yet.
    async fn post_digest(&self, job: &Job, run: &Run) -> Handled {
        let transient = |e: String| Handled::Transient {
            code: FEED_UNAVAILABLE.to_string(),
            detail: e,
        };
        let unposted = match self.feed.unposted(&job.id) {
            Ok(rows) => rows,
            Err(e) => return transient(e.to_string()),
        };
        if unposted.is_empty() {
            return Handled::Nothing;
        }
        let now = self.engine.now();
        let mut rows = match self.feed.of_job_since(&job.id, local_day_start(now)) {
            Ok(rows) => rows,
            Err(e) => return transient(e.to_string()),
        };
        // Rows a crash left from an earlier day are told of today.
        for row in &unposted {
            if !rows.iter().any(|r| r.id == row.id) {
                rows.push(row.clone());
            }
        }
        let more: usize = rows
            .iter()
            .filter_map(|r| r.data.get("more").and_then(Value::as_u64))
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .sum();
        let count = rows.len().saturating_add(more);
        let names: Vec<String> = rows.iter().map(|r| r.title.clone()).collect();
        let folder = folder_of(job);
        let title = digest_title(count, &folder);
        let day = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
        self.sink
            .post(NewInboxItem {
                id: digest_item_id(source_of(job), now),
                kind: InboxKind::WhatsNew,
                status: InboxStatus::Done,
                title: title.clone(),
                detail: Some(digest_detail(&names, count)),
                link: Some(section_link()),
                data: json!({
                    "jobId": job.id,
                    "sourceId": source_of(job),
                    "folder": folder,
                    "day": day,
                    "count": count,
                    "announce": false,
                }),
            })
            .await;
        // Posted first, marked after: a crash between the two posts the same item again.
        let ids: Vec<String> = unposted.iter().map(|r| r.id.clone()).collect();
        if let Err(e) = self.feed.mark_posted(&ids, &run.id) {
            return transient(e.to_string());
        }
        self.sink.changed();
        Handled::Succeeded {
            summary: title,
            scheduled_for: None,
        }
    }
}

#[async_trait]
impl JobHandler for Folders {
    fn kind(&self) -> JobKind {
        JobKind::FolderWatch
    }

    async fn run(&self, job: &Job, run: &Run) -> Handled {
        self.post_digest(job, run).await
    }

    /// A folder watch has no schedule, so it has no waiting decision to fold.
    async fn folded(&self, _job: &Job, _run: &Run) {}

    async fn held(&self, job: &Job, run: &Run) {
        let folder = folder_of(job);
        let reason = run
            .error_detail
            .clone()
            .unwrap_or_else(|| "the app database could not be written".to_string());
        self.sink
            .post(NewInboxItem {
                id: format!("{FOLDER_HOLD_PREFIX}{}", source_of(job)),
                kind: InboxKind::Automation,
                status: InboxStatus::NeedsYou,
                title: format!("New files in {folder} are not shown in What's new"),
                detail: Some(format!(
                    "Stopped after failing three times in a row ({reason}). Resume it once that is fixed."
                )),
                link: Some(section_link()),
                data: json!({
                    "jobId": job.id,
                    "sourceId": source_of(job),
                    "fix": "resume",
                    "resume": "folder_watch",
                }),
            })
            .await;
        self.sink.changed();
    }
}

/// What's new over the engine and the feed.
pub struct WhatsNew {
    folders: Arc<Folders>,
    handler: Arc<dyn JobHandler>,
}

impl std::fmt::Debug for WhatsNew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhatsNew").finish_non_exhaustive()
    }
}

impl WhatsNew {
    pub fn new(engine: Arc<Engine>, feed: Arc<FeedStore>, sink: Arc<dyn WhatsNewSink>) -> Self {
        let folders = Arc::new(Folders { engine, feed, sink });
        let handler: Arc<dyn JobHandler> = folders.clone();
        Self { folders, handler }
    }

    /// The handler of folder watches, for the engine's loop.
    pub fn handler(&self) -> Arc<dyn JobHandler> {
        self.handler.clone()
    }

    fn store(&self) -> &Arc<EngineStore> {
        self.folders.store()
    }

    fn job_of(&self, source_id: &str) -> Result<Option<Job>, String> {
        self.store()
            .job_by_subject(JobKind::FolderWatch, source_id)
            .map_err(|e| e.to_string())
    }

    fn create_job(&self, source_id: &str, folder: &str) -> Result<Job, String> {
        self.store()
            .create_job(
                &NewJob {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind: JobKind::FolderWatch,
                    subject: source_id.to_string(),
                    title: folder_name(folder),
                    schedule: None,
                    input: json!({ "sourceId": source_id, "folder": folder }),
                    overlap: Overlap::QueueOne,
                    catch_up: CatchUp::Skip,
                    max_attempts: 3,
                    created_by: CreatedBy::System,
                    provenance: json!({}),
                },
                self.folders.engine.now(),
            )
            .map_err(|e| e.to_string())
    }

    /// Whether the new files of `source_id` show in What's new (a folder without a job
    /// does: it is on by default).
    pub fn is_on(&self, source_id: &str) -> Result<bool, String> {
        Ok(self.job_of(source_id)?.is_none_or(|job| job.enabled))
    }

    /// A sync of the folder `folder` (source `source_id`) indexed `added` for the first
    /// time (and `added_more` beyond them). Records them, unless the folder's watch is
    /// off, and starts its run. Returns whether What's new shows the folder's new files
    /// (then its item stands for the sync's "indexed" one).
    pub async fn files_added(
        &self,
        source_id: &str,
        folder: &str,
        added: &[String],
        added_more: usize,
    ) -> Result<bool, String> {
        if added.is_empty() {
            return self.is_on(source_id);
        }
        let job = match self.job_of(source_id)? {
            Some(job) if !job.enabled => return Ok(false),
            Some(job) => job,
            None => self.create_job(source_id, folder)?,
        };
        let name = folder_name(folder);
        let last = added.len() - 1;
        let rows: Vec<NewFeedItem> = added
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let mut data = json!({ "sourceId": source_id, "path": path, "folder": name });
                if i == last && added_more > 0 {
                    data["more"] = json!(added_more);
                }
                NewFeedItem {
                    kind: FeedKind::File,
                    source_ref: normalize_source_path(Path::new(path)),
                    job_id: job.id.clone(),
                    title: file_name(path),
                    detail: Some(name.clone()),
                    link: json!({ "view": "library", "target": { "kind": "library", "section": "whats_new" } }),
                    data,
                }
            })
            .collect();
        let inserted = self
            .folders
            .feed
            .insert_if_absent(&rows, self.folders.engine.now())
            .map_err(|e| e.to_string())?;
        if !inserted.is_empty() {
            self.folders.sink.changed();
            self.folders
                .engine
                .event(&self.handler, &job.id)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(true)
    }

    /// The folder `source_id` left the Library: its job, its rows and its items go.
    pub async fn source_removed(&self, source_id: &str) -> Result<(), String> {
        if let Some(job) = self.job_of(source_id)? {
            self.folders
                .feed
                .delete_job(&job.id)
                .map_err(|e| e.to_string())?;
            self.store()
                .delete_job(&job.id)
                .map_err(|e| e.to_string())?;
        }
        self.folders
            .sink
            .resolve_prefixed(
                InboxKind::WhatsNew,
                &format!("{FOLDER_DIGEST_PREFIX}{source_id}:"),
            )
            .await;
        self.folders
            .sink
            .resolve(&format!("{FOLDER_HOLD_PREFIX}{source_id}"))
            .await;
        self.folders.sink.changed();
        Ok(())
    }

    /// Shows the new files of the folder `folder` (source `source_id`) in What's new, or
    /// stops. Turning it on resumes a job on hold and tells of what it had not.
    pub async fn set_folder_watch(
        &self,
        source_id: &str,
        folder: &str,
        on: bool,
    ) -> Result<FolderWatchView, String> {
        let now = self.folders.engine.now();
        let job = match self.job_of(source_id)? {
            Some(job) => job,
            None if on => {
                return Ok(FolderWatchView {
                    source_id: source_id.to_string(),
                    folder: folder_name(folder),
                    on: true,
                    held: false,
                })
            }
            None => self.create_job(source_id, folder)?,
        };
        let patch = JobPatch {
            enabled: Some(on),
            hold_reason: on.then_some(None::<HoldReason>),
            reset_failures: on,
            ..JobPatch::default()
        };
        let job = self
            .store()
            .update_job(&job.id, &patch, now)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "That folder's watch is gone.".to_string())?;
        if on {
            self.folders
                .sink
                .resolve(&format!("{FOLDER_HOLD_PREFIX}{source_id}"))
                .await;
            let pending = self
                .folders
                .feed
                .unposted(&job.id)
                .map_err(|e| e.to_string())?;
            if !pending.is_empty() {
                self.folders
                    .engine
                    .event(&self.handler, &job.id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        self.folders.sink.changed();
        Ok(FolderWatchView {
            source_id: source_id.to_string(),
            folder: folder_name(folder),
            on: job.enabled,
            held: job.hold_reason.is_some(),
        })
    }

    /// The watch of each folder `(source id, folder)`.
    pub fn folders(&self, sources: &[(String, String)]) -> Result<Vec<FolderWatchView>, String> {
        sources
            .iter()
            .map(|(source_id, folder)| {
                let job = self.job_of(source_id)?;
                Ok(FolderWatchView {
                    source_id: source_id.clone(),
                    folder: folder_name(folder),
                    on: job.as_ref().is_none_or(|j| j.enabled),
                    held: job.is_some_and(|j| j.hold_reason.is_some()),
                })
            })
            .collect()
    }

    /// The rows the section lists: newest first, without those dismissed and without
    /// files that are no longer on disk.
    pub fn items(&self) -> Result<Vec<FeedItem>, String> {
        let rows = self
            .folders
            .feed
            .list(MAX_LISTED)
            .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .filter(|row| {
                row.kind != FeedKind::File
                    || row
                        .data
                        .get("path")
                        .and_then(Value::as_str)
                        .is_none_or(|path| Path::new(path).exists())
            })
            .collect())
    }

    /// Marks `ids` seen; returns how many were not seen before.
    pub fn mark_seen(&self, ids: &[String]) -> Result<usize, String> {
        let seen = self
            .folders
            .feed
            .mark_seen(ids, self.folders.engine.now())
            .map_err(|e| e.to_string())?;
        if seen > 0 {
            self.folders.sink.changed();
        }
        Ok(seen)
    }

    /// Dismisses the row `id`, or brings it back (`restore`).
    pub fn dismiss(&self, id: &str, restore: bool) -> Result<bool, String> {
        let found = self
            .folders
            .feed
            .set_dismissed(id, !restore, self.folders.engine.now())
            .map_err(|e| e.to_string())?;
        if found {
            self.folders.sink.changed();
        }
        Ok(found)
    }

    /// The launch: prunes rows past their retention and tells of rows a crash left
    /// untold.
    pub async fn launch(&self) {
        if let Err(e) = self.folders.feed.prune(self.folders.engine.now()) {
            tracing::warn!(error = %e, "What's new could not be pruned");
        }
        let jobs = match self.store().jobs(JobKind::FolderWatch) {
            Ok(jobs) => jobs,
            Err(e) => {
                tracing::warn!(error = %e, "What's new: folder watches could not be read");
                return;
            }
        };
        for job in jobs {
            let pending = self.folders.feed.unposted(&job.id).unwrap_or_default();
            if pending.is_empty() {
                continue;
            }
            if let Err(e) = self.folders.engine.event(&self.handler, &job.id).await {
                tracing::warn!(job = %job.id, error = %e, "What's new: a folder's item could not be posted");
            }
        }
    }
}

// Commands and the hooks the folder sync calls.

/// What What's new tells the app's Inbox and views.
pub(crate) struct TauriSink {
    pub(crate) app: AppHandle,
}

#[async_trait]
impl WhatsNewSink for TauriSink {
    async fn post(&self, item: NewInboxItem) {
        crate::inbox_commands::post(&self.app, item).await;
    }

    async fn resolve(&self, id: &str) {
        crate::inbox_commands::resolve(&self.app, id.to_string()).await;
    }

    async fn resolve_prefixed(&self, kind: InboxKind, prefix: &str) {
        crate::inbox_commands::resolve_prefixed(&self.app, kind, prefix.to_string()).await;
    }

    fn changed(&self) {
        if let Err(e) = self.app.emit(WHATS_NEW_CHANGED_EVENT, ()) {
            tracing::warn!(error = %e, "could not emit {}", WHATS_NEW_CHANGED_EVENT);
        }
    }
}

/// A sync of `source` ended with `report`: its new files go to What's new. Returns
/// whether What's new shows them (then the sync's "indexed" item is not posted).
pub async fn sync_reported(app: &AppHandle, source: &FolderSourceRef, report: &SyncReport) -> bool {
    let Some(whats_new) = crate::engine_commands::whats_new(app).await else {
        return false;
    };
    match whats_new
        .files_added(&source.id, &source.path, &report.added, report.added_more)
        .await
    {
        Ok(shown) => shown,
        Err(e) => {
            tracing::warn!(source_id = %source.id, error = %e, "new files not added to What's new");
            false
        }
    }
}

/// The Library source `source_id` was removed.
pub async fn source_removed(app: &AppHandle, source_id: &str) {
    let Some(whats_new) = crate::engine_commands::whats_new(app).await else {
        return;
    };
    if let Err(e) = whats_new.source_removed(source_id).await {
        tracing::warn!(source_id, error = %e, "What's new of a removed source not cleared");
    }
}

/// The Library's watched folders, as `(source id, folder)`.
pub(crate) fn watched_folders(state: &FolderSyncState) -> Vec<(String, String)> {
    state
        .watched_ids()
        .into_iter()
        .filter_map(|id| state.library_path(&id).map(|path| (id, path)))
        .collect()
}

/// Library → What's new: the rows and each watched folder's switch.
#[tauri::command]
pub async fn whats_new_list(app: AppHandle) -> Result<WhatsNewView, String> {
    let whats_new = crate::engine_commands::whats_new_for_command(&app).await?;
    let sources = watched_folders(&app.state::<FolderSyncState>());
    tokio::task::spawn_blocking(move || {
        Ok(WhatsNewView {
            items: whats_new.items()?,
            folders: whats_new.folders(&sources)?,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The user saw these rows (the section was on screen).
#[tauri::command]
pub async fn whats_new_mark_seen(app: AppHandle, ids: Vec<String>) -> Result<usize, String> {
    let whats_new = crate::engine_commands::whats_new_for_command(&app).await?;
    tokio::task::spawn_blocking(move || whats_new.mark_seen(&ids))
        .await
        .map_err(|e| e.to_string())?
}

/// Dismisses a row, or brings it back (`restore`, the Undo).
#[tauri::command]
pub async fn whats_new_dismiss(app: AppHandle, id: String, restore: bool) -> Result<bool, String> {
    let whats_new = crate::engine_commands::whats_new_for_command(&app).await?;
    tokio::task::spawn_blocking(move || whats_new.dismiss(&id, restore))
        .await
        .map_err(|e| e.to_string())?
}

/// Shows the new files of a watched folder in What's new, or stops.
#[tauri::command]
pub async fn folder_watch_set(
    app: AppHandle,
    source_id: String,
    on: bool,
) -> Result<FolderWatchView, String> {
    let folder = app
        .state::<FolderSyncState>()
        .library_path(&source_id)
        .ok_or_else(|| {
            "That folder is not in the Library, or is not kept in sync now.".to_string()
        })?;
    let whats_new = crate::engine_commands::whats_new_for_command(&app).await?;
    whats_new.set_folder_watch(&source_id, &folder, on).await
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A sink that tells nothing.
    struct QuietSink;

    #[async_trait]
    impl WhatsNewSink for QuietSink {
        async fn post(&self, _item: NewInboxItem) {}
        async fn resolve(&self, _id: &str) {}
        async fn resolve_prefixed(&self, _kind: InboxKind, _prefix: &str) {}
        fn changed(&self) {}
    }

    /// What's new over the `shodh.db` of `dir`, on `clock`.
    pub fn whats_new(dir: &Path, clock: Arc<dyn shodh_rag::engine::clock::Clock>) -> WhatsNew {
        let db = dir.join("shodh.db");
        let store = Arc::new(EngineStore::open(&db, None).unwrap());
        let feed = Arc::new(FeedStore::open(&db, None).unwrap());
        let engine = Arc::new(Engine::with_clock(store, clock, "agent-whats-new-test"));
        WhatsNew::new(engine, feed, Arc::new(QuietSink))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::engine::clock::Clock;
    use shodh_rag::engine::store::RunState;
    use std::sync::Mutex;

    struct TestClock(Mutex<DateTime<Utc>>);

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    #[derive(Default)]
    struct Sink {
        posted: Mutex<Vec<NewInboxItem>>,
        resolved: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl WhatsNewSink for Sink {
        async fn post(&self, item: NewInboxItem) {
            self.posted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(item);
        }
        async fn resolve(&self, id: &str) {
            self.resolved
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(id.to_string());
        }
        async fn resolve_prefixed(&self, _kind: InboxKind, prefix: &str) {
            self.resolved
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("{prefix}*"));
        }
        fn changed(&self) {}
    }

    struct Env {
        dir: tempfile::TempDir,
        clock: Arc<TestClock>,
        sink: Arc<Sink>,
        feed: Arc<FeedStore>,
        store: Arc<EngineStore>,
        whats_new: WhatsNew,
    }

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        let naive = chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap();
        resolve_local(&Local, naive).unwrap()
    }

    fn env(now: DateTime<Utc>) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("shodh.db");
        let store = Arc::new(EngineStore::open(&db, None).unwrap());
        let feed = Arc::new(FeedStore::open(&db, None).unwrap());
        let clock = Arc::new(TestClock(Mutex::new(now)));
        let engine = Arc::new(Engine::with_clock(
            store.clone(),
            clock.clone(),
            "whats-new-test",
        ));
        let sink = Arc::new(Sink::default());
        let whats_new = WhatsNew::new(engine, feed.clone(), sink.clone());
        Env {
            dir,
            clock,
            sink,
            feed,
            store,
            whats_new,
        }
    }

    impl Env {
        fn at(&self, now: DateTime<Utc>) {
            *self.clock.0.lock().unwrap_or_else(|e| e.into_inner()) = now;
        }

        /// A file in the folder, on disk.
        fn file(&self, name: &str) -> String {
            let folder = self.dir.path().join("Research Papers");
            std::fs::create_dir_all(&folder).unwrap();
            let path = folder.join(name);
            std::fs::write(&path, "x").unwrap();
            path.display().to_string()
        }

        fn folder(&self) -> String {
            self.dir
                .path()
                .join("Research Papers")
                .display()
                .to_string()
        }

        fn posted(&self) -> Vec<NewInboxItem> {
            self.sink
                .posted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        fn job(&self) -> Job {
            self.store
                .job_by_subject(JobKind::FolderWatch, "s1")
                .unwrap()
                .expect("a folder watch")
        }

        fn runs(&self) -> Vec<Run> {
            self.store.runs(&self.job().id, 50).unwrap()
        }
    }

    #[tokio::test]
    async fn added_files_become_rows_and_one_digest() {
        let e = env(local(2026, 11, 23, 10, 41));
        let files = [
            e.file("kv-cache-survey.pdf"),
            e.file("b.pdf"),
            e.file("c.pdf"),
        ];
        let shown = e
            .whats_new
            .files_added("s1", &e.folder(), &files, 0)
            .await
            .unwrap();
        assert!(shown);
        assert_eq!(e.whats_new.items().unwrap().len(), 3);
        let posted = e.posted();
        assert_eq!(posted.len(), 1, "one item per run");
        let item = &posted[0];
        assert_eq!(item.id, digest_item_id("s1", local(2026, 11, 23, 10, 41)));
        assert!(item.id.starts_with("whats_new:files:s1:2026112"));
        assert_eq!(item.kind, InboxKind::WhatsNew);
        assert_eq!(item.status, InboxStatus::Done);
        assert_eq!(item.title, "3 new files in Research Papers");
        assert_eq!(
            item.detail.as_deref(),
            Some("kv-cache-survey.pdf, b.pdf, c.pdf")
        );
        assert_eq!(item.data["count"], 3);
        let runs = e.runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Succeeded);
        assert!(e.feed.unposted(&e.job().id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn two_syncs_on_one_day_update_one_digest() {
        let e = env(local(2026, 11, 23, 9, 0));
        let first = [e.file("a.pdf"), e.file("b.pdf")];
        e.whats_new
            .files_added("s1", &e.folder(), &first, 0)
            .await
            .unwrap();
        e.at(local(2026, 11, 23, 15, 0));
        let second = [
            e.file("c.pdf"),
            e.file("d.pdf"),
            e.file("e.pdf"),
            e.file("f.pdf"),
        ];
        e.whats_new
            .files_added("s1", &e.folder(), &second, 0)
            .await
            .unwrap();
        let posted = e.posted();
        assert_eq!(posted.len(), 2, "one post per run");
        assert_eq!(
            posted[0].id, posted[1].id,
            "the day's item is updated, not repeated"
        );
        assert_eq!(posted[1].title, "6 new files in Research Papers");
        assert_eq!(
            posted[1].detail.as_deref(),
            Some("a.pdf, b.pdf, c.pdf and 3 more")
        );
        assert_eq!(e.runs().len(), 2);
        // The next day starts a new item.
        e.at(local(2026, 11, 24, 9, 0));
        e.whats_new
            .files_added("s1", &e.folder(), &[e.file("g.pdf")], 0)
            .await
            .unwrap();
        let next = e.posted().pop().unwrap();
        assert_ne!(next.id, posted[0].id);
        assert_eq!(next.title, "1 new file in Research Papers");
    }

    #[tokio::test]
    async fn a_second_report_of_the_same_file_adds_nothing() {
        let e = env(local(2026, 11, 23, 9, 0));
        let files = [e.file("a.pdf")];
        e.whats_new
            .files_added("s1", &e.folder(), &files, 0)
            .await
            .unwrap();
        e.whats_new
            .files_added("s1", &e.folder(), &files, 0)
            .await
            .unwrap();
        assert_eq!(e.whats_new.items().unwrap().len(), 1);
        assert_eq!(e.posted().len(), 1);
        assert_eq!(e.runs().len(), 1, "no run row for nothing new");
    }

    #[tokio::test]
    async fn files_beyond_the_cap_are_counted() {
        let e = env(local(2026, 11, 23, 9, 0));
        let files = [e.file("a.pdf"), e.file("b.pdf")];
        e.whats_new
            .files_added("s1", &e.folder(), &files, 36)
            .await
            .unwrap();
        assert_eq!(e.posted()[0].title, "38 new files in Research Papers");
        assert_eq!(
            e.posted()[0].detail.as_deref(),
            Some("a.pdf, b.pdf and 36 more")
        );
    }

    #[tokio::test]
    async fn a_crash_between_post_and_mark_reposts_the_same_digest() {
        let e = env(local(2026, 11, 23, 9, 0));
        // Rows recorded, then Shodh stopped before the item was posted and marked.
        let job = e.whats_new.create_job("s1", &e.folder()).unwrap();
        e.feed
            .insert_if_absent(
                &[NewFeedItem {
                    kind: FeedKind::File,
                    source_ref: "a".into(),
                    job_id: job.id.clone(),
                    title: "a.pdf".into(),
                    detail: None,
                    link: json!({}),
                    data: json!({}),
                }],
                e.clock.now(),
            )
            .unwrap();
        e.whats_new.launch().await;
        e.whats_new.launch().await;
        let posted = e.posted();
        assert_eq!(posted.len(), 1, "told of once, then marked");
        assert_eq!(posted[0].title, "1 new file in Research Papers");
    }

    #[tokio::test]
    async fn turning_a_folder_off_records_nothing() {
        let e = env(local(2026, 11, 23, 9, 0));
        assert!(e.whats_new.is_on("s1").unwrap(), "on by default");
        let view = e
            .whats_new
            .set_folder_watch("s1", &e.folder(), false)
            .await
            .unwrap();
        assert!(!view.on);
        let shown = e
            .whats_new
            .files_added("s1", &e.folder(), &[e.file("a.pdf")], 0)
            .await
            .unwrap();
        assert!(!shown);
        assert!(e.whats_new.items().unwrap().is_empty());
        assert!(e.posted().is_empty());
        // On again: later files show.
        assert!(
            e.whats_new
                .set_folder_watch("s1", &e.folder(), true)
                .await
                .unwrap()
                .on
        );
        e.whats_new
            .files_added("s1", &e.folder(), &[e.file("b.pdf")], 0)
            .await
            .unwrap();
        assert_eq!(e.whats_new.items().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn resuming_a_held_folder_clears_the_hold_and_tells_what_waited() {
        let e = env(local(2026, 11, 23, 9, 0));
        let job = e.whats_new.create_job("s1", &e.folder()).unwrap();
        e.store
            .update_job(
                &job.id,
                &JobPatch {
                    hold_reason: Some(Some(HoldReason::Failing)),
                    ..JobPatch::default()
                },
                e.clock.now(),
            )
            .unwrap();
        e.whats_new
            .files_added("s1", &e.folder(), &[e.file("a.pdf")], 0)
            .await
            .unwrap();
        assert!(e.posted().is_empty(), "a job on hold does not run");
        let view = e
            .whats_new
            .set_folder_watch("s1", &e.folder(), true)
            .await
            .unwrap();
        assert!(!view.held);
        assert_eq!(e.posted().len(), 1);
        assert!(e
            .sink
            .resolved
            .lock()
            .unwrap()
            .contains(&format!("{FOLDER_HOLD_PREFIX}s1")));
    }

    #[tokio::test]
    async fn removing_a_source_deletes_its_job_rows_and_items() {
        let e = env(local(2026, 11, 23, 9, 0));
        e.whats_new
            .files_added("s1", &e.folder(), &[e.file("a.pdf")], 0)
            .await
            .unwrap();
        e.whats_new.source_removed("s1").await.unwrap();
        assert!(e
            .store
            .job_by_subject(JobKind::FolderWatch, "s1")
            .unwrap()
            .is_none());
        assert!(e.whats_new.items().unwrap().is_empty());
        let resolved = e.sink.resolved.lock().unwrap().clone();
        assert!(resolved.contains(&format!("{FOLDER_DIGEST_PREFIX}s1:*")));
        assert!(resolved.contains(&format!("{FOLDER_HOLD_PREFIX}s1")));
    }

    #[tokio::test]
    async fn the_list_hides_dismissed_rows_and_missing_files() {
        let e = env(local(2026, 11, 23, 9, 0));
        let kept = e.file("kept.pdf");
        let gone = e.file("gone.pdf");
        let dismissed = e.file("dismissed.pdf");
        e.whats_new
            .files_added("s1", &e.folder(), &[kept, gone.clone(), dismissed], 0)
            .await
            .unwrap();
        std::fs::remove_file(&gone).unwrap();
        let id = e
            .whats_new
            .items()
            .unwrap()
            .into_iter()
            .find(|r| r.title == "dismissed.pdf")
            .unwrap()
            .id;
        assert!(e.whats_new.dismiss(&id, false).unwrap());
        let titles: Vec<String> = e
            .whats_new
            .items()
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(titles, ["kept.pdf"]);
        // Undo.
        assert!(e.whats_new.dismiss(&id, true).unwrap());
        assert_eq!(e.whats_new.items().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn mark_seen_sets_seen_once() {
        let e = env(local(2026, 11, 23, 9, 0));
        e.whats_new
            .files_added("s1", &e.folder(), &[e.file("a.pdf")], 0)
            .await
            .unwrap();
        let ids: Vec<String> = e
            .whats_new
            .items()
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(e.whats_new.mark_seen(&ids).unwrap(), 1);
        assert_eq!(e.whats_new.mark_seen(&ids).unwrap(), 0);
        assert!(e.whats_new.items().unwrap()[0].seen_at.is_some());
    }

    #[tokio::test]
    async fn folders_report_their_switch() {
        let e = env(local(2026, 11, 23, 9, 0));
        e.whats_new
            .set_folder_watch("s2", "C:/Notes", false)
            .await
            .unwrap();
        let views = e
            .whats_new
            .folders(&[("s1".into(), e.folder()), ("s2".into(), "C:/Notes".into())])
            .unwrap();
        assert_eq!(
            views
                .iter()
                .map(|v| (v.folder.as_str(), v.on))
                .collect::<Vec<_>>(),
            [("Research Papers", true), ("Notes", false)]
        );
    }

    #[test]
    fn digest_titles_and_details_read_plainly() {
        assert_eq!(digest_title(1, "Papers"), "1 new file in Papers");
        assert_eq!(digest_title(3, "Papers"), "3 new files in Papers");
        let names: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(digest_detail(&names[..2], 2), "a, b");
        assert_eq!(digest_detail(&names, 4), "a, b, c and 1 more");
        assert_eq!(digest_detail(&names[..1], 40), "a and 39 more");
    }
}
