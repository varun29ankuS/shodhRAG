//! Configure the active LLM from the environment at startup.
//!
//! Opt-in: nothing is configured unless `SHODH_LLM_PROVIDER` is set, so the app
//! never starts sending questions to a cloud provider by surprise. This also lets
//! IT pre-configure an approved provider and model for managed installs.
//!
//! - `SHODH_LLM_PROVIDER`: openrouter | anthropic | openai | google | grok | ollama
//! - `SHODH_LLM_MODEL`: provider model id (optional; a sensible default is used)
//! - API key: the provider's conventional variable, e.g. `OPENROUTER_API_KEY`

use shodh_rag::harness::model_catalog::ModelRef;
use shodh_rag::llm::{ApiProvider, LLMManager, LLMMode};

use crate::llm_commands::LLMState;

pub const PROVIDER_VAR: &str = "SHODH_LLM_PROVIDER";
pub const MODEL_VAR: &str = "SHODH_LLM_MODEL";

#[derive(Debug, Clone)]
struct ProviderSpec {
    provider: ApiProvider,
    label: &'static str,
    key_vars: &'static [&'static str],
    default_model: &'static str,
}

fn spec_for(name: &str) -> Option<ProviderSpec> {
    let spec = match name.trim().to_ascii_lowercase().as_str() {
        "openrouter" => ProviderSpec {
            provider: ApiProvider::OpenRouter,
            label: "OpenRouter",
            key_vars: &["OPENROUTER_API_KEY"],
            default_model: "anthropic/claude-haiku-4.5",
        },
        "anthropic" => ProviderSpec {
            provider: ApiProvider::Anthropic,
            label: "Anthropic",
            key_vars: &["ANTHROPIC_API_KEY"],
            default_model: "claude-haiku-4-5",
        },
        "openai" => ProviderSpec {
            provider: ApiProvider::OpenAI,
            label: "OpenAI",
            key_vars: &["OPENAI_API_KEY"],
            default_model: "gpt-5-mini",
        },
        "google" | "gemini" => ProviderSpec {
            provider: ApiProvider::Google,
            label: "Google",
            key_vars: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            default_model: "gemini-2.5-flash",
        },
        "grok" | "xai" => ProviderSpec {
            provider: ApiProvider::Grok,
            label: "xAI",
            key_vars: &["XAI_API_KEY"],
            default_model: "grok-4",
        },
        "ollama" => ProviderSpec {
            provider: ApiProvider::Ollama,
            label: "Ollama",
            key_vars: &[],
            default_model: "qwen3:4b",
        },
        _ => return None,
    };
    Some(spec)
}

/// Resolved settings, separated from activation so the environment parsing is testable.
#[derive(Debug, Clone)]
struct EnvSelection {
    spec: ProviderSpec,
    api_key: String,
    model: String,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn resolve(get: impl Fn(&str) -> Option<String>) -> Result<Option<EnvSelection>, String> {
    let Some(name) = non_empty(get(PROVIDER_VAR)) else {
        return Ok(None);
    };
    let spec = spec_for(&name).ok_or_else(|| {
        format!("{PROVIDER_VAR}={name} is not supported (use openrouter, anthropic, openai, google, grok or ollama)")
    })?;
    let api_key = if spec.key_vars.is_empty() {
        "ollama".to_string()
    } else {
        spec.key_vars
            .iter()
            .find_map(|var| non_empty(get(var)))
            .ok_or_else(|| {
                format!(
                    "{PROVIDER_VAR}={name} is set but no API key was found in {}",
                    spec.key_vars.join(" or ")
                )
            })?
    };
    let model = non_empty(get(MODEL_VAR)).unwrap_or_else(|| spec.default_model.to_string());
    Ok(Some(EnvSelection {
        spec,
        api_key,
        model,
    }))
}

/// What the environment configured.
#[derive(Debug, Clone)]
pub struct EnvironmentModel {
    /// "provider · model", for the log.
    pub description: String,
    /// The model as the picker names it (`None` for an id the picker cannot hold).
    pub model: Option<ModelRef>,
}

/// Activate the provider named in the environment. `None` when opted out.
pub async fn configure_from_environment(
    state: &LLMState,
) -> Result<Option<EnvironmentModel>, String> {
    let Some(selection) = resolve(|var| std::env::var(var).ok())? else {
        return Ok(None);
    };

    {
        let mut keys = state.api_keys.lock().unwrap_or_else(|e| e.into_inner());
        let key = Some(selection.api_key.clone());
        match selection.spec.provider {
            ApiProvider::OpenRouter => keys.openrouter = key,
            ApiProvider::Anthropic => keys.anthropic = key,
            ApiProvider::OpenAI => keys.openai = key,
            ApiProvider::Google => keys.google = key,
            ApiProvider::Grok => keys.grok = key,
            _ => {}
        }
    }

    let config = {
        let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        config.mode = LLMMode::External {
            provider: selection.spec.provider.clone(),
            api_key: selection.api_key,
            model: selection.model.clone(),
        };
        config.clone()
    };

    let mut manager = LLMManager::new(config);
    manager.initialize().await.map_err(|e| {
        format!(
            "failed to initialize {} model {}: {e}",
            selection.spec.label, selection.model
        )
    })?;
    *state.manager.write().await = Some(manager);

    Ok(Some(EnvironmentModel {
        description: format!("{} · {}", selection.spec.label, selection.model),
        model: crate::model_picker_commands::model_ref_for(
            &selection.spec.provider,
            &selection.model,
        ),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn opted_out_when_provider_unset_or_blank() {
        assert!(resolve(env(&[("OPENROUTER_API_KEY", "k")]))
            .unwrap()
            .is_none());
        assert!(resolve(env(&[(PROVIDER_VAR, "  ")])).unwrap().is_none());
    }

    #[test]
    fn openrouter_with_model_override() {
        let sel = resolve(env(&[
            (PROVIDER_VAR, "OpenRouter"),
            ("OPENROUTER_API_KEY", " sk-or-test "),
            (MODEL_VAR, "stealth/space-bunny-alpha"),
        ]))
        .unwrap()
        .unwrap();
        assert!(matches!(sel.spec.provider, ApiProvider::OpenRouter));
        assert_eq!(sel.api_key, "sk-or-test");
        assert_eq!(sel.model, "stealth/space-bunny-alpha");
    }

    #[test]
    fn default_model_when_unset() {
        let sel = resolve(env(&[
            (PROVIDER_VAR, "openrouter"),
            ("OPENROUTER_API_KEY", "k"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(sel.model, "anthropic/claude-haiku-4.5");
    }

    #[test]
    fn missing_key_and_unknown_provider_are_errors() {
        let err = resolve(env(&[(PROVIDER_VAR, "openrouter")])).unwrap_err();
        assert!(err.contains("OPENROUTER_API_KEY"));
        assert!(resolve(env(&[(PROVIDER_VAR, "acme-ai")])).is_err());
    }

    #[test]
    fn google_accepts_either_key_variable_and_ollama_needs_none() {
        let sel = resolve(env(&[(PROVIDER_VAR, "gemini"), ("GOOGLE_API_KEY", "g")]))
            .unwrap()
            .unwrap();
        assert_eq!(sel.api_key, "g");
        assert!(resolve(env(&[(PROVIDER_VAR, "ollama")])).unwrap().is_some());
    }
}
