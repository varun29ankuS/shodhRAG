//! Streaming response handling for LLM generation

use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

use super::failure::ProviderFailure;

/// Token stream for streaming generation. A failed generation ends with one
/// `Err` (an error chunk, a broken or empty stream), never as if complete.
pub struct TokenStream {
    receiver: mpsc::Receiver<Result<String, ProviderFailure>>,
}

impl TokenStream {
    pub fn new(receiver: mpsc::Receiver<Result<String, ProviderFailure>>) -> Self {
        Self { receiver }
    }

    /// Get next token
    pub async fn next(&mut self) -> Option<Result<String, ProviderFailure>> {
        self.receiver.recv().await
    }

    /// Collect all tokens into a string; the failure if the stream failed.
    pub async fn collect(mut self) -> Result<String, ProviderFailure> {
        let mut result = String::new();
        while let Some(token) = self.next().await {
            result.push_str(&token?);
        }
        Ok(result)
    }
}

impl Stream for TokenStream {
    type Item = Result<String, ProviderFailure>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

/// Streaming response handler
pub struct StreamingResponse {
    pub tokens: Vec<String>,
    pub is_complete: bool,
    pub total_tokens: usize,
}

impl Default for StreamingResponse {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingResponse {
    pub fn new() -> Self {
        Self {
            tokens: Vec::new(),
            is_complete: false,
            total_tokens: 0,
        }
    }

    pub fn add_token(&mut self, token: String) {
        self.tokens.push(token);
        self.total_tokens += 1;
    }

    pub fn complete(&mut self) {
        self.is_complete = true;
    }

    pub fn get_text(&self) -> String {
        self.tokens.join("")
    }
}
