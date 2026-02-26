//! HTTP server for receiving Telegram messages from the bridge

use axum::{
    extract::State as AxumState,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use chrono;
use tauri::Emitter;

use crate::rag_commands::RagState;
use crate::llm_commands::LLMState;
use crate::unified_chat_commands::unified_chat_internal;
use crate::chat_engine::{ChatContext, MessagePlatform};

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    user_id: String,
    username: String,
    message: String,
    chat_id: String,
}

#[derive(Debug, Serialize)]
struct TelegramResponse {
    response: String,
}

#[derive(Clone)]
struct AppState {
    rag_state: Arc<RwLock<RagState>>,
    llm_state: Arc<RwLock<LLMState>>,
    app_handle: Option<tauri::AppHandle>,
}

async fn handle_telegram_message(
    AxumState(state): AxumState<AppState>,
    Json(payload): Json<TelegramMessage>,
) -> Result<Json<TelegramResponse>, (StatusCode, String)> {
    tracing::info!("📨 Telegram message from {}: {}", payload.username, payload.message);

    // Emit event to frontend to show in Chat UI
    if let Some(app_handle) = &state.app_handle {
        let _ = app_handle.emit("telegram-message", serde_json::json!({
            "platform": "telegram",
            "username": payload.username,
            "message": payload.message,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    // Get states with timeout protection
    let response_text = match tokio::time::timeout(
        std::time::Duration::from_secs(60),
        async {
            let rag_state_guard = state.rag_state.read().await;

            // Create chat context with session ID
            let context = ChatContext {
                agent_id: None,
                project: None,
                space_id: None,
                conversation_id: Some(format!("telegram_{}", payload.chat_id)),
                conversation_history: None,
                max_results: None,
                streaming: None,
                custom_system_prompt: None,
            };

            // Use unified chat system with full Memory + GraphRAG + LLM
            let result = unified_chat_internal(
                &rag_state_guard,
                payload.message.clone(),
                Some(context),
                MessagePlatform::Telegram,
                None, // No app_handle for HTTP servers (no streaming)
            ).await;

            drop(rag_state_guard);

            match result {
                Ok(response) => {
                    // Format response with metadata
                    let mut message = response.content.clone();

                    let meta = &response.metadata;
                    if let (Some(model), Some(input_tokens), Some(output_tokens), Some(duration_ms)) =
                        (&meta.model, meta.input_tokens, meta.output_tokens, meta.duration_ms) {
                        let duration_s = duration_ms as f64 / 1000.0;
                        let tok_per_s = output_tokens as f64 / duration_s;

                        message.push_str(&format!(
                            "\n\n🤖 {} │ ↓ {} │ ↑ {} │ ⏱ {:.1}s │ ⚡ {:.1} tok/s",
                            model, input_tokens, output_tokens, duration_s, tok_per_s
                        ));
                    }

                    message
                },
                Err(e) => {
                    tracing::info!("❌ Error from unified_chat: {}", e);
                    format!("Sorry, I encountered an error: {}", e)
                }
            }
        }
    ).await {
        Ok(text) => text,
        Err(_) => {
            tracing::info!("❌ Request timeout after 60 seconds");
            "Sorry, the request timed out. Please try again with a simpler question.".to_string()
        }
    };

    // Emit response event to frontend
    if let Some(app_handle) = &state.app_handle {
        let _ = app_handle.emit("telegram-response", serde_json::json!({
            "platform": "telegram",
            "username": payload.username,
            "message": &response_text,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    tracing::info!("✅ Sent response to {}", payload.username);

    Ok(Json(TelegramResponse { response: response_text }))
}

async fn health_check() -> &'static str {
    "Shodh Telegram Bridge API is running"
}

pub async fn start_server(
    rag_state: RagState,
    llm_state: LLMState,
    app_handle: Option<tauri::AppHandle>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = AppState {
        rag_state: Arc::new(RwLock::new(rag_state)),
        llm_state: Arc::new(RwLock::new(llm_state)),
        app_handle,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(health_check))
        .route("/telegram/chat", post(handle_telegram_message))
        .layer(cors)
        .with_state(app_state);

    let addr = "127.0.0.1:3458";
    tracing::info!("🚀 Telegram Bridge API listening on http://{}/telegram/chat", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
