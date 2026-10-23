//! The suggestions inbox (`memory_proposals` in `shodh.db`, schema version 4), the
//! learning model's daily usage and why its answers were not used
//! (`memory_learn_rejections`, schema version 10).
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

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::decide::{DecidedBy, Decision};
use super::sensitivity::SensitiveReason;
use super::{day_key, normalise_text, LearnCaps, LearnError, LearnResult};
use crate::audit::{open_shared_connection, AuditKey, SharedConnection};
use crate::statements::{PutOutcome, Scope};
use crate::user_memory::MemoryContent;

/// How long a rejected (or undone) suggestion suppresses the same suggestion.
pub const REJECTION_MEMORY_DAYS: i64 = 180;

/// Model calls the import of the removed legacy memory store may make, once, ever: its
/// own allowance, so the import never spends the day's calls (it runs at start, before
/// the user types anything).
pub const LEGACY_IMPORT_CALLS: u32 = 8;
/// State key counting the legacy import's model calls.
const LEGACY_CALLS_KEY: &str = "legacy_import_calls";

/// How long the reasons the learning model's answers were not used are kept.
pub const REJECTION_LOG_DAYS: i64 = 14;
/// Most bytes of the model's answer kept with one rejection.
pub const MAX_EXCERPT_BYTES: usize = 2_048;
/// Most bytes of a rejection's detail.
const MAX_DETAIL_BYTES: usize = 500;
/// Most rejections kept from one model answer.
const MAX_REJECTIONS_PER_ANSWER: usize = 32;

/// Why a model answer, or one memory in it, was not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    /// Reason code (`invalid_json`, `truncated`, `malformed`, `ungrounded`, an ontology
    /// violation code such as `subject_class_mismatch`, ...).
    pub code: String,
    /// What exactly was wrong (a parser or validation message). It may quote the model's
    /// answer, so it is checked like the excerpt, unless the app wrote all of it.
    pub detail: String,
    /// The part of the answer it is about (masked and cut before it is stored).
    pub excerpt: String,
    /// The turn it was learned from, when known.
    pub turn_id: Option<String>,
    /// `detail` is the app's own text, quoting nothing of the answer ("the model stopped
    /// at its 4000-token limit"): only keys and bearer tokens are masked in it.
    pub app_detail: bool,
}

impl Rejection {
    /// A rejection of `excerpt` for `code`; `detail` may quote the model's answer.
    pub fn new(code: &str, detail: impl Into<String>, excerpt: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            detail: detail.into(),
            excerpt: excerpt.into(),
            turn_id: None,
            app_detail: false,
        }
    }

    /// A rejection whose `detail` the app wrote whole, quoting nothing of the answer.
    pub fn with_app_detail(
        code: &str,
        detail: impl Into<String>,
        excerpt: impl Into<String>,
    ) -> Self {
        Self {
            app_detail: true,
            ..Self::new(code, detail, excerpt)
        }
    }

    /// The same rejection, about `turn_id` unless it already names a turn.
    pub fn about(mut self, turn_id: Option<&str>) -> Self {
        if self.turn_id.is_none() {
            self.turn_id = turn_id.map(str::to_string);
        }
        self
    }
}

/// How many answers or memories were not used today for one reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectionCount {
    /// Reason code.
    pub code: String,
    /// The reason in plain words ([`rejection_reason`]).
    pub reason: String,
    /// How many.
    pub count: u32,
}

/// A rejection code in plain words, for Settings → Memory ("1 invalid: subject did not
/// fit the class").
pub fn rejection_reason(code: &str) -> &'static str {
    match code {
        "invalid_json" => "the answer was not JSON in the expected shape",
        "truncated" => "the answer was cut off at the length limit",
        "malformed" => "a memory was not in the expected shape",
        "ungrounded" => "it did not quote your words",
        "outside_slice" => "a kind of memory Shodh did not ask for",
        "invalid_confidence" => "its confidence was not between 0 and 1",
        "invalid_subject" => "its subject was not a valid id",
        "invalid_statement" => "the memory could not be built",
        "subject_class_mismatch" => "subject did not fit the class",
        "unknown_class" => "an unknown kind of memory",
        "unknown_property" => "a field Shodh does not know",
        "not_in_domain" => "a field that does not belong to that kind of memory",
        "cardinality_exceeded" => "several values for a single-valued field",
        "missing_required" => "a required field was missing",
        "type_mismatch" => "a value of the wrong type",
        "invalid_value" | "pattern_mismatch" => "a value in the wrong format",
        "not_in_enum" => "a value outside the allowed choices",
        "range_class_mismatch" => "a link to the wrong kind of thing",
        "invalid_judgement" => "an unreadable decision about similar memories",
        "invalid_revision" => "an unreadable revision of a memory",
        "steps_mismatch" => "steps that do not follow what you did",
        "missing_provenance" | "invalid_provenance" | "invalid_id" | "incompatible_version" => {
            "an internal check failed"
        }
        _ => "another reason",
    }
}

/// `text` fit to keep: anything shaped like a key or bearer token masked (as provider
/// errors are), withheld whole when it looks like a password, secret or financial
/// identifier, and cut to `max_bytes` on a character boundary.
fn masked(text: &str, max_bytes: usize) -> String {
    let sensitive = super::sensitivity::classify_text(text);
    if sensitive.iter().any(|r| {
        matches!(
            r,
            SensitiveReason::Credential | SensitiveReason::FinancialIdentifier
        )
    }) {
        return "[withheld: it looks like a secret or a financial identifier]".to_string();
    }
    safe_cut(text, max_bytes)
}

/// `text` with anything shaped like a key or bearer token masked (as provider errors
/// are), cut to `max_bytes` on a character boundary. For the app's own detail of a
/// rejection, which never holds the model's answer.
fn safe_cut(text: &str, max_bytes: usize) -> String {
    let safe = crate::harness::provider_error::safe_detail(text, None, max_bytes);
    let mut end = safe.len().min(max_bytes);
    while !safe.is_char_boundary(end) {
        end -= 1;
    }
    safe[..end].to_string()
}

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
        /// The user formulated it: their own words, filed when they asked Shodh to
        /// remember something (`model` is then the app version). Stored with extractor
        /// `user`, accepted at once on their request (their approval), or on their Accept
        /// when it is sensitive; never by the automatic-learning policy.
        #[serde(default)]
        by_user: bool,
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
        /// The memory that triggered the revision (its reason may quote it). Suggestions
        /// filed before it was recorded have none.
        #[serde(default)]
        trigger: Option<String>,
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

    /// Ids of the memories it is about: its targets and the memory its decision names.
    fn memory_ids(&self) -> Vec<&str> {
        match self {
            ProposalAction::Remember { decision, .. } => match decision {
                Decision::Add => Vec::new(),
                Decision::Noop { existing } => vec![existing.as_str()],
                Decision::Extend { target }
                | Decision::Supersede { target, .. }
                | Decision::Update { target } => vec![target.as_str()],
                Decision::Historical { current } => vec![current.as_str()],
            },
            ProposalAction::Revise {
                target, trigger, ..
            } => std::iter::once(target.as_str())
                .chain(trigger.as_deref())
                .collect(),
            ProposalAction::Archive { target, .. } => vec![target.as_str()],
            ProposalAction::Link { a, b, .. } => vec![a.as_str(), b.as_str()],
            ProposalAction::Resolve { keep, retire, .. } => vec![keep.as_str(), retire.as_str()],
        }
    }

    /// The same suggestion without any of the memory's words: texts, the quote, values,
    /// reasons and the subject are cleared; ids, the class and the kind of decision stay
    /// (so the stored suggestion still reads back).
    fn redacted(&self) -> ProposalAction {
        let mut action = self.clone();
        match &mut action {
            ProposalAction::Remember {
                content,
                text,
                decision,
                target_text,
                evidence,
                ..
            } => {
                let cleared = redacted_content(content);
                *content = cleared;
                text.clear();
                *target_text = None;
                evidence.clear();
                if let Decision::Supersede { changes, .. } = decision {
                    clear_changes(changes);
                }
            }
            ProposalAction::Revise {
                target_text,
                content,
                text,
                reason,
                ..
            } => {
                let cleared = redacted_content(content);
                *content = cleared;
                target_text.clear();
                text.clear();
                reason.clear();
            }
            ProposalAction::Link {
                a_text,
                b_text,
                reason,
                ..
            } => {
                a_text.clear();
                b_text.clear();
                reason.clear();
            }
            ProposalAction::Resolve {
                keep_text,
                retire_text,
                changes,
                ..
            } => {
                keep_text.clear();
                retire_text.clear();
                clear_changes(changes);
            }
            ProposalAction::Archive { text, .. } => text.clear(),
        }
        action
    }
}

/// Memory content without its words: a note's text, a fact's values and subject.
fn redacted_content(content: &MemoryContent) -> MemoryContent {
    match content {
        MemoryContent::Note { .. } => MemoryContent::Note {
            text: String::new(),
        },
        MemoryContent::Fact { class, .. } => MemoryContent::Fact {
            class: class.clone(),
            subject: None,
            properties: BTreeMap::new(),
            valid_from: None,
        },
    }
}

/// Clears the old and new values of contradicting or replaced values.
fn clear_changes(changes: &mut [super::ValueChange]) {
    for change in changes {
        change.from.clear();
        change.to.clear();
    }
}

/// What a forget erased, for [`Inbox::forget_memories`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForgottenMemories {
    /// The id of every version.
    pub ids: Vec<String>,
    /// The runs (turns) they were learned or approved in.
    pub turns: Vec<String>,
    /// Their provenance sources.
    pub sources: Vec<String>,
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

impl AppliedOutcome {
    /// Ids of the memories it stored, superseded or would change on undo.
    fn memory_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.memory_id.iter().map(String::as_str).collect();
        if let Some(put) = &self.put {
            ids.extend(put.stored.as_deref());
            ids.extend(put.superseded.iter().map(String::as_str));
        }
        match &self.undo {
            Some(Undo::Revert { id }) | Some(Undo::Unarchive { id }) => ids.push(id.as_str()),
            Some(Undo::Unretire { retire, keep }) => {
                ids.push(retire.as_str());
                ids.push(keep.as_str());
            }
            Some(Undo::RestoreLink { a, b, .. }) => {
                ids.push(a.as_str());
                ids.push(b.as_str());
            }
            None => {}
        }
        ids
    }
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
    /// When the memory it stored or was about was forgotten; its texts are cleared then
    /// ([`Inbox::forget_memories`]).
    pub forgotten_at: Option<DateTime<Utc>>,
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
    /// Where the memory goes (`global` or `workspace:<id>`).
    pub scope: Scope,
    /// What applying it did.
    pub outcome: Option<AppliedOutcome>,
    /// Why it failed or went stale.
    pub error: Option<String>,
    /// Whether it can be undone now.
    pub undoable: bool,
    /// The memory it stored or was about was forgotten: its texts are cleared and there
    /// is nothing to undo.
    pub forgotten: bool,
    /// Created.
    pub created_at: DateTime<Utc>,
    /// Decided.
    pub decided_at: Option<DateTime<Utc>>,
}

impl From<Proposal> for ProposalView {
    fn from(p: Proposal) -> Self {
        let forgotten = p.forgotten_at.is_some();
        let undoable = !forgotten
            && matches!(p.status, ProposalStatus::Accepted | ProposalStatus::Learned)
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
            scope: p.scope,
            outcome: p.outcome,
            error: p.error,
            undoable,
            forgotten,
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

/// The inbox in `shodh.db`. Every write is one `IMMEDIATE` transaction during the
/// database's in-process write turn ([`SharedConnection::write`]).
pub struct Inbox {
    conn: SharedConnection,
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
    payload_json, confidence, sensitive_json, outcome_json, error, created_at, decided_at, \
    forgotten_at";

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
        forgotten_at: r
            .get::<_, Option<String>>(14)?
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
            conn: open_shared_connection(path, key)?,
        })
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
        let since = ts(now - Duration::days(REJECTION_MEMORY_DAYS));
        self.conn.write(|tx| {
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
            let made: u32 = tx
                .query_row(
                    "SELECT proposals FROM memory_learn_usage WHERE day = ?1",
                    params![day_key(now)],
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
            let proposal = write_proposal(tx, new, now)?;
            Ok(Some(proposal))
        })
    }

    /// Memories that applied suggestions with `fingerprint` (accepted or learned) stored.
    /// They may have been forgotten since: the suggestions are not changed by a forget.
    pub fn applied_memories(&self, fingerprint: &str) -> LearnResult<Vec<String>> {
        let conn = self.conn.read();
        let mut stmt = conn.prepare(
            "SELECT outcome_json FROM memory_proposals
             WHERE fingerprint = ?1 AND status IN ('accepted', 'learned')",
        )?;
        let outcomes = stmt
            .query_map(params![fingerprint], |r| r.get::<_, Option<String>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(outcomes
            .into_iter()
            .flatten()
            .filter_map(|json| serde_json::from_str::<AppliedOutcome>(&json).ok())
            .filter_map(|outcome| outcome.memory_id)
            .collect())
    }

    /// Stores a suggestion the user asked for in their own words ("Remember that ..."),
    /// unless the same note is already saved or waiting for approval. `applied_kept`:
    /// whether a memory an applied suggestion of the same note stored still exists
    /// ([`Self::applied_memories`]); when the user forgot it, asking again files the note
    /// again. A suggestion of the same note (the same fingerprint) that waits is returned
    /// instead when either is sensitive (it waits for the user's approval already);
    /// otherwise it is replaced: marked stale ([`REPLACED_BY_REQUEST`], no rejection the
    /// learner would remember) and this one stored, so the note is saved as the user's
    /// own words of their turn, never as the learner's version. A note the user rejected
    /// or undid before is filed again: asking again is the user's own decision, which a
    /// past rejection does not overrule (the learner itself still does not suggest it
    /// again, [`Self::insert`]). Today's suggestion cap (a cap on what the model suggests)
    /// does not hold it back; it is counted in today's suggestions.
    pub fn insert_requested(
        &self,
        new: NewProposal,
        now: DateTime<Utc>,
        applied_kept: bool,
    ) -> LearnResult<RequestedFiling> {
        if applied_kept {
            return Ok(RequestedFiling::Kept);
        }
        self.conn.write(|tx| {
            let mut stmt = tx.prepare(&format!(
                "SELECT {COLUMNS} FROM memory_proposals WHERE fingerprint = ?1
             AND status IN ('pending', 'failed') ORDER BY created_at, id"
            ))?;
            let waiting = stmt
                .query_map(params![new.fingerprint], read_proposal)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            if let Some(sensitive) = waiting
                .iter()
                .find(|w| !w.sensitive.is_empty() || !new.sensitive.is_empty())
            {
                return Ok(RequestedFiling::Waiting(Box::new(sensitive.clone())));
            }
            for replaced in &waiting {
                tx.execute(
                    "UPDATE memory_proposals SET status = 'stale', decided_at = ?1, error = ?2
                 WHERE id = ?3 AND status IN ('pending', 'failed')",
                    params![ts(now), REPLACED_BY_REQUEST, replaced.id],
                )?;
            }
            let proposal = write_proposal(tx, new, now)?;
            Ok(RequestedFiling::Inserted(Box::new(proposal)))
        })
    }

    /// The memory a note suggestion applied in turn `turn_id` (accepted or learned, and
    /// not forgotten) stored, when its text reads the same as `text` ([`same_note`]): the
    /// note the user asked Shodh to remember in that turn, so the assistant's own
    /// `remember` of the same words in the same answer is recognised as done.
    pub fn saved_note_in_turn(&self, turn_id: &str, text: &str) -> LearnResult<Option<String>> {
        let conn = self.conn.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM memory_proposals
             WHERE turn_id = ?1 AND kind = 'remember' AND status IN ('accepted', 'learned')
                AND forgotten_at IS NULL
             ORDER BY created_at, id"
        ))?;
        let rows = stmt
            .query_map(params![turn_id], read_proposal)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().find_map(|proposal| {
            let ProposalAction::Remember { text: saved, .. } = &proposal.action else {
                return None;
            };
            if !same_note(saved, text) {
                return None;
            }
            proposal.outcome.and_then(|outcome| outcome.memory_id)
        }))
    }

    /// The "remember" suggestions learned from (or asked for in) turns `turn_ids` of
    /// conversation `conversation_id`, oldest first, whatever their status: what the chat
    /// shows under each turn ("Remembered", "Learned", "Shodh suggests remembering").
    /// At most [`MAX_TURNS_LISTED`] turns.
    pub fn list_for_turns(
        &self,
        conversation_id: &str,
        turn_ids: &[String],
    ) -> LearnResult<Vec<Proposal>> {
        if turn_ids.is_empty() {
            return Ok(Vec::new());
        }
        let turns = &turn_ids[..turn_ids.len().min(MAX_TURNS_LISTED)];
        let conn = self.conn.read();
        let placeholders = (2..turns.len() + 2)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM memory_proposals
             WHERE conversation_id = ?1 AND kind = 'remember' AND origin = 'turn'
                AND turn_id IN ({placeholders})
             ORDER BY created_at, id"
        ))?;
        let values: Vec<&str> = std::iter::once(conversation_id)
            .chain(turns.iter().map(String::as_str))
            .collect();
        let rows = stmt
            .query_map(params_from_iter(values), read_proposal)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// One suggestion.
    pub fn get(&self, id: &str) -> LearnResult<Proposal> {
        let conn = self.conn.read();
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
        let conn = self.conn.read();
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
        let conn = self.conn.read();
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
        self.conn.write(|tx| {
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
            Ok(before)
        })
    }

    /// Puts a suggestion back to `status` (after a failed undo), only from `from`.
    pub fn restore_status(
        &self,
        id: &str,
        from: ProposalStatus,
        status: ProposalStatus,
    ) -> LearnResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE memory_proposals SET status = ?1 WHERE id = ?2 AND status = ?3",
                params![status.as_str(), id, from.as_str()],
            )?;
            Ok(())
        })
    }

    /// Records what applying suggestion `id` did (and the edited action, if it was edited).
    pub fn record_outcome(
        &self,
        id: &str,
        outcome: &AppliedOutcome,
        action: Option<&ProposalAction>,
    ) -> LearnResult<()> {
        let outcome =
            serde_json::to_string(outcome).map_err(|e| LearnError::Database(e.to_string()))?;
        let payload = action
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| LearnError::Database(e.to_string()))?;
        self.conn.write(|tx| {
            match payload {
                Some(payload) => {
                    tx.execute(
                        "UPDATE memory_proposals SET outcome_json = ?1, payload_json = ?2, error = NULL
                     WHERE id = ?3",
                        params![outcome, payload, id],
                    )?;
                }
                None => {
                    tx.execute(
                        "UPDATE memory_proposals SET outcome_json = ?1, error = NULL WHERE id = ?2",
                        params![outcome, id],
                    )?;
                }
            }
            Ok(())
        })
    }

    /// Records why suggestion `id` failed or went stale.
    pub fn record_error(&self, id: &str, error: &str) -> LearnResult<()> {
        let error = crate::harness::truncate_chars(error, 500);
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE memory_proposals SET error = ?1 WHERE id = ?2",
                params![error, id],
            )?;
            Ok(())
        })
    }

    /// Reserves one model call with a prompt of `input_chars` against today's caps.
    pub fn reserve_call(
        &self,
        input_chars: usize,
        caps: &LearnCaps,
        now: DateTime<Utc>,
    ) -> LearnResult<()> {
        let day = day_key(now);
        self.conn.write(|tx| {
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
            bump(tx, &day, "llm_calls", 1)?;
            bump(
                tx,
                &day,
                "input_chars",
                i64::try_from(input).unwrap_or(i64::MAX),
            )?;
            Ok(())
        })
    }

    /// Reserves one model call of the legacy import's one-time allowance
    /// ([`LEGACY_IMPORT_CALLS`]); today's caps are not touched.
    pub fn reserve_legacy_call(&self) -> LearnResult<()> {
        self.conn.write(|tx| {
            let used = legacy_calls(tx)?;
            if used >= LEGACY_IMPORT_CALLS {
                return Err(LearnError::BudgetExhausted(format!(
                    "the {LEGACY_IMPORT_CALLS} model calls for learning from old memories"
                )));
            }
            tx.execute(
                "INSERT INTO memory_learn_state(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![LEGACY_CALLS_KEY, (used + 1).to_string()],
            )?;
            Ok(())
        })
    }

    /// Model calls left of the legacy import's one-time allowance.
    pub fn legacy_calls_left(&self) -> LearnResult<u32> {
        let conn = self.conn.read();
        Ok(LEGACY_IMPORT_CALLS.saturating_sub(legacy_calls(&conn)?))
    }

    /// Records the size of a model answer.
    pub fn record_output(&self, output_chars: usize, now: DateTime<Utc>) -> LearnResult<()> {
        let day = day_key(now);
        self.conn.write(|tx| {
            bump(
                tx,
                &day,
                "output_chars",
                i64::try_from(output_chars).unwrap_or(i64::MAX),
            )?;
            Ok(())
        })
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
        let day = day_key(now);
        self.conn.write(|tx| {
            bump(tx, &day, column, i64::try_from(n).unwrap_or(i64::MAX))?;
            Ok(())
        })
    }

    /// Today's usage.
    pub fn usage(&self, now: DateTime<Utc>) -> LearnResult<UsageToday> {
        let conn = self.conn.read();
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

    /// Keeps why answers of `model`, or memories in them, were not used, and forgets
    /// those older than [`REJECTION_LOG_DAYS`]. Excerpts are masked ([`masked`]).
    pub fn record_rejections(
        &self,
        model: &str,
        rejections: &[Rejection],
        now: DateTime<Utc>,
    ) -> LearnResult<()> {
        if rejections.is_empty() {
            return Ok(());
        }
        let day = day_key(now);
        let at = ts(now);
        self.conn.write(|tx| {
            tx.execute(
                "DELETE FROM memory_learn_rejections WHERE created_at < ?1",
                params![ts(now - Duration::days(REJECTION_LOG_DAYS))],
            )?;
            for rejection in rejections.iter().take(MAX_REJECTIONS_PER_ANSWER) {
                tx.execute(
                    "INSERT INTO memory_learn_rejections(day, turn_id, model, code, detail,
                    raw_excerpt, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        day,
                        rejection.turn_id,
                        crate::harness::truncate_chars(model, 200),
                        crate::harness::truncate_chars(&rejection.code, 64),
                        if rejection.app_detail {
                            safe_cut(&rejection.detail, MAX_DETAIL_BYTES)
                        } else {
                            masked(&rejection.detail, MAX_DETAIL_BYTES)
                        },
                        masked(&rejection.excerpt, MAX_EXCERPT_BYTES),
                        at,
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// Today's rejections by reason, most frequent first (forgetting those older than
    /// [`REJECTION_LOG_DAYS`], so this is a write).
    pub fn rejections_today(&self, now: DateTime<Utc>) -> LearnResult<Vec<RejectionCount>> {
        self.conn.write(|tx| {
            // Kept 14 days even on a day nothing is rejected.
            tx.execute(
                "DELETE FROM memory_learn_rejections WHERE created_at < ?1",
                params![ts(now - Duration::days(REJECTION_LOG_DAYS))],
            )?;
            let mut stmt = tx.prepare(
                "SELECT code, count(*) FROM memory_learn_rejections WHERE day = ?1
             GROUP BY code ORDER BY count(*) DESC, code",
            )?;
            let rows = stmt
                .query_map(params![day_key(now)], |r| {
                    let code: String = r.get(0)?;
                    Ok(RejectionCount {
                        reason: rejection_reason(&code).to_string(),
                        code,
                        count: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// A small stored value (for example the last consolidation time).
    pub fn state(&self, key: &str) -> LearnResult<Option<String>> {
        let conn = self.conn.read();
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
        self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO memory_learn_state(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }

    /// Forgets memories in the suggestions. Each suggestion that stored, superseded or is
    /// about one of `forgotten.ids` (every version of a forgotten memory), or was
    /// triggered by one, loses the memory's words (texts, the quote "You said: ..."
    /// shows, values, reasons, the subject) and its fingerprint (a hash of the text, now
    /// derived from the suggestion's id), and is marked forgotten; waiting ones go stale,
    /// so they are neither shown nor applied. Waiting revisions filed before revisions
    /// recorded their trigger are matched by the trigger's source instead
    /// (`forgotten.sources`): every such revision from a memory of that source goes, as
    /// the trigger cannot be told apart. The rows stay (ids, class), so "Recently learned"
    /// can say what happened. Why the learning model's answers about the turns the
    /// memories came from (`forgotten.turns`) were not used is deleted too: those excerpts
    /// may quote the same words. Rejections without a turn are linked to no memory and
    /// stay until they age out ([`REJECTION_LOG_DAYS`]). Deleted and overwritten content
    /// is zeroed in the database file (`secure_delete`). Returns how many suggestions
    /// changed.
    pub fn forget_memories(
        &self,
        forgotten: &ForgottenMemories,
        now: DateTime<Utc>,
    ) -> LearnResult<usize> {
        if forgotten.ids.is_empty() && forgotten.turns.is_empty() && forgotten.sources.is_empty() {
            return Ok(0);
        }
        // The pragma answers with its new value, so it is read, not executed. It is a
        // setting of this connection, not a write.
        self.conn
            .read()
            .query_row("PRAGMA secure_delete = ON", [], |r| r.get::<_, i64>(0))?;
        self.conn.write(|tx| {
            let mut candidates: Vec<Proposal> = Vec::new();
            {
                let mut stmt = tx.prepare(&format!(
                    "SELECT {COLUMNS} FROM memory_proposals
                 WHERE payload_json LIKE ?1 ESCAPE '!' OR outcome_json LIKE ?1 ESCAPE '!'"
                ))?;
                for id in &forgotten.ids {
                    let pattern = format!("%{}%", like_literal(id));
                    for proposal in stmt.query_map(params![pattern], read_proposal)? {
                        let proposal = proposal?;
                        if !candidates.iter().any(|c| c.id == proposal.id) {
                            candidates.push(proposal);
                        }
                    }
                }
            }
            if !forgotten.sources.is_empty() {
                let mut stmt = tx.prepare(&format!(
                    "SELECT {COLUMNS} FROM memory_proposals
                 WHERE kind = 'revise' AND origin = 'evolve' AND status IN ('pending', 'failed')"
                ))?;
                for proposal in stmt.query_map([], read_proposal)? {
                    let proposal = proposal?;
                    if !candidates.iter().any(|c| c.id == proposal.id) {
                        candidates.push(proposal);
                    }
                }
            }
            let wanted: HashSet<&str> = forgotten.ids.iter().map(String::as_str).collect();
            let mut changed = 0;
            for proposal in &candidates {
                let about = proposal
                    .action
                    .memory_ids()
                    .into_iter()
                    .chain(proposal.outcome.iter().flat_map(AppliedOutcome::memory_ids))
                    .any(|id| wanted.contains(id));
                let waiting = matches!(
                    proposal.status,
                    ProposalStatus::Pending | ProposalStatus::Failed
                );
                let untraced_revision = waiting
                    && matches!(
                        &proposal.action,
                        ProposalAction::Revise { trigger: None, source, .. }
                            if forgotten.sources.contains(source)
                    );
                if !about && !untraced_revision {
                    // The id only appears inside a text (a reason quoting it).
                    continue;
                }
                let payload = serde_json::to_string(&proposal.action.redacted())
                    .map_err(|e| LearnError::Database(e.to_string()))?;
                let (status, decided_at) = if waiting {
                    (ProposalStatus::Stale, Some(ts(now)))
                } else {
                    (proposal.status, proposal.decided_at.map(ts))
                };
                tx.execute(
                    "UPDATE memory_proposals SET payload_json = ?1, status = ?2, decided_at = ?3,
                    error = NULL, forgotten_at = COALESCE(forgotten_at, ?4),
                    fingerprint = 'forgotten:' || id
                 WHERE id = ?5",
                    params![payload, status.as_str(), decided_at, ts(now), proposal.id],
                )?;
                changed += 1;
            }
            for chunk in forgotten.turns.chunks(400) {
                let placeholders = vec!["?"; chunk.len()].join(", ");
                tx.execute(
                    &format!(
                        "DELETE FROM memory_learn_rejections WHERE turn_id IN ({placeholders})"
                    ),
                    params_from_iter(chunk.iter()),
                )?;
            }
            Ok(changed)
        })
    }

    /// Gives the Inbox's approval items of the memory tools the wordless titles their
    /// approval prompts now have ("Remember this", "Update a memory", "Forget a memory"):
    /// items filed before named the memory in the title. An item's tool is read from the
    /// detail the app writes for approvals ("The assistant waits for your approval
    /// (<tool>)."). Returns how many changed.
    pub fn clear_memory_approval_titles(&self) -> LearnResult<usize> {
        // The pragma answers with its new value, so it is read, not executed. It is a
        // setting of this connection, not a write.
        self.conn
            .read()
            .query_row("PRAGMA secure_delete = ON", [], |r| r.get::<_, i64>(0))?;
        self.conn.write(|tx| {
            let mut changed = 0;
            for (tool, title) in [
                ("remember", "Remember this"),
                ("update_memory", "Update a memory"),
                ("forget", "Forget a memory"),
            ] {
                changed += tx.execute(
                    "UPDATE inbox_items SET title = ?1
                 WHERE kind = 'approval' AND detail = ?2 AND title <> ?1",
                    params![
                        title,
                        format!("The assistant waits for your approval ({tool}).")
                    ],
                )?;
            }
            Ok(changed)
        })
    }

    /// Rejects every waiting suggestion (the kill switch). Returns how many.
    pub fn reject_all_pending(&self, now: DateTime<Utc>) -> LearnResult<usize> {
        self.conn.write(|tx| {
            Ok(tx.execute(
                "UPDATE memory_proposals SET status = 'rejected', decided_at = ?1
             WHERE status = 'pending'",
                params![ts(now)],
            )?)
        })
    }
}

/// `text` as a literal in a `LIKE ... ESCAPE '!'` pattern.
fn like_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '!' | '%' | '_') {
            out.push('!');
        }
        out.push(c);
    }
    out
}

/// Model calls the legacy import made so far.
fn legacy_calls(conn: &Connection) -> LearnResult<u32> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM memory_learn_state WHERE key = ?1",
            params![LEGACY_CALLS_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value.and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// What became of the user's own request to remember something
/// ([`super::Learner::file_user_note`]).
#[derive(Debug, Clone, PartialEq)]
pub enum RequestedOutcome {
    /// Saved now as the user's own memory (the request is their approval), with undo.
    Saved(Box<ProposalView>),
    /// Stored as a suggestion waiting for the user: the note is sensitive, or saving it
    /// failed (it can be accepted again).
    Filed(Box<Proposal>),
    /// The same note is already saved or waiting for the user's approval.
    AlreadyKept,
}

/// What [`Inbox::insert_requested`] found or stored.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestedFiling {
    /// A memory of the same note is saved.
    Kept,
    /// The same note waits for the user's approval (this suggestion; it or the request is
    /// sensitive).
    Waiting(Box<Proposal>),
    /// Stored now, waiting.
    Inserted(Box<Proposal>),
}

/// Why a waiting suggestion went stale when the user asked for the same note in their own
/// words ([`Inbox::insert_requested`]).
pub const REPLACED_BY_REQUEST: &str = "replaced by the same note the user asked Shodh to remember";

/// Most turns [`Inbox::list_for_turns`] lists at once.
pub const MAX_TURNS_LISTED: usize = 200;

/// Whether two remembered texts say the same: compared case-folded, with whitespace
/// collapsed and without the closing punctuation or quotes a sentence may carry.
pub fn same_note(a: &str, b: &str) -> bool {
    let key = |text: &str| {
        normalise_text(text)
            .trim_matches(|c: char| {
                matches!(
                    c,
                    '.' | '!' | ';' | ',' | '"' | '\'' | '\u{201c}' | '\u{201d}'
                ) || c.is_whitespace()
            })
            .to_string()
    };
    let a = key(a);
    !a.is_empty() && a == key(b)
}

/// Stores `new` as a waiting suggestion and counts it in today's suggestions.
fn write_proposal(
    tx: &rusqlite::Transaction<'_>,
    new: NewProposal,
    now: DateTime<Utc>,
) -> LearnResult<Proposal> {
    let id = format!("sug-{}", uuid::Uuid::new_v4());
    let payload =
        serde_json::to_string(&new.action).map_err(|e| LearnError::Database(e.to_string()))?;
    let sensitive =
        serde_json::to_string(&new.sensitive).map_err(|e| LearnError::Database(e.to_string()))?;
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
    bump(tx, &day_key(now), "proposals", 1)?;
    Ok(Proposal {
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
        forgotten_at: None,
    })
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
