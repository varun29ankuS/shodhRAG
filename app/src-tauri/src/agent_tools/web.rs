//! The app side of the web tools: policy (Local-only mode, web access) and
//! search provider configuration.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use shodh_rag::harness::tools::web::{FetchUrlTool, SearchPapersTool, WebEnv, WebSearchTool};
use shodh_rag::harness::tools::{RegistryError, ToolRegistry};
use shodh_rag::harness::web::relevance::SharedScorer;
use shodh_rag::harness::web::search::{ApiKey, SearchConfig};
use shodh_rag::harness::web::SafeClient;
use shodh_rag::rag_engine::{RAGEngine, SharedReranker};
use tokio::sync::{OnceCell, RwLock};
use url::Url;

use super::{AgentHost, HostEffects};
use crate::app_settings::SettingsStore;

/// Optional model for OpenRouter web searches.
pub const WEB_SEARCH_MODEL_ENV: &str = "SHODH_WEB_SEARCH_MODEL";
/// Gemini key for Google Search grounding.
pub const GEMINI_KEY_ENV: &str = "GEMINI_API_KEY";
/// Base URL of a SearXNG instance (e.g. `http://searx.lan:8888/search`).
pub const SEARXNG_URL_ENV: &str = "SHODH_SEARXNG_URL";

pub struct AppWebEnv {
    data_dir: PathBuf,
    effects: Arc<dyn HostEffects>,
    rag: Arc<RwLock<RAGEngine>>,
    /// The engine's cross-encoder slot, fetched once: the engine lock is
    /// held for long stretches while indexing, the slot's is not.
    reranker: OnceCell<SharedReranker>,
}

impl AppWebEnv {
    pub fn new(host: &AgentHost) -> Self {
        Self {
            data_dir: host.data_dir.clone(),
            effects: host.effects.clone(),
            rag: host.rag.clone(),
            reranker: OnceCell::new(),
        }
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Why web access is unavailable under the stored policy. An unreadable
/// settings file blocks the web: the safe default when the policy is
/// unknown.
pub fn web_block_reason(data_dir: &std::path::Path) -> Option<String> {
    match SettingsStore::in_dir(data_dir).load() {
        Ok(settings) => settings.policy.web_block_reason().map(str::to_string),
        Err(e) => Some(format!(
            "Web access is unavailable because the settings could not be read ({e})."
        )),
    }
}

#[async_trait]
impl WebEnv for AppWebEnv {
    fn blocked(&self) -> Option<String> {
        web_block_reason(&self.data_dir)
    }

    async fn relevance_scorer(&self) -> Option<SharedScorer> {
        let slot = self
            .reranker
            .get_or_init(|| async { self.rag.read().await.reranker_handle() })
            .await;
        let reranker = slot.read().clone()?;
        Some(Arc::new(reranker))
    }

    async fn search_config(&self) -> SearchConfig {
        SearchConfig {
            openrouter_key: self.effects.openrouter_key().and_then(ApiKey::new),
            openrouter_model: env_value(WEB_SEARCH_MODEL_ENV),
            gemini_key: env_value(GEMINI_KEY_ENV).and_then(ApiKey::new),
            gemini_model: None,
            searxng_url: env_value(SEARXNG_URL_ENV).and_then(|u| match Url::parse(&u) {
                Ok(url) => Some(url),
                Err(e) => {
                    tracing::warn!(
                        "{SEARXNG_URL_ENV} is not a valid URL ({e}); SearXNG is not used"
                    );
                    None
                }
            }),
        }
    }
}

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
    client: &SafeClient,
) -> Result<(), RegistryError> {
    let env: Arc<dyn WebEnv> = Arc::new(AppWebEnv::new(host));
    registry.register(Arc::new(WebSearchTool::new(env.clone(), client.clone())))?;
    registry.register(Arc::new(FetchUrlTool::new(env.clone(), client.clone())))?;
    registry.register(Arc::new(SearchPapersTool::new(env, client.clone())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;

    #[tokio::test]
    async fn policy_blocks_the_web_tools() {
        let t = testing::host().await;
        let env = AppWebEnv::new(&t.host);
        assert_eq!(env.blocked(), None);
        let store = SettingsStore::in_dir(&t.host.data_dir);
        store
            .update(|s| {
                s.policy.web_access = false;
                Ok(())
            })
            .unwrap();
        assert!(env.blocked().unwrap().contains("Web access"));
        store
            .update(|s| {
                s.policy.web_access = true;
                s.policy.local_only = true;
                Ok(())
            })
            .unwrap();
        assert!(env.blocked().unwrap().contains("Local-only"));
        std::fs::write(
            t.host.data_dir.join(crate::app_settings::SETTINGS_FILE),
            "{not json",
        )
        .unwrap();
        assert!(env.blocked().is_some(), "unknown policy blocks the web");
    }
}
