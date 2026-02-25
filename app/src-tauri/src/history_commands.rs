//! Tauri commands for search and chat history management

use crate::chat_history::{ChatHistoryManager, ChatMessage, ExportFormat, MessageRole};
use crate::search_history::{SearchEntry, SearchHistoryManager, SearchSuggestion};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::State;

// ===== Search History Commands =====

#[tauri::command]
pub async fn add_search_history(
    manager: State<'_, Arc<Mutex<SearchHistoryManager>>>,
    query: String,
    space_id: Option<String>,
    result_count: usize,
) -> Result<(), String> {
    let mut manager = manager.lock().map_err(|e| e.to_string())?;
    manager.add_search(query, space_id, result_count)
}

#[tauri::command]
pub async fn get_search_history(
    manager: State<'_, Arc<Mutex<SearchHistoryManager>>>,
    space_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SearchEntry>, String> {
    let manager = manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.get_history(space_id.as_deref(), limit.unwrap_or(10)))
}

#[tauri::command]
pub async fn get_search_suggestions(
    manager: State<'_, Arc<Mutex<SearchHistoryManager>>>,
    query: String,
    space_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SearchSuggestion>, String> {
    let manager = manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.get_suggestions(&query, space_id.as_deref(), limit.unwrap_or(5)))
}

#[tauri::command]
pub async fn clear_search_history(
    manager: State<'_, Arc<Mutex<SearchHistoryManager>>>,
    space_id: Option<String>,
) -> Result<(), String> {
    let mut manager = manager.lock().map_err(|e| e.to_string())?;
    manager.clear_history(space_id.as_deref())
}

// ===== Chat History Commands =====

#[tauri::command]
pub async fn add_chat_message(
    manager: State<'_, Arc<Mutex<ChatHistoryManager>>>,
    space_id: Option<String>,
    role: String,
    content: String,
) -> Result<(), String> {
    let role = match role.as_str() {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        _ => return Err("Invalid role".to_string()),
    };

    let mut manager = manager.lock().map_err(|e| e.to_string())?;
    manager.add_message(space_id, role, content)
}

#[tauri::command]
pub async fn get_chat_history(
    manager: State<'_, Arc<Mutex<ChatHistoryManager>>>,
    space_id: Option<String>,
) -> Result<Vec<ChatMessage>, String> {
    let manager = manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.get_chat_history(space_id.as_deref()))
}

#[tauri::command]
pub async fn clear_chat_history(
    manager: State<'_, Arc<Mutex<ChatHistoryManager>>>,
    space_id: Option<String>,
) -> Result<(), String> {
    let mut manager = manager.lock().map_err(|e| e.to_string())?;
    manager.clear_chat_history(space_id.as_deref())
}

#[tauri::command]
pub async fn get_chat_sessions_summary(
    manager: State<'_, Arc<Mutex<ChatHistoryManager>>>,
) -> Result<Vec<HashMap<String, String>>, String> {
    let manager = manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.get_sessions_summary())
}

#[tauri::command]
pub async fn export_chat_history(
    manager: State<'_, Arc<Mutex<ChatHistoryManager>>>,
    space_id: Option<String>,
    format: String,
) -> Result<String, String> {
    let export_format = match format.as_str() {
        "json" => ExportFormat::Json,
        "markdown" => ExportFormat::Markdown,
        "text" => ExportFormat::Text,
        _ => return Err("Invalid export format".to_string()),
    };

    let manager = manager.lock().map_err(|e| e.to_string())?;
    manager.export_chat_history(space_id.as_deref(), export_format)
}

// ===== Combined Commands =====

#[tauri::command]
pub async fn search_with_history(
    search_manager: State<'_, Arc<Mutex<SearchHistoryManager>>>,
    rag_state: State<'_, crate::rag_commands::RagState>,
    query: String,
    space_id: Option<String>,
    max_results: usize,
) -> Result<Vec<crate::space_commands::SearchResult>, String> {
    // Perform the actual search first
    let response = if let Some(space_id) = space_id.clone() {
        crate::space_commands::search_in_space(
            rag_state.clone(),
            space_id.clone(),
            query.clone(),
            max_results,
        )
        .await?
    } else {
        crate::space_commands::search_global(rag_state, query.clone(), max_results).await?
    };

    // Add to search history with result count
    let result_count = response.len();
    let mut manager = search_manager.lock().map_err(|e| e.to_string())?;
    manager.add_search(query, space_id, result_count)?;

    Ok(response)
}
