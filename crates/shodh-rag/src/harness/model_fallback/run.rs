//! Runs one call down its [`Plan`]: the requested model, then at most
//! [`Bounds::max_fallbacks`] others, within [`Bounds::time_cap`], never after any of the
//! answer reached the user.

use std::future::Future;
use std::time::Duration;

use serde::Serialize;

use super::health::HealthBook;
use super::policy::{decide, Decision, Eligible, MAX_RETRY_WAIT};
use super::resolve::{Candidate, Plan};
use crate::harness::model_catalog::ModelRef;
use crate::harness::provider_error::ProviderErrorKind;
use crate::llm::ProviderFailure;

/// The time, for the time cap and the health book. Tests use a fake clock.
pub trait Clock: Send + Sync {
    /// Unix milliseconds.
    fn now_ms(&self) -> u64;
}

/// Waiting, for a rate limit's `Retry-After`. Tests use a fake that advances their clock.
#[async_trait::async_trait]
pub trait Pause: Send + Sync {
    async fn pause(&self, wait: Duration);
}

/// The computer's clock, and a real wait.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTime;

impl Clock for SystemTime {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl Pause for SystemTime {
    async fn pause(&self, wait: Duration) {
        tokio::time::sleep(wait).await;
    }
}

/// Limits of one call's fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Models tried besides the requested one.
    pub max_fallbacks: usize,
    /// No new attempt (or wait) starts after this much time.
    pub time_cap: Duration,
}

/// At most 2 fallbacks and 2 minutes.
pub const DEFAULT_BOUNDS: Bounds = Bounds {
    max_fallbacks: 2,
    time_cap: Duration::from_secs(120),
};

impl Default for Bounds {
    fn default() -> Self {
        DEFAULT_BOUNDS
    }
}

/// A failed attempt: the provider's failure, and whether any of the answer had already
/// reached the user (then nothing switches).
#[derive(Debug, Clone)]
pub struct AttemptFailure {
    pub failure: ProviderFailure,
    pub streamed: bool,
}

impl From<ProviderFailure> for AttemptFailure {
    fn from(failure: ProviderFailure) -> Self {
        Self {
            failure,
            streamed: false,
        }
    }
}

/// Why a model was left for the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum SwitchReason {
    /// It failed with this kind.
    Failed { kind: ProviderErrorKind },
    /// It failed recently and was skipped (this was its last failure).
    Skipped { last: Option<ProviderErrorKind> },
}

impl SwitchReason {
    /// The failure kind behind the switch, when known.
    pub fn kind(self) -> Option<ProviderErrorKind> {
        match self {
            SwitchReason::Failed { kind } => Some(kind),
            SwitchReason::Skipped { last } => last,
        }
    }
}

/// One model left for the next.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Switch {
    pub from: ModelRef,
    pub from_name: String,
    pub reason: SwitchReason,
}

/// Which model answered, and why it was not the requested one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnsweredBy {
    pub model: ModelRef,
    pub name: String,
    pub requested: ModelRef,
    pub requested_name: String,
    /// In order; empty when the requested model answered.
    pub switches: Vec<Switch>,
}

impl AnsweredBy {
    pub fn fell_back(&self) -> bool {
        self.model != self.requested
    }

    /// "Answered by Gemma 4 31B — Nemotron Super is unavailable", or `None` when the
    /// requested model answered.
    pub fn note(&self) -> Option<String> {
        if !self.fell_back() {
            return None;
        }
        let why = self
            .switches
            .iter()
            .find(|s| s.from == self.requested)
            .or_else(|| self.switches.first())
            .map(|s| format!("{} {}", s.from_name, reason_words(s.reason)));
        Some(match why {
            Some(why) => format!("Answered by {} — {why}", self.name),
            None => format!("Answered by {}", self.name),
        })
    }
}

/// Plain words for why a model was left ("is unavailable").
pub fn reason_words(reason: SwitchReason) -> &'static str {
    match reason {
        SwitchReason::Skipped { .. } => "has been failing, so it was skipped",
        SwitchReason::Failed { kind } => match kind {
            ProviderErrorKind::RateLimited => "is rate-limited",
            ProviderErrorKind::QuotaExhausted => "has used up its quota",
            ProviderErrorKind::ModelUnavailable => "is unavailable",
            ProviderErrorKind::Timeout => "did not answer in time",
            ProviderErrorKind::Network => "could not be reached",
            ProviderErrorKind::ContextTooLong => "cannot take a request this long",
            _ => "could not answer",
        },
    }
}

/// Why a call ended without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Another model would fail the same way (a bad key, a refusal, a bad request).
    NotHelpful,
    /// Part of the answer had reached the user.
    Streamed,
    /// No model fitting the failure was left.
    NoCandidate,
    /// [`Bounds::max_fallbacks`] models were tried after the requested one.
    MaxFallbacks,
    /// [`Bounds::time_cap`] passed.
    TimeCap,
    /// Every model was skipped for failing recently; none was called.
    AllSkipped,
}

/// A call that got no answer.
#[derive(Debug, Clone)]
pub struct Exhausted {
    /// The last failure (for [`StopReason::AllSkipped`], one made up from the requested
    /// model's health).
    pub failure: ProviderFailure,
    pub reason: StopReason,
    pub switches: Vec<Switch>,
    /// Models actually called, in order.
    pub tried: Vec<ModelRef>,
}

/// What a run of a plan gave.
#[derive(Debug, Clone)]
pub struct Answered<T> {
    pub value: T,
    pub by: AnsweredBy,
    pub tried: Vec<ModelRef>,
}

fn skipped_failure(candidate: &Candidate) -> ProviderFailure {
    let kind = candidate
        .health
        .last_failure()
        .unwrap_or(ProviderErrorKind::ModelUnavailable);
    // Worded so the failure classifies as its kind again from text alone.
    let what = match kind {
        ProviderErrorKind::Timeout => "it timed out moments ago",
        ProviderErrorKind::Network => "a connection error ended its last call moments ago",
        _ => "it is temporarily unavailable (it failed moments ago)",
    };
    ProviderFailure {
        kind,
        provider: candidate.model.provider.label().to_string(),
        upstream: None,
        model: candidate.model.model.clone(),
        status: None,
        detail: format!("Skipped: {what}; it is tried again after a short wait."),
    }
}

/// Runs `call` down `plan` (see the module docs and [`super::policy`]), recording each
/// outcome in `health`.
pub async fn run_plan<T, F, Fut>(
    plan: &Plan,
    health: &HealthBook,
    clock: &dyn Clock,
    pause: &dyn Pause,
    bounds: Bounds,
    mut call: F,
) -> Result<Answered<T>, Exhausted>
where
    F: FnMut(Candidate) -> Fut,
    Fut: Future<Output = Result<T, AttemptFailure>>,
{
    let started = clock.now_ms();
    let cap_ms = u64::try_from(bounds.time_cap.as_millis()).unwrap_or(u64::MAX);
    let candidates: Vec<&Candidate> = plan.candidates().collect();
    let mut switches: Vec<Switch> = Vec::new();
    let mut tried: Vec<ModelRef> = Vec::new();
    let mut last: Option<ProviderFailure> = None;
    let mut eligible = Eligible::Any;
    let mut retry_after: Option<Duration> = None;
    let mut waited = false;
    let mut fallbacks = 0usize;
    let mut index = 0usize;
    // The model a rate limit may wait out and retry (once), when nothing else fits.
    let mut retry_same: Option<&Candidate> = None;

    let exhausted = |failure: Option<ProviderFailure>,
                     reason: StopReason,
                     switches: Vec<Switch>,
                     tried: Vec<ModelRef>| Exhausted {
        failure: failure.unwrap_or_else(|| skipped_failure(&plan.requested)),
        reason: if tried.is_empty() && reason == StopReason::NoCandidate {
            StopReason::AllSkipped
        } else {
            reason
        },
        switches,
        tried,
    };

    loop {
        let candidate = match retry_same.take() {
            Some(same) => same,
            None => {
                // The next eligible candidate.
                let next = candidates[index..]
                    .iter()
                    .position(|c| index == 0 || eligible.allows(c))
                    .map(|offset| index + offset);
                match next {
                    Some(i) => {
                        index = i + 1;
                        candidates[i]
                    }
                    None => {
                        // Nothing else fits: a short rate-limit wait, once, on the same model.
                        let elapsed = clock.now_ms().saturating_sub(started);
                        let wait_ms = retry_after
                            .filter(|w| *w <= MAX_RETRY_WAIT)
                            .map(|w| u64::try_from(w.as_millis()).unwrap_or(u64::MAX));
                        let last_model = tried.last().cloned();
                        let same = last_model
                            .and_then(|m| candidates.iter().copied().find(|c| c.model == m));
                        match (wait_ms, same) {
                            (Some(wait_ms), Some(same))
                                if !waited && elapsed.saturating_add(wait_ms) < cap_ms =>
                            {
                                waited = true;
                                retry_after = None;
                                pause.pause(Duration::from_millis(wait_ms)).await;
                                retry_same = Some(same);
                                continue;
                            }
                            _ => {
                                return Err(exhausted(
                                    last,
                                    StopReason::NoCandidate,
                                    switches,
                                    tried,
                                ))
                            }
                        }
                    }
                }
            }
        };
        let is_fallback = candidate.model != plan.requested.model;
        let new_model = !tried.contains(&candidate.model);
        if is_fallback && new_model {
            if fallbacks >= bounds.max_fallbacks {
                return Err(exhausted(last, StopReason::MaxFallbacks, switches, tried));
            }
            if clock.now_ms().saturating_sub(started) >= cap_ms {
                return Err(exhausted(last, StopReason::TimeCap, switches, tried));
            }
        }
        let Some(admission) = health.admit(&candidate.model, clock.now_ms()) else {
            switches.push(Switch {
                from: candidate.model.clone(),
                from_name: candidate.name.clone(),
                reason: SwitchReason::Skipped {
                    last: health
                        .health(&candidate.model, clock.now_ms())
                        .last_failure(),
                },
            });
            continue;
        };
        if is_fallback && new_model {
            fallbacks += 1;
        }
        if new_model {
            tried.push(candidate.model.clone());
        }
        match call(candidate.clone()).await {
            Ok(value) => {
                admission.succeeded();
                return Ok(Answered {
                    value,
                    by: AnsweredBy {
                        model: candidate.model.clone(),
                        name: candidate.name.clone(),
                        requested: plan.requested.model.clone(),
                        requested_name: plan.requested.name.clone(),
                        switches,
                    },
                    tried,
                });
            }
            Err(attempt) => {
                let now = clock.now_ms();
                admission.failed(attempt.failure.kind, now);
                let kind = attempt.failure.kind;
                if attempt.streamed {
                    return Err(exhausted(
                        Some(attempt.failure),
                        StopReason::Streamed,
                        switches,
                        tried,
                    ));
                }
                match decide(&attempt.failure, candidate, now) {
                    Decision::Stop => {
                        return Err(exhausted(
                            Some(attempt.failure),
                            StopReason::NotHelpful,
                            switches,
                            tried,
                        ))
                    }
                    Decision::Switch {
                        eligible: next,
                        retry_after: wait,
                    } => {
                        switches.push(Switch {
                            from: candidate.model.clone(),
                            from_name: candidate.name.clone(),
                            reason: SwitchReason::Failed { kind },
                        });
                        eligible = next;
                        retry_after = wait;
                        last = Some(attempt.failure);
                    }
                }
            }
        }
    }
}
