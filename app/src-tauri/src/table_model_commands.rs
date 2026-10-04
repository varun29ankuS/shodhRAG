//! The optional table structure model and the background refinement of PDF tables.
//!
//! Papers are indexed with the fast layout heuristics. Every PDF with table-candidate
//! pages is then queued ([`spawn_refinement`]); with the model installed, the worker
//! re-parses it with the model on those pages, replaces its chunks when the model
//! structured a table, and re-extracts its Result statements when the paper was
//! scanned for results before. The model is pinned (size and SHA-256) in
//! `shodh_rag::embeddings::model_store::table_model_artifacts`, downloaded only when
//! the user asks, and loaded at startup when its files verify.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::comprehensive_system::ComprehensiveRAG;
use shodh_rag::embeddings::model_store::{
    ArtifactState, ArtifactStatus, HttpByteSource, InstallPhase, InstallProgress, ModelStore,
};
use shodh_rag::processing::table_model::{
    loaded, model_dir, SharedTableModel, TableModel, TableModelError,
};
use shodh_rag::table_refinement::{refine_file, RefineOutcome};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::RwLock as AsyncRwLock;

use crate::audit_commands::AuditState;
use crate::research_commands::{broadcast_change, ResearchState};

/// Progress of a running install (`InstallProgress`, camelCase).
pub const TABLE_MODEL_PROGRESS_EVENT: &str = "table-model-progress";
/// Emitted once the model is loaded.
pub const TABLE_MODEL_READY_EVENT: &str = "table-model-ready";
/// Emitted when a file's tables were refined (`{ filePath, chunks, modelTables }`).
pub const TABLES_REFINED_EVENT: &str = "tables-refined";
/// Error code returned when an install is already running.
pub const INSTALL_IN_PROGRESS_CODE: &str = "install_in_progress";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Intra-op threads of the table model: refinement runs in the background, so it
/// leaves most cores to the app.
fn model_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(1, 4))
        .unwrap_or(2)
}

/// Where the model lives, the loaded model, the refinement queue and the install guard.
pub struct TableModelState {
    pub model_root: PathBuf,
    pub model: SharedTableModel,
    queue: std::sync::Mutex<Option<UnboundedSender<PathBuf>>>,
    install_lock: tokio::sync::Mutex<()>,
}

impl TableModelState {
    pub fn new(model_root: PathBuf) -> Self {
        Self {
            model_root,
            model: SharedTableModel::default(),
            queue: std::sync::Mutex::new(None),
            install_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn store(&self) -> ModelStore {
        ModelStore::table_model(self.model_root.clone())
    }

    /// Load the model when its files are present and verified. Blocking: call from a
    /// blocking thread. `Ok(false)` when it is not installed.
    pub fn load_if_installed(&self) -> Result<bool, String> {
        let status = self.store().status(true).map_err(|e| e.to_string())?;
        if !status
            .artifacts
            .iter()
            .all(|a| a.state == ArtifactState::Verified)
        {
            return Ok(false);
        }
        self.load()?;
        Ok(true)
    }

    fn load(&self) -> Result<(), String> {
        let model = TableModel::load(&model_dir(&self.model_root), model_threads())
            .map_err(|e| e.to_string())?;
        *self.model.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(model));
        Ok(())
    }

    fn enqueue(&self, path: PathBuf) {
        if let Some(queue) = self
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            // A closed queue only means no refinement runs.
            let _ = queue.send(path);
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableModelStatus {
    /// The model is loaded and refines tables.
    pub ready: bool,
    pub installing: bool,
    /// Whether the model can run on this system.
    pub supported: bool,
    pub total_bytes: u64,
    pub artifacts: Vec<ArtifactStatus>,
}

async fn read_status(state: &TableModelState) -> Result<TableModelStatus, String> {
    let ready = loaded(&state.model).is_some();
    let installing = state.install_lock.try_lock().is_err();
    let store = state.store();
    let verify = !ready && !installing;
    let status = tokio::task::spawn_blocking(move || store.status(verify))
        .await
        .map_err(|e| format!("Table model status task failed: {e}"))?
        .map_err(|e| e.to_string())?;
    Ok(TableModelStatus {
        ready,
        installing,
        supported: cfg!(windows),
        total_bytes: status.total_bytes,
        artifacts: status.artifacts,
    })
}

/// Whether the table model is installed and loaded.
#[tauri::command]
pub async fn table_model_status(
    state: State<'_, TableModelState>,
) -> Result<TableModelStatus, String> {
    read_status(&state).await
}

/// Download (or resume), verify and load the table model, then queue every indexed
/// PDF for refinement. Emits [`TABLE_MODEL_PROGRESS_EVENT`] while running and
/// [`TABLE_MODEL_READY_EVENT`] on success.
#[tauri::command]
pub async fn install_table_model(
    app: AppHandle,
    state: State<'_, TableModelState>,
    audit: State<'_, AuditState>,
    rag: State<'_, crate::rag_commands::RagState>,
) -> Result<TableModelStatus, String> {
    if !cfg!(windows) {
        return Err(TableModelError::UnsupportedPlatform.to_string());
    }
    let guard = state.install_lock.try_lock().map_err(|_| {
        format!("{INSTALL_IN_PROGRESS_CODE}: the table model is already being installed")
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
        if let Err(e) = emitter.emit(TABLE_MODEL_PROGRESS_EVENT, p) {
            tracing::debug!(error = %e, "emitting table model progress failed");
        }
    };
    let report = match store.install(&source, &progress).await {
        Ok(report) => report,
        Err(e) => {
            audit.record(AuditRecord::new(
                AuditEventType::RuntimeInstall,
                json!({"component": "table_model", "ok": false, "error": e.to_string()}),
            ));
            tracing::error!(error = %e, "Table model install failed");
            return Err(e.to_string());
        }
    };
    audit.record(AuditRecord::new(
        AuditEventType::RuntimeInstall,
        json!({
            "component": "table_model",
            "ok": true,
            "artifacts": report.artifacts,
            "model_dir": state.model_root.display().to_string(),
        }),
    ));
    let handle = app.clone();
    tokio::task::spawn_blocking(move || handle.state::<TableModelState>().load())
        .await
        .map_err(|e| format!("Loading the table model failed: {e}"))??;
    if let Err(e) = app.emit(TABLE_MODEL_READY_EVENT, ()) {
        tracing::debug!(error = %e, "emitting table model ready failed");
    }
    drop(guard);
    // Files indexed before the model was installed are refined now.
    let sources = rag.rag.read().await.document_sources().await;
    match sources {
        Ok(rows) => {
            for row in rows {
                let path = PathBuf::from(&row.source);
                let is_pdf = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
                if is_pdf && path.is_file() {
                    state.enqueue(path);
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "indexed files could not be listed for refinement"),
    }
    read_status(&state).await
}

/// Connects `engine` to the refinement worker and starts it. Call once at startup,
/// before the engine is shared (`rag` is the shared engine the worker writes to).
pub fn spawn_refinement(
    app: &AppHandle,
    engine: &mut ComprehensiveRAG,
) -> UnboundedReceiver<PathBuf> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    engine.set_refinement_queue(tx.clone());
    *app.state::<TableModelState>()
        .queue
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(tx);
    rx
}

/// Runs the refinement worker over the queue [`spawn_refinement`] returned.
pub fn start_worker(
    app: AppHandle,
    rag: Arc<AsyncRwLock<ComprehensiveRAG>>,
    rx: UnboundedReceiver<PathBuf>,
) {
    tauri::async_runtime::spawn(refinement_worker(app, rag, rx));
}

async fn refinement_worker(
    app: AppHandle,
    rag: Arc<AsyncRwLock<ComprehensiveRAG>>,
    mut rx: UnboundedReceiver<PathBuf>,
) {
    while let Some(path) = rx.recv().await {
        let Some(model) = loaded(&app.state::<TableModelState>().model) else {
            continue;
        };
        let file = path.clone();
        let refined = tokio::task::spawn_blocking(move || refine_file(&file, model)).await;
        let refined = match refined {
            Ok(Ok(Some(refined))) => refined,
            Ok(Ok(None)) => continue,
            Ok(Err(e)) => {
                tracing::warn!(error = %format!("{e:#}"), "table refinement failed: {}", path.display());
                continue;
            }
            Err(e) => {
                tracing::warn!(error = %e, "table refinement task failed: {}", path.display());
                continue;
            }
        };
        let document = refined.parsed.document.clone();
        let parse_ms = refined.parse_ms;
        let outcome = rag.write().await.apply_refined_tables(refined).await;
        let (chunks, model_tables) = match outcome {
            Ok(RefineOutcome::Replaced {
                chunks,
                model_tables,
            }) => (chunks, model_tables),
            Ok(other) => {
                tracing::info!(?other, "table refinement not applied: {}", path.display());
                continue;
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "refined tables could not be indexed: {}", path.display());
                continue;
            }
        };
        let file_path = path.display().to_string();
        tracing::info!(
            chunks,
            model_tables,
            parse_ms,
            "tables refined: {file_path}"
        );
        if let Err(e) = app.emit(
            TABLES_REFINED_EVENT,
            json!({ "filePath": file_path, "chunks": chunks, "modelTables": model_tables }),
        ) {
            tracing::debug!(error = %e, "emitting tables refined failed");
        }
        // Results of a paper scanned before are read again from the structured tables.
        let Some(document) = document else { continue };
        let Ok(services) = app.state::<ResearchState>().services().await else {
            continue;
        };
        match services
            .results
            .reextract_if_scanned(&file_path, &document)
            .await
        {
            Ok(Some(report)) => broadcast_change(&app, "result", Some(&report.file_path)),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "results not re-extracted after refinement: {file_path}")
            }
        }
    }
}
