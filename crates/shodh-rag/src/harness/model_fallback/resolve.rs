//! Which models a call may try, in order.
//!
//! The order is: the model the call asked for → the user's chosen fallback → the rest
//! of the user's pick (when they chose one) → automatic candidates. Automatic
//! candidates are ranked by health, then live uptime, then their catalog order, and a
//! candidate that does not fit the request (tools, images, context, privacy,
//! Local-only mode, how the call reaches the model) is never offered.

use serde::Serialize;

use super::availability::{Availability, AvailabilityBook};
use super::health::{Health, HealthBook};
use crate::harness::model::is_valid_model_id;
use crate::harness::model_catalog::{
    CatalogModel, ModelRef, ModelTier, PrivacyNote, ProviderId, ProviderKind, ToolSupport,
    OPENROUTER_FAST_FALLBACK,
};
use crate::harness::model_picks::PickTable;

/// Automatic candidates kept after ranking.
pub const MAX_AUTOMATIC: usize = 4;
/// Free models whose availability is fetched for one plan, at most.
pub const MAX_AVAILABILITY_IDS: usize = 24;

/// How the call reaches the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Caller {
    /// Through the agent runtime (Ask, side runs): tools are needed, and Ollama does
    /// not work there yet.
    Agent,
    /// One HTTP call from the app (Slack, Telegram, learning, extraction): a
    /// subscription cannot answer it (only the agent runtime holds its sign-in).
    Direct,
}

/// What the call needs from a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallNeeds {
    pub caller: Caller,
    /// Images go to the model as images (not as text read from them).
    pub images: bool,
    /// The prompt's estimated size in tokens, when known.
    pub prompt_tokens: Option<u32>,
}

impl CallNeeds {
    pub fn agent() -> Self {
        Self {
            caller: Caller::Agent,
            images: false,
            prompt_tokens: None,
        }
    }

    /// A direct call with `prompt` (its size estimated at four characters a token).
    pub fn direct(prompt: &str) -> Self {
        Self {
            caller: Caller::Direct,
            images: false,
            prompt_tokens: Some(estimate_tokens(prompt)),
        }
    }
}

/// About four characters a token: generous for English, so a fit is not overstated.
pub fn estimate_tokens(text: &str) -> u32 {
    u32::try_from(text.chars().count().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Room kept beside the prompt for the answer when checking a context window.
const ANSWER_ROOM_TOKENS: u32 = 1_024;

/// Where a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The model the call asked for.
    Requested,
    /// The user's chosen fallback model.
    ChosenFallback,
    /// The rest of the user's pick.
    PickChain,
    /// Chosen by the app.
    Automatic,
}

impl Origin {
    /// The user named this model (it may be less private than the requested one).
    pub fn named_by_user(self) -> bool {
        !matches!(self, Origin::Automatic)
    }
}

/// One model a call may try.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub model: ModelRef,
    /// Display name (the catalog's, else the id).
    pub name: String,
    pub origin: Origin,
    pub context_length: Option<u32>,
    pub privacy: PrivacyNote,
    /// A free OpenRouter model (shares the account's free-model limits).
    pub free: bool,
    pub health: Health,
    /// OpenRouter's live availability, when fetched.
    pub availability: Option<Availability>,
}

/// What [`resolve`] decides from.
#[derive(Debug, Clone)]
pub struct ResolveInputs<'a> {
    pub requested: &'a ModelRef,
    pub chosen_fallback: Option<&'a ModelRef>,
    /// The user's pick, resolved (`model_picks::resolve_chain`); empty without one.
    pub pick_chain: &'a [ModelRef],
    pub catalog: &'a [CatalogModel],
    pub table: &'a PickTable,
    /// The provider fallback order (`model_picks::provider_order`).
    pub order: &'a [ProviderId],
    /// Providers that can answer now.
    pub connected: &'a [ProviderId],
    /// Local-only mode: nothing leaves this computer.
    pub local_only: bool,
    /// `SHODH_ALLOW_STEALTH_MODELS=1`.
    pub allow_stealth: bool,
    /// Stealth models the user confirmed.
    pub stealth_accepted: &'a [String],
    pub needs: CallNeeds,
}

/// The models a call tries: `requested` first, then `fallbacks` in order.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub requested: Candidate,
    pub fallbacks: Vec<Candidate>,
}

impl Plan {
    /// A plan without fallbacks.
    pub fn only(requested: Candidate) -> Self {
        Self {
            requested,
            fallbacks: Vec::new(),
        }
    }

    /// Every candidate, the requested model first.
    pub fn candidates(&self) -> impl Iterator<Item = &Candidate> {
        std::iter::once(&self.requested).chain(&self.fallbacks)
    }
}

fn privacy_rank(privacy: PrivacyNote) -> u8 {
    match privacy {
        PrivacyNote::OnDevice => 2,
        PrivacyNote::ProviderTerms => 1,
        PrivacyNote::MayLogPrompts => 0,
    }
}

fn is_free_id(model: &ModelRef) -> bool {
    model.provider == ProviderId::OpenRouter && model.model.ends_with(":free")
}

impl ResolveInputs<'_> {
    fn listed(&self, model: &ModelRef) -> Option<&CatalogModel> {
        self.catalog
            .iter()
            .find(|m| m.provider == model.provider && m.id == model.model)
    }

    fn privacy_of(&self, model: &ModelRef) -> PrivacyNote {
        match self.listed(model) {
            Some(m) => m.privacy,
            None if model.provider.is_local() => PrivacyNote::OnDevice,
            None if is_free_id(model) || model.is_stealth() => PrivacyNote::MayLogPrompts,
            None => PrivacyNote::ProviderTerms,
        }
    }

    fn name_of(&self, model: &ModelRef) -> String {
        self.listed(model)
            .map(|m| m.name.clone())
            .or_else(|| {
                self.table
                    .providers
                    .get(&model.provider)
                    .and_then(|row| row.models.iter().find(|m| m.id == model.model))
                    .map(|m| m.name.clone())
            })
            .unwrap_or_else(|| model.model.clone())
    }

    fn candidate(
        &self,
        model: &ModelRef,
        origin: Origin,
        health: &HealthBook,
        availability: &AvailabilityBook,
        now_ms: u64,
    ) -> Candidate {
        let listed = self.listed(model);
        Candidate {
            model: model.clone(),
            name: self.name_of(model),
            origin,
            context_length: listed.and_then(|m| m.context_length),
            privacy: self.privacy_of(model),
            free: is_free_id(model) || listed.is_some_and(|m| m.tier == ModelTier::Free),
            health: health.health(model, now_ms),
            availability: (model.provider == ProviderId::OpenRouter)
                .then(|| availability.get(&model.model, now_ms))
                .flatten(),
        }
    }

    /// Whether the call can reach `model` at all: connected, allowed by Local-only mode
    /// and the stealth opt-in, and usable by this kind of caller.
    fn reachable(&self, model: &ModelRef) -> bool {
        let caller_ok = match self.needs.caller {
            Caller::Agent => model.provider.works_with_agent(),
            Caller::Direct => model.provider.kind() != ProviderKind::Subscription,
        };
        caller_ok
            && is_valid_model_id(&model.model)
            && self.connected.contains(&model.provider)
            && (!self.local_only || model.provider.is_local())
            && (!model.is_stealth()
                || self.allow_stealth
                || self.stealth_accepted.iter().any(|id| id == &model.model))
    }

    /// Whether `candidate` fits what the call needs. `floor` is the requested model's
    /// privacy, which an automatic candidate may not go below.
    fn fits(&self, candidate: &Candidate, floor: PrivacyNote) -> bool {
        let listed = self.listed(&candidate.model);
        if self.needs.caller == Caller::Agent && listed.is_some_and(|m| m.tools == ToolSupport::No)
        {
            return false;
        }
        if self.needs.images && listed.and_then(|m| m.image_input) != Some(true) {
            return false;
        }
        if let (Some(prompt), Some(context)) = (self.needs.prompt_tokens, candidate.context_length)
        {
            if prompt.saturating_add(ANSWER_ROOM_TOKENS) > context {
                return false;
            }
        }
        if !candidate.origin.named_by_user()
            && privacy_rank(candidate.privacy) < privacy_rank(floor)
        {
            return false;
        }
        !candidate.availability.is_some_and(|a| a.is_down())
    }

    /// The automatic candidates in catalog order: in Local-only mode the local models;
    /// otherwise the fast model of each connected direct provider (in the fallback
    /// order), the fast model through OpenRouter, models on this computer, then free
    /// OpenRouter models (largest context first).
    fn automatic(&self) -> Vec<ModelRef> {
        let local = self
            .catalog
            .iter()
            .filter(|m| m.tier == ModelTier::Local)
            .map(CatalogModel::model_ref);
        if self.local_only {
            return local.collect();
        }
        let mut out: Vec<ModelRef> = self
            .order
            .iter()
            .filter(|p| **p != ProviderId::OpenRouter && !p.is_local())
            .filter_map(|p| {
                self.table
                    .providers
                    .get(p)
                    .and_then(|row| row.fast.first())
                    .map(|id| ModelRef::new(*p, id.clone()))
            })
            .collect();
        let via_openrouter = ModelRef::new(ProviderId::OpenRouter, OPENROUTER_FAST_FALLBACK);
        let openrouter_listed = self
            .catalog
            .iter()
            .any(|m| m.provider == ProviderId::OpenRouter);
        if !openrouter_listed || self.listed(&via_openrouter).is_some() {
            out.push(via_openrouter);
        }
        out.extend(local);
        let mut free: Vec<&CatalogModel> = self
            .catalog
            .iter()
            .filter(|m| {
                m.provider == ProviderId::OpenRouter
                    && m.tier == ModelTier::Free
                    && !m.stealth
                    && (self.needs.caller == Caller::Direct || m.tools == ToolSupport::Yes)
            })
            .collect();
        free.sort_by(|a, b| {
            b.context_length
                .unwrap_or(0)
                .cmp(&a.context_length.unwrap_or(0))
                .then_with(|| a.id.cmp(&b.id))
        });
        out.extend(free.into_iter().map(CatalogModel::model_ref));
        out
    }
}

/// The free OpenRouter models among the automatic candidates of `inputs`: the ids
/// whose live availability is worth fetching (in the background).
pub fn availability_ids(inputs: &ResolveInputs<'_>) -> Vec<String> {
    if inputs.local_only {
        return Vec::new();
    }
    let mut ids: Vec<String> = Vec::new();
    let models = std::iter::once(inputs.requested.clone())
        .chain(inputs.chosen_fallback.cloned())
        .chain(inputs.pick_chain.iter().cloned())
        .chain(inputs.automatic());
    for model in models {
        if model.provider == ProviderId::OpenRouter
            && is_free_id(&model)
            && !ids.contains(&model.model)
        {
            ids.push(model.model);
        }
    }
    ids.truncate(MAX_AVAILABILITY_IDS);
    ids
}

/// The plan for one call. The requested model is always first (its health decides at
/// call time whether it is skipped); the fallbacks are the fitting candidates, the
/// user's own (chosen fallback, then pick) before automatic ones, which are ranked by
/// health, uptime and catalog order and capped at [`MAX_AUTOMATIC`].
pub fn resolve(
    inputs: &ResolveInputs<'_>,
    health: &HealthBook,
    availability: &AvailabilityBook,
    now_ms: u64,
) -> Plan {
    let requested = inputs.candidate(
        inputs.requested,
        Origin::Requested,
        health,
        availability,
        now_ms,
    );
    let floor = requested.privacy;
    let mut seen: Vec<ModelRef> = vec![inputs.requested.clone()];
    let mut named: Vec<Candidate> = Vec::new();
    let user_named = inputs
        .chosen_fallback
        .map(|m| (m.clone(), Origin::ChosenFallback))
        .into_iter()
        .chain(
            inputs
                .pick_chain
                .iter()
                .map(|m| (m.clone(), Origin::PickChain)),
        );
    for (model, origin) in user_named {
        if seen.contains(&model) || !inputs.reachable(&model) {
            continue;
        }
        seen.push(model.clone());
        let candidate = inputs.candidate(&model, origin, health, availability, now_ms);
        if inputs.fits(&candidate, floor) {
            named.push(candidate);
        }
    }
    let mut automatic: Vec<(usize, Candidate)> = Vec::new();
    for model in inputs.automatic() {
        if seen.contains(&model) || !inputs.reachable(&model) {
            continue;
        }
        seen.push(model.clone());
        let candidate = inputs.candidate(&model, Origin::Automatic, health, availability, now_ms);
        if inputs.fits(&candidate, floor) {
            automatic.push((automatic.len(), candidate));
        }
    }
    automatic.sort_by_key(|(order, c)| {
        (
            c.health.rank(),
            c.availability.map_or(1, |a| a.rank()),
            *order,
        )
    });
    named.extend(
        automatic
            .into_iter()
            .take(MAX_AUTOMATIC)
            .map(|(_, candidate)| candidate),
    );
    Plan {
        requested,
        fallbacks: named,
    }
}
