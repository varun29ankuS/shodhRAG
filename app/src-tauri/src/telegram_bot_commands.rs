//! Auto-start Telegram bot bridge

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

use crate::llm_commands::LLMState;
use crate::rag_commands::RagState;
use crate::space_manager::SpaceManager;
use crate::telegram_http_server;

pub struct TelegramBotState {
    pub process: Mutex<Option<Child>>,
    pub server_started: Arc<AtomicBool>,
}

/// Start the Telegram HTTP server on first bot start (lazy initialization).
/// Uses AtomicBool to ensure it only starts once.
fn ensure_http_server(app: &AppHandle, server_started: &Arc<AtomicBool>) {
    if server_started.swap(true, Ordering::SeqCst) {
        return; // Already started
    }

    let rag_state = app.state::<RagState>();
    let llm_state = app.state::<LLMState>();

    let app_data_dir = app
        .path()
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
        if let Err(e) =
            telegram_http_server::start_server(rag_clone, llm_clone, Some(app_handle)).await
        {
            tracing::error!("Failed to start Telegram HTTP server: {}", e);
        }
    });

    tracing::info!("Telegram HTTP server started (lazy init on bot start)");
}

#[tauri::command]
pub async fn start_telegram_bot(
    token: String,
    app: AppHandle,
    state: State<'_, TelegramBotState>,
) -> Result<(), String> {
    tracing::info!("Starting Telegram bot...");

    // Check if bot is already running and stop it gracefully
    {
        let mut process_guard = state.process.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = process_guard.take() {
            tracing::info!("Found existing Telegram bot process, stopping it gracefully...");
            let _ = child.kill();
            drop(child);
        }
    }
    tracing::info!("Waiting for process to fully terminate...");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Kill any orphaned node processes running telegram bot
    tracing::info!("Checking for orphaned Telegram bot processes...");
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(&["/F", "/FI", "WINDOWTITLE eq *telegram-bridge*"])
            .output();

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // Get the bridge directory - try multiple possible locations
    let possible_dirs = vec![
        std::env::current_dir()
            .ok()
            .map(|d| d.join("telegram-bridge")),
        std::env::current_dir()
            .ok()
            .map(|d| d.join("..").join("telegram-bridge")),
        std::env::current_dir()
            .ok()
            .map(|d| d.join("..").join("..").join("telegram-bridge")),
        Some(std::path::PathBuf::from("./telegram-bridge")),
        Some(std::path::PathBuf::from("../telegram-bridge")),
        Some(std::path::PathBuf::from("../../telegram-bridge")),
    ];

    let bridge_dir = possible_dirs
        .into_iter()
        .flatten()
        .find(|dir| dir.exists())
        .ok_or_else(|| {
            let current = std::env::current_dir().unwrap_or_default();
            format!(
                "telegram-bridge directory not found. Current dir: {:?}",
                current
            )
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
    tracing::info!("Starting Telegram bot process...");

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

    tracing::info!("Telegram bot started successfully!");
    Ok(())
}

#[tauri::command]
pub async fn stop_telegram_bot(state: State<'_, TelegramBotState>) -> Result<(), String> {
    tracing::info!("Stopping Telegram bot...");

    let child = {
        let mut process_guard = state.process.lock().unwrap_or_else(|e| e.into_inner());
        process_guard.take()
    };

    if let Some(mut child) = child {
        let _ = child.kill();
        // Wait for process to exit without blocking the async runtime
        tokio::task::spawn_blocking(move || {
            let _ = child.wait();
        })
        .await
        .ok();
        tracing::info!("Telegram bot stopped");
    } else {
        tracing::info!("No Telegram bot process was running");

        #[cfg(target_os = "windows")]
        {
            let _ = Command::new("taskkill")
                .args(&["/F", "/FI", "WINDOWTITLE eq *telegram-bridge*"])
                .output();
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn check_telegram_bot_status(state: State<'_, TelegramBotState>) -> Result<bool, String> {
    let mut process_guard = state.process.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(ref mut child) = *process_guard {
        match child.try_wait() {
            Ok(Some(_status)) => {
                *process_guard = None;
                Ok(false)
            }
            Ok(None) => Ok(true),
            Err(e) => {
                tracing::warn!("Failed to check Telegram bot process: {}", e);
                Ok(false)
            }
        }
    } else {
        Ok(false)
    }
}
