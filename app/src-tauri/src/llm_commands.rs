//! Tauri commands for LLM integration

use serde::Serialize;
use shodh_rag::llm::{ApiProvider, LLMConfig, LLMManager, LLMMode};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

/// Ollama model used when none is chosen.
pub const OLLAMA_DEFAULT_MODEL: &str = "qwen3:4b";
use tokio::sync::RwLock as AsyncRwLock;

use crate::api_key_store;
use crate::audit_commands::AuditState;
use serde_json::{json, Value};
use shodh_rag::audit::{payload as audit_payload, AuditEventType, AuditRecord};

/// LLM state managed by Tauri
pub struct LLMState {
    pub manager: Arc<AsyncRwLock<Option<LLMManager>>>,
    pub config: Arc<Mutex<LLMConfig>>,
    pub api_keys: Arc<Mutex<ApiKeys>>,
    /// GGUF file chosen for local inference (llama.cpp).
    pub custom_model_path: Arc<Mutex<Option<PathBuf>>>,
}

/// In-memory provider API keys. Persisted copies live only in the OS
/// credential store (see `api_key_store`); values are never serialized to the
/// frontend.
#[derive(Default, Clone)]
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

impl ApiKeys {
    /// Mutable slot for a provider id, or `None` for an unknown provider.
    fn slot_mut(&mut self, provider: &str) -> Option<&mut Option<String>> {
        match provider {
            "openai" => Some(&mut self.openai),
            "anthropic" => Some(&mut self.anthropic),
            "openrouter" => Some(&mut self.openrouter),
            "kimi" => Some(&mut self.kimi),
            "grok" => Some(&mut self.grok),
            "perplexity" => Some(&mut self.perplexity),
            "google" => Some(&mut self.google),
            "baseten" => Some(&mut self.baseten),
            _ => None,
        }
    }

    /// The key of a provider id, if one is set. Never log the result.
    pub fn get(&self, provider: &str) -> Option<String> {
        let value = match provider {
            "openai" => &self.openai,
            "anthropic" => &self.anthropic,
            "openrouter" => &self.openrouter,
            "kimi" => &self.kimi,
            "grok" => &self.grok,
            "perplexity" => &self.perplexity,
            "google" => &self.google,
            "baseten" => &self.baseten,
            _ => return None,
        };
        value.clone()
    }

    fn is_configured(&self, provider: &str) -> bool {
        let value = match provider {
            "openai" => &self.openai,
            "anthropic" => &self.anthropic,
            "openrouter" => &self.openrouter,
            "kimi" => &self.kimi,
            "grok" => &self.grok,
            "perplexity" => &self.perplexity,
            "google" => &self.google,
            "baseten" => &self.baseten,
            _ => return false,
        };
        value.as_deref().is_some_and(|k| !k.trim().is_empty())
    }

    /// Provider ids that currently have a non-empty key.
    pub fn configured_providers(&self) -> Vec<String> {
        api_key_store::KEY_PROVIDERS
            .iter()
            .filter(|p| self.is_configured(p))
            .map(|p| (*p).to_string())
            .collect()
    }

    /// Fill providers that have no key yet with keys loaded from the OS
    /// credential store. Keys already in memory (set by the user or the
    /// environment in this session) are newer and are kept.
    pub fn merge_missing(&mut self, stored: Vec<(&'static str, String)>) -> usize {
        let mut merged = 0;
        for (provider, key) in stored {
            if let Some(slot) = self.slot_mut(provider) {
                if slot.as_deref().is_none_or(|k| k.trim().is_empty()) {
                    *slot = Some(key);
                    merged += 1;
                }
            }
        }
        merged
    }
}

impl std::fmt::Debug for ApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeys")
            .field("configured", &self.configured_providers())
            .field("values", &"[REDACTED]")
            .finish()
    }
}

/// Pick a local GGUF model file for llama.cpp.
#[tauri::command]
pub async fn browse_model_file(app_handle: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let file_path = app_handle
        .dialog()
        .file()
        .add_filter("GGUF models", &["gguf"])
        .blocking_pick_file();

    match file_path {
        Some(tauri_plugin_dialog::FilePath::Path(p)) => Ok(p.to_string_lossy().to_string()),
        Some(tauri_plugin_dialog::FilePath::Url(u)) => Ok(u.to_string()),
        None => Err("No file selected".to_string()),
    }
}

/// Set custom model path from user selection
#[tauri::command]
pub fn set_custom_model_path(
    state: State<'_, LLMState>,
    audit: State<'_, AuditState>,
    model_path: String,
) -> Result<String, String> {
    let result = set_custom_model_path_inner(&state, &model_path);
    if result.is_ok() {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({"action": "custom_model_path", "path": model_path}),
        ));
    }
    result
}

fn set_custom_model_path_inner(state: &LLMState, model_path: &str) -> Result<String, String> {
    let path = PathBuf::from(model_path);
    if !path.is_file() {
        return Err(format!("Model file does not exist: {}", model_path));
    }
    let is_gguf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gguf"));
    if !is_gguf {
        return Err("Select a .gguf model file (llama.cpp format).".to_string());
    }
    *state
        .custom_model_path
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(path);
    Ok(format!("GGUF model selected: {}", model_path))
}

/// Load the selected GGUF model with llama.cpp and make it the active LLM.
async fn activate_local_model(state: &LLMState) -> Result<String, String> {
    let model_path = state
        .custom_model_path
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| "Select a GGUF model file first.".to_string())?;

    let mut config = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    config.mode = LLMMode::Local {
        model_path: model_path.clone(),
    };
    *state.config.lock().unwrap_or_else(|e| e.into_inner()) = config.clone();

    let mut manager = LLMManager::new(config);
    manager.initialize().await.map_err(|e| {
        format!(
            "Failed to load {}: {e}. Make sure it is a valid GGUF model.",
            model_path.display()
        )
    })?;
    *state.manager.write().await = Some(manager);
    Ok(format!("Model loaded from {}", model_path.display()))
}

/// Switch LLM mode
#[tauri::command]
pub async fn switch_llm_mode(
    app: tauri::AppHandle,
    state: State<'_, LLMState>,
    audit: State<'_, AuditState>,
    mode: String,
    model: Option<String>,
    provider: Option<String>,
) -> Result<String, String> {
    // Local inference: the GGUF file picked in settings, run by llama.cpp.
    // ("custom" is the name older frontends used for the same thing.)
    if mode == "local" || mode == "custom" {
        let result = activate_local_model(&state).await;
        let config_mode = state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode
            .clone();
        let payload = match &config_mode {
            LLMMode::Local { .. } => audit_payload::model_switch(&config_mode),
            _ => {
                json!({"action": "model_switch", "mode": "local", "provider": "local", "model": null, "cloud": false})
            }
        };
        audit.record(model_switch_record(payload, &result));
        if result.is_ok() {
            crate::model_picker_commands::note_settings_switch(&app, &config_mode);
        }
        return result;
    }

    let llm_mode = if mode == "ollama" {
        // Ollama API (separate from native local)
        let ollama_model = model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or(OLLAMA_DEFAULT_MODEL);

        LLMMode::External {
            provider: ApiProvider::Ollama,
            api_key: "ollama".to_string(),
            model: ollama_model.to_string(),
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
            Some("ollama") => ApiProvider::Ollama,
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

        let api_key = if matches!(api_provider, ApiProvider::Ollama) {
            // Ollama runs locally, no API key required
            "ollama".to_string()
        } else {
            match api_key_opt {
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
            ApiProvider::Ollama => OLLAMA_DEFAULT_MODEL,
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

    // Provider and model only; the mode's API key is never read.
    let switch_payload = audit_payload::model_switch(&llm_mode);
    tracing::info!("Switching LLM mode to {}", mode);
    let result = activate_mode(&state, llm_mode.clone()).await;
    audit.record(model_switch_record(switch_payload, &result));
    if result.is_ok() {
        crate::model_picker_commands::note_settings_switch(&app, &llm_mode);
    }
    result
}

/// Make `llm_mode` the app's model: the config the agent sessions read when
/// they next start, and the in-process manager used by the other features.
pub(crate) async fn activate_mode(state: &LLMState, llm_mode: LLMMode) -> Result<String, String> {
    let config = {
        let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        config.mode = llm_mode.clone();
        config.clone()
    };

    // Switch mode or create new manager
    let mut manager_lock = state.manager.write().await;

    let result = if let Some(manager) = manager_lock.as_mut() {
        // Try to switch existing manager
        match manager.switch_mode(llm_mode).await {
            Ok(_) => {
                tracing::info!("Mode switched successfully on existing manager");
                Ok("Mode switched successfully".to_string())
            }
            Err(e) => {
                tracing::warn!("Failed to switch mode, creating new manager: {}", e);
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
    };
    drop(manager_lock);
    result
}

/// `settings_change` for a model switch, with its outcome.
fn model_switch_record(mut payload: Value, result: &Result<String, String>) -> AuditRecord {
    if let Value::Object(map) = &mut payload {
        map.insert("ok".to_string(), Value::Bool(result.is_ok()));
        if let Err(e) = result {
            map.insert("error".to_string(), Value::String(e.clone()));
        }
    }
    AuditRecord::new(AuditEventType::SettingsChange, payload)
}

/// Simple token estimator (words * 1.3 ≈ tokens)
fn estimate_tokens(text: &str) -> usize {
    (text.split_whitespace().count() as f32 * 1.3) as usize
}

/// Generate text with LLM
#[tauri::command]
pub async fn llm_generate(state: State<'_, LLMState>, prompt: String) -> Result<String, String> {
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

    // Format with Qwen chat template
    let user_message = prompt.to_string();

    let enhanced_prompt = format!(
        "<|im_start|>system\n{}<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
        system_context.trim(),
        user_message.trim()
    );

    tracing::info!("🔧 System context: {} chars", system_context.len());
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

    serde_json::to_string(&llm_response).map_err(|e| e.to_string())
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
    // CONTEXT INTEGRATION - RAG search results
    // ============================================================================

    let mut full_context_parts = Vec::new();

    // RAG search results with context compression
    tracing::info!("  📄 Adding RAG search results...");
    if !context.is_empty() {
        let original_chars: usize = context.iter().map(|c| c.len()).sum();

        // Compress each context chunk to keep only query-relevant sentences.
        // Use higher sentence limit for broad queries (list all, every, etc.)
        // to avoid discarding structured data (emails, names, IDs).
        let query_lower = query.to_lowercase();
        let is_broad = query_lower.contains("all ")
            || query_lower.contains("every ")
            || query_lower.contains("list ")
            || query_lower.contains("each ");
        let max_sentences = if is_broad { 15 } else { 8 };

        let compressed: Vec<String> = context
            .iter()
            .map(|chunk| {
                shodh_rag::rag::context_compressor::compress_chunk(chunk, &query, max_sentences)
            })
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
    let manager_lock = state.manager.read().await;

    let manager = match manager_lock.as_ref() {
        Some(m) => m,
        None => return Err("LLM not initialized".to_string()),
    };

    let info = match manager.info() {
        Some(i) => i,
        None => return Err("No provider active".to_string()),
    };

    let memory = manager.memory_usage();
    let mode = manager.config().mode.kind();

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
        mode: mode.to_string(),
    })
}

/// Store a provider API key in the OS credential store and make it available
/// to this session. The key is never echoed back to the frontend.
#[tauri::command]
pub async fn set_api_key(
    state: State<'_, LLMState>,
    audit: State<'_, AuditState>,
    provider: String,
    api_key: String,
) -> Result<(), String> {
    if !api_key_store::is_known_provider(&provider) {
        return Err(format!("Unknown provider: {provider}"));
    }
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("API key is empty".to_string());
    }

    // Persist first: if the credential store rejects the key, the session must
    // not silently hold a key that will be gone after restart.
    let to_store = api_key.clone();
    let store_provider = provider.clone();
    tokio::task::spawn_blocking(move || api_key_store::store(&store_provider, &to_store))
        .await
        .map_err(|e| format!("Credential store task failed: {e}"))??;

    {
        let mut api_keys = state.api_keys.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = api_keys.slot_mut(&provider) {
            *slot = Some(api_key);
        }
    }
    // The provider id only; the key value is never passed to the audit log.
    audit.record(AuditRecord::new(
        AuditEventType::SettingsChange,
        audit_payload::api_key_change(&provider, audit_payload::KeyAction::Set),
    ));
    Ok(())
}

/// Remove a provider API key from the OS credential store and this session.
#[tauri::command]
pub async fn delete_api_key(
    state: State<'_, LLMState>,
    audit: State<'_, AuditState>,
    provider: String,
) -> Result<(), String> {
    if !api_key_store::is_known_provider(&provider) {
        return Err(format!("Unknown provider: {provider}"));
    }

    let store_provider = provider.clone();
    tokio::task::spawn_blocking(move || api_key_store::remove(&store_provider))
        .await
        .map_err(|e| format!("Credential store task failed: {e}"))??;

    {
        let mut api_keys = state.api_keys.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = api_keys.slot_mut(&provider) {
            *slot = None;
        }
    }
    audit.record(AuditRecord::new(
        AuditEventType::SettingsChange,
        audit_payload::api_key_change(&provider, audit_payload::KeyAction::Deleted),
    ));
    Ok(())
}

/// Provider ids that have an API key available in this session. Never returns
/// key values.
#[tauri::command]
pub fn get_configured_providers(state: State<'_, LLMState>) -> Vec<String> {
    state
        .api_keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .configured_providers()
}

/// Send a short prompt to the active model and return its reply, so the
/// user can confirm a newly selected model works.
#[tauri::command]
pub async fn test_llm_inference(
    state: State<'_, LLMState>,
    prompt: String,
) -> Result<String, String> {
    const TEST_MAX_TOKENS: usize = 64;
    const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Prompt is empty".to_string());
    }
    let manager_lock = state.manager.read().await;
    let manager = manager_lock
        .as_ref()
        .filter(|m| m.info().is_some())
        .ok_or_else(|| "No model is active. Activate a model first.".to_string())?;
    let reply = tokio::time::timeout(
        TEST_TIMEOUT,
        manager.generate_custom(prompt, TEST_MAX_TOKENS),
    )
    .await
    .map_err(|_| {
        format!(
            "The model did not answer within {} seconds",
            TEST_TIMEOUT.as_secs()
        )
    })?
    .map_err(|e| e.to_string())?;
    let reply = reply.trim().to_string();
    if reply.is_empty() {
        return Err("The model returned an empty reply".to_string());
    }
    Ok(reply)
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

#[cfg(test)]
mod tests {
    use super::ApiKeys;

    #[test]
    fn merge_missing_keeps_session_keys_and_fills_gaps() {
        let mut keys = ApiKeys {
            openai: Some("session-key".to_string()),
            anthropic: Some("   ".to_string()),
            ..ApiKeys::default()
        };
        let merged = keys.merge_missing(vec![
            ("openai", "stored-openai".to_string()),
            ("anthropic", "stored-anthropic".to_string()),
            ("google", "stored-google".to_string()),
        ]);
        assert_eq!(merged, 2);
        assert_eq!(keys.openai.as_deref(), Some("session-key"));
        assert_eq!(keys.anthropic.as_deref(), Some("stored-anthropic"));
        assert_eq!(keys.google.as_deref(), Some("stored-google"));
        assert_eq!(
            keys.configured_providers(),
            vec!["openai", "anthropic", "google"]
        );
    }

    #[test]
    fn debug_output_never_contains_key_values() {
        let keys = ApiKeys {
            openrouter: Some("sk-or-secret-value".to_string()),
            ..ApiKeys::default()
        };
        let rendered = format!("{keys:?}");
        assert!(!rendered.contains("sk-or-secret-value"));
        assert!(rendered.contains("openrouter"));
    }
}
