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
use shodh_rag::harness::tools::{RunScope, ToolRegistry};
use shodh_rag::harness::{
    fetch_omp, resolve_binary_path, select_model, AgentEvent, AgentHarness, AgentProfile,
    HarnessError, LaunchSpec, OmpLayout, OmpModel, OmpSession, SessionConfig, OMP_VERSION,
};
use shodh_rag::llm::{ApiProvider, LLMMode};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use crate::agent_tools::{
    build_registry, web_block_reason, AgentHost, IndexedRoots, TauriEffects, AGENT_CANNOT_DO,
};
use crate::api_key_store;
use crate::app_settings::SettingsStore;
use crate::audit_commands::AuditState;
use crate::llm_commands::LLMState;
use crate::memory_commands::{recall_for_run, with_memories, MemoryState};
use crate::memory_learn::{LearnState, TextOrigin};
use crate::rag_commands::RagState;
use crate::visual_commands::VisualState;
use shodh_rag::audit::payload::is_cloud;
use shodh_rag::audit::LOCAL_OWNER;
use shodh_rag::harness::web::SafeClient;
use shodh_rag::user_memory::Actor;

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
    /// For a focus side thread: the conversation it belongs to. Its tool
    /// calls and questions are audited under that conversation, and its
    /// session is evicted before ordinary ones.
    parent_conversation_id: Option<String>,
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

    /// The conversation the audit log attributes this session's work to.
    fn audit_conversation(&self) -> &str {
        self.parent_conversation_id
            .as_deref()
            .unwrap_or(&self.conversation_id)
    }
}

/// A live session that could be stopped to make room.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EvictionCandidate {
    session_id: String,
    conversation_id: String,
    /// A focus side-thread session.
    focus: bool,
    /// An answer is running.
    busy: bool,
    last_used_ms: u64,
}

/// Which sessions to stop so that `excess` fewer remain. Never the
/// conversations in `keep` (the one starting and, for a side thread, its
/// parent) and never one with an answer running. Idle side-thread sessions
/// go first (they are cheap to restart and their history is replayed),
/// then the least recently used.
fn eviction_order(candidates: &[EvictionCandidate], keep: &[&str], excess: usize) -> Vec<String> {
    let mut idle: Vec<&EvictionCandidate> = candidates
        .iter()
        .filter(|c| !c.busy && !keep.contains(&c.conversation_id.as_str()))
        .collect();
    idle.sort_by(|a, b| {
        b.focus
            .cmp(&a.focus)
            .then(a.last_used_ms.cmp(&b.last_used_ms))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    idle.into_iter()
        .take(excess)
        .map(|c| c.session_id.clone())
        .collect()
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

    /// Stop idle sessions so that, with the ones being launched, at most
    /// [`MAX_LIVE_SESSIONS`] remain (see [`eviction_order`]).
    async fn evict_idle(&self, keep: &[&str]) {
        let live = self.sessions.len() + self.starting.load(Ordering::SeqCst);
        let excess = live.saturating_sub(MAX_LIVE_SESSIONS);
        if excess == 0 {
            return;
        }
        let candidates: Vec<EvictionCandidate> = self
            .sessions
            .iter()
            .map(|e| EvictionCandidate {
                session_id: e.key().clone(),
                conversation_id: e.conversation_id.clone(),
                focus: e.parent_conversation_id.is_some(),
                busy: e.session.active_run_id().is_some(),
                last_used_ms: e.last_used_ms.load(Ordering::Relaxed),
            })
            .collect();
        for session_id in eviction_order(&candidates, keep, excess) {
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

/// Most sources or files one answer may be limited to.
const MAX_SCOPE_ITEMS: usize = 50;

/// Most pages one answer may be limited to.
const MAX_SCOPE_PAGES: usize = 200;

/// What the user limited an answer to: selected sources ("Include this
/// source when answering"), files ("Ask about this file") and, for files,
/// pages (a side question about a passage on page 4).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SendScope {
    pub source_ids: Vec<String>,
    pub source_files: Vec<String>,
    /// 1-based pages of `source_files`; absent or empty means every page.
    pub pages: Option<Vec<u32>>,
    /// The conversation's workspace (source / space id). Scopes memories, not search.
    pub workspace_id: Option<String>,
}

impl SendScope {
    fn validated(self) -> CommandResult<RunScope> {
        let clean = |items: Vec<String>, what: &str| -> CommandResult<Vec<String>> {
            let items: Vec<String> = items
                .into_iter()
                .map(|i| i.trim().to_string())
                .filter(|i| !i.is_empty())
                .collect();
            if items.len() > MAX_SCOPE_ITEMS {
                return Err(AgentCommandError::invalid(format!(
                    "At most {MAX_SCOPE_ITEMS} {what} can limit one answer"
                )));
            }
            if items.iter().any(|i| i.len() > 2048) {
                return Err(AgentCommandError::invalid(format!(
                    "A {what} entry is too long"
                )));
            }
            Ok(items)
        };
        let files = clean(self.source_files, "files")?;
        let mut pages = self.pages.unwrap_or_default();
        pages.sort_unstable();
        pages.dedup();
        if !pages.is_empty() && files.is_empty() {
            return Err(AgentCommandError::invalid(
                "Pages can only limit an answer to files; give sourceFiles too",
            ));
        }
        if pages.contains(&0) {
            return Err(AgentCommandError::invalid("Page numbers start at 1"));
        }
        if pages.len() > MAX_SCOPE_PAGES {
            return Err(AgentCommandError::invalid(format!(
                "At most {MAX_SCOPE_PAGES} pages can limit one answer"
            )));
        }
        let workspace = self
            .workspace_id
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty());
        if workspace.as_ref().is_some_and(|w| w.len() > MAX_ID_LEN) {
            return Err(AgentCommandError::invalid("The workspace id is too long"));
        }
        Ok(RunScope {
            source_ids: clean(self.source_ids, "sources")?,
            files,
            pages,
            workspace,
        })
    }
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
pub(crate) async fn resolve_key(llm: &LLMState, mode: &LLMMode) -> Option<String> {
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
///
/// `parent_conversation_id` marks a focus side-thread session: its work is
/// audited under the parent conversation, the parent's session is kept
/// alive while it starts, and idle side-thread sessions are evicted first.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects each managed state as an argument.
pub async fn agent_start(
    app: AppHandle,
    conversation_id: String,
    profile_id: Option<String>,
    instructions: Option<String>,
    parent_conversation_id: Option<String>,
    sessions: State<'_, AgentSessions>,
    rag: State<'_, RagState>,
    llm: State<'_, LLMState>,
    audit: State<'_, AuditState>,
    memory: State<'_, MemoryState>,
    visuals: State<'_, VisualState>,
    learn: State<'_, LearnState>,
) -> CommandResult<String> {
    let started = Instant::now();
    check_id("conversation id", &conversation_id)?;
    let parent_conversation_id = parent_conversation_id
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty() && *p != conversation_id);
    if let Some(parent) = &parent_conversation_id {
        check_id("parent conversation id", parent)?;
    }
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
                && e.parent_conversation_id == parent_conversation_id
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
    let keep: Vec<&str> = std::iter::once(conversation_id.as_str())
        .chain(parent_conversation_id.as_deref())
        .collect();
    sessions.evict_idle(&keep).await;

    let app_data_dir = app_data_dir(&app)?;
    let registry = sessions
        .registry
        .get_or_try_init(|| async {
            let host = Arc::new(AgentHost {
                data_dir: app_data_dir.clone(),
                rag: rag.rag.clone(),
                audit: audit.log(),
                effects: Arc::new(TauriEffects::new(app.clone())),
                web: SafeClient::system(),
                roots: Arc::new(IndexedRoots {
                    rag: rag.rag.clone(),
                }),
                memory: memory.inner().clone(),
                visuals: visuals.inner().clone(),
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

    // Web tools stay registered (policy can change mid-session and each
    // call re-checks it), but the model is told up front when they are off.
    let web_off = web_block_reason(&app_data_dir).map(|reason| {
        format!("search the web, read web pages or search papers right now: {reason}")
    });
    let cannot_do: Vec<&str> = AGENT_CANNOT_DO
        .iter()
        .copied()
        .chain(web_off.as_deref())
        .collect();
    let mut system_prompt =
        profile.system_prompt(&registry.capability_manifest(&profile, &cannot_do));
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

    let audit_conversation = parent_conversation_id
        .as_deref()
        .unwrap_or(&conversation_id);
    let tool_audit = audit.tool_audit(audit_conversation, &profile_id);
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
    let forward_learn = learn.inner().clone();
    tauri::async_runtime::spawn(async move {
        // Builds each run's `answer` audit event from the stream.
        let mut tap = RunAuditTap::new();
        while let Some(event) = events.recv().await {
            // Learning sees only runs `agent_send` registered (the user's own words).
            forward_learn.observe(&event);
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
            parent_conversation_id,
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
///
/// When memory injection is on (Settings → Memory, default on), memories relevant to
/// `text` are recalled and put in front of the message in a delimited block; the
/// `question` audit event keeps the user's own words only.
///
/// `text_origin` is `typed` when `text` is exactly what the user typed in the main
/// conversation; only such turns are learned from (Settings → Memory → "Learn from
/// conversations"). Side-thread sessions are never learned from.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects each managed state as an argument.
pub async fn agent_send(
    app: AppHandle,
    session_id: String,
    text: String,
    request_id: String,
    history: Option<Vec<HistoryTurn>>,
    scope: Option<SendScope>,
    text_origin: Option<TextOrigin>,
    sessions: State<'_, AgentSessions>,
    audit: State<'_, AuditState>,
    memory: State<'_, MemoryState>,
    learn: State<'_, LearnState>,
) -> CommandResult<String> {
    check_id("request id", &request_id)?;
    let scope = scope
        .map(SendScope::validated)
        .transpose()?
        .unwrap_or_default();
    let entry = sessions.entry(&session_id)?;
    entry.touch();
    // Checked here because the history preamble would hide a leading '/'.
    if text.trim().starts_with('/') {
        return Err(HarnessError::SlashCommand.into());
    }
    let replay = !entry.primed.load(Ordering::SeqCst);
    let memories = if inject_memories(&app) {
        let actor = Actor::agent(
            LOCAL_OWNER,
            entry.audit_conversation(),
            &entry.profile_id,
            &request_id,
        );
        recall_for_run(&memory, &text, scope.workspace.as_deref(), &actor).await
    } else {
        None
    };
    let message = match &history {
        Some(history) if replay => {
            let with_turns = with_history(&text, history);
            match &memories {
                Some(block) => format!(
                    "{block}

{with_turns}"
                ),
                None => with_turns,
            }
        }
        _ => with_memories(memories.as_deref(), &text),
    };
    let scoped = !scope.is_empty();
    // Registered before prompting: the run id is `request_id`, and the first events may
    // arrive before `prompt_scoped` returns.
    let learnable =
        text_origin == Some(TextOrigin::Typed) && entry.parent_conversation_id.is_none();
    if learnable {
        learn.record_question(&entry.conversation_id, &request_id, &text);
    }
    let run_id = match entry
        .session
        .prompt_scoped(&message, Some(request_id.clone()), scope.clone())
        .await
    {
        Ok(run_id) => run_id,
        Err(e) => {
            learn.forget_run(&request_id);
            return Err(e.into());
        }
    };
    entry.primed.store(true, Ordering::SeqCst);
    if scoped {
        tracing::info!(target: "shodh::harness", run_id = %run_id, sources = scope.source_ids.len(), files = scope.files.len(), pages = scope.pages.len(), "answer limited to a scope");
    }
    audit.record(question_record(
        &entry,
        &run_id,
        &text,
        false,
        replay && history.as_ref().is_some_and(|h| !h.is_empty()),
    ));
    Ok(run_id)
}

/// Whether memories are recalled into answers (Settings → Memory). Unreadable settings
/// leave them out: sharing memories with the model needs the user's setting.
fn inject_memories(app: &AppHandle) -> bool {
    let settings = app_data_dir(app)
        .ok()
        .map(|dir| SettingsStore::in_dir(&dir).load());
    match settings {
        Some(Ok(settings)) => settings.memory.inject_memories,
        Some(Err(e)) => {
            tracing::warn!(target: "shodh::memory", error = %e, "settings unreadable; memories not injected");
            false
        }
        None => false,
    }
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
    .conversation(entry.audit_conversation().to_string())
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

    fn candidate(
        id: &str,
        conversation: &str,
        focus: bool,
        busy: bool,
        used: u64,
    ) -> EvictionCandidate {
        EvictionCandidate {
            session_id: id.into(),
            conversation_id: conversation.into(),
            focus,
            busy,
            last_used_ms: used,
        }
    }

    #[test]
    fn idle_focus_sessions_are_evicted_first_and_parents_kept() {
        let live = [
            candidate("s-old", "c-old", false, false, 10),
            candidate("s-parent", "c-parent", false, false, 5),
            candidate("s-focus-new", "c-x--focus--t2", true, false, 90),
            candidate("s-focus-old", "c-x--focus--t1", true, false, 50),
            candidate("s-busy-focus", "c-y--focus--t3", true, true, 1),
            candidate("s-busy", "c-busy", false, true, 2),
        ];
        let keep = ["c-new--focus--t9", "c-parent"];
        assert_eq!(eviction_order(&live, &keep, 1), vec!["s-focus-old"]);
        assert_eq!(
            eviction_order(&live, &keep, 3),
            vec!["s-focus-old", "s-focus-new", "s-old"],
            "then the least recently used ordinary session"
        );
        // Never a kept conversation or a running answer, however many are asked for.
        let all = eviction_order(&live, &keep, 10);
        assert_eq!(all.len(), 3);
        assert!(!all
            .iter()
            .any(|s| s == "s-parent" || s.starts_with("s-busy")));
        assert!(eviction_order(&live, &keep, 0).is_empty());
    }

    #[test]
    fn scopes_limit_pages_only_within_files() {
        let scope = SendScope {
            source_files: vec![" C:/docs/a.pdf ".into()],
            pages: Some(vec![9, 4, 4]),
            ..SendScope::default()
        }
        .validated()
        .unwrap();
        assert_eq!(scope.files, vec!["C:/docs/a.pdf"]);
        assert_eq!(scope.pages, vec![4, 9], "sorted and deduplicated");
        let no_files = SendScope {
            pages: Some(vec![1]),
            ..SendScope::default()
        };
        assert!(no_files.validated().is_err());
        let zero = SendScope {
            source_files: vec!["a.pdf".into()],
            pages: Some(vec![0]),
            ..SendScope::default()
        };
        assert!(zero.validated().is_err());
        let wire: SendScope =
            serde_json::from_str(r#"{"sourceFiles": ["a.pdf"], "pages": [2]}"#).unwrap();
        assert_eq!(wire.validated().unwrap().pages, vec![2]);
        let old: SendScope = serde_json::from_str(r#"{"sourceFiles": ["a.pdf"]}"#).unwrap();
        assert!(old.validated().unwrap().pages.is_empty());
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
