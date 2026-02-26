//! HTTP server for receiving WhatsApp messages from the bridge

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

use crate::whatsapp_commands::WhatsAppBotState;
use crate::rag_commands::RagState;
use crate::unified_chat_commands::unified_chat_internal;
use crate::chat_engine::{ChatContext, MessagePlatform};

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    from: String,
    from_name: String,
    body: String,
    chat_id: String,
    is_group: bool,
}

#[derive(Debug, Serialize)]
struct BridgeResponse {
    message: String,
    sources: Vec<String>,
    confidence: f32,
}

#[derive(Clone)]
struct AppState {
    bot_state: Arc<RwLock<WhatsAppBotState>>,
    rag_state: Arc<RwLock<RagState>>,
}

async fn handle_whatsapp_message(
    AxumState(state): AxumState<AppState>,
    Json(payload): Json<IncomingMessage>,
) -> Result<Json<BridgeResponse>, (StatusCode, String)> {
    tracing::info!("📨 Received WhatsApp message from {}: {}", payload.from, payload.body);

    // Get bot and RAG state
    let bot_state_guard = state.bot_state.read().await;
    let rag_state_guard = state.rag_state.read().await;

    // Process message using the existing command logic
    // We need to wrap it in a tauri::State-like structure
    // For now, let's call the logic directly

    let bot = &bot_state_guard.bot;

    // Check if bot is active
    if !bot.is_bot_active().await {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "Bot is not active".to_string()));
    }

    // Get or auto-create contact
    let contact = match bot.get_contact(&payload.from).await {
        Some(c) => c,
        None => {
            // Auto-create contact
            let new_contact = crate::whatsapp_bot::WhatsAppContact {
                phone: payload.from.clone(),
                name: payload.from_name.clone(),
                assigned_space: None,
                is_authorized: true,
                conversation_id: None,
                preferences: crate::whatsapp_bot::ContactPreferences::default(),
            };
            bot.add_contact(new_contact.clone()).await;
            new_contact
        }
    };

    // Create message
    let message = crate::whatsapp_bot::WhatsAppMessage {
        id: uuid::Uuid::new_v4().to_string(),
        from: payload.from.clone(),
        from_name: payload.from_name.clone(),
        body: payload.body.clone(),
        timestamp: chrono::Utc::now(),
        chat_id: payload.chat_id,
        is_group: payload.is_group,
    };

    // Get or create conversation
    let conversation_id = if let Some(conv) = bot.get_conversation(&payload.from).await {
        conv.id
    } else {
        bot.start_conversation(payload.from.clone()).await
    };

    // Add message to conversation
    bot.add_message(&conversation_id, message).await;

    // Create chat context
    let context = ChatContext {
        agent_id: None,
        project: None,
        space_id: contact.assigned_space.as_ref().map(|uuid| uuid.to_string()),
        conversation_id: Some(conversation_id.clone()),
        conversation_history: None,
        max_results: None,
        streaming: None,
        custom_system_prompt: None,
    };

    // Use unified chat system with full Memory + GraphRAG + LLM
    let result = unified_chat_internal(
        &rag_state_guard,
        payload.body.clone(),
        Some(context),
        MessagePlatform::WhatsApp,
        None, // No app_handle for HTTP servers (no streaming)
    ).await;

    let (response_text, sources, confidence) = match result {
        Ok(response) => {
            // Extract sources from citations (using title field as source name)
            let sources: Vec<String> = response.citations.iter()
                .map(|c| c.title.clone())
                .collect();

            // Calculate average confidence from citations (using score field)
            let confidence = if response.citations.is_empty() {
                0.8  // Default confidence for generated responses
            } else {
                response.citations.iter()
                    .map(|c| c.score)
                    .sum::<f32>() / response.citations.len() as f32
            };

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

            (message, sources, confidence)
        },
        Err(e) => {
            tracing::info!("❌ Error from unified_chat: {}", e);
            (format!("Sorry, I encountered an error: {}", e), Vec::new(), 0.0)
        }
    };

    // Store bot response
    let bot_response = crate::whatsapp_bot::BotResponse {
        message: response_text.clone(),
        sources: sources.clone(),
        confidence,
        used_space: contact.assigned_space,
    };

    bot.add_response(&conversation_id, bot_response).await;

    tracing::info!("✅ Sent response (confidence: {:.1}%)", confidence * 100.0);

    Ok(Json(BridgeResponse {
        message: response_text,
        sources,
        confidence,
    }))
}

async fn health_check() -> &'static str {
    "Shodh WhatsApp Bridge API is running"
}

pub async fn start_server(
    bot_state: WhatsAppBotState,
    rag_state: RagState,
) -> Result<(), Box<dyn std::error::Error>> {
    let app_state = AppState {
        bot_state: Arc::new(RwLock::new(bot_state)),
        rag_state: Arc::new(RwLock::new(rag_state)),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(health_check))
        .route("/whatsapp/process", post(handle_whatsapp_message))
        .layer(cors)
        .with_state(app_state);

    let addr = "127.0.0.1:3456";
    tracing::info!("🚀 WhatsApp Bridge API listening on http://{}", addr);
    tracing::info!("📡 Waiting for messages from WhatsApp bridge on port 3457\n");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
