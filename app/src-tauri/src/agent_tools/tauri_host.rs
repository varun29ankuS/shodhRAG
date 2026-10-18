//! [`HostEffects`] for the running app.

use serde_json::json;
use shodh_rag::audit::payload::{indexing_outcome, source_change, ChangeOrigin};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::tools::ToolContext;
use shodh_rag::indexing::{IndexingOptions, IndexingState};
use tauri::{AppHandle, Emitter, Manager};

use super::{CalendarChange, ConversationChange, FileIndexJob, HostEffects, IndexJob, ModelInfo};
use crate::app_settings::AppSettings;
use crate::background::BackgroundState;
use crate::calendar_commands::{spawn_reindex, CALENDAR_CHANGED_EVENT};
use crate::event_emitter::TauriEventEmitter;
use crate::file_watcher::FolderSyncState;
use crate::llm_commands::LLMState;
use crate::rag_commands::RagState;
use shodh_rag::audit::payload::{is_cloud, provider_id};
use shodh_rag::llm::LLMMode;
use shodh_rag::RAGEngine;
use tokio::sync::RwLock;

pub struct TauriEffects {
    app: AppHandle,
}

impl TauriEffects {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

fn agent_indexing_options() -> IndexingOptions {
    IndexingOptions {
        skip_indexed: false,
        watch_changes: false,
        process_subdirs: true,
        priority: "normal".to_string(),
        // Empty means every supported file type.
        file_types: Vec::new(),
    }
}

/// Emitted when the files of a source change (the agent created a folder,
/// downloaded or indexed a file, indexed a folder) or a source was removed;
/// the Library reads the indexed files again. The payload names the source
/// changed, `{ "sourceId": id }` ([`library_changed_payload`]), or removed,
/// `{ "sourceId": id, "removed": [id] }` ([`library_removed_payload`]), whose
/// card the Library drops.
pub const LIBRARY_CHANGED_EVENT: &str = "library-changed";

/// The [`LIBRARY_CHANGED_EVENT`] of a source whose files changed.
pub fn library_changed_payload(source_id: &str) -> serde_json::Value {
    json!({ "sourceId": source_id })
}

/// The [`LIBRARY_CHANGED_EVENT`] of a source removed from the index.
pub fn library_removed_payload(source_id: &str) -> serde_json::Value {
    json!({ "sourceId": source_id, "removed": [source_id] })
}

/// Emitted when the agent renames or pins a saved conversation; the UI
/// patches its in-memory list so its next save keeps the change.
pub const CONVERSATION_UPDATED_EVENT: &str = "conversation-updated";

impl TauriEffects {
    fn emit_library(&self, payload: serde_json::Value) {
        if let Err(e) = self.app.emit(LIBRARY_CHANGED_EVENT, payload) {
            tracing::warn!("Failed to emit {}: {}", LIBRARY_CHANGED_EVENT, e);
        }
    }
}

#[async_trait::async_trait]
impl HostEffects for TauriEffects {
    fn visuals_changed(&self, conversation_id: &str) {
        crate::visual_commands::broadcast_change(&self.app, Some(conversation_id));
    }

    fn research_changed(&self, kind: &str, file_path: &str) {
        crate::research_commands::broadcast_change(&self.app, kind, Some(file_path));
    }

    fn workspaces_changed(&self, workspace_id: &str) {
        crate::workspace_commands::broadcast_change(&self.app, Some(workspace_id));
    }

    fn settings_changed(&self, settings: &AppSettings) {
        crate::app_settings::broadcast(&self.app, settings);
    }

    fn index_file(&self, ctx: &ToolContext, job: FileIndexJob) {
        let app = self.app.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            // Paused from the tray: wait before starting.
            app.state::<BackgroundState>().wait_until_resumed().await;
            let rag = app.state::<RagState>().rag.clone();
            let emitter = TauriEventEmitter::new(app.clone());
            let indexing = app.state::<IndexingState>();
            // Registered while it runs: removing the source waits for it, and
            // a removed source is not indexed again.
            let result = match indexing.begin_run_at(&job.source_id, &job.path) {
                Some(_run) => {
                    shodh_rag::indexing::index_single_file(
                        &job.path,
                        &job.source_id,
                        &rag,
                        Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
                    )
                    .await
                }
                None => Err(format!(
                    "{} was removed from the Library and is not indexed",
                    job.path
                )),
            };
            if let Err(e) = &result {
                tracing::warn!(target: "shodh::harness", path = %job.path, error = %e, "indexing a downloaded file failed");
            }
            ctx.audit(
                AuditEventType::SourceChange,
                source_change(
                    "add_file",
                    ChangeOrigin::Agent,
                    Some(&job.source_id),
                    Some(&job.path),
                    indexing_outcome(&result),
                ),
            );
            if let Err(e) = app.emit(
                LIBRARY_CHANGED_EVENT,
                library_changed_payload(&job.source_id),
            ) {
                tracing::warn!("Failed to emit {}: {}", LIBRARY_CHANGED_EVENT, e);
            }
        });
    }

    fn library_changed(&self, source_id: &str) {
        self.emit_library(library_changed_payload(source_id));
    }

    async fn remove_source(
        &self,
        rag: &RwLock<RAGEngine>,
        source_id: &str,
    ) -> Result<usize, String> {
        let indexing = self.app.state::<IndexingState>();
        let folders = self.app.state::<FolderSyncState>();
        let path = folders.library_path(source_id);
        let deleted = folders
            .remove_source(rag, &indexing, source_id, path.as_deref())
            .await?;
        self.emit_library(library_removed_payload(source_id));
        Ok(deleted)
    }

    fn documents_dir(&self) -> Option<std::path::PathBuf> {
        match self.app.path().document_dir() {
            Ok(dir) => Some(dir),
            Err(e) => {
                tracing::warn!("The Documents folder is unavailable: {e}");
                None
            }
        }
    }

    fn openrouter_key(&self) -> Option<String> {
        // A key saved in the app wins over `OPENROUTER_API_KEY`
        // (see `api_key_store::choose_key`).
        let llm = self.app.state::<LLMState>();
        let stored = llm
            .api_keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .openrouter
            .clone();
        let configured = match &llm.config.lock().unwrap_or_else(|e| e.into_inner()).mode {
            LLMMode::External {
                provider: shodh_rag::llm::ApiProvider::OpenRouter,
                api_key,
                ..
            } => Some(api_key.clone()),
            _ => None,
        };
        let saved = crate::api_key_store::choose_key(stored, configured).map(|(key, _)| key);
        crate::api_key_store::choose_key(saved, crate::api_key_store::process_env_key("openrouter"))
            .map(|(key, _)| key)
    }

    fn model_info(&self) -> Option<ModelInfo> {
        let llm = self.app.state::<LLMState>();
        let mode = llm
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode
            .clone();
        match mode {
            LLMMode::External {
                provider, model, ..
            } => {
                let id = provider_id(&provider);
                Some(ModelInfo {
                    provider: id.to_string(),
                    model: Some(model),
                    cloud: is_cloud(id),
                })
            }
            LLMMode::Local { model_path } => Some(ModelInfo {
                provider: "local".to_string(),
                model: model_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned()),
                cloud: false,
            }),
            LLMMode::Disabled => None,
        }
    }

    fn conversation_changed(&self, change: ConversationChange) {
        if let Err(e) = self.app.emit(CONVERSATION_UPDATED_EVENT, &change) {
            tracing::warn!("Failed to emit {}: {}", CONVERSATION_UPDATED_EVENT, e);
        }
    }

    fn calendar_changed(&self, change: CalendarChange) {
        spawn_reindex(&self.app, &change);
        // Every task/event change (UI, agent tools, subtasks) goes through
        // here, so open views refresh from one signal.
        if let Err(e) = self.app.emit(CALENDAR_CHANGED_EVENT, ()) {
            tracing::warn!("Failed to emit {}: {}", CALENDAR_CHANGED_EVENT, e);
        }
        // A reminder may have been set, moved or removed.
        crate::reminders::wake(&self.app);
        // A reminder that rang for a task now done or deleted waits on nothing.
        if let Some(id) = settled_reminder(&change) {
            let app = self.app.clone();
            tauri::async_runtime::spawn(async move {
                crate::inbox_commands::resolve(&app, id).await;
            });
        }
    }

    /// Folder indexing takes minutes, so it runs in the background instead of
    /// blocking the agent until it finishes. It holds the engine's write lock only
    /// while storing each file, so searches keep working meanwhile.
    fn start_indexing(&self, ctx: &ToolContext, job: IndexJob) {
        let app = self.app.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            app.state::<BackgroundState>().wait_until_resumed().await;
            let rag = app.state::<RagState>().rag.clone();
            let indexing_state = app.state::<IndexingState>();
            let emitter = TauriEventEmitter::new(app.clone());
            let result = shodh_rag::indexing::index_folder(
                &job.folder,
                &job.source_id,
                &agent_indexing_options(),
                &rag,
                &indexing_state,
                Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
            )
            .await;
            if let Err(e) = &result {
                tracing::warn!(target: "shodh::harness", source_id = %job.source_id, error = %e, "agent-started indexing failed");
            }
            ctx.audit(
                AuditEventType::SourceChange,
                source_change(
                    job.action,
                    ChangeOrigin::Agent,
                    Some(&job.source_id),
                    Some(&job.folder),
                    indexing_outcome(&result),
                ),
            );
            // The Library reads the indexed files again: the folder's are new.
            if let Err(e) = app.emit(
                LIBRARY_CHANGED_EVENT,
                library_changed_payload(&job.source_id),
            ) {
                tracing::warn!("Failed to emit {}: {}", LIBRARY_CHANGED_EVENT, e);
            }
        });
    }
}

/// The Inbox reminder a calendar change settles (`reminders::inbox_item` ids it
/// `reminder:{task id}`): its task was completed or deleted.
fn settled_reminder(change: &CalendarChange) -> Option<String> {
    match change {
        CalendarChange::TaskSaved(task) if task.status == "completed" => {
            Some(format!("reminder:{}", task.id))
        }
        CalendarChange::TaskRemoved(id) => Some(format!("reminder:{id}")),
        CalendarChange::TaskSaved(_)
        | CalendarChange::EventSaved(_)
        | CalendarChange::EventRemoved(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_done_or_deleted_task_settles_its_reminder() {
        let task = |status: &str| {
            serde_json::from_value::<crate::calendar_store::TodoItem>(json!({
                "id": "t1", "title": "Read", "status": status,
                "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z"
            }))
            .unwrap()
        };
        assert_eq!(
            settled_reminder(&CalendarChange::TaskSaved(task("completed"))),
            Some("reminder:t1".to_string())
        );
        assert_eq!(
            settled_reminder(&CalendarChange::TaskRemoved("t2".to_string())),
            Some("reminder:t2".to_string())
        );
        assert_eq!(
            settled_reminder(&CalendarChange::TaskSaved(task("pending"))),
            None
        );
        assert_eq!(
            settled_reminder(&CalendarChange::EventRemoved("e1".to_string())),
            None
        );
    }

    #[test]
    fn library_events_name_the_source_and_what_was_removed() {
        assert_eq!(
            library_changed_payload("folder-1"),
            json!({ "sourceId": "folder-1" })
        );
        assert_eq!(
            library_removed_payload("folder-1"),
            json!({ "sourceId": "folder-1", "removed": ["folder-1"] })
        );
    }
}
