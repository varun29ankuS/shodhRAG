//! ONNX-based LLM implementation - Stubbed for lightweight build
//! Use API providers instead.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::path::Path;

use super::{
    LLMProvider, GenerationConfig, ProviderInfo, MemoryUsage,
    LocalModel, DeviceType, QuantizationType,
};
use super::streaming::TokenStream;

/// Unified tokenizer for ONNX models
pub struct UnifiedTokenizer;

impl UnifiedTokenizer {
    pub fn encode(&self, _text: &str, _add_special: bool) -> Result<Vec<u32>> {
        Err(anyhow!("ONNX LLM tokenizer not available"))
    }

    pub fn decode(&self, _tokens: &[u32]) -> Result<String> {
        Err(anyhow!("ONNX LLM tokenizer not available"))
    }
}

pub struct ONNXLLMProvider;

impl ONNXLLMProvider {
    pub fn new(
        _model: LocalModel,
        _device: DeviceType,
        _quantization: QuantizationType,
        _cache_dir: &Path,
    ) -> Result<Self> {
        Err(anyhow!("ONNX LLM is not available in this build. Use an API provider instead."))
    }
}

#[async_trait]
impl LLMProvider for ONNXLLMProvider {
    async fn generate(&self, _prompt: &str, _config: &GenerationConfig) -> Result<String> {
        Err(anyhow!("ONNX LLM not available"))
    }

    async fn generate_stream(&self, _prompt: &str, _config: &GenerationConfig) -> Result<TokenStream> {
        Err(anyhow!("ONNX LLM not available"))
    }

    async fn generate_with_context(&self, _query: &str, _context: Vec<String>, _config: &GenerationConfig) -> Result<String> {
        Err(anyhow!("ONNX LLM not available"))
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "ONNX LLM (unavailable)".to_string(),
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
