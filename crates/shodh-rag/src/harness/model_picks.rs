//! The four model picks of Settings → Model (Best quality, Fast & cheap,
//! Free, Private) and the curated table they resolve from
//! (`model_picks.json`, one data file that is easy to update).
//!
//! A pick resolves to an ordered chain of models across the connected
//! providers (in the fallback order): the first is the model that answers,
//! the rest are the automatic fallbacks, tried in turn when a model is
//! rate-limited or unavailable.
//!
//! Pure: no I/O.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::model::is_valid_model_id;
use super::model_catalog::{
    CatalogError, CatalogModel, ModelRef, ModelTier, ProviderId, ToolSupport,
};

/// One of the four picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    /// The strongest model of each connected provider.
    Best,
    /// A fast, inexpensive model of each connected provider.
    Fast,
    /// Free models (only providers that offer them: OpenRouter `:free`).
    Free,
    /// Models on this computer (LM Studio).
    Private,
}

impl Pick {
    pub const ALL: [Pick; 4] = [Pick::Best, Pick::Fast, Pick::Free, Pick::Private];

    pub fn label(self) -> &'static str {
        match self {
            Pick::Best => "Best quality",
            Pick::Fast => "Fast & cheap",
            Pick::Free => "Free",
            Pick::Private => "Private (local)",
        }
    }
}

/// A known model of one provider.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KnownModel {
    pub id: String,
    pub name: String,
    /// The same model's OpenRouter id (for its price and context length).
    #[serde(default)]
    pub openrouter: Option<String>,
}

/// One provider's row in the table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ProviderPicks {
    pub best: Vec<String>,
    pub fast: Vec<String>,
    pub free: Vec<String>,
    /// The models listed for the provider ("Show all models").
    pub models: Vec<KnownModel>,
}

impl ProviderPicks {
    fn candidates(&self, pick: Pick) -> &[String] {
        match pick {
            Pick::Best => &self.best,
            Pick::Fast => &self.fast,
            Pick::Free => &self.free,
            Pick::Private => &[],
        }
    }
}

/// The curated table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickTable {
    /// Default fallback order of the providers.
    pub order: Vec<ProviderId>,
    pub providers: BTreeMap<ProviderId, ProviderPicks>,
}

#[derive(Deserialize)]
struct RawTable {
    order: Vec<String>,
    providers: BTreeMap<String, ProviderPicks>,
}

/// Parse and check a table: known providers only, safe model ids, every
/// pick candidate listed under the provider's models (except OpenRouter,
/// whose models come from its live list).
pub fn parse_table(json: &str) -> Result<PickTable, CatalogError> {
    let raw: RawTable =
        serde_json::from_str(json).map_err(|e| CatalogError::Parse(e.to_string()))?;
    let provider = |id: &str| {
        ProviderId::parse(id)
            .filter(|p| p.as_str() == id)
            .ok_or_else(|| CatalogError::Parse(format!("unknown provider {id:?}")))
    };
    let mut order = Vec::new();
    for id in &raw.order {
        let p = provider(id)?;
        if order.contains(&p) {
            return Err(CatalogError::Parse(format!(
                "{id:?} is listed twice in order"
            )));
        }
        order.push(p);
    }
    let mut providers = BTreeMap::new();
    for (id, picks) in raw.providers {
        let p = provider(&id)?;
        if !order.contains(&p) {
            return Err(CatalogError::Parse(format!("{id:?} is missing from order")));
        }
        let ids = picks
            .best
            .iter()
            .chain(&picks.fast)
            .chain(&picks.free)
            .chain(picks.models.iter().map(|m| &m.id))
            .chain(picks.models.iter().filter_map(|m| m.openrouter.as_ref()));
        for model in ids {
            if !is_valid_model_id(model) {
                return Err(CatalogError::InvalidModel(model.clone()));
            }
        }
        if p != ProviderId::OpenRouter {
            for model in picks.best.iter().chain(&picks.fast) {
                if !picks.models.iter().any(|m| &m.id == model) {
                    return Err(CatalogError::Parse(format!(
                        "{id}: pick {model:?} is not in its models"
                    )));
                }
            }
            if !picks.free.is_empty() {
                return Err(CatalogError::Parse(format!("{id} offers no free models")));
            }
        }
        providers.insert(p, picks);
    }
    Ok(PickTable { order, providers })
}

/// The table shipped with the app (`model_picks.json`).
pub fn table() -> &'static PickTable {
    static TABLE: OnceLock<PickTable> = OnceLock::new();
    TABLE.get_or_init(|| match parse_table(include_str!("model_picks.json")) {
        Ok(table) => table,
        Err(e) => {
            // Checked by the tests; an empty table only hides the picks.
            tracing::error!("model_picks.json is invalid: {e}");
            PickTable::default()
        }
    })
}

/// The fallback order: the person's saved order first, then every other
/// provider in the table's default order.
pub fn provider_order(saved: &[ProviderId], defaults: &[ProviderId]) -> Vec<ProviderId> {
    let mut out: Vec<ProviderId> = Vec::new();
    for p in saved.iter().chain(defaults).chain(ProviderId::ALL.iter()) {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out
}

/// What a pick resolves from.
#[derive(Debug, Clone)]
pub struct PickContext<'a> {
    pub table: &'a PickTable,
    /// Providers that can answer now (a key, a signed-in subscription, LM Studio running).
    pub connected: &'a [ProviderId],
    /// The fallback order ([`provider_order`]).
    pub order: &'a [ProviderId],
    /// The model list ([`super::model_catalog::build_catalog`]).
    pub catalog: &'a [CatalogModel],
    pub local_only: bool,
}

impl PickContext<'_> {
    fn connected_in_order(&self) -> impl Iterator<Item = ProviderId> + '_ {
        self.order
            .iter()
            .copied()
            .filter(|p| self.connected.contains(p) && p.works_with_agent())
    }

    fn listed(&self, provider: ProviderId, id: &str) -> bool {
        self.catalog
            .iter()
            .any(|m| m.provider == provider && m.id == id)
    }

    /// OpenRouter models are checked against its live list when one is loaded.
    fn offered(&self, provider: ProviderId, id: &str) -> bool {
        if provider != ProviderId::OpenRouter {
            return true;
        }
        let has_list = self
            .catalog
            .iter()
            .any(|m| m.provider == ProviderId::OpenRouter);
        !has_list || self.listed(provider, id)
    }
}

/// Longest chain a pick resolves to (the model and its fallbacks).
pub const MAX_CHAIN: usize = 6;

/// The models a pick resolves to, best first: one per connected provider
/// (in the fallback order) for Best and Fast, OpenRouter's free models for
/// Free, the models loaded in LM Studio for Private. Empty when no connected
/// provider offers the pick.
pub fn resolve_chain(pick: Pick, ctx: &PickContext<'_>) -> Vec<ModelRef> {
    let mut chain: Vec<ModelRef> = Vec::new();
    let push = |model: ModelRef, chain: &mut Vec<ModelRef>| {
        if chain.len() < MAX_CHAIN && !chain.contains(&model) {
            chain.push(model);
        }
    };
    match pick {
        Pick::Private => {
            for m in ctx.catalog.iter().filter(|m| {
                m.provider == ProviderId::LmStudio && ctx.connected.contains(&m.provider)
            }) {
                push(m.model_ref(), &mut chain);
            }
        }
        _ if ctx.local_only => {}
        Pick::Best | Pick::Fast => {
            for provider in ctx.connected_in_order().filter(|p| !p.is_local()) {
                let Some(row) = ctx.table.providers.get(&provider) else {
                    continue;
                };
                if let Some(id) = row
                    .candidates(pick)
                    .iter()
                    .find(|id| ctx.offered(provider, id))
                {
                    push(ModelRef::new(provider, id.clone()), &mut chain);
                }
            }
        }
        Pick::Free => {
            for provider in ctx.connected_in_order().filter(|p| !p.is_local()) {
                let Some(row) = ctx.table.providers.get(&provider) else {
                    continue;
                };
                if row.free.is_empty() {
                    continue;
                }
                for id in row.free.iter().filter(|id| ctx.offered(provider, id)) {
                    push(ModelRef::new(provider, id.clone()), &mut chain);
                }
                // Other free models with tool calling, largest context first (never stealth).
                let mut more: Vec<&CatalogModel> = ctx
                    .catalog
                    .iter()
                    .filter(|m| {
                        m.provider == provider
                            && m.tier == ModelTier::Free
                            && m.tools == ToolSupport::Yes
                            && !m.stealth
                    })
                    .collect();
                more.sort_by(|a, b| {
                    b.context_length
                        .unwrap_or(0)
                        .cmp(&a.context_length.unwrap_or(0))
                        .then_with(|| a.id.cmp(&b.id))
                });
                for m in more {
                    push(m.model_ref(), &mut chain);
                }
            }
        }
    }
    chain
}

/// The model after `failed` in `chain` (or the chain's first model when
/// `failed` is not in it). `None` at the end of the chain.
pub fn next_in_chain(chain: &[ModelRef], failed: &ModelRef) -> Option<ModelRef> {
    match chain.iter().position(|m| m == failed) {
        Some(i) => chain.get(i + 1).cloned(),
        None => chain.first().cloned(),
    }
}

/// The pick a model belongs to, if any: the first pick whose chain contains it.
pub fn pick_of(model: &ModelRef, ctx: &PickContext<'_>) -> Option<Pick> {
    Pick::ALL
        .into_iter()
        .find(|p| resolve_chain(*p, ctx).contains(model))
}

/// Display name of a model: the catalog's, else the table's, else its id.
pub fn model_name(model: &ModelRef, catalog: &[CatalogModel], table: &PickTable) -> String {
    catalog
        .iter()
        .find(|m| m.provider == model.provider && m.id == model.model)
        .map(|m| m.name.clone())
        .or_else(|| {
            table
                .providers
                .get(&model.provider)
                .and_then(|row| row.models.iter().find(|m| m.id == model.model))
                .map(|m| m.name.clone())
        })
        .unwrap_or_else(|| model.model.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::model_catalog::{parse_openrouter, PrivacyNote};

    fn shipped() -> PickTable {
        parse_table(include_str!("model_picks.json")).expect("model_picks.json is valid")
    }

    fn lm(id: &str) -> CatalogModel {
        CatalogModel {
            provider: ProviderId::LmStudio,
            id: id.into(),
            name: id.into(),
            context_length: None,
            prompt_per_million: Some(0.0),
            completion_per_million: Some(0.0),
            tools: ToolSupport::Unknown,
            tier: ModelTier::Local,
            privacy: PrivacyNote::OnDevice,
            stealth: false,
        }
    }

    const OPENROUTER: &str = r#"{"data":[
        {"id":"anthropic/claude-opus-5.5","name":"Claude Opus 5.5","context_length":1000000,
         "pricing":{"prompt":"0.000004","completion":"0.00002"},"supported_parameters":["tools"]},
        {"id":"anthropic/claude-haiku-4.5","name":"Claude Haiku 4.5","context_length":200000,
         "pricing":{"prompt":"0.000001","completion":"0.000005"},"supported_parameters":["tools"]},
        {"id":"nvidia/nemotron-3-super-120b-a12b:free","name":"Nemotron 3 Super (free)","context_length":262144,
         "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"]},
        {"id":"qwen/qwen3-coder:free","name":"Qwen3 Coder (free)","context_length":262000,
         "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"]},
        {"id":"stealth/space-bunny-alpha","name":"Space Bunny","context_length":900000,
         "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"]},
        {"id":"z-ai/glm-4.5-air:free","name":"GLM","context_length":131072,
         "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["max_tokens"]}
    ]}"#;

    #[test]
    fn the_shipped_table_is_valid_and_covers_every_connectable_provider() {
        let table = shipped();
        for p in ProviderId::ALL {
            if p == ProviderId::Ollama {
                assert!(!table.providers.contains_key(&p), "Ollama is not offered");
                continue;
            }
            assert!(table.order.contains(&p), "{p:?} has a fallback position");
            assert!(table.providers.contains_key(&p), "{p:?} has a row");
            if !p.is_local() {
                let row = &table.providers[&p];
                assert!(
                    !row.best.is_empty() && !row.fast.is_empty(),
                    "{p:?} has Best and Fast"
                );
            }
        }
        let free: Vec<ProviderId> = table
            .providers
            .iter()
            .filter(|(_, r)| !r.free.is_empty())
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(
            free,
            vec![ProviderId::OpenRouter],
            "only OpenRouter offers free models"
        );
        assert!(table.providers[&ProviderId::OpenRouter]
            .free
            .iter()
            .all(|id| id.ends_with(":free")));
    }

    #[test]
    fn invalid_tables_are_refused() {
        assert!(parse_table(r#"{"order":["acme"],"providers":{}}"#).is_err());
        assert!(parse_table(r#"{"order":["openai","openai"],"providers":{}}"#).is_err());
        assert!(parse_table(r#"{"order":[],"providers":{"openai":{}}}"#).is_err());
        assert!(parse_table(
            r#"{"order":["openai"],"providers":{"openai":{"best":["gpt-x"],"models":[]}}}"#
        )
        .is_err());
        assert!(parse_table(
            r#"{"order":["openai"],"providers":{"openai":{"best":["--evil"],"models":[{"id":"--evil","name":"x"}]}}}"#
        )
        .is_err());
        assert!(
            parse_table(r#"{"order":["openai"],"providers":{"openai":{"free":["a:free"]}}}"#)
                .is_err()
        );
    }

    fn ctx<'a>(
        table: &'a PickTable,
        connected: &'a [ProviderId],
        order: &'a [ProviderId],
        catalog: &'a [CatalogModel],
    ) -> PickContext<'a> {
        PickContext {
            table,
            connected,
            order,
            catalog,
            local_only: false,
        }
    }

    #[test]
    fn picks_resolve_per_connected_set() {
        let table = shipped();
        let order = provider_order(&[], &table.order);
        let none: Vec<CatalogModel> = Vec::new();

        // Nothing connected: nothing to pick.
        for pick in Pick::ALL {
            assert!(resolve_chain(pick, &ctx(&table, &[], &order, &none)).is_empty());
        }

        // A Claude subscription and an OpenAI key: one model each, subscription first.
        let connected = [ProviderId::OpenAI, ProviderId::ClaudeSub];
        let best = resolve_chain(Pick::Best, &ctx(&table, &connected, &order, &none));
        assert_eq!(
            best,
            vec![
                ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5"),
                ModelRef::new(ProviderId::OpenAI, "gpt-5.5"),
            ]
        );
        let fast = resolve_chain(Pick::Fast, &ctx(&table, &connected, &order, &none));
        assert_eq!(
            fast[0],
            ModelRef::new(ProviderId::ClaudeSub, "claude-haiku-4-5")
        );
        assert_eq!(fast[1], ModelRef::new(ProviderId::OpenAI, "gpt-5.4-mini"));
        assert!(
            resolve_chain(Pick::Free, &ctx(&table, &connected, &order, &none)).is_empty(),
            "neither offers free models"
        );
        assert!(resolve_chain(Pick::Private, &ctx(&table, &connected, &order, &none)).is_empty());

        // OpenRouter: free models from the table that are listed, then other
        // free tool models by context (never stealth, never without tools).
        let list = parse_openrouter(OPENROUTER).unwrap();
        let or = [ProviderId::OpenRouter];
        let free = resolve_chain(Pick::Free, &ctx(&table, &or, &order, &list));
        assert_eq!(
            free,
            vec![
                ModelRef::new(
                    ProviderId::OpenRouter,
                    "nvidia/nemotron-3-super-120b-a12b:free"
                ),
                ModelRef::new(ProviderId::OpenRouter, "qwen/qwen3-coder:free"),
            ]
        );
        let best = resolve_chain(Pick::Best, &ctx(&table, &or, &order, &list));
        assert_eq!(
            best,
            vec![ModelRef::new(
                ProviderId::OpenRouter,
                "anthropic/claude-opus-5.5"
            )]
        );
        // A table candidate missing from the live list is skipped.
        let without_opus: Vec<CatalogModel> = list
            .iter()
            .filter(|m| m.id != "anthropic/claude-opus-5.5")
            .cloned()
            .collect();
        let best = resolve_chain(Pick::Best, &ctx(&table, &or, &order, &without_opus));
        assert!(best.is_empty(), "openai/gpt-5.5 is not listed either");

        // Private: LM Studio's loaded models, only while it runs (is connected).
        let local = vec![lm("qwen3-8b"), lm("gemma-3-4b")];
        let private = resolve_chain(
            Pick::Private,
            &ctx(&table, &[ProviderId::LmStudio], &order, &local),
        );
        assert_eq!(private.len(), 2);
        assert_eq!(private[0], ModelRef::new(ProviderId::LmStudio, "qwen3-8b"));
        assert!(resolve_chain(Pick::Private, &ctx(&table, &[], &order, &local)).is_empty());

        // Local-only mode: only Private resolves.
        let mut lo = ctx(
            &table,
            &[ProviderId::OpenAI, ProviderId::LmStudio],
            &order,
            &local,
        );
        lo.local_only = true;
        assert!(resolve_chain(Pick::Best, &lo).is_empty());
        assert_eq!(resolve_chain(Pick::Private, &lo).len(), 2);
    }

    #[test]
    fn fallback_follows_the_saved_order_across_providers() {
        let table = shipped();
        let none: Vec<CatalogModel> = Vec::new();
        let connected = [
            ProviderId::ClaudeSub,
            ProviderId::OpenAI,
            ProviderId::Google,
        ];
        let order = provider_order(&[ProviderId::Google, ProviderId::OpenAI], &table.order);
        assert_eq!(
            &order[..3],
            &[
                ProviderId::Google,
                ProviderId::OpenAI,
                ProviderId::ClaudeSub
            ]
        );
        assert_eq!(
            order.len(),
            ProviderId::ALL.len(),
            "every provider has a position"
        );
        let chain = resolve_chain(Pick::Best, &ctx(&table, &connected, &order, &none));
        let providers: Vec<ProviderId> = chain.iter().map(|m| m.provider).collect();
        assert_eq!(
            providers,
            vec![
                ProviderId::Google,
                ProviderId::OpenAI,
                ProviderId::ClaudeSub
            ]
        );

        assert_eq!(next_in_chain(&chain, &chain[0]), Some(chain[1].clone()));
        assert_eq!(next_in_chain(&chain, &chain[2]), None, "end of the list");
        let elsewhere = ModelRef::new(ProviderId::OpenRouter, "a/b");
        assert_eq!(next_in_chain(&chain, &elsewhere), Some(chain[0].clone()));
        assert_eq!(
            pick_of(&chain[1], &ctx(&table, &connected, &order, &none)),
            Some(Pick::Best)
        );
        assert_eq!(
            pick_of(&elsewhere, &ctx(&table, &connected, &order, &none)),
            None
        );
    }

    #[test]
    fn ollama_never_resolves_even_when_connected() {
        let table = shipped();
        let order = provider_order(&[ProviderId::Ollama], &table.order);
        let chain = resolve_chain(
            Pick::Private,
            &ctx(&table, &[ProviderId::Ollama], &order, &[]),
        );
        assert!(chain.is_empty());
    }

    #[test]
    fn names_come_from_the_catalog_then_the_table() {
        let table = shipped();
        let m = ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5");
        assert_eq!(model_name(&m, &[], &table), "Claude Opus 5.5");
        assert_eq!(
            model_name(&ModelRef::new(ProviderId::OpenAI, "custom-x"), &[], &table),
            "custom-x"
        );
    }
}
