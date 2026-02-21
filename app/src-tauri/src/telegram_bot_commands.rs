//! Auto-start Telegram bot bridge

use std::process::{Command, Child};
use std::sync::Mutex;
use tauri::State;

pub struct TelegramBotState {
    pub process: Mutex<Option<Child>>,
}

#[tauri::command]
pub async fn start_telegram_bot(
    token: String,
    state: State<'_, TelegramBotState>,
) -> Result<(), String> {
    tracing::info!("🚀 Starting Telegram bot...");

    // Check if bot is already running and stop it gracefully
    {
        let mut process_guard = state.process.lock().unwrap();
        if let Some(mut child) = process_guard.take() {
            tracing::info!("⚠️ Found existing Telegram bot process, stopping it gracefully...");
            let _ = child.kill();
            drop(child);
            tracing::info!("⏳ Waiting for process to fully terminate...");
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    // Kill any orphaned node processes running telegram bot
    tracing::info!("🔍 Checking for orphaned Telegram bot processes...");
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(&["/F", "/FI", "WINDOWTITLE eq *telegram-bridge*"])
            .output();

        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    // Get the bridge directory - try multiple possible locations
    let possible_dirs = vec![
        std::env::current_dir().ok().map(|d| d.join("telegram-bridge")),
        std::env::current_dir().ok().map(|d| d.join("..").join("telegram-bridge")),
        std::env::current_dir().ok().map(|d| d.join("..").join("..").join("telegram-bridge")),
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
            format!("telegram-bridge directory not found. Current dir: {:?}", current)
        })?;

    tracing::info!("📂 Using bridge directory: {:?}", bridge_dir);

    tracing::info!("📦 Installing dependencies...");

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
            tracing::info!("✅ Dependencies installed");
        }
        Err(e) => {
            return Err(format!("Failed to run npm install: {}", e));
        }
    }

    // Start the bot process
    tracing::info!("🤖 Starting Telegram bot process...");

    #[cfg(target_os = "windows")]
    let child = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        Command::new("cmd")
            .args(&["/C", "node", "server.js", &token])
            .current_dir(&bridge_dir)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Failed to start bot: {}", e))?
    };

    #[cfg(not(target_os = "windows"))]
    let child = Command::new("node")
        .args(&["server.js", &token])
        .current_dir(&bridge_dir)
        .spawn()
        .map_err(|e| format!("Failed to start bot: {}", e))?;

    // Store the process
    let mut process_guard = state.process.lock().unwrap();
    *process_guard = Some(child);

    tracing::info!("✅ Telegram bot started successfully!");
    Ok(())
}

#[tauri::command]
pub async fn stop_telegram_bot(
    state: State<'_, TelegramBotState>,
) -> Result<(), String> {
    tracing::info!("Stopping Telegram bot...");

    let child = {
        let mut process_guard = state.process.lock().unwrap();
        process_guard.take()
    };

    if let Some(mut child) = child {
        let _ = child.kill();
        // Wait for process to exit without blocking the async runtime
        tokio::task::spawn_blocking(move || {
            let _ = child.wait();
        }).await.ok();
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
pub async fn check_telegram_bot_status(
    state: State<'_, TelegramBotState>,
) -> Result<bool, String> {
    let process_guard = state.process.lock().unwrap();
    Ok(process_guard.is_some())
}
