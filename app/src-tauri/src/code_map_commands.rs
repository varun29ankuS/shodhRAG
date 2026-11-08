//! Code mode's code map in the app: one map per Code project
//! ([`shodh_rag::code_map::CodeMap`]), built in the background when the
//! project opens in Code mode and kept current from then on.
//!
//! - **When it refreshes**: when a Code conversation of the project is shown
//!   ([`code_map_open`]), when one of its sessions starts, after the agent's
//!   edits and commands ([`touched`]), and on a file change while the project
//!   is shown (a watcher, dropped with the last view). Requests that come close
//!   together run as one refresh ([`DEBOUNCE`]); a refresh reads only files
//!   whose size, time or contents changed.
//! - **How it runs**: each refresh is an indexing run
//!   ([`IndexingState::begin_run_at`]) under the source `code-map:<key>`, so
//!   the status strip shows it and its Pause and Cancel hold and stop it. It
//!   also waits while any other indexing runs: the Library goes first.
//! - **What it costs at rest**: nothing. The state holds no file open until a
//!   Code project asks for its map; no watcher runs while no Code project is
//!   shown.
//!
//! The agent reads the map through Code sessions' `code_definition`,
//! `code_references`, `code_outline` and `code_dependencies`
//! ([`crate::agent_tools::code_map_tools`]); the UI through the commands here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
/// Requests this close together run as one refresh.
pub const DEBOUNCE: Duration = Duration::from_millis(1_200);
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
    /// A refresh is asked for and starts after a short wait.
    Queued,
    /// Reading files: `done` of `total` looked at.
    Building {
        done: usize,
        total: usize,
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

#[derive(Default)]
struct WatchSlot {
    users: usize,
    watcher: Option<RecommendedWatcher>,
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
    watch: Mutex<WatchSlot>,
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

    /// Ask for a refresh (the worker runs it after [`DEBOUNCE`]).
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

    fn is_watched(&self) -> bool {
        self.watch
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .watcher
            .is_some()
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

/// Every Code project's map, opened when first asked for.
#[derive(Clone)]
pub struct CodeMapState {
    inner: Arc<Inner>,
}

struct Inner {
    data_dir: PathBuf,
    /// The indexing pause (the strip's and the tray's), which also holds a
    /// refresh the agent's tools would run.
    pause: Option<PauseSwitch>,
    projects: Mutex<HashMap<String, Arc<Project>>>,
}

impl CodeMapState {
    pub fn new(data_dir: PathBuf, pause: Option<PauseSwitch>) -> Self {
        CodeMapState {
            inner: Arc::new(Inner {
                data_dir,
                pause,
                projects: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Indexing is paused by the user.
    pub fn paused(&self) -> bool {
        self.inner
            .pause
            .as_ref()
            .is_some_and(PauseSwitch::is_paused)
    }

    /// The project at `root` (registered on first use; nothing is read yet).
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
                    watch: Mutex::new(WatchSlot::default()),
                })
            })
            .clone())
    }

    /// The project's map, opened (or created) on first use. Blocking.
    pub fn map_of(&self, project: &Project) -> Result<Arc<CodeMap>, String> {
        let mut slot = project.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(map) = slot.as_ref() {
            return Ok(map.clone());
        }
        let map = Arc::new(CodeMap::open(&self.inner.data_dir, &project.root)?);
        *slot = Some(map.clone());
        Ok(map)
    }

    /// The map of the project at `root`, opened on a blocking thread.
    pub async fn map_for(&self, root: &Path) -> Result<Arc<CodeMap>, String> {
        let state = self.clone();
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let project = state.project(&root)?;
            state.map_of(&project)
        })
        .await
        .map_err(|e| format!("opening the code map did not finish: {e}"))?
    }

    /// Every project with a refresh reading files now.
    pub fn building(&self) -> Vec<Arc<Project>> {
        let projects = self
            .inner
            .projects
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<Arc<Project>> = projects
            .values()
            .filter(|p| matches!(p.phase(), Phase::Building { .. }))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
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
    /// What the map holds; `None` before it is opened.
    pub map: Option<MapStatus>,
    pub last_run: Option<RefreshReport>,
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
    }
}

fn emit_view(app: &AppHandle, state: &CodeMapState, project: &Project) {
    let view = view_of(state, project, None);
    if let Err(e) = app.emit(CODE_MAP_EVENT, &view) {
        tracing::debug!("Failed to emit {CODE_MAP_EVENT}: {e}");
    }
}

/// Ask for a refresh of `project`, starting its worker on first use.
pub fn schedule(app: &AppHandle, state: &CodeMapState, project: &Arc<Project>, rebuild: bool) {
    project.request(rebuild);
    if !project.worker.swap(true, Ordering::SeqCst) {
        let app = app.clone();
        let state = state.clone();
        let project = project.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                project.wake.notified().await;
                tokio::time::sleep(DEBOUNCE).await;
                let Some(rebuild) = project.take_pending() else {
                    continue;
                };
                run_refresh(&app, &state, &project, rebuild).await;
            }
        });
    }
    emit_view(app, state, project);
}

/// Another indexing run (not a code map) is in progress: the map waits.
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
    let root_text = project.root.display().to_string();
    let Some(run) = indexing.begin_run_at(&project.source_id(), &root_text) else {
        project.set_phase(Phase::Idle);
        return;
    };
    project.set_phase(Phase::Building { done: 0, total: 0 });
    emit_view(app, state, project);
    let control = run.control();
    let pace_app = app.clone();
    let progress_app = app.clone();
    let progress_state = state.clone();
    let progress_project = project.clone();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let pace = move || {
            if control.stopped() {
                Pace::Stop
            } else if control.is_paused() || others_indexing(&pace_app) {
                Pace::Hold
            } else {
                Pace::Go
            }
        };
        let mut last = Instant::now();
        let mut progress = move |done: usize, total: usize| {
            progress_project.set_phase(Phase::Building { done, total });
            if last.elapsed() >= PROGRESS_EVERY || done == total {
                last = Instant::now();
                emit_view(&progress_app, &progress_state, &progress_project);
            }
        };
        map.refresh(&pace, &mut progress)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    drop(run);
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

/// Start watching `project` for changes (once per viewer; the watcher runs
/// while anyone looks).
fn watch_acquire(project: &Arc<Project>) -> Result<(), String> {
    let mut slot = project.watch.lock().unwrap_or_else(|e| e.into_inner());
    slot.users += 1;
    if slot.watcher.is_some() {
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
    slot.watcher = Some(watcher);
    Ok(())
}

fn watch_release(project: &Project) {
    let mut slot = project.watch.lock().unwrap_or_else(|e| e.into_inner());
    slot.users = slot.users.saturating_sub(1);
    if slot.users == 0 {
        slot.watcher = None;
    }
}

/// A Code session of the project at `root` started: bring its map up to date.
pub fn session_started(app: &AppHandle, root: &Path) {
    let state = app.state::<CodeMapState>().inner().clone();
    match state.project(root) {
        Ok(project) => schedule(app, &state, &project, false),
        Err(e) => {
            tracing::debug!(target: "shodh::code_map", error = %e, "no code map for the session")
        }
    }
}

/// The agent edited files or ran a command in the project at `root`.
pub fn touched(app: &AppHandle, root: &Path) {
    session_started(app, root);
}

async fn project_of(
    app: &AppHandle,
    workspaces: &WorkspaceState,
    workspace_id: &str,
) -> Result<(CodeMapState, Arc<Project>), String> {
    let folder = workspace_code_folder(workspaces, Some(workspace_id)).await?;
    let state = app.state::<CodeMapState>().inner().clone();
    let project = state.project(folder.root())?;
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
/// watch its folder until [`code_map_close`].
#[tauri::command]
pub async fn code_map_open(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    workspace_id: String,
) -> Result<CodeMapView, String> {
    let (state, project) = project_of(&app, &workspaces, &workspace_id).await?;
    schedule(&app, &state, &project, false);
    if let Err(e) = watch_acquire(&project) {
        tracing::warn!(target: "shodh::code_map", project = %project.name, error = %e, "file changes are not watched");
    }
    view_async(state, project, workspace_id).await
}

/// The Code conversation is no longer shown: stop watching when no other is.
#[tauri::command]
pub async fn code_map_close(
    app: AppHandle,
    workspaces: State<'_, WorkspaceState>,
    workspace_id: String,
) -> Result<(), String> {
    let (_, project) = project_of(&app, &workspaces, &workspace_id).await?;
    watch_release(&project);
    Ok(())
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

/// A code map refresh reading files now, for the status strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeMapRun {
    pub source_id: String,
    pub name: String,
    pub done: usize,
    pub total: usize,
}

/// The code map refreshes reading files now.
pub fn runs(app: &AppHandle) -> Vec<CodeMapRun> {
    let Some(state) = app.try_state::<CodeMapState>() else {
        return Vec::new();
    };
    state
        .building()
        .iter()
        .filter_map(|p| match p.phase() {
            Phase::Building { done, total } => Some(CodeMapRun {
                source_id: p.source_id(),
                name: p.name.clone(),
                done,
                total,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let view = view_of(&state, &project, Some("ws".into()));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["state"], "queued");
        assert_eq!(json["workspaceId"], "ws");
        assert!(json["map"].is_null());
        let map = state.map_of(&project).unwrap();
        assert!(data.path().join("code-map").is_dir());
        assert!(Arc::ptr_eq(&map, &state.map_of(&project).unwrap()));
    }

    /// Indexes this repository, checks what the map finds in it, and reports
    /// the speed (in CI, on the run's summary page). The build is a test
    /// build: Shodh's own code is unoptimized here, the grammars are not.
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
        let callers = map.references("code_map_tools", None, 20).unwrap();
        assert!(
            callers
                .resolved
                .iter()
                .any(|r| r.path == "app/src-tauri/src/agent_session_commands.rs"),
            "{callers:?}"
        );
        let again_started = Instant::now();
        let again = map.refresh(&|| Pace::Go, &mut |_, _| {}).unwrap();
        let again_ms = again_started.elapsed().as_millis();
        assert_eq!(again.read, 0, "nothing changed, so nothing is read again");
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
        println!("{line}");
        if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(summary) {
                let _ = writeln!(file, "### Code map indexing speed\n\n{line}\n");
            }
        }
    }

    #[test]
    fn edits_and_commands_refresh_the_map_and_reads_do_not() {
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
        let refresh = crate::agent_session_commands::code_map_refresh_after;
        let mut changing = std::collections::HashSet::new();
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
        assert!(changing.is_empty());
        let command = AgentEvent::CommandFinished {
            run_id: "r".into(),
            step_id: "s".into(),
            command_id: "c".into(),
            state: CommandState::Done,
            exit_code: Some(0),
            note: None,
            duration_ms: 5,
        };
        assert!(
            refresh(&mut changing, &command),
            "a command may have changed files"
        );
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
