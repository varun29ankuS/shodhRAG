//! Tauri commands for agent sessions (omp harness).
//!
//! One session (one omp process) per conversation. Every `AgentEvent` is
//! emitted to the WebView as `"agent_event"` with `{ sessionId, event }`.
//! `agent_send` is the Ask path that replaces `unified_chat`.
//!
//! Call `agent_start` when a conversation opens: starting omp verifies the
//! binary and launches the runtime, which takes seconds, so doing it before
//! the first question keeps the first visible activity immediate.

use std::sync::Arc;

use dashmap::DashMap;
use serde::Serialize;
use shodh_rag::harness::profile::is_valid_slug;
use shodh_rag::harness::tools::ToolRegistry;
use shodh_rag::harness::{
    fetch_omp, resolve_binary_path, select_model, AgentEvent, AgentHarness, AgentProfile,
    HarnessError, LaunchSpec, OmpLayout, OmpSession, SessionConfig,
};
use shodh_rag::llm::ApiProvider;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use crate::agent_tools::build_registry;
use crate::llm_commands::LLMState;
use crate::rag_commands::RagState;

/// Tauri event name for agent events.
pub const AGENT_EVENT: &str = "agent_event";

const MAX_ID_LEN: usize = 200;

/// Payload of the `agent_event` Tauri event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventEnvelope {
    pub session_id: String,
    pub event: AgentEvent,
}

struct SessionEntry {
    conversation_id: String,
    profile_id: String,
    session: Arc<OmpSession>,
}

/// Live agent sessions, managed as Tauri state.
#[derive(Default)]
pub struct AgentSessions {
    sessions: DashMap<String, Arc<SessionEntry>>,
    by_conversation: DashMap<String, String>,
    registry: OnceCell<Arc<ToolRegistry>>,
    /// Serialises starts so `agent_start` is idempotent per conversation.
    start_lock: AsyncMutex<()>,
}

impl AgentSessions {
    fn get(&self, session_id: &str) -> Result<Arc<OmpSession>, String> {
        self.sessions
            .get(session_id)
            .map(|entry| entry.session.clone())
            .ok_or_else(|| HarnessError::UnknownSession(session_id.to_string()).to_string())
    }

    fn remove(&self, session_id: &str) -> Option<Arc<SessionEntry>> {
        let (_, entry) = self.sessions.remove(session_id)?;
        self.by_conversation
            .remove_if(&entry.conversation_id, |_, sid| sid == session_id);
        Some(entry)
    }

    /// Stop every sidecar. Called when the app exits.
    pub async fn shutdown_all(&self) {
        let ids: Vec<String> = self.sessions.iter().map(|e| e.key().clone()).collect();
        let entries: Vec<Arc<SessionEntry>> = ids.iter().filter_map(|id| self.remove(id)).collect();
        futures::future::join_all(entries.iter().map(|e| e.session.shutdown())).await;
        if !entries.is_empty() {
            tracing::info!(target: "shodh::harness", sessions = entries.len(), "agent sessions shut down");
        }
    }
}

fn check_id(kind: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > MAX_ID_LEN {
        Err(format!("Invalid {kind}"))
    } else {
        Ok(())
    }
}

fn fallback_key(llm: &LLMState, provider: &ApiProvider) -> Option<String> {
    let keys = llm
        .api_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match provider {
        ApiProvider::OpenAI => keys.openai,
        ApiProvider::Anthropic => keys.anthropic,
        ApiProvider::OpenRouter => keys.openrouter,
        ApiProvider::Google => keys.google,
        ApiProvider::Grok => keys.grok,
        _ => None,
    }
}

/// Start (or reuse) the agent session for a conversation. Returns its id.
#[tauri::command]
pub async fn agent_start(
    app: AppHandle,
    conversation_id: String,
    profile_id: Option<String>,
    sessions: State<'_, AgentSessions>,
    rag: State<'_, RagState>,
    llm: State<'_, LLMState>,
) -> Result<String, String> {
    check_id("conversation id", &conversation_id)?;
    let profile_id = profile_id
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "assistant".to_string());
    if !is_valid_slug(&profile_id) {
        return Err(HarnessError::UnknownProfile(profile_id).to_string());
    }
    let profile = AgentProfile::builtin(&profile_id)
        .ok_or_else(|| HarnessError::UnknownProfile(profile_id.clone()).to_string())?;

    let _guard = sessions.start_lock.lock().await;

    let existing = sessions
        .by_conversation
        .get(&conversation_id)
        .map(|sid| sid.value().clone());
    if let Some(session_id) = existing {
        let reusable = sessions
            .sessions
            .get(&session_id)
            .map(|e| !e.session.is_closed() && e.profile_id == profile_id)
            .unwrap_or(false);
        if reusable {
            return Ok(session_id);
        }
        if let Some(stale) = sessions.remove(&session_id) {
            stale.session.shutdown().await;
        }
    }

    let registry = sessions
        .registry
        .get_or_try_init(|| async {
            build_registry(&app, rag.rag.clone())
                .map(Arc::new)
                .map_err(|e| format!("Agent tools failed to load: {e}"))
        })
        .await?
        .clone();

    let mode = llm
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode
        .clone();
    let model =
        select_model(&mode, |provider| fallback_key(&llm, provider)).map_err(|e| e.to_string())?;

    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("App data directory unavailable: {e}"))?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let launch = LaunchSpec {
        binary: resolve_binary_path(&app_data_dir),
        layout: OmpLayout::new(&app_data_dir),
        model,
        system_prompt: profile.instructions.clone(),
        session_id: session_id.clone(),
    };

    let (session, mut events) = OmpSession::start(SessionConfig {
        launch,
        profile,
        registry,
    })
    .await
    .map_err(|e| e.to_string())?;
    let session = Arc::new(session);

    let forward_app = app.clone();
    let forward_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            let envelope = AgentEventEnvelope {
                session_id: forward_id.clone(),
                event,
            };
            if let Err(e) = forward_app.emit(AGENT_EVENT, &envelope) {
                tracing::warn!(target: "shodh::harness", error = %e, "emitting agent_event failed");
            }
        }
    });

    sessions.sessions.insert(
        session_id.clone(),
        Arc::new(SessionEntry {
            conversation_id: conversation_id.clone(),
            profile_id,
            session,
        }),
    );
    sessions
        .by_conversation
        .insert(conversation_id, session_id.clone());
    Ok(session_id)
}

/// Ask a question (the Ask path). Returns the run id, which is `request_id`.
#[tauri::command]
pub async fn agent_send(
    session_id: String,
    text: String,
    request_id: String,
    sessions: State<'_, AgentSessions>,
) -> Result<String, String> {
    check_id("request id", &request_id)?;
    let session = sessions.get(&session_id)?;
    session
        .prompt(&text, Some(request_id))
        .await
        .map_err(|e| e.to_string())
}

/// Redirect the running answer (starts a new run when idle). Returns the run id.
#[tauri::command]
pub async fn agent_steer(
    session_id: String,
    text: String,
    sessions: State<'_, AgentSessions>,
) -> Result<String, String> {
    let session = sessions.get(&session_id)?;
    session.steer(&text).await.map_err(|e| e.to_string())
}

/// Interrupt the running answer.
#[tauri::command]
pub async fn agent_abort(
    session_id: String,
    sessions: State<'_, AgentSessions>,
) -> Result<(), String> {
    let session = sessions.get(&session_id)?;
    session.abort().await.map_err(|e| e.to_string())
}

/// Approve or decline a pending write/destructive step.
#[tauri::command]
pub async fn agent_approve(
    session_id: String,
    step_id: String,
    approved: bool,
    sessions: State<'_, AgentSessions>,
) -> Result<(), String> {
    check_id("step id", &step_id)?;
    let session = sessions.get(&session_id)?;
    session
        .approve(&step_id, approved)
        .map_err(|e| e.to_string())
}

/// Download and verify the pinned agent runtime. Returns its path.
#[tauri::command]
pub async fn agent_install_runtime(app: AppHandle) -> Result<String, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("App data directory unavailable: {e}"))?;
    fetch_omp(&app_data_dir)
        .await
        .map(|path| path.display().to_string())
        .map_err(|e| e.to_string())
}
