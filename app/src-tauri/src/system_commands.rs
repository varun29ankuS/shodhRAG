//! Thin Tauri wrapper for backend system operations
//! Just bridges frontend ↔ backend, all logic is in shodh_rag::system

use serde::{Deserialize, Serialize};
use shodh_rag::system::{command_executor::*, file_ops::*, os_integration::*};
use tauri::command;

/// Execute file system action
#[command]
pub async fn execute_file_action(action: FileSystemAction) -> Result<FileSystemResult, String> {
    match action {
        FileSystemAction::CreateFolders {
            base_path,
            structure,
        } => create_folder_structure(&base_path, &structure).map_err(|e| e.to_string()),
        FileSystemAction::CreateFile {
            path,
            content,
            overwrite,
        } => create_file(&path, &content, overwrite).map_err(|e| e.to_string()),
        FileSystemAction::Copy {
            source,
            destination,
        } => copy_path(&source, &destination).map_err(|e| e.to_string()),
        FileSystemAction::Move {
            source,
            destination,
        } => move_path(&source, &destination).map_err(|e| e.to_string()),
        FileSystemAction::Delete { path, recursive } => {
            delete_path(&path, recursive).map_err(|e| e.to_string())
        }
        FileSystemAction::ListDirectory { path, recursive } => {
            list_directory(&path, recursive).map_err(|e| e.to_string())
        }
    }
}

/// Execute command (PowerShell/Bash/System)
#[command]
pub async fn execute_command_action(action: CommandAction) -> Result<CommandResult, String> {
    execute_command(&action).map_err(|e| e.to_string())
}

/// Open path in file manager
#[command]
pub async fn open_file_manager(path: String) -> Result<String, String> {
    use std::path::PathBuf;
    let path_buf = PathBuf::from(path);

    open_in_file_manager(&path_buf)
        .map(|_| format!("Opened {:?} in file manager", path_buf))
        .map_err(|e| e.to_string())
}

/// Get system information
#[command]
pub async fn get_system_information() -> Result<SystemInfo, String> {
    get_system_info().map_err(|e| e.to_string())
}

/// List running processes
#[command]
pub async fn get_running_processes() -> Result<Vec<ProcessInfo>, String> {
    list_running_processes().map_err(|e| e.to_string())
}
