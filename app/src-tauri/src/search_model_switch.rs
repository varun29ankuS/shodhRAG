//! The opt-in faster search model: "Faster search (slightly less accurate)".
//!
//! The standard search model (fp16) is the default everywhere. The int8 model
//! indexes faster on processors with fast int8 but may rank some passages
//! lower, so it is only ever the user's choice, and only offered where its
//! int8 kernels are exact and measured (`e5::int8_exact`: x86 with VNNI, Apple
//! silicon). A library keeps the model it was built with; "Re-index for
//! faster search" in the System status moves it, and "Re-index for standard
//! search" moves it back (`shodh_rag::embeddings::model_switch`).
//!
//! The re-index downloads the model it moves to when missing (one download at
//! a time, shared with setup), then embeds the library's stored text again in
//! the background as an indexing run: the status strip shows it, Pause holds
//! it, Stop keeps what it did for Continue, Discard (confirmed in the UI)
//! throws it away and also stops a download. Search keeps using the old
//! index; "Restart Shodh now" first brings over what changed since the build
//! (with progress), writes the window's pending saves, and restarts, which
//! completes the switch. If completing fails, the panel says why and offers
//! Retry or Cancel. A reset or "Clear all documents" discards a switch first.
//!
//! The assistant cannot start, stop, discard or restart any of this
//! (agent-coverage exclusions): it can read the state with `system_status` and
//! open the panel on it, where the user clicks.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use shodh_rag::compute::provider::Accelerator;
use shodh_rag::compute::status::EmbedderReport;
use shodh_rag::embeddings::e5::{installed_variants, int8_exact, E5Variant, E5_DIR};
use shodh_rag::embeddings::model_store::{
    e5_model_artifact, HttpByteSource, InstallProgress, ModelStore,
};
use shodh_rag::embeddings::model_switch::{self, BuildOutcome, SwitchProgress};
use shodh_rag::embeddings::{index_model, EmbeddingModel};
use shodh_rag::indexing::IndexingState;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::indexing_control::SYSTEM_STATUS_EVENT;
use crate::rag_commands::RagState;
use crate::search_models_commands::SearchModelsState;

/// The run's source id in the indexing state (the strip's Stop stops it).
pub const SWITCH_SOURCE: &str = "search-model-switch";

/// What the strip shows for a re-index to `to`.
fn run_label(to: E5Variant) -> &'static str {
    match to {
        E5Variant::Int8 => "Re-indexing for faster search",
        E5Variant::Fp16 => "Re-indexing for standard search",
    }
}

/// Where a switch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SwitchPhase {
    /// None in progress.
    #[default]
    Idle,
    /// The model it moves to is being downloaded.
    Downloading,
    /// The library's text is being embedded again.
    Building,
    /// Stopped (from the strip, or Shodh closed) before it was done; what it
    /// embedded is kept for Continue.
    Stopped,
    /// Built: it completes when Shodh next starts.
    Ready,
    /// It failed; `error` says why. With `finishFailed`, the build was done
    /// and completing it at start failed (Retry restarts).
    Failed,
}

/// The switch, for the System status.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchStatus {
    /// The library's search model ("fp16", "int8"), once recorded.
    pub index_variant: Option<E5Variant>,
    /// The model a re-index moves to: the one in progress, else the one
    /// offered.
    pub target: Option<E5Variant>,
    /// A re-index is offered (none in progress): to int8 for an fp16 library
    /// searched on a processor where int8 is exact; back to fp16 for an int8
    /// library.
    pub offered: bool,
    /// The library is int8 but this processor cannot run int8 exactly:
    /// nothing new is indexed until it moves back to the standard model.
    pub writes_blocked: bool,
    pub phase: SwitchPhase,
    /// Completing the built switch at start failed (see `error`).
    pub finish_failed: bool,
    /// The restart waits for the changes since the build to be brought over.
    pub restarting: bool,
    /// Rows embedded (or found unchanged) of all the rows to embed.
    pub done: u64,
    pub total: u64,
    /// Time left at the speed so far, once there is one.
    pub eta_seconds: Option<u64>,
    /// The download, while it runs.
    pub downloaded_bytes: u64,
    pub download_bytes: u64,
    /// The size of the target model's download when it is not installed.
    pub model_bytes: u64,
    /// The plain reason of a failure.
    pub error: Option<String>,
    /// The technical detail behind it, for "Show details".
    pub error_detail: Option<String>,
}

#[derive(Debug, Default)]
struct Inner {
    running: bool,
    /// Stop or Discard was asked: a download in progress ends.
    stop_requested: bool,
    /// A Discard asked to throw away what was embedded.
    discard: bool,
    /// Restart Shodh once the run has built the switch.
    restart_after: bool,
    phase: SwitchPhase,
    progress: SwitchProgress,
    /// When building started, and how far it was then (for the speed).
    building_since: Option<(Instant, u64)>,
    downloaded: (u64, u64),
    error: Option<String>,
}

/// Managed state: the switch running in this session, if any.
#[derive(Debug)]
pub struct SearchModelSwitchState {
    inner: std::sync::Mutex<Inner>,
    /// Wakes a download to stop it, on Stop or Discard.
    stop: tokio::sync::Notify,
    /// Whether a run is going, for waiting on its end.
    running: tokio::sync::watch::Sender<bool>,
}

impl Default for SearchModelSwitchState {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            stop: tokio::sync::Notify::new(),
            running: tokio::sync::watch::Sender::new(false),
        }
    }
}

impl SearchModelSwitchState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Claims the run (none may be going): `true` when this call may start it.
    fn claim(&self) -> bool {
        let mut inner = self.lock();
        if inner.running {
            return false;
        }
        *inner = Inner {
            running: true,
            phase: SwitchPhase::Downloading,
            ..Inner::default()
        };
        drop(inner);
        self.running.send_replace(true);
        true
    }

    /// The run ended in `result`.
    fn release(&self, result: Result<SwitchPhase, String>) {
        let mut inner = self.lock();
        inner.running = false;
        inner.discard = false;
        inner.restart_after = false;
        match result {
            Ok(phase) => inner.phase = phase,
            Err(error) => {
                tracing::warn!(%error, "Re-indexing the library failed");
                inner.phase = SwitchPhase::Failed;
                inner.error = Some(error);
            }
        }
        drop(inner);
        self.running.send_replace(false);
    }

    /// Marks a running run as asked to stop (nothing when none runs).
    fn request_stop(&self) {
        let mut inner = self.lock();
        if inner.running {
            inner.stop_requested = true;
        }
    }

    /// Waits until no run is going.
    async fn wait_until_idle(&self) {
        let mut running = self.running.subscribe();
        // The sender lives in `self`, so the wait ends only when a run does.
        let _ = running.wait_for(|running| !*running).await;
    }
}

/// Seconds left: `left` rows at the rate of `done` rows in `elapsed`.
fn eta(done: u64, left: u64, elapsed: Duration) -> Option<u64> {
    if done == 0 || elapsed.is_zero() {
        return None;
    }
    let rate = done as f64 / elapsed.as_secs_f64();
    Some((left as f64 / rate).ceil() as u64)
}

/// The re-index offered for a library of `index`: to int8 only where it runs
/// exactly (`exact`) and search ran on the processor (`on_cpu`; on a
/// graphics card fp16 is the faster), and always back to fp16 from int8.
fn offered_target(index: Option<E5Variant>, exact: bool, on_cpu: bool) -> Option<E5Variant> {
    match index? {
        E5Variant::Fp16 if exact && on_cpu => Some(E5Variant::Int8),
        E5Variant::Fp16 => None,
        E5Variant::Int8 => Some(E5Variant::Fp16),
    }
}

/// Where a switch is, from this session's run (`running` with its phase,
/// or the last run's `last` phase) and the switch on disk (`pending`: whether
/// it is built, and why completing it failed).
fn phase_of(
    running: Option<SwitchPhase>,
    last: SwitchPhase,
    pending: Option<(bool, bool)>,
) -> SwitchPhase {
    if let Some(phase) = running {
        return phase;
    }
    match pending {
        Some((true, true)) => SwitchPhase::Failed,
        Some((true, false)) => SwitchPhase::Ready,
        _ if last == SwitchPhase::Failed => SwitchPhase::Failed,
        Some(_) => SwitchPhase::Stopped,
        None => SwitchPhase::Idle,
    }
}

/// A completion failure's plain sentence and its technical detail.
fn split_failure(reason: &str) -> (String, Option<String>) {
    match reason.split_once(" Details: ") {
        Some((plain, detail)) => (plain.to_string(), Some(detail.to_string())),
        None => (reason.to_string(), None),
    }
}

/// The switch as the System status shows it. `report` is what last ran the
/// search model.
pub async fn status(
    state: &SearchModelSwitchState,
    rag_state: &RagState,
    report: Option<&EmbedderReport>,
) -> SwitchStatus {
    let (data_dir, model_dir) = {
        let engine = rag_state.rag.read().await;
        (
            engine.config().data_dir.clone(),
            engine.config().embedding.model_dir.clone(),
        )
    };
    let index_variant = index_model::read(&data_dir)
        .ok()
        .flatten()
        .map(|r| r.variant);
    let pending = model_switch::pending(&data_dir).ok().flatten();
    let installed = installed_variants(&model_dir.join(E5_DIR));
    let on_cpu = report.is_some_and(|r| r.accelerator == Accelerator::Cpu);
    // The int8 check loads ONNX Runtime (about 10 MB), so it runs only when
    // the answer matters: an fp16 library searched on the processor (the
    // search model is loaded then anyway) or an int8 library.
    let exact = match index_variant {
        Some(E5Variant::Fp16) => on_cpu && int8_exact(),
        Some(E5Variant::Int8) => int8_exact(),
        None => false,
    };
    let inner = state.lock();
    let phase = phase_of(
        inner.running.then_some(inner.phase),
        inner.phase,
        pending
            .as_ref()
            .map(|p| (p.built, p.finish_error.is_some())),
    );
    let finish_failed = !inner.running
        && pending
            .as_ref()
            .is_some_and(|p| p.built && p.finish_error.is_some());
    let offer = offered_target(index_variant, exact, on_cpu);
    let target = pending.as_ref().map(|p| p.to).or(offer);
    let offered = phase == SwitchPhase::Idle && pending.is_none() && offer.is_some();
    let eta_seconds = match (phase, inner.building_since) {
        (SwitchPhase::Building, Some((since, at_start))) => eta(
            inner.progress.done.saturating_sub(at_start),
            inner.progress.total.saturating_sub(inner.progress.done),
            since.elapsed(),
        ),
        _ => None,
    };
    let (error, error_detail) = if finish_failed {
        match pending.as_ref().and_then(|p| p.finish_error.as_deref()) {
            Some(reason) => {
                let (plain, detail) = split_failure(reason);
                (Some(plain), detail)
            }
            None => (None, None),
        }
    } else if phase == SwitchPhase::Failed {
        (inner.error.clone(), None)
    } else {
        (None, None)
    };
    SwitchStatus {
        index_variant,
        target,
        offered,
        writes_blocked: index_variant == Some(E5Variant::Int8) && !exact,
        phase,
        finish_failed,
        restarting: inner.running && inner.restart_after,
        done: inner.progress.done,
        total: inner.progress.total,
        eta_seconds,
        downloaded_bytes: inner.downloaded.0,
        download_bytes: inner.downloaded.1,
        model_bytes: match target {
            Some(t) if !installed.contains(&t) => e5_model_artifact(t).size,
            _ => 0,
        },
        error,
        error_detail,
    }
}

fn changed(app: &AppHandle) {
    if let Err(e) = app.emit(SYSTEM_STATUS_EVENT, ()) {
        tracing::debug!("Failed to emit {}: {}", SYSTEM_STATUS_EVENT, e);
    }
}

const NOT_EXACT: &str =
    "This processor cannot run the faster search model exactly, so Shodh keeps the standard one.";

async fn current_status(state: &SearchModelSwitchState, rag_state: &RagState) -> SwitchStatus {
    let report = rag_state.rag.read().await.compute_status().embedder();
    status(state, rag_state, report.as_ref()).await
}

/// Downloads (or verifies) the model of `variant` into `model_dir`, one
/// download at a time (setup's lock), reporting the bytes to the switch
/// state. Stop or Discard ends it at once (the partial file is kept for a
/// later resume); returns `Ok(false)` then.
async fn install_variant(
    app: &AppHandle,
    model_dir: &Path,
    variant: E5Variant,
) -> Result<bool, String> {
    let installs = app.state::<SearchModelsState>();
    let state = app.state::<SearchModelSwitchState>();
    // Registered before the flag is read, so a Stop between the two still
    // wakes it.
    let stopping = state.stop.notified();
    tokio::pin!(stopping);
    stopping.as_mut().enable();
    if state.lock().stop_requested {
        return Ok(false);
    }
    let _one_download = tokio::select! {
        guard = installs.install_lock.lock() => guard,
        () = &mut stopping => return Ok(false),
    };
    let store =
        ModelStore::with_artifacts(model_dir.to_path_buf(), vec![e5_model_artifact(variant)]);
    let source = HttpByteSource::new().map_err(|e| e.to_string())?;
    let progress_app = app.clone();
    let on_download = move |p: &InstallProgress| {
        progress_app
            .state::<SearchModelSwitchState>()
            .lock()
            .downloaded = (p.overall_bytes, p.overall_total);
    };
    tokio::select! {
        installed = store.install(&source, &on_download) => installed
            .map(|_| true)
            .map_err(|e| format!("Downloading the search model failed: {e}")),
        () = &mut stopping => Ok(false),
    }
}

/// The model a re-index moves to: the pending switch's, else the other one.
fn target_of(data_dir: &Path) -> Result<E5Variant, String> {
    let record = index_model::read(data_dir)
        .map_err(|e| format!("{e:#}"))?
        .ok_or("This library's search model is not known yet; search once, then try again.")?;
    Ok(
        match model_switch::pending(data_dir).map_err(|e| format!("{e:#}"))? {
            Some(pending) => pending.to,
            None => match record.variant {
                E5Variant::Fp16 => E5Variant::Int8,
                E5Variant::Int8 => E5Variant::Fp16,
            },
        },
    )
}

/// Starts the re-index (or continues a stopped one, or retries one that
/// failed) in the background: to the faster model for a standard library,
/// back to the standard model for a faster one.
#[tauri::command]
pub async fn start_search_model_switch(
    app: AppHandle,
    state: State<'_, SearchModelSwitchState>,
    rag_state: State<'_, RagState>,
) -> Result<SwitchStatus, String> {
    let data_dir = rag_state.rag.read().await.config().data_dir.clone();
    let target = target_of(&data_dir)?;
    if target == E5Variant::Int8 && !int8_exact() {
        return Err(NOT_EXACT.into());
    }
    if !state.claim() {
        return Err("A re-index of the library is already running.".into());
    }
    let task = app.clone();
    tauri::async_runtime::spawn(async move { run(task, target).await });
    changed(&app);
    Ok(current_status(&state, &rag_state).await)
}

/// Stops the re-index and keeps what it did, for Continue (a download in
/// progress stops too; its partial file is kept).
#[tauri::command]
pub async fn stop_search_model_switch(
    app: AppHandle,
    state: State<'_, SearchModelSwitchState>,
    rag_state: State<'_, RagState>,
    indexing: State<'_, IndexingState>,
) -> Result<SwitchStatus, String> {
    state.request_stop();
    indexing.cancel_source(SWITCH_SOURCE);
    state.stop.notify_waiters();
    tracing::info!("Re-indexing the library stopped");
    changed(&app);
    Ok(current_status(&state, &rag_state).await)
}

/// Discards the re-index: a running one (or its download) stops before its
/// next step and what it embedded is thrown away; a built switch, also one
/// whose completion failed, is cancelled. The UI asks first.
#[tauri::command]
pub async fn cancel_search_model_switch(
    app: AppHandle,
    state: State<'_, SearchModelSwitchState>,
    rag_state: State<'_, RagState>,
    indexing: State<'_, IndexingState>,
) -> Result<SwitchStatus, String> {
    let data_dir = rag_state.rag.read().await.config().data_dir.clone();
    discard(&state, &indexing, &data_dir).await?;
    tracing::info!("Re-indexing the library discarded");
    changed(&app);
    Ok(current_status(&state, &rag_state).await)
}

/// Discards the switch in `data_dir`: a running one is told to discard and
/// waited for; then the switch on disk is cancelled. Either way no switch
/// table is left.
async fn discard(
    state: &SearchModelSwitchState,
    indexing: &IndexingState,
    data_dir: &Path,
) -> Result<(), String> {
    let running = {
        let mut inner = state.lock();
        if inner.running {
            inner.discard = true;
            inner.stop_requested = true;
        }
        inner.running
    };
    if running {
        indexing.cancel_source(SWITCH_SOURCE);
        state.stop.notify_waiters();
        state.wait_until_idle().await;
    }
    model_switch::cancel(data_dir)
        .await
        .map_err(|e| format!("Discarding the re-index failed: {e:#}"))?;
    let mut inner = state.lock();
    inner.phase = SwitchPhase::Idle;
    inner.error = None;
    Ok(())
}

/// Before a reset or "Clear all documents": the switch (which holds a copy
/// of the library) is discarded and waited for, so nothing of it stays on
/// disk.
pub async fn discard_for_reset(app: &AppHandle) -> Result<(), String> {
    let Some(state) = app.try_state::<SearchModelSwitchState>() else {
        return Ok(());
    };
    let data_dir = app
        .state::<RagState>()
        .rag
        .read()
        .await
        .config()
        .data_dir
        .clone();
    discard(&state, &app.state::<IndexingState>(), &data_dir).await?;
    changed(app);
    Ok(())
}

/// Restarts Shodh to complete a built switch: first the changes since the
/// build are brought over in the background (shown as building), so the
/// start embeds little; then the window's pending saves are written and the
/// agents shut down, as for an update; then the restart.
#[tauri::command]
pub async fn restart_for_search_model_switch(
    app: AppHandle,
    state: State<'_, SearchModelSwitchState>,
    rag_state: State<'_, RagState>,
) -> Result<SwitchStatus, String> {
    let data_dir = rag_state.rag.read().await.config().data_dir.clone();
    let pending = model_switch::pending(&data_dir).map_err(|e| format!("{e:#}"))?;
    match pending {
        // A failed completion is retried by the start itself.
        Some(switch) if switch.built && switch.finish_error.is_none() => {
            if !state.claim() {
                return Err("A re-index of the library is already running.".into());
            }
            state.lock().restart_after = true;
            let task = app.clone();
            tauri::async_runtime::spawn(async move { run(task, switch.to).await });
            changed(&app);
            Ok(current_status(&state, &rag_state).await)
        }
        _ => {
            restart_now(&app).await;
            Ok(current_status(&state, &rag_state).await)
        }
    }
}

/// Writes the window's waiting saves and shuts the agents down, then
/// restarts (the start completes the switch).
async fn restart_now(app: &AppHandle) {
    tracing::info!("Restarting to switch the search model");
    crate::shutdown::prepare_exit(app).await;
    app.state::<crate::agent_session_commands::AgentSessions>()
        .shutdown_all()
        .await;
    app.restart();
}

/// At start: a re-index Shodh closed in the middle of goes on (it waits
/// while indexing is paused), unless it was cancelled meanwhile.
pub fn resume_at_start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let data_dir = app
            .state::<RagState>()
            .rag
            .read()
            .await
            .config()
            .data_dir
            .clone();
        let resumed = claim_resume(&app.state::<SearchModelSwitchState>(), &data_dir);
        if let Some(to) = resumed {
            tracing::info!(to = to.label(), "Re-indexing the library goes on");
            run(app.clone(), to).await;
        }
    });
}

/// Claims the run for a switch on disk that is not built, reading the marker
/// again once the run is claimed (a cancel between the two reads wins).
fn claim_resume(state: &SearchModelSwitchState, data_dir: &Path) -> Option<E5Variant> {
    let unbuilt = |dir: &Path| match model_switch::pending(dir) {
        Ok(Some(switch)) if !switch.built => Some(switch.to),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "The search model switch could not be read");
            None
        }
    };
    unbuilt(data_dir)?;
    if !state.claim() {
        return None;
    }
    match unbuilt(data_dir) {
        Some(to) => Some(to),
        None => {
            state.release(Ok(SwitchPhase::Idle));
            None
        }
    }
}

async fn run(app: AppHandle, to: E5Variant) {
    let result = run_switch(&app, to).await;
    let restart = {
        let state = app.state::<SearchModelSwitchState>();
        let restart = state.lock().restart_after && result == Ok(SwitchPhase::Ready);
        state.release(result);
        restart
    };
    changed(&app);
    if restart {
        restart_now(&app).await;
    }
}

fn discarding(app: &AppHandle) -> bool {
    app.state::<SearchModelSwitchState>().lock().discard
}

/// What a run that stopped early ends as: discarded (the switch cancelled)
/// or stopped (its work kept).
async fn after_stop(app: &AppHandle, data_dir: &Path) -> Result<SwitchPhase, String> {
    if discarding(app) {
        model_switch::cancel(data_dir)
            .await
            .map_err(|e| format!("{e:#}"))?;
        return Ok(SwitchPhase::Idle);
    }
    Ok(SwitchPhase::Stopped)
}

async fn run_switch(app: &AppHandle, to: E5Variant) -> Result<SwitchPhase, String> {
    if to == E5Variant::Int8 && !int8_exact() {
        return Err(NOT_EXACT.into());
    }
    let indexing = app.state::<IndexingState>();
    let run = indexing
        .begin_run_at(SWITCH_SOURCE, run_label(to))
        .ok_or("The re-index could not start.")?;
    changed(app);
    let (data_dir, model_dir) = {
        let rag_state = app.state::<RagState>();
        let engine = rag_state.rag.read().await;
        (
            engine.config().data_dir.clone(),
            engine.config().embedding.model_dir.clone(),
        )
    };

    // The model it moves to, verified (kept when it is already installed).
    let downloaded = install_variant(app, &model_dir, to).await?;
    if !downloaded || discarding(app) || run.stopped() {
        drop(run);
        return after_stop(app, &data_dir).await;
    }

    model_switch::start(&data_dir, to)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let embedder: Arc<dyn EmbeddingModel> = {
        let model_dir = model_dir.clone();
        tokio::task::spawn_blocking(move || shodh_rag::rag_engine::switch_model(&model_dir, to))
            .await
            .map_err(|e| format!("Loading the search model failed: {e}"))?
            .map_err(|e| format!("Loading the search model failed: {e:#}"))?
    };
    app.state::<SearchModelSwitchState>().lock().phase = SwitchPhase::Building;
    changed(app);
    let progress_app = app.clone();
    let on_progress = move |p: SwitchProgress| {
        let state = progress_app.state::<SearchModelSwitchState>();
        let mut inner = state.lock();
        inner.progress = p;
        if inner.building_since.is_none() && p.total > 0 {
            inner.building_since = Some((Instant::now(), p.done));
        }
    };
    let outcome = model_switch::build(
        &data_dir,
        embedder,
        || run.wait_while_paused(),
        &on_progress,
    )
    .await
    .map_err(|e| format!("{e:#}"))?;
    drop(run);
    match outcome {
        BuildOutcome::Built => Ok(SwitchPhase::Ready),
        BuildOutcome::Stopped => after_stop(app, &data_dir).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_follows_the_speed_so_far() {
        assert_eq!(eta(0, 100, Duration::from_secs(10)), None);
        assert_eq!(eta(10, 100, Duration::ZERO), None);
        assert_eq!(eta(50, 100, Duration::from_secs(10)), Some(20));
        assert_eq!(eta(30, 0, Duration::from_secs(10)), Some(0));
    }

    /// The faster model is offered only where it runs exactly and search runs
    /// on the processor; the way back to the standard model always is.
    #[test]
    fn the_re_index_offered_follows_the_library_and_the_processor() {
        use E5Variant::{Fp16, Int8};
        assert_eq!(offered_target(Some(Fp16), true, true), Some(Int8));
        assert_eq!(offered_target(Some(Fp16), false, true), None, "not exact");
        assert_eq!(offered_target(Some(Fp16), true, false), None, "on a card");
        for (exact, on_cpu) in [(true, true), (false, false), (false, true)] {
            assert_eq!(offered_target(Some(Int8), exact, on_cpu), Some(Fp16));
        }
        assert_eq!(offered_target(None, true, true), None, "no library yet");
    }

    /// A completion that failed at start is shown as failed (with Retry and
    /// Cancel), never as "ready, restart"; its plain sentence leads.
    #[test]
    fn a_failed_completion_is_never_shown_as_ready() {
        use SwitchPhase::*;
        assert_eq!(phase_of(None, Idle, Some((true, true))), Failed);
        assert_eq!(phase_of(None, Idle, Some((true, false))), Ready);
        assert_eq!(phase_of(None, Idle, Some((false, false))), Stopped);
        assert_eq!(phase_of(None, Failed, Some((false, false))), Failed);
        assert_eq!(phase_of(None, Failed, None), Failed);
        assert_eq!(phase_of(None, Idle, None), Idle);
        assert_eq!(phase_of(Some(Building), Idle, Some((true, true))), Building);
        let (plain, detail) =
            split_failure("Shodh could not finish (x), so it kept. Details: y: z");
        assert_eq!(plain, "Shodh could not finish (x), so it kept.");
        assert_eq!(detail.as_deref(), Some("y: z"));
        assert_eq!(split_failure("plain").1, None);
    }

    /// A switch cancelled between the start's first look and its claim is not
    /// resumed (and does not hold the run); one still there is.
    #[test]
    fn a_switch_cancelled_at_start_is_not_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let state = SearchModelSwitchState::default();
        assert_eq!(claim_resume(&state, dir.path()), None, "no switch");
        index_model::write(dir.path(), E5Variant::Fp16, "built before records").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(model_switch::start(dir.path(), E5Variant::Int8))
            .unwrap();
        assert_eq!(claim_resume(&state, dir.path()), Some(E5Variant::Int8));
        assert!(!state.claim(), "the run is held");
        state.release(Ok(SwitchPhase::Stopped));
        runtime.block_on(model_switch::cancel(dir.path())).unwrap();
        assert_eq!(claim_resume(&state, dir.path()), None);
        assert!(state.claim(), "a cancelled switch does not hold the run");
    }

    /// A discard (before a reset) cancels the switch on disk, so nothing of
    /// it is left; with a run going it first waits for the run to end.
    #[test]
    fn a_discard_cancels_the_switch_and_waits_for_a_run() {
        let dir = tempfile::tempdir().unwrap();
        index_model::write(dir.path(), E5Variant::Fp16, "built before records").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            model_switch::start(dir.path(), E5Variant::Int8)
                .await
                .unwrap();
            let state = Arc::new(SearchModelSwitchState::default());
            let indexing = IndexingState::default();
            discard(&state, &indexing, dir.path()).await.unwrap();
            assert_eq!(model_switch::pending(dir.path()).unwrap(), None);

            model_switch::start(dir.path(), E5Variant::Int8)
                .await
                .unwrap();
            assert!(state.claim());
            let ending = state.clone();
            let ended = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                assert!(ending.lock().discard, "the run was told to discard");
                ending.release(Ok(SwitchPhase::Idle));
            });
            discard(&state, &indexing, dir.path()).await.unwrap();
            ended.await.unwrap();
            assert!(!state.lock().running);
            assert_eq!(model_switch::pending(dir.path()).unwrap(), None);
        });
    }

    #[test]
    fn the_status_serializes_in_camel_case() {
        let status = SwitchStatus {
            index_variant: Some(E5Variant::Fp16),
            target: Some(E5Variant::Int8),
            offered: true,
            phase: SwitchPhase::Building,
            done: 3,
            total: 9,
            eta_seconds: Some(12),
            model_bytes: 278_686_411,
            ..SwitchStatus::default()
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["indexVariant"], "fp16");
        assert_eq!(json["target"], "int8");
        assert_eq!(json["phase"], "building");
        assert_eq!(json["etaSeconds"], 12);
        assert_eq!(json["modelBytes"], 278_686_411u64);
        assert_eq!(json["writesBlocked"], false);
        assert_eq!(json["finishFailed"], false);
        assert!(json["errorDetail"].is_null());
    }
}
