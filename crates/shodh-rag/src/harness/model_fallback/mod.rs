//! One backend fallback for every model call (Ask, side runs, Slack, Telegram, memory
//! learning, extraction).
//!
//! - [`resolve`]: the candidates a call may try, in order — the model it asked for, the
//!   user's chosen fallback, the rest of the user's pick, then automatic candidates —
//!   filtered by what the call needs ([`CallNeeds`]: tools, images, context, privacy,
//!   Local-only mode) and ranked by health and live availability.
//! - [`HealthBook`]: models that failed recently are skipped for a while (a circuit
//!   breaker), in memory.
//! - [`AvailabilityBook`]: OpenRouter's live endpoint status and uptime for free
//!   candidates, fetched in the background.
//! - [`decide`]: falls back only when another model can help (see [`policy`]).
//! - [`run_plan`]: at most two fallbacks, a time cap, never after any of the answer
//!   reached the user; records which model answered and why it switched.
//! - [`FallbackTextModel`]: all of it as a [`crate::research::results::TextModel`].

pub mod availability;
pub mod health;
pub mod policy;
pub mod resolve;
pub mod run;
pub mod text_model;

#[cfg(test)]
pub mod testkit;
#[cfg(test)]
mod tests;

pub use availability::{
    endpoints_url, fetch_availability, parse_endpoints, Availability, AvailabilityBook,
    AVAILABILITY_TTL_MS,
};
pub use health::{
    cool_down_ms, trips_breaker, Admission, Health, HealthBook, FIRST_COOL_DOWN_MS, IDLE_ENTRY_MS,
    LONG_FAILURE_MS, MAX_COOL_DOWN_MS, PROBE_LEASE_MS, STREAK_GAP_MS,
};
pub use policy::{decide, is_daily_free_quota, reset_at_ms, Decision, Eligible, MAX_RETRY_WAIT};
pub use resolve::{
    availability_ids, estimate_tokens, resolve, CallNeeds, Caller, Candidate, Origin, Plan,
    ResolveInputs, MAX_AUTOMATIC,
};
pub use run::{
    reason_words, run_plan, Answered, AnsweredBy, AttemptFailure, Bounds, Clock, Exhausted, Pause,
    StopReason, Switch, SwitchReason, SystemTime, DEFAULT_BOUNDS,
};
pub use text_model::{
    CallReport, CandidateModel, FallbackObserver, FallbackTextModel, Planner, Timing,
};
