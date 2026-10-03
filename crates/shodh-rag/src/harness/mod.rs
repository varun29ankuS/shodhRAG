//! Agent harness: drives a pinned `omp` sidecar over its JSONL RPC protocol,
//! serves Shodh host tools, and normalises everything into one
//! [`AgentEvent`] stream for the UI.
//!
//! Decision record: `docs/adr/0001-agent-harness-omp.md`.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
// CI runs clippy on this module with every warning (rustc + default clippy set) denied.
// Gated on `clippy` so a new compiler lint never breaks a normal build.
#![cfg_attr(clippy, deny(warnings))]

pub mod error;
pub mod events;
pub mod model;
pub mod omp;
pub mod profile;
pub mod protocol;
pub mod session;
pub mod sidecar;
pub mod tools;
pub mod web;

pub use error::HarnessError;
pub use events::{AgentEvent, PlanItem, PlanStatus, RiskTier, RunStatus};
pub use model::{select_model, OmpModel};
pub use omp::{normalise, NormaliserState, StepMeta, StepOutcome};
pub use profile::AgentProfile;
pub use session::{OmpSession, SessionConfig};
pub use sidecar::{
    fetch_omp, resolve_binary_path, FetchProgress, InstalledRuntime, LaunchSpec, OmpLayout,
    OMP_VERSION,
};

/// One agent session as the app sees it. `OmpSession` is the production
/// implementation; spec §7.1's in-process fallback would implement the same
/// trait. Events are delivered on the receiver returned when the session is
/// started.
// async-trait adds `#[must_use]` to the boxed futures it generates, which clippy's
// `double_must_use` flags on code we don't write; the lint does not apply here.
#[allow(clippy::double_must_use)]
#[async_trait::async_trait]
pub trait AgentHarness: Send + Sync {
    fn session_id(&self) -> &str;

    /// `provider/model` the session answers with.
    fn model(&self) -> &str;

    /// Start a run for a new user message. Fails with
    /// [`HarnessError::RunInProgress`] while a run is active. Returns the run id
    /// (`run_id` when given).
    async fn prompt(&self, text: &str, run_id: Option<String>) -> Result<String, HarnessError>;

    /// Redirect the active run; starts a new run when idle. Returns the run id.
    async fn steer(&self, text: &str) -> Result<String, HarnessError>;

    /// Interrupt the active run (stops the provider call).
    async fn abort(&self) -> Result<(), HarnessError>;

    /// Resolve a pending approval.
    fn approve(&self, step_id: &str, approved: bool) -> Result<(), HarnessError>;

    /// Stop the sidecar. Idempotent.
    async fn shutdown(&self);
}

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
