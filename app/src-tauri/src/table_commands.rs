//! Evidence tables: the managed state, the commands of the Tables tab, the fill scheduler
//! and the notifications of a fill's end (see `shodh_rag::research::tables`).
//!
//! - **Model.** A run fills with the model answers come from
//!   ([`crate::memory_learn::configured_table_model`]), resolved when the run starts: a
//!   change of model applies from the next run, and a run in flight finishes with the
//!   model it started with. The API key is read at every call, so a corrected key applies
//!   from the next call. A model whose reasoning cannot be bounded is refused with the
//!   reason ([`shodh_rag::research::tables::reasoning`]).
//! - **Daily cap.** One shared [`DailyCap`] (Settings → Tables, `tables.dailyCallCap`) is
//!   read by every run and hand fill at every check, so a change applies from the next
//!   paper. Hand fills (`table_fill_paper`, `cell_refill`) count against it too.
//! - **Scheduler.** Once the app has loaded its model settings, and then every
//!   [`SCHEDULER_TICK`], the due jobs are run: queued ones, paused ones whose wait is over
//!   (a backoff rung, the provider's reset, the 00:00 UTC cap reset) and running ones
//!   whose run is gone (another process's lease expired, or a run of this process that
//!   ended without writing its end). A job is run once at a time in this process; the
//!   job's claim also refuses a second run anywhere. `fill_start` runs its job at once.
//! - **Events.** Every change emits [`TABLE_CHANGED_EVENT`] with `{ "tableId" }`, and so
//!   does the end of every run, whatever its outcome; the Tables tab polls `fill_status`
//!   while a job is active.
//! - **Notifications.** Only what needs the user ([`crate::notifications::table_outcome`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use shodh_rag::audit::{AuditEventType, AuditKey, AuditRecord};
use shodh_rag::comprehensive_system::ComprehensiveRAG as RAGEngine;
use shodh_rag::harness::grounding::{ScorerSet, SharedEntailment};
use shodh_rag::llm::{LLMConfig, LLMManager};
use shodh_rag::research::results::{ModelReply, ResultRecord, TextModel};
use shodh_rag::research::tables::calls::{
    check_cap, next_utc_midnight, utc_day, CallCounter, DailyCap, MAX_DAILY_CAP, MIN_DAILY_CAP,
};
use shodh_rag::research::tables::fill::{Filler, Halt, PaperFill, PaperRequest};
use shodh_rag::research::tables::jobs::{
    cancel_job, resume_due_jobs, FillEstimate, FillJob, JobRunner, JobState, PauseReason,
    ResultSource, RunReport,
};
use shodh_rag::research::tables::parse::{parse_cut_reply, parse_reply};
use shodh_rag::research::tables::passages::PaperIndex;
use shodh_rag::research::tables::reasoning::{ReasoningCatalog, ReasoningRule};
use shodh_rag::research::tables::{
    Cell, CellStatus, Column, ColumnKind, ColumnSpec, EvidenceTable, TableRow, TableStore,
    TableStoreHandle, MAX_COLUMNS,
};
use shodh_rag::research::{file_name, ResearchError, ResearchResult};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{watch, OnceCell, RwLock as AsyncRwLock};

use crate::answer_check_commands::SharedNli;
use crate::app_settings::{broadcast, SettingsStore};
use crate::audit_commands::AuditState;
use crate::llm_commands::{ApiKeys, LLMState};
use crate::notifications::{announce_table, table_outcome, TableRunEnd};
use crate::research_commands::{ResearchCommandError, ResearchCommandResult, ResearchState};

/// Emitted with `{ "tableId": … }` after anything in a table changed, and after every
/// run of its fill ends.
pub const TABLE_CHANGED_EVENT: &str = "table_changed";
/// How often the scheduler looks for due jobs.
pub const SCHEDULER_TICK: Duration = Duration::from_secs(60);
/// Longest id a command accepts (ids are UUIDs).
pub const MAX_ID_CHARS: usize = 64;
/// Most papers one `rows_add` adds.
pub const MAX_ROWS_PER_ADD: usize = 500;
/// Longest file path a row is added with.
pub const MAX_PATH_CHARS: usize = 4_096;
/// Longest value a user types into a cell (a text cell holds at most 500 characters;
/// the type rule says so for anything up to this).
pub const MAX_EDIT_CHARS: usize = 2_000;
/// Most calls a fill can be started for: far above any table's (22 columns, two calls
/// per paper), low enough to refuse a number that is not an estimate.
pub const MAX_PLANNED_CALLS: usize = 100_000;
/// Where `table_fill_paper` writes what each model call sent and received (under the app
/// data directory).
pub const CAPTURES_DIR: &str = "tables/captures";

type TableResult<T> = ResearchCommandResult<T>;

// ── Argument checks (pure, tested) ───────────────────────────────

fn invalid(message: impl Into<String>) -> ResearchCommandError {
    ResearchCommandError::invalid(message)
}

/// An id: 1 to [`MAX_ID_CHARS`] ASCII letters, digits, `-` or `_`.
pub fn check_id(what: &str, id: &str) -> TableResult<()> {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("That is not a valid {what} id.")))
    }
}

/// A column's spec, cleaned: a question, its lengths, and 2–12 distinct options for a
/// pick-list (none for other kinds).
pub fn check_spec(spec: &ColumnSpec) -> TableResult<ColumnSpec> {
    Ok(spec.cleaned()?)
}

/// Whether a table that has `existing` columns can take one more.
pub fn check_column_room(existing: usize) -> TableResult<()> {
    if existing >= MAX_COLUMNS {
        return Err(invalid(format!(
            "A table has at most {MAX_COLUMNS} columns."
        )));
    }
    Ok(())
}

/// A column's new place: 0 to [`MAX_COLUMNS`] − 1 (a place past the last column is last).
pub fn check_position(position: usize) -> TableResult<()> {
    if position >= MAX_COLUMNS {
        return Err(invalid(format!(
            "A column's place is 0 to {}.",
            MAX_COLUMNS - 1
        )));
    }
    Ok(())
}

/// The papers to add: 1 to [`MAX_ROWS_PER_ADD`] file paths, trimmed, each at most
/// [`MAX_PATH_CHARS`] long and without control characters.
pub fn check_documents(paths: &[String]) -> TableResult<Vec<String>> {
    if paths.is_empty() {
        return Err(invalid("Choose at least one paper."));
    }
    if paths.len() > MAX_ROWS_PER_ADD {
        return Err(invalid(format!(
            "At most {MAX_ROWS_PER_ADD} papers can be added at once."
        )));
    }
    paths
        .iter()
        .map(|path| {
            let path = path.trim();
            if path.is_empty()
                || path.chars().count() > MAX_PATH_CHARS
                || path.chars().any(char::is_control)
            {
                Err(invalid("One of the papers has no usable file path."))
            } else {
                Ok(path.to_string())
            }
        })
        .collect()
}

/// The day's cap of table calls: [`MIN_DAILY_CAP`] to [`MAX_DAILY_CAP`].
pub fn check_daily_cap(calls: usize) -> TableResult<()> {
    Ok(check_cap(calls)?)
}

/// A value typed into a cell: at most [`MAX_EDIT_CHARS`] (its column's type rule checks
/// the rest).
pub fn check_edit(value: &str) -> TableResult<()> {
    if value.chars().count() > MAX_EDIT_CHARS {
        return Err(invalid(format!(
            "A cell value is at most {MAX_EDIT_CHARS} characters."
        )));
    }
    Ok(())
}

/// The calls the user agreed to: at most [`MAX_PLANNED_CALLS`].
pub fn check_planned_calls(calls: usize) -> TableResult<()> {
    if calls > MAX_PLANNED_CALLS {
        return Err(invalid("That is not a call estimate of this table."));
    }
    Ok(())
}

// ── Views ────────────────────────────────────────────────────────

/// A fill job as the Tables tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub table_id: String,
    pub state: JobState,
    pub planned_calls: usize,
    pub done_calls: usize,
    pub pause_reason: Option<PauseReason>,
    /// The code of what paused or stopped the job (for a search failure, with detail).
    pub last_error: Option<String>,
    /// When a paused job goes on by itself (UTC, Unix milliseconds); none while it waits
    /// for the user.
    pub next_attempt_at: Option<i64>,
    pub cancel_requested: bool,
    pub created_at: String,
}

/// A stored UTC time as Unix milliseconds.
fn utc_ms(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp_millis())
}

pub fn job_view(job: &FillJob) -> JobView {
    JobView {
        id: job.id.clone(),
        table_id: job.table_id.clone(),
        state: job.state,
        planned_calls: job.planned_calls,
        done_calls: job.done_calls,
        pause_reason: job.pause_reason,
        last_error: job.last_error.clone(),
        next_attempt_at: job.next_attempt_at.as_deref().and_then(utc_ms),
        cancel_requested: job.cancel_requested,
        created_at: job.created_at.clone(),
    }
}

/// Papers none of whose cells waits in the fill queue: done for this run, or not part of
/// it. (A stale result being refilled counts until its refill.)
pub fn papers_done(rows: &[TableRow], cells: &[Cell]) -> usize {
    rows.iter()
        .filter(|row| {
            !cells
                .iter()
                .any(|c| c.row_id == row.id && c.status == CellStatus::Queued)
        })
        .count()
}

/// The table's fill and the day's calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillStatus {
    /// The table's newest job (active, or how the last fill ended).
    pub job: Option<JobView>,
    pub papers_done: usize,
    pub papers_total: usize,
    /// Table calls made today (UTC day), by every table.
    pub calls_today: usize,
    pub cap: usize,
    /// When the day's count starts again: the next 00:00 UTC (Unix milliseconds); the
    /// interface shows it in local time.
    pub day_resets_at: i64,
}

/// What filling the table would take, and the day's calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimateView {
    #[serde(flatten)]
    pub estimate: FillEstimate,
    pub calls_today: usize,
    pub cap: usize,
    pub day_resets_at: i64,
}

/// A row with the paper's title: the citation graph's title when the graph knows the
/// paper, else its file name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    #[serde(flatten)]
    pub row: TableRow,
    pub title: String,
}

/// A whole table.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableView {
    pub table: EvidenceTable,
    pub columns: Vec<Column>,
    pub rows: Vec<RowView>,
    pub cells: Vec<Cell>,
}

/// The tables settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablesSettings {
    pub daily_call_cap: usize,
    pub min_daily_call_cap: usize,
    pub max_daily_call_cap: usize,
}

fn settings_view(cap: usize) -> TablesSettings {
    TablesSettings {
        daily_call_cap: cap,
        min_daily_call_cap: MIN_DAILY_CAP,
        max_daily_call_cap: MAX_DAILY_CAP,
    }
}

/// What a fill by hand did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandFill {
    /// The cells it stored, as stored.
    pub cells: Vec<Cell>,
    pub calls: usize,
    pub cache_hits: usize,
    /// `daily_cap` when the day's cap stopped it before a call.
    pub halted: Option<&'static str>,
    /// The model's error code, when a call failed (`model_quota`, `model_auth`, …).
    pub model_error: Option<&'static str>,
    /// What kept the whole paper from being filled (`paper_not_indexed`, …).
    pub paper_error: Option<String>,
}

fn halt_code(halt: Halt) -> &'static str {
    match halt {
        Halt::Cancelled => "cancelled",
        Halt::DailyCap => "daily_cap",
    }
}

fn hand_fill(fill: &PaperFill) -> HandFill {
    HandFill {
        cells: fill.cells.clone(),
        calls: fill.calls,
        cache_hits: fill.cache_hits,
        halted: fill.halted.map(halt_code),
        model_error: fill.model_error.as_ref().map(|f| f.code()),
        paper_error: fill.paper_error.clone(),
    }
}

// ── Capture of model calls (table_fill_paper) ────────────────────

/// One model call of a fill by hand, byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedCall {
    pub prompt: String,
    pub prompt_chars: usize,
    pub prompt_sha256: String,
    /// The reply budget the fill asked for (the reasoning allowance comes on top; see
    /// the capture's `reasoningRule`).
    pub max_tokens: usize,
    /// The reply text exactly as the model returned it.
    pub reply: Option<String>,
    /// The provider said it stopped the reply at the limit.
    pub truncated: bool,
    /// The call's error as the adapter reported it (credentials masked).
    pub error: Option<String>,
    /// Cells the tolerant parse found in the reply (none: unreadable).
    pub parsed_cells: Option<usize>,
    /// The parse read the reply as cut off.
    pub cut_off: Option<bool>,
}

/// A model that keeps every call it passes on: what `table_fill_paper` writes to its
/// capture file and returns. It changes nothing about the calls.
struct RecordingModel {
    inner: Arc<dyn TextModel>,
    calls: Mutex<Vec<CapturedCall>>,
}

impl RecordingModel {
    fn new(inner: Arc<dyn TextModel>) -> Self {
        Self {
            inner,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<CapturedCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The record of one call: the prompt, and the reply or error as it came back.
fn captured(prompt: &str, max_tokens: usize, result: &Result<ModelReply, String>) -> CapturedCall {
    let (reply, truncated, error) = match result {
        Ok(reply) => (Some(reply.text.clone()), reply.truncated, None),
        Err(e) => (None, false, Some(e.clone())),
    };
    let parsed = reply.as_deref().map(|text| {
        if truncated {
            parse_cut_reply(text)
        } else {
            parse_reply(text)
        }
    });
    CapturedCall {
        prompt: prompt.to_string(),
        prompt_chars: prompt.chars().count(),
        prompt_sha256: hex::encode(Sha256::digest(prompt.as_bytes())),
        max_tokens,
        reply,
        truncated,
        error,
        parsed_cells: parsed
            .as_ref()
            .and_then(|p| p.as_ref().map(|p| p.cells.len())),
        cut_off: parsed.as_ref().and_then(|p| p.as_ref().map(|p| p.cut_off)),
    }
}

#[async_trait::async_trait]
impl TextModel for RecordingModel {
    fn model_id(&self) -> String {
        self.inner.model_id()
    }

    async fn complete(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
        self.complete_reply(prompt, max_tokens)
            .await
            .map(|reply| reply.text)
    }

    async fn complete_reply(&self, prompt: &str, max_tokens: usize) -> Result<ModelReply, String> {
        let result = self.inner.complete_reply(prompt, max_tokens).await;
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(captured(prompt, max_tokens, &result));
        result
    }
}

/// What `table_fill_paper` returns and writes to its capture file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperCapture {
    pub captured_at: String,
    pub table_id: String,
    pub row_id: String,
    pub document_id: String,
    pub model: String,
    pub reasoning_rule: String,
    pub calls: Vec<CapturedCall>,
    pub result: HandFill,
    /// The capture file, when it could be written.
    pub file: Option<String>,
}

/// The capture file of a fill by hand: `<captures>/<utc time>-<table>-<row>.json`.
pub fn capture_path(dir: &Path, at: DateTime<Utc>, table_id: &str, row_id: &str) -> PathBuf {
    dir.join(format!(
        "{}-{table_id}-{row_id}.json",
        at.format("%Y%m%dT%H%M%S%.3fZ")
    ))
}

fn write_capture(path: &Path, capture: &PaperCapture) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(capture).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

// ── State ────────────────────────────────────────────────────────

/// A paper's accepted Results through the research services, opened when first asked
/// for. They are search hints only: while the services cannot open, a fill runs without.
struct ServiceResults(ResearchState);

#[async_trait::async_trait]
impl ResultSource for ServiceResults {
    async fn accepted(&self, path: &str) -> ResearchResult<Vec<ResultRecord>> {
        let services = self
            .0
            .services()
            .await
            .map_err(|e| ResearchError::Model(e.message))?;
        services.results.accepted(path).await
    }
}

/// The evidence tables' managed state. Clones share it.
#[derive(Clone)]
pub struct TableState {
    inner: Arc<Inner>,
}

struct Inner {
    handle: Arc<TableStoreHandle>,
    database: Option<(PathBuf, Option<AuditKey>)>,
    counter: OnceCell<Arc<CallCounter>>,
    cap: DailyCap,
    data_dir: PathBuf,
    manager: Arc<AsyncRwLock<Option<LLMManager>>>,
    config: Arc<Mutex<LLMConfig>>,
    api_keys: Arc<Mutex<ApiKeys>>,
    rag: Arc<AsyncRwLock<RAGEngine>>,
    nli: SharedNli,
    research: ResearchState,
    catalog: ReasoningCatalog,
    /// Jobs with a run started from this process.
    in_flight: Mutex<HashSet<String>>,
    /// Becomes true once the app has loaded its model settings and keys.
    models_loaded: watch::Receiver<bool>,
    /// A scheduler tick that could not resolve a model was logged.
    blocked_logged: AtomicBool,
}

/// A job's place in the in-flight set, given back when the run's task ends.
struct InFlight {
    state: TableState,
    job_id: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.state
            .inner
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.job_id);
    }
}

/// What a `TableState` is built from.
pub struct TableStateParts {
    pub handle: Arc<TableStoreHandle>,
    pub database: Option<(PathBuf, Option<AuditKey>)>,
    pub data_dir: PathBuf,
    pub llm: (
        Arc<AsyncRwLock<Option<LLMManager>>>,
        Arc<Mutex<LLMConfig>>,
        Arc<Mutex<ApiKeys>>,
    ),
    pub rag: Arc<AsyncRwLock<RAGEngine>>,
    pub nli: SharedNli,
    pub research: ResearchState,
    pub models_loaded: watch::Receiver<bool>,
}

impl TableState {
    pub fn new(parts: TableStateParts) -> Self {
        let cap = SettingsStore::in_dir(&parts.data_dir)
            .load()
            .ok()
            .and_then(|s| DailyCap::new(s.tables.daily_cap()).ok())
            .unwrap_or_default();
        let (manager, config, api_keys) = parts.llm;
        Self {
            inner: Arc::new(Inner {
                handle: parts.handle,
                database: parts.database,
                counter: OnceCell::new(),
                cap,
                data_dir: parts.data_dir,
                manager,
                config,
                api_keys,
                rag: parts.rag,
                nli: parts.nli,
                research: parts.research,
                catalog: ReasoningCatalog::new(),
                in_flight: Mutex::new(HashSet::new()),
                models_loaded: parts.models_loaded,
                blocked_logged: AtomicBool::new(false),
            }),
        }
    }

    async fn store(&self) -> TableResult<Arc<TableStore>> {
        let handle = self.inner.handle.clone();
        tokio::task::spawn_blocking(move || handle.store())
            .await
            .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?
            .map_err(ResearchCommandError::from)
    }

    async fn counter(&self) -> TableResult<Arc<CallCounter>> {
        self.inner
            .counter
            .get_or_try_init(|| async {
                let (path, key) = self.inner.database.clone().ok_or_else(|| {
                    ResearchCommandError::unavailable(
                        "Evidence tables need the app database (shodh.db), which could not be opened; see the audit page.",
                    )
                })?;
                let counter =
                    tokio::task::spawn_blocking(move || CallCounter::open(&path, key.as_ref()))
                        .await
                        .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?
                        .map_err(ResearchCommandError::from)?;
                Ok::<_, ResearchCommandError>(Arc::new(counter))
            })
            .await
            .cloned()
    }

    async fn calls_today(&self) -> TableResult<usize> {
        let counter = self.counter().await?;
        let day = utc_day(Utc::now());
        Ok(tokio::task::spawn_blocking(move || counter.count(day))
            .await
            .map_err(|e| ResearchCommandError::unavailable(e.to_string()))??)
    }

    fn llm(&self) -> LLMState {
        LLMState {
            manager: self.inner.manager.clone(),
            config: self.inner.config.clone(),
            api_keys: self.inner.api_keys.clone(),
            custom_model_path: Arc::new(Mutex::new(None)),
        }
    }

    /// The model tables fill with now, and its reasoning rule.
    async fn model(&self) -> TableResult<(Arc<dyn TextModel>, ReasoningRule)> {
        crate::memory_learn::configured_table_model(
            &self.inner.data_dir,
            &self.llm(),
            &self.inner.catalog,
        )
        .await
        .map_err(ResearchCommandError::unavailable)
    }

    async fn filler(&self, model: Arc<dyn TextModel>) -> TableResult<Filler> {
        // The answer checking model, when installed (loaded on first use, off the async
        // workers); without it the contradiction check is skipped, as for answers.
        let nli = self.inner.nli.clone();
        let entailment: Option<SharedEntailment> = tokio::task::spawn_blocking(move || nli.get())
            .await
            .ok()
            .flatten()
            .map(|model| model as SharedEntailment);
        Ok(Filler {
            store: self.store().await?,
            index: self.inner.rag.clone() as Arc<dyn PaperIndex>,
            model,
            scorers: ScorerSet {
                relevance: None,
                entailment,
            },
        })
    }

    async fn runner(&self) -> TableResult<JobRunner> {
        let (model, _) = self.model().await?;
        Ok(JobRunner::new(
            self.filler(model).await?,
            self.counter().await?,
            self.results(),
            self.inner.cap.clone(),
        ))
    }

    fn results(&self) -> Arc<dyn ResultSource> {
        Arc::new(ServiceResults(self.inner.research.clone()))
    }

    fn gate(&self, counter: Arc<CallCounter>) -> shodh_rag::research::tables::jobs::CounterGate {
        shodh_rag::research::tables::jobs::CounterGate::new(counter, self.inner.cap.clone())
    }

    fn enter(&self, job_id: &str) -> Option<InFlight> {
        let entered = self
            .inner
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(job_id.to_string());
        entered.then(|| InFlight {
            state: self.clone(),
            job_id: job_id.to_string(),
        })
    }

    async fn models_loaded(&self) {
        let mut loaded = self.inner.models_loaded.clone();
        // A bootstrap that ended without saying so (its task failed) closes the channel:
        // the scheduler goes on then too.
        let _ = loaded.wait_for(|done| *done).await;
    }

    /// The paper's request: its row, the table's columns and the row's cells; Results as
    /// hints when a `number` column is asked.
    async fn request(
        &self,
        store: &Arc<TableStore>,
        table_id: &str,
        row_id: &str,
        asked: Option<&str>,
    ) -> TableResult<PaperRequest> {
        let (s, table, wanted) = (store.clone(), table_id.to_string(), row_id.to_string());
        let (row, columns, cells) = blocking(move || {
            let row = s
                .rows(&table)?
                .into_iter()
                .find(|r| r.id == wanted)
                .ok_or_else(|| {
                    ResearchError::NotFound(format!("No row {wanted} in this table."))
                })?;
            let columns = s.columns(&table)?;
            let cells: Vec<Cell> = s
                .cells(&table)?
                .into_iter()
                .filter(|c| c.row_id == row.id)
                .collect();
            Ok((row, columns, cells))
        })
        .await?;
        let wants_hints = columns
            .iter()
            .filter(|c| asked.is_none_or(|id| c.id == id))
            .any(|c| c.spec.kind == ColumnKind::Number);
        let results = if wants_hints {
            match self.results().accepted(&row.document_id).await {
                Ok(results) => results,
                Err(e) => {
                    tracing::warn!(error = %e, "evidence table fill runs without Result hints");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        Ok(PaperRequest {
            row,
            columns,
            cells,
            results,
        })
    }
}

async fn blocking<T, F>(job: F) -> TableResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ResearchResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?
        .map_err(ResearchCommandError::from)
}

fn emit_changed(app: &AppHandle, table_id: &str) {
    if let Err(e) = app.emit(TABLE_CHANGED_EVENT, json!({ "tableId": table_id })) {
        tracing::debug!(error = %e, "table change not emitted");
    }
}

fn now_ms(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

// ── Runs and the scheduler ───────────────────────────────────────

/// Runs a job on its own task unless this process already runs it; at its end, emits the
/// table's change and announces what needs the user.
fn spawn_run(app: &AppHandle, state: &TableState, runner: JobRunner, job_id: String) {
    let Some(in_flight) = state.enter(&job_id) else {
        return;
    };
    let app = app.clone();
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        let _in_flight = in_flight;
        let table_id = match runner.job(&job_id).await {
            Ok(Some(job)) => job.table_id,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(job = %job_id, error = %e, "evidence table fill job could not be read");
                return;
            }
        };
        let report = runner.run(&job_id).await;
        emit_changed(&app, &table_id);
        match report {
            Ok(report) => announce_run(&app, &state, &table_id, &job_id, &report).await,
            Err(e) => {
                tracing::warn!(job = %job_id, error = %e, "evidence table fill run failed; the job is given back")
            }
        }
    });
}

async fn announce_run(
    app: &AppHandle,
    state: &TableState,
    table_id: &str,
    job_id: &str,
    report: &RunReport,
) {
    tracing::info!(
        table = %table_id,
        job = %job_id,
        outcome = ?report.outcome,
        calls = report.calls,
        cells = report.cells,
        not_indexed = report.not_indexed,
        "evidence table fill run ended"
    );
    let Ok(store) = state.store().await else {
        return;
    };
    let (s, table, job) = (store, table_id.to_string(), job_id.to_string());
    let read = blocking(move || Ok((s.table(&table)?, s.fill_job(&job)?))).await;
    let (name, last_error) = match read {
        Ok((table, job)) => (
            table.map(|t| t.name).unwrap_or_default(),
            job.and_then(|j| j.last_error),
        ),
        Err(_) => (String::new(), None),
    };
    let end = TableRunEnd {
        outcome: &report.outcome,
        quota_window: report.quota_window,
        last_error: last_error.as_deref(),
        cap: state.inner.cap.get(),
    };
    if let Some(notice) = table_outcome(table_id, &name, end, *chrono::Local::now().offset()) {
        announce_table(app, notice);
    }
}

/// Runs every due job (see the module documentation).
async fn run_due(app: &AppHandle, state: &TableState) {
    let store = match state.store().await {
        Ok(store) => store,
        Err(e) => {
            tracing::debug!(error = %e.message, "evidence tables unavailable; no fill resumed");
            return;
        }
    };
    let due = match resume_due_jobs(store).await {
        Ok(due) => due,
        Err(e) => {
            tracing::warn!(error = %e, "due evidence table fills could not be read");
            return;
        }
    };
    if due.is_empty() {
        return;
    }
    let runner = match state.runner().await {
        Ok(runner) => {
            state.inner.blocked_logged.store(false, Ordering::SeqCst);
            runner
        }
        Err(e) => {
            // The jobs stay as they are and are taken up once a model can be used.
            if !state.inner.blocked_logged.swap(true, Ordering::SeqCst) {
                tracing::warn!(jobs = due.len(), error = %e.message, "evidence table fills wait for a usable model");
            }
            return;
        }
    };
    for job_id in due {
        spawn_run(app, state, runner.clone(), job_id);
    }
}

/// Starts the scheduler: once the model settings are loaded (so a job is never run, and
/// its calls counted, before the app knows its model and key), then every
/// [`SCHEDULER_TICK`].
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<TableState>().inner().clone();
        state.models_loaded().await;
        let mut tick = tokio::time::interval(SCHEDULER_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            run_due(&app, &state).await;
        }
    });
}

// ── Commands: tables ─────────────────────────────────────────────

#[tauri::command]
pub async fn tables_list(
    tables: State<'_, TableState>,
    workspace_id: String,
) -> TableResult<Vec<EvidenceTable>> {
    check_id("workspace", &workspace_id)?;
    let store = tables.store().await?;
    blocking(move || store.tables(&workspace_id)).await
}

#[tauri::command]
pub async fn table_create(
    app: AppHandle,
    tables: State<'_, TableState>,
    workspace_id: String,
    name: String,
) -> TableResult<EvidenceTable> {
    check_id("workspace", &workspace_id)?;
    let store = tables.store().await?;
    let table = blocking(move || store.create_table(&workspace_id, &name)).await?;
    emit_changed(&app, &table.id);
    Ok(table)
}

#[tauri::command]
pub async fn table_rename(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    name: String,
) -> TableResult<EvidenceTable> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let table = blocking(move || store.rename_table(&table_id, &name)).await?;
    emit_changed(&app, &table.id);
    Ok(table)
}

/// Deletes a table with its columns, rows, cells and jobs. A fill running on it stops
/// after its current call.
#[tauri::command]
pub async fn table_delete(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
) -> TableResult<bool> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let id = table_id.clone();
    let deleted = blocking(move || store.delete_table(&id)).await?;
    emit_changed(&app, &table_id);
    Ok(deleted)
}

#[tauri::command]
pub async fn table_get(tables: State<'_, TableState>, table_id: String) -> TableResult<TableView> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let (table, columns, rows, cells) = blocking(move || {
        let table = store
            .table(&table_id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No table {table_id}.")))?;
        Ok((
            table,
            store.columns(&table_id)?,
            store.rows(&table_id)?,
            store.cells(&table_id)?,
        ))
    })
    .await?;
    let graph = tables.inner.research.graph();
    let rows = rows
        .into_iter()
        .map(|row| {
            let title = graph
                .as_ref()
                .and_then(|g| g.paper_for_file(&row.document_id))
                .and_then(|p| p.title.clone())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| file_name(&row.document_id));
            RowView { row, title }
        })
        .collect();
    Ok(TableView {
        table,
        columns,
        rows,
        cells,
    })
}

// ── Commands: columns ────────────────────────────────────────────

/// The column, checked to belong to the table.
async fn column_of(
    store: &Arc<TableStore>,
    table_id: &str,
    column_id: &str,
) -> TableResult<Column> {
    let (s, id) = (store.clone(), column_id.to_string());
    let column = blocking(move || s.column(&id)).await?;
    match column {
        Some(column) if column.table_id == table_id => Ok(column),
        _ => Err(ResearchError::NotFound(format!("No column {column_id} in this table.")).into()),
    }
}

#[tauri::command]
pub async fn column_add(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    spec: ColumnSpec,
) -> TableResult<Column> {
    check_id("table", &table_id)?;
    let spec = check_spec(&spec)?;
    let store = tables.store().await?;
    let id = table_id.clone();
    let column = blocking(move || {
        let existing = store.columns(&id)?.len();
        check_column_room(existing).map_err(|e| ResearchError::Invalid(e.message))?;
        store.add_column(&id, &spec)
    })
    .await?;
    emit_changed(&app, &table_id);
    Ok(column)
}

#[tauri::command]
pub async fn column_update(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    column_id: String,
    spec: ColumnSpec,
) -> TableResult<Column> {
    check_id("table", &table_id)?;
    check_id("column", &column_id)?;
    let spec = check_spec(&spec)?;
    let store = tables.store().await?;
    column_of(&store, &table_id, &column_id).await?;
    let column = blocking(move || store.update_column(&column_id, &spec)).await?;
    emit_changed(&app, &table_id);
    Ok(column)
}

#[tauri::command]
pub async fn column_delete(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    column_id: String,
) -> TableResult<bool> {
    check_id("table", &table_id)?;
    check_id("column", &column_id)?;
    let store = tables.store().await?;
    column_of(&store, &table_id, &column_id).await?;
    let deleted = blocking(move || store.delete_column(&column_id)).await?;
    emit_changed(&app, &table_id);
    Ok(deleted)
}

/// Moves a column to `position` (0 is first). Returns the columns in their new order.
#[tauri::command]
pub async fn column_move(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    column_id: String,
    position: usize,
) -> TableResult<Vec<Column>> {
    check_id("table", &table_id)?;
    check_id("column", &column_id)?;
    check_position(position)?;
    let store = tables.store().await?;
    column_of(&store, &table_id, &column_id).await?;
    let columns = blocking(move || store.move_column(&column_id, position)).await?;
    emit_changed(&app, &table_id);
    Ok(columns)
}

// ── Commands: rows and cells ─────────────────────────────────────

/// Adds papers (file paths) as rows; a paper the table has is skipped. Returns the rows
/// added.
#[tauri::command]
pub async fn rows_add(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    paths: Vec<String>,
) -> TableResult<Vec<TableRow>> {
    check_id("table", &table_id)?;
    let paths = check_documents(&paths)?;
    let store = tables.store().await?;
    let id = table_id.clone();
    let rows = blocking(move || store.add_rows(&id, &paths)).await?;
    emit_changed(&app, &table_id);
    Ok(rows)
}

#[tauri::command]
pub async fn row_remove(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    row_id: String,
) -> TableResult<bool> {
    check_id("table", &table_id)?;
    check_id("row", &row_id)?;
    let store = tables.store().await?;
    let id = table_id.clone();
    let removed = blocking(move || {
        if !store.rows(&id)?.iter().any(|r| r.id == row_id) {
            return Err(ResearchError::NotFound(format!(
                "No row {row_id} in this table."
            )));
        }
        store.remove_row(&row_id)
    })
    .await?;
    emit_changed(&app, &table_id);
    Ok(removed)
}

/// The user's value for a cell, under its column's type rule (status `edited`).
#[tauri::command]
pub async fn cell_edit(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    row_id: String,
    column_id: String,
    value: String,
) -> TableResult<Cell> {
    check_id("table", &table_id)?;
    check_id("row", &row_id)?;
    check_id("column", &column_id)?;
    check_edit(&value)?;
    let store = tables.store().await?;
    column_of(&store, &table_id, &column_id).await?;
    let cell = blocking(move || store.edit_cell(&row_id, &column_id, &value)).await?;
    emit_changed(&app, &table_id);
    Ok(cell)
}

/// Empties a cell (an edit too); the next fill fills it.
#[tauri::command]
pub async fn cell_clear(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    row_id: String,
    column_id: String,
) -> TableResult<bool> {
    check_id("table", &table_id)?;
    check_id("row", &row_id)?;
    check_id("column", &column_id)?;
    let store = tables.store().await?;
    column_of(&store, &table_id, &column_id).await?;
    let cleared = blocking(move || store.clear_cell(&row_id, &column_id)).await?;
    emit_changed(&app, &table_id);
    Ok(cleared)
}

/// Asks the model again for one cell, over the top 8 passages of its column and past the
/// cache. Counts against the day's cap. A failed refill keeps the cell's result; an
/// edited cell is refused (clear it first).
#[tauri::command]
pub async fn cell_refill(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    row_id: String,
    column_id: String,
) -> TableResult<HandFill> {
    check_id("table", &table_id)?;
    check_id("row", &row_id)?;
    check_id("column", &column_id)?;
    let store = tables.store().await?;
    column_of(&store, &table_id, &column_id).await?;
    let request = tables
        .request(&store, &table_id, &row_id, Some(&column_id))
        .await?;
    let (model, _) = tables.model().await?;
    let filler = tables.filler(model).await?;
    let gate = tables.gate(tables.counter().await?);
    let fill = filler.refill_cell(&request, &column_id, &gate).await?;
    emit_changed(&app, &table_id);
    Ok(hand_fill(&fill))
}

// ── Commands: fill ───────────────────────────────────────────────

/// What filling the table would take (local retrieval and the cache; no model call).
#[tauri::command]
pub async fn fill_estimate(
    tables: State<'_, TableState>,
    table_id: String,
) -> TableResult<EstimateView> {
    check_id("table", &table_id)?;
    let runner = tables.runner().await?;
    let estimate = runner.estimate(&table_id).await?;
    let calls_today = runner.calls_today().await?;
    Ok(EstimateView {
        estimate,
        calls_today,
        cap: tables.inner.cap.get(),
        day_resets_at: now_ms(next_utc_midnight(Utc::now())),
    })
}

/// Starts filling the table (or takes up its paused or queued job) and runs it at once.
/// `planned_calls` is the estimate the user agreed to.
#[tauri::command]
pub async fn fill_start(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    planned_calls: usize,
) -> TableResult<JobView> {
    check_id("table", &table_id)?;
    check_planned_calls(planned_calls)?;
    let runner = tables.runner().await?;
    let job = runner.start(&table_id, planned_calls).await?;
    spawn_run(&app, &tables, runner, job.id.clone());
    emit_changed(&app, &table_id);
    Ok(job_view(&job))
}

/// Cancels the table's active fill: at once when it is queued or paused, after the
/// current call when it runs. Finished cells stay. Needs no model.
#[tauri::command]
pub async fn fill_cancel(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
) -> TableResult<Option<JobView>> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let (s, id) = (store.clone(), table_id.clone());
    let Some(active) = blocking(move || s.active_fill_job(&id)).await? else {
        return Ok(None);
    };
    let job = cancel_job(store, &active.id).await?;
    emit_changed(&app, &table_id);
    Ok(job.as_ref().map(job_view))
}

/// The table's fill and the day's calls. Needs no model.
#[tauri::command]
pub async fn fill_status(
    tables: State<'_, TableState>,
    table_id: String,
) -> TableResult<FillStatus> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let (job, rows, cells) = blocking(move || {
        Ok((
            store.latest_fill_job(&table_id)?,
            store.rows(&table_id)?,
            store.cells(&table_id)?,
        ))
    })
    .await?;
    Ok(FillStatus {
        job: job.as_ref().map(job_view),
        papers_done: papers_done(&rows, &cells),
        papers_total: rows.len(),
        calls_today: tables.calls_today().await?,
        cap: tables.inner.cap.get(),
        day_resets_at: now_ms(next_utc_midnight(Utc::now())),
    })
}

/// Fills one paper by hand with the configured model, counted against the day's cap, and
/// keeps every model call it made byte for byte: they are returned and written to
/// `<app data>/tables/captures/<utc time>-<table>-<row>.json` (on this computer only), so
/// replies of real models can be checked and kept as test fixtures. The fill is the
/// normal one: cells that are current, or answered by the cache, cost no call.
#[tauri::command]
pub async fn table_fill_paper(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    row_id: String,
) -> TableResult<PaperCapture> {
    check_id("table", &table_id)?;
    check_id("row", &row_id)?;
    let store = tables.store().await?;
    let request = tables.request(&store, &table_id, &row_id, None).await?;
    let (model, rule) = tables.model().await?;
    let recording = Arc::new(RecordingModel::new(model));
    let filler = tables.filler(recording.clone()).await?;
    let gate = tables.gate(tables.counter().await?);
    let fill = filler.fill_paper(&request, &gate).await?;
    emit_changed(&app, &table_id);
    let at = Utc::now();
    let mut capture = PaperCapture {
        captured_at: at.to_rfc3339_opts(SecondsFormat::Millis, true),
        table_id: table_id.clone(),
        row_id: row_id.clone(),
        document_id: request.row.document_id.clone(),
        model: recording.model_id(),
        reasoning_rule: rule.describe(),
        calls: recording.calls(),
        result: hand_fill(&fill),
        file: None,
    };
    let path = capture_path(
        &tables.inner.data_dir.join(CAPTURES_DIR),
        at,
        &table_id,
        &row_id,
    );
    let written = capture.clone();
    let target = path.clone();
    match tokio::task::spawn_blocking(move || write_capture(&target, &written)).await {
        Ok(Ok(())) => capture.file = Some(path.display().to_string()),
        Ok(Err(e)) => tracing::warn!(error = %e, "the table fill capture could not be written"),
        Err(e) => tracing::warn!(error = %e, "the table fill capture task failed"),
    }
    Ok(capture)
}

// ── Commands: settings ───────────────────────────────────────────

#[tauri::command]
pub async fn tables_settings_get(tables: State<'_, TableState>) -> TableResult<TablesSettings> {
    Ok(settings_view(tables.inner.cap.get()))
}

/// Sets the day's cap of table calls. User-only. It applies from the next check of
/// every fill, also one in flight.
#[tauri::command]
pub async fn tables_settings_set(
    app: AppHandle,
    tables: State<'_, TableState>,
    audit: State<'_, AuditState>,
    daily_call_cap: usize,
) -> TableResult<TablesSettings> {
    check_daily_cap(daily_call_cap)?;
    let store = SettingsStore::in_dir(&tables.inner.data_dir);
    let (settings, before) = tokio::task::spawn_blocking(move || {
        store.update(|s| {
            let before = s.tables;
            s.tables.daily_call_cap = daily_call_cap;
            Ok(before)
        })
    })
    .await
    .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?
    .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?;
    tables.inner.cap.set(daily_call_cap)?;
    if before != settings.tables {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({
                "action": "tables_preferences_change",
                "old": before,
                "new": settings.tables,
                "via": "ui",
            }),
        ));
    }
    broadcast(&app, &settings);
    Ok(settings_view(tables.inner.cap.get()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::research::tables::calls::DEFAULT_DAILY_CAP;
    use shodh_rag::research::tables::{CellValue, MAX_OPTIONS, MIN_OPTIONS};

    fn spec(kind: ColumnKind, options: usize) -> ColumnSpec {
        ColumnSpec {
            question: "  What   dataset? ".to_string(),
            kind,
            options: (0..options).map(|i| format!("Option {i}")).collect(),
            instructions: String::new(),
        }
    }

    #[test]
    fn ids_are_short_plain_tokens() {
        assert!(check_id("table", "0b6f3c2e-9d1a-4e0b-8a1c-2f3d4e5f6a7b").is_ok());
        assert!(check_id("table", "t_1").is_ok());
        let long = "a".repeat(MAX_ID_CHARS + 1);
        for bad in ["", " ", "a b", "../x", "x;y", "é", long.as_str()] {
            let e = check_id("table", bad).unwrap_err();
            assert_eq!(e.code, "invalid", "{bad:?}");
            assert!(e.message.contains("table id"));
        }
    }

    #[test]
    fn a_pick_list_needs_2_to_12_options_and_others_none() {
        assert!(check_spec(&spec(ColumnKind::Choice, MIN_OPTIONS - 1)).is_err());
        assert!(check_spec(&spec(ColumnKind::Choice, MIN_OPTIONS)).is_ok());
        assert!(check_spec(&spec(ColumnKind::Choice, MAX_OPTIONS)).is_ok());
        let too_many = check_spec(&spec(ColumnKind::Choice, MAX_OPTIONS + 1)).unwrap_err();
        assert_eq!(too_many.code, "invalid");
        assert!(check_spec(&spec(ColumnKind::Text, 2)).is_err());
        let cleaned = check_spec(&spec(ColumnKind::Number, 0)).unwrap();
        assert_eq!(cleaned.question, "What dataset?");
        let mut empty = spec(ColumnKind::Text, 0);
        empty.question = "   ".to_string();
        assert!(check_spec(&empty).is_err());
    }

    #[test]
    fn columns_and_their_places_stay_within_the_table_limit() {
        assert!(check_column_room(0).is_ok());
        assert!(check_column_room(MAX_COLUMNS - 1).is_ok());
        assert_eq!(check_column_room(MAX_COLUMNS).unwrap_err().code, "invalid");
        assert!(check_position(0).is_ok());
        assert!(check_position(MAX_COLUMNS - 1).is_ok());
        assert!(check_position(MAX_COLUMNS).is_err());
    }

    #[test]
    fn papers_are_real_paths_in_bounded_numbers() {
        assert_eq!(
            check_documents(&["  /p/a.pdf ".to_string(), "C:\\p\\b.pdf".to_string()]).unwrap(),
            vec!["/p/a.pdf".to_string(), "C:\\p\\b.pdf".to_string()]
        );
        assert!(check_documents(&[]).is_err());
        assert!(check_documents(&["   ".to_string()]).is_err());
        assert!(check_documents(&["/p/a\u{0}.pdf".to_string()]).is_err());
        assert!(check_documents(&["x".repeat(MAX_PATH_CHARS + 1)]).is_err());
        let many = vec!["/p/a.pdf".to_string(); MAX_ROWS_PER_ADD + 1];
        assert!(check_documents(&many).is_err());
        assert!(check_documents(&many[..MAX_ROWS_PER_ADD]).is_ok());
    }

    #[test]
    fn the_cap_edits_and_planned_calls_are_bounded() {
        assert!(check_daily_cap(DEFAULT_DAILY_CAP).is_ok());
        assert!(check_daily_cap(MIN_DAILY_CAP).is_ok());
        assert!(check_daily_cap(MAX_DAILY_CAP).is_ok());
        assert_eq!(
            check_daily_cap(MIN_DAILY_CAP - 1).unwrap_err().code,
            "invalid"
        );
        assert!(check_daily_cap(MAX_DAILY_CAP + 1).is_err());
        assert!(check_edit(&"x".repeat(MAX_EDIT_CHARS)).is_ok());
        assert!(check_edit(&"x".repeat(MAX_EDIT_CHARS + 1)).is_err());
        assert!(check_planned_calls(0).is_ok());
        assert!(check_planned_calls(MAX_PLANNED_CALLS + 1).is_err());
        assert_eq!(DailyCap::default().get(), DEFAULT_DAILY_CAP);
    }

    fn row(id: &str) -> TableRow {
        TableRow {
            id: id.to_string(),
            table_id: "t".to_string(),
            position: 0,
            document_id: format!("/p/{id}.pdf"),
            added_at: "2026-10-22T09:00:00.000Z".to_string(),
        }
    }

    fn cell(row: &str, column: &str, status: CellStatus) -> Cell {
        Cell {
            table_id: "t".to_string(),
            row_id: row.to_string(),
            column_id: column.to_string(),
            status,
            value: (status == CellStatus::Verified).then(|| CellValue::YesNo { yes: true }),
            quote: None,
            chunk_id: None,
            page: None,
            bboxes: None,
            model: None,
            column_revision: Some(1),
            paper_hash: None,
            filled_at: None,
            edited_at: None,
            error: None,
        }
    }

    #[test]
    fn a_paper_is_done_when_none_of_its_cells_waits_in_the_queue() {
        let rows = [row("a"), row("b"), row("c")];
        let cells = [
            cell("a", "k1", CellStatus::Verified),
            cell("a", "k2", CellStatus::NotFound),
            cell("b", "k1", CellStatus::Verified),
            cell("b", "k2", CellStatus::Queued),
        ];
        assert_eq!(papers_done(&rows, &cells), 2, "a, and c with no cells yet");
        assert_eq!(papers_done(&rows, &[]), 3);
    }

    #[test]
    fn a_job_shows_its_next_attempt_as_utc_milliseconds() {
        let job = FillJob {
            id: "j".to_string(),
            table_id: "t".to_string(),
            state: JobState::Paused,
            planned_calls: 12,
            done_calls: 5,
            created_at: "2026-10-22T09:00:00.000Z".to_string(),
            next_attempt_at: Some("2026-10-23T00:00:00.000Z".to_string()),
            pause_reason: Some(PauseReason::DailyCap),
            last_error: None,
            cancel_requested: false,
            heartbeat_at: None,
            backoff_step: 0,
            failure_streak: 0,
            panics: 0,
        };
        let view = job_view(&job);
        assert_eq!(view.next_attempt_at, Some(1_792_713_600_000));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["nextAttemptAt"], 1_792_713_600_000_i64);
        assert_eq!(json["pauseReason"], "daily_cap");
        assert_eq!(json["state"], "paused");
        assert_eq!(json["doneCalls"], 5);
        let waiting = FillJob {
            next_attempt_at: None,
            ..job
        };
        assert_eq!(job_view(&waiting).next_attempt_at, None);
    }

    #[test]
    fn a_capture_keeps_the_reply_byte_for_byte_and_names_its_file_by_time_table_and_row() {
        let reply = "<think>x</think>\r\n{\"cells\":[{\"column\":\"c1\",\"answer\":null}]}  ";
        let call = captured(
            "the prompt",
            840,
            &Ok(ModelReply {
                text: reply.to_string(),
                truncated: false,
            }),
        );
        assert_eq!(call.reply.as_deref(), Some(reply));
        assert_eq!(call.parsed_cells, Some(1));
        assert_eq!(call.cut_off, Some(false));
        assert_eq!(call.prompt_chars, 10);
        assert_eq!(call.prompt_sha256.len(), 64);
        let cut = captured(
            "p",
            840,
            &Ok(ModelReply {
                text: "<think>never closed".to_string(),
                truncated: true,
            }),
        );
        assert!(cut.truncated);
        assert_eq!(cut.parsed_cells, None);
        let failed = captured("p", 840, &Err("HTTP 429".to_string()));
        assert_eq!(failed.error.as_deref(), Some("HTTP 429"));
        assert_eq!(failed.reply, None);

        use chrono::TimeZone;
        let at = Utc.with_ymd_and_hms(2026, 10, 22, 9, 30, 5).unwrap();
        let path = capture_path(Path::new("/data/tables/captures"), at, "t1", "r1");
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "20261022T093005.000Z-t1-r1.json"
        );
    }

    #[test]
    fn a_capture_file_holds_what_the_command_returns() {
        let dir = tempfile::tempdir().unwrap();
        let capture = PaperCapture {
            captured_at: "2026-10-22T09:30:05.000Z".to_string(),
            table_id: "t1".to_string(),
            row_id: "r1".to_string(),
            document_id: "/p/a.pdf".to_string(),
            model: "test/model:free".to_string(),
            reasoning_rule: ReasoningRule::Plain.describe(),
            calls: vec![captured(
                "p",
                840,
                &Ok(ModelReply {
                    text: "{\"cells\":[]}".to_string(),
                    truncated: false,
                }),
            )],
            result: HandFill {
                cells: Vec::new(),
                calls: 1,
                cache_hits: 0,
                halted: None,
                model_error: None,
                paper_error: None,
            },
            file: None,
        };
        let path = dir.path().join(CAPTURES_DIR).join("c.json");
        write_capture(&path, &capture).unwrap();
        let read: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read["calls"][0]["reply"], "{\"cells\":[]}");
        assert_eq!(read["reasoningRule"], "no reasoning; plain reply budget");
        assert_eq!(read["result"]["calls"], 1);
    }
}
