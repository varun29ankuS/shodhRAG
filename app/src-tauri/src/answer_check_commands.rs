//! The optional answer-checking (entailment) model.
//!
//! Agent answers are checked claim by claim against the passages they cite
//! (`shodh_rag::harness::grounding`). With this model installed the check
//! reads whether a passage actually states a claim; without it, only topic
//! and numbers are checked (by the search reranker). The model is pinned
//! (revision, size and SHA-256) in
//! `shodh_rag::embeddings::model_store::answer_check_artifacts`, downloaded
//! only when the user asks, made available at startup when its files verify,
//! and loaded on first use (dropped again when idle).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::embeddings::model_store::{
    ArtifactState, ArtifactStatus, HttpByteSource, InstallPhase, InstallProgress, ModelStore,
    ANSWER_CHECK_DIR,
};
use shodh_rag::lazy_model::LazyModel;
use shodh_rag::reranking::NliModel;
use tauri::{AppHandle, Emitter, State};

use crate::audit_commands::AuditState;

/// Progress of a running install (`InstallProgress`, camelCase).
pub const ANSWER_CHECK_PROGRESS_EVENT: &str = "answer-check-progress";
/// Emitted once the model is loaded and answers are checked with it.
pub const ANSWER_CHECK_READY_EVENT: &str = "answer-check-ready";
/// Error code returned when an install is already running.
pub const INSTALL_IN_PROGRESS_CODE: &str = "install_in_progress";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// The model, shared with every agent session; loaded on first use.
pub type SharedNli = Arc<LazyModel<NliModel>>;

/// Where the model lives, the loaded model and the single-flight install guard.
pub struct AnswerCheckState {
    pub model_dir: PathBuf,
    pub model: SharedNli,
    install_lock: tokio::sync::Mutex<()>,
}

impl AnswerCheckState {
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            model: Arc::new(LazyModel::new("answer checking model")),
            install_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn store(&self) -> ModelStore {
        ModelStore::answer_check_model(self.model_dir.clone())
    }

    /// Make the model available when its files are present and verified (stamped
    /// or re-hashed); it loads on first use. Blocking: call from a blocking thread.
    pub fn load_if_installed(&self) -> Result<bool, String> {
        let status = self.store().status(true).map_err(|e| e.to_string())?;
        if !status
            .artifacts
            .iter()
            .all(|a| a.state == ArtifactState::Verified)
        {
            return Ok(false);
        }
        self.make_available();
        Ok(true)
    }

    fn make_available(&self) {
        let dir = self.model_dir.join(ANSWER_CHECK_DIR);
        self.model.set_loader(move || NliModel::new(&dir));
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerCheckStatus {
    /// The model is installed and answers are checked with it (it loads on first use).
    pub ready: bool,
    /// An install is running.
    pub installing: bool,
    pub total_bytes: u64,
    pub artifacts: Vec<ArtifactStatus>,
}

async fn read_status(state: &AnswerCheckState) -> Result<AnswerCheckStatus, String> {
    let ready = state.model.available();
    let installing = state.install_lock.try_lock().is_err();
    let store = state.store();
    let verify = !ready && !installing;
    let status = tokio::task::spawn_blocking(move || store.status(verify))
        .await
        .map_err(|e| format!("Answer checking model status task failed: {e}"))?
        .map_err(|e| e.to_string())?;
    Ok(AnswerCheckStatus {
        ready,
        installing,
        total_bytes: status.total_bytes,
        artifacts: status.artifacts,
    })
}

/// Whether the answer checking model is installed and loaded.
#[tauri::command]
pub async fn answer_check_status(
    state: State<'_, AnswerCheckState>,
) -> Result<AnswerCheckStatus, String> {
    read_status(&state).await
}

/// Download (or resume), verify and load the answer checking model. Emits
/// [`ANSWER_CHECK_PROGRESS_EVENT`] while running and
/// [`ANSWER_CHECK_READY_EVENT`] on success.
#[tauri::command]
pub async fn install_answer_check_model(
    app: AppHandle,
    state: State<'_, AnswerCheckState>,
    audit: State<'_, AuditState>,
) -> Result<AnswerCheckStatus, String> {
    let guard = state.install_lock.try_lock().map_err(|_| {
        format!("{INSTALL_IN_PROGRESS_CODE}: the answer checking model is already being installed")
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
        if let Err(e) = emitter.emit(ANSWER_CHECK_PROGRESS_EVENT, p) {
            tracing::debug!(error = %e, "emitting answer check progress failed");
        }
    };
    let report = match store.install(&source, &progress).await {
        Ok(report) => report,
        Err(e) => {
            audit.record(AuditRecord::new(
                AuditEventType::RuntimeInstall,
                json!({"component": "answer_check_model", "ok": false, "error": e.to_string()}),
            ));
            tracing::error!(error = %e, "Answer checking model install failed");
            return Err(e.to_string());
        }
    };
    audit.record(AuditRecord::new(
        AuditEventType::RuntimeInstall,
        json!({
            "component": "answer_check_model",
            "ok": true,
            "artifacts": report.artifacts,
            "model_dir": state.model_dir.display().to_string(),
        }),
    ));
    // Loaded once to report a broken install; it loads again on first use.
    let dir = state.model_dir.join(ANSWER_CHECK_DIR);
    tokio::task::spawn_blocking(move || NliModel::new(&dir))
        .await
        .map_err(|e| format!("Loading the answer checking model failed: {e}"))?
        .map_err(|e| format!("Loading the answer checking model failed: {e:#}"))?;
    state.make_available();
    if let Err(e) = app.emit(ANSWER_CHECK_READY_EVENT, ()) {
        tracing::debug!(error = %e, "emitting answer check ready failed");
    }
    drop(guard);
    read_status(&state).await
}
