//! Pausing, resuming and cancelling indexing, the search model's compute
//! setting, and the System status the window's status strip shows.
//!
//! The pause is one switch ([`shodh_rag::indexing::PauseSwitch`]) shared by
//! the engine's indexing state and the tray, saved in the settings
//! (`indexing.paused`) so indexing paused before a quit stays paused after
//! the next start. Cancelling stops the runs in progress of one source (or
//! all) before their next batch; what they stored stays, and a file counts
//! as indexed only once its batch is stored.

use serde::Serialize;
use shodh_rag::compute::provider::ComputePreference;
use shodh_rag::compute::status::EmbedderReport;
use shodh_rag::indexing::IndexingState;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::app_settings::{broadcast, SettingsStore};
use crate::background::BackgroundState;
use crate::rag_commands::RagState;

/// Emitted with `{ "paused": bool }` whenever indexing is paused or resumed
/// (from the window, the tray or the agent).
pub const INDEXING_PAUSED_EVENT: &str = "indexing-paused";

/// Emitted (without a payload) when what runs search changed: the model
/// loaded or was unloaded, or became slower than calibrated, or recovered.
/// The window reads [`system_status`] again.
pub const SYSTEM_STATUS_EVENT: &str = "system-status-changed";

/// Tells the window about every change of `status` for as long as the app
/// runs.
pub fn watch_compute(
    app: AppHandle,
    status: std::sync::Arc<shodh_rag::compute::status::ComputeStatus>,
) {
    let mut changes = status.subscribe();
    tauri::async_runtime::spawn(async move {
        // The sender lives in the engine's status: the loop ends only with
        // the engine.
        while changes.changed().await.is_ok() {
            if let Err(e) = app.emit(SYSTEM_STATUS_EVENT, ()) {
                tracing::debug!("Failed to emit {}: {}", SYSTEM_STATUS_EVENT, e);
            }
        }
    });
}

/// Pauses or resumes every indexing run, saves it, relabels the tray and
/// tells the window.
pub fn set_paused(app: &AppHandle, paused: bool) {
    // One change at a time: the switch and the saved setting never disagree
    // when the tray and the window act at once.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let background = app.state::<BackgroundState>();
    let changed = background.set_paused(paused);
    match settings_store(app).and_then(|store| {
        store
            .update(|s| {
                s.indexing.paused = background.is_paused();
                Ok(())
            })
            .map_err(|e| e.to_string())
    }) {
        Ok((settings, ())) => broadcast(app, &settings),
        // Paused for this run still; only a restart forgets it.
        Err(e) => tracing::warn!(error = %e, paused, "the indexing pause was not saved"),
    }
    if changed {
        tracing::info!(paused, "indexing paused or resumed");
    }
    if let Err(e) = app.emit(
        INDEXING_PAUSED_EVENT,
        serde_json::json!({ "paused": paused }),
    ) {
        tracing::warn!("Failed to emit {}: {}", INDEXING_PAUSED_EVENT, e);
    }
}

/// Emitted with `{ "part": "search" | "indexing" }` to open the System
/// status panel with that part highlighted (the assistant read or changed it).
pub const SHOW_SYSTEM_STATUS_EVENT: &str = "show-system-status";

/// Opens the System status panel at `part` in the window.
pub fn show(app: &AppHandle, part: &str) {
    if let Err(e) = app.emit(
        SHOW_SYSTEM_STATUS_EVENT,
        serde_json::json!({ "part": part }),
    ) {
        tracing::warn!("Failed to emit {}: {}", SHOW_SYSTEM_STATUS_EVENT, e);
    }
}

/// The System status as JSON (for the assistant's `system_status` tool).
pub async fn status_json(app: &AppHandle) -> serde_json::Value {
    let use_gpu = settings_store(app)
        .and_then(|s| s.load().map_err(|e| e.to_string()))
        .map(|s| s.indexing.use_gpu)
        .unwrap_or(true);
    let rag_state = app.state::<RagState>();
    let indexing = app.state::<IndexingState>();
    let status = read_status(&rag_state, &indexing, use_gpu).await;
    serde_json::to_value(&status).unwrap_or_default()
}

fn settings_store(app: &AppHandle) -> Result<SettingsStore, String> {
    crate::profile::app_data_dir(app)
        .map(|dir| SettingsStore::in_dir(&dir))
        .map_err(|e| format!("Failed to get app data directory: {e}"))
}

/// The compute preference the `use_gpu` setting stands for.
pub fn compute_preference(use_gpu: bool) -> ComputePreference {
    if use_gpu {
        ComputePreference::Auto
    } else {
        ComputePreference::Cpu
    }
}

#[tauri::command]
pub async fn pause_indexing(app: AppHandle) -> Result<bool, String> {
    set_paused(&app, true);
    Ok(true)
}

#[tauri::command]
pub async fn resume_indexing(app: AppHandle) -> Result<bool, String> {
    set_paused(&app, false);
    Ok(false)
}

/// Cancels the indexing of `source_id` (a Library source), or of every
/// source without one. Returns how many runs were stopped.
#[tauri::command]
pub async fn cancel_indexing(
    source_id: Option<String>,
    indexing: State<'_, IndexingState>,
) -> Result<usize, String> {
    let stopped = match source_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => indexing.cancel_source(id),
        _ => indexing.cancel(),
    };
    tracing::info!(source_id = ?source_id, runs = stopped, "indexing cancelled");
    Ok(stopped)
}

/// Whether the search model may use the graphics card. The loaded model is
/// dropped; the next search or file loads it on the new target.
#[tauri::command]
pub async fn set_search_gpu(
    app: AppHandle,
    use_gpu: bool,
    rag_state: State<'_, RagState>,
    indexing: State<'_, IndexingState>,
) -> Result<SystemStatus, String> {
    // The engine first: a setting it could not take is not saved.
    rag_state
        .rag
        .write()
        .await
        .set_compute_preference(compute_preference(use_gpu))
        .map_err(|e| format!("{e:#}"))?;
    let store = settings_store(&app)?;
    let (settings, ()) = store
        .update(|s| {
            s.indexing.use_gpu = use_gpu;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    broadcast(&app, &settings);
    tracing::info!(use_gpu, "search compute setting changed");
    Ok(read_status(&rag_state, &indexing, use_gpu).await)
}

/// A folder (or file) being indexed now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningIndex {
    pub source_id: String,
    pub path: String,
}

/// The parts of the system the status strip and panel show.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    /// The search models are installed.
    pub search_installed: bool,
    /// The setting: the search model may use the graphics card.
    pub use_gpu: bool,
    /// What runs the search model and how fast, once it has loaded (the
    /// last load's report while it is not in memory).
    pub search: Option<EmbedderReport>,
    /// The reranker is installed (it runs on the CPU).
    pub reranker_installed: bool,
    /// The PP-OCRv6 scan reader is installed (it runs on the CPU); without
    /// it scans are read by the system OCR.
    pub scan_reader_installed: bool,
    /// Indexing is paused.
    pub indexing_paused: bool,
    /// The folders and files being indexed now.
    pub running: Vec<RunningIndex>,
}

async fn read_status(
    rag_state: &RagState,
    indexing: &IndexingState,
    use_gpu: bool,
) -> SystemStatus {
    let (search_installed, compute, reranker_installed, scan_reader_installed) = {
        let engine = rag_state.rag.read().await;
        (
            engine.has_search_models(),
            engine.compute_status(),
            engine.reranker_installed(),
            engine.scan_reader_installed(),
        )
    };
    let mut running: Vec<RunningIndex> = indexing
        .running_paths()
        .into_iter()
        .map(|(source_id, path)| RunningIndex { source_id, path })
        .collect();
    running.sort_by(|a, b| a.source_id.cmp(&b.source_id).then(a.path.cmp(&b.path)));
    SystemStatus {
        search_installed,
        use_gpu,
        search: compute.embedder(),
        reranker_installed,
        scan_reader_installed,
        indexing_paused: indexing.is_paused(),
        running,
    }
}

#[tauri::command]
pub async fn system_status(
    app: AppHandle,
    rag_state: State<'_, RagState>,
    indexing: State<'_, IndexingState>,
) -> Result<SystemStatus, String> {
    let use_gpu = settings_store(&app)?
        .load()
        .map(|s| s.indexing.use_gpu)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "settings unreadable; the GPU setting reads as on");
            true
        });
    Ok(read_status(&rag_state, &indexing, use_gpu).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_serializes_in_camel_case() {
        let status = SystemStatus {
            search_installed: true,
            use_gpu: true,
            search: None,
            reranker_installed: true,
            scan_reader_installed: false,
            indexing_paused: true,
            running: vec![RunningIndex {
                source_id: "folder-papers".to_string(),
                path: "C:/Papers".to_string(),
            }],
        };
        let json = serde_json::to_value(&status).expect("json");
        assert_eq!(json["indexingPaused"], true);
        assert_eq!(json["running"][0]["sourceId"], "folder-papers");
        assert_eq!(json["scanReaderInstalled"], false);
        assert!(json["search"].is_null());
    }
}
