//! Tauri commands for WhatsApp Bot

use crate::whatsapp_bot::{WhatsAppBot, WhatsAppContact, WhatsAppMessage, BotResponse, ContactPreferences, ResponseStyle, BotStats};
use crate::rag_commands::RagState;
use crate::space_commands;
use crate::space_manager::SpaceManager;
use crate::whatsapp_http_server;
use tauri::{AppHandle, Manager, State};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::Utc;

pub struct WhatsAppBotState {
    pub bot: Arc<WhatsAppBot>,
    pub bridge_process: std::sync::Mutex<Option<std::process::Child>>,
    pub server_started: Arc<AtomicBool>,
}

impl Default for WhatsAppBotState {
    fn default() -> Self {
        Self {
            bot: Arc::new(WhatsAppBot::new()),
            bridge_process: std::sync::Mutex::new(None),
            server_started: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Start the WhatsApp HTTP server on first bot start (lazy initialization).
/// Uses AtomicBool to ensure it only starts once.
fn ensure_http_server(app: &AppHandle, bot_state: &WhatsAppBotState) {
    if bot_state.server_started.swap(true, Ordering::SeqCst) {
        return; // Already started
    }

    let rag_state = app.state::<RagState>();

    let app_data_dir = app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let bot_state_clone = WhatsAppBotState {
        bot: bot_state.bot.clone(),
        bridge_process: std::sync::Mutex::new(None),
        server_started: bot_state.server_started.clone(),
    };
    let rag_clone = RagState {
        rag: rag_state.rag.clone(),
        notes: std::sync::Mutex::new(Vec::new()),
        space_manager: std::sync::Mutex::new(SpaceManager::with_data_dir(app_data_dir)),
        conversation_manager: rag_state.conversation_manager.clone(),
        memory_system: rag_state.memory_system.clone(),
        personal_assistant: rag_state.personal_assistant.clone(),
        app_paths: rag_state.app_paths.clone(),
        rag_initialized: rag_state.rag_initialized.clone(),
        initialization_lock: rag_state.initialization_lock.clone(),
        artifact_store: rag_state.artifact_store.clone(),
        conversation_id: rag_state.conversation_id.clone(),
        agent_system: rag_state.agent_system.clone(),
        llm_manager: rag_state.llm_manager.clone(),
    };

    tauri::async_runtime::spawn(async move {
        if let Err(e) = whatsapp_http_server::start_server(bot_state_clone, rag_clone).await {
            tracing::error!("Failed to start WhatsApp HTTP server: {}", e);
        }
    });

    tracing::info!("WhatsApp HTTP server started (lazy init on bot start)");
}

/// Initialize WhatsApp bot and start the bridge automatically.
/// `engine` can be "baileys" (lightweight, WebSocket) or "webjs" (Puppeteer-based).
/// Defaults to "baileys" if not specified.
#[tauri::command]
pub async fn whatsapp_initialize(
    app: AppHandle,
    bot_state: State<'_, WhatsAppBotState>,
    bot_phone: String,
    engine: Option<String>,
) -> Result<String, String> {
    let bot = &bot_state.bot;
    let engine = engine.unwrap_or_else(|| "baileys".to_string());

    *bot.bot_phone.write().await = Some(bot_phone.clone());
    bot.set_active(true).await;

    let script = match engine.as_str() {
        "webjs" => "server.js",
        _ => "server-baileys.js",
    };
    let engine_label = match engine.as_str() {
        "webjs" => "whatsapp-web.js (Puppeteer)",
        _ => "Baileys (lightweight)",
    };

    let script_owned = script.to_string();
    let engine_label_owned = engine_label.to_string();

    // Kill any existing bridge process first
    {
        let mut proc_guard = bot_state.bridge_process.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = proc_guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    let possible_dirs = vec![
        std::env::current_dir().ok().and_then(|d| d.parent().map(|p| p.join("whatsapp-bridge"))),
        std::env::current_dir().ok().map(|d| d.join("whatsapp-bridge")),
        std::env::current_dir().ok().map(|d| d.join("../whatsapp-bridge")),
    ];

    let bridge_dir = possible_dirs.into_iter()
        .flatten()
        .find(|dir| dir.exists())
        .ok_or_else(|| "WhatsApp bridge directory not found. Ensure whatsapp-bridge/ folder exists.".to_string())?;

    tracing::info!("Starting WhatsApp bridge ({}) at: {:?}", engine_label, bridge_dir);

    // Install dependencies if needed
    if !bridge_dir.join("node_modules").exists() {
        tracing::info!("Installing bridge dependencies...");

        #[cfg(target_os = "windows")]
        let install_result = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            std::process::Command::new("cmd")
                .args(&["/C", "npm", "install"])
                .current_dir(&bridge_dir)
                .creation_flags(CREATE_NO_WINDOW)
                .output()
        };

        #[cfg(not(target_os = "windows"))]
        let install_result = std::process::Command::new("npm")
            .args(&["install"])
            .current_dir(&bridge_dir)
            .output();

        match install_result {
            Ok(output) if output.status.success() => {
                tracing::info!("Dependencies installed");
            },
            Ok(output) => {
                return Err(format!("npm install failed: {}", String::from_utf8_lossy(&output.stderr)));
            },
            Err(e) => {
                return Err(format!("Failed to run npm install: {} — ensure Node.js is in PATH", e));
            }
        }
    }

    tracing::info!("Starting bridge: node {}", script_owned);

    #[cfg(target_os = "windows")]
    let child = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("cmd")
            .args(&["/C", "node", &script_owned])
            .current_dir(&bridge_dir)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Failed to start bridge: {}", e))?
    };

    #[cfg(not(target_os = "windows"))]
    let child = std::process::Command::new("node")
        .args(&[&script_owned])
        .current_dir(&bridge_dir)
        .spawn()
        .map_err(|e| format!("Failed to start bridge: {}", e))?;

    // Store process handle so we can kill it later
    {
        let mut proc_guard = bot_state.bridge_process.lock().unwrap_or_else(|e| e.into_inner());
        *proc_guard = Some(child);
    }

    tracing::info!("WhatsApp bridge started ({})", engine_label);

    // Start the HTTP server if not already running (lazy init)
    ensure_http_server(&app, &bot_state);

    Ok(format!("WhatsApp bot initialized with {} engine. Bridge starting...", engine_label))
}

/// Add a contact to the bot
#[tauri::command]
pub async fn whatsapp_add_contact(
    bot_state: State<'_, WhatsAppBotState>,
    phone: String,
    name: String,
    assigned_space: Option<String>,
    is_authorized: bool,
) -> Result<String, String> {
    let contact = WhatsAppContact {
        phone: phone.clone(),
        name,
        assigned_space,
        is_authorized,
        conversation_id: None,
        preferences: ContactPreferences::default(),
    };

    bot_state.bot.add_contact(contact).await;
    Ok(format!("Contact {} added successfully", phone))
}

/// Update contact preferences
#[tauri::command]
pub async fn whatsapp_update_contact_preferences(
    bot_state: State<'_, WhatsAppBotState>,
    phone: String,
    language: Option<String>,
    response_style: Option<String>,
    max_response_length: Option<usize>,
    include_sources: Option<bool>,
) -> Result<String, String> {
    let bot = &bot_state.bot;

    if let Some(mut contact) = bot.get_contact(&phone).await {
        if let Some(lang) = language {
            contact.preferences.language = lang;
        }
        if let Some(style) = response_style {
            contact.preferences.response_style = match style.as_str() {
                "formal" => ResponseStyle::Formal,
                "casual" => ResponseStyle::Casual,
                "technical" => ResponseStyle::Technical,
                "concise" => ResponseStyle::Concise,
                _ => ResponseStyle::Casual,
            };
        }
        if let Some(length) = max_response_length {
            contact.preferences.max_response_length = length;
        }
        if let Some(sources) = include_sources {
            contact.preferences.include_sources = sources;
        }

        bot.add_contact(contact).await;
        Ok("Contact preferences updated".to_string())
    } else {
        Err("Contact not found".to_string())
    }
}

/// Assign a knowledge space to a contact
#[tauri::command]
pub async fn whatsapp_assign_space(
    bot_state: State<'_, WhatsAppBotState>,
    phone: String,
    space_id: String,
) -> Result<String, String> {
    let bot = &bot_state.bot;

    if let Some(mut contact) = bot.get_contact(&phone).await {
        contact.assigned_space = Some(space_id.clone());
        bot.add_contact(contact).await;
        Ok(format!("Space {} assigned to contact {}", space_id, phone))
    } else {
        Err("Contact not found".to_string())
    }
}

/// Process incoming WhatsApp message and generate RAG response
#[tauri::command]
pub async fn whatsapp_process_message(
    bot_state: State<'_, WhatsAppBotState>,
    rag_state: State<'_, RagState>,
    from: String,
    from_name: String,
    body: String,
    chat_id: String,
    is_group: bool,
) -> Result<BotResponse, String> {
    let bot = &bot_state.bot;

    // Check if bot is active
    if !bot.is_bot_active().await {
        return Err("Bot is not active".to_string());
    }

    // Get or auto-create contact (no authorization check - reply to everyone)
    let contact = match bot.get_contact(&from).await {
        Some(c) => c,
        None => {
            // Auto-create contact for anyone who messages
            let new_contact = WhatsAppContact {
                phone: from.clone(),
                name: from_name.clone(),
                assigned_space: None, // Global access
                is_authorized: true,  // Auto-authorize everyone
                conversation_id: None,
                preferences: ContactPreferences::default(),
            };
            bot.add_contact(new_contact.clone()).await;
            new_contact
        }
    };

    // Create message object
    let message = WhatsAppMessage {
        id: Uuid::new_v4().to_string(),
        from: from.clone(),
        from_name,
        body: body.clone(),
        timestamp: Utc::now(),
        chat_id,
        is_group,
    };

    // Get or create conversation
    let conversation_id = if let Some(conv) = bot.get_conversation(&from).await {
        conv.id
    } else {
        bot.start_conversation(from.clone()).await
    };

    // Add message to conversation
    bot.add_message(&conversation_id, message).await;

    // Perform RAG search to get context
    let search_results = if let Some(space_id) = &contact.assigned_space {
        space_commands::search_in_space(
            rag_state.clone(),
            space_id.clone(),
            body.clone(),
            5
        ).await.unwrap_or_default()
    } else {
        space_commands::search_global(
            rag_state.clone(),
            body.clone(),
            5
        ).await.unwrap_or_default()
    };

    // Build a professional response with proper formatting and citations
    let (response_text, sources, confidence) = if search_results.is_empty() {
        (
            format!(
                "I apologize, but I couldn't find relevant information in my knowledge base to answer your question about '{}'.\n\nCould you please rephrase your question or ask about something else I might be able to help with?",
                body.chars().take(50).collect::<String>()
            ),
            Vec::new(),
            0.0
        )
    } else {
        // Extract sources
        let mut sources_list = Vec::new();
        for result in &search_results {
            if let Some(file_path) = result.metadata.get("file_path") {
                if !sources_list.contains(file_path) {
                    sources_list.push(file_path.clone());
                }
            }
        }

        // Build comprehensive response with multiple relevant chunks
        let mut full_response = String::from("Based on the information in my knowledge base:\n\n");

        // Add the top results with inline citations
        for (idx, result) in search_results.iter().take(3).enumerate() {
            if idx > 0 {
                full_response.push_str("\n\n");
            }
            full_response.push_str(&result.snippet);

            // Add inline citation
            if let Some(file_path) = result.metadata.get("file_path") {
                let file_name = std::path::Path::new(file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file_path);
                full_response.push_str(&format!(" _[{}]_", file_name));
            }
        }

        // Add sources summary at the end
        if !sources_list.is_empty() {
            full_response.push_str("\n\n📚 *Sources:*\n");
            for (idx, source) in sources_list.iter().enumerate() {
                let file_name = std::path::Path::new(source)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(source);
                full_response.push_str(&format!("{}. {}\n", idx + 1, file_name));
            }
        }

        // Calculate average confidence
        let avg_confidence = search_results.iter().map(|r| r.score).sum::<f32>() / search_results.len() as f32;

        (full_response, sources_list, avg_confidence)
    };

    let bot_response = BotResponse {
        message: response_text,
        sources: sources.clone(),
        confidence,
        used_space: contact.assigned_space.clone(),
    };

    // Store response in conversation
    bot.add_response(&conversation_id, bot_response.clone()).await;

    Ok(bot_response)
}

/// List all contacts
#[tauri::command]
pub async fn whatsapp_list_contacts(
    bot_state: State<'_, WhatsAppBotState>,
) -> Result<Vec<WhatsAppContact>, String> {
    let contacts = bot_state.bot.contacts.read().await;
    Ok(contacts.values().cloned().collect())
}

/// Get bot statistics
#[tauri::command]
pub async fn whatsapp_get_stats(
    bot_state: State<'_, WhatsAppBotState>,
) -> Result<BotStats, String> {
    Ok(bot_state.bot.get_stats().await)
}

/// Activate/deactivate bot
#[tauri::command]
pub async fn whatsapp_set_active(
    bot_state: State<'_, WhatsAppBotState>,
    active: bool,
) -> Result<String, String> {
    bot_state.bot.set_active(active).await;
    Ok(format!("Bot is now {}", if active { "active" } else { "inactive" }))
}

/// Stop the WhatsApp bridge process
#[tauri::command]
pub async fn whatsapp_stop(
    bot_state: State<'_, WhatsAppBotState>,
) -> Result<String, String> {
    tracing::info!("Stopping WhatsApp bridge...");

    bot_state.bot.set_active(false).await;

    let child = {
        let mut proc_guard = bot_state.bridge_process.lock().unwrap_or_else(|e| e.into_inner());
        proc_guard.take()
    };

    if let Some(mut child) = child {
        let _ = child.kill();
        tokio::task::spawn_blocking(move || {
            let _ = child.wait();
        }).await.ok();
        tracing::info!("WhatsApp bridge stopped");
        Ok("WhatsApp bridge stopped".to_string())
    } else {
        tracing::info!("No WhatsApp bridge process was running");
        Ok("No bridge process was running".to_string())
    }
}

/// Remove a contact
#[tauri::command]
pub async fn whatsapp_remove_contact(
    bot_state: State<'_, WhatsAppBotState>,
    phone: String,
) -> Result<String, String> {
    let mut contacts = bot_state.bot.contacts.write().await;
    contacts.remove(&phone);
    Ok(format!("Contact {} removed", phone))
}

/// Get conversation history for a contact
#[tauri::command]
pub async fn whatsapp_get_conversation(
    bot_state: State<'_, WhatsAppBotState>,
    phone: String,
) -> Result<serde_json::Value, String> {
    if let Some(conv) = bot_state.bot.get_conversation(&phone).await {
        Ok(serde_json::to_value(conv).unwrap())
    } else {
        Err("No conversation found for this contact".to_string())
    }
}

/// Test the bot with a sample message (for debugging)
#[tauri::command]
pub async fn whatsapp_test_message(
    bot_state: State<'_, WhatsAppBotState>,
    rag_state: State<'_, RagState>,
    message: String,
) -> Result<BotResponse, String> {
    // Use a test contact
    whatsapp_process_message(
        bot_state,
        rag_state,
        "test_user".to_string(),
        "Test User".to_string(),
        message,
        "test_chat".to_string(),
        false,
    ).await
}
