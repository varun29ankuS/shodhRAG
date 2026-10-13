//! Grounded answers: citations as an enforced output, not a request.
//!
//! After the model finishes an answer, the session ([`super::session`])
//! checks it before the run ends:
//! * [`claims`] splits the answer into claims (sentences, clauses, list
//!   items, table rows; code and math excluded);
//! * [`citations`] parses each claim's `[n]` markers;
//! * [`verify`] checks each claim against the passages it cites with local
//!   models (an entailment model when installed, else the reranker) plus
//!   number and word checks, never an LLM judge, and checks the question's
//!   information needs against every passage;
//! * [`followup`] decides whether to ask the model once to re-ground flagged
//!   claims and/or (at most twice) to search for uncovered needs;
//! * [`absence`] flags a claim that the user has none of their own data
//!   ("You have no tasks") unless a tool that reads that data succeeded in
//!   the run;
//! * [`preface`] keeps a follow-up turn's opening words to the app (an
//!   apology, "here is the corrected answer") out of the answer.
//!
//! Every check is emitted as an `AgentEvent::Grounding` and audited with the
//! answer.

pub mod absence;
pub mod citations;
pub mod claims;
pub mod followup;
pub mod numbers;
pub mod preface;
pub mod verify;

#[cfg(test)]
mod calibration;

use std::sync::Arc;

pub use verify::{
    check_needs, summarise, verify_answer, AnswerMessage, EntailmentScorer, Evidence, OpenedText,
    ScorerSet, Scorers, SharedEntailment, VerifyInput, THRESHOLDS,
};

/// Supplies the local models installed right now (they can be installed
/// while a session runs, so they are asked for at each check).
pub type ScorerProvider = Arc<dyn Fn() -> ScorerSet + Send + Sync>;

/// Reads the user's auto-repair setting at each check.
pub type RepairSetting = Arc<dyn Fn() -> bool + Send + Sync>;

/// How a session grounds its answers.
#[derive(Clone)]
pub struct GroundingConfig {
    pub scorers: ScorerProvider,
    pub auto_repair: RepairSetting,
    /// Whether the check may ask the model for follow-up turns (repair and
    /// coverage). Off for sessions whose answers the app parses in a fixed
    /// format (side threads), which a rewrite could break; their answers
    /// are still checked and flagged.
    pub follow_ups: bool,
}

impl GroundingConfig {
    /// Word-overlap and number checks only, follow-ups and auto-repair on.
    pub fn lexical() -> Self {
        Self {
            scorers: Arc::new(ScorerSet::default),
            auto_repair: Arc::new(|| true),
            follow_ups: true,
        }
    }
}

impl std::fmt::Debug for GroundingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroundingConfig").finish_non_exhaustive()
    }
}
