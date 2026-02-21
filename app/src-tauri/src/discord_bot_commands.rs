//! Auto-start Discord bot bridge

use std::process::{Command, Child};
use std::sync::Mutex;
use tauri::State;

pub struct DiscordBotState {
    pub process: Mutex<Option<Child>>,
}

#[tauri::command]
pub async fn start_discord_bot(
    token: String,
    state: State<'_, DiscordBotState>,
) -> Result<(), String> {
    tracing::info!("🚀 Starting Discord bot...");

    // Check if bot is already running
    {
        let process_guard = state.process.lock().unwrap();
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

    tracing::info!("📂 Using bridge directory: {:?}", bridge_dir);

    tracing::info!("📦 Installing dependencies...");

    // Install dependencies first (Windows-compatible)
    #[cfg(target_os = "windows")]
    let install_result = Command::new("cmd")
        .args(&["/C", "npm", "install"])
        .current_dir(&bridge_dir)
        .output();

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
    tracing::info!("🤖 Starting Discord bot process...");

    #[cfg(target_os = "windows")]
    let child = Command::new("cmd")
        .args(&["/C", "node", "server.js", &token])
        .current_dir(&bridge_dir)
        .spawn()
        .map_err(|e| format!("Failed to start bot: {}", e))?;

    #[cfg(not(target_os = "windows"))]
    let child = Command::new("node")
        .args(&["server.js", &token])
        .current_dir(&bridge_dir)
        .spawn()
        .map_err(|e| format!("Failed to start bot: {}", e))?;

    // Store the process
    let mut process_guard = state.process.lock().unwrap();
    *process_guard = Some(child);

    tracing::info!("✅ Discord bot started successfully!");
    Ok(())
}

#[tauri::command]
pub async fn stop_discord_bot(
    state: State<'_, DiscordBotState>,
) -> Result<(), String> {
    tracing::info!("🛑 Stopping Discord bot...");

    let mut process_guard = state.process.lock().unwrap();

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
    let mut process_guard = state.process.lock().unwrap();

    if let Some(ref mut child) = *process_guard {
        // Check if process is still running
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Process has exited
                *process_guard = None;
                Ok(false)
            }
            Ok(None) => {
                // Process is still running
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
