//! The suggestions inbox (`memory_proposals` in `shodh.db`, schema version 4) and the
//! learning model's daily usage.
//!
//! Status transitions ([`ProposalStatus::after`]):
//!
//! ```text
//! pending ──accept──▶ accepted ──undo──▶ undone
//!    │ ╲──auto────▶ learned  ──undo──▶ undone
//!    │  ╲─reject──▶ rejected
//!    ╰────stale───▶ stale      (the memory it changes was changed or removed)
//! accepted/learned ──fail──▶ failed ──accept──▶ accepted   (the write failed; retry)
//! ```
//!
//! Every transition is a single `UPDATE ... WHERE status IN (...)` in an `IMMEDIATE`
//! transaction, so a suggestion is applied at most once even when two windows accept it.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::decide::{DecidedBy, Decision};
use super::sensitivity::SensitiveReason;
use super::{day_key, LearnCaps, LearnError, LearnResult};
use crate::audit::{open_shared_connection, AuditKey};
use crate::statements::{PutOutcome, Scope};
use crate::user_memory::MemoryContent;

/// How long a rejected (or undone) suggestion suppresses the same suggestion.
pub const REJECTION_MEMORY_DAYS: i64 = 180;

/// What a suggestion changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// Store a memory (add, reinforce, extend, supersede or refine).
    Remember,
    /// A new version of an existing memory (evolution).
    Revise,
    /// Link two memories.
    Link,
    /// Resolve a contradiction: close one memory in favour of another.
    Resolve,
    /// Archive a faded memory (reversible).
    Archive,
}

impl ProposalKind {
    fn as_str(self) -> &'static str {
        match self {
            ProposalKind::Remember => "remember",
            ProposalKind::Revise => "revise",
            ProposalKind::Link => "link",
            ProposalKind::Resolve => "resolve",
            ProposalKind::Archive => "archive",
        }
    }
}

/// Which step produced a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOrigin {
    /// Extraction from a conversation turn.
    Turn,
    /// Evolution of neighbours after a memory changed.
    Evolve,
    /// Consolidation.
    Consolidate,
}

impl ProposalOrigin {
    fn as_str(self) -> &'static str {
        match self {
            ProposalOrigin::Turn => "turn",
            ProposalOrigin::Evolve => "evolve",
            ProposalOrigin::Consolidate => "consolidate",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "turn" => ProposalOrigin::Turn,
            "evolve" => ProposalOrigin::Evolve,
            "consolidate" => ProposalOrigin::Consolidate,
            _ => return None,
        })
    }
}

/// Where a suggestion is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Waiting for the user.
    Pending,
    /// The user accepted it and it was applied.
    Accepted,
    /// The user rejected it.
    Rejected,
    /// Applied automatically under the user's policy.
    Learned,
    /// Applied, then undone.
    Undone,
    /// No longer applicable.
    Stale,
    /// Applying it failed; it can be accepted again.
    Failed,
}

/// An event that moves a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusEvent {
    /// The user accepts.
    Accept,
    /// The user rejects.
    Reject,
    /// The policy applies it.
    AutoApply,
    /// The user undoes an applied suggestion.
    Undo,
    /// It no longer applies.
    Stale,
    /// The write failed.
    Fail,
}

impl StatusEvent {
    fn verb(self) -> &'static str {
        match self {
            StatusEvent::Accept => "accepted",
            StatusEvent::Reject => "rejected",
            StatusEvent::AutoApply => "applied automatically",
            StatusEvent::Undo => "undone",
            StatusEvent::Stale => "marked stale",
            StatusEvent::Fail => "marked failed",
        }
    }
}

impl ProposalStatus {
    /// Stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Accepted => "accepted",
            ProposalStatus::Rejected => "rejected",
            ProposalStatus::Learned => "learned",
            ProposalStatus::Undone => "undone",
            ProposalStatus::Stale => "stale",
            ProposalStatus::Failed => "failed",
        }
    }

    /// Parses [`ProposalStatus::as_str`].
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pending" => ProposalStatus::Pending,
            "accepted" => ProposalStatus::Accepted,
            "rejected" => ProposalStatus::Rejected,
            "learned" => ProposalStatus::Learned,
            "undone" => ProposalStatus::Undone,
            "stale" => ProposalStatus::Stale,
            "failed" => ProposalStatus::Failed,
            _ => return None,
        })
    }

    /// The status after `event`, or the refusal.
    pub fn after(self, event: StatusEvent) -> LearnResult<ProposalStatus> {
        use ProposalStatus as S;
        use StatusEvent as E;
        let next = match (self, event) {
            (S::Pending | S::Failed, E::Accept) => S::Accepted,
            (S::Pending, E::Reject) | (S::Failed, E::Reject) => S::Rejected,
            (S::Pending, E::AutoApply) => S::Learned,
            (S::Accepted | S::Learned, E::Undo) => S::Undone,
            (S::Pending | S::Accepted | S::Learned | S::Failed, E::Stale) => S::Stale,
            (S::Accepted | S::Learned, E::Fail) => S::Failed,
            (from, event) => {
                return Err(LearnError::InvalidTransition {
                    from,
                    action: event.verb(),
                })
            }
        };
        Ok(next)
    }

    /// Statuses from which `event` is allowed.
    fn sources(event: StatusEvent) -> Vec<ProposalStatus> {
        [
            ProposalStatus::Pending,
            ProposalStatus::Accepted,
            ProposalStatus::Rejected,
            ProposalStatus::Learned,
            ProposalStatus::Undone,
            ProposalStatus::Stale,
            ProposalStatus::Failed,
        ]
        .into_iter()
        .filter(|s| s.after(event).is_ok())
        .collect()
    }
}

impl std::fmt::Display for ProposalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a suggestion does, with what is needed to show and apply it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalAction {
    /// Store a memory.
    Remember {
        /// The memory (complete; may be edited before accepting).
        content: MemoryContent,
        /// Text rendering.
        text: String,
        /// How it relates to what is remembered.
        decision: Decision,
        /// Who decided.
        decided_by: DecidedBy,
        /// Text of the memory the decision is about.
        #[serde(default)]
        target_text: Option<String>,
        /// The quote it is grounded in.
        evidence: String,
        /// Model that formulated it.
        model: String,
        /// When the source was written (the memory's time).
        at: DateTime<Utc>,
        /// Provenance source (`conversation://<id>/turn/<run>`).
        source: String,
    },
    /// A new version of `target`.
    Revise {
        /// The memory revised.
        target: String,
        /// Its current text.
        target_text: String,
        /// The new version (complete).
        content: MemoryContent,
        /// Text of the new version.
        text: String,
        /// Why (the memory that triggered the revision).
        reason: String,
        /// Model that formulated it.
        model: String,
        /// When.
        at: DateTime<Utc>,
        /// Provenance source of the triggering memory.
        source: String,
    },
    /// Link two memories that belong together.
    Link {
        /// One memory.
        a: String,
        /// The other.
        b: String,
        /// Text of `a`.
        a_text: String,
        /// Text of `b`.
        b_text: String,
        /// Why.
        reason: String,
    },
    /// Close `retire` in favour of `keep`.
    Resolve {
        /// The memory kept.
        keep: String,
        /// Its text.
        keep_text: String,
        /// The memory closed (kept as history).
        retire: String,
        /// Its text.
        retire_text: String,
        /// The contradicting values.
        changes: Vec<super::ValueChange>,
    },
    /// Archive a faded memory.
    Archive {
        /// The memory.
        target: String,
        /// Its text.
        text: String,
        /// Its strength now.
        strength: f64,
    },
}

impl ProposalAction {
    /// The kind of suggestion.
    pub fn kind(&self) -> ProposalKind {
        match self {
            ProposalAction::Remember { .. } => ProposalKind::Remember,
            ProposalAction::Revise { .. } => ProposalKind::Revise,
            ProposalAction::Link { .. } => ProposalKind::Link,
            ProposalAction::Resolve { .. } => ProposalKind::Resolve,
            ProposalAction::Archive { .. } => ProposalKind::Archive,
        }
    }
}

/// How to undo an applied suggestion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Undo {
    /// Forget the stored memory and reopen what it superseded.
    Revert {
        /// The stored memory.
        id: String,
    },
    /// Reopen an archived memory.
    Unarchive {
        /// The memory.
        id: String,
    },
    /// Reopen a memory closed in favour of another.
    Unretire {
        /// The closed memory.
        retire: String,
        /// The memory kept.
        keep: String,
    },
    /// Restore a link to what it was.
    RestoreLink {
        /// One memory.
        a: String,
        /// The other.
        b: String,
        /// The previous link (weight, co-activations, time), `None` when there was none.
        previous: Option<(f64, u32, DateTime<Utc>)>,
    },
}

/// What applying a suggestion did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedOutcome {
    /// The memory current for this fact afterwards, if any.
    pub memory_id: Option<String>,
    /// The store's outcome, for memory writes.
    pub put: Option<PutOutcomeView>,
    /// How to undo it; `None` when there is nothing to undo (a reinforcement).
    pub undo: Option<Undo>,
}

/// [`PutOutcome`] as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutOutcomeView {
    /// `added`, `updated`, `historical`, `unchanged` or `conflict`.
    pub outcome: String,
    /// The stored id, if a statement was stored.
    pub stored: Option<String>,
    /// Statements it superseded.
    pub superseded: Vec<String>,
}

impl From<&PutOutcome> for PutOutcomeView {
    fn from(outcome: &PutOutcome) -> Self {
        let (name, superseded) = match outcome {
            PutOutcome::Added { .. } => ("added", Vec::new()),
            PutOutcome::Updated { superseded, .. } => ("updated", superseded.clone()),
            PutOutcome::Historical { .. } => ("historical", Vec::new()),
            PutOutcome::Unchanged { .. } => ("unchanged", Vec::new()),
            PutOutcome::Conflict { .. } => ("conflict", Vec::new()),
        };
        Self {
            outcome: name.to_string(),
            stored: outcome.stored_id().map(str::to_string),
            superseded,
        }
    }
}

/// A stored suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// Id (`sug-<uuid>`).
    pub id: String,
    /// Which step produced it.
    pub origin: ProposalOrigin,
    /// Status.
    pub status: ProposalStatus,
    /// Duplicate suppression key.
    pub fingerprint: String,
    /// Scope the memory goes to.
    pub scope: Scope,
    /// Conversation it came from, if any.
    pub conversation_id: Option<String>,
    /// Turn it came from, if any.
    pub turn_id: Option<String>,
    /// What it does.
    pub action: ProposalAction,
    /// Confidence in `[0, 1]`.
    pub confidence: f64,
    /// Why it is sensitive.
    pub sensitive: Vec<SensitiveReason>,
    /// What applying it did.
    pub outcome: Option<AppliedOutcome>,
    /// Why applying it failed or it went stale.
    pub error: Option<String>,
    /// Created.
    pub created_at: DateTime<Utc>,
    /// Decided (accepted, rejected, learned, undone, stale).
    pub decided_at: Option<DateTime<Utc>>,
}

/// A suggestion as the UI shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    /// Id.
    pub id: String,
    /// Kind.
    pub kind: ProposalKind,
    /// Which step produced it.
    pub origin: ProposalOrigin,
    /// Status.
    pub status: ProposalStatus,
    /// What it does (content, decision, texts).
    pub action: ProposalAction,
    /// Confidence.
    pub confidence: f64,
    /// Why it is sensitive (always asks).
    pub sensitive: Vec<SensitiveReason>,
    /// Conversation it came from.
    pub conversation_id: Option<String>,
    /// Turn it came from.
    pub turn_id: Option<String>,
    /// What applying it did.
    pub outcome: Option<AppliedOutcome>,
    /// Why it failed or went stale.
    pub error: Option<String>,
    /// Whether it can be undone now.
    pub undoable: bool,
    /// Created.
    pub created_at: DateTime<Utc>,
    /// Decided.
    pub decided_at: Option<DateTime<Utc>>,
}

impl From<Proposal> for ProposalView {
    fn from(p: Proposal) -> Self {
        let undoable = matches!(p.status, ProposalStatus::Accepted | ProposalStatus::Learned)
            && p.outcome.as_ref().is_some_and(|o| o.undo.is_some());
        Self {
            id: p.id,
            kind: p.action.kind(),
            origin: p.origin,
            status: p.status,
            action: p.action,
            confidence: p.confidence,
            sensitive: p.sensitive,
            conversation_id: p.conversation_id,
            turn_id: p.turn_id,
            outcome: p.outcome,
            error: p.error,
            undoable,
            created_at: p.created_at,
            decided_at: p.decided_at,
        }
    }
}

/// The learning model's use today, and the caps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageToday {
    /// UTC day.
    pub day: String,
    /// Model calls.
    pub llm_calls: u32,
    /// Prompt characters.
    pub input_chars: u64,
    /// Answer characters.
    pub output_chars: u64,
    /// Suggestions created.
    pub proposals: u32,
    /// Candidates dropped as invalid.
    pub invalid: u32,
    /// Candidates refused (not quoting the user, or sensitive in automatic mode).
    pub refused: u32,
}

/// What to count in today's usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageCounter {
    /// Candidates dropped as invalid.
    Invalid,
    /// Candidates refused.
    Refused,
}

/// The inbox in `shodh.db`.
pub struct Inbox {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Inbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inbox").finish_non_exhaustive()
    }
}

fn ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_ts(text: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

fn json_err(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

const COLUMNS: &str = "id, origin, status, fingerprint, scope, conversation_id, turn_id, \
    payload_json, confidence, sensitive_json, outcome_json, error, created_at, decided_at";

fn read_proposal(r: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
    let invalid = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("invalid {what}").into(),
        )
    };
    let origin: String = r.get(1)?;
    let status: String = r.get(2)?;
    let scope: String = r.get(4)?;
    let payload: String = r.get(7)?;
    let sensitive: String = r.get(9)?;
    let outcome: Option<String> = r.get(10)?;
    Ok(Proposal {
        id: r.get(0)?,
        origin: ProposalOrigin::parse(&origin).ok_or_else(|| invalid("origin"))?,
        status: ProposalStatus::parse(&status).ok_or_else(|| invalid("status"))?,
        fingerprint: r.get(3)?,
        scope: scope.parse().map_err(|_| invalid("scope"))?,
        conversation_id: r.get(5)?,
        turn_id: r.get(6)?,
        action: serde_json::from_str(&payload).map_err(json_err)?,
        confidence: r.get(8)?,
        sensitive: serde_json::from_str(&sensitive).map_err(json_err)?,
        outcome: outcome
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(json_err)?,
        error: r.get(11)?,
        created_at: parse_ts(&r.get::<_, String>(12)?)?,
        decided_at: r
            .get::<_, Option<String>>(13)?
            .map(|t| parse_ts(&t))
            .transpose()?,
    })
}

/// A new suggestion before it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct NewProposal {
    /// Which step produced it.
    pub origin: ProposalOrigin,
    /// Duplicate suppression key.
    pub fingerprint: String,
    /// Scope.
    pub scope: Scope,
    /// Conversation.
    pub conversation_id: Option<String>,
    /// Turn.
    pub turn_id: Option<String>,
    /// What it does.
    pub action: ProposalAction,
    /// Confidence.
    pub confidence: f64,
    /// Why it is sensitive.
    pub sensitive: Vec<SensitiveReason>,
}

impl Inbox {
    /// Opens the inbox in `shodh.db` at `path` (applying pending migrations).
    pub fn open(path: &Path, key: Option<&AuditKey>) -> LearnResult<Self> {
        Ok(Self {
            conn: Mutex::new(open_shared_connection(path, key)?),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores a suggestion unless the same one is already waiting or applied, or the user
    /// rejected or undid it recently. Counts it against today's suggestion cap. Returns the stored
    /// suggestion, or `None` when it was suppressed.
    pub fn insert(
        &self,
        new: NewProposal,
        caps: &LearnCaps,
        now: DateTime<Utc>,
    ) -> LearnResult<Option<Proposal>> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let since = ts(now - Duration::days(REJECTION_MEMORY_DAYS));
        let duplicate: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_proposals WHERE fingerprint = ?1 AND (
                status IN ('pending', 'accepted', 'learned', 'failed')
                OR (status IN ('rejected', 'undone') AND decided_at >= ?2)))",
            params![new.fingerprint, since],
            |r| r.get(0),
        )?;
        if duplicate {
            return Ok(None);
        }
        let day = day_key(now);
        let made: u32 = tx
            .query_row(
                "SELECT proposals FROM memory_learn_usage WHERE day = ?1",
                params![day],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if made >= caps.max_proposals_per_day {
            return Err(LearnError::BudgetExhausted(format!(
                "{} suggestions per day",
                caps.max_proposals_per_day
            )));
        }
        let id = format!("sug-{}", uuid::Uuid::new_v4());
        let payload =
            serde_json::to_string(&new.action).map_err(|e| LearnError::Database(e.to_string()))?;
        let sensitive = serde_json::to_string(&new.sensitive)
            .map_err(|e| LearnError::Database(e.to_string()))?;
        tx.execute(
            "INSERT INTO memory_proposals(id, kind, origin, status, fingerprint, scope,
                conversation_id, turn_id, payload_json, confidence, sensitive_json, created_at)
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                new.action.kind().as_str(),
                new.origin.as_str(),
                new.fingerprint,
                new.scope.as_key(),
                new.conversation_id,
                new.turn_id,
                payload,
                new.confidence.clamp(0.0, 1.0),
                sensitive,
                ts(now),
            ],
        )?;
        bump(&tx, &day, "proposals", 1)?;
        tx.commit()?;
        Ok(Some(Proposal {
            id,
            origin: new.origin,
            status: ProposalStatus::Pending,
            fingerprint: new.fingerprint,
            scope: new.scope,
            conversation_id: new.conversation_id,
            turn_id: new.turn_id,
            action: new.action,
            confidence: new.confidence.clamp(0.0, 1.0),
            sensitive: new.sensitive,
            outcome: None,
            error: None,
            created_at: now,
            decided_at: None,
        }))
    }

    /// One suggestion.
    pub fn get(&self, id: &str) -> LearnResult<Proposal> {
        let conn = self.lock();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM memory_proposals WHERE id = ?1"),
            params![id],
            read_proposal,
        )
        .optional()?
        .ok_or_else(|| LearnError::NotFound(id.to_string()))
    }

    /// Suggestions with one of `statuses` (all when empty), newest first.
    pub fn list(&self, statuses: &[ProposalStatus], limit: usize) -> LearnResult<Vec<Proposal>> {
        let conn = self.lock();
        let limit = i64::try_from(limit.clamp(1, 1_000)).unwrap_or(1_000);
        let filter = if statuses.is_empty() {
            String::new()
        } else {
            format!(
                " WHERE status IN ({})",
                statuses
                    .iter()
                    .map(|s| format!("'{}'", s.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM memory_proposals{filter} ORDER BY created_at DESC, id LIMIT ?1"
        ))?;
        let rows = stmt
            .query_map(params![limit], read_proposal)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Number of suggestions waiting for the user.
    pub fn pending_count(&self) -> LearnResult<u32> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT count(*) FROM memory_proposals WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )?)
    }

    /// Moves suggestion `id` by `event` if its current status allows it (atomically).
    /// Returns the suggestion as it was before the move.
    pub fn transition(
        &self,
        id: &str,
        event: StatusEvent,
        now: DateTime<Utc>,
    ) -> LearnResult<Proposal> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM memory_proposals WHERE id = ?1"),
                params![id],
                read_proposal,
            )
            .optional()?
            .ok_or_else(|| LearnError::NotFound(id.to_string()))?;
        let next = before.status.after(event)?;
        let sources = ProposalStatus::sources(event)
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let changed = tx.execute(
            &format!(
                "UPDATE memory_proposals SET status = ?1, decided_at = ?2
                 WHERE id = ?3 AND status IN ({sources})"
            ),
            params![next.as_str(), ts(now), id],
        )?;
        if changed != 1 {
            return Err(LearnError::InvalidTransition {
                from: before.status,
                action: event.verb(),
            });
        }
        tx.commit()?;
        Ok(before)
    }

    /// Puts a suggestion back to `status` (after a failed undo), only from `from`.
    pub fn restore_status(
        &self,
        id: &str,
        from: ProposalStatus,
        status: ProposalStatus,
    ) -> LearnResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE memory_proposals SET status = ?1 WHERE id = ?2 AND status = ?3",
            params![status.as_str(), id, from.as_str()],
        )?;
        Ok(())
    }

    /// Records what applying suggestion `id` did (and the edited action, if it was edited).
    pub fn record_outcome(
        &self,
        id: &str,
        outcome: &AppliedOutcome,
        action: Option<&ProposalAction>,
    ) -> LearnResult<()> {
        let conn = self.lock();
        let outcome =
            serde_json::to_string(outcome).map_err(|e| LearnError::Database(e.to_string()))?;
        match action {
            Some(action) => {
                let payload = serde_json::to_string(action)
                    .map_err(|e| LearnError::Database(e.to_string()))?;
                conn.execute(
                    "UPDATE memory_proposals SET outcome_json = ?1, payload_json = ?2, error = NULL
                     WHERE id = ?3",
                    params![outcome, payload, id],
                )?;
            }
            None => {
                conn.execute(
                    "UPDATE memory_proposals SET outcome_json = ?1, error = NULL WHERE id = ?2",
                    params![outcome, id],
                )?;
            }
        }
        Ok(())
    }

    /// Records why suggestion `id` failed or went stale.
    pub fn record_error(&self, id: &str, error: &str) -> LearnResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE memory_proposals SET error = ?1 WHERE id = ?2",
            params![crate::harness::truncate_chars(error, 500), id],
        )?;
        Ok(())
    }

    /// Reserves one model call with a prompt of `input_chars` against today's caps.
    pub fn reserve_call(
        &self,
        input_chars: usize,
        caps: &LearnCaps,
        now: DateTime<Utc>,
    ) -> LearnResult<()> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let day = day_key(now);
        let (calls, chars): (u32, u64) = tx
            .query_row(
                "SELECT llm_calls, input_chars FROM memory_learn_usage WHERE day = ?1",
                params![day],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get::<_, i64>(1).map(|c| u64::try_from(c).unwrap_or(0))?,
                    ))
                },
            )
            .optional()?
            .unwrap_or((0, 0));
        let input = u64::try_from(input_chars).unwrap_or(u64::MAX);
        if calls >= caps.max_calls_per_day {
            return Err(LearnError::BudgetExhausted(format!(
                "{} model calls per day",
                caps.max_calls_per_day
            )));
        }
        if chars.saturating_add(input) > caps.max_input_chars_per_day {
            return Err(LearnError::BudgetExhausted(format!(
                "{} prompt characters per day",
                caps.max_input_chars_per_day
            )));
        }
        bump(&tx, &day, "llm_calls", 1)?;
        bump(
            &tx,
            &day,
            "input_chars",
            i64::try_from(input).unwrap_or(i64::MAX),
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Records the size of a model answer.
    pub fn record_output(&self, output_chars: usize, now: DateTime<Utc>) -> LearnResult<()> {
        let conn = self.lock();
        bump(
            &conn,
            &day_key(now),
            "output_chars",
            i64::try_from(output_chars).unwrap_or(i64::MAX),
        )?;
        Ok(())
    }

    /// Counts dropped or refused candidates.
    pub fn count(&self, counter: UsageCounter, n: usize, now: DateTime<Utc>) -> LearnResult<()> {
        if n == 0 {
            return Ok(());
        }
        let column = match counter {
            UsageCounter::Invalid => "invalid",
            UsageCounter::Refused => "refused",
        };
        let conn = self.lock();
        bump(
            &conn,
            &day_key(now),
            column,
            i64::try_from(n).unwrap_or(i64::MAX),
        )?;
        Ok(())
    }

    /// Today's usage.
    pub fn usage(&self, now: DateTime<Utc>) -> LearnResult<UsageToday> {
        let conn = self.lock();
        let day = day_key(now);
        let usage = conn
            .query_row(
                "SELECT llm_calls, input_chars, output_chars, proposals, invalid, refused
                 FROM memory_learn_usage WHERE day = ?1",
                params![day],
                |r| {
                    let big = |i: usize| -> rusqlite::Result<u64> {
                        Ok(u64::try_from(r.get::<_, i64>(i)?).unwrap_or(0))
                    };
                    Ok(UsageToday {
                        day: day.clone(),
                        llm_calls: r.get(0)?,
                        input_chars: big(1)?,
                        output_chars: big(2)?,
                        proposals: r.get(3)?,
                        invalid: r.get(4)?,
                        refused: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(usage.unwrap_or(UsageToday {
            day,
            ..Default::default()
        }))
    }

    /// A small stored value (for example the last consolidation time).
    pub fn state(&self, key: &str) -> LearnResult<Option<String>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT value FROM memory_learn_state WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Sets a small stored value.
    pub fn set_state(&self, key: &str, value: &str) -> LearnResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO memory_learn_state(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Rejects every waiting suggestion (the kill switch). Returns how many.
    pub fn reject_all_pending(&self, now: DateTime<Utc>) -> LearnResult<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "UPDATE memory_proposals SET status = 'rejected', decided_at = ?1
             WHERE status = 'pending'",
            params![ts(now)],
        )?)
    }
}

/// Adds `n` to one usage column of `day`. `column` is one of a fixed set.
fn bump(conn: &Connection, day: &str, column: &str, n: i64) -> rusqlite::Result<()> {
    debug_assert!(matches!(
        column,
        "llm_calls" | "input_chars" | "output_chars" | "proposals" | "invalid" | "refused"
    ));
    conn.execute(
        &format!(
            "INSERT INTO memory_learn_usage(day, {column}) VALUES (?1, ?2)
             ON CONFLICT(day) DO UPDATE SET {column} = {column} + excluded.{column}"
        ),
        params![day, n],
    )?;
    Ok(())
}
