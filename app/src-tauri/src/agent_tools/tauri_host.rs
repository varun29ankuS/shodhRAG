//! [`HostEffects`] for the running app.

use shodh_rag::audit::payload::{indexing_outcome, source_change, ChangeOrigin};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::tools::ToolContext;
use shodh_rag::indexing::{IndexingOptions, IndexingState};
use tauri::{AppHandle, Emitter, Manager};

use super::{CalendarChange, ConversationChange, HostEffects, IndexJob};
use crate::calendar_commands::{spawn_reindex, CALENDAR_CHANGED_EVENT};
use crate::event_emitter::TauriEventEmitter;
use crate::rag_commands::RagState;

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

/// Emitted when the agent renames or pins a saved conversation; the UI
/// patches its in-memory list so its next save keeps the change.
pub const CONVERSATION_UPDATED_EVENT: &str = "conversation-updated";

impl HostEffects for TauriEffects {
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
    }

    /// Folder indexing holds the RAG engine's write lock for the whole job,
    /// so it runs in the background instead of blocking the agent (and every
    /// search) until it finishes.
    fn start_indexing(&self, ctx: &ToolContext, job: IndexJob) {
        let app = self.app.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let rag = app.state::<RagState>().rag.clone();
            let indexing_state = app.state::<IndexingState>();
            let emitter = TauriEventEmitter::new(app.clone());
            let mut engine = rag.write().await;
            let result = shodh_rag::indexing::index_folder(
                &job.folder,
                &job.source_id,
                &agent_indexing_options(),
                &mut engine,
                &indexing_state,
                Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
            )
            .await;
            drop(engine);
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
        });
    }
}
