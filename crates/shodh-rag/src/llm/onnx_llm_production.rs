//! Production ONNX LLM implementation - Stubbed for lightweight build

use anyhow::{Result, anyhow};

pub async fn generate_with_kv_cache_optimized(
    _session: &std::sync::Arc<parking_lot::Mutex<ort::session::Session>>,
    _tokenizer: &std::sync::Arc<super::onnx_llm::UnifiedTokenizer>,
    _prompt: &str,
    _config: &super::GenerationConfig,
) -> Result<String> {
    Err(anyhow!("ONNX LLM production mode not available in this build"))
}
