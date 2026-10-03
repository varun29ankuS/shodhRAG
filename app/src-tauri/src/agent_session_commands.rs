//! Tauri commands for agent sessions (omp harness).
//!
//! One session (one omp process) per conversation. Every `AgentEvent` is
//! emitted to the WebView as `"agent_event"` with `{ sessionId, event }`.
//! `agent_send` is the Ask path that replaces `unified_chat`.
//!
//! Call `agent_start` when a conversation opens: starting omp verifies the
//! binary and launches the runtime, which takes seconds, so doing it before
//! the first question keeps the first visible activity immediate.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord, RunAuditTap};
use shodh_rag::harness::model::EnvValue;
use shodh_rag::harness::profile::is_valid_slug;
use shodh_rag::harness::tools::ToolRegistry;
use shodh_rag::harness::{
    fetch_omp, resolve_binary_path, select_model, AgentEvent, AgentHarness, AgentProfile,
    HarnessError, LaunchSpec, OmpLayout, OmpModel, OmpSession, SessionConfig, OMP_VERSION,
};
use shodh_rag::llm::{ApiProvider, LLMMode};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use crate::agent_tools::{build_registry, AgentHost, TauriEffects, AGENT_CANNOT_DO};
use crate::api_key_store;
use crate::app_settings::SettingsStore;
use crate::audit_commands::AuditState;
use crate::llm_commands::LLMState;
use crate::rag_commands::RagState;
use shodh_rag::audit::payload::is_cloud;

/// Tauri event name for agent events.
pub const AGENT_EVENT: &str = "agent_event";

/// Tauri event name for runtime download progress.
pub const RUNTIME_PROGRESS_EVENT: &str = "agent_runtime_progress";

const MAX_ID_LEN: usize = 200;

/// Live sessions kept before idle ones are stopped (each is one process).
const MAX_LIVE_SESSIONS: usize = 4;

/// Custom instructions appended to the profile's system prompt.
const MAX_INSTRUCTIONS_CHARS: usize = 4_000;

/// Earlier turns replayed into a fresh session (e.g. after a restart).
const MAX_HISTORY_TURNS: usize = 10;
const MAX_HISTORY_TURN_CHARS: usize = 2_000;

/// Longest question text stored in the audit log.
const MAX_AUDIT_QUESTION_CHARS: usize = 8_000;

/// Minimum interval between runtime download progress events.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Payload of the `agent_event` Tauri event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventEnvelope {
    pub session_id: String,
    pub event: AgentEvent,
}

/// Error returned by the agent commands. `code` lets the UI offer the right
/// next action without parsing the message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandError {
    /// `runtime_missing`, `runtime_invalid`, `model_config`, `busy`,
    /// `invalid_request`, `session_closed` or `runtime_error`.
    pub code: &'static str,
    pub message: String,
}

impl AgentCommandError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_request",
            message: message.into(),
        }
    }
}

impl From<HarnessError> for AgentCommandError {
    fn from(error: HarnessError) -> Self {
        let code = match &error {
            HarnessError::BinaryMissing { .. } => "runtime_missing",
            HarnessError::HashMismatch { .. }
            | HarnessError::NoPinnedHash(_)
            | HarnessError::UnsupportedPlatform(_) => "runtime_invalid",
            HarnessError::UnsupportedLocalModel
            | HarnessError::LlmDisabled
            | HarnessError::UnsupportedProvider(_)
            | HarnessError::MissingApiKey(_)
            | HarnessError::InvalidModel(_)
            | HarnessError::DisallowedModel(_)
            | HarnessError::LocalOnlyCloudModel(_) => "model_config",
            HarnessError::RunInProgress => "busy",
            HarnessError::SlashCommand
            | HarnessError::EmptyMessage
            | HarnessError::MessageTooLong(_)
            | HarnessError::UnknownProfile(_)
            | HarnessError::NoPendingApproval(_) => "invalid_request",
            HarnessError::SessionClosed | HarnessError::UnknownSession(_) => "session_closed",
            _ => "runtime_error",
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

type CommandResult<T> = Result<T, AgentCommandError>;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

struct SessionEntry {
    conversation_id: String,
    profile_id: String,
    instructions: Option<String>,
    /// Model and credentials the session was started with (see
    /// [`model_fingerprint`]); a change in settings restarts the session.
    model_fingerprint: u64,
    session: Arc<OmpSession>,
    /// Earlier turns have been replayed (or there were none to replay).
    primed: AtomicBool,
    last_used_ms: AtomicU64,
}

impl SessionEntry {
    fn touch(&self) {
        self.last_used_ms.store(now_ms(), Ordering::Relaxed);
    }
}

/// Live agent sessions, managed as Tauri state.
#[derive(Default)]
pub struct AgentSessions {
    sessions: DashMap<String, Arc<SessionEntry>>,
    by_conversation: DashMap<String, String>,
    registry: OnceCell<Arc<ToolRegistry>>,
    /// Per-conversation start locks: `agent_start` is idempotent per
    /// conversation, and a slow start never blocks another conversation.
    start_locks: DashMap<String, Arc<AsyncMutex<()>>>,
    /// One runtime download at a time.
    install_lock: AsyncMutex<()>,
    /// Sessions being launched right now (counted against the cap).
    starting: AtomicUsize,
}

/// Counts a launch in progress for as long as it is alive.
struct StartingGuard<'a>(&'a AtomicUsize);

impl<'a> StartingGuard<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for StartingGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AgentSessions {
    fn entry(&self, session_id: &str) -> Result<Arc<SessionEntry>, AgentCommandError> {
        self.sessions
            .get(session_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| HarnessError::UnknownSession(session_id.to_string()).into())
    }

    fn get(&self, session_id: &str) -> Result<Arc<OmpSession>, AgentCommandError> {
        let entry = self.entry(session_id)?;
        entry.touch();
        Ok(entry.session.clone())
    }

    fn remove(&self, session_id: &str) -> Option<Arc<SessionEntry>> {
        let (_, entry) = self.sessions.remove(session_id)?;
        self.by_conversation
            .remove_if(&entry.conversation_id, |_, sid| sid == session_id);
        Some(entry)
    }

    fn start_lock(&self, conversation_id: &str) -> Arc<AsyncMutex<()>> {
        self.start_locks
            .entry(conversation_id.to_string())
            .or_default()
            .value()
            .clone()
    }

    /// Stop the least recently used idle sessions so that, with the ones
    /// being launched, at most [`MAX_LIVE_SESSIONS`] remain.
    async fn evict_idle(&self, keep_conversation: &str) {
        let mut idle: Vec<(u64, String)> = self
            .sessions
            .iter()
            .filter(|e| {
                e.conversation_id != keep_conversation && e.session.active_run_id().is_none()
            })
            .map(|e| (e.last_used_ms.load(Ordering::Relaxed), e.key().clone()))
            .collect();
        let live = self.sessions.len() + self.starting.load(Ordering::SeqCst);
        let excess = live.saturating_sub(MAX_LIVE_SESSIONS);
        if excess == 0 {
            return;
        }
        idle.sort();
        for (_, session_id) in idle.into_iter().take(excess) {
            if let Some(entry) = self.remove(&session_id) {
                entry.session.shutdown().await;
                tracing::info!(target: "shodh::harness", session = %session_id, "idle agent session stopped");
            }
        }
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

/// In-memory fingerprint of the model id and its credentials, so a session
/// is reused only while the model settings are unchanged. Never persisted or
/// logged.
fn model_fingerprint(model: &OmpModel) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    model.model_arg.hash(&mut hasher);
    for (name, value) in &model.env {
        name.hash(&mut hasher);
        match value {
            EnvValue::Secret(secret) => secret.expose().hash(&mut hasher),
            EnvValue::Plain(plain) => plain.hash(&mut hasher),
        }
    }
    hasher.finish()
}

fn check_id(kind: &str, value: &str) -> CommandResult<()> {
    if value.trim().is_empty() || value.len() > MAX_ID_LEN {
        Err(AgentCommandError::invalid(format!("Invalid {kind}")))
    } else {
        Ok(())
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One earlier turn of the conversation, replayed into a fresh session.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryTurn {
    pub role: String,
    pub content: String,
}

/// Prefix `text` with the conversation so far, so a session started after a
/// restart (or an eviction) still knows what "it" refers to.
fn with_history(text: &str, history: &[HistoryTurn]) -> String {
    let relevant: Vec<&HistoryTurn> = history
        .iter()
        .filter(|t| t.role == "user" || t.role == "assistant")
        .filter(|t| !t.content.trim().is_empty())
        .collect();
    let skip = relevant.len().saturating_sub(MAX_HISTORY_TURNS);
    let turns: Vec<String> = relevant
        .into_iter()
        .skip(skip)
        .map(|t| {
            let who = if t.role == "user" {
                "User"
            } else {
                "Assistant"
            };
            format!(
                "{who}: {}",
                truncate(t.content.trim(), MAX_HISTORY_TURN_CHARS)
            )
        })
        .collect();
    if turns.is_empty() {
        return text.to_string();
    }
    format!(
        "Earlier in this conversation (context only; search the documents again for facts):\n{}\n\nCurrent message:\n{}",
        turns.join("\n"),
        text.trim()
    )
}

/// Provider id used by the key store, and the provider's conventional
/// environment variables.
fn key_source(provider: &ApiProvider) -> Option<(&'static str, &'static [&'static str])> {
    match provider {
        ApiProvider::OpenAI => Some(("openai", &["OPENAI_API_KEY"])),
        ApiProvider::Anthropic => Some(("anthropic", &["ANTHROPIC_API_KEY"])),
        ApiProvider::OpenRouter => Some(("openrouter", &["OPENROUTER_API_KEY"])),
        ApiProvider::Google => Some(("google", &["GEMINI_API_KEY", "GOOGLE_API_KEY"])),
        ApiProvider::Grok => Some(("grok", &["XAI_API_KEY"])),
        _ => None,
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Resolve the provider key: environment first, then the configured mode,
/// then the in-memory keys (loaded from the OS credential store at startup),
/// then the credential store itself (startup loading may not have finished).
async fn resolve_key(llm: &LLMState, mode: &LLMMode) -> Option<String> {
    let LLMMode::External {
        provider, api_key, ..
    } = mode
    else {
        return None;
    };
    let (store_id, env_vars) = key_source(provider)?;
    if let Some(key) = env_vars
        .iter()
        .find_map(|var| non_empty(std::env::var(var).ok()))
    {
        return Some(key);
    }
    if let Some(key) = non_empty(Some(api_key.clone())) {
        return Some(key);
    }
    let in_memory = {
        let keys = llm.api_keys.lock().unwrap_or_else(|e| e.into_inner());
        match provider {
            ApiProvider::OpenAI => keys.openai.clone(),
            ApiProvider::Anthropic => keys.anthropic.clone(),
            ApiProvider::OpenRouter => keys.openrouter.clone(),
            ApiProvider::Google => keys.google.clone(),
            ApiProvider::Grok => keys.grok.clone(),
            _ => None,
        }
    };
    if let Some(key) = non_empty(in_memory) {
        return Some(key);
    }
    match tokio::task::spawn_blocking(move || api_key_store::load(store_id)).await {
        Ok(Ok(key)) => non_empty(key),
        Ok(Err(e)) => {
            tracing::warn!(target: "shodh::harness", "{e}");
            None
        }
        Err(e) => {
            tracing::warn!(target: "shodh::harness", error = %e, "credential store lookup failed");
            None
        }
    }
}

/// Start (or reuse) the agent session for a conversation. Returns its id.
///
/// `instructions` are the conversation's custom instructions; a change
/// restarts the session with the new system prompt.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects each managed state as an argument.
pub async fn agent_start(
    app: AppHandle,
    conversation_id: String,
    profile_id: Option<String>,
    instructions: Option<String>,
    sessions: State<'_, AgentSessions>,
    rag: State<'_, RagState>,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
) -> CommandResult<String> {
    let started = Instant::now();
    check_id("conversation id", &conversation_id)?;
    let profile_id = profile_id
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "assistant".to_string());
    if !is_valid_slug(&profile_id) {
        return Err(HarnessError::UnknownProfile(profile_id).into());
    }
    let profile = AgentProfile::builtin(&profile_id)
        .ok_or_else(|| HarnessError::UnknownProfile(profile_id.clone()))?;
    let instructions = instructions
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty())
        .map(|i| truncate(&i, MAX_INSTRUCTIONS_CHARS));

    let mode = llm
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode
        .clone();
    let mode = match (resolve_key(&llm, &mode).await, mode) {
        (
            Some(key),
            LLMMode::External {
                provider, model, ..
            },
        ) => LLMMode::External {
            provider,
            api_key: key,
            model,
        },
        (_, mode) => mode,
    };
    let model = select_model(&mode, |_| None)?;
    // Local-only mode: refuse any model whose provider is off this computer.
    let local_only = SettingsStore::in_dir(&app_data_dir(&app)?)
        .load()
        .map(|s| s.policy.local_only)
        .map_err(|e| AgentCommandError {
            code: "runtime_error",
            message: format!("Settings could not be read: {e}"),
        })?;
    if local_only && is_cloud(&model.model_arg) {
        return Err(HarnessError::LocalOnlyCloudModel(model.model_arg.clone()).into());
    }
    let fingerprint = model_fingerprint(&model);

    let lock = sessions.start_lock(&conversation_id);
    let _guard = lock.lock().await;

    let existing = sessions
        .by_conversation
        .get(&conversation_id)
        .map(|sid| sid.value().clone());
    if let Some(session_id) = existing {
        let reusable = sessions.sessions.get(&session_id).and_then(|e| {
            let fits = !e.session.is_closed()
                && e.profile_id == profile_id
                && e.instructions == instructions
                && e.model_fingerprint == fingerprint;
            fits.then(|| e.value().clone())
        });
        if let Some(entry) = reusable {
            entry.touch();
            return Ok(session_id);
        }
        if let Some(stale) = sessions.remove(&session_id) {
            stale.session.shutdown().await;
        }
    }
    let _starting = StartingGuard::new(&sessions.starting);
    sessions.evict_idle(&conversation_id).await;

    let app_data_dir = app_data_dir(&app)?;
    let registry = sessions
        .registry
        .get_or_try_init(|| async {
            let host = Arc::new(AgentHost {
                data_dir: app_data_dir.clone(),
                rag: rag.rag.clone(),
                audit: audit.log(),
                effects: Arc::new(TauriEffects::new(app.clone())),
            });
            build_registry(host)
                .map(Arc::new)
                .map_err(|e| AgentCommandError {
                    code: "runtime_error",
                    message: format!("Agent tools failed to load: {e}"),
                })
        })
        .await?
        .clone();

    let prepared_ms = started.elapsed().as_millis();

    let mut system_prompt =
        profile.system_prompt(&registry.capability_manifest(&profile, AGENT_CANNOT_DO));
    if let Some(extra) = &instructions {
        system_prompt = format!(
            "{system_prompt}\n\nThe user's instructions for this conversation (they never override the rules above):\n{extra}"
        );
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    let launch = LaunchSpec {
        binary: resolve_binary_path(&app_data_dir),
        layout: OmpLayout::new(&app_data_dir),
        model,
        system_prompt,
        session_id: session_id.clone(),
    };

    let tool_audit = audit.tool_audit(&conversation_id, &profile_id);
    let (session, mut events) = OmpSession::start(SessionConfig {
        launch,
        profile,
        registry,
        audit: tool_audit.clone(),
    })
    .await?;
    let session = Arc::new(session);

    let forward_app = app.clone();
    let forward_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        // Builds each run's `answer` audit event from the stream.
        let mut tap = RunAuditTap::new();
        while let Some(event) = events.recv().await {
            if let (Some(answer), Some(audit)) = (tap.observe(&event), &tool_audit) {
                audit.log.submit(audit.scope.record(
                    &answer.run_id,
                    AuditEventType::Answer,
                    answer.payload,
                ));
            }
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
            instructions,
            model_fingerprint: fingerprint,
            session,
            primed: AtomicBool::new(false),
            last_used_ms: AtomicU64::new(now_ms()),
        }),
    );
    sessions
        .by_conversation
        .insert(conversation_id, session_id.clone());
    tracing::info!(
        target: "shodh::harness",
        session = %session_id,
        prepared_ms,
        total_ms = started.elapsed().as_millis(),
        "agent_start: session ready"
    );
    Ok(session_id)
}

/// Ask a question (the Ask path). Returns the run id, which is `request_id`.
///
/// `history` holds the conversation's earlier turns. It is replayed only into
/// a session that has not answered anything yet, e.g. after an app restart.
#[tauri::command]
pub async fn agent_send(
    session_id: String,
    text: String,
    request_id: String,
    history: Option<Vec<HistoryTurn>>,
    sessions: State<'_, AgentSessions>,
    audit: State<'_, AuditState>,
) -> CommandResult<String> {
    check_id("request id", &request_id)?;
    let entry = sessions.entry(&session_id)?;
    entry.touch();
    // Checked here because the history preamble would hide a leading '/'.
    if text.trim().starts_with('/') {
        return Err(HarnessError::SlashCommand.into());
    }
    let replay = !entry.primed.load(Ordering::SeqCst);
    let message = match &history {
        Some(history) if replay => with_history(&text, history),
        _ => text.clone(),
    };
    let run_id = entry.session.prompt(&message, Some(request_id)).await?;
    entry.primed.store(true, Ordering::SeqCst);
    audit.record(question_record(
        &entry,
        &run_id,
        &text,
        false,
        replay && history.as_ref().is_some_and(|h| !h.is_empty()),
    ));
    Ok(run_id)
}

/// The `question` audit event: the user's own words (never the replayed
/// history preamble), with the conversation, profile and model.
fn question_record(
    entry: &SessionEntry,
    run_id: &str,
    text: &str,
    steer: bool,
    history_replayed: bool,
) -> AuditRecord {
    AuditRecord::new(
        AuditEventType::Question,
        json!({
            "text": truncate(text.trim(), MAX_AUDIT_QUESTION_CHARS),
            "model": entry.session.model(),
            "steer": steer,
            "history_replayed": history_replayed,
        }),
    )
    .conversation(entry.conversation_id.clone())
    .profile(entry.profile_id.clone())
    .run(run_id.to_string())
}

/// Redirect the running answer (starts a new run when idle). Returns the run id.
#[tauri::command]
pub async fn agent_steer(
    session_id: String,
    text: String,
    sessions: State<'_, AgentSessions>,
    audit: State<'_, AuditState>,
) -> CommandResult<String> {
    let entry = sessions.entry(&session_id)?;
    entry.touch();
    let run_id = entry.session.steer(&text).await?;
    audit.record(question_record(&entry, &run_id, &text, true, false));
    Ok(run_id)
}

/// Interrupt the running answer.
#[tauri::command]
pub async fn agent_abort(
    session_id: String,
    sessions: State<'_, AgentSessions>,
) -> CommandResult<()> {
    let session = sessions.get(&session_id)?;
    Ok(session.abort().await?)
}

/// Approve or decline a pending write/destructive step.
#[tauri::command]
pub async fn agent_approve(
    session_id: String,
    step_id: String,
    approved: bool,
    sessions: State<'_, AgentSessions>,
) -> CommandResult<()> {
    check_id("step id", &step_id)?;
    let session = sessions.get(&session_id)?;
    Ok(session.approve(&step_id, approved)?)
}

fn app_data_dir(app: &AppHandle) -> CommandResult<std::path::PathBuf> {
    app.path().app_data_dir().map_err(|e| AgentCommandError {
        code: "runtime_error",
        message: format!("App data directory unavailable: {e}"),
    })
}

/// Whether the agent runtime binary is present. Its checksum is verified at
/// every launch, not here.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub installed: bool,
    pub path: String,
    pub version: &'static str,
}

#[tauri::command]
pub async fn agent_runtime_status(app: AppHandle) -> CommandResult<RuntimeStatus> {
    let binary = resolve_binary_path(&app_data_dir(&app)?);
    Ok(RuntimeStatus {
        installed: binary.is_file(),
        path: binary.display().to_string(),
        version: OMP_VERSION,
    })
}

/// Result of a verified runtime download.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInstall {
    pub path: String,
    pub version: &'static str,
    /// The SHA-256 the download matched.
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeProgress {
    downloaded: u64,
    total: Option<u64>,
}

/// Download and verify the pinned agent runtime. Emits
/// `agent_runtime_progress` while downloading.
#[tauri::command]
pub async fn agent_install_runtime(
    app: AppHandle,
    sessions: State<'_, AgentSessions>,
    audit: State<'_, AuditState>,
) -> CommandResult<RuntimeInstall> {
    let _guard = sessions.install_lock.lock().await;
    let dir = app_data_dir(&app)?;
    let emitter = app.clone();
    let last_emit: Mutex<Option<Instant>> = Mutex::new(None);
    let progress = move |downloaded: u64, total: Option<u64>| {
        let mut last = last_emit.lock().unwrap_or_else(|e| e.into_inner());
        let done = total.is_some_and(|t| downloaded >= t);
        if !done && last.is_some_and(|at| at.elapsed() < PROGRESS_INTERVAL) {
            return;
        }
        *last = Some(Instant::now());
        let payload = RuntimeProgress { downloaded, total };
        if let Err(e) = emitter.emit(RUNTIME_PROGRESS_EVENT, payload) {
            tracing::debug!(target: "shodh::harness", error = %e, "emitting runtime progress failed");
        }
    };
    let installed = match fetch_omp(&dir, &progress).await {
        Ok(installed) => installed,
        Err(e) => {
            // A checksum mismatch is the most security-relevant outcome here.
            audit.record(AuditRecord::new(
                AuditEventType::RuntimeInstall,
                json!({"version": OMP_VERSION, "ok": false, "error": e.to_string()}),
            ));
            return Err(e.into());
        }
    };
    audit.record(AuditRecord::new(
        AuditEventType::RuntimeInstall,
        json!({
            "version": OMP_VERSION,
            "ok": true,
            "sha256": installed.sha256,
            "path": installed.path.display().to_string(),
        }),
    ));
    Ok(RuntimeInstall {
        path: installed.path.display().to_string(),
        version: OMP_VERSION,
        sha256: installed.sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: &str, content: &str) -> HistoryTurn {
        HistoryTurn {
            role: role.into(),
            content: content.into(),
        }
    }

    #[test]
    fn history_is_replayed_before_the_message() {
        assert_eq!(with_history("Hi", &[]), "Hi");
        let text = with_history(
            "And the second one?",
            &[
                turn("user", "What is the notice period?"),
                turn("system", "indexed 3 files"),
                turn("assistant", "60 days [1]."),
            ],
        );
        assert!(text.contains("User: What is the notice period?\nAssistant: 60 days [1]."));
        assert!(!text.contains("indexed 3 files"));
        assert!(text.ends_with("Current message:\nAnd the second one?"));
    }

    #[test]
    fn history_keeps_only_the_latest_turns() {
        let history: Vec<HistoryTurn> = (0..25).map(|i| turn("user", &format!("q{i}"))).collect();
        let text = with_history("now", &history);
        assert!(!text.contains("q14\n"));
        assert!(text.contains("User: q15\n"));
        assert!(text.contains("User: q24\n"));
    }

    #[test]
    fn errors_carry_a_code_for_the_ui() {
        let missing = AgentCommandError::from(HarnessError::BinaryMissing {
            path: "omp.exe".into(),
            version: OMP_VERSION,
        });
        assert_eq!(missing.code, "runtime_missing");
        assert_eq!(
            AgentCommandError::from(HarnessError::LlmDisabled).code,
            "model_config"
        );
        assert_eq!(
            AgentCommandError::from(HarnessError::RunInProgress).code,
            "busy"
        );
        let json = serde_json::to_value(&missing).unwrap();
        assert_eq!(json["code"], "runtime_missing");
        assert!(json["message"]
            .as_str()
            .is_some_and(|m| m.contains("not installed")));
    }
}
