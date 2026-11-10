//! Recall in the app: the "AI coding sessions" source in the Library, its background
//! sync, and the commands the Library card, the receipt viewer and the agent's recall
//! tools use ([`shodh_rag::recall`]).
//!
//! - **Off until the user turns it on**, then only the tools and the projects they
//!   chose. Nothing is opened or read before that: the index opens on first use.
//! - **Read in place**: transcripts stay where the tools keep them; Recall's own folder
//!   (`<app data>/recall`) holds only its index, catalog and settings.
//! - **How it runs**: a sync is an indexing run under the source [`RECALL_SOURCE_ID`], so
//!   the status strip shows it and its Pause and Stop hold and stop it. It runs when the
//!   settings change and every [`SYNC_EVERY`]; a session is indexed once its tool has not
//!   written to it for the idle time the user set.
//! - **Never in Slack or Telegram answers**: their answers search the document library;
//!   Recall is a library of its own, which only the in-app agent's recall tools and this
//!   card read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shodh_rag::compute::provider::ComputePreference;
use shodh_rag::indexing::IndexingState;
use shodh_rag::recall::catalog::FileState;
use shodh_rag::recall::discover::{self, Roots};
use shodh_rag::recall::index::{RecallIndex, Stopped, SyncControl, SyncProgress, SyncReport};
use shodh_rag::recall::query::{self, Opened, RecallQuery, ScopeAnswer, SearchAnswer};
use shodh_rag::recall::settings::RecallSettings;
use shodh_rag::recall::Tool;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;

/// Recall's sync runs as an indexing run of this source.
pub const RECALL_SOURCE_ID: &str = "recall";
/// Emitted when Recall's state or progress changes.
pub const RECALL_EVENT: &str = "recall-status";
/// How often the transcripts are looked at for new and changed sessions.
pub const SYNC_EVERY: Duration = Duration::from_secs(120);
/// Recall's folder, in the app's data folder.
pub const RECALL_DIR: &str = "recall";

/// What a sync in progress has done, for the card and the status strip.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallRun {
    pub source_id: &'static str,
    pub done: usize,
    pub total: usize,
    /// The project of the session being indexed.
    pub current: Option<String>,
    pub paused: bool,
}

/// What the last sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastSync {
    pub at: String,
    pub indexed: usize,
    pub removed: usize,
    pub failed: usize,
    pub waiting: usize,
    /// `cancelled` or `paused` when it stopped early.
    pub stopped: Option<&'static str>,
}

struct Inner {
    dir: PathBuf,
    model_dir: PathBuf,
    compute: Mutex<ComputePreference>,
    roots: Roots,
    index: RwLock<Option<Arc<RecallIndex>>>,
    settings: Mutex<RecallSettings>,
    settings_error: Mutex<Option<String>>,
    sync_lock: tokio::sync::Mutex<()>,
    run: Mutex<Option<RecallRun>>,
    last: Mutex<Option<LastSync>>,
    error: Mutex<Option<String>>,
    wake: tokio::sync::Notify,
}

/// Recall's state in the app (cheap to clone).
#[derive(Clone)]
pub struct RecallState(Arc<Inner>);

impl RecallState {
    /// Recall in `dir`, searching with the models under `model_dir`. Reads only its
    /// settings file; the index opens on first use.
    pub fn new(dir: PathBuf, model_dir: PathBuf, compute: ComputePreference, roots: Roots) -> Self {
        let (settings, settings_error) = match RecallSettings::load(&dir) {
            Ok(s) => (s, None),
            Err(e) => {
                tracing::warn!(target: "shodh::recall", error = %e, "Recall's settings are unreadable; Recall stays off");
                (
                    RecallSettings::default(),
                    Some(format!("Recall's settings could not be read ({e}); it stays off until they are saved again.")),
                )
            }
        };
        Self(Arc::new(Inner {
            dir,
            model_dir,
            compute: Mutex::new(compute),
            roots,
            index: RwLock::new(None),
            settings: Mutex::new(settings),
            settings_error: Mutex::new(settings_error),
            sync_lock: tokio::sync::Mutex::new(()),
            run: Mutex::new(None),
            last: Mutex::new(None),
            error: Mutex::new(None),
            wake: tokio::sync::Notify::new(),
        }))
    }

    pub fn settings(&self) -> RecallSettings {
        self.0
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn enabled(&self) -> bool {
        self.0
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .enabled
    }

    /// Recall's index, opened on first use (with the search models attached once they
    /// are installed).
    pub async fn index(&self) -> Result<Arc<RecallIndex>, String> {
        if let Some(index) = self.0.index.read().await.clone() {
            self.attach_models_if_installed(&index).await;
            return Ok(index);
        }
        let mut slot = self.0.index.write().await;
        if let Some(index) = slot.clone() {
            return Ok(index);
        }
        let compute = *self.0.compute.lock().unwrap_or_else(|e| e.into_inner());
        let index = RecallIndex::open(&self.0.dir, &self.0.model_dir, compute)
            .await
            .map_err(|e| format!("Recall's index could not be opened ({e:#})"))?;
        let index = Arc::new(index);
        // Its models load on first use and are dropped when idle, like the library's.
        if let Some(idle) = shodh_rag::lazy_model::idle_period() {
            let engine = index.engine().read().await;
            let models: Vec<Arc<dyn shodh_rag::lazy_model::IdleUnload>> =
                vec![engine.embedder_handle(), engine.reranker_handle()];
            tauri::async_runtime::spawn(shodh_rag::lazy_model::unload_idle_models(models, idle));
        }
        *slot = Some(index.clone());
        Ok(index)
    }

    /// Search models installed after the index opened are attached to it.
    async fn attach_models_if_installed(&self, index: &RecallIndex) {
        if index.engine().read().await.has_search_models() {
            return;
        }
        let mut engine = index.engine().write().await;
        let config = engine.config().clone();
        if engine.has_search_models() || !shodh_rag::SearchModels::available(&config) {
            return;
        }
        match shodh_rag::SearchModels::load(&config).and_then(|m| engine.attach_search_models(m)) {
            Ok(()) => {
                tracing::info!(target: "shodh::recall", "search models attached to Recall's index")
            }
            Err(e) => {
                tracing::warn!(target: "shodh::recall", error = %format!("{e:#}"), "Recall's index could not take the search models")
            }
        }
    }

    /// Whether Recall's folder holds an index (something to delete when it is off).
    fn has_index_on_disk(&self) -> bool {
        self.0
            .dir
            .join(shodh_rag::recall::index::CATALOG_FILE)
            .exists()
    }

    pub fn run(&self) -> Option<RecallRun> {
        self.0.run.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_run(&self, run: Option<RecallRun>) {
        *self.0.run.lock().unwrap_or_else(|e| e.into_inner()) = run;
    }

    /// Ask for a sync now.
    pub fn wake(&self) {
        self.0.wake.notify_one();
    }

    /// Saves new settings (checked first) and wakes the sync.
    pub fn save_settings(&self, mut settings: RecallSettings) -> Result<(), String> {
        settings.save(&self.0.dir).map_err(|e| e.to_string())?;
        *self.0.settings.lock().unwrap_or_else(|e| e.into_inner()) = settings;
        *self
            .0
            .settings_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        self.wake();
        Ok(())
    }

    /// Turns Recall off and deletes everything it made: the index (vectors and keyword
    /// index), the catalog with its journal, and the settings with the user's hidden
    /// words. The folder is removed whole, so nothing deleted lingers in free pages or
    /// soft-deleted segments; the transcripts are untouched. Waits for a sync in progress
    /// and for searches holding the index.
    pub async fn delete_all(&self) -> Result<(), String> {
        let _one = self.0.sync_lock.lock().await;
        *self.0.settings.lock().unwrap_or_else(|e| e.into_inner()) = RecallSettings::default();
        let taken = self.0.index.write().await.take();
        if let Some(index) = taken {
            // A search or an open may hold it a moment longer; its files close with it.
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let mut index = index;
            loop {
                match Arc::try_unwrap(index) {
                    Ok(owned) => {
                        drop(owned);
                        break;
                    }
                    Err(shared) if std::time::Instant::now() < deadline => {
                        index = shared;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(_) => {
                        return Err(
                            "Recall's index is still in use; try again in a moment.".to_string()
                        )
                    }
                }
            }
        }
        *self.0.last.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.0.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .0
            .settings_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        remove_folder(&self.0.dir).await
    }

    /// One sync: an indexing run of [`RECALL_SOURCE_ID`] that the status strip shows, holds
    /// and stops. Only one runs at a time.
    pub async fn sync_once(&self, app: &AppHandle) {
        let settings = self.settings();
        // Off, and nothing was ever indexed: nothing to open or read.
        if !settings.enabled && !self.has_index_on_disk() {
            return;
        }
        let _one = self.0.sync_lock.lock().await;
        let index = match self.index().await {
            Ok(index) => index,
            Err(e) => {
                *self.0.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                emit(app);
                return;
            }
        };
        if settings.enabled && !index.engine().read().await.has_search_models() {
            *self.0.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                "Recall needs the search models: install them in the Library first.".to_string(),
            );
            emit(app);
            return;
        }
        let indexing = app.state::<IndexingState>();
        let Some(run) = indexing.begin_run(RECALL_SOURCE_ID) else {
            return;
        };
        if run.wait_while_paused().await {
            return;
        }
        let control = run.control();
        let cancelled = {
            let control = control.clone();
            move || control.stopped()
        };
        let paused = {
            let control = control.clone();
            move || control.is_paused()
        };
        let state = self.clone();
        let progress_app = app.clone();
        let progress = move |p: &SyncProgress| {
            state.set_run(Some(RecallRun {
                source_id: RECALL_SOURCE_ID,
                done: p.done,
                total: p.total,
                current: p.current.clone(),
                paused: control.is_paused(),
            }));
            emit(&progress_app);
        };
        let sync_control = SyncControl {
            cancelled: &cancelled,
            paused: &paused,
            progress: &progress,
        };
        let result = index
            .sync(
                &settings,
                &self.0.roots,
                shodh_rag::recall::index::now_secs(),
                &sync_control,
            )
            .await;
        drop(run);
        self.set_run(None);
        match result {
            Ok(report) => {
                *self.0.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
                *self.0.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(last_sync(&report));
                if report.indexed + report.removed + report.failed > 0 {
                    tracing::info!(target: "shodh::recall", indexed = report.indexed, removed = report.removed, failed = report.failed, parts = report.parts_embedded, "Recall synced");
                }
            }
            Err(e) => {
                tracing::warn!(target: "shodh::recall", error = %format!("{e:#}"), "Recall's sync failed");
                *self.0.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(plain_error(&e));
            }
        }
        emit(app);
    }

    /// The background sync: when the settings change, and every [`SYNC_EVERY`].
    pub fn start(&self, app: AppHandle) {
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            // The app finishes starting first.
            tokio::time::sleep(Duration::from_secs(20)).await;
            loop {
                state.sync_once(&app).await;
                tokio::select! {
                    () = state.0.wake.notified() => {}
                    () = tokio::time::sleep(SYNC_EVERY) => {}
                }
            }
        });
    }
}

/// Removes Recall's folder, trying again for a few seconds while Windows still holds a
/// file the index just closed.
async fn remove_folder(dir: &Path) -> Result<(), String> {
    let mut last_error = None;
    for _ in 0..50 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last_error = Some(e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    Err(format!(
        "Recall's folder could not be deleted ({})",
        last_error.map_or_else(|| "unknown error".to_string(), |e| e.to_string())
    ))
}

fn last_sync(report: &SyncReport) -> LastSync {
    LastSync {
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        indexed: report.indexed,
        removed: report.removed,
        failed: report.failed,
        waiting: report.waiting,
        stopped: report.stopped.map(|s| match s {
            Stopped::Cancelled => "cancelled",
            Stopped::Paused => "paused",
        }),
    }
}

fn plain_error(e: &anyhow::Error) -> String {
    if e.is::<shodh_rag::embeddings::SearchModelsMissing>() {
        return "Recall needs the search models: install them in the Library first.".to_string();
    }
    shodh_rag::recall::clip(&format!("Indexing coding sessions failed: {e:#}"), 300)
}

fn emit(app: &AppHandle) {
    if let Err(e) = app.emit(RECALL_EVENT, ()) {
        tracing::debug!(target: "shodh::recall", error = %e, "the Recall status event was not sent");
    }
}

/// The sync in progress, for the status strip.
pub fn current_run(app: &AppHandle) -> Option<RecallRun> {
    let state = app.try_state::<RecallState>()?;
    let mut run = state.run()?;
    run.paused = app.state::<IndexingState>().is_paused();
    Some(run)
}

// ── The card ────────────────────────────────────────────────────────

/// A tool, as the card shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolView {
    pub id: &'static str,
    pub label: &'static str,
    /// Where it keeps its transcripts.
    pub folder: Option<String>,
    /// Transcript files there (counted by name; none is read).
    pub files: usize,
    pub chosen: bool,
}

/// A project, as the card shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub key: String,
    pub name: String,
    pub dir: String,
    /// `true` included, `false` excluded, `None` not decided yet (not read).
    pub included: Option<bool>,
    pub sessions: usize,
    pub indexed_sessions: usize,
    pub tools: Vec<&'static str>,
    /// Sessions that could not be indexed.
    pub failed: usize,
    pub last_active: Option<String>,
}

/// Recall's state, as the card shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallStatus {
    pub settings: RecallSettings,
    pub tools: Vec<ToolView>,
    pub projects: Vec<ProjectView>,
    pub indexed_sessions: usize,
    pub indexed_turns: u64,
    pub secrets_redacted: u64,
    pub unreadable_lines: u64,
    pub running: Option<RecallRun>,
    pub last_sync: Option<LastSync>,
    pub error: Option<String>,
    pub search_models: bool,
    /// Sessions indexed under other hidden words or rules, not served until they are
    /// indexed again.
    pub stale_sessions: usize,
    /// The model the assistant answers with now: what it reads of the sessions goes there.
    pub assistant_model: Option<crate::agent_tools::ModelInfo>,
}

/// The model the assistant answers with, as the configuration says.
fn assistant_model(app: &AppHandle) -> Option<crate::agent_tools::ModelInfo> {
    use crate::agent_tools::HostEffects;
    crate::agent_tools::TauriEffects::new(app.clone()).model_info()
}

fn count_files(tool: Tool, root: Option<&Path>) -> usize {
    root.map_or(0, |r| discover::transcripts(tool, r).len())
}

/// Builds the card's status. The catalog is read only when the index is open (or was
/// made before): an untouched Recall opens nothing.
pub async fn status(state: &RecallState, app: &AppHandle) -> RecallStatus {
    let settings = state.settings();
    let roots = state.0.roots.clone();
    // While Recall is off not even the tools' folders are listed.
    let counts = if settings.enabled {
        tokio::task::spawn_blocking(move || {
            Tool::ALL
                .iter()
                .map(|t| (*t, count_files(*t, roots.of(*t))))
                .collect::<BTreeMap<_, _>>()
        })
        .await
        .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    let tools = Tool::ALL
        .iter()
        .map(|t| ToolView {
            id: t.id(),
            label: t.label(),
            folder: state.0.roots.of(*t).map(|p| p.display().to_string()),
            files: counts.get(t).copied().unwrap_or(0),
            chosen: settings.tools.contains(t),
        })
        .collect();
    let mut out = RecallStatus {
        settings: settings.clone(),
        tools,
        projects: Vec::new(),
        indexed_sessions: 0,
        indexed_turns: 0,
        secrets_redacted: 0,
        unreadable_lines: 0,
        running: current_run(app),
        last_sync: state
            .0
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        error: state
            .0
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .or_else(|| {
                state
                    .0
                    .settings_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            }),
        search_models: false,
        stale_sessions: 0,
        assistant_model: assistant_model(app),
    };
    if !settings.enabled && !state.has_index_on_disk() {
        return out;
    }
    let Ok(index) = state.index().await else {
        return out;
    };
    out.search_models = index.engine().read().await.has_search_models();
    out.stale_sessions = shodh_rag::recall::query::stale_files(&index, &settings).unwrap_or(0);
    // Folders are shown under the current hidden words (redacted again, so a word added
    // since they were found is hidden too).
    let redactor = settings.redactor().unwrap_or_default();
    let files = match index.catalog().files() {
        Ok(files) => files,
        Err(e) => {
            out.error = Some(format!("Recall's catalog could not be read ({e})"));
            return out;
        }
    };
    struct Acc {
        dir: String,
        chain: Vec<String>,
        sessions: BTreeSet<(Tool, String)>,
        indexed: BTreeSet<(Tool, String)>,
        failed: BTreeSet<(Tool, String)>,
        tools: BTreeSet<Tool>,
        last: Option<i64>,
    }
    let mut projects: BTreeMap<String, Acc> = BTreeMap::new();
    let mut indexed_sessions = BTreeSet::new();
    for f in &files {
        if !settings.tools.contains(&f.tool) {
            continue;
        }
        let acc = projects
            .entry(f.project_key.clone())
            .or_insert_with(|| Acc {
                dir: redactor.redact(&f.project_dir),
                chain: f
                    .chains
                    .first()
                    .cloned()
                    .unwrap_or_else(|| vec![f.project_key.clone()]),
                sessions: BTreeSet::new(),
                indexed: BTreeSet::new(),
                failed: BTreeSet::new(),
                tools: BTreeSet::new(),
                last: None,
            });
        let key = (f.tool, f.session_id.clone());
        acc.sessions.insert(key.clone());
        acc.tools.insert(f.tool);
        acc.last = acc.last.max(f.ended_at.or(Some(f.mtime)));
        match f.state {
            FileState::Indexed => {
                acc.indexed.insert(key.clone());
                indexed_sessions.insert(key);
                out.indexed_turns += u64::from(f.turns);
                out.secrets_redacted += u64::from(f.redacted);
                out.unreadable_lines += u64::from(f.errors);
            }
            FileState::Failed => {
                acc.failed.insert(key);
            }
            FileState::Found | FileState::Excluded => {}
        }
    }
    out.indexed_sessions = indexed_sessions.len();
    let mut views: Vec<ProjectView> = projects
        .into_iter()
        .map(|(key, acc)| ProjectView {
            name: discover::project_name(&acc.dir),
            included: settings.decide(&acc.chain),
            dir: acc.dir,
            sessions: acc.sessions.len(),
            indexed_sessions: acc.indexed.len(),
            tools: acc.tools.iter().map(|t| t.id()).collect(),
            failed: acc.failed.len(),
            last_active: acc
                .last
                .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            key,
        })
        .collect();
    views.sort_by(|a, b| b.last_active.cmp(&a.last_active).then(a.name.cmp(&b.name)));
    out.projects = views;
    out
}

// ── Commands ────────────────────────────────────────────────────────

/// Recall's state for the Library card.
#[tauri::command]
pub async fn recall_status(
    app: AppHandle,
    state: State<'_, RecallState>,
) -> Result<RecallStatus, String> {
    Ok(status(&state, &app).await)
}

/// Saves the user's choices (on or off, tools, projects, their own sensitive
/// patterns, the idle time) and syncs: what is no longer chosen leaves the index.
#[tauri::command]
pub async fn recall_save_settings(
    app: AppHandle,
    state: State<'_, RecallState>,
    settings: RecallSettings,
) -> Result<RecallStatus, String> {
    if settings.enabled {
        state.save_settings(settings)?;
    } else {
        // Off means deleted: nothing Recall made is kept, and nothing runs after.
        state.delete_all().await?;
        emit(&app);
    }
    Ok(status(&state, &app).await)
}

/// Looks at the transcripts now.
#[tauri::command]
pub async fn recall_sync_now(state: State<'_, RecallState>) -> Result<(), String> {
    state.wake();
    Ok(())
}

/// Removes one session from the index; it is never indexed again. The transcript is
/// left as it is.
#[tauri::command]
pub async fn recall_remove_session(
    app: AppHandle,
    state: State<'_, RecallState>,
    tool: String,
    session_id: String,
) -> Result<RecallStatus, String> {
    let tool =
        Tool::from_id(&tool).ok_or_else(|| format!("{tool} is not a coding tool Recall reads"))?;
    // Off, nothing is opened: opening the index would make its folder again.
    if !state.enabled() {
        return Err(OFF.to_string());
    }
    let index = state.index().await?;
    let _one = state.0.sync_lock.lock().await;
    index
        .remove_session(tool, &session_id)
        .await
        .map_err(|e| format!("The session could not be removed ({e:#})"))?;
    drop(_one);
    Ok(status(&state, &app).await)
}

/// A search over the coding sessions, as the card's search box and the viewer ask it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchRequest {
    pub query: String,
    pub tools: Vec<String>,
    pub projects: Vec<String>,
    /// A period (`last week`, `2026-10-12`), or `since` and `until` (RFC 3339).
    pub period: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub entity: Option<String>,
    pub limit: Option<usize>,
}

/// A time a request gives: RFC 3339, or a date (its start; its end for `end_of_day`).
fn time_arg(value: Option<&str>, name: &str, end_of_day: bool) -> Result<Option<i64>, String> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if let Some(t) = shodh_rag::recall::parse_time(v) {
        return Ok(Some(t.timestamp()));
    }
    let day = chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| {
        format!("{name} must be a date (2026-10-12) or a time (2026-10-12T09:00:00Z)")
    })?;
    let at = if end_of_day {
        day.and_hms_opt(23, 59, 59)
    } else {
        day.and_hms_opt(0, 0, 0)
    };
    Ok(at.map(|t| t.and_utc().timestamp()))
}

/// The window a request names: a period, or since and until (a date alone as the end
/// means the end of that day).
pub fn window(
    period: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(Option<i64>, Option<i64>), String> {
    if let Some(p) = period.map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = query::period(p, chrono::Utc::now()).ok_or_else(|| {
            format!("\"{p}\" is not a period Recall knows: use today, yesterday, this week, last week, this month, last month, last N days, or a date like 2026-10-12")
        })?;
        return Ok((Some(a), Some(b)));
    }
    Ok((
        time_arg(since, "since", false)?,
        time_arg(until, "until", true)?,
    ))
}

pub fn tools_arg(tools: &[String]) -> Result<Vec<Tool>, String> {
    tools
        .iter()
        .map(|t| {
            Tool::from_id(t.trim()).ok_or_else(|| {
                format!("{t} is not a coding tool Recall reads (claude_code, codex)")
            })
        })
        .collect()
}

/// Searches the coding sessions.
#[tauri::command]
pub async fn recall_search(
    state: State<'_, RecallState>,
    request: SearchRequest,
) -> Result<SearchAnswer, String> {
    search(&state, &request).await
}

pub async fn search(state: &RecallState, request: &SearchRequest) -> Result<SearchAnswer, String> {
    let settings = state.settings();
    if !settings.enabled {
        return Err(OFF.to_string());
    }
    let (since, until) = window(
        request.period.as_deref(),
        request.since.as_deref(),
        request.until.as_deref(),
    )?;
    let index = state.index().await?;
    query::search(
        &index,
        &settings,
        &RecallQuery {
            text: request.query.clone(),
            tools: tools_arg(&request.tools)?,
            projects: request.projects.clone(),
            since,
            until,
            entity: request.entity.clone(),
            entity_kind: None,
            limit: request.limit.unwrap_or(query::DEFAULT_SESSIONS),
        },
    )
    .await
    .map_err(|e| format!("Searching the coding sessions failed ({e:#})"))
}

/// What a request says while Recall is off.
pub const OFF: &str =
    "Recall is off: turn on \"AI coding sessions\" in the Library to search your coding sessions.";

/// Every session in a window.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScopeRequest {
    pub period: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub tools: Vec<String>,
    pub projects: Vec<String>,
}

/// Every session overlapping a window, complete and unranked.
#[tauri::command]
pub async fn recall_scope(
    state: State<'_, RecallState>,
    request: ScopeRequest,
) -> Result<ScopeAnswer, String> {
    scope(&state, &request).await
}

pub async fn scope(state: &RecallState, request: &ScopeRequest) -> Result<ScopeAnswer, String> {
    let settings = state.settings();
    if !settings.enabled {
        return Err(OFF.to_string());
    }
    let (since, until) = window(
        request.period.as_deref(),
        request.since.as_deref(),
        request.until.as_deref(),
    )?;
    let tools = tools_arg(&request.tools)?;
    let index = state.index().await?;
    let projects = request.projects.clone();
    tokio::task::spawn_blocking(move || {
        query::scope(&index, &settings, since, until, &tools, &projects)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Listing the coding sessions failed ({e:#})"))
}

/// Opens a receipt: the exact turn and the turns around it, read again from the
/// transcript and redacted.
#[tauri::command]
pub async fn recall_open(
    state: State<'_, RecallState>,
    receipt: String,
    around: Option<u32>,
) -> Result<Opened, String> {
    open(&state, &receipt, around.unwrap_or(query::DEFAULT_AROUND)).await
}

pub async fn open(state: &RecallState, receipt: &str, around: u32) -> Result<Opened, String> {
    let settings = state.settings();
    if !settings.enabled {
        return Err(OFF.to_string());
    }
    let index = state.index().await?;
    let receipt = receipt.to_string();
    tokio::task::spawn_blocking(move || query::open(&index, &settings, &receipt, around))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Recall's state for the app, in `app_data_dir/recall`.
pub fn manage(app: &tauri::App, app_data_dir: &Path, model_dir: &Path, use_gpu: bool) {
    let state = RecallState::new(
        app_data_dir.join(RECALL_DIR),
        model_dir.to_path_buf(),
        crate::indexing_control::compute_preference(use_gpu),
        Roots::home(),
    );
    state.start(app.handle().clone());
    app.manage(state);
}

#[cfg(test)]
impl RecallState {
    /// Recall over an index already open, with `settings` (tests).
    pub fn with_index(
        dir: PathBuf,
        index: Arc<RecallIndex>,
        settings: RecallSettings,
        roots: Roots,
    ) -> Self {
        let state = Self::new(
            dir,
            PathBuf::from("no-models"),
            ComputePreference::Cpu,
            roots,
        );
        *state.0.settings.lock().unwrap_or_else(|e| e.into_inner()) = settings;
        *state
            .0
            .index
            .try_write()
            .unwrap_or_else(|_| panic!("a new state's index is not locked")) = Some(index);
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_come_from_periods_or_dates() {
        let (since, until) = window(None, Some("2026-10-12"), Some("2026-10-12")).unwrap();
        assert_eq!(
            until.unwrap() - since.unwrap(),
            86_399,
            "a date as the end is the whole day"
        );
        let (since, until) = window(None, Some("2026-10-12T09:00:00Z"), None).unwrap();
        assert!(since.is_some() && until.is_none());
        assert!(window(Some("someday"), None, None).is_err());
        assert!(window(None, Some("yesterday-ish"), None).is_err());
        let (a, b) = window(Some("last 7 days"), None, None).unwrap();
        assert_eq!(b.unwrap() - a.unwrap(), 7 * 86_400);
    }

    #[test]
    fn tools_are_checked() {
        assert_eq!(tools_arg(&["codex".into()]).unwrap(), vec![Tool::Codex]);
        assert!(tools_arg(&["cursor".into()]).is_err());
    }

    #[tokio::test]
    async fn requests_while_recall_is_off_say_so_and_open_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let state = RecallState::new(
            dir.path().join("recall"),
            dir.path().join("no-models"),
            ComputePreference::Cpu,
            Roots {
                claude_code: None,
                codex: None,
            },
        );
        let request = SearchRequest {
            query: "x".into(),
            ..SearchRequest::default()
        };
        assert_eq!(search(&state, &request).await.unwrap_err(), OFF);
        assert_eq!(
            scope(&state, &ScopeRequest::default()).await.unwrap_err(),
            OFF
        );
        assert_eq!(open(&state, "1:0", 2).await.unwrap_err(), OFF);
        assert!(
            !dir.path().join("recall").exists(),
            "nothing was made while off"
        );
    }
}
