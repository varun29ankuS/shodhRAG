//! The opt-in faster search model: "Faster search (slightly less accurate)".
//!
//! The standard search model (fp16) is the default everywhere. The int8 model
//! indexes and searches about 3x faster on the processor but may rank some
//! passages lower, so it is only ever the user's choice, and only offered on
//! processors that run it exactly (`e5::int8_exact`: VNNI on x86, ARM64):
//! - for a library without vectors yet, in Settings → Search
//!   ([`set_faster_search_for_new_libraries`]);
//! - for a library that has vectors, "Re-index for faster search" in the
//!   System status, and "Re-index for standard search" to go back.
//!
//! A library keeps the model it was built with, so moving one embeds its
//! stored text again (`shodh_rag::embeddings::model_switch`). The model is
//! downloaded only then (279 MB for int8). The re-index is an indexing run:
//! the status strip shows it, Pause holds it, Cancel stops it. Search keeps
//! using the old index; the switch completes when Shodh next starts, and the
//! panel offers the restart. If completing fails, the panel says why and
//! offers Retry (another restart) or Cancel.
//!
//! The assistant cannot start, cancel or restart any of this (agent-coverage
//! exclusions): it can read the state with `system_status` and open the
//! panel on it, where the user clicks.

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

use crate::app_settings::broadcast;
use crate::indexing_control::{settings_store, SYSTEM_STATUS_EVENT};
use crate::rag_commands::RagState;

/// The run's source id in the indexing state (the strip's Cancel stops it).
pub const SWITCH_SOURCE: &str = "search-model-switch";

/// What the strip shows for a re-index to `to`.
fn run_label(to: E5Variant) -> &'static str {
    match to {
        E5Variant::Int8 => "Re-indexing for faster search",
        E5Variant::Fp16 => "Re-indexing for standard search",
    }
}

/// The export a new library is built with for the setting `faster`: int8
/// only when chosen and this processor runs it exactly.
pub fn new_library_variant(faster: bool) -> E5Variant {
    if faster && int8_exact() {
        E5Variant::Int8
    } else {
        E5Variant::Fp16
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
    /// Stopped (cancelled from the strip, or Shodh closed) before it was done;
    /// what it embedded is kept for Continue.
    Stopped,
    /// Built: it completes when Shodh next starts.
    Ready,
    /// It failed; `error` says why. With `finishFailed`, the build was done
    /// and completing it at start failed (Retry restarts).
    Failed,
}

/// The switch, for the System status and Settings → Search.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchStatus {
    /// The library's search model ("fp16", "int8"), once recorded.
    pub index_variant: Option<E5Variant>,
    /// The model a re-index moves to: the one in progress, else the one
    /// offered.
    pub target: Option<E5Variant>,
    /// A re-index is offered (none in progress): to int8 for an fp16 library
    /// searched on a processor that runs int8 exactly; back to fp16 for an
    /// int8 library.
    pub offered: bool,
    /// This processor runs the faster model exactly, so it may be chosen.
    pub faster_available: bool,
    /// New libraries are built with the faster model (Settings → Search).
    pub faster_for_new_libraries: bool,
    pub phase: SwitchPhase,
    /// Completing the built switch at start failed (see `error`).
    pub finish_failed: bool,
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
    pub error: Option<String>,
}

#[derive(Debug, Default)]
struct Inner {
    running: bool,
    /// A Cancel asked to throw away what was embedded.
    discard: bool,
    phase: SwitchPhase,
    progress: SwitchProgress,
    /// When building started, and how far it was then (for the speed).
    building_since: Option<(Instant, u64)>,
    downloaded: (u64, u64),
    error: Option<String>,
}

/// Managed state: the switch running in this session, if any.
#[derive(Debug, Default)]
pub struct SearchModelSwitchState {
    inner: std::sync::Mutex<Inner>,
}

impl SearchModelSwitchState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
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

/// The switch as the System status shows it. `report` is what last ran the
/// search model.
pub async fn status(
    state: &SearchModelSwitchState,
    rag_state: &RagState,
    report: Option<&EmbedderReport>,
) -> SwitchStatus {
    let (data_dir, model_dir, new_library) = {
        let engine = rag_state.rag.read().await;
        (
            engine.config().data_dir.clone(),
            engine.config().embedding.model_dir.clone(),
            engine.config().embedding.new_library_variant,
        )
    };
    let index_variant = index_model::read(&data_dir)
        .ok()
        .flatten()
        .map(|r| r.variant);
    let pending = model_switch::pending(&data_dir).ok().flatten();
    let installed = installed_variants(&model_dir.join(E5_DIR));
    let exact = int8_exact();
    let on_cpu = report.is_some_and(|r| r.accelerator == Accelerator::Cpu);
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
    let error = if finish_failed {
        pending.as_ref().and_then(|p| p.finish_error.clone())
    } else if phase == SwitchPhase::Failed {
        inner.error.clone()
    } else {
        None
    };
    SwitchStatus {
        index_variant,
        target,
        offered,
        faster_available: exact,
        faster_for_new_libraries: new_library == E5Variant::Int8,
        phase,
        finish_failed,
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
    }
}

fn changed(app: &AppHandle) {
    if let Err(e) = app.emit(SYSTEM_STATUS_EVENT, ()) {
        tracing::debug!("Failed to emit {}: {}", SYSTEM_STATUS_EVENT, e);
    }
}

const NOT_EXACT: &str = "This processor cannot run the faster search model exactly (it has no VNNI instructions), so Shodh keeps the standard one.";

async fn current_status(state: &SearchModelSwitchState, rag_state: &RagState) -> SwitchStatus {
    let report = rag_state.rag.read().await.compute_status().embedder();
    status(state, rag_state, report.as_ref()).await
}

/// Downloads (or verifies) the model of `variant` into `model_dir`,
/// reporting the bytes to the switch state.
async fn install_variant(
    app: &AppHandle,
    model_dir: &std::path::Path,
    variant: E5Variant,
) -> Result<(), String> {
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
    store
        .install(&source, &on_download)
        .await
        .map(|_| ())
        .map_err(|e| format!("Downloading the search model failed: {e}"))
}

/// Whether new libraries are built with the faster model (Settings →
/// Search; off by default). Turning it on downloads the faster model when it
/// is missing. A library that has vectors keeps its model.
#[tauri::command]
pub async fn set_faster_search_for_new_libraries(
    app: AppHandle,
    faster: bool,
    state: State<'_, SearchModelSwitchState>,
    rag_state: State<'_, RagState>,
) -> Result<SwitchStatus, String> {
    if faster && !int8_exact() {
        return Err(NOT_EXACT.into());
    }
    if faster {
        let model_dir = rag_state
            .rag
            .read()
            .await
            .config()
            .embedding
            .model_dir
            .clone();
        install_variant(&app, &model_dir, E5Variant::Int8).await?;
    }
    // The engine first: a setting it could not take is not saved.
    rag_state
        .rag
        .write()
        .await
        .set_new_library_variant(new_library_variant(faster))
        .map_err(|e| format!("{e:#}"))?;
    let store = settings_store(&app)?;
    let (settings, ()) = store
        .update(|s| {
            s.indexing.faster_search = faster;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    broadcast(&app, &settings);
    tracing::info!(faster, "search model for new libraries changed");
    changed(&app);
    Ok(current_status(&state, &rag_state).await)
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
    let record = index_model::read(&data_dir)
        .map_err(|e| format!("{e:#}"))?
        .ok_or("This library's search model is not known yet; search once, then try again.")?;
    let target = match model_switch::pending(&data_dir).map_err(|e| format!("{e:#}"))? {
        Some(pending) => pending.to,
        None => match record.variant {
            E5Variant::Fp16 => E5Variant::Int8,
            E5Variant::Int8 => E5Variant::Fp16,
        },
    };
    if target == E5Variant::Int8 && !int8_exact() {
        return Err(NOT_EXACT.into());
    }
    {
        let mut inner = state.lock();
        if inner.running {
            return Err("A re-index of the library is already running.".into());
        }
        *inner = Inner {
            running: true,
            phase: SwitchPhase::Downloading,
            ..Inner::default()
        };
    }
    let task = app.clone();
    tauri::async_runtime::spawn(async move { run(task, target).await });
    changed(&app);
    Ok(current_status(&state, &rag_state).await)
}

/// Cancels the re-index: a running one stops before its next step, and what
/// it embedded is thrown away (a downloaded model is kept: it is the pinned,
/// verified file). A built switch whose completion failed is cancelled the
/// same way.
#[tauri::command]
pub async fn cancel_search_model_switch(
    app: AppHandle,
    state: State<'_, SearchModelSwitchState>,
    rag_state: State<'_, RagState>,
    indexing: State<'_, IndexingState>,
) -> Result<SwitchStatus, String> {
    let running = {
        let mut inner = state.lock();
        if inner.running {
            inner.discard = true;
        }
        inner.running
    };
    if running {
        indexing.cancel_source(SWITCH_SOURCE);
    } else {
        let data_dir = rag_state.rag.read().await.config().data_dir.clone();
        model_switch::cancel(&data_dir)
            .await
            .map_err(|e| format!("Cancelling failed: {e:#}"))?;
        let mut inner = state.lock();
        inner.phase = SwitchPhase::Idle;
        inner.error = None;
    }
    tracing::info!(running, "Re-indexing the library cancelled");
    changed(&app);
    Ok(current_status(&state, &rag_state).await)
}

/// Restarts Shodh, which completes a built switch before anything opens the
/// index (and tries again after a failed completion).
#[tauri::command]
pub fn restart_for_search_model_switch(app: AppHandle) {
    tracing::info!("Restarting to switch the search model");
    app.restart();
}

/// At start: a re-index Shodh closed in the middle of goes on (it waits
/// while indexing is paused).
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
        match model_switch::pending(&data_dir) {
            Ok(Some(switch)) if !switch.built => {
                {
                    let state = app.state::<SearchModelSwitchState>();
                    let mut inner = state.lock();
                    if inner.running {
                        return;
                    }
                    *inner = Inner {
                        running: true,
                        phase: SwitchPhase::Downloading,
                        ..Inner::default()
                    };
                }
                tracing::info!(to = switch.to.label(), "Re-indexing the library goes on");
                run(app.clone(), switch.to).await;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "The search model switch could not be read")
            }
        }
    });
}

async fn run(app: AppHandle, to: E5Variant) {
    let result = run_switch(&app, to).await;
    {
        let state = app.state::<SearchModelSwitchState>();
        let mut inner = state.lock();
        inner.running = false;
        inner.discard = false;
        match result {
            Ok(phase) => inner.phase = phase,
            Err(error) => {
                tracing::warn!(%error, "Re-indexing the library failed");
                inner.phase = SwitchPhase::Failed;
                inner.error = Some(error);
            }
        }
    }
    changed(&app);
}

fn discarding(app: &AppHandle) -> bool {
    app.state::<SearchModelSwitchState>().lock().discard
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
    install_variant(app, &model_dir, to).await?;
    if discarding(app) || run.stopped() {
        drop(run);
        if discarding(app) {
            model_switch::cancel(&data_dir)
                .await
                .map_err(|e| format!("{e:#}"))?;
            return Ok(SwitchPhase::Idle);
        }
        return Ok(SwitchPhase::Stopped);
    }

    model_switch::start(&data_dir, to).map_err(|e| format!("{e:#}"))?;
    let embedder: std::sync::Arc<dyn EmbeddingModel> = {
        let model_dir = model_dir.clone();
        tokio::task::spawn_blocking(move || shodh_rag::rag_engine::switch_model(&model_dir, to))
            .await
            .map_err(|e| format!("Loading the search model failed: {e}"))?
            .map_err(|e| format!("Loading the search model failed: {e:#}"))?
    };
    {
        let state = app.state::<SearchModelSwitchState>();
        let mut inner = state.lock();
        inner.phase = SwitchPhase::Building;
    }
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
        BuildOutcome::Stopped if discarding(app) => {
            model_switch::cancel(&data_dir)
                .await
                .map_err(|e| format!("{e:#}"))?;
            Ok(SwitchPhase::Idle)
        }
        BuildOutcome::Stopped => Ok(SwitchPhase::Stopped),
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
    /// Cancel), never as "ready, restart".
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
    }

    #[test]
    fn the_status_serializes_in_camel_case() {
        let status = SwitchStatus {
            index_variant: Some(E5Variant::Fp16),
            target: Some(E5Variant::Int8),
            offered: true,
            faster_available: true,
            faster_for_new_libraries: false,
            phase: SwitchPhase::Building,
            finish_failed: false,
            done: 3,
            total: 9,
            eta_seconds: Some(12),
            downloaded_bytes: 0,
            download_bytes: 0,
            model_bytes: 278_686_411,
            error: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["indexVariant"], "fp16");
        assert_eq!(json["target"], "int8");
        assert_eq!(json["phase"], "building");
        assert_eq!(json["etaSeconds"], 12);
        assert_eq!(json["modelBytes"], 278_686_411u64);
        assert_eq!(json["fasterForNewLibraries"], false);
        assert_eq!(json["finishFailed"], false);
    }
}
