//! Fill jobs: filling a whole table, paper by paper, under the day's cap of table calls,
//! pausing for the cap, the provider's limits or the user, resuming later (after a restart
//! too), and cancelling.
//!
//! - **One active job per table**, enforced by `shodh.db` (a unique index over the queued,
//!   running and paused jobs of a table). A paused job is active: starting a fill of its
//!   table returns it.
//! - **One run per job: a lease.** A run claims its job with one guarded `UPDATE` that
//!   sets `running`, the process's `run_token` and a heartbeat, and refreshes the
//!   heartbeat before every paper and every model call. Another process may take a
//!   `running` job over only once its heartbeat is older than [`LEASE_MINUTES`]. Within
//!   one process a run holds a slot for its job, so a second run of it is turned away,
//!   and a job left `running` under this process's token with no live run (its run ended
//!   without writing its end) is taken up or cancelled at once, without a restart.
//! - **Cells.** The claim marks the cells that hold no result `queued` (missing, `empty`,
//!   `failed`) in the same transaction. Stale results keep their status and are refilled
//!   in place; a failed refill keeps them ([`TableStore::store_fill`]). Every terminal
//!   transition sets the table's queued cells back to `empty` in its own transaction.
//! - **Calls.** The fill's gate checks, once a paper's calls are planned, that they and
//!   the retry fit under the day's cap, and reserves every call before it is made
//!   ([`CallCounter::try_record`]), so the count errs over by at most one call per crash
//!   and the cap holds across jobs, resumes and restarts.
//! - **Escalation.** Rate limits without a hint, an unreachable model and failing papers
//!   pause on a ladder of growing waits persisted on the job; a successful call resets
//!   it. At its end, and after [`MAX_FAILING_PAPERS`] failing papers in a row, the job
//!   waits for the user, so a dead model or key spends a small, bounded number of calls.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::calls::{next_utc_midnight, utc_day, CallCounter, Clock, DailyCap, SystemClock};
use super::fill::{
    CallGate, Filler, Halt, LimitWindow, ModelFailure, ModelFailureKind, PaperFill, PaperRequest,
    MODEL_AUTH, MODEL_FAILED, MODEL_QUOTA, MODEL_TIMEOUT, PAPER_NOT_INDEXED, SEARCH_FAILED,
    UNPARSABLE_RESPONSE,
};
use super::store::{TableStore, REFILL_FAILED};
use super::{Cell, CellStatus, ColumnKind, TableRow};
use crate::research::results::{ResultRecord, ResultService};
use crate::research::{blocking, ResearchError, ResearchResult};

/// The error of a job stopped by a failure of the fill itself (the database, a task).
pub const FILL_ERROR: &str = "fill_error";
/// The error of a job whose runs panicked [`MAX_PANICS`] times.
pub const FILL_PANICKED: &str = "fill_panicked";
/// The error of a job paused because the database stayed locked.
pub const DATABASE_BUSY: &str = "database_busy";
/// Runs of one job that may panic before the job fails instead of being resumed again.
pub const MAX_PANICS: usize = 2;
/// Papers in a row whose model calls all failed (any failure but quota and auth) before
/// the job waits for the user.
pub const MAX_FAILING_PAPERS: usize = 3;
/// How long a run's heartbeat holds its job against another process. The heartbeat is
/// refreshed before every paper and every model call, so the longest gap between two
/// beats is one model call (the HTTP client's timeout is 300 s) plus the local work of a
/// paper (hashing, retrieval, the grounding model: seconds). Ten minutes is twice the
/// call timeout: a live run is never taken over, and a dead one is resumed within ten
/// minutes of its last beat.
pub const LEASE_MINUTES: i64 = 10;
/// The waits of the backoff ladder, in minutes: rate limits that say nothing of when they
/// reset, and an unreachable model. One rung per pause in a row; past the last the job
/// waits for the user. The ladder tops out at 30 minutes and holds there twice more.
pub const BACKOFF_LADDER_MINUTES: [i64; 7] = [1, 2, 5, 15, 30, 30, 30];
/// The wait after a rate limit whose reset time reads as already past (a fast clock).
pub const STALE_RESET_WAIT_SECS: i64 = 60;

/// This process's claim on the jobs it runs.
static PROCESS_TOKEN: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// This process's run token.
pub(super) fn process_token() -> String {
    PROCESS_TOKEN.clone()
}

/// The jobs this process is running now, by (token, job id).
static LIVE_RUNS: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn live_runs() -> std::sync::MutexGuard<'static, HashSet<(String, String)>> {
    LIVE_RUNS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A run's slot for its job in this process; given back when the run ends, however.
struct LiveRun {
    key: (String, String),
}

impl LiveRun {
    fn enter(token: &str, job_id: &str) -> Option<Self> {
        let key = (token.to_string(), job_id.to_string());
        // The set's lock is let go before a slot exists: a slot dropped under it would
        // lock it again, and a refused caller must not free the holder's slot.
        let entered = live_runs().insert(key.clone());
        if entered {
            Some(Self { key })
        } else {
            None
        }
    }

    fn is_live(token: &str, job_id: &str) -> bool {
        live_runs().contains(&(token.to_string(), job_id.to_string()))
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        live_runs().remove(&self.key);
    }
}

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

/// Why a job is paused (`table_fill_job.pause_reason`). A pause without a time waits for
/// the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// The day's cap of table calls is reached; resumes at the next 00:00 UTC.
    DailyCap,
    /// The provider's rate limit or quota.
    Quota,
    /// The provider rejected the API key; waits for the user.
    Auth,
    /// The model could not be reached.
    Unreachable,
    /// Papers in a row failed; waits for the user.
    Failing,
    /// The database stayed locked by another process's writer; resumes in a minute.
    Busy,
    /// No usable model (none set, Local-only mode against a cloud model, no API key, or a
    /// model whose reasoning cannot be bounded); `last_error` holds which. Waits for the
    /// user; the scheduler takes the job up again once a model can be used.
    ModelUnavailable,
}

impl PauseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DailyCap => "daily_cap",
            Self::Quota => "quota",
            Self::Auth => "auth",
            Self::Unreachable => "unreachable",
            Self::Failing => "failing",
            Self::Busy => "busy",
            Self::ModelUnavailable => "model_unavailable",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "daily_cap" => Some(Self::DailyCap),
            "quota" => Some(Self::Quota),
            "auth" => Some(Self::Auth),
            "unreachable" => Some(Self::Unreachable),
            "failing" => Some(Self::Failing),
            "busy" => Some(Self::Busy),
            "model_unavailable" => Some(Self::ModelUnavailable),
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
    /// The running run's last heartbeat.
    pub heartbeat_at: Option<String>,
    /// Backoff pauses in a row (the rung of [`BACKOFF_LADDER_MINUTES`] the next one uses).
    pub backoff_step: usize,
    /// Papers in a row whose model calls all failed.
    pub failure_streak: usize,
    /// Runs of this job that panicked.
    pub panics: usize,
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
    /// The job was not run: it is running (here, or under a live lease elsewhere), it is
    /// finished, or it does not exist (`state` is `None`).
    NotClaimed {
        state: Option<JobState>,
    },
    /// The job was cancelled, deleted with its table, or taken over during the run.
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
    /// Papers this run took up (each with something to fill), those skipped included.
    pub papers: usize,
    /// Papers skipped because they are not indexed yet.
    pub not_indexed: usize,
    /// For a run paused by the provider's rate limit or quota, which limit it was: a
    /// daily quota costs the rest of the day and is worth the user's attention, a short
    /// limit resumes on its own within minutes.
    pub quota_window: Option<LimitWindow>,
}

impl RunReport {
    fn new(outcome: JobOutcome) -> Self {
        Self {
            outcome,
            calls: 0,
            cells: 0,
            papers: 0,
            not_indexed: 0,
            quota_window: None,
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
pub(super) fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Heartbeats at or before this instant belong to a run that is gone.
fn lease_cutoff(now: DateTime<Utc>) -> String {
    timestamp(now - TimeDelta::minutes(LEASE_MINUTES))
}

fn bad_column(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, message.into())
}

fn as_count(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn as_sql(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

const JOB_COLS: &str = "id, table_id, state, planned_calls, done_calls, created_at, \
     next_attempt_at, pause_reason, last_error, cancel_requested, heartbeat_at, \
     backoff_step, failure_streak, panics";

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
        heartbeat_at: r.get(10)?,
        backoff_step: as_count(r.get(11)?),
        failure_streak: as_count(r.get(12)?),
        panics: as_count(r.get(13)?),
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

fn owner_in(conn: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT run_token FROM table_fill_job WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
}

/// Whether a `running` job's run is gone: its token is this caller's and the caller knows
/// no live run of it here, or another process's lease has expired.
fn run_is_gone(
    job: &FillJob,
    owner: Option<&str>,
    token: &str,
    live_here: bool,
    cutoff: &str,
) -> bool {
    if owner == Some(token) {
        return !live_here;
    }
    job.heartbeat_at
        .as_deref()
        .is_none_or(|beat| beat <= cutoff)
}

/// Who holds a job now, as far as a run of it is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Not running, or running under a run that is gone: a claim takes it.
    Free,
    /// A run of this process runs it.
    RunningHere,
    /// Another process's run holds it under a live lease, until `until` at the latest
    /// (its last heartbeat plus [`LEASE_MINUTES`]).
    Elsewhere { until: DateTime<Utc> },
}

/// The hold of `job` (whose run token is `owner`) for a caller holding `token`.
fn hold_of(
    job: &FillJob,
    owner: Option<&str>,
    token: &str,
    live_here: bool,
    now: DateTime<Utc>,
) -> Hold {
    if job.state != JobState::Running {
        return Hold::Free;
    }
    if owner == Some(token) {
        return if live_here {
            Hold::RunningHere
        } else {
            Hold::Free
        };
    }
    let until = job
        .heartbeat_at
        .as_deref()
        .and_then(|beat| DateTime::parse_from_rfc3339(beat).ok())
        .map(|beat| beat.with_timezone(&Utc) + TimeDelta::minutes(LEASE_MINUTES));
    match until {
        Some(until) if owner.is_some() && until > now => Hold::Elsewhere { until },
        _ => Hold::Free,
    }
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

/// Cancels a job at once: state, and its table's queued cells back to `empty`.
fn cancel_in(conn: &Connection, job: &FillJob) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE table_fill_job SET state = 'cancelled', run_token = NULL,
           next_attempt_at = NULL, pause_reason = NULL, cancel_requested = 1
         WHERE id = ?1",
        params![job.id],
    )?;
    unqueue_in(conn, &job.table_id)?;
    Ok(())
}

/// The job store: `table_fill_job` on the tables' own connection.
impl TableStore {
    /// One fill job.
    pub fn fill_job(&self, id: &str) -> ResearchResult<Option<FillJob>> {
        Ok(job_in(&self.conn().read()?, id)?)
    }

    /// The table's active (queued, running or paused) job.
    pub fn active_fill_job(&self, table_id: &str) -> ResearchResult<Option<FillJob>> {
        Ok(active_in(&self.conn().read()?, table_id)?)
    }

    /// A job with the token of the run that holds it.
    fn fill_job_owned(&self, id: &str) -> ResearchResult<Option<(FillJob, Option<String>)>> {
        let conn = self.conn().read()?;
        let Some(job) = job_in(&conn, id)? else {
            return Ok(None);
        };
        let owner = owner_in(&conn, id)?;
        Ok(Some((job, owner)))
    }

    /// Gives back every job running under `token` (the run's process is ending): back to
    /// `queued`, with no run token and no heartbeat, so the next start of the app takes
    /// them up at once instead of waiting out the lease. Cells already filled stay; the
    /// queued ones stay queued; a cancel asked meanwhile is finished by the next claim.
    /// Returns the jobs given back.
    pub fn release_fill_jobs_held_by(&self, token: &str) -> ResearchResult<usize> {
        Ok(self.conn().write(|tx| {
            tx.execute(
                "UPDATE table_fill_job SET state = 'queued', run_token = NULL, heartbeat_at = NULL
                  WHERE state = 'running' AND run_token = ?1",
                params![token],
            )
        })?)
    }

    /// Pauses a job that cannot get a model (`model_unavailable`, `code` as its error,
    /// waiting for the user), unless it is held by a live run (`token` with `live_here`,
    /// or another process's lease newer than `cutoff`). A job whose cancel was asked is
    /// cancelled instead. Queued cells stay queued. Returns the job as it now is.
    pub fn block_fill_job(
        &self,
        id: &str,
        token: &str,
        live_here: bool,
        cutoff: &str,
        code: &str,
    ) -> ResearchResult<Option<FillJob>> {
        self.conn().write(|tx| -> ResearchResult<_> {
            let Some(job) = job_in(tx, id)? else {
                return Ok(None);
            };
            let free = match job.state {
                JobState::Queued | JobState::Paused => true,
                JobState::Running => {
                    let owner = owner_in(tx, id)?;
                    run_is_gone(&job, owner.as_deref(), token, live_here, cutoff)
                }
                JobState::Done | JobState::Cancelled | JobState::Failed => false,
            };
            if free {
                if job.cancel_requested {
                    cancel_in(tx, &job)?;
                } else {
                    tx.execute(
                        "UPDATE table_fill_job SET state = 'paused',
                           pause_reason = 'model_unavailable', next_attempt_at = NULL,
                           last_error = ?2, run_token = NULL, heartbeat_at = NULL
                         WHERE id = ?1",
                        params![id, code],
                    )?;
                }
            }
            let job = job_in(tx, id)?;
            Ok(job)
        })
    }

    /// Makes the jobs paused for `reason` due at `now` (the cap was raised; a model can be
    /// used again). Returns them.
    pub fn wake_paused_fill_jobs(
        &self,
        reason: PauseReason,
        now: &str,
    ) -> ResearchResult<Vec<FillJob>> {
        self.conn().write(|tx| -> ResearchResult<_> {
            let jobs: Vec<FillJob> = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT {JOB_COLS} FROM table_fill_job
                      WHERE state = 'paused' AND pause_reason = ?1 ORDER BY created_at, id"
                ))?;
                let rows = stmt.query_map(params![reason.as_str()], job_from)?;
                rows.collect::<Result<_, _>>()?
            };
            tx.execute(
                "UPDATE table_fill_job SET next_attempt_at = ?2
                  WHERE state = 'paused' AND pause_reason = ?1",
                params![reason.as_str(), now],
            )?;
            Ok(jobs)
        })
    }

    /// The jobs paused for `reason`, oldest first.
    pub fn paused_fill_jobs(&self, reason: PauseReason) -> ResearchResult<Vec<FillJob>> {
        let conn = self.conn().read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLS} FROM table_fill_job
              WHERE state = 'paused' AND pause_reason = ?1 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![reason.as_str()], job_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The table's newest job, active or finished: how its last fill ended.
    pub fn latest_fill_job(&self, table_id: &str) -> ResearchResult<Option<FillJob>> {
        Ok(self
            .conn()
            .read()?
            .query_row(
                &format!(
                    "SELECT {JOB_COLS} FROM table_fill_job WHERE table_id = ?1
                      ORDER BY created_at DESC, rowid DESC LIMIT 1"
                ),
                params![table_id],
                job_from,
            )
            .optional()?)
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
        self.conn().write(|tx| -> ResearchResult<_> {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM evidence_table WHERE id = ?1)",
                params![table_id],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(ResearchError::NotFound(format!("No table {table_id}.")));
            }
            if let Some(job) = active_in(tx, table_id)? {
                return Ok(job);
            }
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO table_fill_job(id, table_id, created_at, state, planned_calls, done_calls)
                 VALUES (?1, ?2, ?3, 'queued', ?4, 0)",
                params![id, table_id, now, planned],
            )?;
            let job = job_in(tx, &id)?
                .ok_or_else(|| ResearchError::NotFound(format!("No fill job {id}.")))?;
            Ok(job)
        })
    }

    /// Claims a job for the run holding `token` and queues its table's cells, in one
    /// transaction. A job is claimable when it is queued or paused, or running under
    /// this token (the caller holds this process's only slot for the job, so that run is
    /// gone) or under another token whose lease expired (heartbeat at or before
    /// `cutoff`). A claimable job with a pending cancel is cancelled here instead.
    pub(super) fn claim_fill_job(
        &self,
        id: &str,
        token: &str,
        now: &str,
        cutoff: &str,
    ) -> ResearchResult<Claim> {
        self.conn().write(|tx| -> ResearchResult<_> {
            let Some(job) = job_in(tx, id)? else {
                return Ok(Claim::Refused(None));
            };
            let owner = owner_in(tx, id)?;
            let claimable = match job.state {
                JobState::Queued | JobState::Paused => true,
                JobState::Running => run_is_gone(&job, owner.as_deref(), token, false, cutoff),
                JobState::Done | JobState::Cancelled | JobState::Failed => false,
            };
            if !claimable {
                return Ok(Claim::Refused(Some(job)));
            }
            if job.cancel_requested {
                cancel_in(tx, &job)?;
                let cancelled = job_in(tx, id)?;
                return Ok(Claim::Refused(cancelled));
            }
            let claimed = tx.execute(
                "UPDATE table_fill_job SET state = 'running', run_token = ?2, heartbeat_at = ?3,
                   next_attempt_at = NULL, pause_reason = NULL, last_error = NULL
                 WHERE id = ?1 AND cancel_requested = 0
                   AND (state IN ('queued', 'paused')
                        OR (state = 'running'
                            AND (run_token IS NULL OR run_token = ?2
                                 OR heartbeat_at IS NULL OR heartbeat_at <= ?4)))",
                params![id, token, now, cutoff],
            )?;
            if claimed == 0 {
                return Ok(Claim::Refused(Some(job)));
            }
            queue_in(tx, &job.table_id)?;
            let job = job_in(tx, id)?
                .ok_or_else(|| ResearchError::NotFound(format!("No fill job {id}.")))?;
            Ok(Claim::Claimed(job))
        })
    }

    /// Renews the run's lease. True while the run holding `token` owns the job and no
    /// cancel is pending.
    pub(super) fn beat_fill_job(&self, id: &str, token: &str, now: &str) -> ResearchResult<bool> {
        let renewed = self.conn().write(|tx| {
            tx.execute(
                "UPDATE table_fill_job SET heartbeat_at = ?3
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2 AND cancel_requested = 0",
                params![id, token, now],
            )
        })?;
        Ok(renewed > 0)
    }

    /// Adds one call to the job's progress.
    pub(super) fn count_fill_call(&self, id: &str, token: &str) -> ResearchResult<()> {
        self.conn().write(|tx| {
            tx.execute(
                "UPDATE table_fill_job SET done_calls = done_calls + 1
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![id, token],
            )
        })?;
        Ok(())
    }

    /// Writes the run's escalation counters.
    pub(super) fn set_fill_streaks(
        &self,
        id: &str,
        token: &str,
        backoff_step: usize,
        failure_streak: usize,
    ) -> ResearchResult<()> {
        self.conn().write(|tx| {
            tx.execute(
                "UPDATE table_fill_job SET backoff_step = ?3, failure_streak = ?4
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![id, token, as_sql(backoff_step), as_sql(failure_streak)],
            )
        })?;
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
        self.conn().write(|tx| -> ResearchResult<_> {
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
                unqueue_in(tx, &table_id)?;
            }
            Ok(Some(end))
        })
    }

    /// Gives back a job whose run ended without writing its end (an error in the last
    /// write): it is queued again, with its queued cells, and the next resume takes it up.
    pub(super) fn release_fill_job(&self, id: &str, token: &str) -> ResearchResult<bool> {
        let released = self.conn().write(|tx| {
            tx.execute(
                "UPDATE table_fill_job SET state = 'queued', run_token = NULL
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![id, token],
            )
        })?;
        Ok(released > 0)
    }

    /// Counts a panic of the run holding `token`. Under [`MAX_PANICS`] the job is queued
    /// again; at it the job fails with [`FILL_PANICKED`] and its queued cells go back to
    /// `empty`, so a panic that repeats does not spend a call on every resume.
    pub(super) fn record_fill_panic(
        &self,
        id: &str,
        token: &str,
    ) -> ResearchResult<Option<FillJob>> {
        self.conn().write(|tx| -> ResearchResult<_> {
            let owned = tx
                .query_row(
                    "SELECT table_id, panics FROM table_fill_job
                      WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                    params![id, token],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?;
            let Some((table_id, panics)) = owned else {
                return Ok(job_in(tx, id)?);
            };
            let panics = as_count(panics) + 1;
            if panics >= MAX_PANICS {
                tx.execute(
                    "UPDATE table_fill_job SET state = 'failed', run_token = NULL, panics = ?2,
                       next_attempt_at = NULL, pause_reason = NULL, last_error = ?3
                     WHERE id = ?1",
                    params![id, as_sql(panics), FILL_PANICKED],
                )?;
                unqueue_in(tx, &table_id)?;
            } else {
                tx.execute(
                    "UPDATE table_fill_job SET state = 'queued', run_token = NULL, panics = ?2
                     WHERE id = ?1",
                    params![id, as_sql(panics)],
                )?;
            }
            let job = job_in(tx, id)?;
            Ok(job)
        })
    }

    /// Cancels a job. A queued or paused job is cancelled at once and its table's queued
    /// cells go back to `empty`; so is a running job whose run is gone (this caller's
    /// token with no live run here, `live_here` false, or another process's lease expired
    /// at `cutoff`). A live running job is asked to stop, and its run finishes the cancel
    /// after the current call. A finished job is left as it is. Returns the job as it now
    /// is.
    pub fn cancel_fill_job(
        &self,
        id: &str,
        token: &str,
        live_here: bool,
        cutoff: &str,
    ) -> ResearchResult<Option<FillJob>> {
        self.conn().write(|tx| -> ResearchResult<_> {
            let Some(job) = job_in(tx, id)? else {
                return Ok(None);
            };
            match job.state {
                JobState::Queued | JobState::Paused => cancel_in(tx, &job)?,
                JobState::Running => {
                    let owner = owner_in(tx, id)?;
                    if run_is_gone(&job, owner.as_deref(), token, live_here, cutoff) {
                        cancel_in(tx, &job)?;
                    } else {
                        tx.execute(
                            "UPDATE table_fill_job SET cancel_requested = 1 WHERE id = ?1",
                            params![id],
                        )?;
                    }
                }
                JobState::Done | JobState::Cancelled | JobState::Failed => {}
            }
            let job = job_in(tx, id)?;
            Ok(job)
        })
    }

    /// The jobs a runner holding `token` could run now: queued ones, paused ones that are
    /// due, running ones whose lease expired, and running ones under `token` (the caller
    /// leaves out those with a live run here; the flag says which these are). A paused job
    /// that waits for the user (no time) is never due. Oldest first.
    pub(super) fn due_fill_jobs(
        &self,
        token: &str,
        now: &str,
        cutoff: &str,
    ) -> ResearchResult<Vec<(String, bool)>> {
        let conn = self.conn().read()?;
        let mut stmt = conn.prepare(
            "SELECT id, COALESCE(state = 'running' AND run_token = ?1, 0) FROM table_fill_job
              WHERE state = 'queued'
                 OR (state = 'paused' AND next_attempt_at IS NOT NULL
                     AND next_attempt_at <= ?2)
                 OR (state = 'running'
                     AND (run_token IS NULL OR run_token = ?1
                          OR heartbeat_at IS NULL OR heartbeat_at <= ?3))
              ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map(params![token, now, cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// A gate that counts calls against the day's cap with no job: for filling one paper by
/// hand (a refill, a debugging command). Its calls count like a job's.
pub struct CounterGate {
    counter: Arc<CallCounter>,
    clock: Arc<dyn Clock>,
    cap: DailyCap,
}

impl CounterGate {
    pub fn new(counter: Arc<CallCounter>, cap: DailyCap) -> Self {
        Self {
            counter,
            clock: Arc::new(SystemClock),
            cap,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
}

/// Whether `calls` more fit under `cap` today.
fn paper_fits(
    counter: &CallCounter,
    clock: &dyn Clock,
    calls: usize,
    cap: usize,
) -> ResearchResult<bool> {
    let today = counter.count(utc_day(clock.now()))?;
    Ok(today.saturating_add(calls) <= cap)
}

#[async_trait::async_trait]
impl CallGate for CounterGate {
    async fn before_paper(&self, calls: usize) -> ResearchResult<Option<Halt>> {
        let (counter, clock, cap) = (self.counter.clone(), self.clock.clone(), self.cap.get());
        blocking(move || {
            Ok((!paper_fits(&counter, clock.as_ref(), calls, cap)?).then_some(Halt::DailyCap))
        })
        .await
    }

    async fn before_call(&self) -> ResearchResult<Option<Halt>> {
        let (counter, clock, cap) = (self.counter.clone(), self.clock.clone(), self.cap.get());
        blocking(move || {
            let reserved = counter.try_record(utc_day(clock.now()), 1, cap)?;
            Ok(reserved.is_none().then_some(Halt::DailyCap))
        })
        .await
    }
}

/// The gate of one run: it renews the run's lease, stops when the job is no longer the
/// run's or a cancel is pending, refuses a paper whose calls no longer fit the day's cap,
/// and reserves every call before it is made.
struct JobGate {
    store: Arc<TableStore>,
    counter: Arc<CallCounter>,
    clock: Arc<dyn Clock>,
    cap: DailyCap,
    job_id: String,
    token: String,
}

#[async_trait::async_trait]
impl CallGate for JobGate {
    async fn before_paper(&self, calls: usize) -> ResearchResult<Option<Halt>> {
        let store = self.store.clone();
        let counter = self.counter.clone();
        let clock = self.clock.clone();
        let cap = self.cap.get();
        let id = self.job_id.clone();
        let token = self.token.clone();
        blocking(move || {
            if !store.beat_fill_job(&id, &token, &timestamp(clock.now()))? {
                return Ok(Some(Halt::Cancelled));
            }
            if !paper_fits(&counter, clock.as_ref(), calls, cap)? {
                return Ok(Some(Halt::DailyCap));
            }
            Ok(None)
        })
        .await
    }

    async fn before_call(&self) -> ResearchResult<Option<Halt>> {
        let store = self.store.clone();
        let counter = self.counter.clone();
        let clock = self.clock.clone();
        let cap = self.cap.get();
        let id = self.job_id.clone();
        let token = self.token.clone();
        blocking(move || {
            if !store.beat_fill_job(&id, &token, &timestamp(clock.now()))? {
                return Ok(Some(Halt::Cancelled));
            }
            if counter.try_record(utc_day(clock.now()), 1, cap)?.is_none() {
                return Ok(Some(Halt::DailyCap));
            }
            store.count_fill_call(&id, &token)?;
            Ok(None)
        })
        .await
    }
}

/// The escalation counters of a run, as stored on its job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Streaks {
    backoff_step: usize,
    failure_streak: usize,
}

/// Runs the fill jobs of the evidence tables with one model, index and cap.
#[derive(Clone)]
pub struct JobRunner {
    filler: Filler,
    counter: Arc<CallCounter>,
    results: Arc<dyn ResultSource>,
    clock: Arc<dyn Clock>,
    cap: DailyCap,
    token: String,
}

impl std::fmt::Debug for JobRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRunner")
            .field("cap", &self.cap.get())
            .finish_non_exhaustive()
    }
}

/// Cancels a job from this process without a runner, so stopping a fill never needs a
/// usable model: see [`JobRunner::cancel`].
pub async fn cancel_job(store: Arc<TableStore>, job_id: &str) -> ResearchResult<Option<FillJob>> {
    let id = job_id.to_string();
    let token = PROCESS_TOKEN.clone();
    let live = LiveRun::is_live(&token, &id);
    let cutoff = lease_cutoff(SystemClock.now());
    blocking(move || store.cancel_fill_job(&id, &token, live, &cutoff)).await
}

/// Gives back every job this process's runs hold (see
/// [`TableStore::release_fill_jobs_held_by`]): called when the app exits.
pub async fn release_held_jobs(store: Arc<TableStore>) -> ResearchResult<usize> {
    let token = PROCESS_TOKEN.clone();
    blocking(move || store.release_fill_jobs_held_by(&token)).await
}

/// Who holds the job now, for this process on the computer's clock: see [`Hold`].
pub async fn job_hold(store: Arc<TableStore>, job_id: &str) -> ResearchResult<Hold> {
    hold_at(store, job_id, PROCESS_TOKEN.clone(), SystemClock.now()).await
}

pub(super) async fn hold_at(
    store: Arc<TableStore>,
    job_id: &str,
    token: String,
    now: DateTime<Utc>,
) -> ResearchResult<Hold> {
    let id = job_id.to_string();
    let live = LiveRun::is_live(&token, job_id);
    let owned = blocking(move || store.fill_job_owned(&id)).await?;
    Ok(match owned {
        Some((job, owner)) => hold_of(&job, owner.as_deref(), &token, live, now),
        None => Hold::Free,
    })
}

/// Pauses a job of this process's scheduler that cannot get a model: see
/// [`TableStore::block_fill_job`].
pub async fn block_job(
    store: Arc<TableStore>,
    job_id: &str,
    code: &str,
) -> ResearchResult<Option<FillJob>> {
    block_at(
        store,
        job_id,
        code,
        PROCESS_TOKEN.clone(),
        SystemClock.now(),
    )
    .await
}

pub(super) async fn block_at(
    store: Arc<TableStore>,
    job_id: &str,
    code: &str,
    token: String,
    now: DateTime<Utc>,
) -> ResearchResult<Option<FillJob>> {
    let (id, code) = (job_id.to_string(), code.to_string());
    let live = LiveRun::is_live(&token, job_id);
    let cutoff = lease_cutoff(now);
    blocking(move || store.block_fill_job(&id, &token, live, &cutoff, &code)).await
}

/// Makes the jobs paused for `reason` due now: see [`TableStore::wake_paused_fill_jobs`].
pub async fn wake_paused_jobs(
    store: Arc<TableStore>,
    reason: PauseReason,
) -> ResearchResult<Vec<FillJob>> {
    let now = timestamp(SystemClock.now());
    blocking(move || store.wake_paused_fill_jobs(reason, &now)).await
}

/// The jobs a runner of this process could run now, on the computer's clock, without a
/// runner (a scheduler asks this before it resolves a model): see
/// [`JobRunner::resume_due`].
pub async fn resume_due_jobs(store: Arc<TableStore>) -> ResearchResult<Vec<String>> {
    due_jobs(store, PROCESS_TOKEN.clone(), SystemClock.now()).await
}

/// The jobs due at `now` for a runner holding `token`, leaving out those this process is
/// running.
pub(super) async fn due_jobs(
    store: Arc<TableStore>,
    token: String,
    now: DateTime<Utc>,
) -> ResearchResult<Vec<String>> {
    let (at, cutoff) = (timestamp(now), lease_cutoff(now));
    let mine = token.clone();
    let due = blocking(move || store.due_fill_jobs(&mine, &at, &cutoff)).await?;
    Ok(due
        .into_iter()
        .filter(|(id, own_running)| !(*own_running && LiveRun::is_live(&token, id)))
        .map(|(id, _)| id)
        .collect())
}

impl JobRunner {
    /// A runner filling with `filler`, counting its calls in `counter` under the shared
    /// daily cap `cap`, on the computer's clock. The cap is read at every check, so a
    /// change in Settings applies from the next paper of a run in flight.
    pub fn new(
        filler: Filler,
        counter: Arc<CallCounter>,
        results: Arc<dyn ResultSource>,
        cap: DailyCap,
    ) -> Self {
        Self {
            filler,
            counter,
            results,
            clock: Arc::new(SystemClock),
            cap,
            token: PROCESS_TOKEN.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Another process's runner (tests simulate a second process or a restart with it).
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

    /// Cancels a job (see [`TableStore::cancel_fill_job`]): a job whose run is gone, in
    /// this process or another, is cancelled at once.
    pub async fn cancel(&self, job_id: &str) -> ResearchResult<Option<FillJob>> {
        let store = self.store();
        let id = job_id.to_string();
        let token = self.token.clone();
        let live = LiveRun::is_live(&token, &id);
        let cutoff = lease_cutoff(self.clock.now());
        blocking(move || store.cancel_fill_job(&id, &token, live, &cutoff)).await
    }

    /// Who holds the job now, for this runner on its clock: see [`Hold`].
    pub async fn hold(&self, job_id: &str) -> ResearchResult<Hold> {
        hold_at(self.store(), job_id, self.token.clone(), self.clock.now()).await
    }

    /// Pauses a job that cannot get a model, for this runner on its clock: see
    /// [`TableStore::block_fill_job`].
    pub async fn block(&self, job_id: &str, code: &str) -> ResearchResult<Option<FillJob>> {
        block_at(
            self.store(),
            job_id,
            code,
            self.token.clone(),
            self.clock.now(),
        )
        .await
    }

    /// The jobs to run now (see [`TableStore::due_fill_jobs`]), on this runner's clock,
    /// leaving out those this process is running.
    pub async fn resume_due(&self) -> ResearchResult<Vec<String>> {
        due_jobs(self.store(), self.token.clone(), self.clock.now()).await
    }

    /// The table calls made today (UTC date).
    pub async fn calls_today(&self) -> ResearchResult<usize> {
        let counter = self.counter.clone();
        let day = utc_day(self.clock.now());
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
    /// own: a caller that stops waiting does not stop it half way. If the run ends with an
    /// error before writing its end, the job is given back (queued) so it can be resumed
    /// or cancelled at once; if the task panics, the panic is counted on the job, and the
    /// job fails at [`MAX_PANICS`].
    pub async fn run(&self, job_id: &str) -> ResearchResult<RunReport> {
        let runner = self.clone();
        let id = job_id.to_string();
        let joined = tokio::spawn(async move { runner.run_claimed(&id).await }).await;
        let store = self.store();
        let id = job_id.to_string();
        let token = self.token.clone();
        match joined {
            Ok(Ok(report)) => Ok(report),
            Ok(Err(e)) => {
                if let Err(release) = blocking(move || store.release_fill_job(&id, &token)).await {
                    tracing::warn!(
                        job = %job_id,
                        error = %release,
                        "an evidence table fill job could not be given back; it is taken up or cancelled when asked"
                    );
                }
                Err(e)
            }
            Err(panic) => {
                if let Err(record) = blocking(move || store.record_fill_panic(&id, &token)).await {
                    tracing::warn!(
                        job = %job_id,
                        error = %record,
                        "the panic of an evidence table fill job could not be recorded"
                    );
                }
                Err(ResearchError::Task(panic.to_string()))
            }
        }
    }

    async fn run_claimed(&self, job_id: &str) -> ResearchResult<RunReport> {
        let store = self.store();
        let Some(_live) = LiveRun::enter(&self.token, job_id) else {
            let (s, id) = (store.clone(), job_id.to_string());
            let job = blocking(move || s.fill_job(&id)).await?;
            return Ok(RunReport::new(JobOutcome::NotClaimed {
                state: job.map(|j| j.state),
            }));
        };
        let now = self.clock.now();
        let (s, id, token) = (store.clone(), job_id.to_string(), self.token.clone());
        let (at, cutoff) = (timestamp(now), lease_cutoff(now));
        let job = match blocking(move || s.claim_fill_job(&id, &token, &at, &cutoff)).await? {
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
            Err(ResearchError::Busy(_)) => End::Paused {
                reason: PauseReason::Busy,
                until: Some(self.clock.now() + TimeDelta::minutes(BACKOFF_LADDER_MINUTES[0])),
                error: Some(DATABASE_BUSY.to_string()),
            },
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

    /// Writes the escalation counters when they changed.
    async fn save_streaks(
        &self,
        job: &FillJob,
        before: Streaks,
        now: Streaks,
    ) -> ResearchResult<()> {
        if before == now {
            return Ok(());
        }
        let (store, id, token) = (self.store(), job.id.clone(), self.token.clone());
        blocking(move || store.set_fill_streaks(&id, &token, now.backoff_step, now.failure_streak))
            .await
    }

    /// The next rung of the backoff ladder, or `None` past its end (wait for the user).
    fn next_rung(&self, streaks: &mut Streaks) -> Option<DateTime<Utc>> {
        let minutes = BACKOFF_LADDER_MINUTES.get(streaks.backoff_step).copied()?;
        streaks.backoff_step += 1;
        Some(self.clock.now() + TimeDelta::minutes(minutes))
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
            cap: self.cap.clone(),
            job_id: job.id.clone(),
            token: self.token.clone(),
        };
        let mut streaks = Streaks {
            backoff_step: job.backoff_step,
            failure_streak: job.failure_streak,
        };
        for row in rows {
            let (s, id, token) = (store.clone(), job.id.clone(), self.token.clone());
            let now = timestamp(self.clock.now());
            if !blocking(move || s.beat_fill_job(&id, &token, &now)).await? {
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
            report.papers += 1;
            if self.needs_hints(&request, &survey.misses) {
                request.results = self.results_of(&request.row.document_id).await;
            }
            let fill = self.filler.fill_paper(&request, &gate).await?;
            report.calls += fill.calls;
            report.cells += fill.cells.len();
            let before = streaks;
            let end = self.after_paper(&fill, &mut streaks, report);
            self.save_streaks(job, before, streaks).await?;
            if let Some(end) = end {
                return Ok(end);
            }
        }
        Ok(End::Done)
    }

    /// What one paper's fill means for the job: `Some` ends the run there.
    fn after_paper(
        &self,
        fill: &PaperFill,
        streaks: &mut Streaks,
        report: &mut RunReport,
    ) -> Option<End> {
        match fill.halted {
            Some(Halt::Cancelled) => return Some(End::Cancelled),
            Some(Halt::DailyCap) => {
                return Some(End::Paused {
                    reason: PauseReason::DailyCap,
                    until: Some(next_utc_midnight(self.clock.now())),
                    error: None,
                })
            }
            None => {}
        }
        if let Some(failure) = &fill.model_error {
            return match failure.kind {
                ModelFailureKind::Quota => {
                    report.quota_window = Some(failure.window);
                    Some(self.quota_pause(failure, streaks))
                }
                ModelFailureKind::Auth => Some(End::Paused {
                    reason: PauseReason::Auth,
                    until: None,
                    error: Some(MODEL_AUTH.to_string()),
                }),
                ModelFailureKind::Transient => {
                    streaks.failure_streak += 1;
                    if streaks.failure_streak >= MAX_FAILING_PAPERS {
                        return Some(self.failing_pause(MODEL_TIMEOUT));
                    }
                    // The model is not reachable: waiting helps, the next paper does not.
                    Some(match self.next_rung(streaks) {
                        Some(until) => End::Paused {
                            reason: PauseReason::Unreachable,
                            until: Some(until),
                            error: Some(MODEL_TIMEOUT.to_string()),
                        },
                        None => self.failing_pause(MODEL_TIMEOUT),
                    })
                }
                ModelFailureKind::Other => {
                    streaks.failure_streak += 1;
                    (streaks.failure_streak >= MAX_FAILING_PAPERS)
                        .then(|| self.failing_pause(MODEL_FAILED))
                }
            };
        }
        if let Some(error) = &fill.paper_error {
            if error.starts_with(SEARCH_FAILED) {
                return Some(End::Failed(error.clone()));
            }
            if error == PAPER_NOT_INDEXED {
                report.not_indexed += 1;
            }
            return None;
        }
        if fill.calls > 0 {
            let answered = fill.cells.iter().any(|c| {
                c.status != CellStatus::Failed
                    && !c
                        .error
                        .as_deref()
                        .is_some_and(|e| e.starts_with(REFILL_FAILED))
            });
            if answered {
                // A call answered: the model is reachable and the limits are open again.
                *streaks = Streaks {
                    backoff_step: 0,
                    failure_streak: 0,
                };
            } else {
                // Calls made and nothing usable came back (unparsable or empty replies).
                streaks.failure_streak += 1;
                if streaks.failure_streak >= MAX_FAILING_PAPERS {
                    return Some(self.failing_pause(UNPARSABLE_RESPONSE));
                }
            }
        }
        None
    }

    /// Paused until the user starts the job again, after failing papers in a row.
    fn failing_pause(&self, code: &str) -> End {
        End::Paused {
            reason: PauseReason::Failing,
            until: None,
            error: Some(code.to_string()),
        }
    }

    /// When a quota or rate-limit pause ends, in this order: the provider's reset time
    /// when it is ahead; a minute when the reset reads as past (a clock running fast);
    /// the wait the provider asked for; for a short or unknown limit, the next rung of the
    /// backoff ladder (past its end the job waits for the user); for a daily limit, the
    /// provider's daily reset, the next 00:00 UTC.
    fn quota_pause(&self, failure: &ModelFailure, streaks: &mut Streaks) -> End {
        let now = self.clock.now();
        // The window is kept with the pause, so the interface words a daily quota and a
        // short rate limit apart without guessing from the wait.
        let paused = |until: Option<DateTime<Utc>>| End::Paused {
            reason: PauseReason::Quota,
            until,
            error: Some(format!("{MODEL_QUOTA}:{}", failure.window.as_str())),
        };
        if let Some(reset) = failure
            .reset_at_ms
            .and_then(|ms| i64::try_from(ms).ok())
            .and_then(DateTime::<Utc>::from_timestamp_millis)
        {
            return paused(Some(if reset > now {
                reset
            } else {
                now + TimeDelta::seconds(STALE_RESET_WAIT_SECS)
            }));
        }
        if let Some(secs) = failure.retry_after_secs {
            return paused(Some(now + TimeDelta::seconds(i64::from(secs.max(1)))));
        }
        match failure.window {
            LimitWindow::Daily => paused(Some(next_utc_midnight(now))),
            LimitWindow::Short | LimitWindow::Unknown => paused(self.next_rung(streaks)),
        }
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
