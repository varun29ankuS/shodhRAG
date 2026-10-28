//! The fallback as a [`TextModel`]: what Slack, Telegram, learning and one-shot
//! extraction call. Each call plans its candidates for its own prompt (its size decides
//! which context windows fit), runs down them ([`run_plan`]) and reports what happened
//! to an observer (audit, notices). The model that answered is kept for the caller
//! ([`TextModel::answered_by`]).

use std::sync::{Arc, Mutex};

use super::health::HealthBook;
use super::resolve::{CallNeeds, Candidate, Plan};
use super::run::{run_plan, AnsweredBy, AttemptFailure, Bounds, Clock, Exhausted, Pause};
use crate::llm::ProviderFailure;
use crate::research::results::{ModelReply, TextModel};
use crate::user_memory::learn::{LearnError, LearnModel};

/// Plans a call's candidates for what it needs.
pub trait Planner: Send + Sync {
    fn plan(&self, needs: CallNeeds) -> Plan;
}

impl<F> Planner for F
where
    F: Fn(CallNeeds) -> Plan + Send + Sync,
{
    fn plan(&self, needs: CallNeeds) -> Plan {
        self(needs)
    }
}

/// Calls one candidate. Failures are typed, so the policy can tell them apart.
#[async_trait::async_trait]
pub trait CandidateModel: Send + Sync {
    async fn complete(
        &self,
        candidate: &Candidate,
        prompt: &str,
        max_tokens: usize,
    ) -> Result<ModelReply, ProviderFailure>;
}

/// How one call went, for the audit and notices.
#[derive(Debug, Clone)]
pub enum CallReport {
    Answered(AnsweredBy),
    Failed {
        plan: Box<Plan>,
        exhausted: Box<Exhausted>,
    },
}

/// Told about every call.
pub trait FallbackObserver: Send + Sync {
    fn observe(&self, report: &CallReport);
}

/// The time and the waits a call uses.
#[derive(Clone)]
pub struct Timing {
    pub clock: Arc<dyn Clock>,
    pub pause: Arc<dyn Pause>,
    pub bounds: Bounds,
}

/// A [`TextModel`] that falls back down its plan.
pub struct FallbackTextModel {
    planner: Arc<dyn Planner>,
    models: Arc<dyn CandidateModel>,
    health: Arc<HealthBook>,
    timing: Timing,
    observer: Option<Arc<dyn FallbackObserver>>,
    /// The id calls ask for (the requested model of an empty-prompt plan).
    model_id: String,
    last: Mutex<Option<AnsweredBy>>,
}

impl FallbackTextModel {
    pub fn new(
        planner: Arc<dyn Planner>,
        models: Arc<dyn CandidateModel>,
        health: Arc<HealthBook>,
        timing: Timing,
        observer: Option<Arc<dyn FallbackObserver>>,
    ) -> Self {
        let model_id = planner.plan(CallNeeds::direct("")).requested.model.model;
        Self {
            planner,
            models,
            health,
            timing,
            observer,
            model_id,
            last: Mutex::new(None),
        }
    }

    /// One call: the reply, or the last failure once nothing else may be tried.
    pub async fn complete_with_fallback(
        &self,
        prompt: &str,
        max_tokens: usize,
    ) -> Result<(ModelReply, AnsweredBy), Box<Exhausted>> {
        let plan = self.planner.plan(CallNeeds::direct(prompt));
        let models = &self.models;
        let outcome = run_plan(
            &plan,
            &self.health,
            self.timing.clock.as_ref(),
            self.timing.pause.as_ref(),
            self.timing.bounds,
            |candidate| async move {
                models
                    .complete(&candidate, prompt, max_tokens)
                    .await
                    .map_err(AttemptFailure::from)
            },
        )
        .await;
        match outcome {
            Ok(answered) => {
                if let Some(observer) = &self.observer {
                    observer.observe(&CallReport::Answered(answered.by.clone()));
                }
                *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(answered.by.clone());
                Ok((answered.value, answered.by))
            }
            Err(exhausted) => {
                if let Some(observer) = &self.observer {
                    observer.observe(&CallReport::Failed {
                        plan: Box::new(plan),
                        exhausted: exhausted.clone(),
                    });
                }
                Err(exhausted)
            }
        }
    }
}

#[async_trait::async_trait]
impl TextModel for FallbackTextModel {
    fn model_id(&self) -> String {
        self.model_id.clone()
    }

    async fn complete(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
        self.complete_reply(prompt, max_tokens)
            .await
            .map(|reply| reply.text)
    }

    async fn complete_reply(&self, prompt: &str, max_tokens: usize) -> Result<ModelReply, String> {
        self.complete_with_fallback(prompt, max_tokens)
            .await
            .map(|(reply, _)| reply)
            .map_err(|exhausted| exhausted.failure.to_string())
    }

    fn answered_by(&self) -> Option<AnsweredBy> {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Memory learning through the fallback: its answers are strict JSON, so a reply cut off
/// at the limit is refused.
#[async_trait::async_trait]
impl LearnModel for FallbackTextModel {
    fn model_id(&self) -> String {
        self.model_id.clone()
    }

    async fn complete(&self, prompt: &str, max_output_tokens: usize) -> Result<String, LearnError> {
        let (reply, _) = self
            .complete_with_fallback(prompt, max_output_tokens)
            .await
            .map_err(|exhausted| LearnError::Model(exhausted.failure.to_string()))?;
        if reply.truncated {
            return Err(LearnError::Truncated(reply.text));
        }
        Ok(reply.text)
    }
}
