//! llama.cpp LLM Provider - Stubbed for lightweight build
//! Use API providers (OpenAI, Anthropic, etc.) instead.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::path::Path;

use super::{
    LLMProvider, GenerationConfig, ProviderInfo, MemoryUsage,
    LocalModel, DeviceType, QuantizationType,
};
use super::streaming::TokenStream;

pub struct LlamaCppProvider;

impl LlamaCppProvider {
    pub fn new(
        _model: LocalModel,
        _device: DeviceType,
        _quantization: QuantizationType,
        _cache_dir: &Path,
    ) -> Result<Self> {
        Err(anyhow!("Local LLM (llama.cpp) is not available in this build. Use an API provider instead."))
    }
}

#[async_trait]
impl LLMProvider for LlamaCppProvider {
    async fn generate(&self, _prompt: &str, _config: &GenerationConfig) -> Result<String> {
        Err(anyhow!("LlamaCpp provider not available"))
    }

    async fn generate_stream(&self, _prompt: &str, _config: &GenerationConfig) -> Result<TokenStream> {
        Err(anyhow!("LlamaCpp provider not available"))
    }

    async fn generate_with_context(&self, _query: &str, _context: Vec<String>, _config: &GenerationConfig) -> Result<String> {
        Err(anyhow!("LlamaCpp provider not available"))
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "llama.cpp (unavailable)".to_string(),
            model: "none".to_string(),
            context_window: 0,
            supports_streaming: false,
            supports_functions: false,
            is_local: true,
        }
    }

    async fn is_ready(&self) -> bool { false }

    fn memory_usage(&self) -> MemoryUsage {
        MemoryUsage { ram_mb: 0, vram_mb: None, model_size_mb: 0 }
    }
}
