//! LLM Module - Hybrid local/external language model support
//! Supports both local models (via Candle) and external APIs

use anyhow::{Result, anyhow};
use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use async_trait::async_trait;
use tokio::sync::mpsc;
use serde_json::Value as JsonValue;

// Hybrid LLM implementation - llama.cpp for CPU, ONNX Runtime GenAI for GPU
pub mod llamacpp_provider;      // llama.cpp provider (CPU with KV caching)
pub mod genai_provider;         // ONNX Runtime GenAI provider (GPU optimized)
pub mod onnx_llm;
pub mod onnx_llm_production;
pub mod onnxruntime_genai_sys;  // Low-level FFI bindings
pub mod onnxruntime_genai;      // Safe Rust wrappers
pub mod local;
pub mod external;
pub mod simple_external;
pub mod streaming;
pub mod model_manager;
pub mod model_config;
pub mod bpe_tokenizer;
pub mod hf_tokenizer;
pub mod download_tokenizers;
pub mod tokenizer_loader;
pub mod gqa_cache;

pub use llamacpp_provider::LlamaCppProvider;
pub use genai_provider::GenAIProvider;
pub use local::LocalModelProvider;
pub use external::ExternalProvider;
pub use simple_external::SimpleExternalProvider;
pub use streaming::{StreamingResponse, TokenStream};
pub use model_manager::{ModelManager, ModelDownloader};


/// LLM operation mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMMode {
    /// Local model running in-process
    Local {
        model: LocalModel,
        device: DeviceType,
        quantization: QuantizationType,
    },
    /// External API provider
    External {
        provider: ApiProvider,
        api_key: String,
        model: String,
    },
    /// LLM disabled, RAG-only mode
    Disabled,
}

/// Supported local models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LocalModel {
    Phi3Mini,       // Microsoft Phi-3 3.8B
    Phi4,           // Microsoft Phi-4 14B - Latest model
    Mistral7B,      // Mistral 7B Instruct
    Orca2_7B,       // Microsoft Orca 2 7B
    Qwen2_5B,       // Alibaba Qwen2 0.5B
    Gemma2B,        // Google Gemma 2B
    Custom { 
        name: String,
        filename: String,
    }, // Custom model
}

impl LocalModel {
    pub fn model_id(&self) -> &str {
        match self {
            Self::Phi3Mini => "microsoft/Phi-3-mini-4k-instruct-onnx",
            Self::Phi4 => "microsoft/phi-4",
            Self::Mistral7B => "mistralai/Mistral-7B-Instruct-v0.2",
            Self::Orca2_7B => "microsoft/Orca-2-7b",
            Self::Qwen2_5B => "Qwen/Qwen2-0.5B-Instruct",
            Self::Gemma2B => "google/gemma-2b",
            Self::Custom { name, .. } => name,
        }
    }

    pub fn size_gb(&self) -> f32 {
        match self {
            Self::Phi3Mini => 2.0,    // ONNX quantized
            Self::Phi4 => 8.0,        // ONNX quantized 14B model
            Self::Mistral7B => 4.0,   // ONNX quantized
            Self::Orca2_7B => 4.0,    // ONNX quantized
            Self::Qwen2_5B => 1.5,    // ONNX quantized
            Self::Gemma2B => 1.0,     // ONNX quantized
            Self::Custom { .. } => 2.0,
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

/// Device type for local models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceType {
    Cpu,
    Cuda(usize),  // GPU index
    Metal,        // Apple Silicon
}

/// Quantization type for model compression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuantizationType {
    F32,      // Full precision
    F16,      // Half precision
    Q8,       // 8-bit quantization
    Q4,       // 4-bit quantization
    Q4_K_M,   // 4-bit with k-means
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
    async fn generate(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<String>;

    /// Generate with streaming
    async fn generate_stream(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<TokenStream>;

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
        let prompt = messages.iter()
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
        let prompt = messages.iter()
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
        Self { role: ChatRole::System, content: Some(content.into()), tool_calls: None, tool_call_id: None, name: None }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: ChatRole::User, content: Some(content.into()), tool_calls: None, tool_call_id: None, name: None }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: ChatRole::Assistant, content: Some(content.into()), tool_calls: None, tool_call_id: None, name: None }
    }
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self { role: ChatRole::Assistant, content: None, tool_calls: Some(tool_calls), tool_call_id: None, name: None }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: ChatRole::Tool, content: Some(content.into()), tool_calls: None, tool_call_id: Some(tool_call_id.into()), name: Some(name.into()) }
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

/// Hardware detection result
#[derive(Debug, Clone)]
pub struct HardwareInfo {
    pub has_cuda: bool,
    pub has_directml: bool,
    pub has_metal: bool,
    pub recommended_device: DeviceType,
}

impl HardwareInfo {
    /// Auto-detect available hardware acceleration
    pub fn detect() -> Self {
        tracing::info!("Detecting available hardware acceleration");

        // Check for NVIDIA GPU (CUDA)
        let has_cuda = Self::check_cuda();

        // Check for DirectML (Windows GPU - AMD/Intel/NVIDIA)
        let has_directml = Self::check_directml();

        // Check for Metal (Apple Silicon)
        let has_metal = Self::check_metal();

        // Determine recommended device
        let recommended_device = if has_cuda {
            tracing::info!("CUDA GPU detected");
            DeviceType::Cuda(0)
        } else if has_directml {
            tracing::info!("DirectML GPU detected (Windows)");
            DeviceType::Cpu // DirectML uses CPU enum but GPU inference
        } else if has_metal {
            tracing::info!("Metal GPU detected (Apple Silicon)");
            DeviceType::Metal
        } else {
            tracing::info!("No GPU detected, will use CPU");
            DeviceType::Cpu
        };

        HardwareInfo {
            has_cuda,
            has_directml,
            has_metal,
            recommended_device,
        }
    }

    fn check_cuda() -> bool {
        // Check for CUDA availability via environment or nvidia-smi
        if std::env::var("CUDA_PATH").is_ok() || std::env::var("CUDA_HOME").is_ok() {
            return true;
        }

        // Try running nvidia-smi
        #[cfg(not(target_os = "windows"))]
        let result = std::process::Command::new("nvidia-smi").output();
        #[cfg(target_os = "windows")]
        let result = std::process::Command::new("nvidia-smi.exe").output();

        result.map(|o| o.status.success()).unwrap_or(false)
    }

    fn check_directml() -> bool {
        // DirectML is available on Windows 10+ with any GPU
        #[cfg(target_os = "windows")]
        {
            // Check Windows version (DirectML requires Windows 10 build 1903+)
            return true; // Assume available on Windows
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    fn check_metal() -> bool {
        // Metal is available on macOS with Apple Silicon
        #[cfg(target_os = "macos")]
        {
            // Check for Apple Silicon (arm64)
            #[cfg(target_arch = "aarch64")]
            return true;
            #[cfg(not(target_arch = "aarch64"))]
            return false;
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

/// Main LLM manager
pub struct LLMManager {
    config: LLMConfig,
    provider: Option<Box<dyn LLMProvider>>,
    model_cache_dir: PathBuf,
}

impl LLMManager {
    /// Auto-detect best hardware and create config
    pub fn auto_config(mode: LLMMode) -> LLMConfig {
        let hw_info = HardwareInfo::detect();
        tracing::info!(
            cuda = hw_info.has_cuda,
            directml = hw_info.has_directml,
            metal = hw_info.has_metal,
            recommended = ?hw_info.recommended_device,
            "Hardware summary"
        );

        LLMConfig {
            mode,
            ..Default::default()
        }
    }

    /// Create new LLM manager
    pub fn new(config: LLMConfig) -> Self {
        Self {
            config,
            provider: None,
            model_cache_dir: PathBuf::from("./models"),
        }
    }
    
    /// Create new LLM manager with custom cache directory
    pub fn new_with_cache_dir(config: LLMConfig, cache_dir: PathBuf) -> Self {
        Self {
            config,
            provider: None,
            model_cache_dir: cache_dir,
        }
    }
    
    /// Create new LLM manager with custom model and tokenizer paths
    pub fn new_with_paths(config: LLMConfig, model_path: PathBuf, tokenizer_path: Option<PathBuf>) -> Self {
        // Store tokenizer path in environment variable for ONNX provider to pick up
        if let Some(tokenizer) = tokenizer_path {
            std::env::set_var("ROSHERA_TOKENIZER_PATH", tokenizer);
        }
        
        Self {
            config,
            provider: None,
            model_cache_dir: model_path,
        }
    }

    /// Initialize the LLM provider with hybrid backend selection
    pub async fn initialize(&mut self) -> Result<()> {
        match &self.config.mode {
            LLMMode::Local { model, device, quantization } => {
                // HYBRID BACKEND SELECTION
                // llama.cpp for CPU (has KV caching) + ONNX Runtime GenAI for GPU

                let use_gpu = match device {
                    DeviceType::Cpu => {
                        tracing::info!("Device explicitly set to CPU, using llama.cpp");
                        false
                    }
                    DeviceType::Cuda(_) => {
                        tracing::info!("Device explicitly set to CUDA GPU, using ONNX Runtime GenAI");
                        true
                    }
                    DeviceType::Metal => {
                        tracing::info!("Device explicitly set to Metal, using ONNX Runtime GenAI");
                        true
                    }
                };

                // Create appropriate provider based on device
                let provider: Box<dyn LLMProvider> = if use_gpu {
                    // GPU path: Use ONNX Runtime GenAI (CUDA, DirectML, TensorRT)
                    tracing::info!("Initializing ONNX Runtime GenAI for GPU acceleration");
                    Box::new(GenAIProvider::new(
                        model.clone(),
                        device.clone(),
                        quantization.clone(),
                        &self.model_cache_dir,
                    )?)
                } else {
                    // CPU path: Use llama.cpp (KV caching, prompt caching, SIMD)
                    tracing::info!("Initializing llama.cpp for CPU with prompt caching");
                    Box::new(LlamaCppProvider::new(
                        model.clone(),
                        device.clone(),
                        quantization.clone(),
                        &self.model_cache_dir,
                    )?)
                };

                self.provider = Some(provider);
                Ok(())
            }
            LLMMode::External { provider, api_key, model } => {
                // Create simple external provider for better reliability
                let provider = SimpleExternalProvider::new(
                    provider.clone(),
                    api_key.clone(),
                    model.clone(),
                )?;

                self.provider = Some(Box::new(provider));
                Ok(())
            }
            LLMMode::Disabled => {
                self.provider = None;
                Ok(())
            }
        }
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
            None => Err(anyhow!("LLM is disabled or not initialized"))
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
            None => Err(anyhow!("LLM is disabled or not initialized"))
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
            None => Err(anyhow!("LLM is disabled or not initialized"))
        }
    }

    /// Generate with streaming and custom max_tokens
    pub async fn generate_stream_custom(&self, prompt: &str, max_tokens: usize) -> Result<TokenStream> {
        match &self.provider {
            Some(provider) => {
                let mut config = GenerationConfig::from(&self.config);
                config.max_tokens = max_tokens;
                provider.generate_stream(prompt, &config).await
            }
            None => Err(anyhow!("LLM is disabled or not initialized"))
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
                provider.generate_with_context(query, search_results, &config).await
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
                config.max_tokens = max_tokens;  // Override with custom token limit
                provider.generate_with_context(query, search_results, &config).await
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
        self.provider.as_ref()
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
        context.iter().enumerate()
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
        assert_eq!(config.max_tokens, 1024);
    }

    #[test]
    fn test_local_model_info() {
        let model = LocalModel::Phi3Mini;
        assert_eq!(model.model_id(), "microsoft/Phi-3-mini-4k-instruct");
        assert_eq!(model.size_gb(), 3.8);
    }
}