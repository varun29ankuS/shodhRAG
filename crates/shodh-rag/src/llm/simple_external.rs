//! Simplified external API provider implementation with working streaming
//! Production-grade implementation for OpenAI, Anthropic, and other LLM APIs

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use super::{
    streaming::StreamingResponse, ApiProvider, ChatMessage, ChatResponse, ChatRole,
    ChatStreamEvent, GenerationConfig, LLMProvider, MemoryUsage, ProviderInfo, TokenStream,
    ToolCall, ToolSchema,
};

/// External API provider (simplified for reliability)
pub struct SimpleExternalProvider {
    provider: ApiProvider,
    api_key: String,
    model: String,
    client: Client,
}

impl SimpleExternalProvider {
    /// Parse a response body as JSON, returning a clear error if the server returned HTML
    /// (e.g. a gateway error page) instead of valid JSON.
    async fn parse_json_response<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
        endpoint: &str,
    ) -> Result<T> {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read response body from {}: {}", endpoint, e))?;

        // Detect HTML error pages (CDNs/proxies sometimes return 200 with HTML)
        let trimmed = body.trim_start();
        if trimmed.starts_with('<') || trimmed.starts_with("<!") {
            let preview: String = trimmed.chars().take(200).collect();
            return Err(anyhow!(
                "Endpoint {} returned HTML instead of JSON (HTTP {}) — the service may be down or misconfigured. Response: {}",
                endpoint, status, preview
            ));
        }

        serde_json::from_str::<T>(&body).map_err(|e| {
            let preview: String = body.chars().take(300).collect();
            anyhow!(
                "Failed to parse JSON from {} (HTTP {}): {}. Response body: {}",
                endpoint,
                status,
                e,
                preview
            )
        })
    }

    pub fn new(provider: ApiProvider, api_key: String, model: String) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_nodelay(true)
            .build()?;

        tracing::info!(
            provider = ?provider,
            model = %model,
            "Creating SimpleExternalProvider (connect_timeout=15s)"
        );

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
            ApiProvider::Google => format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                self.model
            ),
            ApiProvider::Replicate => "https://api.replicate.com/v1/predictions".to_string(),
            ApiProvider::Baseten => "https://inference.baseten.co/v1/chat/completions".to_string(),
            ApiProvider::Ollama => "http://localhost:11434/v1/chat/completions".to_string(),
            ApiProvider::HuggingFace { model_id } => {
                format!("https://api-inference.huggingface.co/models/{}", model_id)
            }
            ApiProvider::Custom { endpoint } => endpoint.clone(),
        }
    }
}

#[async_trait]
impl LLMProvider for SimpleExternalProvider {
    async fn generate(&self, prompt: &str, config: &GenerationConfig) -> Result<String> {
        match &self.provider {
            ApiProvider::OpenAI
            | ApiProvider::OpenRouter
            | ApiProvider::Together
            | ApiProvider::Grok
            | ApiProvider::Perplexity
            | ApiProvider::Baseten
            | ApiProvider::Ollama => self.openai_compatible_generate(prompt, config).await,
            ApiProvider::Anthropic => self.anthropic_generate(prompt, config).await,
            ApiProvider::Google => self.google_generate(prompt, config).await,
            ApiProvider::HuggingFace { model_id } => {
                self.huggingface_generate(prompt, config, model_id).await
            }
            ApiProvider::Custom { .. } => self.openai_compatible_generate(prompt, config).await,
            ApiProvider::Replicate => Err(anyhow!(
                "Replicate requires streaming implementation - use non-blocking generation"
            )),
        }
    }

    async fn generate_stream(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<TokenStream> {
        match &self.provider {
            ApiProvider::OpenAI
            | ApiProvider::OpenRouter
            | ApiProvider::Together
            | ApiProvider::Grok
            | ApiProvider::Perplexity
            | ApiProvider::Baseten
            | ApiProvider::Ollama
            | ApiProvider::Custom { .. } => self.openai_stream(prompt, config).await,
            _ => {
                // Providers without SSE: fall back to chunked non-streaming.
                // Send word-by-word to simulate streaming (safe for any UTF-8).
                let response = self.generate(prompt, config).await?;
                let (sender, receiver) = tokio::sync::mpsc::channel(256);
                tokio::spawn(async move {
                    // Split on whitespace boundaries to avoid breaking UTF-8 chars
                    let mut chars = response.chars().peekable();
                    let mut chunk = String::with_capacity(40);
                    while let Some(c) = chars.next() {
                        chunk.push(c);
                        // Flush at word boundaries (~30 chars per chunk)
                        if chunk.len() >= 30 && (c == ' ' || c == '\n') {
                            if sender.send(std::mem::take(&mut chunk)).await.is_err() {
                                break;
                            }
                        }
                    }
                    // Flush remainder
                    if !chunk.is_empty() {
                        let _ = sender.send(chunk).await;
                    }
                });
                Ok(TokenStream::new(receiver))
            }
        }
    }

    async fn generate_with_context(
        &self,
        query: &str,
        context: Vec<String>,
        config: &GenerationConfig,
    ) -> Result<String> {
        let prompt = super::format_rag_prompt(query, &context, None);
        self.generate(&prompt, config).await
    }

    fn info(&self) -> ProviderInfo {
        let provider_name = match &self.provider {
            ApiProvider::OpenAI => "OpenAI",
            ApiProvider::Anthropic => "Anthropic",
            ApiProvider::OpenRouter => "OpenRouter",
            ApiProvider::Together => "Together",
            ApiProvider::Grok => "Grok",
            ApiProvider::Perplexity => "Perplexity",
            ApiProvider::Google => "Google",
            ApiProvider::Replicate => "Replicate",
            ApiProvider::Baseten => "Baseten",
            ApiProvider::Ollama => "Ollama",
            ApiProvider::HuggingFace { .. } => "HuggingFace",
            ApiProvider::Custom { .. } => "Custom",
        };

        ProviderInfo {
            name: provider_name.to_string(),
            model: self.model.clone(),
            context_window: match &self.provider {
                ApiProvider::OpenAI => 128000,
                ApiProvider::Anthropic => 200000,
                ApiProvider::OpenRouter => 200000, // OpenRouter supports various models with different contexts
                ApiProvider::Together => 32768,
                ApiProvider::Grok => 131072, // Grok supports 128k context
                ApiProvider::Perplexity => 16384,
                ApiProvider::Google => 1000000, // Gemini 2.5 Pro supports 1M context
                ApiProvider::Replicate => 4096,
                ApiProvider::Baseten => 128000,
                ApiProvider::Ollama => 32768,
                ApiProvider::HuggingFace { .. } => 4096,
                ApiProvider::Custom { .. } => 4096,
            },
            supports_streaming: true,
            supports_functions: matches!(
                self.provider,
                ApiProvider::OpenAI
                    | ApiProvider::Grok
                    | ApiProvider::OpenRouter
                    | ApiProvider::Together
                    | ApiProvider::Anthropic
                    | ApiProvider::Google
                    | ApiProvider::Perplexity
                    | ApiProvider::Ollama
                    | ApiProvider::Custom { .. }
            ),
            is_local: matches!(self.provider, ApiProvider::Ollama),
        }
    }

    async fn is_ready(&self) -> bool {
        true
    }

    fn memory_usage(&self) -> MemoryUsage {
        MemoryUsage {
            ram_mb: 0,
            vram_mb: None,
            model_size_mb: 0,
        }
    }

    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<ChatResponse> {
        match &self.provider {
            ApiProvider::Anthropic => self.anthropic_chat(messages, tools, config).await,
            ApiProvider::Google => self.google_chat(messages, tools, config).await,
            _ => self.openai_chat(messages, tools, config).await,
        }
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatStreamEvent>> {
        match &self.provider {
            ApiProvider::Anthropic => self.anthropic_chat_stream(messages, tools, config).await,
            _ => self.openai_chat_stream(messages, tools, config).await,
        }
    }
}

impl SimpleExternalProvider {
    /// Real SSE streaming for OpenAI-compatible APIs
    async fn openai_stream(&self, prompt: &str, config: &GenerationConfig) -> Result<TokenStream> {
        use futures::StreamExt;

        let request = json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "frequency_penalty": (config.repetition_penalty - 1.0).max(0.0),
            "stream": true
        });

        let endpoint = self.get_endpoint();
        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow!(
                        "Streaming request to {} timed out — check network connectivity",
                        endpoint
                    )
                } else if e.is_connect() {
                    anyhow!("Failed to connect to {} for streaming: {}", endpoint, e)
                } else {
                    anyhow!("Streaming request to {} failed: {}", endpoint, e)
                }
            })?;

        let status = response.status();
        // Check content-type: if the server returned HTML instead of SSE, bail early
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !status.is_success() || content_type.contains("text/html") {
            let error = response.text().await?;
            let preview: String = error.chars().take(300).collect();
            return Err(anyhow!(
                "API streaming error (HTTP {}, content-type: {}): {}",
                status,
                content_type,
                preview
            ));
        }

        let (sender, receiver) = tokio::sync::mpsc::channel::<String>(256);
        let mut byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(_) => break,
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete SSE lines
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data: ") {
                        continue;
                    }

                    let data = &line[6..];
                    if data == "[DONE]" {
                        return;
                    }

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(content) = parsed["choices"][0]["delta"]["content"].as_str() {
                            if !content.is_empty() {
                                if sender.send(content.to_string()).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(TokenStream::new(receiver))
    }

    /// OpenAI-compatible generation
    async fn openai_compatible_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String> {
        let endpoint = self.get_endpoint();
        tracing::debug!(
            endpoint = %endpoint,
            model = %self.model,
            max_tokens = config.max_tokens,
            prompt_len = prompt.len(),
            "Sending OpenAI-compatible request"
        );

        let request = json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "frequency_penalty": (config.repetition_penalty - 1.0).max(0.0),
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
                    tracing::error!(endpoint = %endpoint, "Request timed out (connect or response timeout)");
                    anyhow!("Request to {} timed out — check network connectivity and whether the endpoint is reachable", endpoint)
                } else if e.is_connect() {
                    tracing::error!(endpoint = %endpoint, error = %e, "Connection failed");
                    anyhow!("Failed to connect to {} — check network/firewall/proxy settings: {}", endpoint, e)
                } else {
                    tracing::error!(endpoint = %endpoint, error = %e, "Request failed");
                    anyhow!("Request to {} failed: {}", endpoint, e)
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let error = response.text().await?;
            tracing::error!(endpoint = %endpoint, status = %status, error = %error, "API returned error");
            return Err(anyhow!("API error ({}): {}", status, error));
        }

        let result: OpenAIResponse = Self::parse_json_response(response, &endpoint).await?;

        if result.choices.is_empty() {
            return Err(anyhow!("No choices returned from API"));
        }

        tracing::debug!(
            "API response received, {} chars",
            result.choices[0].message.content.len()
        );
        Ok(result.choices[0].message.content.clone())
    }

    /// Anthropic generation
    async fn anthropic_generate(&self, prompt: &str, config: &GenerationConfig) -> Result<String> {
        let request = json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p
        });

        let response = self
            .client
            .post(self.get_endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Anthropic API error ({}): {}", status, error));
        }

        let endpoint = self.get_endpoint();
        let result: AnthropicResponse = Self::parse_json_response(response, &endpoint).await?;

        if result.content.is_empty() {
            return Err(anyhow!("No content returned from Anthropic API"));
        }

        Ok(result.content[0].text.clone())
    }

    /// Google Gemini generation
    async fn google_generate(&self, prompt: &str, config: &GenerationConfig) -> Result<String> {
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

        let response = self
            .client
            .post(self.get_endpoint())
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Google API error ({}): {}", status, error));
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

    /// HuggingFace generation
    async fn huggingface_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
        _model_id: &str,
    ) -> Result<String> {
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

        let response = self
            .client
            .post(self.get_endpoint())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error = response.text().await?;
            return Err(anyhow!("HuggingFace API error ({}): {}", status, error));
        }

        let endpoint = self.get_endpoint();
        let result: Vec<HuggingFaceResponse> =
            Self::parse_json_response(response, &endpoint).await?;

        if result.is_empty() {
            return Err(anyhow!("No response returned from HuggingFace API"));
        }

        Ok(result[0].generated_text.clone())
    }

    // ==================== Tool-calling: OpenAI-compatible ====================

    fn format_openai_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    ChatRole::System => "system",
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                    ChatRole::Tool => "tool",
                };
                let mut msg = json!({ "role": role });
                if let Some(ref content) = m.content {
                    msg["content"] = json!(content);
                }
                if let Some(ref calls) = m.tool_calls {
                    msg["tool_calls"] = json!(calls
                        .iter()
                        .map(|tc| json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments,
                            }
                        }))
                        .collect::<Vec<_>>());
                }
                if let Some(ref id) = m.tool_call_id {
                    msg["tool_call_id"] = json!(id);
                }
                if let Some(ref name) = m.name {
                    msg["name"] = json!(name);
                }
                msg
            })
            .collect()
    }

    fn format_openai_tools(tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }

    /// Non-streaming chat completion with tool calling (OpenAI-compatible).
    async fn openai_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<ChatResponse> {
        let mut request = json!({
            "model": self.model,
            "messages": Self::format_openai_messages(messages),
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "frequency_penalty": (config.repetition_penalty - 1.0).max(0.0),
            "stream": false
        });

        if !tools.is_empty() {
            request["tools"] = json!(Self::format_openai_tools(tools));
            request["tool_choice"] = json!("auto");
        }

        let endpoint = self.get_endpoint();
        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow!(
                        "Chat request to {} timed out — check network connectivity",
                        endpoint
                    )
                } else if e.is_connect() {
                    anyhow!(
                        "Failed to connect to {} — check network/firewall/proxy: {}",
                        endpoint,
                        e
                    )
                } else {
                    anyhow!("Chat request to {} failed: {}", endpoint, e)
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Chat API error ({}): {}", status, error));
        }

        let body: serde_json::Value = Self::parse_json_response(response, &endpoint).await?;
        let choice = &body["choices"][0]["message"];

        // Check for tool calls
        if let Some(tool_calls) = choice["tool_calls"].as_array() {
            let calls: Vec<ToolCall> = tool_calls
                .iter()
                .filter_map(|tc| {
                    Some(ToolCall {
                        id: tc["id"].as_str()?.to_string(),
                        name: tc["function"]["name"].as_str()?.to_string(),
                        arguments: tc["function"]["arguments"].as_str()?.to_string(),
                    })
                })
                .collect();
            if !calls.is_empty() {
                return Ok(ChatResponse::ToolCalls(calls));
            }
        }

        // Regular content
        let content = choice["content"].as_str().unwrap_or("").to_string();
        Ok(ChatResponse::Content(content))
    }

    /// Streaming chat completion with tool calling (OpenAI-compatible SSE).
    async fn openai_chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatStreamEvent>> {
        use futures::StreamExt;

        let mut request = json!({
            "model": self.model,
            "messages": Self::format_openai_messages(messages),
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "frequency_penalty": (config.repetition_penalty - 1.0).max(0.0),
            "stream": true
        });

        if !tools.is_empty() {
            request["tools"] = json!(Self::format_openai_tools(tools));
            request["tool_choice"] = json!("auto");
        }

        let endpoint = self.get_endpoint();
        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow!(
                        "Chat stream to {} timed out — check network connectivity",
                        endpoint
                    )
                } else if e.is_connect() {
                    anyhow!("Failed to connect to {} for chat stream: {}", endpoint, e)
                } else {
                    anyhow!("Chat stream request to {} failed: {}", endpoint, e)
                }
            })?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !status.is_success() || content_type.contains("text/html") {
            let error = response.text().await?;
            let preview: String = error.chars().take(300).collect();
            return Err(anyhow!(
                "Chat streaming error (HTTP {}, content-type: {}): {}",
                status,
                content_type,
                preview
            ));
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<ChatStreamEvent>(256);
        let mut byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut buffer = String::new();
            // Accumulate streamed tool calls: index -> (id, name, arguments_buffer)
            let mut tool_call_acc: std::collections::HashMap<u64, (String, String, String)> =
                std::collections::HashMap::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(_) => break,
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data: ") {
                        continue;
                    }
                    let data = &line[6..];
                    if data == "[DONE]" {
                        // Flush any accumulated tool calls
                        let mut calls: Vec<(u64, ToolCall)> = tool_call_acc
                            .drain()
                            .map(|(idx, (id, name, args))| {
                                (
                                    idx,
                                    ToolCall {
                                        id,
                                        name,
                                        arguments: args,
                                    },
                                )
                            })
                            .collect();
                        calls.sort_by_key(|(idx, _)| *idx);
                        for (_, tc) in calls {
                            let _ = tx.send(ChatStreamEvent::ToolCallComplete(tc)).await;
                        }
                        let _ = tx.send(ChatStreamEvent::Done).await;
                        return;
                    }

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        let delta = &parsed["choices"][0]["delta"];

                        // Content delta
                        if let Some(content) = delta["content"].as_str() {
                            if !content.is_empty() {
                                if tx
                                    .send(ChatStreamEvent::ContentDelta(content.to_string()))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }

                        // Tool call deltas
                        if let Some(tcs) = delta["tool_calls"].as_array() {
                            for tc_delta in tcs {
                                let idx = tc_delta["index"].as_u64().unwrap_or(0);
                                let entry = tool_call_acc.entry(idx).or_insert_with(|| {
                                    let id = tc_delta["id"].as_str().unwrap_or("").to_string();
                                    let name = tc_delta["function"]["name"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    (id, name, String::new())
                                });
                                // Accumulate id/name if provided in later deltas
                                if let Some(id) = tc_delta["id"].as_str() {
                                    if !id.is_empty() {
                                        entry.0 = id.to_string();
                                    }
                                }
                                if let Some(name) = tc_delta["function"]["name"].as_str() {
                                    if !name.is_empty() {
                                        entry.1 = name.to_string();
                                    }
                                }
                                if let Some(args) = tc_delta["function"]["arguments"].as_str() {
                                    entry.2.push_str(args);
                                }
                            }
                        }
                    }
                }
            }

            // Stream ended without [DONE] — flush accumulated tool calls
            if !tool_call_acc.is_empty() {
                let mut calls: Vec<(u64, ToolCall)> = tool_call_acc
                    .drain()
                    .map(|(idx, (id, name, args))| {
                        (
                            idx,
                            ToolCall {
                                id,
                                name,
                                arguments: args,
                            },
                        )
                    })
                    .collect();
                calls.sort_by_key(|(idx, _)| *idx);
                for (_, tc) in calls {
                    let _ = tx.send(ChatStreamEvent::ToolCallComplete(tc)).await;
                }
            }
            let _ = tx.send(ChatStreamEvent::Done).await;
        });

        Ok(rx)
    }

    // ==================== Tool-calling: Anthropic ====================

    fn format_anthropic_messages(
        messages: &[ChatMessage],
    ) -> (Option<String>, Vec<serde_json::Value>) {
        let mut system_prompt = None;
        let mut api_messages = Vec::new();

        for m in messages {
            match m.role {
                ChatRole::System => {
                    system_prompt = m.content.clone();
                }
                ChatRole::User => {
                    if let Some(ref content) = m.content {
                        api_messages.push(json!({
                            "role": "user",
                            "content": content,
                        }));
                    }
                }
                ChatRole::Assistant => {
                    if let Some(ref calls) = m.tool_calls {
                        // Anthropic: assistant message with tool_use content blocks
                        let content: Vec<serde_json::Value> = calls
                            .iter()
                            .map(|tc| {
                                let args: serde_json::Value =
                                    serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                                json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.name,
                                    "input": args,
                                })
                            })
                            .collect();
                        api_messages.push(json!({
                            "role": "assistant",
                            "content": content,
                        }));
                    } else if let Some(ref content) = m.content {
                        api_messages.push(json!({
                            "role": "assistant",
                            "content": content,
                        }));
                    }
                }
                ChatRole::Tool => {
                    // Anthropic: tool results are user messages with tool_result content blocks
                    if let (Some(ref id), Some(ref content)) = (&m.tool_call_id, &m.content) {
                        api_messages.push(json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": id,
                                "content": content,
                            }]
                        }));
                    }
                }
            }
        }
        (system_prompt, api_messages)
    }

    fn format_anthropic_tools(tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect()
    }

    async fn anthropic_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<ChatResponse> {
        let (system_prompt, api_messages) = Self::format_anthropic_messages(messages);

        let mut request = json!({
            "model": self.model,
            "messages": api_messages,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p
        });

        if let Some(ref sys) = system_prompt {
            request["system"] = json!(sys);
        }
        if !tools.is_empty() {
            request["tools"] = json!(Self::format_anthropic_tools(tools));
        }

        let response = self
            .client
            .post(self.get_endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Anthropic chat error ({}): {}", status, error));
        }

        let endpoint = self.get_endpoint();
        let body: serde_json::Value = Self::parse_json_response(response, &endpoint).await?;

        // Parse Anthropic response content blocks
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        if let Some(content) = body["content"].as_array() {
            for block in content {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"].as_str() {
                            text_parts.push(text.to_string());
                        }
                    }
                    Some("tool_use") => {
                        if let (Some(id), Some(name)) =
                            (block["id"].as_str(), block["name"].as_str())
                        {
                            let args = serde_json::to_string(&block["input"])
                                .unwrap_or_else(|_| "{}".to_string());
                            tool_calls.push(ToolCall {
                                id: id.to_string(),
                                name: name.to_string(),
                                arguments: args,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        if !tool_calls.is_empty() {
            Ok(ChatResponse::ToolCalls(tool_calls))
        } else {
            Ok(ChatResponse::Content(text_parts.join("")))
        }
    }

    async fn anthropic_chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatStreamEvent>> {
        use futures::StreamExt;

        let (system_prompt, api_messages) = Self::format_anthropic_messages(messages);

        let mut request = json!({
            "model": self.model,
            "messages": api_messages,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "stream": true
        });

        if let Some(ref sys) = system_prompt {
            request["system"] = json!(sys);
        }
        if !tools.is_empty() {
            request["tools"] = json!(Self::format_anthropic_tools(tools));
        }

        let response = self
            .client
            .post(self.get_endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !status.is_success() || content_type.contains("text/html") {
            let error = response.text().await?;
            let preview: String = error.chars().take(300).collect();
            return Err(anyhow!(
                "Anthropic streaming error (HTTP {}, content-type: {}): {}",
                status,
                content_type,
                preview
            ));
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<ChatStreamEvent>(256);
        let mut byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut buffer = String::new();
            // Track current tool use block being streamed
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_args = String::new();
            let mut in_tool_use = false;

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(_) => break,
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data: ") {
                        continue;
                    }
                    let data = &line[6..];

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        match parsed["type"].as_str() {
                            Some("content_block_start") => {
                                let block = &parsed["content_block"];
                                if block["type"].as_str() == Some("tool_use") {
                                    in_tool_use = true;
                                    current_tool_id =
                                        block["id"].as_str().unwrap_or("").to_string();
                                    current_tool_name =
                                        block["name"].as_str().unwrap_or("").to_string();
                                    current_tool_args.clear();
                                }
                            }
                            Some("content_block_delta") => {
                                let delta = &parsed["delta"];
                                match delta["type"].as_str() {
                                    Some("text_delta") => {
                                        if let Some(text) = delta["text"].as_str() {
                                            if !text.is_empty() {
                                                if tx
                                                    .send(ChatStreamEvent::ContentDelta(
                                                        text.to_string(),
                                                    ))
                                                    .await
                                                    .is_err()
                                                {
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    Some("input_json_delta") => {
                                        if let Some(partial) = delta["partial_json"].as_str() {
                                            current_tool_args.push_str(partial);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Some("content_block_stop") => {
                                if in_tool_use {
                                    let tc = ToolCall {
                                        id: std::mem::take(&mut current_tool_id),
                                        name: std::mem::take(&mut current_tool_name),
                                        arguments: std::mem::take(&mut current_tool_args),
                                    };
                                    let _ = tx.send(ChatStreamEvent::ToolCallComplete(tc)).await;
                                    in_tool_use = false;
                                }
                            }
                            Some("message_stop") => {
                                let _ = tx.send(ChatStreamEvent::Done).await;
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }
            let _ = tx.send(ChatStreamEvent::Done).await;
        });

        Ok(rx)
    }

    // ==================== Tool-calling: Google Gemini ====================

    async fn google_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<ChatResponse> {
        let mut contents = Vec::new();
        let mut system_instruction = None;

        for m in messages {
            match m.role {
                ChatRole::System => {
                    system_instruction = m.content.clone();
                }
                ChatRole::User => {
                    if let Some(ref content) = m.content {
                        contents.push(json!({
                            "role": "user",
                            "parts": [{ "text": content }]
                        }));
                    }
                }
                ChatRole::Assistant => {
                    if let Some(ref calls) = m.tool_calls {
                        let parts: Vec<serde_json::Value> = calls
                            .iter()
                            .map(|tc| {
                                let args: serde_json::Value =
                                    serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                                json!({
                                    "functionCall": {
                                        "name": tc.name,
                                        "args": args,
                                    }
                                })
                            })
                            .collect();
                        contents.push(json!({ "role": "model", "parts": parts }));
                    } else if let Some(ref content) = m.content {
                        contents.push(json!({
                            "role": "model",
                            "parts": [{ "text": content }]
                        }));
                    }
                }
                ChatRole::Tool => {
                    if let (Some(ref name), Some(ref content)) = (&m.name, &m.content) {
                        let result: serde_json::Value =
                            serde_json::from_str(content).unwrap_or(json!({ "result": content }));
                        contents.push(json!({
                            "role": "user",
                            "parts": [{
                                "functionResponse": {
                                    "name": name,
                                    "response": result,
                                }
                            }]
                        }));
                    }
                }
            }
        }

        let mut request = json!({
            "contents": contents,
            "generationConfig": {
                "temperature": config.temperature,
                "topP": config.top_p,
                "topK": config.top_k,
                "maxOutputTokens": config.max_tokens,
            }
        });

        if let Some(ref sys) = system_instruction {
            request["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
        }

        if !tools.is_empty() {
            let functions: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            request["tools"] = json!([{ "functionDeclarations": functions }]);
        }

        let response = self
            .client
            .post(self.get_endpoint())
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error = response.text().await?;
            return Err(anyhow!("Google chat error ({}): {}", status, error));
        }

        let endpoint = self.get_endpoint();
        let body: serde_json::Value = Self::parse_json_response(response, &endpoint).await?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        if let Some(parts) = body["candidates"][0]["content"]["parts"].as_array() {
            for part in parts {
                if let Some(text) = part["text"].as_str() {
                    text_parts.push(text.to_string());
                }
                if let Some(fc) = part.get("functionCall") {
                    if let Some(name) = fc["name"].as_str() {
                        let args =
                            serde_json::to_string(&fc["args"]).unwrap_or_else(|_| "{}".to_string());
                        tool_calls.push(ToolCall {
                            id: format!(
                                "call_{}",
                                uuid::Uuid::new_v4()
                                    .to_string()
                                    .split('-')
                                    .next()
                                    .unwrap_or("0")
                            ),
                            name: name.to_string(),
                            arguments: args,
                        });
                    }
                }
            }
        }

        if !tool_calls.is_empty() {
            Ok(ChatResponse::ToolCalls(tool_calls))
        } else {
            Ok(ChatResponse::Content(text_parts.join("")))
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
