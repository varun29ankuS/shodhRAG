//! "Re-index for faster search": moves a library built with the fp16 search
//! model to the int8 one, about 3x faster on the processor, without mixing
//! the two (`shodh_rag::embeddings::model_switch`).
//!
//! An index keeps the model it was built with, so a library indexed before
//! the int8 model shipped (or on a graphics card) stays fp16 when search runs
//! on the processor; this offers the switch there. It downloads the int8
//! model when it is missing (279 MB), then embeds the library's stored text
//! again in the background as an indexing run: the status strip shows it,
//! Pause holds it, Cancel stops it. Search keeps using the old index; the
//! switch completes when Shodh next starts, and the panel offers the restart.

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

/// The run's source id in the indexing state (the strip's Cancel stops it).
pub const SWITCH_SOURCE: &str = "search-model-switch";
/// What the strip shows for the run.
const SWITCH_LABEL: &str = "Re-indexing for faster search";
/// The export the switch moves to.
const TARGET: E5Variant = E5Variant::Int8;

/// Where a switch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SwitchPhase {
    /// None in progress.
    #[default]
    Idle,
    /// The faster model is being downloaded.
    Downloading,
    /// The library's text is being embedded again.
    Building,
    /// Stopped (cancelled from the strip, or Shodh closed) before it was done;
    /// what it embedded is kept for Continue.
    Stopped,
    /// Built: it completes when Shodh next starts.
    Ready,
    /// It failed; `error` says why.
    Failed,
}

/// The switch, for the System status.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchStatus {
    /// The library's search model ("fp16", "int8"), once recorded.
    pub index_variant: Option<E5Variant>,
    /// The switch is offered: the library is fp16, search runs on the
    /// processor, and no switch is in progress.
    pub offered: bool,
    pub phase: SwitchPhase,
    /// Rows embedded (or found unchanged) of all the rows to embed.
    pub done: u64,
    pub total: u64,
    /// Time left at the speed so far, once there is one.
    pub eta_seconds: Option<u64>,
    /// The download, while it runs.
    pub downloaded_bytes: u64,
    pub download_bytes: u64,
    /// The size of the faster model's download when it is not installed.
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
    let installed = installed_variants(&model_dir.join(E5_DIR)).contains(&TARGET);
    let inner = state.lock();
    let phase = if inner.running {
        inner.phase
    } else {
        match &pending {
            Some(p) if p.built => SwitchPhase::Ready,
            _ if inner.phase == SwitchPhase::Failed => SwitchPhase::Failed,
            Some(_) => SwitchPhase::Stopped,
            None => SwitchPhase::Idle,
        }
    };
    let on_cpu = report.is_some_and(|r| r.accelerator == Accelerator::Cpu);
    // Only where the int8 model runs exactly: elsewhere its vectors would come
    // out different from the ones it was measured with.
    let offered = phase == SwitchPhase::Idle
        && pending.is_none()
        && index_variant == Some(E5Variant::Fp16)
        && on_cpu
        && int8_exact();
    let eta_seconds = match (phase, inner.building_since) {
        (SwitchPhase::Building, Some((since, at_start))) => eta(
            inner.progress.done.saturating_sub(at_start),
            inner.progress.total.saturating_sub(inner.progress.done),
            since.elapsed(),
        ),
        _ => None,
    };
    SwitchStatus {
        index_variant,
        offered,
        phase,
        done: inner.progress.done,
        total: inner.progress.total,
        eta_seconds,
        downloaded_bytes: inner.downloaded.0,
        download_bytes: inner.downloaded.1,
        model_bytes: if installed {
            0
        } else {
            e5_model_artifact(TARGET).size
        },
        error: (phase == SwitchPhase::Failed)
            .then(|| inner.error.clone())
            .flatten(),
    }
}

fn changed(app: &AppHandle) {
    if let Err(e) = app.emit(SYSTEM_STATUS_EVENT, ()) {
        tracing::debug!("Failed to emit {}: {}", SYSTEM_STATUS_EVENT, e);
    }
}

/// Starts the switch (or continues a stopped one) in the background.
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
    if record.variant == TARGET {
        return Err("This library already uses the faster search model.".into());
    }
    if !int8_exact() {
        return Err(
            "This processor cannot run the faster search model exactly (it has no VNNI instructions), so Shodh keeps the current one."
                .into(),
        );
    }
    {
        let mut inner = state.lock();
        if inner.running {
            return Err("Re-indexing for faster search is already running.".into());
        }
        *inner = Inner {
            running: true,
            phase: SwitchPhase::Downloading,
            ..Inner::default()
        };
    }
    let task = app.clone();
    tauri::async_runtime::spawn(async move { run(task).await });
    changed(&app);
    let report = rag_state.rag.read().await.compute_status().embedder();
    Ok(status(&state, &rag_state, report.as_ref()).await)
}

/// Cancels the switch: a running one stops before its next step, and what it
/// embedded is thrown away (the download is kept: it is the pinned, verified
/// model, and a later switch needs no second download).
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
        state.lock().phase = SwitchPhase::Idle;
    }
    tracing::info!(running, "Re-indexing for faster search cancelled");
    changed(&app);
    let report = rag_state.rag.read().await.compute_status().embedder();
    Ok(status(&state, &rag_state, report.as_ref()).await)
}

/// Restarts Shodh, which completes a built switch before anything opens the
/// index.
#[tauri::command]
pub fn restart_for_search_model_switch(app: AppHandle) {
    tracing::info!("Restarting to switch the search model");
    app.restart();
}

/// At start: a switch Shodh closed in the middle of goes on (it waits while
/// indexing is paused).
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
                tracing::info!("Re-indexing for faster search goes on");
                run(app.clone()).await;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "The search model switch could not be read")
            }
        }
    });
}

async fn run(app: AppHandle) {
    let result = run_switch(&app).await;
    {
        let state = app.state::<SearchModelSwitchState>();
        let mut inner = state.lock();
        inner.running = false;
        inner.discard = false;
        match result {
            Ok(phase) => inner.phase = phase,
            Err(error) => {
                tracing::warn!(%error, "Re-indexing for faster search failed");
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

async fn run_switch(app: &AppHandle) -> Result<SwitchPhase, String> {
    if !int8_exact() {
        return Err(
            "This processor cannot run the faster search model exactly (it has no VNNI instructions)."
                .into(),
        );
    }
    let indexing = app.state::<IndexingState>();
    let run = indexing
        .begin_run_at(SWITCH_SOURCE, SWITCH_LABEL)
        .ok_or("Re-indexing for faster search could not start.")?;
    changed(app);
    let (data_dir, model_dir) = {
        let engine = app.state::<RagState>().rag.read().await;
        (
            engine.config().data_dir.clone(),
            engine.config().embedding.model_dir.clone(),
        )
    };

    // The faster model, verified (kept when it is already installed).
    let store = ModelStore::with_artifacts(model_dir.clone(), vec![e5_model_artifact(TARGET)]);
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
        .map_err(|e| format!("Downloading the faster search model failed: {e}"))?;
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

    model_switch::start(&data_dir, TARGET).map_err(|e| format!("{e:#}"))?;
    let embedder: std::sync::Arc<dyn EmbeddingModel> = {
        let model_dir = model_dir.clone();
        tokio::task::spawn_blocking(move || shodh_rag::rag_engine::switch_model(&model_dir, TARGET))
            .await
            .map_err(|e| format!("Loading the faster search model failed: {e}"))?
            .map_err(|e| format!("Loading the faster search model failed: {e:#}"))?
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

    #[test]
    fn the_status_serializes_in_camel_case() {
        let status = SwitchStatus {
            index_variant: Some(E5Variant::Fp16),
            offered: true,
            phase: SwitchPhase::Building,
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
        assert_eq!(json["phase"], "building");
        assert_eq!(json["etaSeconds"], 12);
        assert_eq!(json["modelBytes"], 278_686_411u64);
    }
}
