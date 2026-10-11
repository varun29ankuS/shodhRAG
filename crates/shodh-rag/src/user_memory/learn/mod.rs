//! Learning from conversations: an LLM in the loop that suggests memories, under the
//! user's policy (Settings → Memory → "Learn from conversations": off / ask / automatic).
//!
//! The pipeline, every step bounded in size and cost:
//! 1. **Extract** ([`extract`]): after a turn completes, the user's own words (never tool
//!    results, retrieved passages or web pages — assistant text is context for resolving
//!    references only) are sent with the ontology slice for that text
//!    ([`shodh_ontology::Ontology::slice_for`] + `render_prompt`). The model answers in a
//!    strict JSON schema; every candidate must quote its evidence verbatim from the user's
//!    text and validate against the ontology. Invalid candidates are dropped and counted,
//!    never coerced.
//! 2. **Decide** ([`decide`]): ADD / NOOP / SUPERSEDE / UPDATE against similar existing
//!    memories — deterministically first ([`shodh_ontology::Ontology::supersedes`], exact
//!    duplicates); the model only judges the ambiguous rest, and only when the user lets
//!    memories be shared with the model.
//! 3. **Approve** ([`engine`], [`inbox`]): suggestions wait in the inbox for Accept / Edit /
//!    Reject. In automatic mode, high-confidence, non-sensitive, deterministically decided
//!    suggestions are applied under [`super::WriteAuthority::LearnPolicy`] and listed as
//!    learned, with undo. Sensitive candidates ([`sensitivity`]) always ask.
//! 4. **Evolve** ([`evolve`]): a new memory may link to, or propose new versions of, a few
//!    neighbouring memories — never silent edits.
//! 5. **Consolidate** ([`consolidate`]): at most daily, recent episodes become typed facts,
//!    recurring successful tool sequences become procedures, contradictions are proposed
//!    for resolution and faded episodes for (reversible) archiving.
//!
//! Daily caps bound the model's calls, prompt size and suggestions ([`LearnCaps`]).
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod consolidate;
pub mod decide;
pub mod engine;
pub mod evolve;
pub mod extract;
pub mod inbox;
pub mod sensitivity;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::MemoryError;

pub use decide::{DecidedBy, Decision, ValueChange};
pub use engine::{ConsolidationReport, Learner, TurnReport};
pub use extract::TurnInput;
pub use inbox::{
    AppliedOutcome, Inbox, Proposal, ProposalAction, ProposalKind, ProposalOrigin, ProposalStatus,
    ProposalView, StatusEvent, Undo, UsageToday,
};
pub use sensitivity::SensitiveReason;

/// How learning from conversations is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearnMode {
    /// Nothing is extracted; no model is called.
    Off,
    /// Suggestions wait for the user's decision.
    #[default]
    Ask,
    /// High-confidence, non-sensitive suggestions are applied (with undo); the rest ask.
    Auto,
}

impl LearnMode {
    /// Stable name (`off`, `ask`, `auto`).
    pub fn as_str(self) -> &'static str {
        match self {
            LearnMode::Off => "off",
            LearnMode::Ask => "ask",
            LearnMode::Auto => "auto",
        }
    }
}

/// Daily limits on the learning model's use (per UTC day).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LearnCaps {
    /// Model calls per day (extraction, decisions, evolution, consolidation).
    pub max_calls_per_day: u32,
    /// Prompt characters sent per day.
    pub max_input_chars_per_day: u64,
    /// Suggestions created per day.
    pub max_proposals_per_day: u32,
}

impl Default for LearnCaps {
    fn default() -> Self {
        Self {
            max_calls_per_day: 60,
            max_input_chars_per_day: 400_000,
            max_proposals_per_day: 40,
        }
    }
}

/// Upper bounds the user may set for [`LearnCaps`].
pub const MAX_CALLS_PER_DAY_LIMIT: u32 = 1_000;
/// Upper bound for [`LearnCaps::max_input_chars_per_day`].
pub const MAX_INPUT_CHARS_LIMIT: u64 = 10_000_000;
/// Upper bound for [`LearnCaps::max_proposals_per_day`].
pub const MAX_PROPOSALS_LIMIT: u32 = 500;

/// The user's learning policy, read again before every model call and every write so a
/// change (or the kill switch) takes effect immediately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnPolicy {
    /// Off / ask / automatic.
    pub mode: LearnMode,
    /// Lowest extractor confidence applied automatically, in `[0.5, 1]`.
    pub auto_min_confidence: f64,
    /// Whether stored memories may be sent to the model (the user's "give memories to
    /// the model" setting). When false, decisions are deterministic only and evolution and
    /// consolidation do not call the model.
    pub share_memories_with_model: bool,
    /// Daily cost caps.
    pub caps: LearnCaps,
}

impl Default for LearnPolicy {
    fn default() -> Self {
        Self {
            mode: LearnMode::Ask,
            auto_min_confidence: DEFAULT_AUTO_MIN_CONFIDENCE,
            share_memories_with_model: true,
            caps: LearnCaps::default(),
        }
    }
}

/// Default confidence threshold for automatic learning.
pub const DEFAULT_AUTO_MIN_CONFIDENCE: f64 = 0.85;

/// Where the learner reads the current policy.
pub trait PolicySource: Send + Sync {
    /// The policy now.
    fn policy(&self) -> LearnPolicy;
}

/// A fixed policy.
impl PolicySource for LearnPolicy {
    fn policy(&self) -> LearnPolicy {
        self.clone()
    }
}

/// The model the learner calls: one prompt in, the model's text out. Implemented by the
/// app over its configured provider; tests use a scripted implementation.
#[async_trait::async_trait]
pub trait LearnModel: Send + Sync {
    /// Model id recorded as the extractor version (`claude-haiku-4-5`, `qwen3:4b`, ...).
    fn model_id(&self) -> String;

    /// Completes `prompt` with at most `max_output_tokens` tokens.
    async fn complete(&self, prompt: &str, max_output_tokens: usize) -> Result<String, LearnError>;
}

/// Errors of the learning pipeline.
#[derive(Debug, thiserror::Error)]
pub enum LearnError {
    /// Learning is off (or was turned off while working).
    #[error("learning from conversations is off")]
    Disabled,
    /// No model can be used for learning right now.
    #[error("no model is available for learning: {0}")]
    ModelUnavailable(String),
    /// The model call failed.
    #[error("the learning model failed: {0}")]
    Model(String),
    /// A daily cap is reached.
    #[error("today's learning limit is reached ({0})")]
    BudgetExhausted(String),
    /// The model's answer does not follow the schema.
    #[error("the learning model's answer is not valid: {0}")]
    InvalidOutput(String),
    /// No suggestion with this id.
    #[error("no suggestion with id `{0}`")]
    NotFound(String),
    /// The suggestion is not in a state that allows this.
    #[error("the suggestion is {from} and cannot be {action}")]
    InvalidTransition {
        /// Current status.
        from: ProposalStatus,
        /// What was attempted.
        action: &'static str,
    },
    /// The suggestion no longer applies (the memory it changes was changed or removed).
    #[error("the suggestion no longer applies: {0}")]
    Stale(String),
    /// The suggestions database failed.
    #[error("suggestions database error: {0}")]
    Database(String),
    /// The memory layer failed or refused the write.
    #[error(transparent)]
    Memory(#[from] MemoryError),
}

impl From<rusqlite::Error> for LearnError {
    fn from(e: rusqlite::Error) -> Self {
        LearnError::Database(e.to_string())
    }
}

impl From<crate::audit::AuditError> for LearnError {
    fn from(e: crate::audit::AuditError) -> Self {
        LearnError::Database(e.to_string())
    }
}

impl From<crate::statements::StatementError> for LearnError {
    fn from(e: crate::statements::StatementError) -> Self {
        LearnError::Memory(MemoryError::from(e))
    }
}

/// Result alias of the learning pipeline.
pub type LearnResult<T> = Result<T, LearnError>;

/// Case-folds and collapses whitespace, for comparing quoted evidence with its source:
/// tolerant of formatting, never of content.
pub(crate) fn normalise_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `evidence` (non-trivial) occurs in `source` after [`normalise_text`].
pub(crate) fn quotes(source: &str, evidence: &str) -> bool {
    let evidence = normalise_text(evidence);
    evidence.chars().filter(|c| c.is_alphanumeric()).count() >= 3
        && normalise_text(source).contains(&evidence)
}

/// The UTC day key of `at` (`YYYY-MM-DD`).
pub(crate) fn day_key(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

/// Strips one surrounding Markdown code fence (```json ... ```), the only formatting a
/// model's JSON answer is forgiven.
pub(crate) fn strip_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.strip_suffix("```").unwrap_or(rest).trim()
}
