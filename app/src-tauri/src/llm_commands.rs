//! Tauri commands for LLM integration

use serde::Serialize;
use shodh_rag::harness::model_catalog::ProviderId;
use shodh_rag::llm::{LLMConfig, LLMManager, LLMMode};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

use tokio::sync::RwLock as AsyncRwLock;

use crate::api_key_store;
use crate::audit_commands::AuditState;
use shodh_rag::audit::{payload as audit_payload, AuditEventType, AuditRecord};

/// LLM state managed by Tauri
pub struct LLMState {
    pub manager: Arc<AsyncRwLock<Option<LLMManager>>>,
    pub config: Arc<Mutex<LLMConfig>>,
    pub api_keys: Arc<Mutex<ApiKeys>>,
    /// GGUF file chosen for local inference (llama.cpp).
    pub custom_model_path: Arc<Mutex<Option<PathBuf>>>,
}

/// In-memory provider API keys. Persisted copies live only in the OS
/// credential store (see `api_key_store`); values are never serialized to the
/// frontend.
#[derive(Default, Clone)]
pub struct ApiKeys {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
    pub openrouter: Option<String>,
    pub kimi: Option<String>,
    pub grok: Option<String>,
    pub perplexity: Option<String>,
    pub google: Option<String>,
    pub baseten: Option<String>,
    /// Bumped every time a provider's key is set or removed in this run
    /// ([`ApiKeys::set`]), so work that read a key can tell it changed since.
    generations: KeyGenerations,
}

/// How many times each provider's key changed in this run (see
/// [`ApiKeys::generation`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyGenerations(std::collections::HashMap<String, u64>);

impl KeyGenerations {
    /// The generation of `provider`'s key (0 until it first changes).
    pub fn get(&self, provider: &str) -> u64 {
        self.0.get(provider).copied().unwrap_or(0)
    }

    fn bump(&mut self, provider: &str) {
        *self.0.entry(provider.to_string()).or_insert(0) += 1;
    }
}

impl ApiKeys {
    /// Mutable slot for a provider id, or `None` for an unknown provider.
    fn slot_mut(&mut self, provider: &str) -> Option<&mut Option<String>> {
        match provider {
            "openai" => Some(&mut self.openai),
            "anthropic" => Some(&mut self.anthropic),
            "openrouter" => Some(&mut self.openrouter),
            "kimi" => Some(&mut self.kimi),
            "grok" => Some(&mut self.grok),
            "perplexity" => Some(&mut self.perplexity),
            "google" => Some(&mut self.google),
            "baseten" => Some(&mut self.baseten),
            _ => None,
        }
    }

    /// Set (or clear) the key of a provider id, and bump its generation.
    /// Unknown ids are ignored.
    pub fn set(&mut self, provider: &str, key: Option<String>) {
        if let Some(slot) = self.slot_mut(provider) {
            *slot = key;
            self.generations.bump(provider);
        }
    }

    /// The generation of `provider`'s key: it changes whenever the key is
    /// set or removed, never otherwise.
    pub fn generation(&self, provider: &str) -> u64 {
        self.generations.get(provider)
    }

    /// Every provider's generation now, to compare against later.
    pub fn generations(&self) -> KeyGenerations {
        self.generations.clone()
    }

    /// The key of a provider id, if one is set. Never log the result.
    pub fn get(&self, provider: &str) -> Option<String> {
        let value = match provider {
            "openai" => &self.openai,
            "anthropic" => &self.anthropic,
            "openrouter" => &self.openrouter,
            "kimi" => &self.kimi,
            "grok" => &self.grok,
            "perplexity" => &self.perplexity,
            "google" => &self.google,
            "baseten" => &self.baseten,
            _ => return None,
        };
        value.clone()
    }

    fn is_configured(&self, provider: &str) -> bool {
        let value = match provider {
            "openai" => &self.openai,
            "anthropic" => &self.anthropic,
            "openrouter" => &self.openrouter,
            "kimi" => &self.kimi,
            "grok" => &self.grok,
            "perplexity" => &self.perplexity,
            "google" => &self.google,
            "baseten" => &self.baseten,
            _ => return false,
        };
        value.as_deref().is_some_and(|k| !k.trim().is_empty())
    }

    /// Provider ids that currently have a non-empty key.
    pub fn configured_providers(&self) -> Vec<String> {
        api_key_store::KEY_PROVIDERS
            .iter()
            .filter(|p| self.is_configured(p))
            .map(|p| (*p).to_string())
            .collect()
    }

    /// Fill providers that have no key yet with keys loaded from the OS
    /// credential store. Keys already in memory (set by the user or the
    /// environment in this session) are newer and are kept. `since` are the
    /// generations from before the store was read: a provider whose key was
    /// saved or removed while it was read is left alone, so a key removed
    /// during startup never comes back from the copy read before.
    pub fn merge_missing(
        &mut self,
        stored: Vec<(&'static str, String)>,
        since: &KeyGenerations,
    ) -> usize {
        let mut merged = 0;
        for (provider, key) in stored {
            if self.generations.get(provider) != since.get(provider) {
                continue;
            }
            if let Some(slot) = self.slot_mut(provider) {
                if slot.as_deref().is_none_or(|k| k.trim().is_empty()) {
                    *slot = Some(key);
                    merged += 1;
                }
            }
        }
        merged
    }
}

impl std::fmt::Debug for ApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeys")
            .field("configured", &self.configured_providers())
            .field("values", &"[REDACTED]")
            .finish()
    }
}

/// Make `llm_mode` the app's model: the config the agent sessions read when
/// they next start, and the in-process manager used by the other features.
pub(crate) async fn activate_mode(state: &LLMState, llm_mode: LLMMode) -> Result<String, String> {
    let config = {
        let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        config.mode = llm_mode.clone();
        config.clone()
    };

    // Switch mode or create new manager
    let mut manager_lock = state.manager.write().await;

    let result = if let Some(manager) = manager_lock.as_mut() {
        // Try to switch existing manager
        match manager.switch_mode(llm_mode).await {
            Ok(_) => {
                tracing::info!("Mode switched successfully on existing manager");
                Ok("Mode switched successfully".to_string())
            }
            Err(e) => {
                tracing::warn!("Failed to switch mode, creating new manager: {}", e);
                let mut new_manager = LLMManager::new(config);
                match new_manager.initialize().await {
                    Ok(_) => {
                        *manager_lock = Some(new_manager);
                        tracing::info!("Created new manager successfully");
                        Ok("LLM initialized successfully".to_string())
                    }
                    Err(init_err) => {
                        tracing::warn!("Failed to initialize new manager: {}", init_err);
                        Err(format!("Failed to initialize LLM: {}", init_err))
                    }
                }
            }
        }
    } else {
        // No manager exists, create new one
        tracing::info!("No existing manager, creating new one");
        let mut new_manager = LLMManager::new(config);
        match new_manager.initialize().await {
            Ok(_) => {
                *manager_lock = Some(new_manager);
                tracing::info!("Created new manager successfully");
                Ok("LLM initialized successfully".to_string())
            }
            Err(e) => {
                tracing::warn!("Failed to initialize new manager: {}", e);
                Err(format!("Failed to initialize LLM: {}", e))
            }
        }
    };
    drop(manager_lock);
    result
}

/// Replace the key in `mode` with `key` (empty: no key) when `mode` answers
/// through `provider`. Returns whether it does.
pub(crate) fn replace_provider_key(mode: &mut LLMMode, provider: ProviderId, key: &str) -> bool {
    match mode {
        LLMMode::External {
            provider: configured,
            api_key,
            ..
        } if ProviderId::from_api(configured) == Some(provider) => {
            *api_key = key.to_string();
            true
        }
        _ => false,
    }
}

/// `provider`'s saved key was removed: drop every copy of it. The configured
/// mode keeps its model (the picker still shows it) but carries the
/// environment's key, or none; the in-process manager is rebuilt with the
/// environment's key, or dropped when there is none, so nothing in this
/// process can still send the removed key.
pub(crate) async fn forget_provider_key(
    state: &LLMState,
    provider: ProviderId,
    env_key: Option<String>,
) {
    let key = env_key.map(|k| k.trim().to_string()).unwrap_or_default();
    {
        let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        replace_provider_key(&mut config.mode, provider, &key);
    }
    let mut manager = state.manager.write().await;
    let Some(mut mode) = manager.as_ref().map(|m| m.config().mode.clone()) else {
        return;
    };
    if !replace_provider_key(&mut mode, provider, &key) {
        return;
    }
    if key.is_empty() {
        *manager = None;
        return;
    }
    if let Some(current) = manager.as_mut() {
        if let Err(e) = current.switch_mode(mode).await {
            tracing::warn!(
                provider = provider.as_str(),
                "the LLM was not rebuilt with the environment key: {e}"
            );
            *manager = None;
        }
    }
}

/// A key for `provider` was saved: when the configured model answers
/// through `provider`, it (and the in-process manager, which removing a key
/// may have dropped) uses the new key from now on.
pub(crate) async fn use_provider_key(state: &LLMState, provider: ProviderId, key: &str) {
    let mode = {
        let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        let mut mode = config.mode.clone();
        if !replace_provider_key(&mut mode, provider, key.trim()) {
            return;
        }
        mode
    };
    if let Err(e) = activate_mode(state, mode).await {
        tracing::warn!(
            provider = provider.as_str(),
            "the LLM was not rebuilt with the saved key: {e}"
        );
    }
}

/// Get LLM info
#[tauri::command]
pub async fn get_llm_info(state: State<'_, LLMState>) -> Result<LLMInfo, String> {
    let manager_lock = state.manager.read().await;

    let manager = match manager_lock.as_ref() {
        Some(m) => m,
        None => return Err("LLM not initialized".to_string()),
    };

    let info = match manager.info() {
        Some(i) => i,
        None => return Err("No provider active".to_string()),
    };

    let memory = manager.memory_usage();
    let mode = manager.config().mode.kind();

    Ok(LLMInfo {
        provider: info.name,
        model: info.model,
        context_window: info.context_window,
        supports_streaming: info.supports_streaming,
        is_local: info.is_local,
        memory_usage: memory.map(|m| MemoryInfo {
            ram_mb: m.ram_mb,
            vram_mb: m.vram_mb,
            model_size_mb: m.model_size_mb,
        }),
        mode: mode.to_string(),
    })
}

/// Store a provider API key in the OS credential store and make it available
/// to this session. The key is never echoed back to the frontend.
#[tauri::command]
pub async fn set_api_key(
    state: State<'_, LLMState>,
    audit: State<'_, AuditState>,
    provider: String,
    api_key: String,
) -> Result<(), String> {
    if !api_key_store::is_known_provider(&provider) {
        return Err(format!("Unknown provider: {provider}"));
    }
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("API key is empty".to_string());
    }

    // Persist first: if the credential store rejects the key, the session must
    // not silently hold a key that will be gone after restart.
    let to_store = api_key.clone();
    let store_provider = provider.clone();
    tokio::task::spawn_blocking(move || api_key_store::store(&store_provider, &to_store))
        .await
        .map_err(|e| format!("Credential store task failed: {e}"))??;

    {
        let mut api_keys = state.api_keys.lock().unwrap_or_else(|e| e.into_inner());
        api_keys.set(&provider, Some(api_key));
    }
    // The provider id only; the key value is never passed to the audit log.
    audit.record(AuditRecord::new(
        AuditEventType::SettingsChange,
        audit_payload::api_key_change(&provider, audit_payload::KeyAction::Set),
    ));
    Ok(())
}

// Helper functions and types

#[derive(Serialize)]
pub struct LLMInfo {
    provider: String,
    model: String,
    context_window: usize,
    supports_streaming: bool,
    is_local: bool,
    memory_usage: Option<MemoryInfo>,
    mode: String,
}

#[derive(Serialize)]
pub struct MemoryInfo {
    ram_mb: usize,
    vram_mb: Option<usize>,
    model_size_mb: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::llm::ApiProvider;

    fn mode(provider: ApiProvider, key: &str) -> LLMMode {
        LLMMode::External {
            provider,
            api_key: key.into(),
            model: "some/model".into(),
        }
    }

    fn key_of(mode: &LLMMode) -> Option<&str> {
        match mode {
            LLMMode::External { api_key, .. } => Some(api_key),
            _ => None,
        }
    }

    async fn state_with(mode: LLMMode) -> LLMState {
        let config = LLMConfig {
            mode,
            ..LLMConfig::default()
        };
        let mut manager = LLMManager::new(config.clone());
        manager.initialize().await.unwrap();
        LLMState {
            manager: Arc::new(AsyncRwLock::new(Some(manager))),
            config: Arc::new(Mutex::new(config)),
            api_keys: Arc::new(Mutex::new(ApiKeys::default())),
            custom_model_path: Arc::new(Mutex::new(None)),
        }
    }

    #[tokio::test]
    async fn a_removed_key_leaves_no_copy_in_the_mode_or_the_manager() {
        let state = state_with(mode(ApiProvider::OpenRouter, "sk-or-removed-key")).await;
        forget_provider_key(&state, ProviderId::OpenRouter, None).await;
        assert_eq!(key_of(&state.config.lock().unwrap().mode), Some(""));
        assert!(state.manager.read().await.is_none(), "no key: no manager");

        // With an environment key, both carry that key.
        let state = state_with(mode(ApiProvider::OpenRouter, "sk-or-removed-key")).await;
        forget_provider_key(
            &state,
            ProviderId::OpenRouter,
            Some(" sk-or-env-key ".into()),
        )
        .await;
        assert_eq!(
            key_of(&state.config.lock().unwrap().mode),
            Some("sk-or-env-key")
        );
        let manager = state.manager.read().await;
        let manager_mode = &manager.as_ref().unwrap().config().mode;
        assert_eq!(key_of(manager_mode), Some("sk-or-env-key"));
        drop(manager);

        // Another provider's key stays.
        let state = state_with(mode(ApiProvider::Anthropic, "sk-ant-kept-key")).await;
        forget_provider_key(&state, ProviderId::OpenRouter, None).await;
        assert_eq!(
            key_of(&state.config.lock().unwrap().mode),
            Some("sk-ant-kept-key")
        );
        assert!(state.manager.read().await.is_some());
    }

    #[tokio::test]
    async fn a_key_saved_after_a_removal_is_used_again() {
        let state = state_with(mode(ApiProvider::OpenRouter, "sk-or-removed-key")).await;
        forget_provider_key(&state, ProviderId::OpenRouter, None).await;
        use_provider_key(&state, ProviderId::OpenRouter, "sk-or-new-key").await;
        assert_eq!(
            key_of(&state.config.lock().unwrap().mode),
            Some("sk-or-new-key")
        );
        let manager = state.manager.read().await;
        let manager_mode = &manager.as_ref().expect("rebuilt").config().mode;
        assert_eq!(key_of(manager_mode), Some("sk-or-new-key"));
        drop(manager);

        // A key for another provider leaves the configured model alone.
        use_provider_key(&state, ProviderId::Anthropic, "sk-ant-other").await;
        assert_eq!(
            key_of(&state.config.lock().unwrap().mode),
            Some("sk-or-new-key")
        );
    }

    #[test]
    fn only_the_providers_key_is_replaced() {
        let mut m = mode(ApiProvider::OpenRouter, "sk-or-old");
        assert!(replace_provider_key(&mut m, ProviderId::OpenRouter, ""));
        assert_eq!(key_of(&m), Some(""));
        let mut other = mode(ApiProvider::Google, "g-key");
        assert!(!replace_provider_key(
            &mut other,
            ProviderId::OpenRouter,
            ""
        ));
        assert_eq!(key_of(&other), Some("g-key"));
        let mut disabled = LLMMode::Disabled;
        assert!(!replace_provider_key(
            &mut disabled,
            ProviderId::OpenRouter,
            ""
        ));
    }

    #[test]
    fn merge_missing_keeps_session_keys_and_fills_gaps() {
        let mut keys = ApiKeys {
            openai: Some("session-key".to_string()),
            anthropic: Some("   ".to_string()),
            ..ApiKeys::default()
        };
        let merged = keys.merge_missing(
            vec![
                ("openai", "stored-openai".to_string()),
                ("anthropic", "stored-anthropic".to_string()),
                ("google", "stored-google".to_string()),
            ],
            &KeyGenerations::default(),
        );
        assert_eq!(merged, 2);
        assert_eq!(keys.openai.as_deref(), Some("session-key"));
        assert_eq!(keys.anthropic.as_deref(), Some("stored-anthropic"));
        assert_eq!(keys.google.as_deref(), Some("stored-google"));
        assert_eq!(
            keys.configured_providers(),
            vec!["openai", "anthropic", "google"]
        );
    }

    #[test]
    fn a_key_removed_while_the_store_is_read_is_not_restored() {
        let mut keys = ApiKeys::default();
        keys.set("openrouter", Some("sk-or-old".to_string()));
        // Startup reads the credential store...
        let since = keys.generations();
        // ...while the user removes the OpenRouter key. Google's is untouched.
        keys.set("openrouter", None);
        let merged = keys.merge_missing(
            vec![
                ("openrouter", "sk-or-old".to_string()),
                ("google", "g-key".to_string()),
            ],
            &since,
        );
        assert_eq!(merged, 1);
        assert_eq!(keys.openrouter, None, "the removed key stays removed");
        assert_eq!(keys.google.as_deref(), Some("g-key"));
    }

    #[test]
    fn every_change_of_a_key_moves_its_generation() {
        let mut keys = ApiKeys::default();
        assert_eq!(keys.generation("openrouter"), 0);
        keys.set("openrouter", Some("a".to_string()));
        keys.set("openrouter", None);
        assert_eq!(keys.generation("openrouter"), 2);
        assert_eq!(keys.generation("google"), 0);
        keys.set("not-a-provider", Some("x".to_string()));
        assert_eq!(keys.generation("not-a-provider"), 0);
    }

    #[test]
    fn debug_output_never_contains_key_values() {
        let keys = ApiKeys {
            openrouter: Some("sk-or-secret-value".to_string()),
            ..ApiKeys::default()
        };
        let rendered = format!("{keys:?}");
        assert!(!rendered.contains("sk-or-secret-value"));
        assert!(rendered.contains("openrouter"));
    }
}
