//! Reasoning tokens and a fill's reply budget.
//!
//! On OpenRouter, reasoning tokens are output tokens and on most providers they count
//! against the request's `max_tokens`. A reasoning model asked for a table reply within
//! [`super::prompt::reply_budget`] can therefore spend the budget thinking and send back a
//! cut or empty reply, which costs the paper its one retry. What a model does is read only
//! from the metadata OpenRouter publishes for it (`GET /api/v1/models`, a public list; no
//! key or user data is sent), never guessed from its name:
//!
//! - `reasoning` absent and no `reasoning` request parameter: the model does not reason.
//!   The budget is used as is ([`ReasoningRule::Plain`]).
//! - `reasoning.mandatory: false`: reasoning can be turned off, and it is, with
//!   `{"effort": "none"}` when the model lists `none` among its efforts and
//!   `{"enabled": false}` otherwise. The budget is used as is ([`ReasoningRule::Off`]).
//!   (`exclude: true` is not used: it only hides the reasoning, which is still produced
//!   and still counts.)
//! - `reasoning.mandatory: true` with `supports_max_tokens`: the reasoning is capped with
//!   `{"max_tokens": REASONING_ALLOWANCE_TOKENS}` and the request's `max_tokens` is the
//!   reply budget plus that allowance (OpenRouter requires `max_tokens` strictly above
//!   the reasoning budget).
//! - `reasoning.mandatory: true` with `minimal` or `low` among its efforts (or every
//!   effort accepted): the lowest of those is asked for and the budget is raised by the
//!   allowance ([`ReasoningRule::Bounded`]).
//! - `reasoning` reported only as a request parameter, without the metadata object: the
//!   model may reason and nothing tells how to bound it, so nothing is sent and the budget
//!   is raised by the allowance.
//! - Any other mandatory reasoning (no token budget, and only medium efforts or more, or
//!   no effort selection at all): the reasoning cannot be bounded, and the model is not
//!   offered for tables ([`ReasoningRule::Unbounded`]).
//!
//! The rule is only for OpenRouter models; other providers' requests carry no reasoning
//! parameter and keep the plain budget. If the list cannot be fetched, the plain budget is
//! used (a cut reply is then handled as cut by the fill) and the failure is logged once.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Output tokens allowed for reasoning on top of a reply's budget, for a model whose
/// reasoning cannot be turned off but can be held low. A fill prompt holds about 6,000
/// tokens of passages; a low-effort pass over it fits in this, and a reply budget is at
/// most [`super::prompt::REPLY_TOKEN_CAP`], so a request asks for at most about 6,000
/// output tokens.
pub const REASONING_ALLOWANCE_TOKENS: usize = 4_096;
/// How long OpenRouter's model list is reused.
pub const MODEL_LIST_TTL: Duration = Duration::from_secs(3_600);

/// Efforts, lowest first, that keep a mandatory reasoning pass small.
const LOW_EFFORTS: [&str; 2] = ["minimal", "low"];

/// The efforts a model accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Efforts {
    /// `supported_efforts` omitted: the model does not expose effort selection.
    NotSelectable,
    /// `supported_efforts: null`: every gateway effort is accepted.
    Any,
    /// The efforts listed, highest first.
    Listed(Vec<String>),
}

/// What OpenRouter says about a model's reasoning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelReasoning {
    /// No `reasoning` object and no `reasoning` request parameter.
    None,
    /// The `reasoning` request parameter is accepted, with no `reasoning` object.
    Undescribed,
    /// The `reasoning` object.
    Described {
        mandatory: bool,
        supports_max_tokens: bool,
        efforts: Efforts,
    },
}

/// What a table fill sends a model about reasoning, and its reply budget.
#[derive(Debug, Clone, PartialEq)]
pub enum ReasoningRule {
    /// No reasoning: nothing is sent; the reply budget as is.
    Plain,
    /// Reasoning turned off with `request`; the reply budget as is.
    Off { request: Value },
    /// Reasoning held low with `request` (none for a model that says nothing of how), and
    /// the reply budget raised by `allowance`.
    Bounded {
        request: Option<Value>,
        allowance: usize,
    },
    /// The model always reasons and nothing bounds it: not offered for tables.
    Unbounded,
}

impl ReasoningRule {
    /// The request's `max_tokens` for a reply of `reply_budget` tokens.
    pub fn max_tokens(&self, reply_budget: usize) -> usize {
        match self {
            Self::Bounded { allowance, .. } => reply_budget.saturating_add(*allowance),
            Self::Plain | Self::Off { .. } | Self::Unbounded => reply_budget,
        }
    }

    /// The request's `reasoning` object, when one is sent.
    pub fn request(&self) -> Option<&Value> {
        match self {
            Self::Off { request } => Some(request),
            Self::Bounded { request, .. } => request.as_ref(),
            Self::Plain | Self::Unbounded => None,
        }
    }

    /// Whether the model may fill tables.
    pub fn usable(&self) -> bool {
        !matches!(self, Self::Unbounded)
    }

    /// The rule in a few words (for logs and capture files).
    pub fn describe(&self) -> String {
        match self {
            Self::Plain => "no reasoning; plain reply budget".to_string(),
            Self::Off { request } => format!("reasoning off ({request}); plain reply budget"),
            Self::Bounded {
                request: Some(request),
                allowance,
            } => format!("reasoning held low ({request}); reply budget + {allowance}"),
            Self::Bounded {
                request: None,
                allowance,
            } => format!("reasoning not described; reply budget + {allowance}"),
            Self::Unbounded => "reasoning cannot be bounded; not offered for tables".to_string(),
        }
    }
}

/// Why a model is not offered for tables.
pub fn unbounded_message(model: &str) -> String {
    format!(
        "{model} always reasons before it answers, and OpenRouter offers no way to keep that \
         reasoning short, so it would spend a table cell's reply budget thinking. Choose \
         another model for tables (Settings → Model)."
    )
}

/// What a `/models` entry says about the model's reasoning.
pub fn model_reasoning(entry: &Value) -> ModelReasoning {
    let Some(reasoning) = entry.get("reasoning").filter(|r| r.is_object()) else {
        let accepts = entry
            .get("supported_parameters")
            .and_then(Value::as_array)
            .is_some_and(|params| params.iter().any(|p| p.as_str() == Some("reasoning")));
        return if accepts {
            ModelReasoning::Undescribed
        } else {
            ModelReasoning::None
        };
    };
    let efforts = match reasoning.get("supported_efforts") {
        None => Efforts::NotSelectable,
        Some(Value::Null) => Efforts::Any,
        Some(Value::Array(list)) => Efforts::Listed(
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        Some(_) => Efforts::NotSelectable,
    };
    ModelReasoning::Described {
        mandatory: reasoning
            .get("mandatory")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        supports_max_tokens: reasoning
            .get("supports_max_tokens")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        efforts,
    }
}

/// The rule for a model (see the module documentation).
pub fn rule_for(model: &ModelReasoning) -> ReasoningRule {
    match model {
        ModelReasoning::None => ReasoningRule::Plain,
        ModelReasoning::Undescribed => ReasoningRule::Bounded {
            request: None,
            allowance: REASONING_ALLOWANCE_TOKENS,
        },
        ModelReasoning::Described {
            mandatory: false,
            efforts,
            ..
        } => {
            let none_listed =
                matches!(efforts, Efforts::Listed(list) if list.iter().any(|e| e == "none"));
            ReasoningRule::Off {
                request: if none_listed {
                    json!({ "effort": "none" })
                } else {
                    json!({ "enabled": false })
                },
            }
        }
        ModelReasoning::Described {
            mandatory: true,
            supports_max_tokens: true,
            ..
        } => ReasoningRule::Bounded {
            request: Some(json!({ "max_tokens": REASONING_ALLOWANCE_TOKENS })),
            allowance: REASONING_ALLOWANCE_TOKENS,
        },
        ModelReasoning::Described {
            mandatory: true,
            efforts,
            ..
        } => {
            let low = match efforts {
                Efforts::Any => Some(LOW_EFFORTS[0]),
                Efforts::Listed(list) => LOW_EFFORTS
                    .iter()
                    .copied()
                    .find(|low| list.iter().any(|e| e == low)),
                Efforts::NotSelectable => None,
            };
            match low {
                Some(effort) => ReasoningRule::Bounded {
                    request: Some(json!({ "effort": effort })),
                    allowance: REASONING_ALLOWANCE_TOKENS,
                },
                None => ReasoningRule::Unbounded,
            }
        }
    }
}

/// Each model's reasoning, by id, from a `/models` response.
pub fn parse_models(body: &str) -> Result<HashMap<String, ModelReasoning>, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "the model list has no data".to_string())?;
    Ok(data
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            Some((id.to_string(), model_reasoning(entry)))
        })
        .collect())
}

type Listed = (Instant, Arc<HashMap<String, ModelReasoning>>);

/// OpenRouter's reasoning metadata, fetched when needed and kept for [`MODEL_LIST_TTL`].
#[derive(Debug, Default)]
pub struct ReasoningCatalog {
    list: Mutex<Option<Listed>>,
    warned: AtomicBool,
}

impl ReasoningCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rule for the OpenRouter model `model`, fetching the list when it is missing or
    /// old.
    pub async fn rule(&self, model: &str) -> ReasoningRule {
        self.rule_with(model, || async {
            crate::harness::catalog_fetch::fetch_openrouter_body()
                .await
                .map_err(|e| e.to_string())
        })
        .await
    }

    /// [`Self::rule`] with the list fetched by `fetch`. A list that cannot be fetched or
    /// read leaves the last one in use, and with none the plain budget; the first such
    /// failure is logged at warn, later ones at debug. A model the list does not hold gets
    /// the plain budget.
    pub async fn rule_with<F, Fut>(&self, model: &str, fetch: F) -> ReasoningRule
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        let cached = self.list.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let fresh = cached
            .as_ref()
            .filter(|(at, _)| at.elapsed() < MODEL_LIST_TTL)
            .map(|(_, list)| list.clone());
        let list = match fresh {
            Some(list) => Some(list),
            None => match fetch().await.and_then(|body| parse_models(&body)) {
                Ok(parsed) => {
                    let parsed = Arc::new(parsed);
                    *self.list.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((Instant::now(), parsed.clone()));
                    Some(parsed)
                }
                Err(e) => {
                    if self.warned.swap(true, Ordering::SeqCst) {
                        tracing::debug!(error = %e, "OpenRouter's model list is still unavailable");
                    } else {
                        tracing::warn!(
                            error = %e,
                            "OpenRouter's model list could not be read; evidence table fills use the plain reply budget"
                        );
                    }
                    cached.map(|(_, list)| list)
                }
            },
        };
        match list.as_ref().and_then(|list| list.get(model)) {
            Some(reasoning) => rule_for(reasoning),
            None => ReasoningRule::Plain,
        }
    }

    /// Whether a failure to read the list was logged.
    pub fn warned(&self) -> bool {
        self.warned.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entries of OpenRouter's `/models` list as served on 2026-10-22 (id, name, supported
    /// parameters and the `reasoning` object, unchanged).
    const RECORDED: &str = include_str!("fixtures/openrouter_models_2026-10-22.json");

    fn not_fetched() -> Result<String, String> {
        panic!("a fresh list is not fetched again")
    }

    fn recorded(id: &str) -> ReasoningRule {
        let models = parse_models(RECORDED).unwrap();
        rule_for(models.get(id).unwrap_or_else(|| panic!("{id} is recorded")))
    }

    #[test]
    fn a_model_that_does_not_reason_keeps_the_plain_budget() {
        let rule = recorded("unbiased/pareto-26.10");
        assert_eq!(rule, ReasoningRule::Plain);
        assert_eq!(rule.max_tokens(840), 840);
        assert!(rule.request().is_none());
    }

    #[test]
    fn optional_reasoning_is_turned_off_the_way_the_model_accepts() {
        // Lists `none` among its efforts.
        let inkling = recorded("thinkingmachines/inkling:free");
        assert_eq!(inkling.request(), Some(&json!({ "effort": "none" })));
        assert_eq!(inkling.max_tokens(840), 840);
        let nemotron = recorded("nvidia/nemotron-3-super-120b-a12b:free");
        assert_eq!(nemotron.request(), Some(&json!({ "effort": "none" })));
        // No effort selection: the gateway's switch.
        let gemma = recorded("google/gemma-4-31b-it:free");
        assert_eq!(gemma.request(), Some(&json!({ "enabled": false })));
        assert_eq!(gemma.max_tokens(840), 840);
        assert!(gemma.usable());
    }

    #[test]
    fn mandatory_reasoning_is_held_low_and_paid_for_in_the_budget() {
        let minimax = recorded("minimax/minimax-m3.1-flash");
        assert_eq!(minimax.request(), Some(&json!({ "effort": "low" })));
        assert_eq!(minimax.max_tokens(840), 840 + REASONING_ALLOWANCE_TOKENS);
        let muse = recorded("meta/muse-wonder");
        assert_eq!(
            muse.request(),
            Some(&json!({ "effort": "minimal" })),
            "the lowest effort listed"
        );
        // A model that takes a reasoning budget (none is listed today; the shape is
        // OpenRouter's documented `supports_max_tokens`).
        let budgeted = model_reasoning(&json!({
            "id": "x/y",
            "reasoning": { "mandatory": true, "supports_max_tokens": true }
        }));
        let rule = rule_for(&budgeted);
        assert_eq!(
            rule.request(),
            Some(&json!({ "max_tokens": REASONING_ALLOWANCE_TOKENS }))
        );
        assert!(
            rule.max_tokens(840) > REASONING_ALLOWANCE_TOKENS,
            "max_tokens stays above the reasoning budget"
        );
    }

    #[test]
    fn reasoning_that_cannot_be_bounded_is_not_offered() {
        for id in ["liquid/lfm-2.5-2.6b:free", "sakana/fugu-max"] {
            let rule = recorded(id);
            assert_eq!(rule, ReasoningRule::Unbounded, "{id}");
            assert!(!rule.usable());
        }
        assert!(unbounded_message("liquid/lfm-2.5-2.6b:free").contains("Choose another model"));
    }

    #[test]
    fn a_model_that_only_accepts_the_parameter_gets_the_allowance() {
        let rule = recorded("openrouter/free");
        assert_eq!(
            rule,
            ReasoningRule::Bounded {
                request: None,
                allowance: REASONING_ALLOWANCE_TOKENS
            }
        );
        assert!(rule.request().is_none());
        assert_eq!(rule.max_tokens(840), 840 + REASONING_ALLOWANCE_TOKENS);
        // Every effort accepted: the lowest gateway effort.
        let any = model_reasoning(&json!({
            "id": "x/any",
            "reasoning": { "mandatory": true, "supported_efforts": null }
        }));
        assert_eq!(
            rule_for(&any).request(),
            Some(&json!({ "effort": "minimal" }))
        );
    }

    #[tokio::test]
    async fn the_list_is_fetched_once_and_an_unreachable_list_means_the_plain_budget() {
        let catalog = ReasoningCatalog::new();
        let down = catalog
            .rule_with("minimax/minimax-m3.1-flash", || async {
                Err::<String, String>("connection refused".to_string())
            })
            .await;
        assert_eq!(down, ReasoningRule::Plain);
        assert!(catalog.warned());
        let fetched = catalog
            .rule_with("minimax/minimax-m3.1-flash", || async {
                Ok::<String, String>(RECORDED.to_string())
            })
            .await;
        assert!(matches!(fetched, ReasoningRule::Bounded { .. }));
        // Kept: a later failure is never asked, and an unknown id is plain.
        let cached = catalog
            .rule_with("liquid/lfm-2.5-2.6b:free", || async { not_fetched() })
            .await;
        assert_eq!(cached, ReasoningRule::Unbounded);
        let unknown = catalog
            .rule_with("nobody/unknown-model", || async { not_fetched() })
            .await;
        assert_eq!(unknown, ReasoningRule::Plain);
        assert!(parse_models("not json").is_err());
        assert!(parse_models("{}").is_err());
    }
}
