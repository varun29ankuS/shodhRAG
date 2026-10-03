//! Generated visuals (the gallery): managed state and the commands of the gallery and the
//! focus pop-out.
//!
//! Visuals live in `generated_visuals` of `<app_data_dir>/shodh.db` (see
//! `shodh_rag::visuals`). The store opens on first use with the database path and key the
//! audit log was opened with. Every call runs on the blocking pool (SQLite is synchronous).
//!
//! Changes are broadcast as [`VISUALS_CHANGED_EVENT`] so an open gallery refreshes, also
//! when the agent revised or organised a visual.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_rag::audit::AuditKey;
use shodh_rag::visuals::{
    CaptureReport, NewVersion, NewVisual, VisualAuthor, VisualDetail, VisualError, VisualOrigin,
    VisualPage, VisualQuery, VisualResult, VisualStore,
};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::OnceCell;

use crate::audit_commands::AuditState;

/// Emitted with `{ "conversationId": ... }` (null when several conversations changed)
/// after visuals were recorded, revised, renamed, pinned, annotated, deleted or restored.
pub const VISUALS_CHANGED_EVENT: &str = "visuals-changed";

/// Most answers one backfill call may carry.
pub const MAX_BACKFILL_BATCHES: usize = 2_000;

/// Error of a visuals command. `code` lets the UI pick its message without parsing text:
/// `not_found`, `deleted`, `invalid`, `unavailable` or `storage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualCommandError {
    pub code: &'static str,
    pub message: String,
}

impl VisualCommandError {
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "unavailable",
            message: message.into(),
        }
    }
}

impl From<VisualError> for VisualCommandError {
    fn from(error: VisualError) -> Self {
        let code = match &error {
            VisualError::NotFound(_) => "not_found",
            VisualError::Deleted(_) => "deleted",
            VisualError::Invalid(_) => "invalid",
            VisualError::Open(_) => "unavailable",
            VisualError::Sqlite(_) | VisualError::Json(_) | VisualError::Corrupt(_) => "storage",
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for VisualCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type VisualCommandResult<T> = Result<T, VisualCommandError>;

/// Managed state: the visuals store, opened on first use. Clones share it (the agent tools
/// hold one).
#[derive(Clone)]
pub struct VisualState {
    inner: Arc<Inner>,
}

struct Inner {
    store: OnceCell<Arc<VisualStore>>,
    database: Option<(PathBuf, Option<AuditKey>)>,
}

impl VisualState {
    /// State over the database the audit log opened (`None` when it could not be opened).
    pub fn new(audit: &AuditState) -> Self {
        Self::at(audit.database())
    }

    /// State over `shodh.db` at `database` (path and key).
    pub fn at(database: Option<(PathBuf, Option<AuditKey>)>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: OnceCell::new(),
                database,
            }),
        }
    }

    /// The store, opening it if needed. Fails (and retries on the next call) while the
    /// database is unavailable.
    pub async fn store(&self) -> VisualCommandResult<Arc<VisualStore>> {
        self.inner
            .store
            .get_or_try_init(|| async {
                let (path, key) = self.inner.database.clone().ok_or_else(|| {
                    VisualCommandError::unavailable(
                        "The gallery needs the app database (shodh.db), which could not be \
                         opened; see the audit page.",
                    )
                })?;
                let store =
                    tokio::task::spawn_blocking(move || VisualStore::open(&path, key.as_ref()))
                        .await
                        .map_err(|e| {
                            VisualCommandError::unavailable(format!(
                                "Opening the gallery failed: {e}"
                            ))
                        })??;
                Ok(Arc::new(store))
            })
            .await
            .cloned()
    }

    /// Runs `call` against the store on the blocking pool.
    pub async fn run<T, F>(&self, call: F) -> VisualCommandResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&VisualStore) -> VisualResult<T> + Send + 'static,
    {
        let store = self.store().await?;
        tokio::task::spawn_blocking(move || call(&store))
            .await
            .map_err(|e| VisualCommandError {
                code: "storage",
                message: format!("The gallery task failed: {e}"),
            })?
            .map_err(VisualCommandError::from)
    }
}

/// Tell open views that visuals of `conversation_id` (or of several conversations) changed.
pub fn broadcast_change(app: &AppHandle, conversation_id: Option<&str>) {
    if let Err(e) = app.emit(
        VISUALS_CHANGED_EVENT,
        json!({ "conversationId": conversation_id }),
    ) {
        tracing::warn!(target: "shodh::visuals", error = %e, "visuals change not broadcast");
    }
}

/// The visual blocks of one answer, for the backfill.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureBatch {
    pub origin: VisualOrigin,
    pub blocks: Vec<NewVisual>,
}

/// Outcome of the backfill.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillReport {
    pub created: usize,
    /// Answers whose visuals could not be recorded (each is logged).
    pub failed: usize,
}

/// Records the visual blocks of a finished answer (main or side). Blocks already recorded
/// for this answer are not added again.
#[tauri::command]
pub async fn visuals_capture(
    app: AppHandle,
    state: State<'_, VisualState>,
    origin: VisualOrigin,
    blocks: Vec<NewVisual>,
) -> VisualCommandResult<CaptureReport> {
    let conversation = origin.conversation_id.clone();
    let report = state
        .run(move |store| store.capture(&origin, &blocks))
        .await?;
    for skipped in &report.skipped {
        tracing::debug!(target: "shodh::visuals", index = skipped.index, reason = %skipped.reason, "visual block not recorded");
    }
    if !report.created.is_empty() {
        broadcast_change(&app, Some(&conversation));
    }
    Ok(report)
}

/// Whether the one-time backfill of earlier conversations already ran.
#[tauri::command]
pub async fn visuals_backfill_status(state: State<'_, VisualState>) -> VisualCommandResult<bool> {
    state.run(|store| store.backfill_done()).await
}

/// Records the visuals of conversations saved before the gallery existed. The UI sends the
/// answers in chunks; the chunk with `finished` (the default) marks the backfill as done, so
/// an interrupted backfill runs again on the next open. Idempotent: answers already
/// recorded add nothing.
#[tauri::command]
pub async fn visuals_backfill(
    app: AppHandle,
    state: State<'_, VisualState>,
    batches: Vec<CaptureBatch>,
    finished: Option<bool>,
) -> VisualCommandResult<BackfillReport> {
    let finished = finished.unwrap_or(true);
    if batches.len() > MAX_BACKFILL_BATCHES {
        return Err(VisualError::Invalid(format!(
            "{} answers in one backfill; at most {MAX_BACKFILL_BATCHES}",
            batches.len()
        ))
        .into());
    }
    let report = state
        .run(move |store| {
            let mut report = BackfillReport::default();
            for batch in &batches {
                match store.capture(&batch.origin, &batch.blocks) {
                    Ok(r) => report.created += r.created.len(),
                    Err(e) => {
                        report.failed += 1;
                        tracing::warn!(target: "shodh::visuals", conversation = %batch.origin.conversation_id, error = %e, "backfilling an answer's visuals failed");
                    }
                }
            }
            if finished {
                store.mark_backfill_done()?;
            }
            Ok(report)
        })
        .await?;
    if report.created > 0 {
        broadcast_change(&app, None);
    }
    Ok(report)
}

/// One page of the gallery.
#[tauri::command]
pub async fn visuals_list(
    state: State<'_, VisualState>,
    query: VisualQuery,
) -> VisualCommandResult<VisualPage> {
    state.run(move |store| store.list(&query)).await
}

/// How many visuals a conversation has (the "Visuals (N)" button).
#[tauri::command]
pub async fn visuals_count(
    state: State<'_, VisualState>,
    conversation_id: String,
) -> VisualCommandResult<u64> {
    state
        .run(move |store| store.count_for_conversation(&conversation_id))
        .await
}

/// A visual with its versions: the version `id`, or version `version` of its chain, or
/// with `latest` the chain's latest version.
#[tauri::command]
pub async fn visuals_get(
    state: State<'_, VisualState>,
    id: String,
    version: Option<u32>,
    latest: Option<bool>,
) -> VisualCommandResult<VisualDetail> {
    state
        .run(move |store| match (version, latest.unwrap_or(false)) {
            (Some(v), _) => store.version(&id, v),
            (None, true) => store.latest(&id),
            (None, false) => store.get(&id),
        })
        .await
}

fn changed(
    app: &AppHandle,
    result: VisualCommandResult<VisualDetail>,
) -> VisualCommandResult<VisualDetail> {
    if let Ok(detail) = &result {
        broadcast_change(app, Some(&detail.visual.conversation_id));
    }
    result
}

#[tauri::command]
pub async fn visuals_rename(
    app: AppHandle,
    state: State<'_, VisualState>,
    id: String,
    title: String,
) -> VisualCommandResult<VisualDetail> {
    let result = state.run(move |store| store.rename(&id, &title)).await;
    changed(&app, result)
}

#[tauri::command]
pub async fn visuals_set_pinned(
    app: AppHandle,
    state: State<'_, VisualState>,
    id: String,
    pinned: bool,
) -> VisualCommandResult<VisualDetail> {
    let result = state.run(move |store| store.set_pinned(&id, pinned)).await;
    changed(&app, result)
}

#[tauri::command]
pub async fn visuals_set_note(
    app: AppHandle,
    state: State<'_, VisualState>,
    id: String,
    note: String,
) -> VisualCommandResult<VisualDetail> {
    let result = state.run(move |store| store.set_note(&id, &note)).await;
    changed(&app, result)
}

/// Saves a refinement made in the pop-out as the chain's next version.
#[tauri::command]
pub async fn visuals_add_version(
    app: AppHandle,
    state: State<'_, VisualState>,
    base_id: String,
    source: String,
    params: Option<Value>,
    instruction: Option<String>,
) -> VisualCommandResult<VisualDetail> {
    let result = state
        .run(move |store| {
            store.add_version(
                &base_id,
                &NewVersion {
                    source,
                    params,
                    instruction,
                    author: VisualAuthor::User,
                },
            )
        })
        .await;
    changed(&app, result)
}

/// Deletes a visual (every version). Returns the id of its chain, for undo.
#[tauri::command]
pub async fn visuals_delete(
    app: AppHandle,
    state: State<'_, VisualState>,
    id: String,
) -> VisualCommandResult<String> {
    let root = state.run(move |store| store.delete(&id)).await?;
    broadcast_change(&app, None);
    Ok(root)
}

/// Undoes a delete.
#[tauri::command]
pub async fn visuals_restore(
    app: AppHandle,
    state: State<'_, VisualState>,
    id: String,
) -> VisualCommandResult<String> {
    let root = state.run(move |store| store.restore(&id)).await?;
    broadcast_change(&app, None);
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::visuals::VisualKind;

    #[tokio::test]
    async fn state_opens_lazily_and_maps_errors_to_codes() {
        let unavailable = VisualState::at(None);
        let error = unavailable.store().await.unwrap_err();
        assert_eq!(error.code, "unavailable");

        let dir = tempfile::tempdir().unwrap();
        let state = VisualState::at(Some((dir.path().join("shodh.db"), None)));
        let origin = VisualOrigin {
            conversation_id: "c1".into(),
            message_id: Some("m1".into()),
            thread_id: None,
            turn_id: None,
        };
        let report = state
            .run(move |store| {
                store.capture(
                    &origin,
                    &[NewVisual {
                        kind: VisualKind::Equation,
                        title: "Energy".into(),
                        source: "E = mc^2".into(),
                        params: None,
                    }],
                )
            })
            .await
            .unwrap();
        assert_eq!(report.created.len(), 1);
        let missing = state.run(|store| store.get("nope")).await.unwrap_err();
        assert_eq!(missing.code, "not_found");
        let id = report.created[0].clone();
        let invalid = state
            .run(move |store| store.rename(&id, " "))
            .await
            .unwrap_err();
        assert_eq!(invalid.code, "invalid");
    }

    #[test]
    fn backfill_batches_deserialize_from_the_frontend_shape() {
        let batch: CaptureBatch = serde_json::from_value(json!({
            "origin": { "conversationId": "c1", "messageId": "m1" },
            "blocks": [{ "kind": "svg", "title": "Sketch", "source": "<svg/>" }]
        }))
        .unwrap();
        assert_eq!(batch.origin.thread_id, None);
        assert_eq!(batch.blocks[0].kind, VisualKind::Svg);
    }
}
