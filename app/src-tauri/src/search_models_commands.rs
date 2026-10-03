//! First-run setup of the search models (E5 embeddings + reranker).
//!
//! The app starts without them: the RAG engine runs in a "needs setup" state
//! in which search and indexing return `search_models_missing`. The frontend
//! asks [`search_models_status`] and shows a setup card; [`install_search_models`]
//! downloads the pinned files (verified SHA-256, resumable), loads them on a
//! blocking thread and attaches them to the running engine, so no restart is
//! needed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::embeddings::model_store::{
    ArtifactStatus, HttpByteSource, InstallPhase, InstallProgress, ModelStore,
};
use shodh_rag::SearchModels;
use tauri::{AppHandle, Emitter, State};

use crate::audit_commands::AuditState;
use crate::rag_commands::RagState;

/// Progress of a running install (`InstallProgress`, camelCase).
pub const SEARCH_MODELS_PROGRESS_EVENT: &str = "search-models-progress";
/// Emitted once the models are attached and search works.
pub const SEARCH_MODELS_READY_EVENT: &str = "search-models-ready";
/// Error code returned when an install is already running.
pub const INSTALL_IN_PROGRESS_CODE: &str = "install_in_progress";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Where the search models live and the single-flight install guard.
pub struct SearchModelsState {
    pub model_dir: PathBuf,
    install_lock: tokio::sync::Mutex<()>,
}

impl SearchModelsState {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            install_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn store(&self) -> ModelStore {
        ModelStore::search_models(self.model_dir.clone())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchModelsStatus {
    /// Search models are loaded in the running engine.
    pub ready: bool,
    /// An install is running.
    pub installing: bool,
    pub model_dir: String,
    pub total_bytes: u64,
    pub artifacts: Vec<ArtifactStatus>,
}

async fn read_status(
    state: &SearchModelsState,
    rag_state: &RagState,
    verify: bool,
) -> Result<SearchModelsStatus, String> {
    let ready = rag_state.rag.read().await.has_search_models();
    let installing = state.install_lock.try_lock().is_err();
    let store = state.store();
    // Hash unstamped files only when the result matters (not ready) and no
    // install is writing them.
    let verify = verify && !ready && !installing;
    let status = tokio::task::spawn_blocking(move || store.status(verify))
        .await
        .map_err(|e| format!("Search model status task failed: {e}"))?
        .map_err(|e| e.to_string())?;
    Ok(SearchModelsStatus {
        ready,
        installing,
        model_dir: state.model_dir.display().to_string(),
        total_bytes: status.total_bytes,
        artifacts: status.artifacts,
    })
}

/// Whether search is set up, and the state of each pinned model file.
#[tauri::command]
pub async fn search_models_status(
    state: State<'_, SearchModelsState>,
    rag_state: State<'_, RagState>,
) -> Result<SearchModelsStatus, String> {
    read_status(&state, &rag_state, true).await
}

/// Download (or resume), verify and load the search models, then attach
/// them to the running engine. Emits [`SEARCH_MODELS_PROGRESS_EVENT`] while
/// running and [`SEARCH_MODELS_READY_EVENT`] on success.
#[tauri::command]
pub async fn install_search_models(
    app: AppHandle,
    state: State<'_, SearchModelsState>,
    rag_state: State<'_, RagState>,
    audit: State<'_, AuditState>,
) -> Result<SearchModelsStatus, String> {
    let guard = state.install_lock.try_lock().map_err(|_| {
        format!("{INSTALL_IN_PROGRESS_CODE}: the search models are already being installed")
    })?;

    let store = state.store();
    let source = HttpByteSource::new().map_err(|e| e.to_string())?;
    let emitter = app.clone();
    let last_emit = std::sync::Mutex::new(None::<(Instant, InstallPhase)>);
    let progress = move |p: &InstallProgress| {
        let mut last = last_emit.lock().unwrap_or_else(|e| e.into_inner());
        let throttled = last.is_some_and(|(at, phase)| {
            phase == p.phase
                && p.phase == InstallPhase::Downloading
                && at.elapsed() < PROGRESS_INTERVAL
        });
        if throttled {
            return;
        }
        *last = Some((Instant::now(), p.phase));
        if let Err(e) = emitter.emit(SEARCH_MODELS_PROGRESS_EVENT, p) {
            tracing::debug!(error = %e, "emitting search model progress failed");
        }
    };

    let report = match store.install(&source, &progress).await {
        Ok(report) => report,
        Err(e) => {
            audit.record(AuditRecord::new(
                AuditEventType::RuntimeInstall,
                json!({"component": "search_models", "ok": false, "error": e.to_string()}),
            ));
            tracing::error!(error = %e, "Search model install failed");
            return Err(e.to_string());
        }
    };
    audit.record(AuditRecord::new(
        AuditEventType::RuntimeInstall,
        json!({
            "component": "search_models",
            "ok": true,
            "artifacts": report.artifacts,
            "model_dir": state.model_dir.display().to_string(),
        }),
    ));

    if !rag_state.rag.read().await.has_search_models() {
        let config = rag_state.rag.read().await.config().clone();
        // Loading reads ~600 MB and builds ONNX sessions: keep it off the
        // async workers and outside the engine lock.
        let models = tokio::task::spawn_blocking(move || SearchModels::load(&config))
            .await
            .map_err(|e| format!("Loading the search models failed: {e}"))?
            .map_err(|e| format!("Loading the search models failed: {e:#}"))?;
        rag_state
            .rag
            .write()
            .await
            .attach_search_models(models)
            .map_err(|e| format!("Attaching the search models failed: {e:#}"))?;
    }

    if let Err(e) = app.emit(SEARCH_MODELS_READY_EVENT, ()) {
        tracing::debug!(error = %e, "emitting search models ready failed");
    }
    // Calendar items are skipped while search is not set up; index them now.
    let reindex_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::calendar_commands::reindex_all_calendar_data(&reindex_handle).await;
    });

    drop(guard);
    read_status(&state, &rag_state, false).await
}
