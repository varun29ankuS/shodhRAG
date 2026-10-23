//! Long-term memory: managed state, the Settings → Memory commands, the recall injected
//! at the start of an agent run and, at its end, the reinforcement of only the memories
//! the answer used.
//!
//! Memories are typed statements in the LanceDB table `statements` (next to the document
//! index in `<app_data_dir>/lance_data`) with their dynamics in `shodh.db`. The store
//! opens on first use: it needs the search models (the embedder), which may be installed
//! after the app starts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::Serialize;
use shodh_rag::audit::{AuditKey, AuditLog};
use shodh_rag::embeddings::{EmbeddingModel, SearchModelsMissing};
use shodh_rag::harness::{AgentEvent, RunStatus};
use shodh_rag::statements::{
    DynamicsStore, EmbedderSource, Scope, StatementError, StatementResult, StatementStore,
    SystemClock,
};
use shodh_rag::user_memory::learn::Inbox;
use shodh_rag::user_memory::{
    render_core_counted, render_injection_counted, Actor, Feedback, ListRequest, MemoryContent,
    MemoryError, MemoryRecord, MemoryService, Origin, RecallMode, RecallRequest, RecalledMemory,
    RememberOutcome, CORE_MAX_CHARS, MAX_INJECTION_CHARS,
};
use shodh_rag::RAGEngine;
use tauri::State;
use tokio::sync::{OnceCell, RwLock as AsyncRwLock};

use crate::audit_commands::AuditState;

/// Memories recalled into one agent run.
pub const INJECTED_MEMORIES: usize = 6;

/// Runs with memories whose end has not been seen yet, kept at most (a crashed session
/// never sends its end): the oldest is dropped first.
const MAX_TRACKED_RUNS: usize = 64;

/// Most characters of an answer kept to judge which memories it used. The judgement
/// reads the first [`shodh_rag::user_memory::usage::MAX_ANSWER_SENTENCES`] sentences, which
/// fit in this.
const MAX_TRACKED_ANSWER_CHARS: usize = 24_000;

/// Most memories one lookup or feedback names (an answer has at most
/// [`shodh_rag::user_memory::CORE_MAX_ITEMS`] core memories and [`INJECTED_MEMORIES`]
/// recalled ones, 26).
const MAX_IDS_PER_CALL: usize = 32;

/// Most characters of everything put in front of one question: the core block
/// ([`CORE_MAX_CHARS`]) and the recalled memories' block ([`MAX_INJECTION_CHARS`]),
/// separator included (about 875 tokens).
pub const MAX_RUN_MEMORY_CHARS: usize = CORE_MAX_CHARS + MAX_INJECTION_CHARS;

/// Between the core block and the recalled memories' block.
const BLOCK_SEPARATOR: &str = "\n";

/// The app version recorded on memories the user states.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The ontology memories are typed against: the core plus the built-in research pack.
pub fn memory_ontology() -> Result<shodh_ontology::Ontology, String> {
    shodh_ontology::Ontology::builtin_with_packs(&["research"]).map_err(|e| e.to_string())
}

/// The engine's embedding model, looked up per call (models can be attached later).
struct EngineEmbedder(Arc<AsyncRwLock<RAGEngine>>);

#[async_trait::async_trait]
impl EmbedderSource for EngineEmbedder {
    async fn embedder(&self) -> StatementResult<Arc<dyn EmbeddingModel>> {
        self.0
            .read()
            .await
            .shared_embeddings()
            .ok_or_else(|| StatementError::EmbeddingUnavailable(SearchModelsMissing.to_string()))
    }
}

struct Deps {
    lance_dir: PathBuf,
    rag: Arc<AsyncRwLock<RAGEngine>>,
    audit: Option<Arc<AuditLog>>,
    database: Option<(PathBuf, Option<AuditKey>)>,
}

/// Managed state: the memory service, opened on first use. Clones share it (the agent
/// tools hold one).
#[derive(Clone)]
pub struct MemoryState {
    inner: Arc<Inner>,
}

struct Inner {
    service: OnceCell<Arc<MemoryService>>,
    deps: Option<Deps>,
    /// Told after a memory is forgotten or edited ([`MemoryState::changed`]).
    on_change: std::sync::OnceLock<ChangeHook>,
    /// Runs memories were put in front of, by run id, until they end
    /// ([`MemoryState::observe`]).
    runs: DashMap<String, TrackedRun>,
}

/// A run memories were injected into, and its answer so far.
struct TrackedRun {
    /// The injected memories, in injection order.
    ids: Vec<String>,
    /// The scope they were recalled from.
    scope: Scope,
    /// Who the use is audited as (the run's conversation, profile and run).
    actor: Actor,
    /// Earlier tries of the same question (Retry): what they reinforced is not reinforced
    /// again.
    prior_run_ids: Vec<String>,
    answer: RunAnswer,
    at: Instant,
}

/// The answer text of a run from its events: text blocks in order, without the working
/// notes that led into tool calls (`TextInterim`) and without drafts a revision replaced
/// (`Grounding`), at most [`MAX_TRACKED_ANSWER_CHARS`].
#[derive(Default)]
struct RunAnswer {
    blocks: Vec<(String, String)>,
    interim: HashSet<String>,
    chars: usize,
}

impl RunAnswer {
    fn push(&mut self, message_id: &str, delta: &str) {
        let room = MAX_TRACKED_ANSWER_CHARS.saturating_sub(self.chars);
        if room == 0 {
            return;
        }
        let kept: String = delta.chars().take(room).collect();
        self.chars += kept.chars().count();
        match self
            .blocks
            .iter_mut()
            .rev()
            .find(|(id, _)| id == message_id)
        {
            Some((_, text)) => text.push_str(&kept),
            None => self.blocks.push((message_id.to_string(), kept)),
        }
    }

    fn set_interim(&mut self, message_id: &str, interim: bool) {
        if interim {
            self.interim.insert(message_id.to_string());
        } else {
            self.interim.remove(message_id);
        }
    }

    fn drop_superseded(&mut self, superseded: &[String]) {
        self.blocks.retain(|(id, _)| !superseded.contains(id));
    }

    fn text(&self) -> String {
        self.blocks
            .iter()
            .filter(|(id, _)| !self.interim.contains(id))
            .map(|(_, text)| text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// What [`MemoryState::on_change`] runs.
type ChangeHook = Arc<dyn Fn() + Send + Sync>;

impl MemoryState {
    /// State that opens the store under `app_data_dir` on first use.
    pub fn new(app_data_dir: &Path, rag: Arc<AsyncRwLock<RAGEngine>>, audit: &AuditState) -> Self {
        Self {
            inner: Arc::new(Inner {
                service: OnceCell::new(),
                deps: Some(Deps {
                    lance_dir: app_data_dir.join("lance_data"),
                    rag,
                    audit: audit.log(),
                    database: audit.database(),
                }),
                on_change: std::sync::OnceLock::new(),
                runs: DashMap::new(),
            }),
        }
    }

    /// State over an already open service (tests).
    #[cfg(test)]
    pub fn ready(service: Arc<MemoryService>) -> Self {
        Self {
            inner: Arc::new(Inner {
                service: OnceCell::new_with(Some(service)),
                deps: None,
                on_change: std::sync::OnceLock::new(),
                runs: DashMap::new(),
            }),
        }
    }

    /// Runs `hook` after every forget or edit of a memory, from Settings or the assistant
    /// (set once, at start): the open chats re-read their memory lines, so a forgotten
    /// memory's words do not stay on screen (#72). Later calls are ignored.
    pub fn on_change(&self, hook: impl Fn() + Send + Sync + 'static) {
        if self.inner.on_change.set(Arc::new(hook)).is_err() {
            tracing::debug!(target: "shodh::memory", "the memory change hook is set already");
        }
    }

    /// A memory was forgotten or edited.
    pub fn changed(&self) {
        if let Some(hook) = self.inner.on_change.get() {
            hook();
        }
    }

    /// `agent_send` put memories `ids` (recalled from `workspace`'s scope) in front of run
    /// `run_id`, a try of a question whose earlier tries were `prior_run_ids`: at its end,
    /// the ones the answer used are reinforced, unless an earlier try reinforced them
    /// ([`MemoryState::observe`]). Registered before the prompt is sent, so no event of the
    /// run is missed.
    pub fn track_run(
        &self,
        run_id: &str,
        ids: Vec<String>,
        workspace: Option<&str>,
        actor: Actor,
        prior_run_ids: Vec<String>,
    ) {
        if ids.is_empty() {
            return;
        }
        if self.inner.runs.len() >= MAX_TRACKED_RUNS {
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
            TrackedRun {
                ids,
                scope: Scope::for_workspace(workspace),
                actor,
                prior_run_ids,
                answer: RunAnswer::default(),
                at: Instant::now(),
            },
        );
    }

    /// Records that memories `ids` were given to run `run_id`, so the user's feedback on
    /// that answer is accepted for them ([`MemoryService::record_injected`]), and which of
    /// them (`core`) were core memories, so its list says "always included"
    /// ([`MemoryService::record_core_injected`]). A failure is logged; the answer goes on
    /// (feedback on it is then refused).
    pub async fn record_injected(&self, run_id: &str, ids: &[String], core: &[String]) {
        let recorded = match self.service().await {
            Ok(service) => match service.record_injected(run_id, ids).await {
                Ok(()) if core.is_empty() => Ok(()),
                Ok(()) => service
                    .record_core_injected(run_id, core)
                    .await
                    .map_err(error_text),
                Err(e) => Err(error_text(e)),
            },
            Err(e) => Err(e),
        };
        if let Err(e) = recorded {
            tracing::warn!(target: "shodh::memory", error = %e, "the memories given to an answer were not recorded");
        }
    }

    /// A tracked run whose prompt failed: nothing to reinforce.
    pub fn forget_run(&self, run_id: &str) {
        self.inner.runs.remove(run_id);
    }

    /// Follows the agent event stream for the tracked runs: their answer text, and their
    /// end. When a run completes, the memories its answer used are reinforced in the
    /// background ([`MemoryService::record_answer_use`]), off the event stream; a run that
    /// was interrupted or failed reinforces nothing. Returns the background task, if one
    /// was started.
    pub fn observe(&self, event: &AgentEvent) -> Option<tokio::task::JoinHandle<()>> {
        match event {
            AgentEvent::TextDelta {
                run_id,
                message_id,
                delta,
            } => {
                if let Some(mut run) = self.inner.runs.get_mut(run_id) {
                    run.answer.push(message_id, delta);
                }
                None
            }
            AgentEvent::TextInterim {
                run_id,
                message_id,
                interim,
            } => {
                if let Some(mut run) = self.inner.runs.get_mut(run_id) {
                    run.answer.set_interim(message_id, *interim);
                }
                None
            }
            AgentEvent::Grounding { run_id, report } => {
                if let Some(mut run) = self.inner.runs.get_mut(run_id) {
                    run.answer.drop_superseded(&report.superseded_message_ids);
                }
                None
            }
            AgentEvent::RunFinished { run_id, status, .. } => {
                let (_, run) = self.inner.runs.remove(run_id)?;
                if *status != RunStatus::Completed {
                    tracing::debug!(target: "shodh::memory", run = %run_id, "the answer did not complete; no memory reinforced");
                    return None;
                }
                let state = self.clone();
                let run_id = run_id.clone();
                Some(tokio::spawn(async move {
                    state.reinforce_used(&run_id, run).await;
                }))
            }
            _ => None,
        }
    }

    /// Reinforces the memories of a completed run that its answer used.
    async fn reinforce_used(&self, run_id: &str, run: TrackedRun) {
        let service = match self.service().await {
            Ok(service) => service,
            Err(e) => {
                tracing::debug!(target: "shodh::memory", error = %e, "memory unavailable at the end of the answer; nothing reinforced");
                return;
            }
        };
        match service
            .record_answer_use(
                run_id,
                &run.prior_run_ids,
                &run.ids,
                &run.answer.text(),
                &run.scope,
                &run.actor,
            )
            .await
        {
            Ok(used) => {
                tracing::debug!(target: "shodh::memory", injected = run.ids.len(), used = used.len(), "memories the answer used were reinforced")
            }
            Err(e) => {
                tracing::warn!(target: "shodh::memory", error = %e, "recording the memories an answer used failed")
            }
        }
    }

    /// The memory service, opening it if needed. Fails (and retries on the next call)
    /// while the audit database is unavailable or the store cannot open.
    pub async fn service(&self) -> Result<Arc<MemoryService>, String> {
        self.inner
            .service
            .get_or_try_init(|| async {
                let deps = self
                    .inner
                    .deps
                    .as_ref()
                    .ok_or_else(|| "Memory is not available in this build.".to_string())?;
                let (db_path, key) = deps.database.clone().ok_or_else(|| {
                    "Memory needs the app database (shodh.db), which could not be opened; see the audit page."
                        .to_string()
                })?;
                // The inbox is opened with the memory, not only with learning: forgetting
                // clears a memory's words from its suggestions even while learning is off.
                let (dynamics, inbox) = tokio::task::spawn_blocking(move || {
                    let dynamics =
                        DynamicsStore::open(&db_path, key.as_ref()).map_err(|e| e.to_string())?;
                    let inbox = Inbox::open(&db_path, key.as_ref()).map_err(|e| e.to_string())?;
                    Ok::<_, String>((dynamics, inbox))
                })
                .await
                .map_err(|e| format!("Opening memory failed: {e}"))?
                .map_err(|e| format!("Opening memory failed: {e}"))?;
                let dimension = deps.rag.read().await.config().embedding.dimension;
                let ontology = Arc::new(memory_ontology()?);
                let store = StatementStore::open(
                    &deps.lance_dir,
                    dimension,
                    ontology,
                    Arc::new(dynamics),
                    Arc::new(EngineEmbedder(deps.rag.clone())),
                    Arc::new(SystemClock),
                )
                .await
                .map_err(|e| format!("Opening memory failed: {e}"))?;
                let service = Arc::new(
                    MemoryService::new(Arc::new(store), deps.audit.clone(), APP_VERSION)
                        .with_inbox(Arc::new(inbox)),
                );
                // Finishes what earlier runs left (erased rows still in the table's history,
                // memories forgotten before forgetting erased everything), in the background.
                service.start_maintenance();
                Ok(service)
            })
            .await
            .cloned()
    }
}

/// Text of a memory error for the UI.
pub fn error_text(error: MemoryError) -> String {
    error.to_string()
}

/// Longest the memory recall may delay the start of an answer (it waits for the engine
/// while indexing holds it). On timeout the answer starts without memories.
pub const RECALL_TIMEOUT: Duration = Duration::from_millis(1_500);

/// The memories put in front of one run's question.
#[derive(Debug, Clone, PartialEq)]
pub struct RunMemories {
    /// The delimited blocks for the message ([`with_memories`]): the core block first,
    /// then the recalled memories' block, at most [`MAX_RUN_MEMORY_CHARS`].
    pub block: String,
    /// The memories in them, in order: core ones first (what the chat shows and the end of
    /// the run judges).
    pub ids: Vec<String>,
    /// Those of `ids` that are core memories (in front of every question of the scope).
    pub core_ids: Vec<String>,
}

/// The memories for the start of an agent run: the scope's core memories (standing
/// context, selected by class: [`MemoryService::core_for_run`]) and those relevant to
/// `text` (recall), read-only within one [`RECALL_TIMEOUT`]. A memory in the core block is
/// not repeated among the recalled ones. Nothing is reinforced here: being put in front of
/// a question is not use, for core memories as for the others. Only the memories the answer
/// used are reinforced, when the run completes ([`MemoryState::track_run`],
/// [`MemoryState::observe`]). `None` when there is nothing to inject; the core and the
/// recalled memories fail open on their own (no search models, an error or the shared
/// deadline passing leaves the other in place), and the answer goes on.
pub async fn recall_for_run(
    memory: &MemoryState,
    text: &str,
    workspace: Option<&str>,
    actor: &Actor,
) -> Option<RunMemories> {
    let scope = Scope::for_workspace(workspace);
    let deadline = tokio::time::Instant::now() + RECALL_TIMEOUT;
    let service = match tokio::time::timeout_at(deadline, memory.service()).await {
        Ok(Ok(service)) => service,
        Ok(Err(e)) => {
            tracing::debug!(target: "shodh::memory", error = %e, "memory unavailable; nothing injected");
            return None;
        }
        Err(_) => {
            tracing::warn!(target: "shodh::memory", "memory unavailable in time; nothing injected");
            return None;
        }
    };
    // The core needs no search models: it fails open on its own, and so does recall.
    let core = match tokio::time::timeout_at(deadline, service.core_for_run(&scope)).await {
        Ok(Ok(core)) => core,
        Ok(Err(e)) => {
            tracing::warn!(target: "shodh::memory", error = %e, "core memories not read; none injected");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!(target: "shodh::memory", "core memories timed out; none injected");
            Vec::new()
        }
    };
    let core_block = render_core_counted(&core);
    let in_core = core_block.as_ref().map_or(0, |(_, n)| *n);
    // As many more as the core may take back, so the recalled block is not left short
    // by memories that are in the core block already.
    let request = RecallRequest::new(
        text,
        scope.clone(),
        INJECTED_MEMORIES + in_core,
        RecallMode::Inspect,
    );
    let recalled = match tokio::time::timeout_at(deadline, service.recall(&request, actor)).await {
        Ok(Ok(recalled)) => recalled,
        Ok(Err(MemoryError::Statement(StatementError::EmbeddingUnavailable(_)))) => {
            tracing::debug!(target: "shodh::memory", "search models not installed; no memories recalled");
            Vec::new()
        }
        Ok(Err(e)) => {
            tracing::warn!(target: "shodh::memory", error = %e, "memory recall failed; only core injected");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!(target: "shodh::memory", "memory recall timed out; only core injected");
            Vec::new()
        }
    };
    compose_run_memories(&core, core_block, recalled)
}

/// The run's blocks from the selected core memories (`core`, whose rendered block holds
/// the first ones) and the recalled ones: those in the core block are dropped from the
/// recalled ones, which keep at most [`INJECTED_MEMORIES`] within what the core block
/// left of [`MAX_RUN_MEMORY_CHARS`] (never more than [`MAX_INJECTION_CHARS`]).
fn compose_run_memories(
    core: &[RecalledMemory],
    core_block: Option<(String, usize)>,
    recalled: Vec<RecalledMemory>,
) -> Option<RunMemories> {
    let core_ids: Vec<String> = core_block
        .as_ref()
        .map(|(_, n)| core[..*n].iter().map(|m| m.memory.id.clone()).collect())
        .unwrap_or_default();
    let recalled: Vec<RecalledMemory> = recalled
        .into_iter()
        .filter(|m| !core_ids.contains(&m.memory.id))
        .take(INJECTED_MEMORIES)
        .collect();
    let core_chars = core_block
        .as_ref()
        .map_or(0, |(block, _)| block.len() + BLOCK_SEPARATOR.len());
    let budget = MAX_RUN_MEMORY_CHARS
        .saturating_sub(core_chars)
        .min(MAX_INJECTION_CHARS);
    let recalled_block = render_injection_counted(&recalled, budget);
    let mut ids = core_ids.clone();
    let block = match (core_block, recalled_block) {
        (None, None) => return None,
        (Some((core, _)), None) => core,
        (None, Some((block, n))) => {
            ids.extend(recalled[..n].iter().map(|m| m.memory.id.clone()));
            block
        }
        (Some((core, _)), Some((block, n))) => {
            ids.extend(recalled[..n].iter().map(|m| m.memory.id.clone()));
            format!("{core}{BLOCK_SEPARATOR}{block}")
        }
    };
    Some(RunMemories {
        block,
        ids,
        core_ids,
    })
}

/// `message` with the memory block in front of it, delimited from the user's words.
pub fn with_memories(block: Option<&str>, message: &str) -> String {
    match block {
        Some(block) => format!("{block}\n\nCurrent message:\n{message}"),
        None => message.to_string(),
    }
}

/// Memories for Settings → Memory: current ones (and history on request), optionally
/// searched. Listing is not use: nothing is reinforced.
#[tauri::command]
pub async fn memory_list(
    request: Option<ListRequest>,
    memory: State<'_, MemoryState>,
) -> Result<Vec<MemoryRecord>, String> {
    let service = memory.service().await?;
    service
        .list(&request.unwrap_or_default())
        .await
        .map_err(error_text)
}

/// Every version of the fact a memory belongs to, oldest first.
#[tauri::command]
pub async fn memory_history(
    id: String,
    memory: State<'_, MemoryState>,
) -> Result<Vec<MemoryRecord>, String> {
    let service = memory.service().await?;
    service.history(&id).await.map_err(error_text)
}

/// Edit a memory in Settings: `content` is the complete new fact. The old version is
/// kept as history. Audited as `memory_write`.
#[tauri::command]
pub async fn memory_update(
    id: String,
    content: MemoryContent,
    memory: State<'_, MemoryState>,
) -> Result<RememberOutcome, String> {
    let service = memory.service().await?;
    let outcome = service
        .update(
            &id,
            content,
            false,
            None,
            &Origin::user_interface(APP_VERSION),
            &Actor::ui(),
        )
        .await
        .map_err(error_text)?;
    memory.changed();
    Ok(outcome)
}

/// Pin (exempt from decay) or unpin a memory. Audited as `memory_write`.
#[tauri::command]
pub async fn memory_set_pinned(
    id: String,
    pinned: bool,
    memory: State<'_, MemoryState>,
) -> Result<MemoryRecord, String> {
    let service = memory.service().await?;
    service
        .set_pinned(&id, pinned, &Actor::ui())
        .await
        .map_err(error_text)
}

/// Forget a memory and all its versions. Audited as `memory_forget`.
#[tauri::command]
pub async fn memory_forget(
    id: String,
    memory: State<'_, MemoryState>,
) -> Result<Vec<String>, String> {
    let service = memory.service().await?;
    let forgotten = service
        .forget(&id, None, &Actor::ui())
        .await
        .map_err(error_text)?;
    memory.changed();
    Ok(forgotten)
}

/// One memory an answer was given, as its "Used N memories" list shows it now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsedMemory {
    /// The memory's id, as the answer keeps it.
    pub id: String,
    /// The memory as it is now; `None` when it was forgotten (its words are gone).
    pub memory: Option<MemoryRecord>,
    /// The user marked it not relevant to this answer.
    pub not_relevant: bool,
    /// It was given to the answer as a core memory (in front of every question).
    pub always_included: bool,
}

/// Checks the answer's run id and memory ids sent with a lookup or feedback.
fn checked_ids(run_id: &str, ids: &[String]) -> Result<(), String> {
    if run_id.trim().is_empty() || run_id.len() > 200 {
        return Err("The answer could not be identified.".to_string());
    }
    if ids.len() > MAX_IDS_PER_CALL || ids.iter().any(|id| id.trim().is_empty() || id.len() > 200) {
        return Err("The memories could not be identified.".to_string());
    }
    Ok(())
}

/// The memories `ids` an answer (run `run_id`) was given, read now, in order: a forgotten
/// memory comes back without its words. The answer keeps the ids only. Reading is not
/// use: nothing is reinforced.
#[tauri::command]
pub async fn memory_used_lookup(
    run_id: String,
    ids: Vec<String>,
    memory: State<'_, MemoryState>,
) -> Result<Vec<UsedMemory>, String> {
    checked_ids(&run_id, &ids)?;
    let service = memory.service().await?;
    let records = service.lookup(&ids).await.map_err(error_text)?;
    let marked = service
        .marked_irrelevant(&run_id, &ids)
        .await
        .map_err(error_text)?;
    let core = service
        .always_included(&run_id, &ids)
        .await
        .map_err(error_text)?;
    Ok(ids
        .into_iter()
        .zip(records)
        .map(|(id, memory)| UsedMemory {
            not_relevant: marked.contains(&id),
            always_included: core.contains(&id),
            id,
            memory,
        })
        .collect())
}

/// The core memories of Settings, Memory (global ones), or of workspace `workspace`'s
/// chats: what is put in front of every question there, as it would be now (those that fit
/// in the core block, best first). Reading is not use: nothing is reinforced.
#[tauri::command]
pub async fn memory_core_list(
    workspace: Option<String>,
    memory: State<'_, MemoryState>,
) -> Result<Vec<MemoryRecord>, String> {
    let service = memory.service().await?;
    let workspace = workspace.filter(|w| !w.trim().is_empty());
    let core = service
        .core_for_run(&Scope::for_workspace(workspace.as_deref()))
        .await
        .map_err(error_text)?;
    let shown = render_core_counted(&core).map_or(0, |(_, n)| n);
    Ok(core.into_iter().take(shown).map(|m| m.memory).collect())
}

/// "Not relevant" on memory `id` in the answer of run `run_id`: the memory loses a little
/// importance and its links to the answer's other memories weaken
/// ([`MemoryService::record_feedback`]); it is not edited or deleted. Refused unless the
/// backend recorded `id` as given to that answer; the other memories are read from that
/// record, not taken from the caller. Once per answer: returns `false` when it was marked
/// before. Audited as `memory_write` with ids only.
#[tauri::command]
pub async fn memory_mark_not_relevant(
    run_id: String,
    id: String,
    memory: State<'_, MemoryState>,
) -> Result<bool, String> {
    checked_ids(&run_id, std::slice::from_ref(&id))?;
    let service = memory.service().await?;
    service
        .record_feedback(&run_id, &id, Feedback::Irrelevant, &Actor::ui())
        .await
        .map_err(error_text)
}

/// What the forget confirmation tells about the audit log.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetNotice {
    /// Audit events written before memory events stopped carrying words still show some
    /// of this memory's.
    pub older_entries_have_words: bool,
}

/// What forgetting memory `id` leaves in the audit log, for the confirmation.
#[tauri::command]
pub async fn memory_forget_notice(
    id: String,
    memory: State<'_, MemoryState>,
) -> Result<ForgetNotice, String> {
    let service = memory.service().await?;
    Ok(ForgetNotice {
        older_entries_have_words: service.audit_holds_words(&id).await.map_err(error_text)?,
    })
}

/// Where an export was written and how many memories it holds.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryExport {
    pub path: String,
    pub memories: usize,
}

/// Export every memory (with history) as JSON to `path` (from the save dialog).
#[tauri::command]
pub async fn memory_export(
    path: String,
    memory: State<'_, MemoryState>,
) -> Result<MemoryExport, String> {
    let path = export_path(&path)?;
    let service = memory.service().await?;
    let document = service.export().await.map_err(error_text)?;
    let memories = document["memories"].as_array().map_or(0, Vec::len);
    let text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
    let target = path.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&target, text))
        .await
        .map_err(|e| format!("Export failed: {e}"))?
        .map_err(|e| format!("Export to {} failed: {e}", path.display()))?;
    Ok(MemoryExport {
        path: path.display().to_string(),
        memories,
    })
}

fn export_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err("Choose where to save the export".to_string());
    }
    match path.parent() {
        Some(parent) if parent.is_dir() => {}
        _ => return Err(format!("The folder for {} does not exist", path.display())),
    }
    if path.is_dir() {
        return Err(format!("{} is a folder", path.display()));
    }
    let is_json = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    Ok(if is_json {
        path
    } else {
        path.with_extension("json")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_is_kept_apart_from_the_users_words() {
        assert_eq!(with_memories(None, "hello"), "hello");
        let message = with_memories(Some("<memory>\n- x\n</memory>"), "What's next?");
        assert!(message.starts_with("<memory>"));
        assert!(message.ends_with("Current message:\nWhat's next?"));
    }

    #[test]
    fn exports_are_json_files_in_existing_folders() {
        let dir = tempfile::tempdir().unwrap();
        let p = export_path(&dir.path().join("mem.txt").display().to_string()).unwrap();
        assert_eq!(p.extension().unwrap(), "json");
        let p = export_path(&dir.path().join("mem.JSON").display().to_string()).unwrap();
        assert_eq!(p.file_name().unwrap(), "mem.JSON");
        assert!(export_path("relative.json").is_err());
        assert!(export_path(&dir.path().display().to_string()).is_err());
        assert!(export_path(
            &dir.path()
                .join("missing")
                .join("m.json")
                .display()
                .to_string()
        )
        .is_err());
    }

    /// A note the learner formulated from a conversation and the user approved: recalled
    /// when relevant, never a core memory (not in the user's own words).
    fn learned() -> Origin {
        Origin {
            source: shodh_rag::user_memory::conversation_source("conv-0", "run-0"),
            extractor: shodh_ontology::ExtractorKind::Llm,
            extractor_version: "model".into(),
            confidence: 0.9,
            authority: shodh_rag::user_memory::WriteAuthority::UserApproval {
                step_id: "step-0".into(),
            },
        }
    }

    /// A memory state over `dir` with two learned notes (not core); returns it, the audit
    /// log and the notes' ids (invoices, gym).
    async fn two_notes(dir: &Path) -> (MemoryState, Arc<AuditLog>, String, String) {
        let audit = Arc::new(AuditLog::open(dir.join("shodh.db"), None).unwrap());
        let memory = crate::agent_tools::testing::memory_state(dir, Some(audit.clone())).await;
        let service = memory.service().await.unwrap();
        let mut ids = Vec::new();
        for text in ["Invoices go to Priya", "Gym on Tuesdays"] {
            let saved = service
                .remember(
                    MemoryContent::Note { text: text.into() },
                    Scope::Global,
                    &learned(),
                    &Actor::ui(),
                )
                .await
                .unwrap();
            ids.push(saved.memory.unwrap().id);
        }
        let gym = ids.pop().unwrap();
        let invoices = ids.pop().unwrap();
        (memory, audit, invoices, gym)
    }

    /// `memory_use` audit events, after every queued event is written.
    fn use_events(audit: &AuditLog) -> Vec<shodh_rag::audit::AuditRow> {
        use shodh_rag::audit::{AuditEventType, AuditQuery, AuditRecord};
        // The writer is FIFO: once this append commits, every earlier submit has too.
        audit
            .append(AuditRecord::new(
                AuditEventType::Question,
                serde_json::json!({}),
            ))
            .unwrap();
        // A queued event the writer failed to record would be missing below; name its error.
        assert_eq!(audit.unrecorded(), None);
        audit
            .query(&AuditQuery {
                types: vec![AuditEventType::MemoryUse],
                ..Default::default()
            })
            .unwrap()
    }

    async fn use_count(memory: &MemoryState, id: &str) -> u32 {
        memory
            .service()
            .await
            .unwrap()
            .get(id)
            .await
            .unwrap()
            .use_count
    }

    fn delta(run_id: &str, message_id: &str, text: &str) -> AgentEvent {
        AgentEvent::TextDelta {
            run_id: run_id.into(),
            message_id: message_id.into(),
            delta: text.into(),
        }
    }

    fn finished(run_id: &str, status: RunStatus) -> AgentEvent {
        AgentEvent::RunFinished {
            run_id: run_id.into(),
            status,
            duration_ms: 1,
            error: None,
            provider_error: None,
        }
    }

    #[tokio::test]
    async fn a_run_gets_relevant_memories_and_reinforces_none_at_injection() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, audit, invoices, gym) = two_notes(dir.path()).await;
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        // The default floor is tuned for E5; the test embedder scores identical words 1.
        let recalled = recall_for_run(&memory, "Invoices go to Priya?", None, &actor)
            .await
            .unwrap();
        assert!(recalled.block.contains("Invoices go to Priya"));
        assert!(!recalled.block.contains("Gym"));
        assert_eq!(recalled.ids, vec![invoices.clone()]);
        // Being put in front of a question is not use.
        assert_eq!(use_count(&memory, &invoices).await, 0);
        assert_eq!(use_count(&memory, &gym).await, 0);
        assert!(use_events(&audit).is_empty());
        assert!(
            recall_for_run(&memory, "Explain gradient descent", None, &actor)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_completed_run_reinforces_only_the_memories_its_answer_used() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, audit, invoices, gym) = two_notes(dir.path()).await;
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        memory.track_run(
            "run-1",
            vec![invoices.clone(), gym.clone()],
            None,
            actor,
            Vec::new(),
        );
        // A working note before a tool call is not the answer, even when it restates a
        // memory.
        assert!(memory
            .observe(&delta("run-1", "m1", "Gym on Tuesdays."))
            .is_none());
        memory.observe(&AgentEvent::TextInterim {
            run_id: "run-1".into(),
            message_id: "m1".into(),
            interim: true,
        });
        memory.observe(&delta("run-1", "m2", "Invoices go to "));
        memory.observe(&delta("run-1", "m2", "Priya. Send them today."));
        // Another run's text does not count.
        memory.observe(&delta("run-9", "m3", "Gym on Tuesdays."));
        let reinforcing = memory
            .observe(&finished("run-1", RunStatus::Completed))
            .expect("a completed run with memories is judged");
        reinforcing.await.unwrap();
        assert_eq!(use_count(&memory, &invoices).await, 1);
        assert_eq!(use_count(&memory, &gym).await, 0);
        let used = use_events(&audit);
        assert_eq!(used.len(), 1);
        assert_eq!(used[0].conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(used[0].run_id.as_deref(), Some("run-1"));
        assert_eq!(used[0].payload["ids"], serde_json::json!([invoices]));
        let payload = used[0].payload.to_string();
        assert!(
            !payload.contains("Priya") && !payload.contains("Gym"),
            "{payload}"
        );
        // The run's end was handled: a repeated end changes nothing.
        assert!(memory
            .observe(&finished("run-1", RunStatus::Completed))
            .is_none());
    }

    #[tokio::test]
    async fn an_interrupted_or_failed_run_reinforces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, audit, invoices, _) = two_notes(dir.path()).await;
        for (run, status) in [("run-2", RunStatus::Aborted), ("run-3", RunStatus::Error)] {
            let actor = Actor::agent("local-owner", "conv-1", "assistant", run);
            memory.track_run(run, vec![invoices.clone()], None, actor, Vec::new());
            memory.observe(&delta(run, "m1", "Invoices go to Priya."));
            assert!(memory.observe(&finished(run, status)).is_none());
        }
        // A run whose prompt failed is forgotten before it ends.
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-4");
        memory.track_run("run-4", vec![invoices.clone()], None, actor, Vec::new());
        memory.forget_run("run-4");
        memory.observe(&delta("run-4", "m1", "Invoices go to Priya."));
        assert!(memory
            .observe(&finished("run-4", RunStatus::Completed))
            .is_none());
        assert_eq!(use_count(&memory, &invoices).await, 0);
        assert!(use_events(&audit).is_empty());
    }

    #[tokio::test]
    async fn used_memories_are_looked_up_live_and_marked_not_relevant_once() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, _audit, invoices, gym) = two_notes(dir.path()).await;
        let service = memory.service().await.unwrap();
        let ids = vec![invoices.clone(), gym.clone()];
        assert!(checked_ids("run-1", &ids).is_ok());
        assert!(checked_ids("", &ids).is_err());
        let too_many: Vec<String> = (0..=MAX_IDS_PER_CALL).map(|i| format!("m{i}")).collect();
        assert!(checked_ids("run-1", &too_many).is_err());
        // Feedback is refused for an answer the memory was not given to.
        assert!(service
            .record_feedback("run-1", &gym, Feedback::Irrelevant, &Actor::ui())
            .await
            .is_err());
        memory.record_injected("run-1", &ids, &[]).await;
        let applied = service
            .record_feedback("run-1", &gym, Feedback::Irrelevant, &Actor::ui())
            .await
            .unwrap();
        assert!(applied);
        service.forget(&invoices, None, &Actor::ui()).await.unwrap();
        let records = service.lookup(&ids).await.unwrap();
        let marked = service.marked_irrelevant("run-1", &ids).await.unwrap();
        assert!(records[0].is_none(), "a forgotten memory has no words");
        assert_eq!(records[1].as_ref().unwrap().text, "Gym on Tuesdays");
        assert!(marked.contains(&gym) && !marked.contains(&invoices));
    }

    /// A note the user typed in Settings (a core memory) in `workspace`'s scope.
    async fn own_note(memory: &MemoryState, text: &str, workspace: Option<&str>) -> String {
        memory
            .service()
            .await
            .unwrap()
            .remember(
                MemoryContent::Note { text: text.into() },
                Scope::for_workspace(workspace),
                &Origin::user_interface("t"),
                &Actor::ui(),
            )
            .await
            .unwrap()
            .memory
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn core_memories_go_in_front_of_every_question_first_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, audit, invoices, gym) = two_notes(dir.path()).await;
        let rule = own_note(&memory, "Keep answers short", None).await;
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        // A question the core has nothing to do with still gets it, and only it.
        let unrelated = recall_for_run(&memory, "Explain gradient descent", None, &actor)
            .await
            .unwrap();
        assert_eq!(unrelated.ids, vec![rule.clone()]);
        assert_eq!(unrelated.core_ids, vec![rule.clone()]);
        assert!(unrelated.block.starts_with(&format!(
            "<memory>\n{}",
            shodh_rag::user_memory::CORE_BLOCK_TITLE
        )));
        assert!(!unrelated.block.contains("Invoices") && !unrelated.block.contains("Gym"));
        // A relevant question gets the core first, then the recalled memories.
        let relevant = recall_for_run(&memory, "Invoices go to Priya?", None, &actor)
            .await
            .unwrap();
        assert_eq!(relevant.ids, vec![rule.clone(), invoices.clone()]);
        assert_eq!(relevant.core_ids, vec![rule.clone()]);
        let core_at = relevant.block.find("Keep answers short").unwrap();
        let recalled_at = relevant
            .block
            .find(shodh_rag::user_memory::MEMORY_BLOCK_TITLE)
            .unwrap();
        assert!(core_at < recalled_at, "{}", relevant.block);
        assert!(relevant.block.len() <= MAX_RUN_MEMORY_CHARS);
        // A core memory the question is about is not repeated among the recalled ones.
        let about_core = recall_for_run(&memory, "Keep answers short?", None, &actor)
            .await
            .unwrap();
        assert_eq!(about_core.ids, vec![rule.clone()]);
        assert_eq!(about_core.block.matches("Keep answers short").count(), 1);
        // Being put in front of a question is not use.
        for id in [&rule, &invoices, &gym] {
            assert_eq!(use_count(&memory, id).await, 0);
        }
        assert!(use_events(&audit).is_empty());
        // Forgotten, it is gone from the next question.
        memory
            .service()
            .await
            .unwrap()
            .forget(&rule, None, &Actor::ui())
            .await
            .unwrap();
        assert!(
            recall_for_run(&memory, "Explain gradient descent", None, &actor)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_workspace_chat_never_gets_another_workspaces_core() {
        let dir = tempfile::tempdir().unwrap();
        let (memory, _audit, _, _) = two_notes(dir.path()).await;
        let mine = own_note(&memory, "This workspace cites in IEEE style", Some("a")).await;
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        let in_a = recall_for_run(&memory, "Explain gradient descent", Some("a"), &actor)
            .await
            .unwrap();
        assert_eq!(in_a.core_ids, vec![mine]);
        for other in [Some("b"), None] {
            assert!(
                recall_for_run(&memory, "Explain gradient descent", other, &actor)
                    .await
                    .is_none(),
                "{other:?}"
            );
        }
    }

    /// A state over `dir` with one core rule (a note the user typed) and one recallable
    /// learned note, and the switch of its embedder.
    async fn rule_and_note(
        dir: &Path,
    ) -> (
        MemoryState,
        crate::agent_tools::testing::EmbedderSwitch,
        String,
        String,
    ) {
        let (memory, switch) = crate::agent_tools::testing::memory_state_switched(dir, None).await;
        let rule = own_note(&memory, "Keep answers short", None).await;
        let service = memory.service().await.unwrap();
        let note = service
            .remember(
                MemoryContent::Note {
                    text: "Invoices go to Priya".into(),
                },
                Scope::Global,
                &learned(),
                &Actor::ui(),
            )
            .await
            .unwrap()
            .memory
            .unwrap()
            .id;
        (memory, switch, rule, note)
    }

    #[tokio::test]
    async fn core_is_still_injected_when_the_search_models_are_missing() {
        use crate::agent_tools::testing::Embedder;
        let dir = tempfile::tempdir().unwrap();
        let (memory, switch, rule, _) = rule_and_note(dir.path()).await;
        switch.set(Embedder::Missing);
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        let given = recall_for_run(&memory, "Invoices go to Priya?", None, &actor)
            .await
            .expect("the core needs no search models");
        assert_eq!(given.ids, vec![rule.clone()]);
        assert_eq!(given.core_ids, vec![rule]);
        assert!(given.block.contains("Keep answers short"));
        assert!(!given.block.contains("Invoices"));
    }

    #[tokio::test]
    async fn core_is_still_injected_when_recall_fails() {
        use crate::agent_tools::testing::Embedder;
        let dir = tempfile::tempdir().unwrap();
        let (memory, switch, rule, _) = rule_and_note(dir.path()).await;
        switch.set(Embedder::Failing);
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        let given = recall_for_run(&memory, "Invoices go to Priya?", None, &actor)
            .await
            .expect("a failing recall leaves the core");
        assert_eq!(given.ids, vec![rule]);
        assert!(!given.block.contains("Invoices"));
    }

    #[tokio::test]
    async fn core_is_still_injected_when_recall_times_out() {
        use crate::agent_tools::testing::Embedder;
        let dir = tempfile::tempdir().unwrap();
        let (memory, switch, rule, _) = rule_and_note(dir.path()).await;
        switch.set(Embedder::Held);
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        let started = std::time::Instant::now();
        let given = recall_for_run(&memory, "Invoices go to Priya?", None, &actor)
            .await
            .expect("a slow recall leaves the core");
        assert!(started.elapsed() < RECALL_TIMEOUT * 3);
        assert_eq!(given.ids, vec![rule]);
        assert!(!given.block.contains("Invoices"));
    }

    #[tokio::test]
    async fn a_core_memory_the_answer_did_not_use_is_not_reinforced_and_is_listed_as_always_included(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (memory, audit, _, _) = two_notes(dir.path()).await;
        let rule = own_note(&memory, "Keep answers short", None).await;
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-5");
        let given = recall_for_run(&memory, "Explain gradient descent", None, &actor)
            .await
            .unwrap();
        memory.track_run("run-5", given.ids.clone(), None, actor, Vec::new());
        memory
            .record_injected("run-5", &given.ids, &given.core_ids)
            .await;
        memory.observe(&delta(
            "run-5",
            "m1",
            "Gradient descent follows the negative gradient of the loss.",
        ));
        memory
            .observe(&finished("run-5", RunStatus::Completed))
            .expect("a completed run with memories is judged")
            .await
            .unwrap();
        assert_eq!(use_count(&memory, &rule).await, 0);
        assert!(use_events(&audit).is_empty());
        let service = memory.service().await.unwrap();
        let always = service.always_included("run-5", &given.ids).await.unwrap();
        assert!(always.contains(&rule));
    }

    #[test]
    fn the_whole_injection_budget_is_the_core_budget_plus_the_recalled_one() {
        assert_eq!(
            MAX_RUN_MEMORY_CHARS,
            shodh_rag::user_memory::CORE_MAX_CHARS + MAX_INJECTION_CHARS
        );
        assert_eq!(MAX_RUN_MEMORY_CHARS, 3_500);
        assert!(shodh_rag::user_memory::CORE_MAX_ITEMS + INJECTED_MEMORIES <= MAX_IDS_PER_CALL);
    }

    #[test]
    fn the_memory_ontology_loads() {
        let ontology = memory_ontology().unwrap();
        for class in [
            "Note",
            "Preference",
            "Person",
            "Project",
            "Decision",
            "Concept",
        ] {
            assert!(ontology.class(class).is_some(), "{class}");
        }
    }
}
