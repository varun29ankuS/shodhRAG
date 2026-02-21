//! Streaming response handling for LLM generation

use tokio::sync::mpsc;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Token stream for streaming generation
pub struct TokenStream {
    receiver: mpsc::Receiver<String>,
}

impl TokenStream {
    pub fn new(receiver: mpsc::Receiver<String>) -> Self {
        Self { receiver }
    }
    
    /// Get next token
    pub async fn next(&mut self) -> Option<String> {
        self.receiver.recv().await
    }
    
    /// Collect all tokens into a string
    pub async fn collect(mut self) -> String {
        let mut result = String::new();
        while let Some(token) = self.next().await {
            result.push_str(&token);
        }
        result
    }
}

impl Stream for TokenStream {
    type Item = String;
    
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