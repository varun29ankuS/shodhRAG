//! Auto-start Discord bot bridge

use std::process::{Command, Child};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

use crate::rag_commands::RagState;
use crate::llm_commands::LLMState;
use crate::space_manager::SpaceManager;
use crate::discord_http_server;

pub struct DiscordBotState {
    pub process: Mutex<Option<Child>>,
    pub server_started: Arc<AtomicBool>,
}

/// Start the Discord HTTP server on first bot start (lazy initialization).
/// Uses AtomicBool to ensure it only starts once.
fn ensure_http_server(app: &AppHandle, server_started: &Arc<AtomicBool>) {
    if server_started.swap(true, Ordering::SeqCst) {
        return; // Already started
    }

    let rag_state = app.state::<RagState>();
    let llm_state = app.state::<LLMState>();

    let app_data_dir = app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let rag_clone = RagState {
        rag: rag_state.rag.clone(),
        notes: Mutex::new(Vec::new()),
        space_manager: Mutex::new(SpaceManager::with_data_dir(app_data_dir)),
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
    let llm_clone = LLMState {
        manager: llm_state.manager.clone(),
        model_manager: llm_state.model_manager.clone(),
        config: llm_state.config.clone(),
        api_keys: llm_state.api_keys.clone(),
        custom_model_path: llm_state.custom_model_path.clone(),
        custom_tokenizer_path: llm_state.custom_tokenizer_path.clone(),
    };

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = discord_http_server::start_server(rag_clone, llm_clone, Some(app_handle)).await {
            tracing::error!("Failed to start Discord HTTP server: {}", e);
        }
    });

    tracing::info!("Discord HTTP server started (lazy init on bot start)");
}

#[tauri::command]
pub async fn start_discord_bot(
    token: String,
    app: AppHandle,
    state: State<'_, DiscordBotState>,
) -> Result<(), String> {
    tracing::info!("Starting Discord bot...");

    // Check if bot is already running
    {
        let process_guard = state.process.lock().unwrap_or_else(|e| e.into_inner());
        if process_guard.is_some() {
            return Err("Discord bot is already running. Stop it first before starting a new instance.".to_string());
        }
    }

    // Get the bridge directory - try multiple possible locations
    let possible_dirs = vec![
        std::env::current_dir().ok().map(|d| d.join("discord-bridge")),
        std::env::current_dir().ok().map(|d| d.join("..").join("discord-bridge")),
        std::env::current_dir().ok().map(|d| d.join("..").join("..").join("discord-bridge")),
        Some(std::path::PathBuf::from("./discord-bridge")),
        Some(std::path::PathBuf::from("../discord-bridge")),
        Some(std::path::PathBuf::from("../../discord-bridge")),
    ];

    let bridge_dir = possible_dirs
        .into_iter()
        .flatten()
        .find(|dir| dir.exists())
        .ok_or_else(|| {
            let current = std::env::current_dir().unwrap_or_default();
            format!("discord-bridge directory not found. Current dir: {:?}", current)
        })?;

    tracing::info!("Using bridge directory: {:?}", bridge_dir);

    tracing::info!("Installing dependencies...");

    // Install dependencies first (Windows-compatible)
    #[cfg(target_os = "windows")]
    let install_result = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        Command::new("cmd")
            .args(&["/C", "npm", "install"])
            .current_dir(&bridge_dir)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    };

    #[cfg(not(target_os = "windows"))]
    let install_result = Command::new("npm")
        .arg("install")
        .current_dir(&bridge_dir)
        .output();

    match install_result {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Failed to install dependencies: {}", stderr));
            }
            tracing::info!("Dependencies installed");
        }
        Err(e) => {
            return Err(format!("Failed to run npm install: {}", e));
        }
    }

    // Start the bot process
    tracing::info!("Starting Discord bot process...");

    #[cfg(target_os = "windows")]
    let child = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        Command::new("cmd")
            .args(&["/C", "node", "server.js"])
            .env("BOT_TOKEN", &token)
            .current_dir(&bridge_dir)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Failed to start bot: {}", e))?
    };

    #[cfg(not(target_os = "windows"))]
    let child = Command::new("node")
        .args(&["server.js"])
        .env("BOT_TOKEN", &token)
        .current_dir(&bridge_dir)
        .spawn()
        .map_err(|e| format!("Failed to start bot: {}", e))?;

    // Store the process
    let mut process_guard = state.process.lock().unwrap_or_else(|e| e.into_inner());
    *process_guard = Some(child);

    // Start the HTTP server if not already running (lazy init)
    ensure_http_server(&app, &state.server_started);

    tracing::info!("Discord bot started successfully!");
    Ok(())
}

#[tauri::command]
pub async fn stop_discord_bot(
    state: State<'_, DiscordBotState>,
) -> Result<(), String> {
    tracing::info!("Stopping Discord bot...");

    let mut process_guard = state.process.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(mut child) = process_guard.take() {
        child.kill().map_err(|e| format!("Failed to kill process: {}", e))?;
        tracing::info!("Discord bot stopped");
        Ok(())
    } else {
        Err("No Discord bot process running".to_string())
    }
}

#[tauri::command]
pub async fn check_discord_bot_status(
    state: State<'_, DiscordBotState>,
) -> Result<bool, String> {
    let mut process_guard = state.process.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(ref mut child) = *process_guard {
        match child.try_wait() {
            Ok(Some(_status)) => {
                *process_guard = None;
                Ok(false)
            }
            Ok(None) => {
                Ok(true)
            }
            Err(e) => {
                tracing::warn!("Failed to check Discord bot process: {}", e);
                Ok(false)
            }
        }
    } else {
        Ok(false)
    }
}
