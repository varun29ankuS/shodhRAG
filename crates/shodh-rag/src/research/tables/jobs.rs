//! Fill jobs: filling a whole table, paper by paper, under the day's cap of table calls,
//! pausing for the cap, the provider's quota or the user, resuming later (after a restart
//! too), and cancelling.
//!
//! - **One active job per table**, enforced by `shodh.db` (a unique index over the queued,
//!   running and paused jobs of a table). A paused job is active: starting a fill of its
//!   table returns it.
//! - **One run per job.** A run claims its job with one guarded `UPDATE` that sets
//!   `running` and the process's `run_token`. A job running under this process's token is
//!   never claimed again; a job left `running` under another token was left by a process
//!   that is gone (one app process uses a `shodh.db`: the app is single-instance, and a
//!   separate profile has its own folder), so it is claimed and resumed. Every later write
//!   of the run is guarded by the token, so a run whose job was cancelled, deleted or
//!   claimed elsewhere stops.
//! - **Cells.** The claim marks the cells that hold no result `queued` (missing, `empty`,
//!   `failed`) in the same transaction. Stale results keep their status and are refilled
//!   in place, so a cancel never erases an old answer. Every terminal transition sets the
//!   table's queued cells back to `empty` in its own transaction: queued cells exist only
//!   while their table has an active job. Cells are written only through
//!   [`TableStore::store_fill`], which never overwrites an edit or a newer column.
//! - **Calls.** Before each paper with a cache miss the run checks that
//!   [`CALLS_PER_PAPER_RESERVE`] calls are left under the cap; before each model call the
//!   gate reserves the call ([`CallCounter::try_record`]), so the count errs over by at
//!   most one call per crash and the cap holds across jobs, resumes and restarts.

use std::sync::{Arc, LazyLock};

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::Serialize;

use super::calls::{
    local_day, next_local_midnight, CallCounter, Clock, SystemClock, CALLS_PER_PAPER_RESERVE,
    MIN_DAILY_CAP,
};
use super::fill::{
    CallGate, Filler, Halt, ModelFailure, ModelFailureKind, PaperRequest, MODEL_AUTH, MODEL_QUOTA,
    MODEL_TIMEOUT, PAPER_NOT_INDEXED, SEARCH_FAILED,
};
use super::store::TableStore;
use super::{Cell, ColumnKind, TableRow};
use crate::research::results::{ResultRecord, ResultService};
use crate::research::{blocking, ResearchError, ResearchResult};

/// The error of a job stopped by a failure of the fill itself (the database, a task).
pub const FILL_ERROR: &str = "fill_error";
/// How long a job waits after the model was unreachable for two papers in a row.
pub const UNREACHABLE_PAUSE_MINUTES: i64 = 30;

/// This process's claim on the jobs it runs.
static PROCESS_TOKEN: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// Where a fill job stands (`table_fill_job.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Done,
    Cancelled,
    Failed,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "done" => Some(Self::Done),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Queued, running or paused: the table's one active job.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }
}

/// Why a job is paused (`table_fill_job.pause_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// The day's cap of table calls is reached; resumes at the next local midnight.
    DailyCap,
    /// The provider's rate limit or quota; resumes when it resets.
    Quota,
    /// The provider rejected the API key; waits for the user.
    Auth,
    /// The model could not be reached for two papers in a row; tries again later.
    Unreachable,
}

impl PauseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DailyCap => "daily_cap",
            Self::Quota => "quota",
            Self::Auth => "auth",
            Self::Unreachable => "unreachable",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "daily_cap" => Some(Self::DailyCap),
            "quota" => Some(Self::Quota),
            "auth" => Some(Self::Auth),
            "unreachable" => Some(Self::Unreachable),
            _ => None,
        }
    }
}

/// A fill job of one table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillJob {
    pub id: String,
    pub table_id: String,
    pub state: JobState,
    /// The calls the user was told the fill needs.
    pub planned_calls: usize,
    /// The calls made so far.
    pub done_calls: usize,
    pub created_at: String,
    /// When a paused job is due again (UTC, RFC 3339); none while it waits for the user.
    pub next_attempt_at: Option<String>,
    pub pause_reason: Option<PauseReason>,
    /// The code (and for a search failure its detail) of what stopped or paused the job.
    pub last_error: Option<String>,
    /// A cancel was asked while the job was running.
    pub cancel_requested: bool,
}

/// What filling a table would take: papers and cells that need a fill, the cells the cache
/// answers, and the model calls for the rest (a possible retry aside).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillEstimate {
    pub papers: usize,
    pub columns: usize,
    pub cells: usize,
    pub calls: usize,
    pub cached: usize,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// Every paper was walked.
    Done,
    Cancelled,
    Failed {
        error: String,
    },
    /// Paused; due again at `until`, or when the user starts it (`None`).
    Paused {
        reason: PauseReason,
        until: Option<DateTime<Utc>>,
    },
    /// The job was not run: it is running in this process already, it is finished, or it
    /// does not exist (`state` is `None`).
    NotClaimed {
        state: Option<JobState>,
    },
    /// The job was cancelled, deleted with its table, or claimed elsewhere during the run.
    Lost,
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub outcome: JobOutcome,
    /// Model calls made in this run.
    pub calls: usize,
    /// Cells stored in this run.
    pub cells: usize,
    /// Papers skipped because they are not indexed yet.
    pub not_indexed: usize,
}

impl RunReport {
    fn new(outcome: JobOutcome) -> Self {
        Self {
            outcome,
            calls: 0,
            cells: 0,
            not_indexed: 0,
        }
    }
}

/// The accepted Results of a paper, used as search hints for `number` columns.
#[async_trait::async_trait]
pub trait ResultSource: Send + Sync {
    async fn accepted(&self, path: &str) -> ResearchResult<Vec<ResultRecord>>;
}

#[async_trait::async_trait]
impl ResultSource for ResultService {
    async fn accepted(&self, path: &str) -> ResearchResult<Vec<ResultRecord>> {
        Ok(self.list(path).await?.results)
    }
}

/// How a run ends, as written to its job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum End {
    Done,
    Cancelled,
    Failed(String),
    Paused {
        reason: PauseReason,
        until: Option<DateTime<Utc>>,
        error: Option<String>,
    },
}

impl End {
    fn outcome(&self) -> JobOutcome {
        match self {
            Self::Done => JobOutcome::Done,
            Self::Cancelled => JobOutcome::Cancelled,
            Self::Failed(error) => JobOutcome::Failed {
                error: error.clone(),
            },
            Self::Paused { reason, until, .. } => JobOutcome::Paused {
                reason: *reason,
                until: *until,
            },
        }
    }
}

/// The result of a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Claim {
    Claimed(FillJob),
    /// Not claimable; the job as it is (cancelled by the claim itself when a cancel was
    /// pending), or `None` when there is no such job.
    Refused(Option<FillJob>),
}

/// Fixed-width UTC with milliseconds and `Z`: string order is time order.
fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn bad_column(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, message.into())
}

fn as_count(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

const JOB_COLS: &str = "id, table_id, state, planned_calls, done_calls, created_at, \
     next_attempt_at, pause_reason, last_error, cancel_requested";

fn job_from(r: &Row<'_>) -> rusqlite::Result<FillJob> {
    let state: String = r.get(2)?;
    let state =
        JobState::parse(&state).ok_or_else(|| bad_column(2, format!("unknown state {state}")))?;
    let reason: Option<String> = r.get(7)?;
    let pause_reason = match reason {
        Some(text) => Some(
            PauseReason::parse(&text)
                .ok_or_else(|| bad_column(7, format!("unknown pause reason {text}")))?,
        ),
        None => None,
    };
    Ok(FillJob {
        id: r.get(0)?,
        table_id: r.get(1)?,
        state,
        planned_calls: as_count(r.get(3)?),
        done_calls: as_count(r.get(4)?),
        created_at: r.get(5)?,
        next_attempt_at: r.get(6)?,
        pause_reason,
        last_error: r.get(8)?,
        cancel_requested: r.get::<_, i64>(9)? != 0,
    })
}

fn job_in(conn: &Connection, id: &str) -> rusqlite::Result<Option<FillJob>> {
    conn.query_row(
        &format!("SELECT {JOB_COLS} FROM table_fill_job WHERE id = ?1"),
        params![id],
        job_from,
    )
    .optional()
}

fn active_in(conn: &Connection, table_id: &str) -> rusqlite::Result<Option<FillJob>> {
    conn.query_row(
        &format!(
            "SELECT {JOB_COLS} FROM table_fill_job
              WHERE table_id = ?1 AND state IN ('queued', 'running', 'paused')"
        ),
        params![table_id],
        job_from,
    )
    .optional()
}

/// Marks the table's cells that hold no result `queued`: a missing cell is inserted, an
/// `empty` or `failed` one is changed. Edits and results (stale or not) are not touched.
fn queue_in(conn: &Connection, table_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO table_cell(table_id, row_id, column_id, status)
         SELECT r.table_id, r.id, k.id, 'queued'
           FROM table_row r JOIN table_column k ON k.table_id = r.table_id
          WHERE r.table_id = ?1
            AND NOT EXISTS (SELECT 1 FROM table_cell c
                             WHERE c.row_id = r.id AND c.column_id = k.id)",
        params![table_id],
    )?;
    conn.execute(
        "UPDATE table_cell SET status = 'queued', error = NULL
          WHERE table_id = ?1 AND status IN ('empty', 'failed')",
        params![table_id],
    )?;
    Ok(())
}

/// Sets the table's queued cells back to `empty`.
fn unqueue_in(conn: &Connection, table_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE table_cell SET status = 'empty' WHERE table_id = ?1 AND status = 'queued'",
        params![table_id],
    )
}

/// The job store: `table_fill_job` on the tables' own connection.
impl TableStore {
    /// One fill job.
    pub fn fill_job(&self, id: &str) -> ResearchResult<Option<FillJob>> {
        Ok(job_in(&self.lock(), id)?)
    }

    /// The table's active (queued, running or paused) job.
    pub fn active_fill_job(&self, table_id: &str) -> ResearchResult<Option<FillJob>> {
        Ok(active_in(&self.lock(), table_id)?)
    }

    /// A new queued job for the table, or its active job when it has one.
    pub(super) fn create_fill_job(
        &self,
        table_id: &str,
        planned_calls: usize,
        now: &str,
    ) -> ResearchResult<FillJob> {
        let planned = i64::try_from(planned_calls)
            .map_err(|_| ResearchError::Invalid(format!("{planned_calls} calls is too many.")))?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence_table WHERE id = ?1)",
            params![table_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(ResearchError::NotFound(format!("No table {table_id}.")));
        }
        if let Some(job) = active_in(&tx, table_id)? {
            return Ok(job);
        }
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO table_fill_job(id, table_id, created_at, state, planned_calls, done_calls)
             VALUES (?1, ?2, ?3, 'queued', ?4, 0)",
            params![id, table_id, now, planned],
        )?;
        let job = job_in(&tx, &id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No fill job {id}.")))?;
        tx.commit()?;
        Ok(job)
    }

    /// Claims a job for the run holding `token` and queues its table's cells, in one
    /// transaction. A job is claimable when it is queued or paused, or running under
    /// another token (its process is gone). A claimable job with a pending cancel is
    /// cancelled here instead.
    pub(super) fn claim_fill_job(&self, id: &str, token: &str) -> ResearchResult<Claim> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(job) = job_in(&tx, id)? else {
            return Ok(Claim::Refused(None));
        };
        let owner: Option<String> = tx.query_row(
            "SELECT run_token FROM table_fill_job WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        let claimable = match job.state {
            JobState::Queued | JobState::Paused => true,
            JobState::Running => owner.as_deref() != Some(token),
            JobState::Done | JobState::Cancelled | JobState::Failed => false,
        };
        if !claimable {
            return Ok(Claim::Refused(Some(job)));
        }
        if job.cancel_requested {
            tx.execute(
                "UPDATE table_fill_job SET state = 'cancelled', run_token = NULL,
                   next_attempt_at = NULL, pause_reason = NULL
                 WHERE id = ?1",
                params![id],
            )?;
            unqueue_in(&tx, &job.table_id)?;
            let cancelled = job_in(&tx, id)?;
            tx.commit()?;
            return Ok(Claim::Refused(cancelled));
        }
        let claimed = tx.execute(
            "UPDATE table_fill_job SET state = 'running', run_token = ?2, next_attempt_at = NULL,
               pause_reason = NULL, last_error = NULL
             WHERE id = ?1 AND cancel_requested = 0
               AND (state IN ('queued', 'paused')
                    OR (state = 'running' AND (run_token IS NULL OR run_token <> ?2)))",
            params![id, token],
        )?;
        if claimed == 0 {
            return Ok(Claim::Refused(Some(job)));
        }
        queue_in(&tx, &job.table_id)?;
        let job = job_in(&tx, id)?
            .ok_or_else(|| ResearchError::NotFound(format!("No fill job {id}.")))?;
        tx.commit()?;
        Ok(Claim::Claimed(job))
    }

    /// Whether the run holding `token` still owns the job and no cancel is pending.
    pub(super) fn fill_job_standing(&self, id: &str, token: &str) -> ResearchResult<bool> {
        let cancel: Option<i64> = self
            .lock()
            .query_row(
                "SELECT cancel_requested FROM table_fill_job
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![id, token],
                |r| r.get(0),
            )
            .optional()?;
        Ok(cancel == Some(0))
    }

    /// Adds one call to the job's progress.
    pub(super) fn count_fill_call(&self, id: &str, token: &str) -> ResearchResult<()> {
        self.lock().execute(
            "UPDATE table_fill_job SET done_calls = done_calls + 1
              WHERE id = ?1 AND state = 'running' AND run_token = ?2",
            params![id, token],
        )?;
        Ok(())
    }

    /// Ends the run holding `token`: writes the job's new state and, when it is final,
    /// sets the table's queued cells back to `empty`, in one transaction. A pause asked
    /// while a cancel is pending cancels. Returns how the job ended, or `None` when the
    /// run no longer owns it.
    pub(super) fn finish_fill_job(
        &self,
        id: &str,
        token: &str,
        end: &End,
    ) -> ResearchResult<Option<End>> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let owned: Option<(String, i64)> = tx
            .query_row(
                "SELECT table_id, cancel_requested FROM table_fill_job
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![id, token],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((table_id, cancel)) = owned else {
            return Ok(None);
        };
        let end = match end {
            End::Paused { .. } if cancel != 0 => End::Cancelled,
            other => other.clone(),
        };
        match &end {
            End::Done => tx.execute(
                "UPDATE table_fill_job SET state = 'done', run_token = NULL,
                   next_attempt_at = NULL, pause_reason = NULL, last_error = NULL
                 WHERE id = ?1",
                params![id],
            )?,
            End::Cancelled => tx.execute(
                "UPDATE table_fill_job SET state = 'cancelled', run_token = NULL,
                   next_attempt_at = NULL, pause_reason = NULL
                 WHERE id = ?1",
                params![id],
            )?,
            End::Failed(error) => tx.execute(
                "UPDATE table_fill_job SET state = 'failed', run_token = NULL,
                   next_attempt_at = NULL, pause_reason = NULL, last_error = ?2
                 WHERE id = ?1",
                params![id, error],
            )?,
            End::Paused {
                reason,
                until,
                error,
            } => tx.execute(
                "UPDATE table_fill_job SET state = 'paused', run_token = NULL,
                   next_attempt_at = ?2, pause_reason = ?3, last_error = ?4
                 WHERE id = ?1",
                params![id, (*until).map(timestamp), reason.as_str(), error],
            )?,
        };
        if !matches!(end, End::Paused { .. }) {
            unqueue_in(&tx, &table_id)?;
        }
        tx.commit()?;
        Ok(Some(end))
    }

    /// Gives back a job whose run ended without writing its end (its task failed): it is
    /// queued again, with its queued cells, and the next resume takes it up.
    pub(super) fn release_fill_job(&self, id: &str, token: &str) -> ResearchResult<bool> {
        let released = self.lock().execute(
            "UPDATE table_fill_job SET state = 'queued', run_token = NULL
              WHERE id = ?1 AND state = 'running' AND run_token = ?2",
            params![id, token],
        )?;
        Ok(released > 0)
    }

    /// Cancels a job. A queued or paused job is cancelled at once and its table's queued
    /// cells go back to `empty`; a running job is asked to stop, and its run (or, when its
    /// process is gone, the next claim) finishes the cancel after the current call. A
    /// finished job is left as it is. Returns the job as it now is.
    pub fn cancel_fill_job(&self, id: &str) -> ResearchResult<Option<FillJob>> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(job) = job_in(&tx, id)? else {
            return Ok(None);
        };
        match job.state {
            JobState::Queued | JobState::Paused => {
                tx.execute(
                    "UPDATE table_fill_job SET state = 'cancelled', run_token = NULL,
                       next_attempt_at = NULL, pause_reason = NULL, cancel_requested = 1
                     WHERE id = ?1",
                    params![id],
                )?;
                unqueue_in(&tx, &job.table_id)?;
            }
            JobState::Running => {
                tx.execute(
                    "UPDATE table_fill_job SET cancel_requested = 1 WHERE id = ?1",
                    params![id],
                )?;
            }
            JobState::Done | JobState::Cancelled | JobState::Failed => {}
        }
        let job = job_in(&tx, id)?;
        tx.commit()?;
        Ok(job)
    }

    /// The jobs a runner holding `token` should run now: queued ones, running ones left by
    /// a process that is gone, and paused ones that are due. A paused job that waits for
    /// the user (no time) is never due. Oldest first.
    pub(super) fn due_fill_jobs(&self, token: &str, now: &str) -> ResearchResult<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id FROM table_fill_job
              WHERE state = 'queued'
                 OR (state = 'running' AND (run_token IS NULL OR run_token <> ?1))
                 OR (state = 'paused' AND next_attempt_at IS NOT NULL
                     AND next_attempt_at <= ?2)
              ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map(params![token, now], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The gate of one run: before each call it checks the run still owns its job and no
/// cancel is pending, then reserves the call under the day's cap and counts it on the job.
struct JobGate {
    store: Arc<TableStore>,
    counter: Arc<CallCounter>,
    clock: Arc<dyn Clock>,
    cap: usize,
    job_id: String,
    token: String,
}

#[async_trait::async_trait]
impl CallGate for JobGate {
    async fn before_call(&self) -> ResearchResult<Option<Halt>> {
        let store = self.store.clone();
        let counter = self.counter.clone();
        let clock = self.clock.clone();
        let cap = self.cap;
        let id = self.job_id.clone();
        let token = self.token.clone();
        blocking(move || {
            if !store.fill_job_standing(&id, &token)? {
                return Ok(Some(Halt::Cancelled));
            }
            let day = local_day(clock.as_ref(), clock.now());
            if counter.try_record(day, 1, cap)?.is_none() {
                return Ok(Some(Halt::DailyCap));
            }
            store.count_fill_call(&id, &token)?;
            Ok(None)
        })
        .await
    }
}

/// Runs the fill jobs of the evidence tables with one model, index and cap.
#[derive(Clone)]
pub struct JobRunner {
    filler: Filler,
    counter: Arc<CallCounter>,
    results: Arc<dyn ResultSource>,
    clock: Arc<dyn Clock>,
    cap: usize,
    token: String,
}

impl std::fmt::Debug for JobRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRunner")
            .field("cap", &self.cap)
            .finish_non_exhaustive()
    }
}

impl JobRunner {
    /// A runner filling with `filler`, counting its calls in `counter` under a daily cap
    /// of `cap` calls (at least [`MIN_DAILY_CAP`]), on the computer's clock.
    pub fn new(
        filler: Filler,
        counter: Arc<CallCounter>,
        results: Arc<dyn ResultSource>,
        cap: usize,
    ) -> ResearchResult<Self> {
        if cap < MIN_DAILY_CAP {
            return Err(ResearchError::Invalid(format!(
                "The daily cap of table calls is at least {MIN_DAILY_CAP}."
            )));
        }
        Ok(Self {
            filler,
            counter,
            results,
            clock: Arc::new(SystemClock),
            cap,
            token: PROCESS_TOKEN.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Another process's runner (tests simulate a restart with it).
    #[cfg(test)]
    pub(crate) fn with_token(mut self, token: &str) -> Self {
        self.token = token.to_string();
        self
    }

    fn store(&self) -> Arc<TableStore> {
        self.filler.store.clone()
    }

    /// A fill of the table, queued; or the table's active job when it has one.
    /// `planned_calls` is the estimate the user confirmed.
    pub async fn start(&self, table_id: &str, planned_calls: usize) -> ResearchResult<FillJob> {
        let store = self.store();
        let table = table_id.to_string();
        let now = timestamp(self.clock.now());
        blocking(move || store.create_fill_job(&table, planned_calls, &now)).await
    }

    pub async fn job(&self, job_id: &str) -> ResearchResult<Option<FillJob>> {
        let store = self.store();
        let id = job_id.to_string();
        blocking(move || store.fill_job(&id)).await
    }

    pub async fn cancel(&self, job_id: &str) -> ResearchResult<Option<FillJob>> {
        let store = self.store();
        let id = job_id.to_string();
        blocking(move || store.cancel_fill_job(&id)).await
    }

    /// The jobs to run now (see [`TableStore::due_fill_jobs`]), on this runner's clock.
    pub async fn resume_due(&self) -> ResearchResult<Vec<String>> {
        let store = self.store();
        let token = self.token.clone();
        let now = timestamp(self.clock.now());
        blocking(move || store.due_fill_jobs(&token, &now)).await
    }

    /// The table calls made today (local date).
    pub async fn calls_today(&self) -> ResearchResult<usize> {
        let counter = self.counter.clone();
        let day = local_day(self.clock.as_ref(), self.clock.now());
        blocking(move || counter.count(day)).await
    }

    /// What filling the table would take, from local retrieval and the cache only: no
    /// model call. A paper the fill would make no call for (its file unreadable, not
    /// indexed, not seen by the search, or answered from the cache) adds no call.
    pub async fn estimate(&self, table_id: &str) -> ResearchResult<FillEstimate> {
        let store = self.store();
        let table = table_id.to_string();
        let (rows, columns, cells) = blocking(move || {
            Ok((
                store.rows(&table)?,
                store.columns(&table)?,
                store.cells(&table)?,
            ))
        })
        .await?;
        let mut estimate = FillEstimate {
            columns: columns.len(),
            ..FillEstimate::default()
        };
        for row in rows {
            let row_cells: Vec<Cell> = cells
                .iter()
                .filter(|c| c.row_id == row.id)
                .cloned()
                .collect();
            let mut request = PaperRequest {
                row,
                columns: columns.clone(),
                cells: row_cells,
                results: Vec::new(),
            };
            let survey = self.filler.survey(&request).await?;
            if survey.needing.is_empty() {
                continue;
            }
            estimate.papers += 1;
            estimate.cells += survey.needing.len();
            estimate.cached += survey.needing.len() - survey.misses.len();
            if survey.misses.is_empty() {
                continue;
            }
            if self.needs_hints(&request, &survey.misses) {
                request.results = self.results_of(&request.row.document_id).await;
            }
            estimate.calls += self.filler.planned_calls(&request, &survey).await;
        }
        Ok(estimate)
    }

    /// Runs a job until it is done, paused, cancelled or failed. The run is a task of its
    /// own: a caller that stops waiting does not stop it half way. If the task fails
    /// (panics), the job is released for the next resume and the error returned.
    pub async fn run(&self, job_id: &str) -> ResearchResult<RunReport> {
        let runner = self.clone();
        let id = job_id.to_string();
        match tokio::spawn(async move { runner.run_claimed(&id).await }).await {
            Ok(report) => report,
            Err(e) => {
                let store = self.store();
                let id = job_id.to_string();
                let token = self.token.clone();
                if let Err(release) = blocking(move || store.release_fill_job(&id, &token)).await {
                    tracing::warn!(
                        job = %job_id,
                        error = %release,
                        "an evidence table fill job could not be released after its task failed"
                    );
                }
                Err(ResearchError::Task(e.to_string()))
            }
        }
    }

    async fn run_claimed(&self, job_id: &str) -> ResearchResult<RunReport> {
        let store = self.store();
        let (s, id, token) = (store.clone(), job_id.to_string(), self.token.clone());
        let job = match blocking(move || s.claim_fill_job(&id, &token)).await? {
            Claim::Claimed(job) => job,
            Claim::Refused(job) => {
                return Ok(RunReport::new(JobOutcome::NotClaimed {
                    state: job.map(|j| j.state),
                }))
            }
        };
        let mut report = RunReport::new(JobOutcome::Done);
        let end = match self.walk(&job, &mut report).await {
            Ok(end) => end,
            Err(e) => End::Failed(format!("{FILL_ERROR}: {e}")),
        };
        let (s, id, token) = (store, job_id.to_string(), self.token.clone());
        let ended = blocking(move || s.finish_fill_job(&id, &token, &end)).await?;
        report.outcome = match ended {
            Some(end) => end.outcome(),
            None => JobOutcome::Lost,
        };
        Ok(report)
    }

    /// Fills the job's papers in row order, each as it is when its turn comes.
    async fn walk(&self, job: &FillJob, report: &mut RunReport) -> ResearchResult<End> {
        let store = self.store();
        let (s, table) = (store.clone(), job.table_id.clone());
        let rows: Vec<TableRow> = blocking(move || s.rows(&table)).await?;
        let gate = JobGate {
            store: store.clone(),
            counter: self.counter.clone(),
            clock: self.clock.clone(),
            cap: self.cap,
            job_id: job.id.clone(),
            token: self.token.clone(),
        };
        let mut unreachable_streak = 0usize;
        for row in rows {
            let (s, id, token) = (store.clone(), job.id.clone(), self.token.clone());
            if !blocking(move || s.fill_job_standing(&id, &token)).await? {
                return Ok(End::Cancelled);
            }
            // The paper as it is now: a column changed or an edit made since the run began
            // is seen, and a row removed meanwhile is skipped.
            let (s, table, row_id) = (store.clone(), job.table_id.clone(), row.id.clone());
            let current = blocking(move || {
                let Some(row) = s.rows(&table)?.into_iter().find(|r| r.id == row_id) else {
                    return Ok(None);
                };
                let columns = s.columns(&table)?;
                let cells: Vec<Cell> = s
                    .cells(&table)?
                    .into_iter()
                    .filter(|c| c.row_id == row_id)
                    .collect();
                Ok(Some((row, columns, cells)))
            })
            .await?;
            let Some((row, columns, cells)) = current else {
                continue;
            };
            let mut request = PaperRequest {
                row,
                columns,
                cells,
                results: Vec::new(),
            };
            let survey = self.filler.survey(&request).await?;
            if survey.hash.is_some() && survey.needing.is_empty() {
                continue;
            }
            if !survey.misses.is_empty() {
                let now = self.clock.now();
                let day = local_day(self.clock.as_ref(), now);
                let counter = self.counter.clone();
                let today = blocking(move || counter.count(day)).await?;
                if today + CALLS_PER_PAPER_RESERVE > self.cap {
                    return Ok(self.paused_for_cap(now));
                }
                if self.needs_hints(&request, &survey.misses) {
                    request.results = self.results_of(&request.row.document_id).await;
                }
            }
            let fill = self
                .filler
                .fill_paper_with(&request, Some(&gate as &dyn CallGate))
                .await?;
            report.calls += fill.calls;
            report.cells += fill.cells.len();
            match fill.halted {
                Some(Halt::Cancelled) => return Ok(End::Cancelled),
                Some(Halt::DailyCap) => return Ok(self.paused_for_cap(self.clock.now())),
                None => {}
            }
            match &fill.model_error {
                Some(failure) if failure.kind == ModelFailureKind::Quota => {
                    return Ok(End::Paused {
                        reason: PauseReason::Quota,
                        until: Some(self.quota_until(failure)),
                        error: Some(MODEL_QUOTA.to_string()),
                    });
                }
                Some(failure) if failure.kind == ModelFailureKind::Auth => {
                    return Ok(End::Paused {
                        reason: PauseReason::Auth,
                        until: None,
                        error: Some(MODEL_AUTH.to_string()),
                    });
                }
                Some(failure) if failure.kind == ModelFailureKind::Transient => {
                    unreachable_streak += 1;
                    if unreachable_streak >= 2 {
                        return Ok(End::Paused {
                            reason: PauseReason::Unreachable,
                            until: Some(
                                self.clock.now() + TimeDelta::minutes(UNREACHABLE_PAUSE_MINUTES),
                            ),
                            error: Some(MODEL_TIMEOUT.to_string()),
                        });
                    }
                }
                _ => unreachable_streak = 0,
            }
            if let Some(error) = fill
                .cells
                .iter()
                .filter_map(|c| c.error.as_deref())
                .find(|e| e.starts_with(SEARCH_FAILED))
            {
                return Ok(End::Failed(error.to_string()));
            }
            if fill
                .cells
                .iter()
                .any(|c| c.error.as_deref() == Some(PAPER_NOT_INDEXED))
            {
                report.not_indexed += 1;
            }
        }
        Ok(End::Done)
    }

    /// Paused for the day's cap until the next local midnight.
    fn paused_for_cap(&self, now: DateTime<Utc>) -> End {
        End::Paused {
            reason: PauseReason::DailyCap,
            until: Some(next_local_midnight(self.clock.as_ref(), now)),
            error: None,
        }
    }

    /// When a quota pause ends: the provider's reset time, else the wait it asked for,
    /// else the next local midnight (a daily limit that did not say when it resets).
    fn quota_until(&self, failure: &ModelFailure) -> DateTime<Utc> {
        let now = self.clock.now();
        let reset = failure
            .reset_at_ms
            .and_then(|ms| i64::try_from(ms).ok())
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .filter(|at| *at > now);
        if let Some(at) = reset {
            return at;
        }
        if let Some(secs) = failure.retry_after_secs {
            return now + TimeDelta::seconds(i64::from(secs.max(1)));
        }
        next_local_midnight(self.clock.as_ref(), now)
    }

    /// Whether any of the columns a fill will ask about is a `number` column, whose search
    /// the paper's Results steer.
    fn needs_hints(&self, request: &PaperRequest, misses: &[usize]) -> bool {
        misses.iter().any(|&i| {
            request
                .columns
                .get(i)
                .is_some_and(|c| c.spec.kind == ColumnKind::Number)
        })
    }

    /// The paper's accepted Results; none when they cannot be read (they are only hints).
    async fn results_of(&self, path: &str) -> Vec<ResultRecord> {
        match self.results.accepted(path).await {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!(error = %e, "evidence table fill runs without Result hints");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
