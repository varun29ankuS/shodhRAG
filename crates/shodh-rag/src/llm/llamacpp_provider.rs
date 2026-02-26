//! llama.cpp LLM Provider — native local inference via llama-cpp-2 bindings.
//!
//! Loads GGUF models directly and runs inference on CPU.
//! Streaming is handled via `spawn_blocking` + mpsc channels since
//! llama.cpp is synchronous and CPU-bound.

use anyhow::{Result, Context as AnyhowContext, anyhow};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use super::{
    DeviceType, GenerationConfig, LLMProvider, LocalModel, MemoryUsage, ProviderInfo,
    QuantizationType,
};
use super::streaming::TokenStream;

/// Information about the loaded model for metadata/info reporting.
struct ModelInfo {
    name: String,
    context_window: usize,
    size_mb: usize,
}

pub struct LlamaCppProvider {
    model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    info: ModelInfo,
}

// SAFETY: LlamaModel and LlamaBackend are thread-safe for read-only operations.
// Mutable state (LlamaContext) is created per-inference call and not shared.
unsafe impl Send for LlamaCppProvider {}
unsafe impl Sync for LlamaCppProvider {}

impl LlamaCppProvider {
    pub fn new(
        model_variant: LocalModel,
        _device: DeviceType,
        _quantization: QuantizationType,
        cache_dir: &Path,
    ) -> Result<Self> {
        // Initialize the llama.cpp backend
        let backend = LlamaBackend::init().context("Failed to initialize llama.cpp backend")?;

        // Resolve the GGUF file path
        let gguf_path = Self::resolve_model_path(&model_variant, cache_dir)?;

        tracing::info!(
            model = %model_variant.model_id(),
            path = %gguf_path.display(),
            "Loading GGUF model via llama.cpp"
        );

        // Configure model params — CPU only, no GPU offload
        let model_params = LlamaModelParams::default();

        // Load the model
        let model = LlamaModel::load_from_file(&backend, &gguf_path, &model_params)
            .map_err(|e| anyhow!("Failed to load GGUF model from {}: {:?}", gguf_path.display(), e))?;

        let info = ModelInfo {
            name: Self::model_display_name(&model_variant),
            context_window: Self::model_context_window(&model_variant),
            size_mb: (model_variant.size_gb() * 1024.0) as usize,
        };

        tracing::info!(
            model = %info.name,
            context_window = info.context_window,
            "llama.cpp model loaded successfully"
        );

        Ok(Self {
            model: Arc::new(model),
            backend: Arc::new(backend),
            info,
        })
    }

    /// Resolve GGUF file path from LocalModel variant and cache directory.
    fn resolve_model_path(model: &LocalModel, cache_dir: &Path) -> Result<PathBuf> {
        // If cache_dir itself is a GGUF file, use it directly
        if cache_dir.is_file()
            && cache_dir
                .extension()
                .map(|e| e == "gguf")
                .unwrap_or(false)
        {
            return Ok(cache_dir.to_path_buf());
        }

        let filename = Self::model_filename(model);
        let path = cache_dir.join(&filename);

        if path.exists() {
            return Ok(path);
        }

        // Try parent directory (models might be one level up from cache_dir)
        if let Some(parent) = cache_dir.parent() {
            let parent_path = parent.join(&filename);
            if parent_path.exists() {
                return Ok(parent_path);
            }
        }

        // Search for any GGUF file in the directory as fallback
        if cache_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(cache_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().map(|e| e == "gguf").unwrap_or(false) {
                        tracing::warn!(
                            expected = %filename,
                            found = %p.display(),
                            "Expected model not found, using first GGUF file in directory"
                        );
                        return Ok(p);
                    }
                }
            }
        }

        Err(anyhow!(
            "GGUF model file not found. Expected '{}' in {}",
            filename,
            cache_dir.display()
        ))
    }

    /// Map LocalModel enum to actual GGUF filename on disk.
    fn model_filename(model: &LocalModel) -> String {
        match model {
            LocalModel::Phi3Mini => "Phi-3-mini-128k-instruct.Q4_K_M.gguf".to_string(),
            LocalModel::Phi4 => "phi-4.Q4_K_M.gguf".to_string(),
            LocalModel::Mistral7B => "mistral-7b-instruct-v0.2.Q4_K_M.gguf".to_string(),
            LocalModel::Orca2_7B => "orca-2-7b.Q4_K_M.gguf".to_string(),
            LocalModel::Qwen2_5B => "qwen2.5-1.5b-instruct-q4_k_m.gguf".to_string(),
            LocalModel::Gemma2B => "gemma-2b.Q4_K_M.gguf".to_string(),
            LocalModel::Sarvam1 => "sarvam-1.Q5_K_M.gguf".to_string(),
            LocalModel::Custom { filename, .. } => filename.clone(),
        }
    }

    fn model_display_name(model: &LocalModel) -> String {
        match model {
            LocalModel::Phi3Mini => "Phi-3 Mini 128K (Q4_K_M)".to_string(),
            LocalModel::Phi4 => "Phi-4 (Q4_K_M)".to_string(),
            LocalModel::Mistral7B => "Mistral 7B Instruct (Q4_K_M)".to_string(),
            LocalModel::Orca2_7B => "Orca 2 7B (Q4_K_M)".to_string(),
            LocalModel::Qwen2_5B => "Qwen 2.5 1.5B Instruct (Q4_K_M)".to_string(),
            LocalModel::Gemma2B => "Gemma 2B (Q4_K_M)".to_string(),
            LocalModel::Sarvam1 => "Sarvam-1 2B Indic (Q5_K_M)".to_string(),
            LocalModel::Custom { name, .. } => name.clone(),
        }
    }

    fn model_context_window(model: &LocalModel) -> usize {
        match model {
            LocalModel::Phi3Mini => 131072,  // 128K context
            LocalModel::Phi4 => 16384,
            LocalModel::Mistral7B => 8192,
            LocalModel::Orca2_7B => 4096,
            LocalModel::Qwen2_5B => 32768,
            LocalModel::Gemma2B => 8192,
            LocalModel::Sarvam1 => 4096,
            LocalModel::Custom { .. } => 8192,
        }
    }

    /// Run synchronous inference. Called from both `generate()` and `generate_stream()`.
    /// If `token_sender` is Some, tokens are streamed as they're generated.
    fn run_inference(
        model: &LlamaModel,
        backend: &LlamaBackend,
        prompt: &str,
        config: &GenerationConfig,
        token_sender: Option<mpsc::Sender<String>>,
    ) -> Result<String> {
        // Limit context to a reasonable size for inference (not the model's max)
        let n_ctx = 4096u32.min(8192);

        let ctx_params = LlamaContextParams::default().with_n_ctx(std::num::NonZeroU32::new(n_ctx));
        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create llama context: {:?}", e))?;

        // Tokenize the prompt
        let tokens = model
            .str_to_token(prompt, llama_cpp_2::model::AddBos::Always)
            .map_err(|e| anyhow!("Tokenization failed: {:?}", e))?;

        let n_prompt = tokens.len();
        if n_prompt == 0 {
            return Ok(String::new());
        }

        // Truncate prompt tokens if they exceed context
        let max_prompt_tokens = (n_ctx as usize).saturating_sub(config.max_tokens.min(2048));
        let tokens = if n_prompt > max_prompt_tokens {
            tracing::warn!(
                n_prompt = n_prompt,
                max = max_prompt_tokens,
                "Prompt truncated to fit context window"
            );
            tokens[n_prompt - max_prompt_tokens..].to_vec()
        } else {
            tokens
        };
        let n_prompt = tokens.len();

        // Feed prompt tokens into context via batch.
        // Process in chunks of n_batch (default 2048) to avoid exceeding
        // llama.cpp's per-decode token limit.
        let n_batch = 2048usize;
        let mut batch = LlamaBatch::new(n_batch, 1);

        let mut processed = 0usize;
        while processed < n_prompt {
            batch.clear();
            let chunk_end = (processed + n_batch).min(n_prompt);

            for i in processed..chunk_end {
                let is_last = i == n_prompt - 1;
                batch
                    .add(tokens[i], i as i32, &[0], is_last)
                    .map_err(|_| anyhow!("Failed to add token to batch"))?;
            }

            ctx.decode(&mut batch)
                .map_err(|e| anyhow!("Prompt decode chunk {}-{} failed: {:?}", processed, chunk_end, e))?;

            processed = chunk_end;
        }

        // Set up sampler chain with repetition penalty to prevent loops.
        // penalties(last_n, repeat_penalty, freq_penalty, presence_penalty)
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::penalties(256, 1.15, 0.0, 0.0),
            LlamaSampler::temp(config.temperature),
            LlamaSampler::top_p(config.top_p, 1),
            LlamaSampler::top_k(config.top_k as i32),
            LlamaSampler::dist(config.seed.unwrap_or(0) as u32),
        ]);

        // Generation loop
        let max_tokens = config.max_tokens.min(2048);
        let mut output = String::new();
        let mut n_decoded = 0usize;
        let mut cur_pos = n_prompt as i32;

        let eos_token = model.token_eos();

        // Built-in stop patterns that catch common small-model loops
        // (in addition to any user-provided stop sequences)
        let builtin_stops: &[&str] = &[
            "User Question:",
            "User:",
            "\nQuestion:",
            "<|im_end|>",
            "<|endoftext|>",
            "<|end|>",
            "Assistant:",
            "\n\nAnswer:",
        ];

        loop {
            if n_decoded >= max_tokens {
                break;
            }

            // Sample next token
            let new_token = sampler.sample(&ctx, -1);

            // Check for end of stream
            if new_token == eos_token {
                break;
            }

            // Decode token to text
            #[allow(deprecated)]
            let token_str = model
                .token_to_str(new_token, llama_cpp_2::model::Special::Tokenize)
                .unwrap_or_default();

            if !token_str.is_empty() {
                output.push_str(&token_str);

                // Send token if streaming
                if let Some(ref sender) = token_sender {
                    if sender.blocking_send(token_str).is_err() {
                        // Receiver dropped, stop generation
                        break;
                    }
                }
            }

            // Check user-provided stop sequences
            let should_stop = config
                .stop_sequences
                .iter()
                .any(|seq| output.ends_with(seq));
            if should_stop {
                for seq in &config.stop_sequences {
                    if output.ends_with(seq) {
                        output.truncate(output.len() - seq.len());
                        break;
                    }
                }
                break;
            }

            // Check built-in stop patterns (prevent repetition loops)
            let hit_builtin = builtin_stops.iter().any(|pat| output.ends_with(pat));
            if hit_builtin {
                for pat in builtin_stops {
                    if output.ends_with(pat) {
                        output.truncate(output.len() - pat.len());
                        break;
                    }
                }
                break;
            }

            // Detect repetition: if the last 200 chars repeat a pattern 3+ times, stop
            if n_decoded > 100 && n_decoded % 50 == 0 {
                let tail = if output.len() > 300 { &output[output.len()-300..] } else { &output };
                if has_repetition(tail) {
                    tracing::warn!(tokens = n_decoded, "Repetition detected, stopping generation");
                    break;
                }
            }

            n_decoded += 1;

            // Prepare next batch with the generated token
            batch.clear();
            batch
                .add(new_token, cur_pos, &[0], true)
                .map_err(|_| anyhow!("Failed to add generated token to batch"))?;
            cur_pos += 1;

            ctx.decode(&mut batch)
                .map_err(|e| anyhow!("Decode step {} failed: {:?}", n_decoded, e))?;
        }

        tracing::debug!(
            prompt_tokens = n_prompt,
            generated_tokens = n_decoded,
            "llama.cpp inference complete"
        );

        Ok(output)
    }
}

/// Detect if text contains a repeating pattern (e.g., the same 50+ char block
/// appears 3+ times). Used to break infinite generation loops in small models.
fn has_repetition(text: &str) -> bool {
    let len = text.len();
    if len < 100 {
        return false;
    }

    // Check for repeating blocks of various sizes (30-100 chars)
    for block_size in [30, 50, 80] {
        if len < block_size * 3 {
            continue;
        }
        let last_block = &text[len - block_size..];
        let search_area = &text[..len - block_size];
        let count = search_area
            .as_bytes()
            .windows(block_size)
            .filter(|w| *w == last_block.as_bytes())
            .count();
        if count >= 2 {
            return true;
        }
    }

    false
}

#[async_trait]
impl LLMProvider for LlamaCppProvider {
    async fn generate(&self, prompt: &str, config: &GenerationConfig) -> Result<String> {
        let model = Arc::clone(&self.model);
        let backend = Arc::clone(&self.backend);
        let prompt = prompt.to_string();
        let config = config.clone();

        tokio::task::spawn_blocking(move || {
            Self::run_inference(&model, &backend, &prompt, &config, None)
        })
        .await
        .map_err(|e| anyhow!("Inference task panicked: {}", e))?
    }

    async fn generate_stream(
        &self,
        prompt: &str,
        config: &GenerationConfig,
    ) -> Result<TokenStream> {
        let model = Arc::clone(&self.model);
        let backend = Arc::clone(&self.backend);
        let prompt = prompt.to_string();
        let config = config.clone();

        let (tx, rx) = mpsc::channel(256);

        tokio::task::spawn_blocking(move || {
            if let Err(e) = Self::run_inference(&model, &backend, &prompt, &config, Some(tx)) {
                tracing::error!("Streaming inference failed: {}", e);
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
        let prompt = super::format_rag_prompt(query, &context, None);
        self.generate(&prompt, config).await
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: format!("llama.cpp ({})", self.info.name),
            model: self.info.name.clone(),
            context_window: self.info.context_window,
            supports_streaming: true,
            supports_functions: false,
            is_local: true,
        }
    }

    async fn is_ready(&self) -> bool {
        true
    }

    fn memory_usage(&self) -> MemoryUsage {
        MemoryUsage {
            ram_mb: self.info.size_mb,
            vram_mb: None,
            model_size_mb: self.info.size_mb,
        }
    }
}
