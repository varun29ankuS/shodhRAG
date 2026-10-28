//! Test doubles for callers of the fallback (Slack, Telegram): a scripted provider per
//! model id, and a fallback model over a fixed plan with a frozen clock. No network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::health::{Health, HealthBook};
use super::resolve::{CallNeeds, Candidate, Origin, Plan};
use super::run::{Bounds, Clock, Pause, DEFAULT_BOUNDS};
use super::text_model::{CandidateModel, FallbackTextModel, Timing};
use crate::harness::model_catalog::{ModelRef, PrivacyNote, ProviderId};
use crate::llm::failure::CallContext;
use crate::llm::ProviderFailure;
use crate::research::results::ModelReply;

/// An OpenRouter candidate named `name`.
pub fn candidate(id: &str, name: &str, origin: Origin) -> Candidate {
    Candidate {
        model: ModelRef::new(ProviderId::OpenRouter, id),
        name: name.to_string(),
        origin,
        context_length: Some(131_072),
        privacy: PrivacyNote::MayLogPrompts,
        free: id.ends_with(":free"),
        paid: !id.ends_with(":free"),
        health: Health::Healthy,
        availability: None,
        reachable: true,
    }
}

/// A 404 "no endpoints" from OpenRouter for `id`, as the provider client reports it.
pub fn unavailable(id: &str) -> ProviderFailure {
    CallContext::new("OpenRouter", id, "").http(
        404,
        r#"{"error":{"message":"No endpoints found for this model.","code":404}}"#,
    )
}

/// OpenRouter's daily free-model limit, resetting at `reset_ms`.
pub fn daily_quota(id: &str, reset_ms: u64) -> ProviderFailure {
    CallContext::new("OpenRouter", id, "").http(
        429,
        &format!(
            r#"{{"error":{{"message":"Rate limit exceeded: free-models-per-day. Add 10 credits to unlock 1000 free model requests per day","code":429,"metadata":{{"headers":{{"X-RateLimit-Limit":"50","X-RateLimit-Remaining":"0","X-RateLimit-Reset":"{reset_ms}"}}}}}}}}"#
        ),
    )
}

/// Answers per model id; records the ids called.
pub struct ScriptedModels {
    replies: HashMap<String, Result<String, ProviderFailure>>,
    pub calls: Mutex<Vec<String>>,
}

impl ScriptedModels {
    pub fn new(replies: Vec<(&str, Result<&str, ProviderFailure>)>) -> Arc<Self> {
        Arc::new(Self {
            replies: replies
                .into_iter()
                .map(|(id, r)| (id.to_string(), r.map(str::to_string)))
                .collect(),
            calls: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl CandidateModel for ScriptedModels {
    async fn complete(
        &self,
        candidate: &Candidate,
        _prompt: &str,
        _max_tokens: usize,
    ) -> Result<ModelReply, ProviderFailure> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(candidate.model.model.clone());
        match self.replies.get(&candidate.model.model) {
            Some(Ok(text)) => Ok(ModelReply {
                text: text.clone(),
                truncated: false,
            }),
            Some(Err(failure)) => Err(failure.clone()),
            None => Err(unavailable(&candidate.model.model)),
        }
    }
}

/// A clock stopped at `0` ms past an instant, which waits do not move.
pub struct Frozen(pub u64);

impl Clock for Frozen {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

#[async_trait::async_trait]
impl Pause for Frozen {
    async fn pause(&self, _wait: Duration) {}
}

/// A fallback model over `plan`, calling `models`, at `now_ms`.
pub fn fallback_model(plan: Plan, models: Arc<ScriptedModels>, now_ms: u64) -> FallbackTextModel {
    fallback_model_with(plan, models, now_ms, DEFAULT_BOUNDS)
}

pub fn fallback_model_with(
    plan: Plan,
    models: Arc<ScriptedModels>,
    now_ms: u64,
    bounds: Bounds,
) -> FallbackTextModel {
    let frozen = Arc::new(Frozen(now_ms));
    FallbackTextModel::new(
        Arc::new(move |_needs: CallNeeds| plan.clone()),
        models,
        Arc::new(HealthBook::new()),
        Timing {
            clock: frozen.clone(),
            pause: frozen,
            bounds,
        },
        None,
    )
}
