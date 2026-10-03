//! LLM module: local GGUF inference (llama.cpp) and external API providers.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::fmt;
use std::path::PathBuf;
use tokio::sync::mpsc;

pub mod external;
pub mod llamacpp_provider; // llama.cpp provider (CPU, GGUF models)
pub mod simple_external;
pub mod streaming;

pub use external::ExternalProvider;
pub use llamacpp_provider::LlamaCppProvider;
pub use simple_external::SimpleExternalProvider;
pub use streaming::{StreamingResponse, TokenStream};

/// LLM operation mode
#[derive(Clone, Serialize, Deserialize)]
pub enum LLMMode {
    /// Local GGUF model run in-process by llama.cpp.
    Local { model_path: PathBuf },
    /// External API provider
    External {
        provider: ApiProvider,
        /// Never serialized: keys live only in memory and the OS keychain.
        #[serde(skip_serializing, default)]
        api_key: String,
        model: String,
    },
    /// LLM disabled, RAG-only mode
    Disabled,
}

impl LLMMode {
    /// Stable mode name for the UI and logs: `local`, `external` or `disabled`.
    pub fn kind(&self) -> &'static str {
        match self {
            LLMMode::Local { .. } => "local",
            LLMMode::External { .. } => "external",
            LLMMode::Disabled => "disabled",
        }
    }
}

/// Redacts the API key: `LLMMode` is logged and shown in diagnostics.
impl fmt::Debug for LLMMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LLMMode::Local { model_path } => f
                .debug_struct("Local")
                .field("model_path", model_path)
                .finish(),
            LLMMode::External {
                provider, model, ..
            } => f
                .debug_struct("External")
                .field("provider", provider)
                .field("api_key", &"[REDACTED]")
                .field("model", model)
                .finish(),
            LLMMode::Disabled => f.write_str("Disabled"),
        }
    }
}

/// External API providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApiProvider {
    OpenAI,
    Anthropic,
    OpenRouter,
    Together,
    Grok,
    Perplexity,
    Google,
    Replicate,
    Baseten,
    Ollama,
    HuggingFace { model_id: String },
    Custom { endpoint: String },
}

/// LLM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMConfig {
    pub mode: LLMMode,
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: usize,
    pub repetition_penalty: f32,
    pub streaming: bool,
    pub context_window: usize,
    pub system_prompt: Option<String>,
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            mode: LLMMode::Disabled,
            max_tokens: 8192,
            temperature: 0.7,
            top_p: 0.95,
            top_k: 40,
            repetition_penalty: 1.1,
            streaming: true,
            context_window: 8192,
            system_prompt: None,
        }
    }
}

/// Core trait for LLM providers
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Generate a completion
    async fn generate(&self, prompt: &str, config: &GenerationConfig) -> Result<String>;

    /// Generate with streaming
    async fn generate_stream(&self, prompt: &str, config: &GenerationConfig)
        -> Result<TokenStream>;

    /// Generate with RAG context
    async fn generate_with_context(
        &self,
        query: &str,
        context: Vec<String>,
        config: &GenerationConfig,
    ) -> Result<String>;

    /// Chat completion with full message history and optional tool schemas.
    /// Returns ChatResponse::Content or ChatResponse::ToolCalls.
    /// Default implementation ignores tools and falls back to generate().
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<ChatResponse> {
        // Default: flatten messages to a single prompt and call generate()
        let prompt = messages
            .iter()
            .filter_map(|m| m.content.as_ref().map(|c| format!("{:?}: {}", m.role, c)))
            .collect::<Vec<_>>()
            .join("\n");
        let text = self.generate(&prompt, config).await?;
        Ok(ChatResponse::Content(text))
    }

    /// Streaming chat completion with tool support.
    /// Returns a channel that yields ChatStreamEvent items.
    /// Default implementation falls back to generate_stream().
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
        config: &GenerationConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatStreamEvent>> {
        let prompt = messages
            .iter()
            .filter_map(|m| m.content.as_ref().map(|c| format!("{:?}: {}", m.role, c)))
            .collect::<Vec<_>>()
            .join("\n");
        let mut token_stream = self.generate_stream(&prompt, config).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            while let Some(token) = token_stream.next().await {
                if tx.send(ChatStreamEvent::ContentDelta(token)).await.is_err() {
                    break;
                }
            }
            let _ = tx.send(ChatStreamEvent::Done).await;
        });
        Ok(rx)
    }

    /// Get provider info
    fn info(&self) -> ProviderInfo;

    /// Check if provider is ready
    async fn is_ready(&self) -> bool;

    /// Get memory usage
    fn memory_usage(&self) -> MemoryUsage;
}

/// Generation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: usize,
    pub repetition_penalty: f32,
    pub stop_sequences: Vec<String>,
    pub seed: Option<u64>,
}

impl From<&LLMConfig> for GenerationConfig {
    fn from(config: &LLMConfig) -> Self {
        Self {
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            top_p: config.top_p,
            top_k: config.top_k,
            repetition_penalty: config.repetition_penalty,
            stop_sequences: vec![],
            seed: None,
        }
    }
}

// ==================== Tool Calling Types ====================

/// A chat message with role, content, and optional tool call metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: Option<String>,
    /// Tool calls requested by the assistant (only present when role=Assistant)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// ID of the tool call this message is responding to (only present when role=Tool)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Name of the tool (only present when role=Tool)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: ChatRole::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool call emitted by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool call (used to correlate with tool result)
    pub id: String,
    /// Name of the tool to invoke
    pub name: String,
    /// JSON arguments string
    pub arguments: String,
}

/// Schema describing a tool the LLM can call (OpenAI-compatible format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name (must match what the LLM will emit)
    pub name: String,
    /// Human-readable description for the LLM
    pub description: String,
    /// JSON Schema for the tool's parameters
    pub parameters: JsonValue,
}

/// The result of a chat completion — either text content or tool call requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatResponse {
    /// LLM produced text content (final answer)
    Content(String),
    /// LLM wants to call tools before answering
    ToolCalls(Vec<ToolCall>),
}

/// A streaming event from the chat completion.
#[derive(Debug, Clone)]
pub enum ChatStreamEvent {
    /// A token of text content
    ContentDelta(String),
    /// A tool call was fully received (streamed tool calls are assembled first)
    ToolCallComplete(ToolCall),
    /// Stream is done
    Done,
}

/// Provider information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub model: String,
    pub context_window: usize,
    pub supports_streaming: bool,
    pub supports_functions: bool,
    pub is_local: bool,
}

/// Memory usage stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub ram_mb: usize,
    pub vram_mb: Option<usize>,
    pub model_size_mb: usize,
}

/// Main LLM manager
pub struct LLMManager {
    config: LLMConfig,
    provider: Option<Box<dyn LLMProvider>>,
}

impl LLMManager {
    /// Create new LLM manager
    pub fn new(config: LLMConfig) -> Self {
        Self {
            config,
            provider: None,
        }
    }

    /// Create the provider for the configured mode.
    pub async fn initialize(&mut self) -> Result<()> {
        match &self.config.mode {
            LLMMode::Local { model_path } => {
                // llama.cpp loads and memory-maps the whole model; keep it off
                // the async workers.
                let path = model_path.clone();
                let provider = tokio::task::spawn_blocking(move || LlamaCppProvider::new(&path))
                    .await
                    .map_err(|e| anyhow!("llama.cpp loader task failed: {e}"))??;
                self.provider = Some(Box::new(provider));
                Ok(())
            }
            LLMMode::External {
                provider,
                api_key,
                model,
            } => {
                let provider =
                    SimpleExternalProvider::new(provider.clone(), api_key.clone(), model.clone())?;
                self.provider = Some(Box::new(provider));
                Ok(())
            }
            LLMMode::Disabled => {
                self.provider = None;
                Ok(())
            }
        }
    }

    /// The active configuration.
    pub fn config(&self) -> &LLMConfig {
        &self.config
    }

    /// Switch to a different mode
    pub async fn switch_mode(&mut self, new_mode: LLMMode) -> Result<()> {
        // Clean up current provider
        if let Some(provider) = self.provider.take() {
            drop(provider); // This will free resources
        }

        // Update config and reinitialize
        self.config.mode = new_mode;
        self.initialize().await
    }

    /// Generate completion
    pub async fn generate(&self, prompt: &str) -> Result<String> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                // Ensure sufficient tokens for complete responses (floor at 4096)
                config.max_tokens = config.max_tokens.max(8192);
                provider.generate(prompt, &config).await
            }
            None => Err(anyhow!("LLM is disabled or not initialized")),
        }
    }

    /// Generate completion with custom max_tokens
    pub async fn generate_custom(&self, prompt: &str, max_tokens: usize) -> Result<String> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                config.max_tokens = max_tokens;
                provider.generate(prompt, &config).await
            }
            None => Err(anyhow!("LLM is disabled or not initialized")),
        }
    }

    /// Generate with streaming
    pub async fn generate_stream(&self, prompt: &str) -> Result<TokenStream> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                // Ensure sufficient tokens for complete responses
                config.max_tokens = config.max_tokens.max(8192);
                provider.generate_stream(prompt, &config).await
            }
            None => Err(anyhow!("LLM is disabled or not initialized")),
        }
    }

    /// Generate with streaming and custom max_tokens
    pub async fn generate_stream_custom(
        &self,
        prompt: &str,
        max_tokens: usize,
    ) -> Result<TokenStream> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                config.max_tokens = max_tokens;
                provider.generate_stream(prompt, &config).await
            }
            None => Err(anyhow!("LLM is disabled or not initialized")),
        }
    }

    /// Generate with RAG context
    pub async fn generate_with_rag(
        &self,
        query: &str,
        search_results: Vec<String>,
    ) -> Result<String> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                // RAG responses need more tokens for citations and structured output
                config.max_tokens = config.max_tokens.max(8192);
                provider
                    .generate_with_context(query, search_results, &config)
                    .await
            }
            None => {
                // Fallback to simple concatenation if LLM is disabled
                Ok(format!(
                    "Query: {}\n\nRelevant Information:\n{}",
                    query,
                    search_results.join("\n\n")
                ))
            }
        }
    }

    /// Generate with RAG context and custom max_tokens
    pub async fn generate_with_rag_custom(
        &self,
        query: &str,
        search_results: Vec<String>,
        max_tokens: usize,
    ) -> Result<String> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                config.max_tokens = max_tokens; // Override with custom token limit
                provider
                    .generate_with_context(query, search_results, &config)
                    .await
            }
            None => {
                // Fallback to simple concatenation if LLM is disabled
                Ok(format!(
                    "Query: {}\n\nRelevant Information:\n{}",
                    query,
                    search_results.join("\n\n")
                ))
            }
        }
    }

    /// Chat completion with message history and optional tool calling.
    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
    ) -> Result<ChatResponse> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                config.max_tokens = config.max_tokens.max(8192);
                provider.chat(messages, tools, &config).await
            }
            None => Err(anyhow!("LLM is disabled or not initialized")),
        }
    }

    /// Streaming chat completion with tool calling support.
    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSchema],
    ) -> Result<tokio::sync::mpsc::Receiver<ChatStreamEvent>> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                config.max_tokens = config.max_tokens.max(8192);
                provider.chat_stream(messages, tools, &config).await
            }
            None => Err(anyhow!("LLM is disabled or not initialized")),
        }
    }

    /// Check if the current provider supports function/tool calling.
    pub fn supports_tools(&self) -> bool {
        self.provider
            .as_ref()
            .map(|p| p.info().supports_functions)
            .unwrap_or(false)
    }

    /// Get current provider info
    pub fn info(&self) -> Option<ProviderInfo> {
        self.provider.as_ref().map(|p| p.info())
    }

    /// Get memory usage
    pub fn memory_usage(&self) -> Option<MemoryUsage> {
        self.provider.as_ref().map(|p| p.memory_usage())
    }

    /// Check if ready
    pub async fn is_ready(&self) -> bool {
        match &self.provider {
            Some(provider) => provider.is_ready().await,
            None => true, // Disabled mode is always "ready"
        }
    }
}

/// Format prompt for RAG
pub fn format_rag_prompt(query: &str, context: &[String], system_prompt: Option<&str>) -> String {
    let system = system_prompt.unwrap_or(
        "You are an intelligent AI assistant with access to a comprehensive knowledge base. \
         \n\n🚨 CRITICAL: When presenting data/numbers/comparisons/statistics, you MUST use ```table or ```chart code blocks. DO NOT just describe data - SHOW it in structured format using code blocks!\
         \n\nCRITICAL INSTRUCTIONS:\
         \n\n**1. Smart Entity Matching**\
         \n   - Match partial names to full names in context\
         \n   - Find name variations, aliases, and abbreviations\
         \n   - Look for related mentions across all documents\
         \n   - Scan ENTIRE context for ANY occurrence of the entity\
         \n\n**2. Universal Relationship Detection** (EXTREMELY IMPORTANT)\
         \n   When asked about ANY entity (person, company, code module, etc.), ALWAYS extract ALL relationships:\
         \n   \
         \n   - **Family**: Spouse, Partner, Husband, Wife, Father, Mother, Son, Daughter, Brother, Sister, Child, Parent, Relative\
         \n   - **Professional**: Employer, Employee, Manager, Supervisor, Client, Customer, Vendor, Supplier, Colleague, Coworker, Boss, Assistant\
         \n   - **Legal**: Lawyer, Attorney, Judge, Plaintiff, Defendant, Witness, Guardian, Trustee, Beneficiary, Executor\
         \n   - **Educational**: Teacher, Student, Professor, Instructor, Mentor, Tutor, Advisor, Dean, Principal, Classmate\
         \n   - **Medical**: Doctor, Patient, Nurse, Therapist, Physician, Surgeon, Caregiver\
         \n   - **Business**: Partner, Shareholder, Director, Investor, Founder, CEO, Board Member, Contractor, Consultant\
         \n   - **Code/Technical**: Imports, Depends on, Calls, Inherits from, Implements, Uses, References, Extends\
         \n   - **Generic patterns**: \"X is Y's ...\", \"X of Y\", \"X for Y\", \"X works with Y\", \"X reports to Y\", \"X managed by Y\"\
         \n   \
         \n   **Detection Strategy**:\
         \n   - Look for structured fields: \"Relationship: VALUE\", \"Role: VALUE\", \"Position: VALUE\"\
         \n   - Look for possessive constructions: \"John's lawyer\", \"Mary's client\", \"ABC Corp's vendor\"\
         \n   - Look for relational verbs: \"works for\", \"employed by\", \"managed by\", \"teaches\", \"represents\"\
         \n   - Look for co-occurrence patterns: if document mentions both entities, extract their relationship\
         \n   - Check metadata: job titles, roles, organizational charts\
         \n   \
         \n   **When asked about X**: If ANY document mentions X in relation to Y, report it. \
         \n   Example: \"Tell me about Person A\" → Find \"Spouse: Person A\" → Report \"Person A is the spouse of Person B\"\
         \n\n**3. Code Understanding** (when context contains code):\
         \n   - Identify functions, classes, modules, and their relationships\
         \n   - Understand import/dependency chains\
         \n   - Explain code architecture and data flow\
         \n   - Reference specific file paths and line numbers when known\
         \n   - Distinguish between documentation and implementation\
         \n\n**4. Exhaustive Context Search**\
         \nBefore saying 'no information found', scan EVERY document for:\
         \n   - Direct mentions (full or partial names)\
         \n   - Metadata (file paths, headers, tags, categories, titles)\
         \n   - Indirect mentions (as someone's relative, colleague, client, etc.)\
         \n   - Relationship fields (ANY field indicating connection between entities)\
         \n   - Structured data (tables, forms, key-value pairs)\
         \n   - Unstructured text (narrative descriptions of relationships)\
         \n   - Code references (imports, function calls, class inheritance)\
         \n   - Documentation references (comments, docstrings, markdown)\
         \n\n**5. Always Cite Sources**\
         \n   - Reference specific document numbers: [Document 1], [Document 2], etc.\
         \n   - Include file paths when available\
         \n   - Quote EXACT text when citing relationships: \"According to [Document 1]: 'Spouse: [Name]'\"\
         \n   - Be precise about WHERE the information was found\
         \n\n**6. High Accuracy Standard**\
         \n   - Only state 'no information found' after thoroughly checking ALL context\
         \n   - Be specific about what information IS available\
         \n   - Suggest related information if exact match not found\
         \n   - If asked about X and you find X mentioned as Y's [relationship], ALWAYS include: \"X is Y's [relationship]\"\
         \n   - Provide comprehensive answers that synthesize information from multiple documents\
         \n\n**7. Response Quality**\
         \n   - Start with direct answer to the question\
         \n   - Include ALL relevant relationships found\
         \n   - Cite sources for every claim\
         \n   - If multiple documents mention the entity, synthesize information from all of them\
         \n   - Use clear, professional language\
         \n\n**8. STRUCTURED OUTPUT GENERATION (CRITICAL)**\
         \n\n   When user asks for data/comparisons/statistics/metrics:\
         \n   ✅ DO: Output actual ```table or ```chart code blocks with data\
         \n   ❌ DON'T: Say \"Here's a table\" without the code block\
         \n   ❌ DON'T: Say \"I'll create a chart\" without the code block\
         \n\n   Examples:\
         \n   User: \"Show Q4 sales\" → Output ```table block with actual sales data\
         \n   User: \"Compare regions\" → Output ```chart block with actual JSON\
         \n   User: \"List top 10\" → Output ```table block with actual list\
         \n\n   Table format: Use markdown tables in ```table blocks\
         \n   Chart format: Use JSON in ```chart blocks with fields: type, title, data{labels, datasets}\
         \n   Supported chart types: bar, line, pie, scatter, area\
         \n\nBe comprehensive, intelligent, and precise in every response. ALWAYS extract and report ALL relationship information found in the context."
    );

    // Format context with clear document boundaries
    let formatted_context = if context.is_empty() {
        "No specific context documents available.".to_string()
    } else {
        context
            .iter()
            .enumerate()
            .map(|(i, doc)| format!("[Document {}]\n{}", i + 1, doc))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    format!(
        "{}\n\n=== CONTEXT DOCUMENTS ===\n{}\n=== END CONTEXT ===\n\nUser Question: {}\n\nAssistant Response:",
        system,
        formatted_context,
        query
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_config_default() {
        let config = LLMConfig::default();
        assert!(matches!(config.mode, LLMMode::Disabled));
        assert_eq!(config.max_tokens, 8192);
    }

    #[test]
    fn debug_output_redacts_the_api_key() {
        let mode = LLMMode::External {
            provider: ApiProvider::OpenRouter,
            api_key: "sk-or-secret".to_string(),
            model: "m".to_string(),
        };
        let rendered = format!("{mode:?}");
        assert!(!rendered.contains("sk-or-secret"));
        assert!(rendered.contains("[REDACTED]"));
        let json = serde_json::to_string(&mode).unwrap();
        assert!(!json.contains("sk-or-secret"));
        assert_eq!(mode.kind(), "external");
        assert_eq!(
            LLMMode::Local {
                model_path: PathBuf::from("m.gguf")
            }
            .kind(),
            "local"
        );
    }
}
