//! [`HostEffects`] for the running app.

use shodh_rag::audit::payload::{indexing_outcome, source_change, ChangeOrigin};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::tools::ToolContext;
use shodh_rag::indexing::{IndexingOptions, IndexingState};
use tauri::{AppHandle, Emitter, Manager};

use super::{CalendarChange, ConversationChange, HostEffects, IndexJob, ModelInfo};
use crate::app_settings::AppSettings;
use crate::calendar_commands::{spawn_reindex, CALENDAR_CHANGED_EVENT};
use crate::event_emitter::TauriEventEmitter;
use crate::llm_commands::LLMState;
use crate::rag_commands::RagState;
use shodh_rag::audit::payload::{is_cloud, provider_id};
use shodh_rag::llm::LLMMode;

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
    fn settings_changed(&self, settings: &AppSettings) {
        crate::app_settings::broadcast(&self.app, settings);
    }

    fn openrouter_key(&self) -> Option<String> {
        let non_empty =
            |v: Option<String>| v.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        if let Some(key) = non_empty(std::env::var("OPENROUTER_API_KEY").ok()) {
            return Some(key);
        }
        let llm = self.app.state::<LLMState>();
        let stored = llm
            .api_keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .openrouter
            .clone();
        if let Some(key) = non_empty(stored) {
            return Some(key);
        }
        let config = llm.config.lock().unwrap_or_else(|e| e.into_inner());
        match &config.mode {
            LLMMode::External {
                provider: shodh_rag::llm::ApiProvider::OpenRouter,
                api_key,
                ..
            } => non_empty(Some(api_key.clone())),
            _ => None,
        }
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
