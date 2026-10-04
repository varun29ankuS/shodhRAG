//! Learning memories from conversations (Settings → Memory → "Learn from conversations"):
//! the app side of `shodh_rag::user_memory::learn`.
//!
//! - **Capture.** `agent_send` hands over the user's own words of a turn the frontend
//!   marks as typed by the user (not a side-thread question, not a composed summary
//!   request); the agent event stream supplies the answer text (context only) and whether
//!   the run completed. Aborted and failed runs are not learned from.
//! - **Debounce.** Completed turns of a conversation are collected for [`DEBOUNCE`]; a new
//!   turn restarts the wait, so a quick back-and-forth is one extraction (at most
//!   [`MAX_BATCH_TURNS`] turns).
//! - **Model.** The configured provider, optionally with a cheaper model id
//!   (`learnModel`); a local model in Local-only mode, never a cloud one.
//! - **Consolidation.** At most daily, when the app has been idle for [`IDLE_BEFORE_SLEEP`].
//! - **Kill switch.** `SHODH_MEMORY_LEARNING=off` (managed installs) forces learning off;
//!   "Stop learning" in Settings turns it off, drops queued turns and rejects waiting
//!   suggestions.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditKey, AuditRecord};
use shodh_rag::harness::{AgentEvent, RunStatus};
use shodh_rag::llm::{
    ApiProvider, GenerationConfig, LLMConfig, LLMManager, LLMMode, LLMProvider,
    SimpleExternalProvider,
};
use shodh_rag::user_memory::learn::engine::ModelSource;
use shodh_rag::user_memory::learn::{
    ConsolidationReport, Inbox, LearnCaps, LearnError, LearnMode, LearnModel, LearnPolicy, Learner,
    PolicySource, ProposalStatus, ProposalView, TurnInput, UsageToday,
};
use shodh_rag::user_memory::{Actor, MemoryContent};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{OnceCell, RwLock as AsyncRwLock};

use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;
use crate::llm_commands::{ApiKeys, LLMState};
use crate::memory_commands::MemoryState;

/// Emitted with `{ pending }` whenever suggestions change.
pub const SUGGESTIONS_CHANGED_EVENT: &str = "memory-suggestions-changed";

/// Environment kill switch: `off` disables learning whatever the settings say.
pub const KILL_SWITCH_VAR: &str = "SHODH_MEMORY_LEARNING";

/// Quiet time after a turn before its conversation is learned from.
pub const DEBOUNCE: Duration = Duration::from_secs(20);
/// Turns learned from in one extraction.
pub const MAX_BATCH_TURNS: usize = 4;
/// Answer text kept per run as context.
const MAX_ANSWER_CONTEXT: usize = 4_000;
/// Runs awaiting completion that are tracked at once.
const MAX_TRACKED_RUNS: usize = 64;
/// How often the background loop checks whether to consolidate.
const SLEEP_CHECK: Duration = Duration::from_secs(30 * 60);
/// Idle time before consolidating.
pub const IDLE_BEFORE_SLEEP: Duration = Duration::from_secs(10 * 60);

/// Whether the environment kill switch is on.
pub fn kill_switch() -> bool {
    std::env::var(KILL_SWITCH_VAR)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "off" | "0" | "false"
            )
        })
        .unwrap_or(false)
}

/// The policy from the settings: the kill switch wins; unreadable settings mean off.
fn policy_from(settings: Option<&AppSettings>) -> LearnPolicy {
    let Some(settings) = settings else {
        return LearnPolicy {
            mode: LearnMode::Off,
            ..LearnPolicy::default()
        };
    };
    let memory = &settings.memory;
    LearnPolicy {
        mode: if kill_switch() {
            LearnMode::Off
        } else {
            memory.learn_mode
        },
        auto_min_confidence: memory.auto_min_confidence,
        share_memories_with_model: memory.inject_memories,
        caps: memory.learn_caps,
    }
}

struct SettingsPolicy(PathBuf);

impl PolicySource for SettingsPolicy {
    fn policy(&self) -> LearnPolicy {
        match SettingsStore::in_dir(&self.0).load() {
            Ok(settings) => policy_from(Some(&settings)),
            Err(e) => {
                tracing::warn!(target: "shodh::memory", error = %e, "settings unreadable; learning is off");
                policy_from(None)
            }
        }
    }
}

/// Which model learning would use, or why none.
#[derive(Debug, Clone)]
enum ModelChoice {
    /// The local model loaded in the LLM manager.
    Local { id: String },
    /// An API provider (Ollama counts as local).
    External {
        provider: ApiProvider,
        model: String,
        local: bool,
    },
}

/// Chooses the learning model from the LLM mode and settings (pure, tested).
fn choose_model(
    mode: &LLMMode,
    learn_model: Option<&str>,
    local_only: bool,
) -> Result<ModelChoice, String> {
    match mode {
        LLMMode::Disabled => Err("no model is configured (Settings → Model)".to_string()),
        LLMMode::Local { model_path } => Ok(ModelChoice::Local {
            id: model_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "local".to_string()),
        }),
        LLMMode::External {
            provider, model, ..
        } => {
            let local = matches!(provider, ApiProvider::Ollama);
            if local_only && !local {
                return Err(
                    "Local-only mode is on: learning needs a local model (Settings → Model)"
                        .to_string(),
                );
            }
            Ok(ModelChoice::External {
                provider: provider.clone(),
                model: learn_model
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .unwrap_or(model)
                    .to_string(),
                local,
            })
        }
    }
}

/// The app's model for learning, resolved per call from the current settings.
struct AppModel {
    data_dir: PathBuf,
    manager: Arc<AsyncRwLock<Option<LLMManager>>>,
    config: Arc<std::sync::Mutex<LLMConfig>>,
    api_keys: Arc<std::sync::Mutex<ApiKeys>>,
    choice: ModelChoice,
}

impl AppModel {
    fn generation(&self, max_tokens: usize) -> GenerationConfig {
        let config = self
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut generation = GenerationConfig::from(&config);
        generation.max_tokens = max_tokens;
        // Extraction is classification, not writing: no creativity wanted.
        generation.temperature = 0.0;
        generation
    }
}

#[async_trait::async_trait]
impl LearnModel for AppModel {
    fn model_id(&self) -> String {
        match &self.choice {
            ModelChoice::Local { id } => id.clone(),
            ModelChoice::External { model, .. } => model.clone(),
        }
    }

    async fn complete(&self, prompt: &str, max_output_tokens: usize) -> Result<String, LearnError> {
        // Re-checked per call: the user may have turned Local-only mode on since.
        let local_only = SettingsStore::in_dir(&self.data_dir)
            .load()
            .map(|s| s.policy.local_only)
            .unwrap_or(true);
        match &self.choice {
            ModelChoice::Local { .. } => {
                let guard = self.manager.read().await;
                let manager = guard.as_ref().ok_or_else(|| {
                    LearnError::ModelUnavailable("the local model is not loaded".into())
                })?;
                manager
                    .generate_custom(prompt, max_output_tokens)
                    .await
                    .map_err(|e| LearnError::Model(format!("{e:#}")))
            }
            ModelChoice::External {
                provider,
                model,
                local,
            } => {
                if local_only && !local {
                    return Err(LearnError::ModelUnavailable(
                        "Local-only mode is on".to_string(),
                    ));
                }
                let mode = self
                    .config
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .mode
                    .clone();
                let llm = LLMState {
                    manager: self.manager.clone(),
                    config: self.config.clone(),
                    api_keys: self.api_keys.clone(),
                    custom_model_path: Arc::new(std::sync::Mutex::new(None)),
                };
                let key = match provider {
                    ApiProvider::Ollama => "ollama".to_string(),
                    _ => crate::agent_session_commands::resolve_key(&llm, &mode)
                        .await
                        .ok_or_else(|| {
                            LearnError::ModelUnavailable(
                                "no API key for the configured provider".to_string(),
                            )
                        })?,
                };
                let client = SimpleExternalProvider::new(provider.clone(), key, model.clone())
                    .map_err(|e| LearnError::Model(format!("{e:#}")))?;
                client
                    .generate(prompt, &self.generation(max_output_tokens))
                    .await
                    .map_err(|e| LearnError::Model(format!("{e:#}")))
            }
        }
    }
}

/// The model answers come from (without the learning override), for one-shot extraction
/// tasks outside memory such as naming the columns of a results table. Honours Local-only
/// mode like learning does: a cloud model is refused while it is on.
pub(crate) fn configured_model(
    data_dir: &Path,
    llm: &LLMState,
) -> Result<Arc<dyn shodh_rag::research::results::TextModel>, String> {
    let settings = SettingsStore::in_dir(data_dir)
        .load()
        .map_err(|e| e.to_string())?;
    let mode = llm
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode
        .clone();
    let choice = choose_model(&mode, None, settings.policy.local_only)?;
    Ok(Arc::new(AppModel {
        data_dir: data_dir.to_path_buf(),
        manager: llm.manager.clone(),
        config: llm.config.clone(),
        api_keys: llm.api_keys.clone(),
        choice,
    }))
}

#[async_trait::async_trait]
impl shodh_rag::research::results::TextModel for AppModel {
    fn model_id(&self) -> String {
        LearnModel::model_id(self)
    }

    async fn complete(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
        LearnModel::complete(self, prompt, max_tokens)
            .await
            .map_err(|e| e.to_string())
    }
}

struct AppModels {
    data_dir: PathBuf,
    manager: Arc<AsyncRwLock<Option<LLMManager>>>,
    config: Arc<std::sync::Mutex<LLMConfig>>,
    api_keys: Arc<std::sync::Mutex<ApiKeys>>,
}

impl ModelSource for AppModels {
    fn model(&self) -> Result<Arc<dyn LearnModel>, String> {
        let settings = SettingsStore::in_dir(&self.data_dir)
            .load()
            .map_err(|e| e.to_string())?;
        let mode = self
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode
            .clone();
        let choice = choose_model(
            &mode,
            settings.memory.learn_model.as_deref(),
            settings.policy.local_only,
        )?;
        Ok(Arc::new(AppModel {
            data_dir: self.data_dir.clone(),
            manager: self.manager.clone(),
            config: self.config.clone(),
            api_keys: self.api_keys.clone(),
            choice,
        }))
    }
}

/// A run whose answer is streaming.
struct PendingRun {
    conversation_id: String,
    user_text: String,
    /// Answer text by text block (a revised answer drops the draft it replaced).
    answer: Vec<(String, String)>,
    answer_len: usize,
    at: DateTime<Utc>,
}

/// Completed turns of one conversation waiting for the debounce.
#[derive(Default)]
struct Batch {
    turns: Vec<TurnInput>,
    generation: u64,
}

/// Managed state of learning.
#[derive(Clone)]
pub struct LearnState {
    inner: Arc<Inner>,
}

struct Inner {
    data_dir: PathBuf,
    memory: MemoryState,
    database: Option<(PathBuf, Option<AuditKey>)>,
    models: Arc<AppModels>,
    learner: OnceCell<Arc<Learner>>,
    runs: DashMap<String, PendingRun>,
    batches: DashMap<String, Batch>,
    generation: AtomicU64,
    last_activity_ms: AtomicI64,
    app: AppHandle,
}

impl LearnState {
    /// Learning for the app (opened on first use).
    pub fn new(
        app: AppHandle,
        data_dir: &Path,
        memory: MemoryState,
        llm: &LLMState,
        audit: &AuditState,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                data_dir: data_dir.to_path_buf(),
                memory,
                database: audit.database(),
                models: Arc::new(AppModels {
                    data_dir: data_dir.to_path_buf(),
                    manager: llm.manager.clone(),
                    config: llm.config.clone(),
                    api_keys: llm.api_keys.clone(),
                }),
                learner: OnceCell::new(),
                runs: DashMap::new(),
                batches: DashMap::new(),
                generation: AtomicU64::new(0),
                last_activity_ms: AtomicI64::new(Utc::now().timestamp_millis()),
                app,
            }),
        }
    }

    /// The learner, opening it (and the memory service) if needed.
    pub async fn learner(&self) -> Result<Arc<Learner>, String> {
        self.inner
            .learner
            .get_or_try_init(|| async {
                let service = self.inner.memory.service().await?;
                let (path, key) = self.inner.database.clone().ok_or_else(|| {
                    "Learning needs the app database (shodh.db), which could not be opened."
                        .to_string()
                })?;
                let inbox = tokio::task::spawn_blocking(move || Inbox::open(&path, key.as_ref()))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                Ok(Arc::new(Learner::new(
                    service,
                    Arc::new(inbox),
                    Arc::new(SettingsPolicy(self.inner.data_dir.clone())),
                    self.inner.models.clone(),
                )))
            })
            .await
            .cloned()
    }

    fn policy(&self) -> LearnPolicy {
        SettingsPolicy(self.inner.data_dir.clone()).policy()
    }

    /// `agent_send` registers the user's words of a run. Only text the user typed in the
    /// main conversation is passed here (see the module docs).
    pub fn record_question(&self, conversation_id: &str, run_id: &str, user_text: &str) {
        self.touch();
        if self.policy().mode == LearnMode::Off || user_text.trim().is_empty() {
            return;
        }
        if self.inner.runs.len() >= MAX_TRACKED_RUNS {
            // Runs whose end was never seen (a crashed session): drop the oldest.
            let oldest = self
                .inner
                .runs
                .iter()
                .min_by_key(|r| r.at)
                .map(|r| r.key().clone());
            if let Some(oldest) = oldest {
                self.inner.runs.remove(&oldest);
            }
        }
        self.inner.runs.insert(
            run_id.to_string(),
            PendingRun {
                conversation_id: conversation_id.to_string(),
                user_text: user_text.trim().to_string(),
                answer: Vec::new(),
                answer_len: 0,
                at: Utc::now(),
            },
        );
    }

    /// Observes the agent event stream: answer text, and the end of a run.
    pub fn observe(&self, event: &AgentEvent) {
        match event {
            AgentEvent::TextDelta {
                run_id,
                message_id,
                delta,
            } => {
                if let Some(mut run) = self.inner.runs.get_mut(run_id) {
                    if run.answer_len + delta.len() <= MAX_ANSWER_CONTEXT {
                        run.answer_len += delta.len();
                        match run.answer.iter_mut().rev().find(|(id, _)| id == message_id) {
                            Some((_, text)) => text.push_str(delta),
                            None => run.answer.push((message_id.clone(), delta.clone())),
                        }
                    }
                }
            }
            AgentEvent::Grounding { run_id, report } => {
                if let Some(mut run) = self.inner.runs.get_mut(run_id) {
                    run.answer
                        .retain(|(id, _)| !report.superseded_message_ids.contains(id));
                }
            }
            AgentEvent::RunFinished { run_id, status, .. } => {
                self.touch();
                let Some((_, run)) = self.inner.runs.remove(run_id) else {
                    return;
                };
                if *status != RunStatus::Completed {
                    return;
                }
                self.enqueue(TurnInput {
                    conversation_id: run.conversation_id,
                    turn_id: run_id.clone(),
                    user_text: run.user_text,
                    assistant_context: Some(
                        run.answer
                            .into_iter()
                            .map(|(_, text)| text)
                            .collect::<Vec<_>>()
                            .join(
                                "

",
                            ),
                    )
                    .filter(|a| !a.trim().is_empty()),
                    at: run.at,
                });
            }
            _ => {}
        }
    }

    /// Forgets a registered run whose prompt failed.
    pub fn forget_run(&self, run_id: &str) {
        self.inner.runs.remove(run_id);
    }

    fn touch(&self) {
        self.inner
            .last_activity_ms
            .store(Utc::now().timestamp_millis(), Ordering::SeqCst);
    }

    fn enqueue(&self, turn: TurnInput) {
        let conversation = turn.conversation_id.clone();
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut batch = self.inner.batches.entry(conversation.clone()).or_default();
            batch.turns.push(turn);
            if batch.turns.len() > MAX_BATCH_TURNS {
                batch.turns.remove(0);
            }
            batch.generation = generation;
        }
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            let turns = {
                let Some(batch) = state.inner.batches.get(&conversation) else {
                    return;
                };
                if batch.generation != generation {
                    return; // A newer turn restarted the wait.
                }
                drop(batch);
                match state.inner.batches.remove(&conversation) {
                    Some((_, batch)) => batch.turns,
                    None => return,
                }
            };
            state.learn(turns).await;
        });
    }

    async fn learn(&self, turns: Vec<TurnInput>) {
        let learner = match self.learner().await {
            Ok(learner) => learner,
            Err(e) => {
                tracing::debug!(target: "shodh::memory", error = %e, "learning unavailable");
                return;
            }
        };
        match learner.learn_from_turns(&turns).await {
            Ok(report) => {
                if !report.proposed.is_empty() {
                    tracing::info!(
                        target: "shodh::memory",
                        proposed = report.proposed.len(),
                        learned = report.learned.len(),
                        dropped = report.dropped.values().sum::<usize>(),
                        "learned from a conversation"
                    );
                }
                self.notify(&learner);
            }
            Err(LearnError::Disabled) => {}
            Err(e) => {
                tracing::info!(target: "shodh::memory", error = %e, "learning from a conversation was skipped")
            }
        }
    }

    /// Tells the windows how many suggestions wait.
    fn notify(&self, learner: &Learner) {
        let pending = learner.inbox().pending_count().unwrap_or(0);
        if let Err(e) = self
            .inner
            .app
            .emit(SUGGESTIONS_CHANGED_EVENT, json!({ "pending": pending }))
        {
            tracing::debug!(target: "shodh::memory", error = %e, "emitting suggestions change failed");
        }
    }

    /// The background loop: consolidates at most daily, when idle.
    pub fn spawn_sleep(&self) {
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(SLEEP_CHECK).await;
                let idle_ms = Utc::now().timestamp_millis()
                    - state.inner.last_activity_ms.load(Ordering::SeqCst);
                let idle = idle_ms
                    >= i64::try_from(IDLE_BEFORE_SLEEP.as_millis()).unwrap_or(i64::MAX)
                    && state.inner.runs.is_empty()
                    && state.inner.batches.is_empty();
                if !idle || state.policy().mode == LearnMode::Off {
                    continue;
                }
                let Ok(learner) = state.learner().await else {
                    continue;
                };
                if !learner.consolidation_due().unwrap_or(false) {
                    continue;
                }
                match learner.consolidate(false).await {
                    Ok(report) => {
                        tracing::info!(target: "shodh::memory", proposed = report.proposed.len(), learned = report.learned.len(), "memory consolidation finished");
                        state.notify(&learner);
                    }
                    Err(e) => {
                        tracing::info!(target: "shodh::memory", error = %e, "memory consolidation skipped")
                    }
                }
            }
        });
    }

    /// Drops every queued turn (the kill switch).
    fn clear_queues(&self) {
        self.inner.runs.clear();
        self.inner.batches.clear();
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
    }
}

// ── Commands ─────────────────────────────────────────────────────

/// Learning status for Settings → Memory and the composer badge.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnStatus {
    /// The mode in the settings.
    pub mode: LearnMode,
    /// The environment kill switch is on (learning is off whatever the mode).
    pub kill_switch: bool,
    /// Whether a model is available for learning.
    pub available: bool,
    /// Why not, if not.
    pub unavailable_reason: Option<String>,
    /// The model learning uses.
    pub model: Option<String>,
    /// Suggestions waiting.
    pub pending: u32,
    /// Today's use.
    pub usage: UsageToday,
    /// The caps.
    pub caps: LearnCaps,
    /// The last consolidation.
    pub last_consolidation: Option<DateTime<Utc>>,
}

#[tauri::command]
pub async fn memory_learn_status(learn: State<'_, LearnState>) -> Result<LearnStatus, String> {
    let policy = learn.policy();
    let settings = SettingsStore::in_dir(&learn.inner.data_dir)
        .load()
        .map_err(|e| e.to_string())?;
    let model = learn.inner.models.model();
    let learner = learn.learner().await?;
    let now = learner.service().store().now();
    Ok(LearnStatus {
        mode: settings.memory.learn_mode,
        kill_switch: kill_switch(),
        available: model.is_ok(),
        unavailable_reason: model.as_ref().err().cloned(),
        model: model.ok().map(|m| m.model_id()),
        pending: learner.inbox().pending_count().map_err(|e| e.to_string())?,
        usage: learner.inbox().usage(now).map_err(|e| e.to_string())?,
        caps: policy.caps,
        last_consolidation: learner.last_consolidation().map_err(|e| e.to_string())?,
    })
}

fn parse_statuses(statuses: Option<Vec<String>>) -> Result<Vec<ProposalStatus>, String> {
    statuses
        .unwrap_or_default()
        .iter()
        .map(|s| ProposalStatus::parse(s).ok_or_else(|| format!("unknown status `{s}`")))
        .collect()
}

/// Suggestions with the given statuses (all when none), newest first.
#[tauri::command]
pub async fn memory_suggestions_list(
    statuses: Option<Vec<String>>,
    limit: Option<usize>,
    learn: State<'_, LearnState>,
) -> Result<Vec<ProposalView>, String> {
    let statuses = parse_statuses(statuses)?;
    let learner = learn.learner().await?;
    learner
        .list(&statuses, limit.unwrap_or(200))
        .map_err(|e| e.to_string())
}

/// Accept a suggestion (optionally edited). The user's own decision: the write is
/// audited as `memory_write` with this suggestion as its approval.
#[tauri::command]
pub async fn memory_suggestion_accept(
    id: String,
    edit: Option<MemoryContent>,
    learn: State<'_, LearnState>,
) -> Result<ProposalView, String> {
    let learner = learn.learner().await?;
    let view = learner
        .accept(&id, edit, &Actor::ui())
        .await
        .map_err(|e| e.to_string());
    learn.notify(&learner);
    let view = view?;
    spawn_evolve(&learn, learner, id);
    Ok(view)
}

fn spawn_evolve(learn: &LearnState, learner: Arc<Learner>, id: String) {
    let state = learn.clone();
    tauri::async_runtime::spawn(async move {
        match learner.evolve_accepted(&id).await {
            Ok(0) | Err(LearnError::Disabled) => {}
            Ok(_) => state.notify(&learner),
            Err(e) => tracing::debug!(target: "shodh::memory", error = %e, "evolution skipped"),
        }
    });
}

/// Result of one suggestion in a batch accept.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    pub id: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Accept several suggestions as they are.
#[tauri::command]
pub async fn memory_suggestions_accept_many(
    ids: Vec<String>,
    learn: State<'_, LearnState>,
) -> Result<Vec<BatchResult>, String> {
    if ids.len() > 200 {
        return Err("Accept at most 200 suggestions at once".to_string());
    }
    let learner = learn.learner().await?;
    let results = learner.accept_many(&ids, &Actor::ui()).await;
    learn.notify(&learner);
    Ok(results
        .into_iter()
        .map(|(id, result)| {
            if result.is_ok() {
                spawn_evolve(&learn, learner.clone(), id.clone());
            }
            BatchResult {
                ok: result.is_ok(),
                error: result.err().map(|e| e.to_string()),
                id,
            }
        })
        .collect())
}

/// Reject a suggestion (it is not made again for a while).
#[tauri::command]
pub async fn memory_suggestion_reject(
    id: String,
    learn: State<'_, LearnState>,
) -> Result<ProposalView, String> {
    let learner = learn.learner().await?;
    let view = learner.reject(&id).map_err(|e| e.to_string());
    learn.notify(&learner);
    view
}

/// Undo an accepted or learned suggestion. Audited as `memory_write`.
#[tauri::command]
pub async fn memory_suggestion_undo(
    id: String,
    learn: State<'_, LearnState>,
) -> Result<ProposalView, String> {
    let learner = learn.learner().await?;
    let view = learner
        .undo(&id, &Actor::ui())
        .await
        .map_err(|e| e.to_string());
    learn.notify(&learner);
    view
}

/// Stop learning now: mode off, queued turns dropped, waiting suggestions rejected.
#[tauri::command]
pub async fn memory_learning_stop(
    app: AppHandle,
    learn: State<'_, LearnState>,
    audit: State<'_, AuditState>,
) -> Result<usize, String> {
    let store = SettingsStore::in_dir(&learn.inner.data_dir);
    let (settings, before) = store
        .update(|s| Ok(std::mem::replace(&mut s.memory.learn_mode, LearnMode::Off)))
        .map_err(|e| e.to_string())?;
    learn.clear_queues();
    let learner = learn.learner().await?;
    let discarded = learner.discard_pending().map_err(|e| e.to_string())?;
    audit.record(AuditRecord::new(
        AuditEventType::SettingsChange,
        json!({
            "action": "memory_learning_stop",
            "old": before,
            "new": LearnMode::Off,
            "discarded": discarded,
            "via": "ui",
        }),
    ));
    broadcast(&app, &settings);
    learn.notify(&learner);
    Ok(discarded)
}

/// Consolidate now (normally at most daily, when idle).
#[tauri::command]
pub async fn memory_consolidate_now(
    learn: State<'_, LearnState>,
) -> Result<ConsolidationReport, String> {
    let learner = learn.learner().await?;
    let report = learner.consolidate(true).await.map_err(|e| e.to_string());
    learn.notify(&learner);
    report
}

/// Whether `agent_send` should hand a run's text to learning: only text the user typed
/// in a main conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextOrigin {
    /// Typed by the user in the composer.
    Typed,
    /// Composed by the app (a side-thread summary request, a retry of one) or unknown.
    #[default]
    Composed,
}

/// Registers the learning state with the app (call in `setup`).
pub fn manage(app: &tauri::App, data_dir: &Path) {
    let memory = app.state::<MemoryState>().inner().clone();
    let state = LearnState::new(
        app.handle().clone(),
        data_dir,
        memory,
        &app.state::<LLMState>(),
        &app.state::<AuditState>(),
    );
    state.spawn_sleep();
    app.manage(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external(provider: ApiProvider) -> LLMMode {
        LLMMode::External {
            provider,
            api_key: String::new(),
            model: "big-model".into(),
        }
    }

    #[test]
    fn the_learning_model_follows_the_provider_and_local_only_mode() {
        let id = |choice: Result<ModelChoice, String>| match choice {
            Ok(ModelChoice::External { model, local, .. }) => Ok((model, local)),
            Ok(ModelChoice::Local { id }) => Ok((id, true)),
            Err(e) => Err(e),
        };
        assert!(choose_model(&LLMMode::Disabled, None, false).is_err());
        assert_eq!(
            id(choose_model(
                &external(ApiProvider::Anthropic),
                Some(" claude-haiku-4-5 "),
                false
            )),
            Ok(("claude-haiku-4-5".to_string(), false))
        );
        assert_eq!(
            id(choose_model(
                &external(ApiProvider::OpenRouter),
                None,
                false
            )),
            Ok(("big-model".to_string(), false))
        );
        // Local-only: a cloud provider is refused, a local one is fine.
        assert!(choose_model(&external(ApiProvider::OpenAI), None, true)
            .unwrap_err()
            .contains("Local-only"));
        assert!(choose_model(&external(ApiProvider::Ollama), None, true).is_ok());
        let local = LLMMode::Local {
            model_path: PathBuf::from("C:/models/qwen3-4b.gguf"),
        };
        assert_eq!(
            id(choose_model(&local, Some("ignored"), true)),
            Ok(("qwen3-4b.gguf".to_string(), true))
        );
    }

    #[test]
    fn policy_comes_from_settings_and_unreadable_settings_mean_off() {
        assert_eq!(policy_from(None).mode, LearnMode::Off);
        let mut settings = AppSettings::default();
        assert_eq!(policy_from(Some(&settings)).mode, LearnMode::Ask);
        settings.memory.learn_mode = LearnMode::Auto;
        settings.memory.inject_memories = false;
        let policy = policy_from(Some(&settings));
        assert_eq!(policy.mode, LearnMode::Auto);
        assert!(!policy.share_memories_with_model);
    }

    #[test]
    fn statuses_parse_strictly() {
        assert_eq!(
            parse_statuses(Some(vec!["pending".into(), "learned".into()])).unwrap(),
            vec![ProposalStatus::Pending, ProposalStatus::Learned]
        );
        assert!(parse_statuses(Some(vec!["maybe".into()])).is_err());
        assert!(parse_statuses(None).unwrap().is_empty());
    }
}
