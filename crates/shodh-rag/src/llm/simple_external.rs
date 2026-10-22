//! Simplified external API provider implementation with working streaming
//! Production-grade implementation for OpenAI, Anthropic, and other LLM APIs

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use crate::harness::provider_error::ProviderErrorKind;

use super::failure::{pump, AnthropicSse, CallContext, OpenAiSse, ProviderFailure, StreamItem};
use super::{
    ApiProvider, ChatMessage, ChatResponse, ChatRole, ChatStreamEvent, GenerationConfig,
    LLMProvider, MemoryUsage, ProviderInfo, SubscriptionService, TokenStream, ToolCall, ToolSchema,
    LM_STUDIO_DEFAULT_BASE_URL,
};

/// Why a subscription model cannot answer outside the agent.
fn subscription_only(service: SubscriptionService) -> anyhow::Error {
    anyhow!(
        "{} answers through the assistant in Ask only. Choose a model with an API key or a local model for this feature.",
        service.label()
    )
}

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
            ApiProvider::LmStudio => format!("{LM_STUDIO_DEFAULT_BASE_URL}/chat/completions"),
            // Never called: `generate` refuses subscriptions before any request.
            ApiProvider::Subscription(_) => String::new(),
            ApiProvider::HuggingFace { model_id } => {
                format!("https://api-inference.huggingface.co/models/{}", model_id)
            }
            ApiProvider::Custom { endpoint } => endpoint.clone(),
        }
    }

    fn provider_name(&self) -> &'static str {
        match &self.provider {
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
            ApiProvider::LmStudio => "LM Studio",
            ApiProvider::Subscription(service) => service.label(),
            ApiProvider::HuggingFace { .. } => "HuggingFace",
            ApiProvider::Custom { .. } => "Custom",
        }
    }

    /// Who this call goes to, for its failures (which never carry the key).
    fn call(&self) -> CallContext {
        CallContext::new(
            self.provider_name(),
            self.model.clone(),
            self.api_key.clone(),
        )
    }

    /// Send `request`: a provider failure when it got no answer or an
    /// error status (classified from the status and body).
    async fn send_checked(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let call = self.call();
        let response = request
            .send()
            .await
            .map_err(|e| failed(call.transport(&e)))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(failed(call.http(status.as_u16(), &body)));
        }
        Ok(response)
    }

    /// [`Self::send_checked`] for a stream: also a failure when the server
    /// answered with a web page instead of an event stream.
    async fn send_stream(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let response = self.send_checked(request).await?;
        let html = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.contains("text/html"));
        if html {
            let status = response.status().as_u16();
            return Err(failed(self.call().failure(
                ProviderErrorKind::ModelUnavailable,
                Some(status),
                "The provider answered with a web page instead of a stream",
            )));
        }
        Ok(response)
    }

    /// A `200 OK` JSON body, or the error it carries.
    async fn checked_body(
        &self,
        response: reqwest::Response,
        endpoint: &str,
    ) -> Result<serde_json::Value> {
        let body: serde_json::Value = Self::parse_json_response(response, endpoint).await?;
        match self.call().body_error(&body) {
            Some(failure) => Err(failed(failure)),
            None => Ok(body),
        }
    }
}

/// A completion, and whether the provider stopped it at the token limit (so the text is
/// cut off: a JSON answer is incomplete).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedCompletion {
    /// The text written (empty when the whole budget went to hidden reasoning).
    pub text: String,
    /// The provider reported that it stopped at the token limit.
    pub truncated: bool,
}

impl SimpleExternalProvider {
    /// Completes `prompt` like [`LLMProvider::generate`], and reports whether the answer
    /// was cut off at `config.max_tokens` (OpenAI-compatible APIs, Anthropic and Gemini
    /// say so; other providers are reported as not cut off).
    pub async fn generate_bounded(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<BoundedCompletion> {
        match &self.provider {
            ApiProvider::OpenAI
            | ApiProvider::OpenRouter
            | ApiProvider::Together
            | ApiProvider::Grok
            | ApiProvider::Perplexity
            | ApiProvider::Baseten
            | ApiProvider::Ollama
            | ApiProvider::LmStudio
            | ApiProvider::Custom { .. } => self.openai_compatible_complete(prompt, config).await,
            ApiProvider::Anthropic => self.anthropic_complete(prompt, config).await,
            ApiProvider::Google => self.google_complete(prompt, config).await,
            ApiProvider::Subscription(_)
            | ApiProvider::HuggingFace { .. }
            | ApiProvider::Replicate => Ok(BoundedCompletion {
                text: LLMProvider::generate(self, prompt, config).await?,
                truncated: false,
            }),
        }
    }
}

/// A completion of `text`; `None` when it is empty although the provider did not stop it
/// at the token limit (an empty answer). Read the stop reason before the empty check: a
/// reasoning model can spend its whole budget thinking and stop with no text at all.
fn bounded(text: String, truncated: bool) -> Option<BoundedCompletion> {
    (truncated || !text.trim().is_empty()).then_some(BoundedCompletion { text, truncated })
}

/// The completion in an OpenAI-compatible body (`finish_reason: "length"` is cut off).
fn openai_completion(body: &serde_json::Value) -> Option<BoundedCompletion> {
    let choice = &body["choices"][0];
    bounded(
        choice["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        choice["finish_reason"].as_str() == Some("length"),
    )
}

/// The completion in an Anthropic answer (`stop_reason: "max_tokens"` is cut off).
fn anthropic_completion(result: &AnthropicResponse) -> Option<BoundedCompletion> {
    bounded(
        result
            .content
            .first()
            .map(|block| block.text.clone())
            .unwrap_or_default(),
        result.stop_reason.as_deref() == Some("max_tokens"),
    )
}

/// The completion in a Gemini answer (`finishReason: "MAX_TOKENS"` is cut off).
fn google_completion(result: &GoogleResponse) -> Option<BoundedCompletion> {
    let candidate = result.candidates.first();
    bounded(
        candidate
            .and_then(|c| c.content.as_ref())
            .and_then(|c| c.parts.first())
            .map(|p| p.text.clone())
            .unwrap_or_default(),
        candidate.and_then(|c| c.finish_reason.as_deref()) == Some("MAX_TOKENS"),
    )
}

/// Log a provider failure and make it the call's error (callers can
/// `downcast_ref::<ProviderFailure>()`).
fn failed(failure: ProviderFailure) -> anyhow::Error {
    failure.log();
    anyhow::Error::new(failure)
}

/// Token stream items: text, or the failure that ends the stream.
fn token_item(item: StreamItem) -> Option<std::result::Result<String, ProviderFailure>> {
    match item {
        StreamItem::Content(text) => Some(Ok(text)),
        StreamItem::Failed(failure) => Some(Err(failure)),
        StreamItem::ToolCall(_) | StreamItem::Done => None,
    }
}

/// Chat stream events.
fn chat_event(item: StreamItem) -> Option<ChatStreamEvent> {
    Some(match item {
        StreamItem::Content(text) => ChatStreamEvent::ContentDelta(text),
        StreamItem::ToolCall(call) => ChatStreamEvent::ToolCallComplete(call),
        StreamItem::Done => ChatStreamEvent::Done,
        StreamItem::Failed(failure) => ChatStreamEvent::Error(failure),
    })
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
            | ApiProvider::Ollama
            | ApiProvider::LmStudio => self.openai_compatible_generate(prompt, config).await,
            ApiProvider::Subscription(service) => Err(subscription_only(*service)),
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
            | ApiProvider::LmStudio
            | ApiProvider::Custom { .. } => self.openai_stream(prompt, config).await,
            ApiProvider::Subscription(service) => Err(subscription_only(*service)),
            _ => {
                // Providers without SSE: fall back to chunked non-streaming.
                // Send word-by-word to simulate streaming (safe for any UTF-8).
                let response = self.generate(prompt, config).await?;
                let (sender, receiver) = tokio::sync::mpsc::channel(256);
                tokio::spawn(async move {
                    // Split on whitespace boundaries to avoid breaking UTF-8 chars
                    let chars = response.chars();
                    let mut chunk = String::with_capacity(40);
                    for c in chars {
                        chunk.push(c);
                        // Flush at word boundaries (~30 chars per chunk)
                        if chunk.len() >= 30
                            && (c == ' ' || c == '\n')
                            && sender.send(Ok(std::mem::take(&mut chunk))).await.is_err()
                        {
                            break;
                        }
                    }
                    // Flush remainder
                    if !chunk.is_empty() {
                        let _ = sender.send(Ok(chunk)).await;
                    }
                });
                Ok(TokenStream::new(receiver))
            }
        }
    }

    fn info(&self) -> ProviderInfo {
        let provider_name = self.provider_name();

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
                ApiProvider::Ollama | ApiProvider::LmStudio => 32768,
                ApiProvider::Subscription(_) => 200000,
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
                    | ApiProvider::LmStudio
                    | ApiProvider::Custom { .. }
            ),
            is_local: matches!(self.provider, ApiProvider::Ollama | ApiProvider::LmStudio),
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

        let response = self
            .send_stream(
                self.client
                    .post(self.get_endpoint())
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&request),
            )
            .await?;

        let (sender, receiver) = tokio::sync::mpsc::channel(256);
        tokio::spawn(pump(
            response,
            OpenAiSse::new(self.call()),
            sender,
            token_item,
        ));
        Ok(TokenStream::new(receiver))
    }

    /// OpenAI-compatible generation
    async fn openai_compatible_generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String> {
        let completion = self.openai_compatible_complete(prompt, config).await?;
        if completion.text.trim().is_empty() {
            return Err(failed(self.call().empty()));
        }
        Ok(completion.text)
    }

    /// The body of an OpenAI-compatible completion request.
    fn openai_request(&self, prompt: &str, config: &GenerationConfig) -> serde_json::Value {
        let mut request = json!({
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
        if config.minimal_reasoning && matches!(self.provider, ApiProvider::OpenRouter) {
            // OpenRouter normalises the effort across providers and ignores it for models
            // that do not reason; OpenAI's own `reasoning_effort` is refused by those.
            request["reasoning"] = json!({ "effort": "minimal" });
        }
        request
    }

    /// One OpenAI-compatible completion and whether it stopped at `max_tokens`
    /// (`finish_reason: "length"`).
    async fn openai_compatible_complete(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<BoundedCompletion> {
        let endpoint = self.get_endpoint();
        tracing::debug!(
            endpoint = %endpoint,
            model = %self.model,
            max_tokens = config.max_tokens,
            prompt_len = prompt.len(),
            "Sending OpenAI-compatible request"
        );

        let request = self.openai_request(prompt, config);

        let response = self
            .send_checked(
                self.client
                    .post(&endpoint)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&request),
            )
            .await?;
        let body = self.checked_body(response, &endpoint).await?;
        let completion = openai_completion(&body).ok_or_else(|| failed(self.call().empty()))?;
        tracing::debug!("API response received, {} chars", completion.text.len());
        Ok(completion)
    }

    /// Anthropic generation
    async fn anthropic_generate(&self, prompt: &str, config: &GenerationConfig) -> Result<String> {
        let completion = self.anthropic_complete(prompt, config).await?;
        if completion.text.trim().is_empty() {
            return Err(failed(self.call().empty()));
        }
        Ok(completion.text)
    }

    /// One Anthropic completion and whether it stopped at `max_tokens`
    /// (`stop_reason: "max_tokens"`).
    async fn anthropic_complete(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<BoundedCompletion> {
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
            .send_checked(
                self.client
                    .post(self.get_endpoint())
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(&request),
            )
            .await?;

        let endpoint = self.get_endpoint();
        let result: AnthropicResponse = Self::parse_json_response(response, &endpoint).await?;
        anthropic_completion(&result).ok_or_else(|| failed(self.call().empty()))
    }

    /// Google Gemini generation
    async fn google_generate(&self, prompt: &str, config: &GenerationConfig) -> Result<String> {
        let completion = self.google_complete(prompt, config).await?;
        if completion.text.trim().is_empty() {
            return Err(failed(self.call().empty()));
        }
        Ok(completion.text)
    }

    /// One Gemini completion and whether it stopped at `maxOutputTokens`
    /// (`finishReason: "MAX_TOKENS"`).
    async fn google_complete(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<BoundedCompletion> {
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
            .send_checked(
                self.client
                    .post(self.get_endpoint())
                    .header("Content-Type", "application/json")
                    .header("x-goog-api-key", &self.api_key)
                    .json(&request),
            )
            .await?;

        let endpoint = self.get_endpoint();
        let result: GoogleResponse = Self::parse_json_response(response, &endpoint).await?;
        google_completion(&result).ok_or_else(|| failed(self.call().empty()))
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
            .send_checked(
                self.client
                    .post(self.get_endpoint())
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&request),
            )
            .await?;

        let endpoint = self.get_endpoint();
        let result: Vec<HuggingFaceResponse> =
            Self::parse_json_response(response, &endpoint).await?;

        match result.first() {
            Some(r) if !r.generated_text.trim().is_empty() => Ok(r.generated_text.clone()),
            _ => Err(failed(self.call().empty())),
        }
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
            .send_checked(
                self.client
                    .post(&endpoint)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&request),
            )
            .await?;

        let body = self.checked_body(response, &endpoint).await?;
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
        let content = choice["content"].as_str().unwrap_or("");
        if content.trim().is_empty() {
            return Err(failed(self.call().empty()));
        }
        Ok(ChatResponse::Content(content.to_string()))
    }

    /// Streaming chat completion with tool calling (OpenAI-compatible SSE).
    async fn openai_chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatStreamEvent>> {
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

        let response = self
            .send_stream(
                self.client
                    .post(self.get_endpoint())
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&request),
            )
            .await?;

        let (tx, rx) = tokio::sync::mpsc::channel::<ChatStreamEvent>(256);
        tokio::spawn(pump(response, OpenAiSse::new(self.call()), tx, chat_event));
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
            .send_checked(
                self.client
                    .post(self.get_endpoint())
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(&request),
            )
            .await?;

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

        let text = text_parts.join("");
        if !tool_calls.is_empty() {
            Ok(ChatResponse::ToolCalls(tool_calls))
        } else if text.trim().is_empty() {
            Err(failed(self.call().empty()))
        } else {
            Ok(ChatResponse::Content(text))
        }
    }

    async fn anthropic_chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatStreamEvent>> {
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
            .send_stream(
                self.client
                    .post(self.get_endpoint())
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(&request),
            )
            .await?;

        let (tx, rx) = tokio::sync::mpsc::channel::<ChatStreamEvent>(256);
        tokio::spawn(pump(
            response,
            AnthropicSse::new(self.call()),
            tx,
            chat_event,
        ));
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
            .send_checked(
                self.client
                    .post(self.get_endpoint())
                    .header("Content-Type", "application/json")
                    .header("x-goog-api-key", &self.api_key)
                    .json(&request),
            )
            .await?;

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

        let text = text_parts.join("");
        if !tool_calls.is_empty() {
            Ok(ChatResponse::ToolCalls(tool_calls))
        } else if text.trim().is_empty() {
            Err(failed(self.call().empty()))
        } else {
            Ok(ChatResponse::Content(text))
        }
    }
}

/// Response structures
#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    #[serde(default)]
    stop_reason: Option<String>,
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
    /// Missing when the candidate stopped before writing anything (at the token limit).
    #[serde(default)]
    content: Option<GoogleContent>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(minimal_reasoning: bool) -> GenerationConfig {
        GenerationConfig {
            max_tokens: 300,
            temperature: 0.0,
            top_p: 1.0,
            top_k: 40,
            repetition_penalty: 1.0,
            stop_sequences: vec![],
            seed: None,
            minimal_reasoning,
        }
    }

    #[test]
    fn minimal_reasoning_is_asked_of_openrouter_only() {
        let ask = |provider: ApiProvider, minimal: bool| {
            SimpleExternalProvider::new(provider, "key".into(), "model".into())
                .unwrap()
                .openai_request("prompt", &config(minimal))
        };
        let routed = ask(ApiProvider::OpenRouter, true);
        assert_eq!(routed["reasoning"]["effort"], "minimal");
        assert_eq!(routed["max_tokens"], 300);
        assert!(ask(ApiProvider::OpenRouter, false)
            .get("reasoning")
            .is_none());
        // Other OpenAI-compatible endpoints refuse a reasoning setting on models that
        // have none, so it is not sent to them.
        assert!(ask(ApiProvider::OpenAI, true).get("reasoning").is_none());
        assert!(ask(ApiProvider::OpenAI, true)
            .get("reasoning_effort")
            .is_none());
    }

    #[test]
    fn a_completion_stopped_at_the_token_limit_is_reported_as_cut_off() {
        let cut = openai_completion(&json!({"choices": [{
            "message": {"content": "{\"memories\": [{\"turn\": \"T1\""},
            "finish_reason": "length"
        }]}))
        .unwrap();
        assert!(cut.truncated);
        assert!(cut.text.starts_with("{\"memories\""));
        // All of the budget went to hidden reasoning: no text, still cut off (not empty).
        let thought = openai_completion(&json!({"choices": [{
            "message": {"content": null}, "finish_reason": "length"
        }]}))
        .unwrap();
        assert!(thought.truncated && thought.text.is_empty());
        let done = openai_completion(&json!({"choices": [{
            "message": {"content": "{\"memories\": []}"}, "finish_reason": "stop"
        }]}))
        .unwrap();
        assert!(!done.truncated);
        assert!(openai_completion(&json!({"choices": [{
            "message": {"content": "  "}, "finish_reason": "stop"
        }]}))
        .is_none());
    }

    #[test]
    fn anthropic_and_gemini_stop_reasons_are_read() {
        let anthropic: AnthropicResponse = serde_json::from_value(json!({
            "content": [{"type": "text", "text": "{\"memories\": ["}],
            "stop_reason": "max_tokens"
        }))
        .unwrap();
        assert!(anthropic_completion(&anthropic).unwrap().truncated);
        let anthropic: AnthropicResponse = serde_json::from_value(json!({
            "content": [{"type": "text", "text": "{}"}], "stop_reason": "end_turn"
        }))
        .unwrap();
        assert!(!anthropic_completion(&anthropic).unwrap().truncated);
        let gemini: GoogleResponse = serde_json::from_value(json!({
            "candidates": [{"finishReason": "MAX_TOKENS"}]
        }))
        .unwrap();
        let cut = google_completion(&gemini).unwrap();
        assert!(cut.truncated && cut.text.is_empty());
        let gemini: GoogleResponse = serde_json::from_value(json!({
            "candidates": [{"content": {"parts": [{"text": "{}"}]}, "finishReason": "STOP"}]
        }))
        .unwrap();
        assert!(!google_completion(&gemini).unwrap().truncated);
    }
}
