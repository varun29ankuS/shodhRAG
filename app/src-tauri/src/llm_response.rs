//! LLM Response with metadata (token counts, timing, etc.)

use serde::{Deserialize, Serialize};

/// LLM generation response with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// Generated text
    pub response: String,

    /// Number of input tokens processed
    pub input_tokens: usize,

    /// Number of output tokens generated
    pub output_tokens: usize,

    /// Generation duration in milliseconds
    pub duration_ms: u64,

    /// Tokens per second (generation speed)
    pub tokens_per_sec: f32,

    /// Query intent classification
    pub query_intent: Option<String>,
}

impl LLMResponse {
    pub fn new(
        response: String,
        input_tokens: usize,
        output_tokens: usize,
        duration_ms: u64,
    ) -> Self {
        let tokens_per_sec = if duration_ms > 0 {
            (output_tokens as f64 / (duration_ms as f64 / 1000.0)) as f32
        } else {
            0.0
        };

        Self {
            response,
            input_tokens,
            output_tokens,
            duration_ms,
            tokens_per_sec,
            query_intent: None,
        }
    }

    pub fn with_intent(mut self, intent: String) -> Self {
        self.query_intent = Some(intent);
        self
    }
}
