//! Evidence tables: the managed state, the commands of the Tables tab, the fill scheduler
//! and the notifications of a fill's end (see `shodh_rag::research::tables`).
//!
//! - **Model.** A run fills with the model answers come from
//!   ([`crate::memory_learn::configured_table_model`]), resolved when the run starts: a
//!   change of model applies from the next run, and a run in flight finishes with the
//!   model it started with. The API key is read at every call, so a corrected key applies
//!   from the next call. No model set, Local-only mode against a cloud model, no API key,
//!   or a model whose reasoning cannot be bounded is refused with its code
//!   (`model_refused` for the last, `model_unavailable` for the others).
//! - **Daily cap.** One shared [`DailyCap`] (Settings → Tables, `tables.dailyCallCap`) is
//!   read by every run and hand fill at every check, so a change applies from the next
//!   paper. Hand fills (`table_fill_paper`, `cell_refill`) count against it too. Raising
//!   the cap wakes the jobs paused for it.
//! - **Scheduler** ([`Scheduler`]). Once the app has loaded its model settings, and then
//!   every [`SCHEDULER_TICK`], the due jobs are run; a job that cannot get a model is
//!   paused (`model_unavailable`) and the user told once, and each check looks again, so
//!   fixing the model, the key or the setting lets it go on within a minute.
//!   `fill_start` runs its job at once and says honestly when it could not.
//! - **Exit.** When the app exits, the jobs its runs hold are given back (queued, no run
//!   token), so the next start takes them up at once instead of waiting out their lease.
//! - **Hand fills** run one per row at a time, and never on a table whose fill is running,
//!   so a double click or a refill during a fill cannot pay twice.
//! - **Captures** (`table_fill_paper`, a diagnostics setting) are kept in `shodh.db`, a
//!   few per row, and go with their row, table, workspace or forgotten paper; they leave
//!   the database only through `table_captures_export`.
//! - **Events.** Every change emits [`TABLE_CHANGED_EVENT`] with `{ "tableId" }`, and so
//!   does the end of every run, whatever its outcome; the Tables tab polls `fill_status`
//!   while a job is active.
//! - **Notifications.** Only what needs the user ([`crate::notifications::table_outcome`]);
//!   the cap and the provider's daily quota at most once a UTC day across all tables.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
    cancel_job, job_hold, release_held_jobs, wake_paused_jobs, FillEstimate, FillJob, Hold,
    JobOutcome, JobRunner, JobState, PauseReason, ResultSource, RunReport,
};
use shodh_rag::research::tables::parse::{parse_cut_reply, parse_reply};
use shodh_rag::research::tables::passages::PaperIndex;
use shodh_rag::research::tables::reasoning::{ReasoningCatalog, ReasoningRule};
use shodh_rag::research::tables::schedule::{RunEvents, RunnerError, RunnerSource, Scheduler};
use shodh_rag::research::tables::{
    Cell, CellStatus, Column, ColumnKind, ColumnSpec, EvidenceTable, TableRow, TableStore,
    TableStoreHandle, MAX_COLUMNS,
};
use shodh_rag::research::{file_name, path_key, ResearchError, ResearchResult};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{watch, OnceCell, RwLock as AsyncRwLock};

use crate::answer_check_commands::SharedNli;
use crate::app_settings::{broadcast, SettingsStore};
use crate::audit_commands::AuditState;
use crate::llm_commands::{ApiKeys, LLMState};
use crate::memory_learn::TableModelError;
use crate::notifications::{
    announce_table, once_a_day_key, table_outcome, OncePerDay, TableRunEnd,
};
use crate::research_commands::{ResearchCommandError, ResearchCommandResult, ResearchState};

/// Emitted with `{ "tableId": … }` after anything in a table changed, and after every
/// run of its fill ends.
pub const TABLE_CHANGED_EVENT: &str = "table_changed";
/// How often the scheduler looks for due jobs (and at jobs waiting for a model).
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
/// The least a paper can cost: its call and the one retry. With fewer calls left today a
/// fill cannot start.
pub const LEAST_PAPER_CALLS: usize = 2;

/// A command error with its own code (see [`ResearchCommandError`]).
fn coded(code: &'static str, message: impl Into<String>) -> ResearchCommandError {
    ResearchCommandError {
        code,
        message: message.into(),
    }
}

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

/// The command error of a model tables cannot fill with: `model_refused` for a model
/// whose reasoning cannot be bounded (choose another one), `model_unavailable` for no
/// model, Local-only mode or a missing key, `unavailable` for unreadable settings.
pub fn model_error(error: &TableModelError) -> ResearchCommandError {
    let code = match error {
        TableModelError::RefusedReasoning(_) => "model_refused",
        TableModelError::Settings(_) => "unavailable",
        TableModelError::NoModel | TableModelError::LocalOnly | TableModelError::NoKey => {
            "model_unavailable"
        }
    };
    coded(code, error.message())
}

/// Why a fill by hand may not start now: the table's fill is running (`fill_running`).
pub fn hand_fill_refusal(active: Option<&FillJob>) -> Option<ResearchCommandError> {
    active
        .filter(|job| job.state == JobState::Running)
        .map(|_| {
            coded(
                "fill_running",
                "This table is being filled. Wait for the fill to end, or cancel it, before filling by hand.",
            )
        })
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
    /// The code of what paused or stopped the job (for a search failure, with detail;
    /// for `model_unavailable`, which: `refused_reasoning`, `local_only`, `no_key`,
    /// `no_model`).
    pub last_error: Option<String>,
    /// When a paused job goes on by itself (UTC, Unix milliseconds); none while it waits
    /// for the user.
    pub next_attempt_at: Option<i64>,
    /// The running run's last heartbeat (UTC, Unix milliseconds): a `running` job whose
    /// heartbeat is minutes old is held by a run that is gone.
    pub heartbeat_at: Option<i64>,
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
        heartbeat_at: job.heartbeat_at.as_deref().and_then(utc_ms),
        cancel_requested: job.cancel_requested,
        created_at: job.created_at.clone(),
    }
}

/// Whether a cell holds the end of a fill (an answer, a not-found, a failure) or an edit.
fn settled(status: CellStatus) -> bool {
    matches!(
        status,
        CellStatus::Verified
            | CellStatus::Unverified
            | CellStatus::NotFound
            | CellStatus::Edited
            | CellStatus::Failed
    )
}

/// The papers `job` filled: rows whose every column holds a settled cell, at least one
/// of them written since the job was created. None without a job; a paper that was
/// already filled before the job, or not reached before a cancel or a failure, is not
/// counted.
pub fn papers_done(
    job: Option<&FillJob>,
    rows: &[TableRow],
    columns: &[Column],
    cells: &[Cell],
) -> usize {
    let Some(job) = job else {
        return 0;
    };
    if columns.is_empty() {
        return 0;
    }
    let since = job.created_at.as_str();
    rows.iter()
        .filter(|row| {
            let row_cells: Vec<&Cell> = cells.iter().filter(|c| c.row_id == row.id).collect();
            let complete = columns.iter().all(|column| {
                row_cells
                    .iter()
                    .any(|c| c.column_id == column.id && settled(c.status))
            });
            let touched = row_cells.iter().any(|c| {
                c.filled_at.as_deref().is_some_and(|at| at >= since)
                    || c.edited_at.as_deref().is_some_and(|at| at >= since)
            });
            complete && touched
        })
        .count()
}

/// The table's fill and the day's calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillStatus {
    /// The table's newest job (active, or how the last fill ended).
    pub job: Option<JobView>,
    /// Papers that job filled ([`papers_done`]).
    pub papers_done: usize,
    pub papers_total: usize,
    /// Table calls made today (UTC day), by every table.
    pub calls_today: usize,
    pub cap: usize,
    /// When the day's count starts again: the next 00:00 UTC (Unix milliseconds); the
    /// interface shows it in local time.
    pub day_resets_at: i64,
    /// Who runs the job while it is `running` ([`run_holder`]): `here`, `elsewhere`
    /// (another run of Shodh, under a live lease) or `stopped` (no run holds it: the
    /// scheduler takes it up at its next check). None for a job that is not running.
    pub run: Option<&'static str>,
    /// For a job held `elsewhere`: when that run's lease ends at the latest, after which
    /// the fill goes on here (UTC, Unix milliseconds).
    pub lease_expires_at: Option<i64>,
}

/// Who runs a `running` job, for the status line: this app (`here`), another run of
/// Shodh under a live lease (`elsewhere`, with the lease's end), or nobody (`stopped`: the
/// run that held it is gone and the scheduler takes it up). None when the job is not
/// running.
pub fn run_holder(
    state: Option<JobState>,
    hold: Hold,
    running_here: bool,
) -> (Option<&'static str>, Option<i64>) {
    if state != Some(JobState::Running) {
        return (None, None);
    }
    match hold {
        Hold::Elsewhere { until } => (Some("elsewhere"), Some(now_ms(until))),
        Hold::RunningHere => (Some("here"), None),
        Hold::Free if running_here => (Some("here"), None),
        Hold::Free => (Some("stopped"), None),
    }
}

/// What `fill_start` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillStart {
    pub job: JobView,
    /// `started`; `already_running` (this app is filling it); `held_elsewhere` (an
    /// earlier run of Shodh still holds it, until `leaseExpiresAt` at the latest, when it
    /// goes on by itself); `at_cap` (today's calls are used up: it goes on at
    /// `dayResetsAt`).
    pub start: &'static str,
    pub lease_expires_at: Option<i64>,
    pub day_resets_at: i64,
}

/// How a fill the user starts goes: held by another run, running here, stopped by the
/// day's cap before its first paper, or started.
pub fn start_state(hold: Hold, running_here: bool, calls_today: usize, cap: usize) -> &'static str {
    match hold {
        Hold::Elsewhere { .. } => "held_elsewhere",
        Hold::RunningHere => "already_running",
        Hold::Free if running_here => "already_running",
        Hold::Free if calls_today.saturating_add(LEAST_PAPER_CALLS) > cap => "at_cap",
        Hold::Free => "started",
    }
}

/// What `fill_cancel` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillCancel {
    /// The job as it now is.
    pub job: Option<JobView>,
    /// The cancel is asked and not done yet: the job runs (here, after its current call)
    /// or is held by an earlier run of Shodh (by `leaseExpiresAt` at the latest).
    pub pending: bool,
    pub lease_expires_at: Option<i64>,
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

/// A row with the paper's title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    #[serde(flatten)]
    pub row: TableRow,
    pub title: String,
}

/// A paper's title: the citation graph's when it knows the paper, else the file name as
/// the file system spelled it at indexing, else the name in the row's key (which is
/// lower-cased on Windows). Both maps are keyed by `path_key`.
pub fn row_title(
    document_id: &str,
    graph_titles: &HashMap<String, String>,
    file_names: &HashMap<String, String>,
) -> String {
    graph_titles
        .get(document_id)
        .or_else(|| file_names.get(document_id))
        .cloned()
        .unwrap_or_else(|| file_name(document_id))
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
    /// `table_fill_paper` (the capture of a fill's model calls) is allowed.
    pub diagnostics: bool,
}

fn settings_view(cap: usize, diagnostics: bool) -> TablesSettings {
    TablesSettings {
        daily_call_cap: cap,
        min_daily_call_cap: MIN_DAILY_CAP,
        max_daily_call_cap: MAX_DAILY_CAP,
        diagnostics,
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

/// The rows with a fill by hand running, one at a time per row.
#[derive(Debug, Clone, Default)]
pub struct HandFills {
    rows: Arc<Mutex<HashSet<String>>>,
}

/// A row's hold on hand fills, let go when dropped.
#[derive(Debug)]
pub struct HandFillHold {
    rows: Arc<Mutex<HashSet<String>>>,
    row_id: String,
}

impl Drop for HandFillHold {
    fn drop(&mut self) {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.row_id);
    }
}

impl HandFills {
    /// Holds `row_id` for a fill by hand, or `None` when one runs on it.
    pub fn try_hold(&self, row_id: &str) -> Option<HandFillHold> {
        let held = self
            .rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(row_id.to_string());
        held.then(|| HandFillHold {
            rows: self.rows.clone(),
            row_id: row_id.to_string(),
        })
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

/// A model that keeps every call it passes on, for `table_fill_paper`. It changes nothing
/// about the calls.
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

/// What `table_fill_paper` returns and keeps (in `shodh.db`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperCapture {
    pub captured_at: String,
    pub table_id: String,
    pub row_id: String,
    pub document_id: String,
    pub model: String,
    pub reasoning_rule: String,
    /// Every model call the fill made, also when it then failed.
    pub calls: Vec<CapturedCall>,
    /// What the fill did, when it ended.
    pub result: Option<HandFill>,
    /// What stopped the fill, when it failed after (or before) its calls.
    pub error: Option<String>,
    /// The kept capture's id; none when it could not be kept.
    pub capture_id: Option<String>,
}

/// The capture of a fill by hand from its calls and how it ended: kept whole when the
/// fill failed after spending calls.
#[allow(clippy::too_many_arguments)]
pub fn paper_capture(
    at: DateTime<Utc>,
    table_id: &str,
    row_id: &str,
    document_id: &str,
    model: String,
    rule: &ReasoningRule,
    calls: Vec<CapturedCall>,
    ended: Result<&PaperFill, String>,
) -> PaperCapture {
    let (result, error) = match ended {
        Ok(fill) => (Some(hand_fill(fill)), None),
        Err(e) => (None, Some(e)),
    };
    PaperCapture {
        captured_at: at.to_rfc3339_opts(SecondsFormat::Millis, true),
        table_id: table_id.to_string(),
        row_id: row_id.to_string(),
        document_id: document_id.to_string(),
        model,
        reasoning_rule: rule.describe(),
        calls,
        result,
        error,
        capture_id: None,
    }
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
    scheduler: Arc<Scheduler>,
    hand_fills: HandFills,
    /// The cap and daily-quota notices shown today.
    once_a_day: Mutex<OncePerDay>,
    /// Becomes true once the app has loaded its model settings and keys.
    models_loaded: watch::Receiver<bool>,
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
                scheduler: Scheduler::new(),
                hand_fills: HandFills::default(),
                once_a_day: Mutex::new(OncePerDay::default()),
                models_loaded: parts.models_loaded,
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
    async fn table_model(&self) -> Result<(Arc<dyn TextModel>, ReasoningRule), TableModelError> {
        crate::memory_learn::configured_table_model(
            &self.inner.data_dir,
            &self.llm(),
            &self.inner.catalog,
        )
        .await
    }

    /// [`Self::table_model`] for a command.
    async fn model(&self) -> TableResult<(Arc<dyn TextModel>, ReasoningRule)> {
        self.table_model().await.map_err(|e| model_error(&e))
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

    async fn runner_with(&self, model: Arc<dyn TextModel>) -> TableResult<JobRunner> {
        Ok(JobRunner::new(
            self.filler(model).await?,
            self.counter().await?,
            self.results(),
            self.inner.cap.clone(),
        ))
    }

    async fn runner(&self) -> TableResult<JobRunner> {
        let (model, _) = self.model().await?;
        self.runner_with(model).await
    }

    fn results(&self) -> Arc<dyn ResultSource> {
        Arc::new(ServiceResults(self.inner.research.clone()))
    }

    fn gate(&self, counter: Arc<CallCounter>) -> shodh_rag::research::tables::jobs::CounterGate {
        shodh_rag::research::tables::jobs::CounterGate::new(counter, self.inner.cap.clone())
    }

    fn events(&self, app: &AppHandle) -> Arc<dyn RunEvents> {
        Arc::new(AppEvents {
            app: app.clone(),
            state: self.clone(),
        })
    }

    /// One check of the scheduler now.
    async fn check(&self, app: &AppHandle) {
        let store = match self.store().await {
            Ok(store) => store,
            Err(e) => {
                tracing::debug!(error = %e.message, "evidence tables unavailable; no fill resumed");
                return;
            }
        };
        if let Err(e) = self
            .inner
            .scheduler
            .tick(store, self, self.events(app))
            .await
        {
            tracing::warn!(error = %e, "evidence table fills could not be checked");
        }
    }

    /// A check of the scheduler on a task of its own (after a setting changed).
    fn check_soon(&self, app: &AppHandle) {
        let (state, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn(async move { state.check(&app).await });
    }

    /// Holds the row for a fill by hand: refused while the table's fill runs
    /// (`fill_running`) or another fill by hand runs on the row (`hand_fill_running`).
    async fn hold_hand_fill(
        &self,
        store: &Arc<TableStore>,
        table_id: &str,
        row_id: &str,
    ) -> TableResult<HandFillHold> {
        let (s, table) = (store.clone(), table_id.to_string());
        let active = blocking(move || s.active_fill_job(&table)).await?;
        if let Some(refusal) = hand_fill_refusal(active.as_ref()) {
            return Err(refusal);
        }
        self.inner.hand_fills.try_hold(row_id).ok_or_else(|| {
            coded(
                "hand_fill_running",
                "This paper is being filled by hand already.",
            )
        })
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

    /// The file names of the table's papers as the file system spelled them at indexing,
    /// by `path_key`.
    async fn file_names(&self, rows: &[TableRow]) -> HashMap<String, String> {
        let wanted: HashSet<&str> = rows.iter().map(|r| r.document_id.as_str()).collect();
        let engine = self.inner.rag.read().await;
        let (sources, names) = match (
            engine.document_sources().await,
            engine.document_names().await,
        ) {
            (Ok(sources), Ok(names)) => (sources, names),
            (Err(e), _) | (_, Err(e)) => {
                tracing::debug!(error = %format!("{e:#}"), "indexed file names not read; titles use the row keys");
                return HashMap::new();
            }
        };
        drop(engine);
        sources
            .into_iter()
            .filter_map(|row| {
                let key = path_key(&row.source);
                if !wanted.contains(key.as_str()) {
                    return None;
                }
                let names = names.get(&row.doc_id)?;
                let name = names
                    .display_path
                    .as_deref()
                    .map(file_name)
                    .or_else(|| names.filename.clone())?;
                Some((key, name))
            })
            .collect()
    }
}

/// The scheduler's runner: the model tables use now. A model that cannot be used is a
/// block (the job waits for the user); anything else is a failure.
#[async_trait::async_trait]
impl RunnerSource for TableState {
    async fn runner(&self) -> Result<JobRunner, RunnerError> {
        let model = match self.table_model().await {
            Ok((model, _)) => model,
            Err(TableModelError::Settings(e)) => return Err(RunnerError::Unavailable(e)),
            Err(e) => {
                return Err(RunnerError::Blocked {
                    code: e.code(),
                    message: e.message(),
                })
            }
        };
        self.runner_with(model)
            .await
            .map_err(|e| RunnerError::Unavailable(e.message))
    }
}

/// The scheduler's events in the app: `table_changed`, and the notifications.
struct AppEvents {
    app: AppHandle,
    state: TableState,
}

impl RunEvents for AppEvents {
    fn changed(&self, table_id: &str) {
        emit_changed(&self.app, table_id);
    }

    fn ended(&self, job: &FillJob, report: &RunReport, by_user: bool) {
        let (app, state, job, report) = (
            self.app.clone(),
            self.state.clone(),
            job.clone(),
            report.clone(),
        );
        tauri::async_runtime::spawn(async move {
            tracing::info!(
                table = %job.table_id,
                job = %job.id,
                outcome = ?report.outcome,
                calls = report.calls,
                papers = report.papers,
                not_indexed = report.not_indexed,
                "evidence table fill run ended"
            );
            announce_end(&app, &state, &job, &report.outcome, &report, by_user).await;
        });
    }

    fn blocked(&self, job: &FillJob, message: &str) {
        tracing::warn!(table = %job.table_id, job = %job.id, reason = %message, "evidence table fill waits for a usable model");
        let (app, state, job) = (self.app.clone(), self.state.clone(), job.clone());
        tauri::async_runtime::spawn(async move {
            let outcome = JobOutcome::Paused {
                reason: PauseReason::ModelUnavailable,
                until: None,
            };
            let report = RunReport {
                outcome: outcome.clone(),
                calls: 0,
                cells: 0,
                papers: 0,
                not_indexed: 0,
                quota_window: None,
            };
            announce_end(&app, &state, &job, &outcome, &report, false).await;
        });
    }
}

/// Tells what needs the user about the end of `job`'s run (see
/// [`crate::notifications::table_outcome`]); the cap and the daily quota once a UTC day.
async fn announce_end(
    app: &AppHandle,
    state: &TableState,
    job: &FillJob,
    outcome: &JobOutcome,
    report: &RunReport,
    by_user: bool,
) {
    let Ok(store) = state.store().await else {
        return;
    };
    let (s, table, id) = (store, job.table_id.clone(), job.id.clone());
    let read = blocking(move || Ok((s.table(&table)?, s.fill_job(&id)?))).await;
    let (name, last_error) = match read {
        Ok((table, job)) => (
            table.map(|t| t.name).unwrap_or_default(),
            job.and_then(|j| j.last_error),
        ),
        Err(_) => (String::new(), None),
    };
    let end = TableRunEnd {
        outcome,
        quota_window: report.quota_window,
        last_error: last_error.as_deref(),
        cap: state.inner.cap.get(),
        papers: report.papers,
        not_indexed: report.not_indexed,
        calls: report.calls,
        by_user,
    };
    let Some(notice) = table_outcome(&job.table_id, &name, end, *chrono::Local::now().offset())
    else {
        return;
    };
    if let Some(key) = once_a_day_key(&end) {
        let admitted = state
            .inner
            .once_a_day
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(key, utc_day(Utc::now()));
        if !admitted {
            return;
        }
    }
    announce_table(app, notice);
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

// ── The scheduler and exit ───────────────────────────────────────

/// Waits until the app has loaded its model settings and keys (`loaded` turns true), or
/// until its sender is gone (the loading task ended without saying so).
pub async fn models_loaded(mut loaded: watch::Receiver<bool>) {
    let _ = loaded.wait_for(|done| *done).await;
}

/// Starts the scheduler: once the model settings are loaded (so a job is never run, and
/// its calls counted, before the app knows its model and key), then every
/// [`SCHEDULER_TICK`].
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<TableState>().inner().clone();
        models_loaded(state.inner.models_loaded.clone()).await;
        let mut tick = tokio::time::interval(SCHEDULER_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            state.check(&app).await;
        }
    });
}

/// Gives back the jobs this process's runs hold, when the app exits: the next start
/// takes them up at once.
pub fn release_on_exit(app: &AppHandle) {
    let Some(state) = app.try_state::<TableState>() else {
        return;
    };
    let state = state.inner().clone();
    tauri::async_runtime::block_on(async move {
        let Ok(store) = state.store().await else {
            return;
        };
        match release_held_jobs(store).await {
            Ok(0) => {}
            Ok(released) => tracing::info!(
                jobs = released,
                "evidence table fills given back for the next start"
            ),
            Err(e) => {
                tracing::warn!(error = %e, "evidence table fills could not be given back; they resume once their lease runs out")
            }
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

/// Deletes a table with its columns, rows, cells, captures and jobs. A fill running on it
/// stops after its current call.
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
    let graph_titles: HashMap<String, String> = tables
        .inner
        .research
        .graph()
        .map(|graph| {
            graph
                .papers()
                .iter()
                .filter(|p| p.in_library)
                .filter_map(|p| {
                    let title = p.title.as_deref()?.trim();
                    let file = p.file_path.as_deref()?;
                    (!title.is_empty()).then(|| (path_key(file), title.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let file_names = tables.file_names(&rows).await;
    let rows = rows
        .into_iter()
        .map(|row| {
            let title = row_title(&row.document_id, &graph_titles, &file_names);
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

/// Removes a row with its cells and captures.
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
/// edited cell is refused (clear it first). Refused while the table's fill runs
/// (`fill_running`) or the row is filled by hand already (`hand_fill_running`).
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
    let _hold = tables.hold_hand_fill(&store, &table_id, &row_id).await?;
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
/// `planned_calls` is the estimate the user agreed to. Says honestly how it went
/// ([`FillStart`]): a job an earlier run of Shodh still holds is not reported as running.
#[tauri::command]
pub async fn fill_start(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    planned_calls: usize,
) -> TableResult<FillStart> {
    check_id("table", &table_id)?;
    check_planned_calls(planned_calls)?;
    let runner = tables.runner().await?;
    let job = runner.start(&table_id, planned_calls).await?;
    let hold = job_hold(tables.store().await?, &job.id).await?;
    let calls_today = tables.calls_today().await?;
    let scheduler = tables.inner.scheduler.clone();
    let start = start_state(
        hold,
        scheduler.is_running(&job.id),
        calls_today,
        tables.inner.cap.get(),
    );
    if matches!(start, "started" | "at_cap") {
        // At the cap the run pauses before any call, until the day's reset; the answer
        // says so, and no notification repeats it.
        scheduler.spawn_run(runner, job.id.clone(), tables.events(&app), true);
    }
    emit_changed(&app, &table_id);
    Ok(FillStart {
        job: job_view(&job),
        start,
        lease_expires_at: match hold {
            Hold::Elsewhere { until } => Some(now_ms(until)),
            Hold::Free | Hold::RunningHere => None,
        },
        day_resets_at: now_ms(next_utc_midnight(Utc::now())),
    })
}

/// Cancels the table's active fill: at once when it is queued or paused, after the
/// current call when it runs here. A job an earlier run of Shodh still holds keeps the
/// cancel asked (`pending`), and it is done by the lease's end at the latest. Finished
/// cells stay. Needs no model.
#[tauri::command]
pub async fn fill_cancel(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
) -> TableResult<FillCancel> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let (s, id) = (store.clone(), table_id.clone());
    let Some(active) = blocking(move || s.active_fill_job(&id)).await? else {
        return Ok(FillCancel {
            job: None,
            pending: false,
            lease_expires_at: None,
        });
    };
    let job = cancel_job(store.clone(), &active.id).await?;
    let hold = job_hold(store, &active.id).await?;
    emit_changed(&app, &table_id);
    Ok(FillCancel {
        pending: job.as_ref().is_some_and(|j| j.state == JobState::Running),
        lease_expires_at: match hold {
            Hold::Elsewhere { until } => Some(now_ms(until)),
            Hold::Free | Hold::RunningHere => None,
        },
        job: job.as_ref().map(job_view),
    })
}

/// The table's fill and the day's calls. Needs no model.
#[tauri::command]
pub async fn fill_status(
    tables: State<'_, TableState>,
    table_id: String,
) -> TableResult<FillStatus> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let (job, rows, columns, cells) = blocking(move || {
        Ok((
            store.latest_fill_job(&table_id)?,
            store.rows(&table_id)?,
            store.columns(&table_id)?,
            store.cells(&table_id)?,
        ))
    })
    .await?;
    let (run, lease_expires_at) = match job.as_ref() {
        Some(j) if j.state == JobState::Running => {
            let hold = job_hold(tables.store().await?, &j.id).await?;
            run_holder(
                Some(j.state),
                hold,
                tables.inner.scheduler.is_running(&j.id),
            )
        }
        _ => (None, None),
    };
    Ok(FillStatus {
        run,
        lease_expires_at,
        papers_done: papers_done(job.as_ref(), &rows, &columns, &cells),
        job: job.as_ref().map(job_view),
        papers_total: rows.len(),
        calls_today: tables.calls_today().await?,
        cap: tables.inner.cap.get(),
        day_resets_at: now_ms(next_utc_midnight(Utc::now())),
    })
}

/// Fills one paper by hand with the configured model, counted against the day's cap,
/// and keeps every model call it made byte for byte: they are returned and kept in
/// `shodh.db` (the row's newest few; they go with the row), also when the fill fails
/// after its calls. Only with Settings → Tables → diagnostics on. The fill is the normal
/// one: cells that are current, or answered by the cache, cost no call.
#[tauri::command]
pub async fn table_fill_paper(
    app: AppHandle,
    tables: State<'_, TableState>,
    table_id: String,
    row_id: String,
) -> TableResult<PaperCapture> {
    check_id("table", &table_id)?;
    check_id("row", &row_id)?;
    let diagnostics = SettingsStore::in_dir(&tables.inner.data_dir)
        .load()
        .map(|s| s.tables.diagnostics)
        .unwrap_or(false);
    if !diagnostics {
        return Err(coded(
            "diagnostics_off",
            "Filling one paper with its model calls kept is a diagnostics tool: turn it on in Settings → Tables.",
        ));
    }
    let store = tables.store().await?;
    let _hold = tables.hold_hand_fill(&store, &table_id, &row_id).await?;
    let request = tables.request(&store, &table_id, &row_id, None).await?;
    let (model, rule) = tables.model().await?;
    let recording = Arc::new(RecordingModel::new(model));
    let filler = tables.filler(recording.clone()).await?;
    let gate = tables.gate(tables.counter().await?);
    let ended = filler.fill_paper(&request, &gate).await;
    emit_changed(&app, &table_id);
    let mut capture = paper_capture(
        Utc::now(),
        &table_id,
        &row_id,
        &request.row.document_id,
        recording.model_id(),
        &rule,
        recording.calls(),
        ended.as_ref().map_err(|e| e.to_string()),
    );
    let json = serde_json::to_string(&capture)
        .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?;
    let (s, table, row, at) = (
        store,
        table_id.clone(),
        row_id.clone(),
        capture.captured_at.clone(),
    );
    match blocking(move || s.save_capture(&table, &row, &at, &json)).await {
        Ok(id) => capture.capture_id = Some(id),
        Err(e) => tracing::warn!(error = %e.message, "the table fill capture could not be kept"),
    }
    Ok(capture)
}

/// The table's kept captures (see `table_fill_paper`), as the JSON each was kept as, for
/// the user to save (test fixtures are made from them).
#[tauri::command]
pub async fn table_captures_export(
    tables: State<'_, TableState>,
    table_id: String,
) -> TableResult<Vec<serde_json::Value>> {
    check_id("table", &table_id)?;
    let store = tables.store().await?;
    let captures = blocking(move || store.captures(&table_id)).await?;
    captures
        .iter()
        .map(|c| {
            serde_json::from_str(&c.capture_json)
                .map_err(|e| ResearchCommandError::unavailable(e.to_string()))
        })
        .collect()
}

// ── Commands: settings ───────────────────────────────────────────

#[tauri::command]
pub async fn tables_settings_get(tables: State<'_, TableState>) -> TableResult<TablesSettings> {
    let diagnostics = SettingsStore::in_dir(&tables.inner.data_dir)
        .load()
        .map(|s| s.tables.diagnostics)
        .unwrap_or(false);
    Ok(settings_view(tables.inner.cap.get(), diagnostics))
}

/// Sets the day's cap of table calls (and, when given, the diagnostics switch). User-only.
/// The cap applies from the next check of every fill, also one in flight; raising it
/// takes up the jobs paused for it at once.
#[tauri::command]
pub async fn tables_settings_set(
    app: AppHandle,
    tables: State<'_, TableState>,
    audit: State<'_, AuditState>,
    daily_call_cap: usize,
    diagnostics: Option<bool>,
) -> TableResult<TablesSettings> {
    check_daily_cap(daily_call_cap)?;
    let store = SettingsStore::in_dir(&tables.inner.data_dir);
    let (settings, before) = tokio::task::spawn_blocking(move || {
        store.update(|s| {
            let before = s.tables;
            s.tables.daily_call_cap = daily_call_cap;
            if let Some(on) = diagnostics {
                s.tables.diagnostics = on;
            }
            Ok(before)
        })
    })
    .await
    .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?
    .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?;
    let raised = daily_call_cap > tables.inner.cap.get();
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
    if raised {
        let woken = wake_paused_jobs(tables.store().await?, PauseReason::DailyCap).await?;
        for job in &woken {
            emit_changed(&app, &job.table_id);
        }
        if !woken.is_empty() {
            tables.check_soon(&app);
        }
    }
    Ok(settings_view(
        tables.inner.cap.get(),
        settings.tables.diagnostics,
    ))
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

    #[test]
    fn a_model_tables_cannot_use_has_its_own_code() {
        assert_eq!(
            model_error(&TableModelError::RefusedReasoning("m".to_string())).code,
            "model_refused"
        );
        for unusable in [
            TableModelError::NoModel,
            TableModelError::LocalOnly,
            TableModelError::NoKey,
        ] {
            let e = model_error(&unusable);
            assert_eq!(e.code, "model_unavailable", "{unusable:?}");
            assert!(e.message.contains("Settings → Model"), "{}", e.message);
        }
        assert_eq!(
            model_error(&TableModelError::Settings("x".to_string())).code,
            "unavailable"
        );
    }

    fn job(state: JobState) -> FillJob {
        FillJob {
            id: "j".to_string(),
            table_id: "t".to_string(),
            state,
            planned_calls: 12,
            done_calls: 5,
            created_at: "2026-10-22T09:00:00.000Z".to_string(),
            next_attempt_at: Some("2026-10-23T00:00:00.000Z".to_string()),
            pause_reason: Some(PauseReason::DailyCap),
            last_error: None,
            cancel_requested: false,
            heartbeat_at: Some("2026-10-22T09:05:00.000Z".to_string()),
            backoff_step: 0,
            failure_streak: 0,
            panics: 0,
        }
    }

    #[test]
    fn hand_fills_never_overlap_a_running_fill_or_each_other() {
        let running = job(JobState::Running);
        assert_eq!(
            hand_fill_refusal(Some(&running)).unwrap().code,
            "fill_running"
        );
        for quiet in [JobState::Paused, JobState::Queued, JobState::Done] {
            assert!(hand_fill_refusal(Some(&job(quiet))).is_none(), "{quiet:?}");
        }
        assert!(hand_fill_refusal(None).is_none());
        let fills = HandFills::default();
        let first = fills.try_hold("r1").unwrap();
        assert!(fills.try_hold("r1").is_none(), "a double click pays once");
        let other = fills.try_hold("r2").unwrap();
        drop(first);
        assert!(fills.try_hold("r1").is_some(), "free again when it ends");
        drop(other);
    }

    #[test]
    fn a_users_fill_says_how_it_started() {
        let until = Utc::now();
        assert_eq!(
            start_state(Hold::Elsewhere { until }, false, 0, 30),
            "held_elsewhere"
        );
        assert_eq!(
            start_state(Hold::RunningHere, false, 0, 30),
            "already_running"
        );
        assert_eq!(start_state(Hold::Free, true, 0, 30), "already_running");
        assert_eq!(start_state(Hold::Free, false, 29, 30), "at_cap");
        assert_eq!(start_state(Hold::Free, false, 28, 30), "started");
    }

    #[test]
    fn the_status_says_who_runs_a_running_fill() {
        let until = Utc::now();
        let running = Some(JobState::Running);
        assert_eq!(
            run_holder(running, Hold::Elsewhere { until }, false),
            (Some("elsewhere"), Some(until.timestamp_millis()))
        );
        assert_eq!(
            run_holder(running, Hold::RunningHere, false),
            (Some("here"), None)
        );
        assert_eq!(run_holder(running, Hold::Free, true), (Some("here"), None));
        assert_eq!(
            run_holder(running, Hold::Free, false),
            (Some("stopped"), None)
        );
        for state in [JobState::Queued, JobState::Paused, JobState::Done] {
            assert_eq!(
                run_holder(Some(state), Hold::Free, false),
                (None, None),
                "{state:?}"
            );
        }
        assert_eq!(run_holder(None, Hold::Free, false), (None, None));
    }

    fn row(id: &str) -> TableRow {
        TableRow {
            id: id.to_string(),
            table_id: "t".to_string(),
            position: 0,
            document_id: format!("/p/{id}.pdf"),
            added_at: "2026-10-22T08:00:00.000Z".to_string(),
        }
    }

    fn column(id: &str) -> Column {
        Column {
            id: id.to_string(),
            table_id: "t".to_string(),
            position: 0,
            spec: spec(ColumnKind::YesNo, 0),
            key: "k".to_string(),
            revision: 1,
            created_at: "2026-10-22T08:00:00.000Z".to_string(),
            updated_at: "2026-10-22T08:00:00.000Z".to_string(),
        }
    }

    fn cell(row: &str, column: &str, status: CellStatus, filled_at: &str) -> Cell {
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
            filled_at: Some(filled_at.to_string()),
            edited_at: None,
            error: None,
        }
    }

    #[test]
    fn only_papers_the_job_filled_count_as_done() {
        let rows = [row("a"), row("b"), row("c"), row("d")];
        let columns = [column("k1"), column("k2")];
        let before = "2026-10-22T08:30:00.000Z";
        let during = "2026-10-22T09:10:00.000Z";
        let job = job(JobState::Running);
        let cells = [
            // a: both columns filled by this job (one failed: it was reached).
            cell("a", "k1", CellStatus::Verified, during),
            cell("a", "k2", CellStatus::Failed, during),
            // b: filled before the job; nothing to do in it.
            cell("b", "k1", CellStatus::Verified, before),
            cell("b", "k2", CellStatus::NotFound, before),
            // c: one column still waits.
            cell("c", "k1", CellStatus::Verified, during),
            cell("c", "k2", CellStatus::Queued, during),
        ];
        assert_eq!(papers_done(Some(&job), &rows, &columns, &cells), 1);
        assert_eq!(
            papers_done(None, &rows, &columns, &cells),
            0,
            "no job, nothing done"
        );
        // A table never filled has nothing done.
        assert_eq!(papers_done(Some(&job), &rows, &columns, &[]), 0);
        // After a cancel (or a failure) the waiting cells are emptied: the papers the job
        // reached stay done, the others do not become done.
        let mut cancelled = cells.to_vec();
        cancelled[5].status = CellStatus::Empty;
        let ended = FillJob {
            state: JobState::Cancelled,
            ..job.clone()
        };
        assert_eq!(papers_done(Some(&ended), &rows, &columns, &cancelled), 1);
        let failed = FillJob {
            state: JobState::Failed,
            ..job
        };
        assert_eq!(papers_done(Some(&failed), &rows, &columns, &cancelled), 1);
    }

    #[test]
    fn a_job_shows_its_times_as_utc_milliseconds() {
        let view = job_view(&job(JobState::Paused));
        assert_eq!(view.next_attempt_at, Some(1_792_713_600_000));
        assert_eq!(view.heartbeat_at, Some(1_792_659_900_000));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["nextAttemptAt"], 1_792_713_600_000_i64);
        assert_eq!(json["heartbeatAt"], 1_792_659_900_000_i64);
        assert_eq!(json["pauseReason"], "daily_cap");
        assert_eq!(json["state"], "paused");
        assert_eq!(json["doneCalls"], 5);
        let waiting = FillJob {
            next_attempt_at: None,
            heartbeat_at: None,
            ..job(JobState::Paused)
        };
        assert_eq!(job_view(&waiting).next_attempt_at, None);
        assert_eq!(job_view(&waiting).heartbeat_at, None);
    }

    #[test]
    fn titles_keep_the_case_the_file_system_spelled() {
        let path = r"C:\Papers\Attention Is All You Need.pdf";
        let key = path_key(path);
        let names = HashMap::from([(key.clone(), "Attention Is All You Need.pdf".to_string())]);
        assert_eq!(
            row_title(&key, &HashMap::new(), &names),
            "Attention Is All You Need.pdf"
        );
        let graph = HashMap::from([(key.clone(), "Attention Is All You Need".to_string())]);
        assert_eq!(
            row_title(&key, &graph, &names),
            "Attention Is All You Need",
            "the citation graph's title first"
        );
        assert_eq!(
            row_title(&key, &HashMap::new(), &HashMap::new()),
            file_name(&key),
            "the key's name when nothing else is known"
        );
    }

    #[test]
    fn a_capture_keeps_the_reply_byte_for_byte_and_the_calls_of_a_fill_that_failed() {
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
        let failed_call = captured("p", 840, &Err("HTTP 429".to_string()));
        assert_eq!(failed_call.error.as_deref(), Some("HTTP 429"));
        assert_eq!(failed_call.reply, None);

        // The fill failed (the database was busy) after two calls: both are kept.
        use chrono::TimeZone;
        let at = Utc.with_ymd_and_hms(2026, 10, 22, 9, 30, 5).unwrap();
        let capture = paper_capture(
            at,
            "t1",
            "r1",
            "/p/a.pdf",
            "test/model:free".to_string(),
            &ReasoningRule::Plain,
            vec![call.clone(), cut],
            Err("research database is busy: locked".to_string()),
        );
        assert_eq!(capture.calls.len(), 2);
        assert_eq!(capture.calls[0].reply.as_deref(), Some(reply));
        assert_eq!(capture.result, None);
        assert_eq!(
            capture.error.as_deref(),
            Some("research database is busy: locked")
        );
        assert_eq!(capture.captured_at, "2026-10-22T09:30:05.000Z");
        let json = serde_json::to_value(&capture).unwrap();
        assert_eq!(json["reasoningRule"], "no reasoning; plain reply budget");
        assert_eq!(json["calls"][0]["reply"], reply);
        let filled = paper_capture(
            at,
            "t1",
            "r1",
            "/p/a.pdf",
            "m".to_string(),
            &ReasoningRule::Plain,
            vec![call],
            Ok(&PaperFill {
                calls: 1,
                ..PaperFill::default()
            }),
        );
        assert_eq!(filled.result.unwrap().calls, 1);
        assert_eq!(filled.error, None);
    }

    #[tokio::test]
    async fn the_scheduler_waits_for_the_model_settings_and_not_forever() {
        let (sent, loaded) = watch::channel(false);
        let waiting = tokio::spawn(models_loaded(loaded));
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "waits while the settings load");
        sent.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        // A loading task that ended without saying so does not hold it back.
        let (gone, loaded) = watch::channel(false);
        drop(gone);
        tokio::time::timeout(Duration::from_secs(5), models_loaded(loaded))
            .await
            .unwrap();
    }
}
