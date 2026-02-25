//! Tauri commands for LLM integration

use serde::{Deserialize, Serialize};
use shodh_rag::llm::{
    ApiProvider, DeviceType, LLMConfig, LLMManager, LLMMode, LocalModel, ModelManager,
    QuantizationType,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};
use tokio::sync::RwLock as AsyncRwLock;

/// LLM state managed by Tauri
pub struct LLMState {
    pub manager: Arc<AsyncRwLock<Option<LLMManager>>>,
    pub model_manager: Arc<ModelManager>,
    pub config: Arc<Mutex<LLMConfig>>,
    pub api_keys: Arc<Mutex<ApiKeys>>,
    pub custom_model_path: Arc<Mutex<Option<PathBuf>>>,
    pub custom_tokenizer_path: Arc<Mutex<Option<PathBuf>>>,
}

/// API keys held in memory for the current session.
/// Keys are never serialized to disk (see `#[serde(skip_serializing)]` on LLMMode).
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct ApiKeys {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
    pub openrouter: Option<String>,
    pub kimi: Option<String>,
    pub grok: Option<String>,
    pub perplexity: Option<String>,
    pub google: Option<String>,
    pub baseten: Option<String>,
}

/// Browse and select model file (supports both ONNX and GGUF)
#[tauri::command]
pub async fn browse_model_file(
    app_handle: tauri::AppHandle,
    backend: Option<String>,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let mut dialog = app_handle.dialog().file();

    // Add filters based on backend type
    match backend.as_deref() {
        Some("llamacpp") => {
            dialog = dialog
                .add_filter("GGUF Models", &["gguf"])
                .add_filter("All Files", &["*"]);
        }
        Some("onnx") => {
            dialog = dialog
                .add_filter("ONNX Models", &["onnx"])
                .add_filter("All Files", &["*"]);
        }
        _ => {
            // Default: show both formats
            dialog = dialog
                .add_filter("Model Files", &["gguf", "onnx"])
                .add_filter("GGUF Models", &["gguf"])
                .add_filter("ONNX Models", &["onnx"])
                .add_filter("All Files", &["*"]);
        }
    }

    let file_path = dialog.blocking_pick_file();

    match file_path {
        Some(path) => {
            // FilePath enum can be either Path or Url
            let path_str = match path {
                tauri_plugin_dialog::FilePath::Path(p) => p.to_string_lossy().to_string(),
                tauri_plugin_dialog::FilePath::Url(u) => u.to_string(),
            };
            Ok(path_str)
        }
        None => Err("No file selected".to_string()),
    }
}

/// Browse and select tokenizer file
#[tauri::command]
pub async fn browse_tokenizer_file(app_handle: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let file_path = app_handle
        .dialog()
        .file()
        .add_filter("Tokenizer Files", &["json"])
        .add_filter("All Files", &["*"])
        .blocking_pick_file();

    match file_path {
        Some(path) => {
            let path_str = match path {
                tauri_plugin_dialog::FilePath::Path(p) => p.to_string_lossy().to_string(),
                tauri_plugin_dialog::FilePath::Url(u) => u.to_string(),
            };
            Ok(path_str)
        }
        None => Err("No file selected".to_string()),
    }
}

/// Set custom model path from user selection
#[tauri::command]
pub fn set_custom_model_path(
    state: State<'_, LLMState>,
    model_path: String,
) -> Result<String, String> {
    let path = PathBuf::from(&model_path);

    // Verify the file exists
    if !path.exists() {
        return Err(format!("Model file does not exist: {}", model_path));
    }

    // Check if it's a valid model file (ONNX or GGUF)
    let extension = path.extension().and_then(|s| s.to_str());
    match extension {
        Some("onnx") => {
            // Store the custom path
            *state
                .custom_model_path
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(path.clone());

            // Check for associated .data file (ONNX specific)
            let data_file = path.with_extension("onnx.data");
            let has_data_file = data_file.exists();

            Ok(format!(
                "✅ ONNX model path set successfully\n📁 Path: {}\n📦 Data file: {}",
                model_path,
                if has_data_file {
                    "Found"
                } else {
                    "Not found (may not be required)"
                }
            ))
        }
        Some("gguf") => {
            // Store the custom path
            *state
                .custom_model_path
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(path.clone());

            Ok(format!(
                "✅ GGUF model path set successfully\n📁 Path: {}\n🚀 Backend: llama.cpp (tokenizer built-in)",
                model_path
            ))
        }
        _ => Err("Invalid model file. Please select a .gguf or .onnx file.".to_string()),
    }
}

/// Set custom tokenizer path from user selection
#[tauri::command]
pub fn set_custom_tokenizer_path(
    state: State<'_, LLMState>,
    tokenizer_path: String,
) -> Result<String, String> {
    let path = PathBuf::from(&tokenizer_path);

    // Verify the file exists
    if !path.exists() {
        return Err(format!("Tokenizer file does not exist: {}", tokenizer_path));
    }

    // Store the custom tokenizer path
    *state
        .custom_tokenizer_path
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(path);

    Ok(format!(
        "Tokenizer path set successfully: {}",
        tokenizer_path
    ))
}

/// Initialize LLM with custom path if set
#[tauri::command]
pub async fn initialize_llm_with_custom_path(state: State<'_, LLMState>) -> Result<String, String> {
    let custom_model_path = state
        .custom_model_path
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let custom_tokenizer_path = state
        .custom_tokenizer_path
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    if let Some(model_path) = custom_model_path {
        // Create config with custom model
        let mut config = state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        config.mode = LLMMode::Local {
            model: LocalModel::Custom {
                name: "custom".to_string(),
                filename: model_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("model.onnx")
                    .to_string(),
            },
            device: DeviceType::Cpu,
            quantization: QuantizationType::Q4,
        };

        // Update stored config
        *state.config.lock().unwrap_or_else(|e| e.into_inner()) = config.clone();

        // Pass both model and tokenizer paths
        // Create a custom manager that knows about both paths
        let mut manager =
            LLMManager::new_with_paths(config, model_path.clone(), custom_tokenizer_path.clone());

        // Initialize
        manager.initialize().await.map_err(|e| {
            format!("Failed to initialize model: {}. Please ensure the model file is valid and compatible.", e)
        })?;

        // Store manager
        *state.manager.write().await = Some(manager);

        let mut success_msg = format!("Model loaded successfully from: {}", model_path.display());
        if let Some(tokenizer_path) = custom_tokenizer_path {
            success_msg.push_str(&format!("\nTokenizer: {}", tokenizer_path.display()));
        }
        Ok(success_msg)
    } else {
        Err("No custom model path set. Please select a model file first.".to_string())
    }
}

/// Initialize LLM manager
#[tauri::command]
pub async fn initialize_llm(state: State<'_, LLMState>, mode: String) -> Result<String, String> {
    let config = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    // Parse mode string to LLMMode
    let llm_mode = parse_llm_mode(&mode)?;

    // Create new config with mode
    let mut new_config = config;
    new_config.mode = llm_mode;

    // Update stored config
    *state.config.lock().unwrap_or_else(|e| e.into_inner()) = new_config.clone();

    // Create and initialize manager
    let mut manager = LLMManager::new(new_config);
    manager.initialize().await.map_err(|e| e.to_string())?;

    // Store manager
    *state.manager.write().await = Some(manager);

    Ok("LLM initialized successfully".to_string())
}

/// Switch LLM mode
#[tauri::command]
pub async fn switch_llm_mode(
    state: State<'_, LLMState>,
    mode: String,
    model: Option<String>,
    provider: Option<String>,
) -> Result<String, String> {
    // Check if user wants to use custom model
    if mode == "custom" {
        let custom_path = state
            .custom_model_path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if custom_path.is_none() {
            return Err("Please select a model file first using the Browse button".to_string());
        }
        return initialize_llm_with_custom_path(state).await;
    }

    let llm_mode = if mode == "local" {
        let model_enum = match model.as_deref() {
            Some("phi3") => LocalModel::Phi3Mini,
            Some("phi4") => LocalModel::Phi4,
            Some("qwen") => LocalModel::Qwen2_5B,
            _ => LocalModel::Phi3Mini,
        };

        LLMMode::Local {
            model: model_enum,
            device: DeviceType::Cpu,
            quantization: QuantizationType::Q4,
        }
    } else if mode == "external" {
        let api_provider = match provider.as_deref() {
            Some("openai") => ApiProvider::OpenAI,
            Some("anthropic") => ApiProvider::Anthropic,
            Some("openrouter") => ApiProvider::OpenRouter,
            Some("kimi") => ApiProvider::OpenAI, // Kimi uses OpenAI-compatible API
            Some("grok") => ApiProvider::Grok,
            Some("perplexity") => ApiProvider::Perplexity,
            Some("google") => ApiProvider::Google,
            Some("baseten") => ApiProvider::Baseten,
            _ => ApiProvider::OpenAI,
        };

        // Get API key (handle Kimi specially since it uses OpenAI-compatible API but separate key)
        let api_keys = state.api_keys.lock().unwrap_or_else(|e| e.into_inner());
        let api_key_opt = if provider.as_deref() == Some("kimi") {
            api_keys.kimi.clone()
        } else {
            match &api_provider {
                ApiProvider::OpenAI => api_keys.openai.clone(),
                ApiProvider::Anthropic => api_keys.anthropic.clone(),
                ApiProvider::OpenRouter => api_keys.openrouter.clone(),
                ApiProvider::Grok => api_keys.grok.clone(),
                ApiProvider::Perplexity => api_keys.perplexity.clone(),
                ApiProvider::Google => api_keys.google.clone(),
                ApiProvider::Baseten => api_keys.baseten.clone(),
                _ => None,
            }
        };

        let api_key = match api_key_opt {
            Some(key) if !key.trim().is_empty() => {
                tracing::info!(
                    "Found API key for {:?} (length: {})",
                    api_provider,
                    key.len()
                );
                key
            }
            _ => {
                tracing::info!("No API key found for {:?}", api_provider);
                return Err(format!("API key not configured for provider: {:?}. Please enter your API key in the settings.", api_provider));
            }
        };

        // Use appropriate default model for each provider
        let default_model = match &api_provider {
            ApiProvider::OpenAI => "gpt-4o-mini",
            ApiProvider::Anthropic => "claude-3-haiku-20240307",
            ApiProvider::OpenRouter => "deepseek/deepseek-chat",
            ApiProvider::Together => "meta-llama/Llama-3-8b-chat-hf",
            ApiProvider::Grok => "grok-2-1212",
            ApiProvider::Perplexity => "llama-3.1-sonar-small-128k-online",
            ApiProvider::Google => "gemini-2.0-flash-exp",
            _ => "gpt-4o-mini",
        };

        LLMMode::External {
            provider: api_provider,
            api_key,
            model: model.unwrap_or_else(|| default_model.to_string()),
        }
    } else {
        LLMMode::Disabled
    };

    // Update config
    let mut config = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    config.mode = llm_mode.clone();
    *state.config.lock().unwrap_or_else(|e| e.into_inner()) = config.clone();

    // Switch mode or create new manager
    tracing::info!("Attempting to switch to mode: {}", mode);
    let mut manager_lock = state.manager.write().await;

    if let Some(manager) = manager_lock.as_mut() {
        // Try to switch existing manager
        match manager.switch_mode(llm_mode).await {
            Ok(_) => {
                tracing::info!("Mode switched successfully on existing manager");
                Ok("Mode switched successfully".to_string())
            }
            Err(e) => {
                tracing::warn!("Failed to switch mode, creating new manager: {}", e);
                // Create new manager if switch fails
                let mut new_manager = LLMManager::new(config);
                match new_manager.initialize().await {
                    Ok(_) => {
                        *manager_lock = Some(new_manager);
                        tracing::info!("Created new manager successfully");
                        Ok("LLM initialized successfully".to_string())
                    }
                    Err(init_err) => {
                        tracing::warn!("Failed to initialize new manager: {}", init_err);
                        Err(format!("Failed to initialize LLM: {}", init_err))
                    }
                }
            }
        }
    } else {
        // No manager exists, create new one
        tracing::info!("No existing manager, creating new one");
        let mut new_manager = LLMManager::new(config);
        match new_manager.initialize().await {
            Ok(_) => {
                *manager_lock = Some(new_manager);
                tracing::info!("Created new manager successfully");
                Ok("LLM initialized successfully".to_string())
            }
            Err(e) => {
                tracing::warn!("Failed to initialize new manager: {}", e);
                Err(format!("Failed to initialize LLM: {}", e))
            }
        }
    }
}

/// Simple token estimator (words * 1.3 ≈ tokens)
fn estimate_tokens(text: &str) -> usize {
    (text.split_whitespace().count() as f32 * 1.3) as usize
}

/// Generate text with LLM
#[tauri::command]
pub async fn llm_generate(
    state: State<'_, LLMState>,
    context_state: State<'_, crate::context_commands::ContextState>,
    rag_state: State<'_, crate::rag_commands::RagState>,
    prompt: String,
) -> Result<String, String> {
    use crate::llm_response::LLMResponse;
    use std::time::Instant;

    let start_time = Instant::now();

    let manager_lock = state.manager.read().await;
    let manager = manager_lock.as_ref().ok_or("LLM not initialized")?;

    // Use context optimizer to classify query and build appropriate context
    use shodh_rag::rag::build_context_for_query;
    let (system_context, query_intent, context_tier) = build_context_for_query(&prompt);

    tracing::info!(
        "🔍 Query intent: {:?}, Context tier: {:?}",
        query_intent,
        context_tier
    );

    // Get conversation history from ContextAccumulator (DISABLED - context accumulator not migrated)
    let llm_context = String::new();

    // INTEGRATION: Retrieve relevant memories for additional context (only for non-greeting queries)
    let mut memory_context = String::new();
    if query_intent != shodh_rag::rag::ContextQueryIntent::Greeting {
        let memory_system_guard = rag_state.memory_system.read().await;
        if let Some(memory_system) = &*memory_system_guard {
            use shodh_rag::memory::{Query as MemQuery, RetrievalMode};

            let query = MemQuery {
                query_text: Some(prompt.clone()),
                query_embedding: None,
                retrieval_mode: RetrievalMode::Similarity,
                max_results: 5,
                importance_threshold: Some(0.5),
                time_range: None,
                experience_types: None,
            };

            if let Ok(memories) = memory_system.read().await.retrieve(&query) {
                if !memories.is_empty() {
                    memory_context = format!("\n## Relevant Past Context:\n");
                    for (i, memory) in memories.iter().take(3).enumerate() {
                        memory_context.push_str(&format!(
                            "{}. {}\n",
                            i + 1,
                            memory.experience.content
                        ));
                    }
                    tracing::info!(
                        "💡 Retrieved {} relevant memories ({} chars)",
                        memories.len(),
                        memory_context.len()
                    );
                }
            }
        }
        drop(memory_system_guard);
    }

    // Format with Qwen chat template
    let user_message = if memory_context.is_empty() {
        prompt.to_string()
    } else {
        format!("{}{}", prompt, memory_context)
    };

    let enhanced_prompt = format!(
        "<|im_start|>system\n{}<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
        system_context.trim(),
        user_message.trim()
    );

    tracing::info!(
        "🔧 System context: {} chars, Memory context: {} chars",
        system_context.len(),
        memory_context.len()
    );
    tracing::info!("📤 Full prompt length: {} chars", enhanced_prompt.len());

    // Intent-based max_tokens
    let max_tokens = match query_intent {
        shodh_rag::rag::ContextQueryIntent::Greeting => 50,
        shodh_rag::rag::ContextQueryIntent::SimpleQuestion => 150,
        shodh_rag::rag::ContextQueryIntent::DocumentQuery => 4096,
        shodh_rag::rag::ContextQueryIntent::CodeAnalysis => 2048,
        shodh_rag::rag::ContextQueryIntent::SystemQuery => 1000,
    };

    tracing::info!(
        "🎯 Intent-based max_tokens: {} (for {:?})",
        max_tokens,
        query_intent
    );

    let response = manager
        .generate_custom(&enhanced_prompt, max_tokens)
        .await
        .map_err(|e| e.to_string())?;

    let duration_ms = start_time.elapsed().as_millis() as u64;
    let input_tokens = estimate_tokens(&enhanced_prompt);
    let output_tokens = estimate_tokens(&response);

    tracing::info!(
        "📥 LLM response: {} chars, {} tokens in {:.2}s ({:.1} tok/s)",
        response.len(),
        output_tokens,
        duration_ms as f64 / 1000.0,
        output_tokens as f64 / (duration_ms as f64 / 1000.0)
    );

    // Return structured response with metadata
    let llm_response = LLMResponse::new(response, input_tokens, output_tokens, duration_ms)
        .with_intent(format!("{:?}", query_intent));

    Ok(serde_json::to_string(&llm_response).map_err(|e| e.to_string())?)
}

/// Stream generation (returns stream ID)
#[tauri::command]
pub async fn llm_generate_stream(
    state: State<'_, LLMState>,
    prompt: String,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let manager_lock = state.manager.read().await;
    let manager = manager_lock.as_ref().ok_or("LLM not initialized")?;

    let stream_id = uuid::Uuid::new_v4().to_string();

    // Get stream
    let mut stream = manager
        .generate_stream(&prompt)
        .await
        .map_err(|e| e.to_string())?;

    // Spawn task to emit tokens
    let stream_id_clone = stream_id.clone();
    tokio::spawn(async move {
        while let Some(token) = stream.next().await {
            let _ = app_handle.emit(
                "llm-token",
                &StreamToken {
                    stream_id: stream_id_clone.clone(),
                    token,
                    is_complete: false,
                },
            );
        }

        // Emit completion
        let _ = app_handle.emit(
            "llm-token",
            &StreamToken {
                stream_id: stream_id_clone,
                token: String::new(),
                is_complete: true,
            },
        );
    });

    Ok(stream_id)
}

/// Stream generation with RAG context
#[tauri::command]
pub async fn llm_generate_stream_with_rag(
    state: State<'_, LLMState>,
    rag_state: State<'_, crate::rag_commands::RagState>,
    query: String,
    context: Vec<String>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    tracing::info!("🚀 Starting INTEGRATED streaming generation with full context");
    tracing::info!("  Query: {:?}", query);
    tracing::info!("  RAG context items: {}", context.len());

    // Track timing
    let stream_start = std::time::Instant::now();

    let manager_lock = state.manager.read().await;
    let manager = manager_lock.as_ref().ok_or("LLM not initialized")?;

    // ============================================================================
    // FULL CONTEXT INTEGRATION - Memory + Conversation + Knowledge Graph + RAG
    // ============================================================================

    let mut full_context_parts = Vec::new();

    // 1. Get recent conversation history
    tracing::info!("  📖 Retrieving conversation history...");
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    if let Some(ref conv_mgr) = *conv_mgr_guard {
        if let Ok(Some(conversation)) = conv_mgr.get_last_conversation().await {
            let recent_messages: Vec<String> = conversation
                .messages
                .iter()
                .rev()
                .take(5) // Last 5 messages
                .rev()
                .map(|m| format!("{:?}: {}", m.role, m.content))
                .collect();

            if !recent_messages.is_empty() {
                full_context_parts.push(format!(
                    "## Recent Conversation\n{}",
                    recent_messages.join("\n")
                ));
                tracing::info!("    ✓ Added {} recent messages", recent_messages.len());
            }
        }
    }
    drop(conv_mgr_guard);

    // 2. Get relevant memories (with timeout to prevent deadlock)
    tracing::info!("  🧠 Retrieving relevant memories...");

    let memory_result = tokio::time::timeout(tokio::time::Duration::from_secs(3), async {
        let memory_guard = rag_state.memory_system.read().await;
        if let Some(ref memory_arc) = *memory_guard {
            let memory = memory_arc.read().await;

            let memory_query = shodh_rag::memory::Query {
                query_text: Some(query.clone()),
                query_embedding: None,
                retrieval_mode: shodh_rag::memory::RetrievalMode::Hybrid,
                max_results: 3,
                importance_threshold: Some(0.6),
                time_range: Some((
                    chrono::Utc::now() - chrono::Duration::hours(24),
                    chrono::Utc::now(),
                )),
                experience_types: None,
            };

            memory.retrieve(&memory_query).ok()
        } else {
            None
        }
    })
    .await;

    match memory_result {
        Ok(Some(memories)) if !memories.is_empty() => {
            let memory_context: Vec<String> = memories
                .iter()
                .map(|m| {
                    format!(
                        "Memory (importance: {:.2}): {}",
                        m.importance, m.experience.content
                    )
                })
                .collect();
            full_context_parts.push(format!(
                "## Relevant Memories\n{}",
                memory_context.join("\n")
            ));
            tracing::info!("    ✓ Added {} relevant memories", memories.len());
        }
        Ok(_) => {
            tracing::info!("    ℹ No relevant memories found");
        }
        Err(_) => {
            tracing::info!(
                "    ⚠ Memory retrieval timed out after 3s, continuing without memories"
            );
        }
    }
    // 3. Knowledge graph context (removed - not available in new API)
    {
        tracing::info!("    ⚠ Knowledge graph not available in this build");
    }

    // 4. Add RAG search results with context compression
    tracing::info!("  📄 Adding RAG search results...");
    if !context.is_empty() {
        let original_chars: usize = context.iter().map(|c| c.len()).sum();

        // Compress each context chunk to keep only query-relevant sentences
        // This cuts token usage by 50-70% while preserving signal
        let compressed: Vec<String> = context
            .iter()
            .map(|chunk| shodh_rag::rag::context_compressor::compress_chunk(chunk, &query, 8))
            .filter(|c| !c.is_empty())
            .collect();

        let compressed_chars: usize = compressed.iter().map(|c| c.len()).sum();
        let reduction = if original_chars > 0 {
            ((original_chars - compressed_chars) as f64 / original_chars as f64 * 100.0) as u32
        } else {
            0
        };

        full_context_parts.push(format!(
            "## Retrieved Documents\n{}",
            compressed.join("\n\n")
        ));
        tracing::info!(
            "    ✓ Added {} RAG results (compressed {}% : {} → {} chars)",
            compressed.len(),
            reduction,
            original_chars,
            compressed_chars
        );
    }

    // 5. Build system prompt with context awareness
    use shodh_rag::rag::context_optimizer::build_context_for_query;
    let (system_prompt, intent, tier) = build_context_for_query(&query);

    tracing::info!("  🎯 Query intent: {:?}, Context tier: {:?}", intent, tier);
    tracing::info!("  📦 Total context parts: {}", full_context_parts.len());

    // Build final prompt with ALL context
    let full_context_text = if !full_context_parts.is_empty() {
        format!("\n\n# CONTEXT\n\n{}\n\n", full_context_parts.join("\n\n"))
    } else {
        String::new()
    };

    let prompt = format!(
        "<|im_start|>system\n{}<|im_end|>\n<|im_start|>user\n{}Question: {}<|im_end|>\n<|im_start|>assistant\n",
        system_prompt.trim(),
        full_context_text,
        query.trim()
    );

    tracing::info!("  📏 Final prompt length: {} chars", prompt.len());

    // Calculate input tokens (word count * 1.3 for tokenization estimate)
    let input_tokens = (prompt.split_whitespace().count() as f32 * 1.3) as usize;
    tracing::info!("  📥 Input tokens (estimated): {}", input_tokens);
    tracing::info!("  ▶️  Starting LLM streaming...");

    let stream_id = uuid::Uuid::new_v4().to_string();
    let mut stream = manager
        .generate_stream(&prompt)
        .await
        .map_err(|e| e.to_string())?;

    let stream_id_clone = stream_id.clone();
    let start_time = std::time::Instant::now();
    let input_tokens_for_spawn = input_tokens; // Move into closure
    let manager_info = manager.info();
    tracing::info!("🔍 Manager info result: {:?}", manager_info);
    let model_name = manager_info
        .map(|info| info.model)
        .unwrap_or_else(|| "llm".to_string()); // Get model name before moving into closure
    tracing::info!("🔍 Model name extracted: {:?}", model_name);
    let stream_start_for_spawn = stream_start;

    tokio::spawn(async move {
        let mut output_token_count = 0;
        let mut accumulated_text = String::new();

        while let Some(token) = stream.next().await {
            output_token_count += 1;
            accumulated_text.push_str(&token);

            let _ = app_handle.emit(
                "llm-token",
                &StreamToken {
                    stream_id: stream_id_clone.clone(),
                    token,
                    is_complete: false,
                },
            );
        }

        // Calculate final metrics
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let duration_s = duration_ms as f64 / 1000.0;
        let tokens_per_sec = if duration_s > 0.0 {
            output_token_count as f64 / duration_s
        } else {
            0.0
        };

        tracing::info!("  ✅ Streaming complete:");
        tracing::info!("     📤 Output tokens: {}", output_token_count);
        tracing::info!("     ⏱️  Duration: {:.2}s", duration_s);
        tracing::info!("     ⚡ Speed: {:.1} tok/s", tokens_per_sec);

        // Log performance metrics
        let total_duration = stream_start_for_spawn.elapsed().as_secs_f64();
        tracing::info!(
            "     📊 Stream complete: {} input tokens, {} output tokens, {:.2}s total",
            input_tokens_for_spawn,
            output_token_count,
            total_duration
        );

        // Emit completion with metadata
        let _ = app_handle.emit(
            "llm-token",
            &StreamToken {
                stream_id: stream_id_clone.clone(),
                token: String::new(),
                is_complete: true,
            },
        );

        // Emit metadata event for frontend display
        let _ = app_handle.emit(
            "llm-metadata",
            &serde_json::json!({
                "stream_id": stream_id_clone,
                "model": model_name,
                "input_tokens": input_tokens_for_spawn,
                "output_tokens": output_token_count,
                "duration_ms": duration_ms,
                "duration_s": duration_s,
                "tokens_per_sec": tokens_per_sec,
                "total_chars": accumulated_text.len(),
                "metadata_line": format!(
                    "⏱️ {:.1}s | 📥 {} → 📤 {} tokens | ⚡ {:.1} tok/s",
                    duration_s, input_tokens_for_spawn, output_token_count, tokens_per_sec
                )
            }),
        );
    });

    Ok(stream_id)
}

/// Get LLM info
#[tauri::command]
pub async fn get_llm_info(state: State<'_, LLMState>) -> Result<LLMInfo, String> {
    tracing::info!("=== get_llm_info called ===");
    let manager_lock = state.manager.read().await;

    let manager = match manager_lock.as_ref() {
        Some(m) => m,
        None => {
            tracing::info!("No LLM manager found");
            return Err("LLM not initialized".to_string());
        }
    };

    let info = match manager.info() {
        Some(i) => i,
        None => {
            tracing::info!("No provider info available");
            return Err("No provider active".to_string());
        }
    };

    tracing::info!("Provider info: {:?}", info);
    let memory = manager.memory_usage();
    let config = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    tracing::info!("Config mode: {:?}", config.mode);

    Ok(LLMInfo {
        provider: info.name,
        model: info.model,
        context_window: info.context_window,
        supports_streaming: info.supports_streaming,
        is_local: info.is_local,
        memory_usage: memory.map(|m| MemoryInfo {
            ram_mb: m.ram_mb,
            vram_mb: m.vram_mb,
            model_size_mb: m.model_size_mb,
        }),
        mode: format!("{:?}", config.mode),
    })
}

/// Set API key for a provider. Keys are held in memory for the current session.
/// For persistent secure storage, integrate with the OS keychain
/// (Windows Credential Manager / macOS Keychain / Linux Secret Service).
#[tauri::command]
pub fn set_api_key(
    state: State<'_, LLMState>,
    provider: String,
    api_key: String,
) -> Result<(), String> {
    let mut api_keys = state.api_keys.lock().unwrap_or_else(|e| e.into_inner());

    match provider.as_str() {
        "openai" => api_keys.openai = Some(api_key),
        "anthropic" => api_keys.anthropic = Some(api_key),
        "openrouter" => api_keys.openrouter = Some(api_key),
        "kimi" => api_keys.kimi = Some(api_key),
        "grok" => api_keys.grok = Some(api_key),
        "perplexity" => api_keys.perplexity = Some(api_key),
        "google" => api_keys.google = Some(api_key),
        "baseten" => api_keys.baseten = Some(api_key),
        _ => return Err("Unknown provider".to_string()),
    }

    Ok(())
}

/// Check if model is cached
#[tauri::command]
pub async fn is_model_cached(state: State<'_, LLMState>, model: String) -> Result<bool, String> {
    // For custom models, check if path is set
    if model == "custom" {
        let has_custom = state
            .custom_model_path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        return Ok(has_custom);
    }

    let _model_enum = match model.as_str() {
        "phi3" => LocalModel::Phi3Mini,
        "phi4" => LocalModel::Phi4,
        "qwen" => LocalModel::Qwen2_5B,
        _ => return Err("Unknown model".to_string()),
    };

    // Always return true for now to prevent download loops
    Ok(true)
}

/// Download model
#[tauri::command]
pub async fn download_model(
    state: State<'_, LLMState>,
    model: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let model_enum = match model.as_str() {
        "phi3" => LocalModel::Phi3Mini,
        "phi4" => LocalModel::Phi4,
        "qwen" => LocalModel::Qwen2_5B,
        _ => return Err("Unknown model".to_string()),
    };

    let model_manager = state.model_manager.clone();

    // Download in background and emit progress
    tokio::spawn(async move {
        // Start download
        let download_result = model_manager.download_model(&model_enum).await;

        // Monitor progress
        loop {
            let progress = model_manager.get_progress().await;

            let has_error = progress.error.is_some();
            let is_complete = progress.is_complete;

            let _ = app_handle.emit(
                "model-download-progress",
                &ModelDownloadProgress {
                    model: model.clone(),
                    percentage: progress.percentage(),
                    downloaded_mb: (progress.downloaded / 1024 / 1024) as u32,
                    total_mb: (progress.total_size / 1024 / 1024) as u32,
                    is_complete: progress.is_complete,
                    error: progress.error,
                },
            );

            if is_complete || has_error {
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        if let Err(e) = download_result {
            let _ = app_handle.emit("model-download-error", &format!("Download failed: {}", e));
        }
    });

    Ok(())
}

/// Get model cache info
#[tauri::command]
pub async fn get_model_cache_info(state: State<'_, LLMState>) -> Result<CacheInfo, String> {
    let cached_models = state
        .model_manager
        .list_cached_models()
        .await
        .map_err(|e| e.to_string())?;

    let cache_size = state
        .model_manager
        .get_cache_size()
        .await
        .map_err(|e| e.to_string())?;

    Ok(CacheInfo {
        cached_models,
        total_size_mb: (cache_size / 1024 / 1024) as u32,
    })
}

/// Delete cached model
#[tauri::command]
pub async fn delete_cached_model(state: State<'_, LLMState>, model: String) -> Result<(), String> {
    let model_enum = match model.as_str() {
        "phi3" => LocalModel::Phi3Mini,
        "qwen" => LocalModel::Qwen2_5B,
        _ => return Err("Unknown model".to_string()),
    };

    state
        .model_manager
        .delete_model(&model_enum)
        .await
        .map_err(|e| e.to_string())
}

/// Update LLM config
#[tauri::command]
pub fn update_llm_config(
    state: State<'_, LLMState>,
    temperature: Option<f32>,
    max_tokens: Option<usize>,
    top_p: Option<f32>,
    top_k: Option<usize>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(temp) = temperature {
        config.temperature = temp;
    }
    if let Some(max) = max_tokens {
        config.max_tokens = max;
    }
    if let Some(p) = top_p {
        config.top_p = p;
    }
    if let Some(k) = top_k {
        config.top_k = k;
    }

    Ok(())
}

/// Get current custom model path
#[tauri::command]
pub fn get_custom_model_path(state: State<'_, LLMState>) -> Result<Option<String>, String> {
    let path = state
        .custom_model_path
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Ok(path.map(|p| p.to_string_lossy().to_string()))
}

// Helper functions and types

fn parse_llm_mode(mode: &str) -> Result<LLMMode, String> {
    match mode {
        "disabled" => Ok(LLMMode::Disabled),
        _ => Err("Invalid mode".to_string()),
    }
}

#[derive(Serialize, Clone)]
struct StreamToken {
    stream_id: String,
    token: String,
    is_complete: bool,
}

#[derive(Serialize)]
pub struct LLMInfo {
    provider: String,
    model: String,
    context_window: usize,
    supports_streaming: bool,
    is_local: bool,
    memory_usage: Option<MemoryInfo>,
    mode: String,
}

#[derive(Serialize)]
pub struct MemoryInfo {
    ram_mb: usize,
    vram_mb: Option<usize>,
    model_size_mb: usize,
}

#[derive(Serialize)]
pub struct ModelDownloadProgress {
    model: String,
    percentage: f32,
    downloaded_mb: u32,
    total_mb: u32,
    is_complete: bool,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct CacheInfo {
    cached_models: Vec<String>,
    total_size_mb: u32,
}
