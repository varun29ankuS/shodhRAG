//! Agent harness: drives a pinned `omp` sidecar over its JSONL RPC protocol,
//! serves Shodh host tools, and normalises everything into one
//! [`AgentEvent`] stream for the UI.
//!
//! Decision record: `docs/adr/0001-agent-harness-omp.md`.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod events;
pub mod omp;
pub mod profile;
pub mod protocol;
pub mod tools;

pub use events::{AgentEvent, PlanItem, PlanStatus, RiskTier, RunStatus};
pub use omp::{normalise, NormaliserState, StepMeta, StepOutcome};
pub use profile::AgentProfile;

/// Truncate to at most `max` characters, appending an ellipsis when cut.
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::truncate_chars;

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 10), "héllo");
        assert_eq!(truncate_chars("héllo wörld", 5), "héll…");
    }
}
