//! Safe Rust wrappers for ONNX Runtime GenAI - Stubbed
//! Not available in lightweight build.

use anyhow::{Result, anyhow};

pub struct Model;
pub struct Tokenizer;

pub struct SearchOptions {
    pub max_length: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub repetition_penalty: f32,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_length: 4096,
            temperature: 0.7,
            top_p: 0.95,
            top_k: 40,
            repetition_penalty: 1.1,
        }
    }
}

pub fn generate_text(_model: &Model, _tokenizer: &Tokenizer, _prompt: &str, _options: &SearchOptions) -> Result<String> {
    Err(anyhow!("ONNX Runtime GenAI not available in this build"))
}
