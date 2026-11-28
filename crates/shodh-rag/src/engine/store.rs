//! The engine's jobs and runs, in `shodh.db` (`engine_job`, `engine_run`, migration 19).
//!
//! - A job has at most one **active** run (queued, running or waiting): the partial
//!   unique index `engine_run_active` refuses a second, and [`EngineStore::insert_run`]
//!   returns the active one instead.
//! - A run is **claimed** with one guarded `UPDATE` that sets `running`, the run token and
//!   a heartbeat, as the table runner claims its jobs: another process may take a running
//!   run over only once its heartbeat is older than [`super::lease::LEASE_MINUTES`].
//! - Foreign keys are not enforced in this database, so deleting a job deletes its runs
//!   in the same transaction.
//! - Runs older than [`RETENTION_DAYS`] are pruned, but never the latest
//!   [`KEEP_LATEST_RUNS`] of a job, and never an active run. A system job's runs (the
//!   app's own work, never shown to the user) are kept [`SYSTEM_RETENTION_DAYS`], and
//!   always its newest run, which its schedule is planned from.
//! - A job without a schedule may carry a **due time its owner sets**
//!   ([`EngineStore::set_due_at_most`]): it runs once at that time, and the time is taken
//!   when the run is planned ([`EngineStore::take_due`]).

use std::path::Path;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::Value;

use super::lease::timestamp;
use super::schedule::Schedule;
use crate::audit::{open_shared_connection, AuditError, AuditKey, SharedConnection};

/// Runs of the last this many days are kept.
pub const RETENTION_DAYS: i64 = 90;
/// The runs of a job kept whatever their age.
pub const KEEP_LATEST_RUNS: i64 = 20;
/// Runs of a system job of the last this many days are kept: its history is for support,
/// not for the user.
pub const SYSTEM_RETENTION_DAYS: i64 = 7;

/// Error of the engine's store.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// `shodh.db` could not be opened or migrated.
    #[error("The app database could not be opened: {0}")]
    Open(#[from] AuditError),
    #[error("Scheduler database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("A scheduler task stopped: {0}")]
    Task(String),
}

pub type EngineResult<T> = Result<T, EngineError>;

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident {
        $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

text_enum!(
    /// What a job does.
    JobKind {
        TaskRepeat => "task_repeat",
        FolderWatch => "folder_watch",
        PaperAlert => "paper_alert",
        Playbook => "playbook",
        DailyReview => "daily_review",
        System => "system",
    }
);

text_enum!(
    /// What happens when a job falls due while its previous run is still active.
    Overlap {
        /// Record that the previous run was still going.
        Skip => "skip",
        /// Run once more after it.
        QueueOne => "queue_one",
    }
);

text_enum!(
    /// What happens to the times a job missed while the computer slept or Shodh was
    /// closed.
    CatchUp {
        /// One run for the latest missed time.
        Once => "once",
        /// A skipped run is recorded.
        Skip => "skip",
    }
);

text_enum!(
    /// Why a job does not run until the user resumes it.
    HoldReason {
        User => "user",
        Failing => "failing",
        ModelUnavailable => "model_unavailable",
        Offline => "offline",
    }
);

text_enum!(
    /// Who made a job.
    CreatedBy {
        User => "user",
        Agent => "agent",
        System => "system",
    }
);

text_enum!(
    /// Why a run started.
    Trigger {
        Schedule => "schedule",
        CatchUp => "catch_up",
        Manual => "manual",
        Event => "event",
        Retry => "retry",
    }
);

text_enum!(
    /// Where a run stands.
    RunState {
        Queued => "queued",
        Running => "running",
        Waiting => "waiting",
        Succeeded => "succeeded",
        Failed => "failed",
        Skipped => "skipped",
        Cancelled => "cancelled",
    }
);

impl RunState {
    /// Queued, running or waiting: the job's one active run.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Waiting)
    }
}

text_enum!(
    /// What a waiting run waits for.
    WaitReason {
        Approval => "approval",
        /// The user's answer to a question the run asked.
        Decision => "decision",
        Yield => "yield",
        Quota => "quota",
        TableJob => "table_job",
        ModelUnavailable => "model_unavailable",
        Offline => "offline",
        /// The next rung of the backoff ladder.
        Backoff => "backoff",
    }
);

/// A job.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    /// What the job is about (for a repeating task: the latest copy's id).
    pub subject: String,
    pub title: String,
    pub schedule: Option<Schedule>,
    /// What the job's runs work from (for a repeating task: the latest copy's snapshot).
    pub input: Value,
    pub enabled: bool,
    pub overlap: Overlap,
    pub catch_up: CatchUp,
    pub max_attempts: u32,
    /// The next time the job is due (UTC, RFC 3339): a cache, rewritten on every plan.
    pub next_due_at: Option<String>,
    pub hold_reason: Option<HoldReason>,
    /// Failed runs in a row.
    pub failure_streak: u32,
    pub created_by: CreatedBy,
    pub provenance: Value,
    pub created_at: String,
    pub updated_at: String,
}

/// A new job.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: String,
    pub kind: JobKind,
    pub subject: String,
    pub title: String,
    pub schedule: Option<Schedule>,
    pub input: Value,
    pub overlap: Overlap,
    pub catch_up: CatchUp,
    pub max_attempts: u32,
    pub created_by: CreatedBy,
    pub provenance: Value,
}

/// Changes to a job; `None` keeps a field.
#[derive(Debug, Clone, Default)]
pub struct JobPatch {
    pub subject: Option<String>,
    pub title: Option<String>,
    pub schedule: Option<Schedule>,
    pub input: Option<Value>,
    pub hold_reason: Option<Option<HoldReason>>,
    pub next_due_at: Option<Option<String>>,
    pub reset_failures: bool,
    /// Switches the job off (its schedule and events start no run) or back on.
    pub enabled: Option<bool>,
}

/// A run of a job.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub job_id: String,
    pub trigger: Trigger,
    /// The occurrence the run is for (UTC, RFC 3339).
    pub scheduled_for: Option<String>,
    /// Earlier occurrences the run stands for (missed while asleep or waiting).
    pub missed: u32,
    pub state: RunState,
    pub wait_reason: Option<WaitReason>,
    /// Transient failures so far.
    pub attempt: u32,
    pub next_attempt_at: Option<String>,
    #[serde(skip_serializing)]
    pub run_token: Option<String>,
    pub heartbeat_at: Option<String>,
    /// The model calls the run planned: its estimate, which it never goes past.
    pub planned_calls: u32,
    /// The model calls it made.
    pub used_calls: u32,
    /// The conversation that holds what its agent did, when it had one.
    pub conversation_id: Option<String>,
    /// What the run did, in words.
    pub summary: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
}

impl Run {
    /// [`Run::scheduled_for`] as an instant.
    pub fn scheduled_at(&self) -> Option<DateTime<Utc>> {
        parse_instant(self.scheduled_for.as_deref()?)
    }
}

/// A new run.
#[derive(Debug, Clone)]
pub struct NewRun {
    pub id: String,
    pub job_id: String,
    pub trigger: Trigger,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub missed: u32,
    /// `queued`, or a finished state for a run recorded without running (`skipped`,
    /// `succeeded`).
    pub state: RunState,
    pub summary: Option<String>,
}

/// How a running run ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    Succeeded {
        summary: String,
        /// The occurrence the run turned out to be for, when it chose one.
        scheduled_for: Option<DateTime<Utc>>,
    },
    Skipped {
        summary: String,
    },
    Failed {
        code: String,
        detail: String,
    },
    Cancelled {
        summary: String,
    },
}

/// The instant a stored timestamp names.
pub fn parse_instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn bad_column(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, message.into())
}

fn as_count(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

fn parsed<T>(index: usize, text: String, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    parse(&text).ok_or_else(|| bad_column(index, format!("unknown value {text}")))
}

fn parsed_opt<T>(
    index: usize,
    text: Option<String>,
    parse: fn(&str) -> Option<T>,
) -> rusqlite::Result<Option<T>> {
    text.map(|t| parsed(index, t, parse)).transpose()
}

fn json_col(index: usize, text: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&text).map_err(|e| bad_column(index, e.to_string()))
}

pub(super) const JOB_COLS: &str =
    "id, kind, subject, title, schedule_json, input_json, enabled, overlap, \
     catch_up, max_attempts, next_due_at, hold_reason, failure_streak, created_by, \
     provenance_json, created_at, updated_at";

pub(super) fn job_from(r: &Row<'_>) -> rusqlite::Result<Job> {
    let schedule: Option<String> = r.get(4)?;
    let schedule = schedule
        .map(|text| {
            serde_json::from_str::<Schedule>(&text).map_err(|e| bad_column(4, e.to_string()))
        })
        .transpose()?;
    Ok(Job {
        id: r.get(0)?,
        kind: parsed(1, r.get(1)?, JobKind::parse)?,
        subject: r.get(2)?,
        title: r.get(3)?,
        schedule,
        input: json_col(5, r.get(5)?)?,
        enabled: r.get::<_, i64>(6)? != 0,
        overlap: parsed(7, r.get(7)?, Overlap::parse)?,
        catch_up: parsed(8, r.get(8)?, CatchUp::parse)?,
        max_attempts: as_count(r.get(9)?),
        next_due_at: r.get(10)?,
        hold_reason: parsed_opt(11, r.get(11)?, HoldReason::parse)?,
        failure_streak: as_count(r.get(12)?),
        created_by: parsed(13, r.get(13)?, CreatedBy::parse)?,
        provenance: json_col(14, r.get(14)?)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

const RUN_COLS: &str = "id, job_id, trigger_kind, scheduled_for, missed, state, wait_reason, \
     attempt, next_attempt_at, run_token, heartbeat_at, summary, error_code, error_detail, \
     started_at, ended_at, created_at, planned_calls, used_calls, conversation_id";

fn run_from(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        job_id: r.get(1)?,
        trigger: parsed(2, r.get(2)?, Trigger::parse)?,
        scheduled_for: r.get(3)?,
        missed: as_count(r.get(4)?),
        state: parsed(5, r.get(5)?, RunState::parse)?,
        wait_reason: parsed_opt(6, r.get(6)?, WaitReason::parse)?,
        attempt: as_count(r.get(7)?),
        next_attempt_at: r.get(8)?,
        run_token: r.get(9)?,
        heartbeat_at: r.get(10)?,
        summary: r.get(11)?,
        error_code: r.get(12)?,
        error_detail: r.get(13)?,
        started_at: r.get(14)?,
        ended_at: r.get(15)?,
        created_at: r.get(16)?,
        planned_calls: as_count(r.get(17)?),
        used_calls: as_count(r.get(18)?),
        conversation_id: r.get(19)?,
    })
}

pub(super) fn job_in(conn: &Connection, id: &str) -> rusqlite::Result<Option<Job>> {
    conn.query_row(
        &format!("SELECT {JOB_COLS} FROM engine_job WHERE id = ?1"),
        params![id],
        job_from,
    )
    .optional()
}

pub(super) fn run_in(conn: &Connection, id: &str) -> rusqlite::Result<Option<Run>> {
    conn.query_row(
        &format!("SELECT {RUN_COLS} FROM engine_run WHERE id = ?1"),
        params![id],
        run_from,
    )
    .optional()
}

pub(super) fn active_in(conn: &Connection, job_id: &str) -> rusqlite::Result<Option<Run>> {
    conn.query_row(
        &format!(
            "SELECT {RUN_COLS} FROM engine_run
              WHERE job_id = ?1 AND state IN ('queued', 'running', 'waiting')"
        ),
        params![job_id],
        run_from,
    )
    .optional()
}

fn schedule_json(schedule: &Schedule) -> EngineResult<String> {
    serde_json::to_string(schedule).map_err(|e| EngineError::Invalid(e.to_string()))
}

/// The engine's jobs and runs in `shodh.db`.
pub struct EngineStore {
    pub(super) conn: SharedConnection,
}

impl std::fmt::Debug for EngineStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineStore").finish_non_exhaustive()
    }
}

impl EngineStore {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> EngineResult<Self> {
        Ok(Self {
            conn: open_shared_connection(path, key)?,
        })
    }

    pub fn create_job(&self, new: &NewJob, now: DateTime<Utc>) -> EngineResult<Job> {
        if let Some(schedule) = &new.schedule {
            schedule
                .validate()
                .map_err(|e| EngineError::Invalid(e.to_string()))?;
        }
        if !(1..=5).contains(&new.max_attempts) {
            return Err(EngineError::Invalid(format!(
                "A job tries 1 to 5 times, not {}.",
                new.max_attempts
            )));
        }
        let schedule = new.schedule.as_ref().map(schedule_json).transpose()?;
        let at = timestamp(now);
        self.conn.write(|tx| -> EngineResult<Job> {
            tx.execute(
                "INSERT INTO engine_job (id, kind, subject, title, schedule_json, input_json,
                     enabled, overlap, catch_up, max_attempts, created_by, provenance_json,
                     created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                params![
                    new.id,
                    new.kind.as_str(),
                    new.subject,
                    new.title,
                    schedule,
                    new.input.to_string(),
                    new.overlap.as_str(),
                    new.catch_up.as_str(),
                    i64::from(new.max_attempts),
                    new.created_by.as_str(),
                    new.provenance.to_string(),
                    at,
                ],
            )?;
            job_in(tx, &new.id)?.ok_or_else(|| EngineError::NotFound(new.id.clone()))
        })
    }

    pub fn job(&self, id: &str) -> EngineResult<Option<Job>> {
        let conn = self.conn.read()?;
        Ok(job_in(&conn, id)?)
    }

    /// Every job of `kind`, oldest first.
    pub fn jobs(&self, kind: JobKind) -> EngineResult<Vec<Job>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLS} FROM engine_job WHERE kind = ?1 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![kind.as_str()], job_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn job_by_subject(&self, kind: JobKind, subject: &str) -> EngineResult<Option<Job>> {
        let conn = self.conn.read()?;
        Ok(conn
            .query_row(
                &format!("SELECT {JOB_COLS} FROM engine_job WHERE kind = ?1 AND subject = ?2"),
                params![kind.as_str(), subject],
                job_from,
            )
            .optional()?)
    }

    /// Applies `patch`; `None` when there is no such job.
    pub fn update_job(
        &self,
        id: &str,
        patch: &JobPatch,
        now: DateTime<Utc>,
    ) -> EngineResult<Option<Job>> {
        if let Some(schedule) = &patch.schedule {
            schedule
                .validate()
                .map_err(|e| EngineError::Invalid(e.to_string()))?;
        }
        let schedule = patch.schedule.as_ref().map(schedule_json).transpose()?;
        let at = timestamp(now);
        self.conn.write(|tx| -> EngineResult<Option<Job>> {
            if job_in(tx, id)?.is_none() {
                return Ok(None);
            }
            if let Some(subject) = &patch.subject {
                tx.execute(
                    "UPDATE engine_job SET subject = ?2 WHERE id = ?1",
                    params![id, subject],
                )?;
            }
            if let Some(title) = &patch.title {
                tx.execute(
                    "UPDATE engine_job SET title = ?2 WHERE id = ?1",
                    params![id, title],
                )?;
            }
            if let Some(schedule) = &schedule {
                tx.execute(
                    "UPDATE engine_job SET schedule_json = ?2 WHERE id = ?1",
                    params![id, schedule],
                )?;
            }
            if let Some(input) = &patch.input {
                tx.execute(
                    "UPDATE engine_job SET input_json = ?2 WHERE id = ?1",
                    params![id, input.to_string()],
                )?;
            }
            if let Some(hold) = &patch.hold_reason {
                tx.execute(
                    "UPDATE engine_job SET hold_reason = ?2 WHERE id = ?1",
                    params![id, hold.map(HoldReason::as_str)],
                )?;
            }
            if let Some(next) = &patch.next_due_at {
                tx.execute(
                    "UPDATE engine_job SET next_due_at = ?2 WHERE id = ?1",
                    params![id, next],
                )?;
            }
            if patch.reset_failures {
                tx.execute(
                    "UPDATE engine_job SET failure_streak = 0 WHERE id = ?1",
                    params![id],
                )?;
            }
            if let Some(enabled) = patch.enabled {
                tx.execute(
                    "UPDATE engine_job SET enabled = ?2 WHERE id = ?1",
                    params![id, i64::from(enabled)],
                )?;
            }
            tx.execute(
                "UPDATE engine_job SET updated_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
            Ok(job_in(tx, id)?)
        })
    }

    /// Deletes a job and its runs, with their steps and approvals, and its cached step
    /// results. True when it was there.
    pub fn delete_job(&self, id: &str) -> EngineResult<bool> {
        self.conn.write(|tx| -> EngineResult<bool> {
            tx.execute(
                "DELETE FROM engine_step
                  WHERE run_id IN (SELECT id FROM engine_run WHERE job_id = ?1)",
                params![id],
            )?;
            tx.execute("DELETE FROM engine_approval WHERE job_id = ?1", params![id])?;
            tx.execute("DELETE FROM engine_cache WHERE job_id = ?1", params![id])?;
            tx.execute("DELETE FROM engine_run WHERE job_id = ?1", params![id])?;
            Ok(tx.execute("DELETE FROM engine_job WHERE id = ?1", params![id])? > 0)
        })
    }

    /// Counts a failed run; returns the failed runs in a row.
    pub fn record_failure(&self, id: &str) -> EngineResult<u32> {
        self.conn.write(|tx| -> EngineResult<u32> {
            tx.execute(
                "UPDATE engine_job SET failure_streak = failure_streak + 1 WHERE id = ?1",
                params![id],
            )?;
            let streak: Option<i64> = tx
                .query_row(
                    "SELECT failure_streak FROM engine_job WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(streak.map_or(0, as_count))
        })
    }

    /// Inserts a run. A queued run of a job that has an active run is not inserted: the
    /// active run is returned as `Err`.
    pub fn insert_run(&self, new: &NewRun, now: DateTime<Utc>) -> EngineResult<Result<Run, Run>> {
        let at = timestamp(now);
        let scheduled = new.scheduled_for.map(timestamp);
        let ended = (!new.state.is_active()).then(|| at.clone());
        self.conn.write(|tx| -> EngineResult<Result<Run, Run>> {
            if new.state.is_active() {
                if let Some(active) = active_in(tx, &new.job_id)? {
                    return Ok(Err(active));
                }
            }
            tx.execute(
                "INSERT INTO engine_run (id, job_id, trigger_kind, scheduled_for, missed, state,
                     summary, ended_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    new.id,
                    new.job_id,
                    new.trigger.as_str(),
                    scheduled,
                    i64::from(new.missed),
                    new.state.as_str(),
                    new.summary,
                    ended,
                    at,
                ],
            )?;
            let run = run_in(tx, &new.id)?.ok_or_else(|| EngineError::NotFound(new.id.clone()))?;
            Ok(Ok(run))
        })
    }

    pub fn run(&self, id: &str) -> EngineResult<Option<Run>> {
        let conn = self.conn.read()?;
        Ok(run_in(&conn, id)?)
    }

    /// The job's active run.
    pub fn active_run(&self, job_id: &str) -> EngineResult<Option<Run>> {
        let conn = self.conn.read()?;
        Ok(active_in(&conn, job_id)?)
    }

    /// The job's runs, newest first, at most `limit`.
    pub fn runs(&self, job_id: &str, limit: usize) -> EngineResult<Vec<Run>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM engine_run WHERE job_id = ?1
              ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![job_id, limit], run_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The latest occurrence the job's runs stand for.
    pub fn latest_scheduled_for(&self, job_id: &str) -> EngineResult<Option<DateTime<Utc>>> {
        let conn = self.conn.read()?;
        let latest: Option<String> = conn.query_row(
            "SELECT MAX(scheduled_for) FROM engine_run WHERE job_id = ?1",
            params![job_id],
            |r| r.get(0),
        )?;
        Ok(latest.as_deref().and_then(parse_instant))
    }

    /// The earliest time a waiting run of an unheld, enabled job of `kind` is due again.
    pub fn next_retry_at(&self, kind: JobKind) -> EngineResult<Option<DateTime<Utc>>> {
        let conn = self.conn.read()?;
        let next: Option<String> = conn.query_row(
            "SELECT MIN(r.next_attempt_at) FROM engine_run r JOIN engine_job j ON j.id = r.job_id
              WHERE r.state = 'waiting' AND r.next_attempt_at IS NOT NULL
                AND j.enabled = 1 AND j.hold_reason IS NULL AND j.kind = ?1",
            params![kind.as_str()],
            |r| r.get(0),
        )?;
        Ok(next.as_deref().and_then(parse_instant))
    }

    /// Claims a run for the caller holding `token`. A run is claimable when it is queued;
    /// waiting with a retry time at or before `now` (or any waiting run, when `wake` is
    /// set: the user answered it); or running under `token` with no live run in this
    /// process (`live_here` false), or under another token whose heartbeat is at or before
    /// `cutoff`. Returns the claimed run.
    pub fn claim(
        &self,
        id: &str,
        token: &str,
        live_here: bool,
        wake: bool,
        now: DateTime<Utc>,
        cutoff: &str,
    ) -> EngineResult<Option<Run>> {
        let at = timestamp(now);
        self.conn.write(|tx| -> EngineResult<Option<Run>> {
            let Some(run) = run_in(tx, id)? else {
                return Ok(None);
            };
            let claimable = match run.state {
                RunState::Queued => true,
                RunState::Waiting => {
                    wake || run
                        .next_attempt_at
                        .as_deref()
                        .is_some_and(|next| next <= at.as_str())
                }
                RunState::Running => {
                    if run.run_token.as_deref() == Some(token) {
                        !live_here
                    } else {
                        run.heartbeat_at
                            .as_deref()
                            .is_none_or(|beat| beat <= cutoff)
                    }
                }
                _ => false,
            };
            if !claimable {
                return Ok(None);
            }
            tx.execute(
                "UPDATE engine_run SET state = 'running', run_token = ?2, heartbeat_at = ?3,
                     wait_reason = NULL, next_attempt_at = NULL,
                     started_at = COALESCE(started_at, ?3)
                  WHERE id = ?1",
                params![id, token, at],
            )?;
            Ok(run_in(tx, id)?)
        })
    }

    /// Renews the run's lease. True while the run holding `token` owns it.
    pub fn beat(&self, id: &str, token: &str, now: DateTime<Utc>) -> EngineResult<bool> {
        let at = timestamp(now);
        let renewed = self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_run SET heartbeat_at = ?3
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![id, token, at],
            )
        })?;
        Ok(renewed > 0)
    }

    /// Puts the running run holding `token` in wait for `reason`, until `until` (none: until
    /// it is answered), with its transient failures so far and the last error.
    #[allow(clippy::too_many_arguments)]
    pub fn wait(
        &self,
        id: &str,
        token: &str,
        reason: WaitReason,
        until: Option<DateTime<Utc>>,
        attempt: u32,
        error: Option<(&str, &str)>,
    ) -> EngineResult<Option<Run>> {
        let until = until.map(timestamp);
        let (code, detail) = error.unzip();
        self.conn.write(|tx| -> EngineResult<Option<Run>> {
            let changed = tx.execute(
                "UPDATE engine_run SET state = 'waiting', run_token = NULL, wait_reason = ?3,
                     next_attempt_at = ?4, attempt = ?5,
                     error_code = COALESCE(?6, error_code),
                     error_detail = COALESCE(?7, error_detail)
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![
                    id,
                    token,
                    reason.as_str(),
                    until,
                    i64::from(attempt),
                    code,
                    detail
                ],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(run_in(tx, id)?)
        })
    }

    /// Ends the running run holding `token`. `None` when the run no longer owns it.
    pub fn finish(
        &self,
        id: &str,
        token: &str,
        end: &End,
        now: DateTime<Utc>,
    ) -> EngineResult<Option<Run>> {
        let at = timestamp(now);
        let (state, summary, code, detail, scheduled) = match end {
            End::Succeeded {
                summary,
                scheduled_for,
            } => (
                RunState::Succeeded,
                Some(summary.as_str()),
                None,
                None,
                scheduled_for.map(timestamp),
            ),
            End::Skipped { summary } => {
                (RunState::Skipped, Some(summary.as_str()), None, None, None)
            }
            End::Cancelled { summary } => (
                RunState::Cancelled,
                Some(summary.as_str()),
                None,
                None,
                None,
            ),
            End::Failed { code, detail } => (
                RunState::Failed,
                None,
                Some(code.as_str()),
                Some(detail.as_str()),
                None,
            ),
        };
        self.conn.write(|tx| -> EngineResult<Option<Run>> {
            let changed = tx.execute(
                "UPDATE engine_run SET state = ?3, run_token = NULL, wait_reason = NULL,
                     next_attempt_at = NULL, summary = COALESCE(?4, summary),
                     error_code = COALESCE(?5, error_code),
                     error_detail = COALESCE(?6, error_detail),
                     scheduled_for = COALESCE(?7, scheduled_for), ended_at = ?8
                  WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![
                    id,
                    token,
                    state.as_str(),
                    summary,
                    code,
                    detail,
                    scheduled,
                    at
                ],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(run_in(tx, id)?)
        })
    }

    /// Deletes a run that did nothing worth recording (it found nothing to do).
    pub fn delete_run(&self, id: &str, token: &str) -> EngineResult<bool> {
        let removed = self.conn.write(|tx| {
            tx.execute(
                "DELETE FROM engine_run WHERE id = ?1 AND state = 'running' AND run_token = ?2",
                params![id, token],
            )
        })?;
        Ok(removed > 0)
    }

    /// Moves a waiting run to a newer occurrence: `scheduled_for` becomes `to`, and the
    /// occurrences it now also stands for are added to `missed`. No run is added.
    pub fn fold(&self, id: &str, to: DateTime<Utc>, add_missed: u32) -> EngineResult<Option<Run>> {
        let to = timestamp(to);
        self.conn.write(|tx| -> EngineResult<Option<Run>> {
            let changed = tx.execute(
                "UPDATE engine_run SET scheduled_for = ?2, missed = missed + ?3
                  WHERE id = ?1 AND state = 'waiting'",
                params![id, to, i64::from(add_missed)],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(run_in(tx, id)?)
        })
    }

    /// Ends a waiting run that will never be answered (its job stopped), from outside any
    /// run. True when it was waiting.
    pub fn cancel_waiting(
        &self,
        id: &str,
        summary: &str,
        now: DateTime<Utc>,
    ) -> EngineResult<bool> {
        let at = timestamp(now);
        let changed = self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_run SET state = 'cancelled', wait_reason = NULL,
                     next_attempt_at = NULL, summary = ?2, ended_at = ?3
                  WHERE id = ?1 AND state = 'waiting'",
                params![id, summary, at],
            )
        })?;
        Ok(changed > 0)
    }

    /// Deletes finished runs older than [`RETENTION_DAYS`], keeping the latest
    /// [`KEEP_LATEST_RUNS`] of each job; finished runs of a system job older than
    /// [`SYSTEM_RETENTION_DAYS`], keeping its newest; the steps and approvals of runs that
    /// are gone; and the cached step results older than
    /// [`CACHE_RETENTION_DAYS`](super::cache::CACHE_RETENTION_DAYS). Returns how many runs
    /// went.
    pub fn prune(&self, now: DateTime<Utc>) -> EngineResult<usize> {
        let cutoff = timestamp(now - TimeDelta::days(RETENTION_DAYS));
        let system_cutoff = timestamp(now - TimeDelta::days(SYSTEM_RETENTION_DAYS));
        let cache_cutoff = timestamp(now - TimeDelta::days(super::cache::CACHE_RETENTION_DAYS));
        let removed = self.conn.write(|tx| -> rusqlite::Result<usize> {
            tx.execute(
                "DELETE FROM engine_cache WHERE created_at < ?1",
                params![cache_cutoff],
            )?;
            let removed = tx.execute(
                "DELETE FROM engine_run WHERE rowid IN (
                    SELECT r.rowid FROM (
                        SELECT rowid, job_id, state, created_at, ROW_NUMBER() OVER (
                            PARTITION BY job_id ORDER BY created_at DESC, rowid DESC
                        ) AS n FROM engine_run
                    ) r LEFT JOIN engine_job j ON j.id = r.job_id
                     WHERE r.state NOT IN ('queued', 'running', 'waiting')
                       AND CASE WHEN j.kind = ?4
                                THEN r.created_at < ?3 AND r.n > 1
                                ELSE r.created_at < ?1 AND r.n > ?2
                           END
                 )",
                params![
                    cutoff,
                    KEEP_LATEST_RUNS,
                    system_cutoff,
                    JobKind::System.as_str()
                ],
            )?;
            if removed > 0 {
                tx.execute(
                    "DELETE FROM engine_step WHERE run_id NOT IN (SELECT id FROM engine_run)",
                    [],
                )?;
                tx.execute(
                    "DELETE FROM engine_approval WHERE run_id NOT IN (SELECT id FROM engine_run)",
                    [],
                )?;
            }
            Ok(removed)
        })?;
        Ok(removed)
    }

    /// Sets the model calls a run plans (its estimate).
    pub fn set_planned(&self, id: &str, calls: u32) -> EngineResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_run SET planned_calls = ?2 WHERE id = ?1",
                params![id, i64::from(calls)],
            )
        })?;
        Ok(())
    }

    /// Counts `calls` more model calls of a run; returns its calls so far.
    pub fn add_used(&self, id: &str, calls: u32) -> EngineResult<u32> {
        let used = self.conn.write(|tx| -> rusqlite::Result<Option<i64>> {
            tx.execute(
                "UPDATE engine_run SET used_calls = used_calls + ?2 WHERE id = ?1",
                params![id, i64::from(calls)],
            )?;
            tx.query_row(
                "SELECT used_calls FROM engine_run WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
        })?;
        Ok(used.map_or(0, as_count))
    }

    /// Removes a job's schedule: from now on it runs only when started or by an event.
    pub fn clear_schedule(&self, id: &str) -> EngineResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_job SET schedule_json = NULL, next_due_at = NULL WHERE id = ?1",
                params![id],
            )
        })?;
        Ok(())
    }

    /// Makes a job without a schedule due at `at`, or sooner when it is due sooner
    /// already: a due time set meanwhile is never pushed back. True when the job is there
    /// and has no schedule (a scheduled job's due time is its schedule's).
    pub fn set_due_at_most(&self, id: &str, at: DateTime<Utc>) -> EngineResult<bool> {
        let at = timestamp(at);
        let changed = self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_job
                    SET next_due_at = CASE
                        WHEN next_due_at IS NULL OR next_due_at > ?2 THEN ?2
                        ELSE next_due_at END
                  WHERE id = ?1 AND schedule_json IS NULL",
                params![id, at],
            )
        })?;
        Ok(changed > 0)
    }

    /// Takes the due time of a job without a schedule when it has come (at or before
    /// `now`): it is cleared and returned, so it starts one run. `None` when it has not
    /// come, or the job has none.
    pub fn take_due(&self, id: &str, now: DateTime<Utc>) -> EngineResult<Option<DateTime<Utc>>> {
        let now = timestamp(now);
        let due = self.conn.write(|tx| -> rusqlite::Result<Option<String>> {
            let due: Option<String> = tx
                .query_row(
                    "SELECT next_due_at FROM engine_job
                      WHERE id = ?1 AND schedule_json IS NULL
                        AND next_due_at IS NOT NULL AND next_due_at <= ?2",
                    params![id, now],
                    |r| r.get(0),
                )
                .optional()?;
            if due.is_some() {
                tx.execute(
                    "UPDATE engine_job SET next_due_at = NULL WHERE id = ?1",
                    params![id],
                )?;
            }
            Ok(due)
        })?;
        Ok(due.as_deref().and_then(parse_instant))
    }

    /// Records the conversation that holds what a run's agent did.
    pub fn set_conversation(&self, id: &str, conversation_id: &str) -> EngineResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE engine_run SET conversation_id = ?2 WHERE id = ?1",
                params![id, conversation_id],
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::lease::{lease_cutoff, LEASE_MINUTES};
    use chrono::{NaiveTime, TimeZone};
    use serde_json::json;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .unwrap()
    }

    fn store(dir: &tempfile::TempDir) -> EngineStore {
        EngineStore::open(&dir.path().join("shodh.db"), None).expect("open")
    }

    fn job(store: &EngineStore, id: &str) -> Job {
        store
            .create_job(
                &NewJob {
                    id: id.to_string(),
                    kind: JobKind::TaskRepeat,
                    subject: format!("task-{id}"),
                    title: "Submit timesheet".to_string(),
                    schedule: Some(Schedule::Daily {
                        at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    }),
                    input: json!({ "title": "Submit timesheet" }),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by: CreatedBy::User,
                    provenance: json!({}),
                },
                at(1, 8, 0),
            )
            .unwrap()
    }

    #[test]
    fn a_job_can_be_switched_off_and_on() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = job(&store, "j1");
        assert!(created.enabled);
        let off = JobPatch {
            enabled: Some(false),
            ..JobPatch::default()
        };
        let job = store.update_job("j1", &off, at(1, 9, 0)).unwrap().unwrap();
        assert!(!job.enabled);
        assert_eq!(job.title, created.title, "nothing else changes");
        let on = JobPatch {
            enabled: Some(true),
            ..JobPatch::default()
        };
        assert!(
            store
                .update_job("j1", &on, at(1, 9, 5))
                .unwrap()
                .unwrap()
                .enabled
        );
    }

    fn queued(job_id: &str, id: &str) -> NewRun {
        NewRun {
            id: id.to_string(),
            job_id: job_id.to_string(),
            trigger: Trigger::Schedule,
            scheduled_for: Some(at(1, 9, 0)),
            missed: 0,
            state: RunState::Queued,
            summary: None,
        }
    }

    #[test]
    fn jobs_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let made = job(&s, "j1");
        assert_eq!(s.job("j1").unwrap(), Some(made.clone()));
        assert_eq!(
            s.job_by_subject(JobKind::TaskRepeat, "task-j1").unwrap(),
            Some(made.clone())
        );
        assert_eq!(s.jobs(JobKind::TaskRepeat).unwrap(), vec![made]);
        assert!(s.jobs(JobKind::Playbook).unwrap().is_empty());
        let patched = s
            .update_job(
                "j1",
                &JobPatch {
                    subject: Some("task-2".into()),
                    hold_reason: Some(Some(HoldReason::Failing)),
                    ..JobPatch::default()
                },
                at(2, 8, 0),
            )
            .unwrap()
            .unwrap();
        assert_eq!(patched.subject, "task-2");
        assert_eq!(patched.hold_reason, Some(HoldReason::Failing));
        assert!(s
            .update_job("nope", &JobPatch::default(), at(2, 8, 0))
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_live_lease_is_never_taken_and_a_stale_one_is_after_ten_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        job(&s, "j1");
        s.insert_run(&queued("j1", "r1"), at(1, 9, 0))
            .unwrap()
            .unwrap();
        let start = at(1, 9, 0);
        assert!(s
            .claim("r1", "a", false, false, start, &lease_cutoff(start))
            .unwrap()
            .is_some());
        let almost = start + TimeDelta::minutes(LEASE_MINUTES) - TimeDelta::seconds(1);
        assert!(
            s.claim("r1", "b", false, false, almost, &lease_cutoff(almost))
                .unwrap()
                .is_none(),
            "a live lease of another process is never taken"
        );
        assert!(
            s.claim("r1", "a", true, false, almost, &lease_cutoff(almost))
                .unwrap()
                .is_none(),
            "a run live in this process is not claimed twice"
        );
        let stale = start + TimeDelta::minutes(LEASE_MINUTES);
        let taken = s
            .claim("r1", "b", false, false, stale, &lease_cutoff(stale))
            .unwrap()
            .unwrap();
        assert_eq!(taken.run_token.as_deref(), Some("b"));
        assert!(!s.beat("r1", "a", stale).unwrap(), "the old holder lost it");
        assert!(s.beat("r1", "b", stale).unwrap());
    }

    #[test]
    fn a_second_active_run_returns_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        job(&s, "j1");
        let first = s
            .insert_run(&queued("j1", "r1"), at(1, 9, 0))
            .unwrap()
            .unwrap();
        let second = s.insert_run(&queued("j1", "r2"), at(1, 9, 1)).unwrap();
        assert_eq!(second, Err(first.clone()));
        let skipped = NewRun {
            state: RunState::Skipped,
            summary: Some("The previous run was still going".into()),
            ..queued("j1", "r3")
        };
        let recorded = s.insert_run(&skipped, at(1, 9, 2)).unwrap().unwrap();
        assert_eq!(recorded.state, RunState::Skipped);
        assert!(recorded.ended_at.is_some());
        assert_eq!(s.active_run("j1").unwrap(), Some(first));
    }

    #[test]
    fn deleting_a_job_deletes_its_runs() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        job(&s, "j1");
        job(&s, "j2");
        s.insert_run(&queued("j1", "r1"), at(1, 9, 0))
            .unwrap()
            .unwrap();
        s.insert_run(&queued("j2", "r2"), at(1, 9, 0))
            .unwrap()
            .unwrap();
        assert!(s.delete_job("j1").unwrap());
        assert!(!s.delete_job("j1").unwrap());
        assert!(s.run("r1").unwrap().is_none());
        assert!(s.run("r2").unwrap().is_some());
    }

    #[test]
    fn retention_keeps_ninety_days_and_never_fewer_than_twenty_per_job() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        job(&s, "old");
        job(&s, "busy");
        let long_ago = at(1, 9, 0) - TimeDelta::days(200);
        // 25 old runs of one job: the latest 20 stay.
        for i in 0..25 {
            let run = NewRun {
                state: RunState::Succeeded,
                ..queued("old", &format!("old-{i:02}"))
            };
            s.insert_run(&run, long_ago + TimeDelta::minutes(i))
                .unwrap()
                .unwrap();
        }
        // 30 runs of another: 25 old ones and 5 recent; the 5 recent and 15 more stay.
        for i in 0..25 {
            let run = NewRun {
                state: RunState::Skipped,
                ..queued("busy", &format!("busy-old-{i:02}"))
            };
            s.insert_run(&run, long_ago + TimeDelta::minutes(i))
                .unwrap()
                .unwrap();
        }
        for i in 0..5 {
            let run = NewRun {
                state: RunState::Succeeded,
                ..queued("busy", &format!("busy-new-{i}"))
            };
            s.insert_run(&run, at(1, 9, 0) + TimeDelta::minutes(i))
                .unwrap()
                .unwrap();
        }
        let removed = s.prune(at(2, 9, 0)).unwrap();
        assert_eq!(removed, 5 + 10);
        assert_eq!(s.runs("old", 100).unwrap().len(), 20);
        let busy = s.runs("busy", 100).unwrap();
        assert_eq!(busy.len(), 20);
        assert!(busy.iter().any(|r| r.id == "busy-new-0"));
        assert!(s.run("old-00").unwrap().is_none());
        assert!(s.run("old-24").unwrap().is_some());
    }

    fn system_job(store: &EngineStore, id: &str) -> Job {
        store
            .create_job(
                &NewJob {
                    id: id.to_string(),
                    kind: JobKind::System,
                    subject: id.to_string(),
                    title: id.to_string(),
                    schedule: None,
                    input: json!({ "system": id }),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by: CreatedBy::System,
                    provenance: json!({}),
                },
                at(1, 8, 0),
            )
            .unwrap()
    }

    #[test]
    fn a_system_job_keeps_seven_days_of_runs_and_always_its_newest() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        system_job(&s, "table_tick");
        system_job(&s, "quiet");
        job(&s, "user");
        let now = at(20, 9, 0);
        let eight_days = now - TimeDelta::days(8);
        for i in 0..5 {
            let run = NewRun {
                state: RunState::Succeeded,
                ..queued("table_tick", &format!("tick-old-{i}"))
            };
            s.insert_run(&run, eight_days + TimeDelta::minutes(i))
                .unwrap()
                .unwrap();
        }
        for i in 0..3 {
            let run = NewRun {
                state: RunState::Succeeded,
                ..queued("table_tick", &format!("tick-new-{i}"))
            };
            s.insert_run(&run, now - TimeDelta::days(6) + TimeDelta::minutes(i))
                .unwrap()
                .unwrap();
        }
        for i in 0..3 {
            let run = NewRun {
                state: RunState::Succeeded,
                ..queued("quiet", &format!("quiet-{i}"))
            };
            s.insert_run(&run, eight_days + TimeDelta::minutes(i))
                .unwrap()
                .unwrap();
        }
        let user = NewRun {
            state: RunState::Succeeded,
            ..queued("user", "user-old")
        };
        s.insert_run(&user, eight_days).unwrap().unwrap();
        assert_eq!(
            s.prune(now).unwrap(),
            5 + 2,
            "old system runs go, the user's stays"
        );
        let ticks: Vec<String> = s
            .runs("table_tick", 100)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ticks, ["tick-new-2", "tick-new-1", "tick-new-0"]);
        let quiet: Vec<String> = s
            .runs("quiet", 100)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(quiet, ["quiet-2"], "a system job keeps its newest run");
        assert!(s.run("user-old").unwrap().is_some());
    }

    #[test]
    fn an_owner_set_due_time_is_only_ever_brought_forward_and_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        system_job(&s, "reminder_plan");
        job(&s, "scheduled");
        assert!(s.set_due_at_most("reminder_plan", at(1, 10, 0)).unwrap());
        assert!(s.set_due_at_most("reminder_plan", at(1, 11, 0)).unwrap());
        assert_eq!(
            s.job("reminder_plan")
                .unwrap()
                .unwrap()
                .next_due_at
                .as_deref(),
            Some(timestamp(at(1, 10, 0)).as_str()),
            "a later time never pushes a due time back"
        );
        assert!(s.set_due_at_most("reminder_plan", at(1, 9, 30)).unwrap());
        assert!(
            !s.set_due_at_most("scheduled", at(1, 9, 0)).unwrap(),
            "a scheduled job's due time is its schedule's"
        );
        assert!(!s.set_due_at_most("nope", at(1, 9, 0)).unwrap());
        assert_eq!(s.take_due("reminder_plan", at(1, 9, 29)).unwrap(), None);
        assert_eq!(
            s.take_due("reminder_plan", at(1, 9, 31)).unwrap(),
            Some(at(1, 9, 30))
        );
        assert_eq!(
            s.take_due("reminder_plan", at(1, 9, 32)).unwrap(),
            None,
            "taken once"
        );
        assert_eq!(s.job("reminder_plan").unwrap().unwrap().next_due_at, None);
    }

    #[test]
    fn fold_moves_to_the_newest_occurrence_and_counts_missed() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        job(&s, "j1");
        s.insert_run(&queued("j1", "r1"), at(1, 9, 0))
            .unwrap()
            .unwrap();
        let now = at(1, 9, 0);
        s.claim("r1", "a", false, false, now, &lease_cutoff(now))
            .unwrap()
            .unwrap();
        s.wait("r1", "a", WaitReason::Decision, None, 0, None)
            .unwrap()
            .unwrap();
        let folded = s.fold("r1", at(3, 9, 0), 2).unwrap().unwrap();
        assert_eq!(folded.scheduled_at(), Some(at(3, 9, 0)));
        assert_eq!(folded.missed, 2);
        assert_eq!(folded.state, RunState::Waiting);
        assert_eq!(s.runs("j1", 10).unwrap().len(), 1, "no run is added");
        assert_eq!(s.latest_scheduled_for("j1").unwrap(), Some(at(3, 9, 0)));
    }

    #[test]
    fn three_failures_in_a_row_are_counted_and_reset_by_success() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        job(&s, "j1");
        assert_eq!(s.record_failure("j1").unwrap(), 1);
        assert_eq!(s.record_failure("j1").unwrap(), 2);
        assert_eq!(s.record_failure("j1").unwrap(), 3);
        let reset = s
            .update_job(
                "j1",
                &JobPatch {
                    reset_failures: true,
                    ..JobPatch::default()
                },
                at(2, 9, 0),
            )
            .unwrap()
            .unwrap();
        assert_eq!(reset.failure_streak, 0);
    }

    #[test]
    fn a_waiting_run_is_claimed_when_due_or_answered() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        job(&s, "j1");
        s.insert_run(&queued("j1", "r1"), at(1, 9, 0))
            .unwrap()
            .unwrap();
        let now = at(1, 9, 0);
        s.claim("r1", "a", false, false, now, &lease_cutoff(now))
            .unwrap()
            .unwrap();
        s.wait(
            "r1",
            "a",
            WaitReason::Backoff,
            Some(at(1, 9, 5)),
            1,
            Some(("calendar_unreadable", "locked")),
        )
        .unwrap()
        .unwrap();
        let early = at(1, 9, 4);
        assert!(s
            .claim("r1", "a", false, false, early, &lease_cutoff(early))
            .unwrap()
            .is_none());
        let due = at(1, 9, 5);
        let run = s
            .claim("r1", "a", false, false, due, &lease_cutoff(due))
            .unwrap()
            .unwrap();
        assert_eq!(run.attempt, 1);
        assert_eq!(run.error_code.as_deref(), Some("calendar_unreadable"));
        s.wait("r1", "a", WaitReason::Decision, None, 1, None)
            .unwrap()
            .unwrap();
        assert!(s
            .claim("r1", "a", false, false, due, &lease_cutoff(due))
            .unwrap()
            .is_none());
        assert!(s
            .claim("r1", "a", false, true, due, &lease_cutoff(due))
            .unwrap()
            .is_some());
        let done = s
            .finish(
                "r1",
                "a",
                &End::Succeeded {
                    summary: "Made".into(),
                    scheduled_for: Some(at(2, 9, 0)),
                },
                due,
            )
            .unwrap()
            .unwrap();
        assert_eq!(done.state, RunState::Succeeded);
        assert_eq!(done.scheduled_at(), Some(at(2, 9, 0)));
        assert!(s.active_run("j1").unwrap().is_none());
    }
}
