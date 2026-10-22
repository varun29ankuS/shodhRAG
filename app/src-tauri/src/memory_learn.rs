//! Learning memories from conversations (Settings → Memory → "Learn from conversations"):
//! the app side of `shodh_rag::user_memory::learn`.
//!
//! - **Capture.** `agent_send` hands over the user's own words of a turn: a message the
//!   frontend marks as typed by the user, or the question typed in a side thread (never
//!   the content the side-thread message is composed around, nor a composed summary
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

use std::collections::HashSet;
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
    ApiProvider, BoundedCompletion, GenerationConfig, LLMConfig, LLMManager, LLMMode,
    SimpleExternalProvider,
};
use shodh_rag::research::results::ModelReply;
use shodh_rag::research::tables::reasoning::{unbounded_message, ReasoningCatalog, ReasoningRule};
use shodh_rag::user_memory::learn::engine::ModelSource;
use shodh_rag::user_memory::learn::extract::learned_scope;
use shodh_rag::user_memory::learn::{
    ConsolidationReport, Inbox, LearnCaps, LearnError, LearnMode, LearnModel, LearnPolicy, Learner,
    MemoryRequest, PolicySource, ProposalStatus, ProposalView, RejectionCount, RequestedOutcome,
    TurnInput, UsageToday,
};
use shodh_rag::user_memory::{Actor, MemoryContent};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{OnceCell, RwLock as AsyncRwLock};

use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;
use crate::llm_commands::{ApiKeys, LLMState};
use crate::memory_commands::MemoryState;

/// Chat messages saved by the removed legacy memory system (relative to the data dir).
const LEGACY_MEMORIES: &str = "memory_store/memories.json";
/// Written once the legacy messages were learned from. The legacy file is never changed.
const LEGACY_IMPORTED: &str = "memory_store/learned-v1";
/// How many legacy messages (oldest first) were already learned from: each is sent to the
/// model at most once, across starts.
const LEGACY_PROGRESS: &str = "memory_store/learned-progress-v1";
/// How long after start the legacy messages are learned from.
const LEGACY_IMPORT_DELAY: Duration = Duration::from_secs(120);

/// The user's own messages among the legacy memories, oldest first, as turns of one
/// conversation. Assistant messages are skipped: only what the user wrote is a source of
/// memories (see [`TurnInput`]).
fn legacy_turns(json: &str) -> Result<Vec<TurnInput>, serde_json::Error> {
    #[derive(Deserialize)]
    struct LegacyMemory {
        id: String,
        experience: LegacyExperience,
        created_at: DateTime<Utc>,
    }
    #[derive(Deserialize)]
    struct LegacyExperience {
        content: String,
        #[serde(default)]
        metadata: std::collections::HashMap<String, String>,
    }
    let mut memories: Vec<LegacyMemory> = serde_json::from_str(json)?;
    memories.sort_by_key(|m| m.created_at);
    Ok(memories
        .into_iter()
        .filter(|m| m.experience.metadata.get("role").map(String::as_str) == Some("user"))
        .filter(|m| !m.experience.content.trim().is_empty())
        .map(|m| TurnInput {
            conversation_id: "legacy-memory-store".to_string(),
            turn_id: m.id,
            user_text: m.experience.content.trim().to_string(),
            assistant_context: None,
            at: m.created_at,
            scope: learned_scope(None),
        })
        .collect())
}

/// Legacy messages already learned from, as saved in `path` (0 when none or unreadable).
fn legacy_progress(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// Saves how many legacy messages were learned from.
fn save_legacy_progress(path: &Path, done: usize) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, done.to_string())?;
    std::fs::rename(&tmp, path)
}

/// What became of the user's request to remember something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestHandled {
    /// Saved as the user's memory (they can undo it from the chat).
    Saved,
    /// Filed as a suggestion waiting for the user (a sensitive note, or one whose save
    /// failed).
    Filed,
    /// The same note is already saved, or waiting for the user's approval.
    AlreadyKept,
}

/// Emitted whenever suggestions change, with `{ pending }`, and when they changed for
/// turns of one conversation (a request saved, a turn learned from) also with
/// `{ conversationId, turnIds, proposalIds }` so the open chat can show what was saved
/// under those turns. Ids only: never a memory's words.
pub const SUGGESTIONS_CHANGED_EVENT: &str = "memory-suggestions-changed";

/// Most turns the chat asks about at once ([`memory_suggestions_for_turns`]).
const MAX_CHAT_TURNS: usize = shodh_rag::user_memory::learn::inbox::MAX_TURNS_LISTED;

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
    /// What a call asks of a reasoning model and its output budget (evidence tables;
    /// plain for everything else).
    reasoning: ReasoningRule,
}

impl AppModel {
    fn generation(&self, max_tokens: usize) -> GenerationConfig {
        let config = self
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut generation = GenerationConfig::from(&config);
        generation.max_tokens = self.reasoning.max_tokens(max_tokens);
        generation.reasoning = self.reasoning.request().cloned();
        // Extraction is classification, not writing: no creativity wanted.
        generation.temperature = 0.0;
        generation
    }

    /// The model's reply to `prompt`, and whether the provider cut it off at the limit.
    async fn complete_bounded(
        &self,
        prompt: &str,
        max_output_tokens: usize,
    ) -> Result<BoundedCompletion, LearnError> {
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
                // The local runtime does not say where it stopped.
                manager
                    .generate_custom(prompt, max_output_tokens)
                    .await
                    .map(|text| BoundedCompletion {
                        text,
                        truncated: false,
                    })
                    .map_err(model_error)
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
                    .map_err(model_error)?;
                external_completion(&client, prompt, &self.generation(max_output_tokens)).await
            }
        }
    }
}

/// A failed model call as a learning error. `{:#}` keeps the whole provider failure: its
/// status, and its detail with the rate-limit headers in front, which a table fill reads
/// the reset time from.
fn model_error(e: anyhow::Error) -> LearnError {
    LearnError::Model(format!("{e:#}"))
}

/// One call to an API provider, its failure kept whole ([`model_error`]).
async fn external_completion(
    client: &SimpleExternalProvider,
    prompt: &str,
    generation: &GenerationConfig,
) -> Result<BoundedCompletion, LearnError> {
    client
        .generate_bounded(prompt, generation)
        .await
        .map_err(model_error)
}

/// A learning error as the text a one-shot extraction caller classifies.
fn text_model_error(e: LearnError) -> String {
    e.to_string()
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
        // Learning answers are strict JSON: an answer cut off at the limit is refused.
        let completion = self.complete_bounded(prompt, max_output_tokens).await?;
        if completion.truncated {
            return Err(LearnError::Truncated(completion.text));
        }
        Ok(completion.text)
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
        reasoning: ReasoningRule::Plain,
    }))
}

/// Why evidence tables cannot fill with the configured model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableModelError {
    /// The settings could not be read.
    Settings(String),
    /// No model is configured.
    NoModel,
    /// Local-only mode is on and the model is a cloud one.
    LocalOnly,
    /// The provider needs an API key and none is saved.
    NoKey,
    /// The model always reasons and its reasoning cannot be bounded (the model id).
    RefusedReasoning(String),
}

impl TableModelError {
    /// The code a paused job and a command error carry.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Settings(_) => "settings_unreadable",
            Self::NoModel => "no_model",
            Self::LocalOnly => "local_only",
            Self::NoKey => "no_key",
            Self::RefusedReasoning(_) => "refused_reasoning",
        }
    }

    /// What it means, in words.
    pub fn message(&self) -> String {
        match self {
            Self::Settings(e) => format!("The settings could not be read: {e}"),
            Self::NoModel => "No model is configured (Settings → Model).".to_string(),
            Self::LocalOnly => {
                "Local-only mode is on, so tables need a local model (Settings → Model)."
                    .to_string()
            }
            Self::NoKey => "The model's provider needs an API key (Settings → Model).".to_string(),
            Self::RefusedReasoning(model) => unbounded_message(model),
        }
    }
}

/// The model choice for tables and its reasoning rule, `rule` giving the OpenRouter
/// rule of a model id (other providers get the plain budget).
async fn table_choice<F, Fut>(
    mode: &LLMMode,
    local_only: bool,
    rule: F,
) -> Result<(ModelChoice, ReasoningRule), TableModelError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = ReasoningRule>,
{
    if matches!(mode, LLMMode::Disabled) {
        return Err(TableModelError::NoModel);
    }
    // With a model set, the only refusal left is Local-only mode against a cloud model.
    let choice = choose_model(mode, None, local_only).map_err(|_| TableModelError::LocalOnly)?;
    let reasoning = match &choice {
        ModelChoice::External {
            provider: ApiProvider::OpenRouter,
            model,
            ..
        } => rule(model.clone()).await,
        _ => ReasoningRule::Plain,
    };
    if !reasoning.usable() {
        let model = match &choice {
            ModelChoice::Local { id } => id.clone(),
            ModelChoice::External { model, .. } => model.clone(),
        };
        return Err(TableModelError::RefusedReasoning(model));
    }
    Ok((choice, reasoning))
}

/// The model evidence tables fill with: the one answers come from, as
/// [`configured_model`] resolves it, with OpenRouter's reasoning rule for it
/// ([`shodh_rag::research::tables::reasoning`]). Refused, with the reason, when no model
/// is set, Local-only mode refuses it, its provider has no API key, or its reasoning
/// cannot be bounded. Returns the model and its rule.
pub(crate) async fn configured_table_model(
    data_dir: &Path,
    llm: &LLMState,
    catalog: &ReasoningCatalog,
) -> Result<
    (
        Arc<dyn shodh_rag::research::results::TextModel>,
        ReasoningRule,
    ),
    TableModelError,
> {
    let settings = SettingsStore::in_dir(data_dir)
        .load()
        .map_err(|e| TableModelError::Settings(e.to_string()))?;
    let mode = llm
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode
        .clone();
    let (choice, reasoning) = table_choice(&mode, settings.policy.local_only, |model| async move {
        catalog.rule(&model).await
    })
    .await?;
    if let ModelChoice::External { provider, .. } = &choice {
        if !matches!(provider, ApiProvider::Ollama)
            && crate::agent_session_commands::resolve_key(llm, &mode)
                .await
                .is_none()
        {
            return Err(TableModelError::NoKey);
        }
    }
    let model: Arc<dyn shodh_rag::research::results::TextModel> = Arc::new(AppModel {
        data_dir: data_dir.to_path_buf(),
        manager: llm.manager.clone(),
        config: llm.config.clone(),
        api_keys: llm.api_keys.clone(),
        choice,
        reasoning: reasoning.clone(),
    });
    Ok((model, reasoning))
}

#[async_trait::async_trait]
impl shodh_rag::research::results::TextModel for AppModel {
    fn model_id(&self) -> String {
        LearnModel::model_id(self)
    }

    async fn complete(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
        // A reply cut off at the limit is returned as far as it was written: the callers
        // of a one-shot extraction read what is complete in it and check everything they
        // use. A caller that must know it was cut asks `complete_reply`.
        self.complete_reply(prompt, max_tokens)
            .await
            .map(|reply| reply.text)
    }

    async fn complete_reply(&self, prompt: &str, max_tokens: usize) -> Result<ModelReply, String> {
        self.complete_bounded(prompt, max_tokens)
            .await
            .map(|completion| ModelReply {
                text: completion.text,
                truncated: completion.truncated,
            })
            .map_err(text_model_error)
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
            reasoning: ReasoningRule::Plain,
        }))
    }
}

/// A run whose answer is streaming.
struct PendingRun {
    conversation_id: String,
    /// The conversation's workspace: what is learned from the turn goes there.
    workspace: Option<String>,
    user_text: String,
    /// Answer text by text block (a revised answer drops the draft it replaced).
    answer: Vec<(String, String)>,
    /// Text blocks that are the model's working notes before its tool calls, not answer.
    interim: HashSet<String>,
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
                // The inbox forgetting clears, when the memory service has one.
                let inbox = match service.inbox() {
                    Some(inbox) => inbox.clone(),
                    None => {
                        let (path, key) = self.inner.database.clone().ok_or_else(|| {
                            "Learning needs the app database (shodh.db), which could not be opened."
                                .to_string()
                        })?;
                        let inbox =
                            tokio::task::spawn_blocking(move || Inbox::open(&path, key.as_ref()))
                                .await
                                .map_err(|e| e.to_string())?
                                .map_err(|e| e.to_string())?;
                        Arc::new(inbox)
                    }
                };
                Ok(Arc::new(Learner::new(
                    service,
                    inbox,
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

    /// `agent_send` registers the user's words of a run. Only text the user typed is
    /// passed here (see the module docs).
    pub fn record_question(
        &self,
        conversation_id: &str,
        run_id: &str,
        user_text: &str,
        workspace: Option<&str>,
    ) {
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
                workspace: workspace.map(str::to_string),
                user_text: user_text.trim().to_string(),
                answer: Vec::new(),
                interim: HashSet::new(),
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
            AgentEvent::TextInterim {
                run_id,
                message_id,
                interim,
            } => {
                if let Some(mut run) = self.inner.runs.get_mut(run_id) {
                    if *interim {
                        run.interim.insert(message_id.clone());
                    } else {
                        run.interim.remove(message_id);
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
                            .filter(|(id, _)| !run.interim.contains(id))
                            .map(|(_, text)| text)
                            .collect::<Vec<_>>()
                            .join(
                                "

",
                            ),
                    )
                    .filter(|a| !a.trim().is_empty()),
                    at: run.at,
                    scope: learned_scope(run.workspace.as_deref()),
                });
            }
            _ => {}
        }
    }

    /// Saves what the user explicitly asked Shodh to remember in run `run_id` (`request`,
    /// matched on their typed words) as their note, before the answer starts and without
    /// a model; a sensitive note is filed for them to approve. Asking is not learning: the
    /// learning mode does not apply (Off included). The chat is told with the suggestion's
    /// id ([`SUGGESTIONS_CHANGED_EVENT`]) and shows "Remembered · Undo" under the message.
    /// Returns what became of it, so the answering model can be told; `None` when it could
    /// not be filed (the request is then left to the assistant).
    pub async fn file_memory_request(
        &self,
        request: &MemoryRequest,
        conversation_id: &str,
        run_id: &str,
        workspace: Option<&str>,
    ) -> Option<RequestHandled> {
        self.touch();
        let learner = match self.learner().await {
            Ok(learner) => learner,
            Err(e) => {
                tracing::info!(target: "shodh::memory", error = %e, "the request to remember was left to the assistant");
                return None;
            }
        };
        let at = learner.service().store().now();
        match learner
            .file_user_note(
                request,
                conversation_id,
                run_id,
                at,
                learned_scope(workspace),
            )
            .await
        {
            Ok(RequestedOutcome::Saved(view)) => {
                self.notify_turns(&learner, conversation_id, &[run_id], &[view.id.as_str()]);
                // As after an Accept in Settings: neighbouring memories may be linked.
                spawn_evolve(self, learner.clone(), view.id.clone());
                Some(RequestHandled::Saved)
            }
            Ok(RequestedOutcome::Filed(proposal)) => {
                self.notify_turns(
                    &learner,
                    conversation_id,
                    &[run_id],
                    &[proposal.id.as_str()],
                );
                Some(RequestHandled::Filed)
            }
            Ok(RequestedOutcome::AlreadyKept) => Some(RequestHandled::AlreadyKept),
            Err(e) => {
                tracing::warn!(target: "shodh::memory", error = %e, "the request to remember was not filed; left to the assistant");
                None
            }
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
        let Some(conversation_id) = turns.first().map(|t| t.conversation_id.clone()) else {
            return;
        };
        let turn_ids: Vec<&str> = turns.iter().map(|t| t.turn_id.as_str()).collect();
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
                let proposed: Vec<&str> = report.proposed.iter().map(String::as_str).collect();
                self.notify_turns(&learner, &conversation_id, &turn_ids, &proposed);
                crate::notifications::memory_suggestions(
                    &self.inner.app,
                    report.proposed.len().saturating_sub(report.learned.len()),
                );
            }
            Err(LearnError::Disabled) => {}
            Err(e) => {
                // The model call itself failed (not a limit or a missing model).
                if matches!(e, LearnError::Model(_)) {
                    crate::notifications::learning_failed(&self.inner.app);
                }
                tracing::info!(target: "shodh::memory", error = %e, "learning from a conversation was skipped")
            }
        }
    }

    /// A memory was forgotten or edited: tells the windows (the chat re-reads its lines).
    fn memory_changed(&self) {
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            match state.learner().await {
                Ok(learner) => state.notify(&learner),
                Err(e) => {
                    tracing::debug!(target: "shodh::memory", error = %e, "suggestions unavailable; no change told")
                }
            }
        });
    }

    /// Tells the windows how many suggestions wait.
    fn notify(&self, learner: &Learner) {
        let pending = learner.inbox().pending_count().unwrap_or(0);
        self.emit_change(json!({ "pending": pending }));
    }

    /// [`Self::notify`] for suggestions `proposal_ids` filed for turns `turn_ids` of
    /// `conversation_id`, so the open chat shows them under those turns.
    fn notify_turns(
        &self,
        learner: &Learner,
        conversation_id: &str,
        turn_ids: &[&str],
        proposal_ids: &[&str],
    ) {
        let pending = learner.inbox().pending_count().unwrap_or(0);
        self.emit_change(json!({
            "pending": pending,
            "conversationId": conversation_id,
            "turnIds": turn_ids,
            "proposalIds": proposal_ids,
        }));
    }

    fn emit_change(&self, payload: serde_json::Value) {
        if let Err(e) = self.inner.app.emit(SUGGESTIONS_CHANGED_EVENT, payload) {
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
                        crate::notifications::memory_suggestions(
                            &state.inner.app,
                            report.proposed.len().saturating_sub(report.learned.len()),
                        );
                    }
                    Err(e) => {
                        if matches!(e, LearnError::Model(_)) {
                            crate::notifications::learning_failed(&state.inner.app);
                        }
                        tracing::info!(target: "shodh::memory", error = %e, "memory consolidation skipped")
                    }
                }
            }
        });
    }

    /// Learns once from what the user wrote in chats the removed legacy memory system
    /// saved, like from a conversation: facts become suggestions (applied under automatic
    /// learning, with the usual sensitivity rules). Skipped while learning is off; it is
    /// tried again at the next start until it completes.
    pub async fn import_legacy_memories(&self) {
        let path = self.inner.data_dir.join(LEGACY_MEMORIES);
        let marker = self.inner.data_dir.join(LEGACY_IMPORTED);
        if marker.exists() || !path.exists() || self.policy().mode == LearnMode::Off {
            return;
        }
        let turns = match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|json| legacy_turns(&json).map_err(|e| e.to_string()))
        {
            Ok(turns) => turns,
            Err(e) => {
                tracing::warn!(target: "shodh::memory", error = %e, "legacy memories unreadable; not imported");
                return;
            }
        };
        let learner = match self.learner().await {
            Ok(learner) => learner,
            Err(e) => {
                tracing::debug!(target: "shodh::memory", error = %e, "learning unavailable; legacy memories not imported yet");
                return;
            }
        };
        // The legacy messages are chat text, not typed facts: the model reads them, under the
        // import's own one-time allowance (never the day's calls), each message at most once.
        let progress = self.inner.data_dir.join(LEGACY_PROGRESS);
        let mut done = legacy_progress(&progress).min(turns.len());
        let mut proposed = 0;
        for batch in turns[done..].chunks(MAX_BATCH_TURNS) {
            let suggestions_full = match learner.learn_from_legacy(batch).await {
                Ok(report) => {
                    proposed += report.proposed.len();
                    report
                        .skipped
                        .as_deref()
                        .is_some_and(|s| s.starts_with("today's limit"))
                }
                Err(LearnError::BudgetExhausted(what)) => {
                    tracing::info!(target: "shodh::memory", limit = %what, remaining = turns.len() - done, "legacy memories: the import's allowance is spent; the rest is not learned from");
                    break;
                }
                Err(e) => {
                    tracing::info!(target: "shodh::memory", error = %e, "legacy memories not imported yet");
                    return;
                }
            };
            done += batch.len();
            if let Err(e) = save_legacy_progress(&progress, done) {
                tracing::warn!(target: "shodh::memory", error = %e, "legacy import progress not saved; stopped");
                return;
            }
            if suggestions_full {
                tracing::info!(target: "shodh::memory", "legacy memories: today's suggestion limit reached; continued at a later start");
                self.notify(&learner);
                return;
            }
        }
        self.notify(&learner);
        match std::fs::write(&marker, Utc::now().to_rfc3339()) {
            Ok(()) => {
                tracing::info!(target: "shodh::memory", messages = turns.len(), proposed, "legacy memories learned from")
            }
            Err(e) => {
                tracing::warn!(target: "shodh::memory", error = %e, "legacy import could not be marked done")
            }
        }
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
    /// Why answers or memories were not used today, by reason, most frequent first.
    pub rejections: Vec<RejectionCount>,
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
        rejections: learner
            .inbox()
            .rejections_today(now)
            .map_err(|e| e.to_string())?,
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
    let views = learner
        .list(&statuses, limit.unwrap_or(200))
        .map_err(|e| e.to_string())?;
    Ok(learner.with_current_undo(views).await)
}

/// The "remember" suggestions of turns `turn_ids` (run ids) of conversation
/// `conversation_id`, oldest first: what the chat shows under each turn ("Remembered",
/// "Learned", "Shodh suggests remembering"). Read live, so a memory undone or forgotten
/// since shows as such and its words are never kept in the conversation.
#[tauri::command]
pub async fn memory_suggestions_for_turns(
    conversation_id: String,
    turn_ids: Vec<String>,
    learn: State<'_, LearnState>,
) -> Result<Vec<ProposalView>, String> {
    if turn_ids.len() > MAX_CHAT_TURNS {
        return Err(format!("Ask about at most {MAX_CHAT_TURNS} turns at once"));
    }
    let learner = learn.learner().await?;
    let proposals = learner
        .inbox()
        .list_for_turns(&conversation_id, &turn_ids)
        .map_err(|e| e.to_string())?;
    // A memory edited since cannot be undone from its line.
    Ok(learner
        .with_current_undo(proposals.into_iter().map(ProposalView::from).collect())
        .await)
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
        memory.clone(),
        &app.state::<LLMState>(),
        &app.state::<AuditState>(),
    );
    state.spawn_sleep();
    // A forget or an edit (Settings or the assistant) changes what the chat shows under
    // the turns the memory came from. Held weakly: the memory state holds the learning state's
    // hook, and the learning state holds the memory state.
    let weak = Arc::downgrade(&state.inner);
    memory.on_change(move || {
        if let Some(inner) = weak.upgrade() {
            LearnState { inner }.memory_changed();
        }
    });
    let legacy = state.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(LEGACY_IMPORT_DELAY).await;
        legacy.import_legacy_memories().await;
    });
    app.manage(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_memories_give_the_users_own_messages_oldest_first() {
        let json = r#"[
            {"id": "b", "experience": {"experience_type": "Conversation", "content": "Sure, here is a summary.",
              "metadata": {"role": "assistant"}}, "importance": 0.5, "created_at": "2026-03-01T10:00:00Z"},
            {"id": "c", "experience": {"experience_type": "Conversation", "content": " I moved to Pune last month. ",
              "metadata": {"role": "user", "platform": "Desktop"}}, "created_at": "2026-03-02T10:00:00Z"},
            {"id": "a", "experience": {"experience_type": "Conversation", "content": "My manager is Asha.",
              "metadata": {"role": "user"}}, "created_at": "2026-02-01T10:00:00Z"},
            {"id": "d", "experience": {"experience_type": "Conversation", "content": "  ",
              "metadata": {"role": "user"}}, "created_at": "2026-03-03T10:00:00Z"}
        ]"#;
        let turns = legacy_turns(json).unwrap();
        let texts: Vec<(&str, &str)> = turns
            .iter()
            .map(|t| (t.turn_id.as_str(), t.user_text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                ("a", "My manager is Asha."),
                ("c", "I moved to Pune last month.")
            ]
        );
        assert!(turns.iter().all(|t| t.assistant_context.is_none()));
        assert!(turns.iter().all(|t| t.scope == learned_scope(None)));
        assert!(legacy_turns("{}").is_err());
    }

    #[test]
    fn legacy_import_progress_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learned-progress-v1");
        assert_eq!(legacy_progress(&path), 0, "nothing learned yet");
        save_legacy_progress(&path, 8).unwrap();
        assert_eq!(legacy_progress(&path), 8);
        save_legacy_progress(&path, 12).unwrap();
        assert_eq!(legacy_progress(&path), 12);
        std::fs::write(&path, "not a number").unwrap();
        assert_eq!(legacy_progress(&path), 0);
    }

    #[tokio::test]
    async fn tables_take_the_reasoning_rule_only_for_openrouter_and_refuse_what_they_cannot_use() {
        let bounded = ReasoningRule::Bounded {
            request: Some(json!({ "effort": "low" })),
            allowance: 4096,
        };
        let rule = |r: ReasoningRule| move |_model: String| std::future::ready(r);
        let openrouter = external(ApiProvider::OpenRouter);
        assert_eq!(
            table_choice(&openrouter, false, rule(ReasoningRule::Unbounded))
                .await
                .unwrap_err(),
            TableModelError::RefusedReasoning("big-model".to_string())
        );
        assert_eq!(
            table_choice(&openrouter, false, rule(bounded.clone()))
                .await
                .unwrap()
                .1,
            bounded
        );
        // Another provider never gets OpenRouter's rule.
        let (_, plain) = table_choice(
            &external(ApiProvider::OpenAI),
            false,
            rule(ReasoningRule::Unbounded),
        )
        .await
        .unwrap();
        assert_eq!(plain, ReasoningRule::Plain);
        let refused = table_choice(&openrouter, true, rule(bounded.clone()))
            .await
            .unwrap_err();
        assert_eq!(refused, TableModelError::LocalOnly);
        assert_eq!(refused.code(), "local_only");
        let unset = table_choice(&LLMMode::Disabled, false, rule(bounded.clone()))
            .await
            .unwrap_err();
        assert_eq!(unset.code(), "no_model");
        let (_, local) = table_choice(&external(ApiProvider::Ollama), true, rule(bounded))
            .await
            .unwrap();
        assert_eq!(local, ReasoningRule::Plain);
        assert!(TableModelError::RefusedReasoning("m".to_string())
            .message()
            .contains("Choose another model"));
        assert_eq!(TableModelError::NoKey.code(), "no_key");
    }

    #[test]
    fn a_bounded_rule_raises_the_reply_budget_and_asks_for_low_reasoning() {
        let model = |reasoning: ReasoningRule| AppModel {
            data_dir: PathBuf::new(),
            manager: Arc::new(AsyncRwLock::new(None)),
            config: Arc::new(std::sync::Mutex::new(LLMConfig::default())),
            api_keys: Arc::new(std::sync::Mutex::new(ApiKeys::default())),
            choice: ModelChoice::External {
                provider: ApiProvider::OpenRouter,
                model: "m".to_string(),
                local: false,
            },
            reasoning,
        };
        let bounded = model(ReasoningRule::Bounded {
            request: Some(json!({ "effort": "low" })),
            allowance: 4096,
        })
        .generation(840);
        assert_eq!(bounded.max_tokens, 840 + 4096);
        assert_eq!(bounded.reasoning, Some(json!({ "effort": "low" })));
        assert_eq!(bounded.temperature, 0.0);
        let plain = model(ReasoningRule::Plain).generation(840);
        assert_eq!(plain.max_tokens, 840);
        assert_eq!(plain.reasoning, None);
        let off = model(ReasoningRule::Off {
            request: json!({ "enabled": false }),
        })
        .generation(840);
        assert_eq!(off.max_tokens, 840);
        assert_eq!(off.reasoning, Some(json!({ "enabled": false })));
    }

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
        assert_eq!(policy_from(Some(&settings)).mode, LearnMode::Auto);
        settings.memory.learn_mode = LearnMode::Ask;
        settings.memory.inject_memories = false;
        let policy = policy_from(Some(&settings));
        assert_eq!(policy.mode, LearnMode::Ask);
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

    /// Answers the first request on a local port with `status` and the JSON `body`, the
    /// way a provider answers. Returns the chat completions endpoint.
    async fn serve_once(status: &'static str, body: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let n = socket.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
        });
        format!("http://{addr}/v1/chat/completions")
    }

    fn client_for(endpoint: String) -> SimpleExternalProvider {
        SimpleExternalProvider::new(
            ApiProvider::Custom { endpoint },
            "sk-or-v1-00000000000000000000000000000000".to_string(),
            "test/model:free".to_string(),
        )
        .unwrap()
    }

    /// OpenRouter's 429 for the exhausted daily free quota goes through the real HTTP
    /// path and the learning model's error mapping, and the evidence table fill still
    /// reads its reset time and its window from it. The body is long, with the headers
    /// last, as OpenRouter sends them after an upstream's raw text.
    #[tokio::test]
    async fn an_openrouter_daily_limit_reaches_the_table_fill_with_its_reset_time() {
        use shodh_rag::research::tables::fill::{LimitWindow, ModelFailure, ModelFailureKind};
        let reset_ms: u64 = 4_102_444_800_000; // 2100-01-01T00:00:00Z
        let body = format!(
            r#"{{"error":{{"message":"Rate limit exceeded: free-models-per-day. Add 10 credits to unlock 1000 free model requests per day","code":429,"metadata":{{"raw":"{}","provider_name":null,"headers":{{"X-RateLimit-Limit":"50","X-RateLimit-Remaining":"0","X-RateLimit-Reset":"{reset_ms}"}}}}}},"user_id":"user_2xYz"}}"#,
            "upstream says: slow down. ".repeat(40)
        );
        let client = client_for(serve_once("429 Too Many Requests", body).await);
        let generation = GenerationConfig::from(&LLMConfig::default());
        let error = external_completion(&client, "prompt", &generation)
            .await
            .unwrap_err();
        let failure = ModelFailure::classify(&text_model_error(error));
        assert_eq!(failure.kind, ModelFailureKind::Quota);
        assert_eq!(failure.window, LimitWindow::Daily);
        assert_eq!(failure.reset_at_ms, Some(reset_ms));
        assert!(!failure.detail.contains("sk-or-v1"));
    }

    /// A reply the provider stopped at the limit keeps that mark through the adapter.
    #[tokio::test]
    async fn a_reply_cut_at_the_limit_is_reported_as_cut() {
        let body = r#"{"choices":[{"message":{"content":"{\"cells\":[{\"column\":\"c1\""},"finish_reason":"length"}]}"#.to_string();
        let client = client_for(serve_once("200 OK", body).await);
        let generation = GenerationConfig::from(&LLMConfig::default());
        let completion = external_completion(&client, "prompt", &generation)
            .await
            .unwrap();
        assert!(completion.truncated);
        assert!(completion.text.starts_with("{\"cells\""));
    }
}
