//! HTTP server for receiving Discord messages from the bridge

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
struct DiscordMessage {
    user_id: String,
    username: String,
    message: String,
    channel_id: String,
    guild_id: String,
}

#[derive(Debug, Serialize)]
struct DiscordResponse {
    response: String,
}

#[derive(Clone)]
struct AppState {
    rag_state: Arc<RwLock<RagState>>,
    llm_state: Arc<RwLock<LLMState>>,
    app_handle: Option<tauri::AppHandle>,
}

async fn handle_discord_message(
    AxumState(state): AxumState<AppState>,
    Json(payload): Json<DiscordMessage>,
) -> Result<Json<DiscordResponse>, (StatusCode, String)> {
    tracing::info!("Discord message from {}: {}", payload.username, payload.message);

    // Emit event to frontend
    if let Some(app_handle) = &state.app_handle {
        let _ = app_handle.emit("discord-message", serde_json::json!({
            "platform": "discord",
            "username": payload.username,
            "message": payload.message,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    // Process with timeout protection (60 seconds)
    let response_text = match tokio::time::timeout(
        std::time::Duration::from_secs(60),
        async {
            let rag_state_guard = state.rag_state.read().await;

            let context = ChatContext {
                agent_id: None,
                project: None,
                space_id: None,
                conversation_id: Some(format!("discord_{}", payload.channel_id)),
                conversation_history: None,
                max_results: None,
                streaming: None,
                custom_system_prompt: None,
            };

            let result = unified_chat_internal(
                &rag_state_guard,
                payload.message.clone(),
                Some(context),
                MessagePlatform::Discord,
                None,
            ).await;

            drop(rag_state_guard);

            match result {
                Ok(response) => {
                    let mut message = response.content.clone();

                    let meta = &response.metadata;
                    if let (Some(model), Some(input_tokens), Some(output_tokens), Some(duration_ms)) =
                        (&meta.model, meta.input_tokens, meta.output_tokens, meta.duration_ms) {
                        let duration_s = duration_ms as f64 / 1000.0;
                        let tok_per_s = output_tokens as f64 / duration_s;

                        message.push_str(&format!(
                            "\n\n{} | in:{} out:{} | {:.1}s | {:.1} tok/s",
                            model, input_tokens, output_tokens, duration_s, tok_per_s
                        ));
                    }

                    message
                },
                Err(e) => {
                    tracing::warn!("Error from unified_chat: {}", e);
                    format!("Sorry, I encountered an error: {}", e)
                }
            }
        }
    ).await {
        Ok(text) => text,
        Err(_) => {
            tracing::warn!("Discord request timeout after 60 seconds");
            "Sorry, the request timed out. Please try again with a simpler question.".to_string()
        }
    };

    // Emit response event to frontend
    if let Some(app_handle) = &state.app_handle {
        let _ = app_handle.emit("discord-response", serde_json::json!({
            "platform": "discord",
            "username": payload.username,
            "message": &response_text,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    tracing::info!("Sent response to {}", payload.username);

    Ok(Json(DiscordResponse { response: response_text }))
}

async fn health_check() -> &'static str {
    "Shodh Discord Bridge API is running"
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
        .route("/discord/chat", post(handle_discord_message))
        .layer(cors)
        .with_state(app_state);

    let addr = "127.0.0.1:3459";
    tracing::info!("Discord Bridge API listening on http://{}/discord/chat", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
