//! Code mode's code map in the app: one map per Code project
//! ([`shodh_rag::code_map::CodeMap`]), built in the background when the
//! project opens in Code mode and kept current from then on.
//!
//! - **When it refreshes**: when a Code conversation of the project is shown
//!   ([`code_map_open`]), when one of its sessions starts, after the agent's
//!   edits and the commands that may change files ([`touched`]), and on a
//!   file change while the project is shown (a watcher). Requests are
//!   coalesced until they go quiet ([`DEBOUNCE`], at most [`MAX_DEBOUNCE`]);
//!   a refresh reads only files whose size, time or contents changed.
//! - **How it runs**: each refresh is an indexing run under the source
//!   `code-map:<key>` (without a path, so the Library never takes it for one
//!   of its folders): the status strip shows it, Pause and Stop hold and stop
//!   it, and it waits while other indexing runs, unless an agent tool needs
//!   the map now ([`CodeMapState::request`] with `urgent`). While it waits it
//!   holds nothing, so the agent's tools read and update the map meanwhile.
//! - **Leases**: each shown conversation holds a lease ([`code_map_open`]
//!   returns its id, [`code_map_close`] gives it back). The watcher runs
//!   while a lease is held; a reload of the window drops every lease; a
//!   workspace deleted drops its leases and its map's file.
//! - **What it costs at rest**: nothing before first use. After the last
//!   lease is given back and nothing used the map for [`IDLE`], its database
//!   and model are closed and its worker ends.
//!
//! The agent reads the map through Code sessions' `code_definition`,
//! `code_references`, `code_outline` and `code_dependencies`
//! ([`crate::agent_tools::code_map_tools`]); the UI through the commands here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use shodh_rag::code_map::{CodeMap, MapStatus, Outline, Pace, RefreshReport, SymbolHit};
use shodh_rag::indexing::{IndexingState, PauseSwitch};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::agent_session_commands::workspace_code_folder;
use crate::workspace_commands::WorkspaceState;

/// Indexing runs of code maps are under `code-map:<key>`.
pub const CODE_MAP_SOURCE_PREFIX: &str = "code-map:";
/// Emitted with a [`CodeMapView`] when a map's state or progress changes.
pub const CODE_MAP_EVENT: &str = "code-map-status";
/// A refresh starts once requests have been quiet this long...
pub const DEBOUNCE: Duration = Duration::from_millis(1_200);
/// ...or this long after the first of them, whichever comes first.
pub const MAX_DEBOUNCE: Duration = Duration::from_secs(10);
/// A map nobody looks at or used for this long is closed.
pub const IDLE: Duration = Duration::from_secs(5 * 60);
/// Progress is sent at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(300);
/// Symbols a jump search returns at most.
const MAX_FOUND: usize = 30;

/// A map's state, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum Phase {
    /// Nothing is running.
    Idle,
    /// A refresh is asked for and starts once requests go quiet.
    Queued,
    /// Reading files: `done` of `total` looked at.
    Building {
        done: usize,
        total: usize,
    },
    /// A refresh waits: indexing is paused, or other indexing runs first.
    Waiting {
        done: usize,
        total: usize,
        /// `paused` or `otherIndexing`.
        reason: &'static str,
    },
    Failed {
        error: String,
    },
}

#[derive(Default)]
struct Pending {
    wanted: bool,
    rebuild: bool,
}

/// One project's map and its refresh worker.
pub struct Project {
    pub key: String,
    /// The folder as the workspace gives it.
    pub root: PathBuf,
    canonical: PathBuf,
    pub name: String,
    map: Mutex<Option<Arc<CodeMap>>>,
    pending: Mutex<Pending>,
    wake: tokio::sync::Notify,
    worker: AtomicBool,
    phase: Mutex<Phase>,
    last_run: Mutex<Option<RefreshReport>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    /// Leases held on it (shown conversations).
    viewers: AtomicUsize,
    /// An agent tool needs the map now: the refresh does not wait behind
    /// other indexing (it still holds for the user's pause).
    urgent: AtomicBool,
    last_used: Mutex<Instant>,
    /// Its workspace was deleted: the map's file goes once nothing uses it.
    forgotten: AtomicBool,
}

impl Project {
    pub fn source_id(&self) -> String {
        format!("{CODE_MAP_SOURCE_PREFIX}{}", self.key)
    }

    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = phase;
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    /// Ask for a refresh (the worker runs it once requests go quiet).
    fn request(&self, rebuild: bool) {
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.wanted = true;
            pending.rebuild |= rebuild;
        }
        {
            let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*phase, Phase::Idle | Phase::Failed { .. }) {
                *phase = Phase::Queued;
            }
        }
        self.wake.notify_one();
    }

    fn take_pending(&self) -> Option<bool> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !pending.wanted {
            return None;
        }
        let rebuild = pending.rebuild;
        *pending = Pending::default();
        Some(rebuild)
    }

    fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wanted
    }

    fn is_watched(&self) -> bool {
        self.watcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    fn is_open(&self) -> bool {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    fn busy(&self) -> bool {
        matches!(
            self.phase(),
            Phase::Queued | Phase::Building { .. } | Phase::Waiting { .. }
        )
    }

    /// The project-relative path of an absolute one the watcher reports.
    fn relative(&self, path: &Path) -> Option<String> {
        let rel = path
            .strip_prefix(&self.root)
            .or_else(|_| path.strip_prefix(&self.canonical))
            .ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }
}

/// A shown conversation's hold on its project's map.
struct Lease {
    project: Arc<Project>,
    workspace_id: String,
}

/// Every Code project's map, opened when first asked for.
#[derive(Clone)]
pub struct CodeMapState {
    inner: Arc<Inner>,
}

struct Inner {
    data_dir: PathBuf,
    /// The indexing pause (the strip's and the tray's).
    pause: Option<PauseSwitch>,
    projects: Mutex<HashMap<String, Arc<Project>>>,
    leases: Mutex<HashMap<u64, Lease>>,
    next_lease: AtomicU64,
    /// The app, once it runs: refreshes go through its indexing runs. Without
    /// it (tests) a refresh asked for runs at once on the caller's thread.
    app: OnceLock<AppHandle>,
}

impl CodeMapState {
    pub fn new(data_dir: PathBuf, pause: Option<PauseSwitch>) -> Self {
        CodeMapState {
            inner: Arc::new(Inner {
                data_dir,
                pause,
                projects: Mutex::new(HashMap::new()),
                leases: Mutex::new(HashMap::new()),
                next_lease: AtomicU64::new(1),
                app: OnceLock::new(),
            }),
        }
    }

    /// Refreshes asked for from now on run as the app's indexing runs.
    pub fn attach(&self, app: AppHandle) {
        let _ = self.inner.app.set(app);
    }

    /// Indexing is paused by the user.
    pub fn paused(&self) -> bool {
        self.inner
            .pause
            .as_ref()
            .is_some_and(PauseSwitch::is_paused)
    }

    /// The project at `root` (registered on first use; nothing is read yet).
    /// Blocking (canonicalizes the folder).
    pub fn project(&self, root: &Path) -> Result<Arc<Project>, String> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|e| format!("the code folder could not be read ({e})"))?;
        let key = CodeMap::key_of(&canonical);
        let mut projects = self
            .inner
            .projects
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Ok(projects
            .entry(key.clone())
            .or_insert_with(|| {
                let name = canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| canonical.display().to_string());
                Arc::new(Project {
                    key,
                    root: root.to_path_buf(),
                    canonical,
                    name,
                    map: Mutex::new(None),
                    pending: Mutex::new(Pending::default()),
                    wake: tokio::sync::Notify::new(),
                    worker: AtomicBool::new(false),
                    phase: Mutex::new(Phase::Idle),
                    last_run: Mutex::new(None),
                    watcher: Mutex::new(None),
                    viewers: AtomicUsize::new(0),
                    urgent: AtomicBool::new(false),
                    last_used: Mutex::new(Instant::now()),
                    forgotten: AtomicBool::new(false),
                })
            })
            .clone())
    }

    /// The project's map, opened (or created) on first use. Blocking.
    pub fn map_of(&self, project: &Project) -> Result<Arc<CodeMap>, String> {
        project.touch();
        let mut slot = project.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(map) = slot.as_ref() {
            return Ok(map.clone());
        }
        let map = Arc::new(CodeMap::open(&self.inner.data_dir, &project.root)?);
        *slot = Some(map.clone());
        Ok(map)
    }

    /// The project and map of the folder at `root`, on a blocking thread.
    pub async fn open_for(&self, root: &Path) -> Result<(Arc<Project>, Arc<CodeMap>), String> {
        let state = self.clone();
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let project = state.project(&root)?;
            let map = state.map_of(&project)?;
            Ok((project, map))
        })
        .await
        .map_err(|e| format!("opening the code map did not finish: {e}"))?
    }

    /// Ask for a refresh of `project`: in the app, a background indexing run
    /// (`urgent`: it does not wait behind other indexing); without the app,
    /// at once on this thread. Blocking only without the app.
    pub fn request(&self, project: &Arc<Project>, rebuild: bool, urgent: bool) {
        if urgent {
            project.urgent.store(true, Ordering::SeqCst);
        }
        match self.inner.app.get() {
            Some(app) => schedule(app, self, project, rebuild),
            None => {
                let result = self.map_of(project).and_then(|map| {
                    if rebuild {
                        map.clear()?;
                    }
                    map.refresh(&|| Pace::Go, &mut |_, _| {})
                });
                match result {
                    Ok(report) => {
                        *project.last_run.lock().unwrap_or_else(|e| e.into_inner()) = Some(report);
                        project.set_phase(Phase::Idle);
                    }
                    Err(error) => project.set_phase(Phase::Failed { error }),
                }
            }
        }
    }

    /// Every project with a refresh reading files or waiting now.
    pub fn running(&self) -> Vec<Arc<Project>> {
        let projects = self
            .inner
            .projects
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<Arc<Project>> = projects
            .values()
            .filter(|p| matches!(p.phase(), Phase::Building { .. } | Phase::Waiting { .. }))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Hold the project's map for a shown conversation of `workspace_id`;
    /// the returned lease id gives it back ([`Self::release`]).
    pub fn lease(&self, project: &Arc<Project>, workspace_id: &str) -> u64 {
        let id = self.inner.next_lease.fetch_add(1, Ordering::SeqCst);
        project.viewers.fetch_add(1, Ordering::SeqCst);
        project.touch();
        if let Err(e) = start_watching(project) {
            tracing::warn!(target: "shodh::code_map", project = %project.name, error = %e, "file changes are not watched");
        }
        self.inner
            .leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id,
                Lease {
                    project: project.clone(),
                    workspace_id: workspace_id.to_string(),
                },
            );
        id
    }

    /// Give back a lease; the last one of a project stops its watcher.
    /// Returns whether the lease was held.
    pub fn release(&self, lease: u64) -> bool {
        let taken = self
            .inner
            .leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&lease);
        match taken {
            Some(lease) => {
                release_viewer(&lease.project);
                true
            }
            None => false,
        }
    }

    /// Give back every lease (the window was reloaded: its conversations
    /// are gone without saying so).
    pub fn release_all(&self) -> usize {
        let all: Vec<Lease> = self
            .inner
            .leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, lease)| lease)
            .collect();
        for lease in &all {
            release_viewer(&lease.project);
        }
        all.len()
    }

    /// Leases held now (for tests and diagnostics).
    pub fn leases(&self) -> usize {
        self.inner
            .leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// The workspace `workspace_id` was deleted: its leases are given back,
    /// and the map of `root` (when given and no other workspace uses it) is
    /// closed and its file deleted, at once or as soon as a running refresh
    /// ends. Blocking.
    pub fn forget(&self, workspace_id: &str, root: Option<&Path>, still_used: bool) {
        let mine: Vec<u64> = self
            .inner
            .leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, lease)| lease.workspace_id == workspace_id)
            .map(|(id, _)| *id)
            .collect();
        for id in mine {
            self.release(id);
        }
        let (Some(root), false) = (root, still_used) else {
            return;
        };
        let Ok(canonical) = std::fs::canonicalize(root) else {
            // The folder is gone too: its map is found by its key alone.
            return;
        };
        let key = CodeMap::key_of(&canonical);
        let project = self
            .inner
            .projects
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        let path = CodeMap::map_path_of(&self.inner.data_dir, &canonical);
        match project {
            Some(project) => {
                project.forgotten.store(true, Ordering::SeqCst);
                if let Some(app) = self.inner.app.get() {
                    app.state::<IndexingState>()
                        .cancel_source(&project.source_id());
                }
                if !project.busy() {
                    self.close_forgotten(&project);
                }
            }
            None => {
                if let Err(e) = CodeMap::delete_files(&path) {
                    tracing::warn!(target: "shodh::code_map", error = %e, "the code map of a deleted workspace was not deleted");
                }
            }
        }
    }

    /// Close a forgotten project's map and delete its file.
    fn close_forgotten(&self, project: &Arc<Project>) {
        *project.map.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *project.watcher.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.inner
            .projects
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&project.key);
        let path = CodeMap::map_path_of(&self.inner.data_dir, &project.canonical);
        match CodeMap::delete_files(&path) {
            Ok(()) => {
                tracing::info!(target: "shodh::code_map", project = %project.name, "the code map of a deleted workspace was deleted")
            }
            Err(e) => {
                tracing::warn!(target: "shodh::code_map", project = %project.name, error = %e, "the code map of a deleted workspace was not deleted")
            }
        }
    }

    /// Close `project`'s map when nobody looks at it, nothing runs, and it
    /// was not used for `idle`. Returns whether it was closed.
    pub fn close_if_idle(&self, project: &Project, idle: Duration) -> bool {
        if project.viewers.load(Ordering::SeqCst) > 0
            || project.busy()
            || project.has_pending()
            || project.idle_for() < idle
        {
            return false;
        }
        let mut slot = project.map.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            return false;
        }
        *slot = None;
        tracing::debug!(target: "shodh::code_map", project = %project.name, "idle code map closed");
        true
    }
}

fn release_viewer(project: &Project) {
    let before = project
        .viewers
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            Some(n.saturating_sub(1))
        })
        .unwrap_or(0);
    if before <= 1 {
        *project.watcher.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    project.touch();
}

/// A map as the UI shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeMapView {
    pub workspace_id: Option<String>,
    pub source_id: String,
    pub name: String,
    #[serde(flatten)]
    pub phase: Phase,
    /// Indexing is paused (a refresh waits).
    pub paused: bool,
    /// File changes are being watched (the project is shown).
    pub watching: bool,
    /// What the map holds; `None` while it is not open.
    pub map: Option<MapStatus>,
    pub last_run: Option<RefreshReport>,
    /// The lease [`code_map_open`] took (give it back with [`code_map_close`]).
    pub lease: Option<u64>,
}

/// The view of `project` (reads the map's counts; blocking).
pub fn view_of(
    state: &CodeMapState,
    project: &Project,
    workspace_id: Option<String>,
) -> CodeMapView {
    let opened = project
        .map
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let map = opened.and_then(|m| m.status().ok());
    CodeMapView {
        workspace_id,
        source_id: project.source_id(),
        name: project.name.clone(),
        phase: project.phase(),
        paused: state.paused(),
        watching: project.is_watched(),
        map,
        last_run: project
            .last_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        lease: None,
    }
}

/// Send the project's view to the window (counts read on a blocking thread,
/// never on the caller's).
fn emit_view(app: &AppHandle, state: &CodeMapState, project: &Arc<Project>) {
    let (app, state, project) = (app.clone(), state.clone(), project.clone());
    tauri::async_runtime::spawn_blocking(move || emit_view_now(&app, &state, &project));
}

fn emit_view_now(app: &AppHandle, state: &CodeMapState, project: &Project) {
    let view = view_of(state, project, None);
    if let Err(e) = app.emit(CODE_MAP_EVENT, &view) {
        tracing::debug!("Failed to emit {CODE_MAP_EVENT}: {e}");
    }
}

/// Wait until requests have been quiet for [`DEBOUNCE`] (at most
/// [`MAX_DEBOUNCE`] from the first).
async fn quiet(project: &Project) {
    let started = Instant::now();
    loop {
        let left = MAX_DEBOUNCE.saturating_sub(started.elapsed());
        if left.is_zero() {
            return;
        }
        tokio::select! {
            () = project.wake.notified() => {}
            () = tokio::time::sleep(DEBOUNCE.min(left)) => return,
        }
    }
}

/// Ask for a refresh of `project`, starting its worker when it has none.
pub fn schedule(app: &AppHandle, state: &CodeMapState, project: &Arc<Project>, rebuild: bool) {
    project.request(rebuild);
    if !project.worker.swap(true, Ordering::SeqCst) {
        let app = app.clone();
        let state = state.clone();
        let project = project.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    () = project.wake.notified() => {}
                    () = tokio::time::sleep(IDLE) => {
                        if close_idle(&state, &project) {
                            break;
                        }
                        continue;
                    }
                }
                // An urgent request runs at once; others once things go quiet.
                if !project.urgent.load(Ordering::SeqCst) {
                    quiet(&project).await;
                }
                let Some(rebuild) = project.take_pending() else {
                    continue;
                };
                run_refresh(&app, &state, &project, rebuild).await;
                if project.forgotten.load(Ordering::SeqCst) {
                    let (closing_state, closing) = (state.clone(), project.clone());
                    let _ = tokio::task::spawn_blocking(move || {
                        closing_state.close_forgotten(&closing)
                    })
                    .await;
                    project.worker.store(false, Ordering::SeqCst);
                    break;
                }
            }
        });
    }
    emit_view(app, state, project);
}

/// The worker ends with an idle map: closed, and no request came meanwhile.
fn close_idle(state: &CodeMapState, project: &Project) -> bool {
    if !state.close_if_idle(project, IDLE) && project.is_open() {
        return false;
    }
    if project.viewers.load(Ordering::SeqCst) > 0 {
        return false;
    }
    project.worker.store(false, Ordering::SeqCst);
    // A request that came in between is not lost: its worker goes on.
    if project.has_pending() && !project.worker.swap(true, Ordering::SeqCst) {
        project.wake.notify_one();
        return false;
    }
    true
}

/// Another indexing run (not a code map) is in progress.
fn others_indexing(app: &AppHandle) -> bool {
    app.state::<IndexingState>()
        .running_sources()
        .iter()
        .any(|source| !source.starts_with(CODE_MAP_SOURCE_PREFIX))
}

async fn run_refresh(app: &AppHandle, state: &CodeMapState, project: &Arc<Project>, rebuild: bool) {
    let map = {
        let state = state.clone();
        let project = project.clone();
        tokio::task::spawn_blocking(move || {
            let map = state.map_of(&project)?;
            if rebuild {
                map.clear()?;
            }
            Ok::<_, String>(map)
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
    };
    let map = match map {
        Ok(map) => map,
        Err(error) => {
            tracing::warn!(project = %project.name, %error, "the code map could not be opened");
            project.set_phase(Phase::Failed { error });
            emit_view(app, state, project);
            return;
        }
    };
    let indexing = app.state::<IndexingState>();
    // Registered without a path: it is not one of the Library's folders.
    let Some(run) = indexing.begin_run(&project.source_id()) else {
        project.set_phase(Phase::Idle);
        return;
    };
    project.set_phase(Phase::Building { done: 0, total: 0 });
    emit_view(app, state, project);
    let control = run.control();
    let pace_app = app.clone();
    let pace_state = state.clone();
    let pace_project = project.clone();
    let progress_app = app.clone();
    let progress_state = state.clone();
    let progress_project = project.clone();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let pace = move || {
            let (done, total) = match pace_project.phase() {
                Phase::Building { done, total } | Phase::Waiting { done, total, .. } => {
                    (done, total)
                }
                _ => (0, 0),
            };
            if control.stopped() {
                return Pace::Stop;
            }
            let waiting = if control.is_paused() {
                Some("paused")
            } else if !pace_project.urgent.load(Ordering::SeqCst) && others_indexing(&pace_app) {
                Some("otherIndexing")
            } else {
                None
            };
            match waiting {
                Some(reason) => {
                    let now = Phase::Waiting {
                        done,
                        total,
                        reason,
                    };
                    if pace_project.phase() != now {
                        pace_project.set_phase(now);
                        emit_view_now(&pace_app, &pace_state, &pace_project);
                    }
                    Pace::Hold
                }
                None => {
                    if matches!(pace_project.phase(), Phase::Waiting { .. }) {
                        pace_project.set_phase(Phase::Building { done, total });
                    }
                    Pace::Go
                }
            }
        };
        let mut last = Instant::now();
        let mut progress = move |done: usize, total: usize| {
            if !matches!(progress_project.phase(), Phase::Waiting { .. }) {
                progress_project.set_phase(Phase::Building { done, total });
            }
            if last.elapsed() >= PROGRESS_EVERY || done == total {
                last = Instant::now();
                emit_view_now(&progress_app, &progress_state, &progress_project);
            }
        };
        map.refresh(&pace, &mut progress)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    drop(run);
    project.urgent.store(false, Ordering::SeqCst);
    match result {
        Ok(report) => {
            tracing::info!(
                target: "shodh::code_map",
                project = %project.name,
                listed = report.listed,
                read = report.read,
                removed = report.removed,
                stopped = report.stopped,
                ms = started.elapsed().as_millis() as u64,
                "code map refreshed"
            );
            *project.last_run.lock().unwrap_or_else(|e| e.into_inner()) = Some(report);
            project.set_phase(Phase::Idle);
        }
        Err(error) => {
            tracing::warn!(target: "shodh::code_map", project = %project.name, %error, "code map refresh failed");
            project.set_phase(Phase::Failed { error });
        }
    }
    emit_view(app, state, project);
}

/// Whether a watcher event can change the map.
fn relevant(project: &Project, event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| {
        let Some(rel) = project.relative(path) else {
            return false;
        };
        if shodh_rag::code_map::walk::classify(&rel).is_some() {
            return true;
        }
        // A folder removed or renamed takes its files with it.
        matches!(
            event.kind,
            EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
        ) && !rel.split('/').any(|part| {
            shodh_rag::code_review::structure::SKIPPED_DIRS.contains(&part) || part.starts_with('.')
        })
    })
}

/// Start the project's watcher, if it has none.
fn start_watching(project: &Arc<Project>) -> Result<(), String> {
    let mut slot = project.watcher.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return Ok(());
    }
    let weak = Arc::downgrade(project);
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let Some(project) = weak.upgrade() else {
            return;
        };
        let changed = match &result {
            Ok(event) => relevant(&project, event),
            // Events may have been lost: look at the whole folder again.
            Err(_) => true,
        };
        if changed {
            project.request(false);
        }
    })
    .map_err(|e| format!("the code folder cannot be watched: {e}"))?;
    watcher
        .watch(&project.root, RecursiveMode::Recursive)
        .map_err(|e| format!("the code folder cannot be watched: {e}"))?;
    *slot = Some(watcher);
    Ok(())
}

/// A Code session of the project at `root` started, or the agent changed
/// files there: bring its map up to date (in the background).
pub fn touched(app: &AppHandle, root: &Path) {
    let app = app.clone();
    let root = root.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<CodeMapState>().inner().clone();
        match state.project(&root) {
            Ok(project) => schedule(&app, &state, &project, false),
            Err(e) => {
                tracing::debug!(target: "shodh::code_map", error = %e, "no code map for the folder")
            }
        }
    });
}

/// A Code session of the project at `root` started.
pub fn session_started(app: &AppHandle, root: &Path) {
    touched(app, root);
}

async fn project_of(
    app: &AppHandle,
    workspaces: &WorkspaceState,
    workspace_id: &str,
) -> Result<(CodeMapState, Arc<Project>), String> {
    let folder = workspace_code_folder(workspaces, Some(workspace_id)).await?;
    let state = app.state::<CodeMapState>().inner().clone();
    let root = folder.root().to_path_buf();
    let found = state.clone();
    let project = tokio::task::spawn_blocking(move || found.project(&root))
        .await
        .map_err(|e| e.to_string())??;
    Ok((state, project))
}

async fn view_async(
    state: CodeMapState,
    project: Arc<Project>,
    workspace_id: String,
) -> Result<CodeMapView, String> {
    tokio::task::spawn_blocking(move || view_of(&state, &project, Some(workspace_id)))
        .await
        .map_err(|e| e.to_string())
}

/// A Code conversation of the workspace is shown: build or refresh its map and
/// watch its folder until [`code_map_close`] gives back the view's `lease`.
#[tauri::command]
pub async fn code_map_open(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    workspace_id: String,
) -> Result<CodeMapView, String> {
    let (state, project) = project_of(&app, &workspaces, &workspace_id).await?;
    schedule(&app, &state, &project, false);
    let lease = {
        let (state, project, workspace) = (state.clone(), project.clone(), workspace_id.clone());
        tokio::task::spawn_blocking(move || state.lease(&project, &workspace))
            .await
            .map_err(|e| e.to_string())?
    };
    let mut view = view_async(state, project, workspace_id).await?;
    view.lease = Some(lease);
    Ok(view)
}

/// The conversation is no longer shown: give its lease back (also when its
/// workspace or folder has changed since). Unknown leases are ignored.
#[tauri::command]
pub async fn code_map_close(app: AppHandle, lease: u64) -> Result<bool, String> {
    let state = app.state::<CodeMapState>().inner().clone();
    tokio::task::spawn_blocking(move || state.release(lease))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn code_map_status(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    workspace_id: String,
) -> Result<CodeMapView, String> {
    let (state, project) = project_of(&app, &workspaces, &workspace_id).await?;
    view_async(state, project, workspace_id).await
}

/// Empty the map and read every file again.
#[tauri::command]
pub async fn code_map_rebuild(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    workspace_id: String,
) -> Result<CodeMapView, String> {
    let (state, project) = project_of(&app, &workspaces, &workspace_id).await?;
    tracing::info!(target: "shodh::code_map", project = %project.name, "code map rebuild asked for");
    schedule(&app, &state, &project, true);
    view_async(state, project, workspace_id).await
}

/// Stop the refresh running now (what it read stays; the rest is read next time).
#[tauri::command]
pub async fn code_map_cancel(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    indexing: State<'_, IndexingState>,
    workspace_id: String,
) -> Result<CodeMapView, String> {
    let (state, project) = project_of(&app, &workspaces, &workspace_id).await?;
    let stopped = indexing.cancel_source(&project.source_id());
    tracing::info!(target: "shodh::code_map", project = %project.name, stopped, "code map refresh cancelled");
    view_async(state, project, workspace_id).await
}

/// A file's symbols, for the code pane's symbol jump.
#[tauri::command]
pub async fn code_map_outline(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    workspace_id: String,
    path: String,
) -> Result<Outline, String> {
    let (state, project) = project_of(&app, &workspaces, &workspace_id).await?;
    tokio::task::spawn_blocking(move || state.map_of(&project)?.outline(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// Symbols of the project whose names start with `query`, for the symbol jump.
#[tauri::command]
pub async fn code_map_find(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    workspace_id: String,
    query: String,
) -> Result<Vec<SymbolHit>, String> {
    let query: String = query.trim().chars().take(100).collect();
    let (state, project) = project_of(&app, &workspaces, &workspace_id).await?;
    tokio::task::spawn_blocking(move || state.map_of(&project)?.symbols_matching(&query, MAX_FOUND))
        .await
        .map_err(|e| e.to_string())?
}

/// A code map refresh reading files or waiting now, for the status strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeMapRun {
    pub source_id: String,
    pub name: String,
    pub done: usize,
    pub total: usize,
    /// Why it waits (`paused`, `otherIndexing`), when it does.
    pub waiting: Option<&'static str>,
}

/// The code map refreshes reading files or waiting now.
pub fn runs(app: &AppHandle) -> Vec<CodeMapRun> {
    let Some(state) = app.try_state::<CodeMapState>() else {
        return Vec::new();
    };
    state
        .running()
        .iter()
        .filter_map(|p| match p.phase() {
            Phase::Building { done, total } => Some(CodeMapRun {
                source_id: p.source_id(),
                name: p.name.clone(),
                done,
                total,
                waiting: None,
            }),
            Phase::Waiting {
                done,
                total,
                reason,
            } => Some(CodeMapRun {
                source_id: p.source_id(),
                name: p.name.clone(),
                done,
                total,
                waiting: Some(reason),
            }),
            _ => None,
        })
        .collect()
}

/// Whether a command a Code session ran can only have read files, so the map
/// need not be refreshed after it. Conservative: anything not known to only
/// read counts as a change.
pub fn reads_only(command: &str) -> bool {
    const SEPARATORS: [&str; 6] = ["&&", "||", ";", "|", ">", "`"];
    let command = command.trim();
    if command.is_empty()
        || SEPARATORS.iter().any(|s| command.contains(s))
        || command.contains("$(")
    {
        return false;
    }
    let words: Vec<&str> = command.split_whitespace().collect();
    let program = words[0]
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(words[0])
        .trim_end_matches(".exe");
    match program {
        "ls" | "dir" | "cat" | "head" | "tail" | "wc" | "pwd" | "echo" | "grep" | "rg"
        | "which" | "type" | "file" | "stat" | "tree" | "less" | "more" => true,
        "git" => matches!(
            words.get(1).copied(),
            Some("status" | "log" | "diff" | "show" | "blame" | "grep" | "ls-files" | "rev-parse")
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    #[test]
    fn projects_are_registered_without_reading_anything() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let state = CodeMapState::new(data.path().to_path_buf(), None);
        let project = state.project(folder.path()).unwrap();
        assert_eq!(project.phase(), Phase::Idle);
        assert!(project.source_id().starts_with(CODE_MAP_SOURCE_PREFIX));
        assert!(
            !data.path().join("code-map").exists(),
            "no map file before the map is asked for"
        );
        let again = state.project(folder.path()).unwrap();
        assert!(Arc::ptr_eq(&project, &again));
        project.request(true);
        assert_eq!(project.phase(), Phase::Queued);
        assert_eq!(project.take_pending(), Some(true));
        assert_eq!(project.take_pending(), None);
        project.set_phase(Phase::Idle);
        let view = view_of(&state, &project, Some("ws".into()));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["state"], "idle");
        assert_eq!(json["workspaceId"], "ws");
        assert!(json["map"].is_null());
        let map = state.map_of(&project).unwrap();
        assert!(data.path().join("code-map").is_dir());
        assert!(Arc::ptr_eq(&map, &state.map_of(&project).unwrap()));
        project.set_phase(Phase::Waiting {
            done: 3,
            total: 9,
            reason: "otherIndexing",
        });
        let json = serde_json::to_value(view_of(&state, &project, None)).unwrap();
        assert_eq!(json["state"], "waiting");
        assert_eq!(json["reason"], "otherIndexing");
    }

    #[test]
    fn leases_start_and_stop_the_watcher_and_survive_a_changed_workspace() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let state = CodeMapState::new(data.path().to_path_buf(), None);
        let project = state.project(folder.path()).unwrap();
        let a = state.lease(&project, "ws-1");
        let b = state.lease(&project, "ws-1");
        assert!(project.is_watched());
        assert!(state.release(a));
        assert!(project.is_watched(), "another conversation still shows it");
        assert!(!state.release(a), "a lease is given back once");
        assert!(state.release(b));
        assert!(!project.is_watched());
        // A reload of the window gives every lease back.
        state.lease(&project, "ws-1");
        state.lease(&project, "ws-2");
        assert_eq!(state.release_all(), 2);
        assert_eq!(state.leases(), 0);
        assert!(!project.is_watched());
        // A deleted workspace gives its leases back, and its map file goes.
        state.lease(&project, "ws-3");
        write(folder.path(), "src/a.rs", "pub fn a() {}\n");
        state.request(&project, false, false);
        let file = CodeMap::map_path(data.path(), folder.path()).unwrap();
        assert!(file.is_file());
        state.forget("ws-3", Some(folder.path()), false);
        assert_eq!(state.leases(), 0);
        assert!(!project.is_watched());
        assert!(!file.exists(), "the map of a deleted workspace is deleted");
    }

    #[test]
    fn an_idle_map_is_closed_and_its_file_released() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        write(folder.path(), "src/a.rs", "pub fn a() {}\n");
        let state = CodeMapState::new(data.path().to_path_buf(), None);
        let project = state.project(folder.path()).unwrap();
        let lease = state.lease(&project, "ws");
        state.request(&project, false, false);
        assert!(project.is_open());
        assert!(
            !state.close_if_idle(&project, Duration::ZERO),
            "not while shown"
        );
        state.release(lease);
        assert!(state.close_if_idle(&project, Duration::ZERO));
        assert!(!project.is_open());
        // Closed means the database is closed: its file can be deleted.
        let file = CodeMap::map_path(data.path(), folder.path()).unwrap();
        CodeMap::delete_files(&file).unwrap();
        assert!(!file.exists());
    }

    /// Indexes this repository, checks what the map finds in it, and reports
    /// the speed (in CI, on the run's summary page). It measures; it is not a
    /// gate on speed. The build is a test build: Shodh's own code is
    /// unoptimized here, the grammars are not.
    #[test]
    fn the_code_map_indexes_this_repository_and_reports_its_speed() {
        use shodh_rag::code_map::SymbolKind;
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let data = tempfile::tempdir().unwrap();
        let map = CodeMap::open(data.path(), &repo).unwrap();
        let started = Instant::now();
        let first = map.refresh(&|| Pace::Go, &mut |_, _| {}).unwrap();
        let elapsed = started.elapsed();
        let status = map.status().unwrap();
        assert!(!first.stopped && !first.partial, "{first:?}");
        assert!(status.files > 500, "{status:?}");
        let found = map
            .definitions("CodeMap", Some(SymbolKind::Struct), None, 5)
            .unwrap();
        assert!(
            found
                .matches
                .iter()
                .any(|h| h.path == "crates/shodh-rag/src/code_map/mod.rs"),
            "{found:?}"
        );
        let deps = map
            .dependencies("crates/shodh-rag/src/code_map/view.rs")
            .unwrap();
        assert_eq!(deps[0].module, "shodh_rag::code_map::view");
        assert!(
            deps[0]
                .imports
                .iter()
                .any(|e| e.module == "shodh_rag::code_review::structure"),
            "{deps:?}"
        );
        let callers = map.references("code_map_tools", None, None, 20).unwrap();
        assert!(
            callers
                .resolved
                .iter()
                .any(|r| r.path == "app/src-tauri/src/agent_session_commands.rs"),
            "{callers:?}"
        );
        // `Store::open(…)` in the file that defines `CodeMap::open` is not a use of it.
        let open = map.references("CodeMap::open", None, None, 200).unwrap();
        assert!(
            !open
                .resolved
                .iter()
                .any(|r| r.qualifier.as_deref() == Some("Store")),
            "{open:?}"
        );
        // A common name: every use is looked at, and the counts are honest.
        let news = map.references("new", None, None, 12).unwrap();
        assert!(news.uses_named > 4_000, "{}", news.uses_named);
        assert_eq!(news.uses_scanned, news.uses_named, "every use is looked at");
        assert!(news.resolved.len() <= 12 && news.by_name.len() <= 12);
        assert_eq!(
            news.resolved_total + news.by_name_total,
            news.uses_named as usize,
            "each use is counted once, resolved or by name"
        );
        assert!(news.truncated);
        // A refresh that changes nothing keeps the model.
        map.model().unwrap();
        let again_started = Instant::now();
        let again = map.refresh(&|| Pace::Go, &mut |_, _| {}).unwrap();
        let again_ms = again_started.elapsed().as_millis();
        assert_eq!(again.read, 0, "nothing changed, so nothing is read again");
        assert!(
            map.model_is_current(),
            "an unchanged refresh keeps the model"
        );
        let seconds = elapsed.as_secs_f64().max(0.001);
        let line = format!(
            "Code map of this repository: {} source files ({} KB read), {} symbols and {} \
             references in {:.2} s ({:.0} files a second, test build); a refresh with nothing \
             changed took {} ms.",
            status.files,
            first.bytes_read / 1024,
            status.symbols,
            status.references,
            seconds,
            status.files as f64 / seconds,
            again_ms
        );
        // Written to the process's own stdout (libtest captures only the
        // print macros), so the Tests job's log shows it as well.
        use std::io::Write;
        let _ = writeln!(std::io::stdout().lock(), "{line}");
        if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
            if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(summary) {
                let _ = writeln!(file, "### Code map indexing speed\n\n{line}\n");
            }
        }
    }

    #[test]
    fn edits_and_commands_that_may_change_files_refresh_the_map() {
        use shodh_rag::harness::events::{AgentEvent, CommandState};
        use shodh_rag::harness::RiskTier;
        let started = |step: &str, tier: RiskTier| AgentEvent::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            parent_step_id: None,
            tool: "edit".into(),
            label: "Edit".into(),
            args: serde_json::json!({}),
            tier,
            at_ms: 0,
        };
        let finished = |step: &str| AgentEvent::StepFinished {
            run_id: "r".into(),
            step_id: step.into(),
            ok: true,
            summary: String::new(),
            detail: None,
            duration_ms: 1,
            invalid_arguments: false,
        };
        let command_started = |id: &str, command: &str| AgentEvent::CommandStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            command_id: id.into(),
            command: command.into(),
            time_limit_secs: 60,
            sandbox: String::new(),
            at_ms: 0,
        };
        let command_finished = |id: &str| AgentEvent::CommandFinished {
            run_id: "r".into(),
            step_id: "s".into(),
            command_id: id.into(),
            state: CommandState::Done,
            exit_code: Some(0),
            note: None,
            duration_ms: 5,
        };
        let refresh = crate::agent_session_commands::code_map_refresh_after;
        let mut changing = crate::agent_session_commands::ChangingSteps::default();
        assert!(!refresh(&mut changing, &started("read", RiskTier::Read)));
        assert!(
            !refresh(&mut changing, &finished("read")),
            "a read changes nothing"
        );
        assert!(!refresh(&mut changing, &started("edit", RiskTier::Write)));
        assert!(
            refresh(&mut changing, &finished("edit")),
            "an edit refreshes the map"
        );
        assert!(!refresh(
            &mut changing,
            &command_started("c1", "git status")
        ));
        assert!(
            !refresh(&mut changing, &command_finished("c1")),
            "git status only reads"
        );
        assert!(!refresh(&mut changing, &command_started("c2", "cargo fmt")));
        assert!(
            refresh(&mut changing, &command_finished("c2")),
            "a command may change files"
        );
        assert!(
            refresh(&mut changing, &command_finished("unknown")),
            "unknown: a change"
        );
        assert!(reads_only("ls -la src"));
        assert!(!reads_only("cat a > b"));
        assert!(!reads_only("git checkout main"));
        assert!(!reads_only("grep x a && rm a"));
    }

    #[test]
    fn the_watcher_ignores_build_output_and_secrets() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let state = CodeMapState::new(data.path().to_path_buf(), None);
        let project = state.project(folder.path()).unwrap();
        let event =
            |path: &str, kind: EventKind| Event::new(kind).add_path(folder.path().join(path));
        let modify = EventKind::Modify(notify::event::ModifyKind::Any);
        assert!(relevant(&project, &event("src/lib.rs", modify)));
        assert!(relevant(&project, &event("app/tsconfig.json", modify)));
        assert!(!relevant(&project, &event("target/debug/x.rs", modify)));
        assert!(!relevant(
            &project,
            &event("node_modules/a/index.js", modify)
        ));
        assert!(!relevant(&project, &event(".env", modify)));
        assert!(!relevant(&project, &event("README.md", modify)));
        assert!(relevant(
            &project,
            &event(
                "src/old_folder",
                EventKind::Remove(notify::event::RemoveKind::Folder)
            )
        ));
        assert!(!relevant(
            &project,
            &event(
                "src/a.rs",
                EventKind::Access(notify::event::AccessKind::Any)
            )
        ));
    }
}
