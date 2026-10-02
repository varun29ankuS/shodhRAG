//! Thin Tauri wrappers for context tracking commands.
//! Business logic lives in shodh_rag::context.

use crate::rag_commands::RagState;
use serde::{Deserialize, Serialize};
use tauri::State;

/// Context state — session ID for this Tauri window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextState {
    pub session_id: String,
}

impl ContextState {
    pub fn new(session_id: String) -> Self {
        Self { session_id }
    }
}

#[tauri::command]
pub async fn track_user_message(
    message: String,
    state: State<'_, ContextState>,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    if let Some(ref conv_mgr) = *conv_mgr_guard {
        let current = conv_mgr
            .get_last_conversation()
            .await
            .map_err(|e| format!("Failed to get conversation: {}", e))?;

        if current.is_none() {
            conv_mgr
                .start_conversation(format!("Session {}", state.session_id))
                .await
                .map_err(|e| format!("Failed to start conversation: {}", e))?;
        }

        conv_mgr
            .add_message(shodh_rag::agent::MessageRole::User, message)
            .await
            .map_err(|e| format!("Failed to add message: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn track_assistant_message(
    message: String,
    _state: State<'_, ContextState>,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    if let Some(ref conv_mgr) = *conv_mgr_guard {
        conv_mgr
            .add_message(shodh_rag::agent::MessageRole::Assistant, message)
            .await
            .map_err(|e| format!("Failed to add message: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn track_search(
    query: String,
    results: Vec<String>,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let memory_guard = rag_state.memory_system.read().await;
    if let Some(ref memory_arc) = *memory_guard {
        shodh_rag::context::track_search(&query, &results, memory_arc).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn track_document_view(
    doc_id: String,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let memory_guard = rag_state.memory_system.read().await;
    if let Some(ref memory_arc) = *memory_guard {
        shodh_rag::context::track_document_view(&doc_id, memory_arc).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_context_summary(
    state: State<'_, ContextState>,
    rag_state: State<'_, RagState>,
) -> Result<String, String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    let conv_mgr = conv_mgr_guard.as_ref();

    let memory_guard = rag_state.memory_system.read().await;
    let memory = memory_guard.as_ref();

    shodh_rag::context::build_context_summary(&state.session_id, conv_mgr, memory).await
}

#[tauri::command]
pub async fn build_llm_context(rag_state: State<'_, RagState>) -> Result<String, String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    let conv_mgr = conv_mgr_guard.as_ref();

    let memory_guard = rag_state.memory_system.read().await;
    let memory = memory_guard.as_ref();

    shodh_rag::context::build_llm_context(conv_mgr, memory).await
}

#[tauri::command]
pub async fn save_session_to_memory(
    _session_name: String,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    if let Some(ref conv_mgr) = *conv_mgr_guard {
        conv_mgr
            .end_conversation()
            .await
            .map_err(|e| format!("Failed to save conversation: {}", e))?;
    } else {
        return Err("Conversation manager not initialized".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn restore_session_from_memory(
    session_name: String,
    rag_state: State<'_, RagState>,
) -> Result<String, String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    if let Some(ref conv_mgr) = *conv_mgr_guard {
        if let Some(context) = conv_mgr
            .continue_last_conversation()
            .await
            .map_err(|e| format!("Failed to restore session: {}", e))?
        {
            Ok(format!(
                "Restored session: {}\nLast topic: {}\nKey points: {}",
                session_name,
                context.last_topic,
                context.key_points.join(", ")
            ))
        } else {
            Err("No previous session found".to_string())
        }
    } else {
        Err("Conversation manager not initialized".to_string())
    }
}

#[tauri::command]
pub async fn start_task(
    name: String,
    category: String,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let memory_guard = rag_state.memory_system.read().await;
    if let Some(ref memory_arc) = *memory_guard {
        shodh_rag::context::track_task_start(&name, &category, memory_arc).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_full_context(rag_state: State<'_, RagState>) -> Result<serde_json::Value, String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    let conv_mgr = conv_mgr_guard.as_ref();

    let memory_guard = rag_state.memory_system.read().await;
    let memory = memory_guard.as_ref();

    shodh_rag::context::get_full_context(conv_mgr, memory).await
}

#[tauri::command]
pub async fn clear_context(rag_state: State<'_, RagState>) -> Result<(), String> {
    let conv_mgr_guard = rag_state.conversation_manager.read().await;
    if let Some(ref conv_mgr) = *conv_mgr_guard {
        conv_mgr
            .end_conversation()
            .await
            .map_err(|e| format!("Failed to end conversation: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn update_context(
    _interaction_type: String,
    _data: serde_json::Value,
    _state: State<'_, ContextState>,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn track_search_refinement(
    old_query: String,
    new_query: String,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let memory_guard = rag_state.memory_system.read().await;
    if let Some(ref memory_arc) = *memory_guard {
        shodh_rag::context::track_search_refinement(&old_query, &new_query, memory_arc).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn track_filter(
    filter_type: String,
    filter_value: String,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let memory_guard = rag_state.memory_system.read().await;
    if let Some(ref memory_arc) = *memory_guard {
        shodh_rag::context::track_filter(&filter_type, &filter_value, memory_arc).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn rebuild_memory_index(_rag_state: State<'_, RagState>) -> Result<String, String> {
    Ok("Vector indexing feature available in backend but requires MemorySystem refactor to enable.".to_string())
}

#[tauri::command]
pub async fn search_memory(
    query: String,
    max_results: usize,
    rag_state: State<'_, RagState>,
) -> Result<Vec<String>, String> {
    let memory_guard = rag_state.memory_system.read().await;
    if let Some(ref memory_arc) = *memory_guard {
        shodh_rag::context::search_memory(&query, max_results, memory_arc).await
    } else {
        Err("Memory system not initialized".to_string())
    }
}
