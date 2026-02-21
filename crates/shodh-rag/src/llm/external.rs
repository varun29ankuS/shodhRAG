//! External API providers for LLM
//! Supports OpenAI, Anthropic, and custom endpoints

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use serde_json::json;
use tokio::sync::mpsc;
use futures_util::stream::StreamExt;

use super::{
    LLMProvider, GenerationConfig,
    ProviderInfo, MemoryUsage, TokenStream,
    streaming::StreamingResponse,
};
use crate::llm::ApiProvider;

/// External API provider
pub struct ExternalProvider {
    provider: ApiProvider,
    api_key: String,
    model: String,
    client: Client,
}

impl ExternalProvider {
    /// Parse a response body as JSON, returning a clear error if the server returned HTML.
    async fn parse_json_response<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
        endpoint: &str,
    ) -> Result<T> {
        let status = response.status();
        let body = response.text().await.map_err(|e| {
            anyhow!("Failed to read response body from {}: {}", endpoint, e)
        })?;
        let trimmed = body.trim_start();
        if trimmed.starts_with('<') || trimmed.starts_with("<!") {
            let preview: String = trimmed.chars().take(200).collect();
            return Err(anyhow!(
                "Endpoint {} returned HTML instead of JSON (HTTP {}) — service may be down. Response: {}",
                endpoint, status, preview
            ));
        }
        serde_json::from_str::<T>(&body).map_err(|e| {
            let preview: String = body.chars().take(300).collect();
            anyhow!("Failed to parse JSON from {} (HTTP {}): {}. Body: {}", endpoint, status, e, preview)
        })
    }

    /// Create new external provider
    pub fn new(provider: ApiProvider, api_key: String, model: String) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_nodelay(true)
            .build()?;

        Ok(Self {
            provider,
            api_key,
            model,
            client,
        })
    }
    
    fn get_endpoint(&self) -> String {
        match &self.provider {
            ApiProvider::OpenAI => "https://api.openai.com/v1/chat/completions".to_string(),
            ApiProvider::Anthropic => "https://api.anthropic.com/v1/messages".to_string(),
            ApiProvider::OpenRouter => "https://openrouter.ai/api/v1/chat/completions".to_string(),
            ApiProvider::Together => "https://api.together.xyz/v1/chat/completions".to_string(),
            ApiProvider::Grok => "https://api.x.ai/v1/chat/completions".to_string(),
            ApiProvider::Perplexity => "https://api.perplexity.ai/chat/completions".to_string(),
            ApiProvider::Google => format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model),
            ApiProvider::Replicate => "https://api.replicate.com/v1/predictions".to_string(),
            ApiProvider::Baseten => "https://inference.baseten.co/v1/chat/completions".to_string(),
            ApiProvider::Ollama => "http://localhost:11434/v1/chat/completions".to_string(),
            ApiProvider::HuggingFace { .. } => "https://api-inference.huggingface.co/models".to_string(),
            ApiProvider::Custom { endpoint } => endpoint.clone(),
        }
    }
}

#[async_trait]
impl LLMProvider for ExternalProvider {
    async fn generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String> {
        match &self.provider {
            ApiProvider::OpenAI | ApiProvider::Together | ApiProvider::Grok | ApiProvider::Perplexity | ApiProvider::Baseten | ApiProvider::Ollama => {
                self.openai_compatible_generate(prompt, config).await
            }
            ApiProvider::Anthropic => {
                self.anthropic_generate(prompt, config).await
            }
            ApiProvider::Google => {
                self.google_generate(prompt, config).await
            }
            ApiProvider::OpenRouter => {
                self.openai_compatible_generate(prompt, config).await
            }
            ApiProvider::HuggingFace { model_id } => {
                self.huggingface_generate(prompt, config, model_id).await
            }
            ApiProvider::Replicate => {
                self.replicate_generate(prompt, config).await
            }
            ApiProvider::Custom { .. } => {
                // Assume OpenAI compatible by default
                self.openai_compatible_generate(prompt, config).await
            }
        }
    }

    async fn generate_stream(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<TokenStream> {
        let (tx, rx) = mpsc::channel(100);

        let provider = self.provider.clone();
        let api_key = self.api_key.clone();
        let model = self.model.clone();
        let endpoint = self.get_endpoint();
        let prompt = prompt.to_string();
        let config = config.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            match provider {
                ApiProvider::OpenAI | ApiProvider::Together | ApiProvider::Grok |
                ApiProvider::Perplexity | ApiProvider::Baseten | ApiProvider::Ollama | ApiProvider::Custom { .. } => {
                    stream_openai_compatible(client, endpoint, api_key, model, prompt, config, tx).await
                }
                ApiProvider::Anthropic => {
                    stream_anthropic(client, api_key, model, prompt, config, tx).await
                }
                ApiProvider::Google => {
                    let _ = tx.send("Google streaming not yet implemented, use non-streaming mode".to_string()).await;
                }
                ApiProvider::OpenRouter => {
                    stream_openai_compatible(client, endpoint, api_key, model, prompt, config, tx).await
                }
                ApiProvider::HuggingFace { model_id } => {
                    stream_huggingface(client, api_key, model_id.clone(), prompt, config, tx).await
                }
                ApiProvider::Replicate => {
                    stream_replicate(client, api_key, model, prompt, config, tx).await
                }
            }
        });

        Ok(TokenStream::new(rx))
    }

    async fn generate_with_context(
        &self,
        query: &str,
        context: Vec<String>,
        config: &GenerationConfig,
    ) -> Result<String> {
        // Extract system context if present (first item should be system_context from build_system_context())
        let (system_prompt, user_context) = if !context.is_empty() && context[0].contains("STRUCTURED OUTPUT FORMAT") {
            // First context item is the system prompt with STRUCTURED_OUTPUT_INSTRUCTIONS
            (Some(context[0].as_str()), &context[1..])
        } else {
            (None, context.as_slice())
        };

        let prompt = super::format_rag_prompt(query, user_context, system_prompt);
        self.generate(&prompt, config).await
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: format!("{:?}", self.provider),
            model: self.model.clone(),
            context_window: match &self.provider {
                ApiProvider::OpenAI => 128000, // GPT-4 Turbo
                ApiProvider::Anthropic => 200000, // Claude 3
                ApiProvider::OpenRouter => 200000, // Various models with different contexts
                ApiProvider::Together => 32768,
                ApiProvider::Grok => 131072,  // Grok supports 128k context
                ApiProvider::Perplexity => 16384,
                ApiProvider::Google => 1000000, // Gemini 2.5 Pro supports 1M context
                ApiProvider::Replicate => 4096,
                ApiProvider::Baseten => 128000, // GPT-OSS-120B supports 128k context
                ApiProvider::Ollama => 32768,
                ApiProvider::HuggingFace { .. } => 4096,
                ApiProvider::Custom { .. } => 4096,
            },
            supports_streaming: true,
            supports_functions: matches!(self.provider, ApiProvider::OpenAI | ApiProvider::Ollama),
            is_local: matches!(self.provider, ApiProvider::Ollama),
        }
    }

    async fn is_ready(&self) -> bool {
        // Could ping the API endpoint to check
        true
    }

    fn memory_usage(&self) -> MemoryUsage {
        MemoryUsage {
            ram_mb: 0, // External APIs don't use local memory
            vram_mb: None,
            model_size_mb: 0,
        }
    }
}

impl ExternalProvider {
    /// OpenAI-compatible generation (OpenAI, Together, etc.)
    async fn openai_compatible_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String> {
        let endpoint = self.get_endpoint();
        let request = json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "frequency_penalty": config.repetition_penalty - 1.0,
            "stream": false
        });

        let response = self.client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow!("Request to {} timed out — check network connectivity", endpoint)
                } else if e.is_connect() {
                    anyhow!("Failed to connect to {} — check network/firewall/proxy: {}", endpoint, e)
                } else {
                    anyhow!("Request to {} failed: {}", endpoint, e)
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error = response.text().await?;
            return Err(anyhow!("API error ({}): {}", status, error));
        }

        let result: OpenAIResponse = Self::parse_json_response(response, &endpoint).await?;
        result.choices.first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| anyhow!("OpenAI returned empty choices array"))
    }

    /// Anthropic generation
    async fn anthropic_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String> {
        let request = json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
        });
        
        let response = self.client
            .post(self.get_endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request)
            .send()
            .await?;
        
        if !response.status().is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Anthropic API error: {}", error));
        }
        
        let endpoint = self.get_endpoint();
        let result: AnthropicResponse = Self::parse_json_response(response, &endpoint).await?;
        result.content.first()
            .map(|c| c.text.clone())
            .ok_or_else(|| anyhow!("Anthropic returned empty content array"))
    }

    /// Google Gemini API generation
    async fn google_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String> {
        let request = json!({
            "contents": [{
                "parts": [{"text": prompt}]
            }],
            "generationConfig": {
                "temperature": config.temperature,
                "topP": config.top_p,
                "topK": config.top_k,
                "maxOutputTokens": config.max_tokens,
            }
        });

        let response = self.client
            .post(self.get_endpoint())
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Google API error: {}", error));
        }

        let endpoint = self.get_endpoint();
        let result: GoogleResponse = Self::parse_json_response(response, &endpoint).await?;
        if let Some(candidate) = result.candidates.first() {
            if let Some(part) = candidate.content.parts.first() {
                return Ok(part.text.clone());
            }
        }

        Err(anyhow!("No response from Google Gemini"))
    }

    /// HuggingFace Inference API generation
    async fn huggingface_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
        model_id: &str,
    ) -> Result<String> {
        let endpoint = format!("https://api-inference.huggingface.co/models/{}", model_id);
        
        let request = json!({
            "inputs": prompt,
            "parameters": {
                "max_new_tokens": config.max_tokens,
                "temperature": config.temperature,
                "top_p": config.top_p,
                "repetition_penalty": config.repetition_penalty,
                "do_sample": true,
                "return_full_text": false
            }
        });
        
        let response = self.client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await?;
        
        if !response.status().is_success() {
            let error = response.text().await?;
            return Err(anyhow!("HuggingFace API error: {}", error));
        }
        
        let result: Vec<HuggingFaceResponse> = Self::parse_json_response(response, &endpoint).await?;
        result.first()
            .map(|r| r.generated_text.clone())
            .ok_or_else(|| anyhow!("HuggingFace returned empty response array"))
    }
    
    /// Replicate API generation
    async fn replicate_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String> {
        let request = json!({
            "version": self.model,
            "input": {
                "prompt": prompt,
                "max_tokens": config.max_tokens,
                "temperature": config.temperature,
                "top_p": config.top_p,
                "repetition_penalty": config.repetition_penalty
            }
        });
        
        let response = self.client
            .post("https://api.replicate.com/v1/predictions")
            .header("Authorization", format!("Token {}", self.api_key))
            .json(&request)
            .send()
            .await?;
        
        if !response.status().is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Replicate API error: {}", error));
        }
        
        let result: ReplicateResponse = Self::parse_json_response(response, "https://api.replicate.com/v1/predictions").await?;
        
        // Poll for completion
        let prediction_url = format!("https://api.replicate.com/v1/predictions/{}", result.id);
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 60; // 5 minutes with 5 second intervals
        
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            
            let status_response = self.client
                .get(&prediction_url)
                .header("Authorization", format!("Token {}", self.api_key))
                .send()
                .await?;
            
            let status: ReplicateStatusResponse = Self::parse_json_response(status_response, &prediction_url).await?;
            
            match status.status.as_str() {
                "succeeded" => {
                    if let Some(output) = status.output {
                        if let Some(text) = output.as_array().and_then(|arr| arr.first()) {
                            if let Some(result_text) = text.as_str() {
                                return Ok(result_text.to_string());
                            }
                        }
                        return Ok(output.to_string());
                    }
                    return Err(anyhow!("Replicate prediction succeeded but no output"));
                }
                "failed" => {
                    return Err(anyhow!("Replicate prediction failed: {:?}", status.error));
                }
                "canceled" => {
                    return Err(anyhow!("Replicate prediction was canceled"));
                }
                _ => {
                    attempts += 1;
                    if attempts >= MAX_ATTEMPTS {
                        return Err(anyhow!("Replicate prediction timeout"));
                    }
                }
            }
        }
    }
}

/// Response structures
#[derive(Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize)]
struct OpenAIMessage {
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    text: String,
}

#[derive(Deserialize)]
struct GoogleResponse {
    candidates: Vec<GoogleCandidate>,
}

#[derive(Deserialize)]
struct GoogleCandidate {
    content: GoogleContent,
}

#[derive(Deserialize)]
struct GoogleContent {
    parts: Vec<GooglePart>,
}

#[derive(Deserialize)]
struct GooglePart {
    text: String,
}

#[derive(Deserialize)]
struct HuggingFaceResponse {
    generated_text: String,
}

#[derive(Deserialize)]
struct ReplicateResponse {
    id: String,
}

#[derive(Deserialize)]
struct ReplicateStatusResponse {
    status: String,
    output: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

/// Streaming helpers
async fn stream_openai_compatible(
    client: Client,
    endpoint: String,
    api_key: String,
    model: String,
    prompt: String,
    config: GenerationConfig,
    tx: mpsc::Sender<String>,
) {
    let request = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": prompt}
        ],
        "max_tokens": config.max_tokens,
        "temperature": config.temperature,
        "top_p": config.top_p,
        "frequency_penalty": config.repetition_penalty - 1.0,
        "stream": true
    });

    let response = match client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("OpenAI-compatible stream request failed: {}", e);
            return;
        }
    };

    if !response.status().is_success() {
        tracing::error!("OpenAI-compatible stream API error: {}", response.status());
        return;
    }

    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                let chunk_str = String::from_utf8_lossy(&chunk);
                for line in chunk_str.lines() {
                    if line.starts_with("data: ") {
                        let data = &line[6..];
                        if data == "[DONE]" {
                            return;
                        }
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(choices) = parsed["choices"].as_array() {
                                if let Some(choice) = choices.first() {
                                    if let Some(delta) = choice["delta"].as_object() {
                                        if let Some(content) = delta["content"].as_str() {
                                            if tx.send(content.to_string()).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("OpenAI-compatible stream chunk error: {}", e);
                break;
            }
        }
    }
}

async fn stream_anthropic(
    client: Client,
    api_key: String,
    model: String,
    prompt: String,
    config: GenerationConfig,
    tx: mpsc::Sender<String>,
) {
    let request = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": prompt}
        ],
        "max_tokens": config.max_tokens,
        "temperature": config.temperature,
        "top_p": config.top_p,
        "stream": true
    });

    let response = match client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Anthropic stream request failed: {}", e);
            return;
        }
    };

    if !response.status().is_success() {
        tracing::error!("Anthropic stream API error: {}", response.status());
        return;
    }

    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                let chunk_str = String::from_utf8_lossy(&chunk);
                for line in chunk_str.lines() {
                    if line.starts_with("data: ") {
                        let data = &line[6..];
                        // Anthropic uses "message_stop" event, not "[DONE]"
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(event_type) = parsed["type"].as_str() {
                                match event_type {
                                    "content_block_delta" => {
                                        if let Some(delta) = parsed["delta"].as_object() {
                                            if let Some(text) = delta["text"].as_str() {
                                                if tx.send(text.to_string()).await.is_err() {
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    "message_stop" => return,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Anthropic stream chunk error: {}", e);
                break;
            }
        }
    }
}


async fn stream_huggingface(
    client: Client,
    api_key: String,
    model_id: String,
    prompt: String,
    config: GenerationConfig,
    tx: mpsc::Sender<String>,
) {
    let endpoint = format!("https://api-inference.huggingface.co/models/{}", model_id);
    let request = json!({
        "inputs": prompt,
        "parameters": {
            "max_new_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "repetition_penalty": config.repetition_penalty,
            "do_sample": true,
            "return_full_text": false
        },
        "stream": true
    });

    let response = match client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("HuggingFace stream request failed: {}", e);
            return;
        }
    };

    if !response.status().is_success() {
        tracing::error!("HuggingFace stream API error: {}", response.status());
        return;
    }

    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                let chunk_str = String::from_utf8_lossy(&chunk);
                for line in chunk_str.lines() {
                    if line.starts_with("data: ") {
                        let data = &line[6..];
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(token) = parsed["token"]["text"].as_str() {
                                if tx.send(token.to_string()).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("HuggingFace stream chunk error: {}", e);
                break;
            }
        }
    }
}

async fn stream_replicate(
    client: Client,
    api_key: String,
    model: String,
    prompt: String,
    config: GenerationConfig,
    tx: mpsc::Sender<String>,
) {
    let request = json!({
        "version": model,
        "input": {
            "prompt": prompt,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "repetition_penalty": config.repetition_penalty
        },
        "stream": true
    });

    let response = match client
        .post("https://api.replicate.com/v1/predictions")
        .header("Authorization", format!("Token {}", api_key))
        .json(&request)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Replicate stream request failed: {}", e);
            return;
        }
    };

    let result = match response.json::<ReplicateResponse>().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Replicate stream response parse failed: {}", e);
            return;
        }
    };

    let prediction_url = format!("https://api.replicate.com/v1/predictions/{}/stream", result.id);
    let stream_response = match client
        .get(&prediction_url)
        .header("Authorization", format!("Token {}", api_key))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Replicate stream connection failed: {}", e);
            return;
        }
    };

    let mut stream = stream_response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                let chunk_str = String::from_utf8_lossy(&chunk);
                for line in chunk_str.lines() {
                    if line.starts_with("data: ") {
                        let data = &line[6..];
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(text) = parsed.as_str() {
                                if tx.send(text.to_string()).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Replicate stream chunk error: {}", e);
                break;
            }
        }
    }
}