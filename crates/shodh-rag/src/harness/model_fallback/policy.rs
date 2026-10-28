//! What a failed call does next: try another model, wait once and retry, or stop.
//!
//! Falling back only when it can help:
//! - **Unavailable, 5xx, timeout, broken connection**: the next candidate at once.
//! - **Rate limits.** OpenRouter limits free models (ids ending `:free`) per account, at
//!   20 requests a minute and 50 a day (1000 a day once 10 credits were bought), and
//!   "governs capacity globally": more keys or accounts do not raise them
//!   (<https://openrouter.ai/docs/api/reference/limits>, read 2026-10-28). Its errors
//!   name those limits `free-models-per-min` and `free-models-per-day`. So when the
//!   *account's* limit was hit, another free model hits the same limit: only a model
//!   outside it helps (another provider, or a paid model). With none, a short
//!   `Retry-After` (at most [`MAX_RETRY_WAIT`]) is waited once. An *upstream* rate limit
//!   (one provider of one free model is saturated; OpenRouter says "rate-limited
//!   upstream" and names the provider) is about that model only: the next candidate
//!   helps. Any other provider's rate limit counts against that provider's account.
//! - **Quota used up** (the free models' daily quota, credits): a model outside that
//!   quota helps; another free model does not. With none, the call stops and says when
//!   the quota resets.
//! - **Bad key, content refused, bad request, empty or unknown failures**: never another
//!   model; the real reason is shown.
//! - **Context too long**: only a candidate with a larger context window.

use std::time::Duration;

use super::resolve::Candidate;
use crate::harness::provider_error::{classify, limit_reset_at_ms, ProviderErrorKind};
use crate::llm::ProviderFailure;

/// The longest `Retry-After` waited for (once) instead of giving up.
pub const MAX_RETRY_WAIT: Duration = Duration::from_secs(20);

/// Which candidates may follow a failure.
#[derive(Debug, Clone, PartialEq)]
pub enum Eligible {
    /// Any candidate.
    Any,
    /// Only candidates outside the limit `failed` hit: for a free OpenRouter model, a
    /// model that is not free on OpenRouter; otherwise another provider.
    OutsideLimitOf(Candidate),
    /// Only candidates with a context window larger than this (any known one when the
    /// failed model's is unknown).
    LargerContext(Option<u32>),
}

impl Eligible {
    pub fn allows(&self, candidate: &Candidate) -> bool {
        match self {
            Eligible::Any => true,
            Eligible::OutsideLimitOf(failed) => {
                if failed.free {
                    !candidate.free
                } else {
                    candidate.model.provider != failed.model.provider
                }
            }
            Eligible::LargerContext(limit) => match (candidate.context_length, limit) {
                (Some(context), Some(limit)) => context > *limit,
                (Some(_), None) => true,
                (None, _) => false,
            },
        }
    }
}

/// What to do after a failure.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Try the next candidate that is [`Eligible`]. `retry_after` is the provider's
    /// wait, for when no candidate is eligible (rate limits only).
    Switch {
        eligible: Eligible,
        retry_after: Option<Duration>,
    },
    /// Another model would fail the same way, or must not be used: stop.
    Stop,
}

/// The account's own limit, not one provider of one model: hopping free models does
/// not help.
fn account_limit(lower: &str) -> bool {
    lower.contains("free-models-per-min") || lower.contains("free-models-per-day")
}

/// One saturated provider of one model (OpenRouter's "rate-limited upstream").
fn upstream_limit(lower: &str, failure: &ProviderFailure) -> bool {
    lower.contains("rate-limited upstream")
        || lower.contains("rate limited upstream")
        || (failure.upstream.is_some() && !account_limit(lower))
}

/// The decision for `failure` of `failed`, made at `now_ms`.
pub fn decide(failure: &ProviderFailure, failed: &Candidate, now_ms: u64) -> Decision {
    let switch = |eligible| Decision::Switch {
        eligible,
        retry_after: None,
    };
    match failure.kind {
        ProviderErrorKind::ModelUnavailable
        | ProviderErrorKind::Timeout
        | ProviderErrorKind::Network => switch(Eligible::Any),
        ProviderErrorKind::RateLimited => {
            let text = format!("{} {}", failure.status.unwrap_or(429), failure.detail);
            let lower = text.to_ascii_lowercase();
            if failed.free && upstream_limit(&lower, failure) {
                return switch(Eligible::Any);
            }
            let retry_after = classify(&text, None, now_ms)
                .retry_after_secs
                .map(|secs| Duration::from_secs(u64::from(secs)));
            Decision::Switch {
                eligible: Eligible::OutsideLimitOf(failed.clone()),
                retry_after,
            }
        }
        ProviderErrorKind::QuotaExhausted => switch(Eligible::OutsideLimitOf(failed.clone())),
        ProviderErrorKind::ContextTooLong => switch(Eligible::LargerContext(failed.context_length)),
        ProviderErrorKind::Auth
        | ProviderErrorKind::ContentFiltered
        | ProviderErrorKind::InvalidRequest
        | ProviderErrorKind::EmptyResponse
        | ProviderErrorKind::Other => Decision::Stop,
    }
}

/// When the limit `failure` reports resets (Unix milliseconds), if it says: OpenRouter
/// copies `X-RateLimit-Reset` into its error, and the failure's detail keeps it first.
pub fn reset_at_ms(failure: &ProviderFailure, now_ms: u64) -> Option<u64> {
    limit_reset_at_ms(&failure.detail, now_ms)
}

/// The free-model daily quota is what ran out (rather than credits, or a minute's
/// limit).
pub fn is_daily_free_quota(failure: &ProviderFailure) -> bool {
    let lower = failure.detail.to_ascii_lowercase();
    lower.contains("free-models-per-day")
        || (failure.kind == ProviderErrorKind::QuotaExhausted
            && lower.contains("per-day")
            && lower.contains("free"))
}
