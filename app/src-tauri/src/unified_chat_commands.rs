//! Tauri commands for unified chat system

use tauri::{State, Manager};
use crate::rag_commands::RagState;
use crate::chat_engine::{ChatEngine, UserMessage, ChatContext, AssistantResponse, MessagePlatform, Artifact};
use crate::artifact_store::ArtifactStore;
use std::sync::Arc;
use tokio::sync::RwLock as AsyncRwLock;
use serde_json;

/// Internal unified chat function - can be called by both Tauri commands and HTTP servers
pub async fn unified_chat_internal(
    rag_state: &RagState,
    message: String,
    context: Option<ChatContext>,
    platform: MessagePlatform,
    app_handle: Option<tauri::AppHandle>,
) -> Result<AssistantResponse, String> {
    tracing::info!("🔵 unified_chat_internal called from {:?}: {}", platform, message.chars().take(50).collect::<String>());

    // Get components from state
    let rag = rag_state.rag.clone();

    // Get agent system (use directly from state)
    let agent_system = rag_state.agent_system.clone();

    // Get personal assistant (unwrap the nested Arc<RwLock<Option<Arc<RwLock<...>>>>>)
    let pa_guard = rag_state.personal_assistant.read().await;
    let personal_assistant = if let Some(ref pa_arc) = *pa_guard {
        pa_arc.clone()
    } else {
        drop(pa_guard);
        return Err("Personal assistant not initialized. Please initialize RAG first.".to_string());
    };
    drop(pa_guard);

    // Get memory system (unwrap nested structure)
    let mem_guard = rag_state.memory_system.read().await;
    let memory_system = if let Some(ref mem_arc) = *mem_guard {
        mem_arc.clone()
    } else {
        drop(mem_guard);
        return Err("Memory system not initialized. Please initialize RAG first.".to_string());
    };
    drop(mem_guard);

    // Get LLM manager (optional, use directly from state)
    let llm_manager = Some(rag_state.llm_manager.clone());

    // Debug: Check if LLM manager is initialized
    {
        let llm_guard = rag_state.llm_manager.read().await;
        tracing::info!("LLM Manager Status: {}", if llm_guard.is_some() { "Initialized" } else { "Not Initialized" });
    }

    // Wire LLM manager + RAG engine into AgentSystem for real tool-calling execution (lazy init)
    {
        let agent_sys_guard = rag_state.agent_system.read().await;
        if let Some(ref agent_sys_arc) = *agent_sys_guard {
            let mut agent_sys = agent_sys_arc.write().await;
            if agent_sys.llm_manager_ref_is_none() {
                agent_sys.set_llm_manager_ref(rag_state.llm_manager.clone());
                // Also inject RAG engine so agents can search documents
                agent_sys.set_rag_engine(rag_state.rag.clone()).await;
            }
        }
    }

    // Create chat engine
    let engine = ChatEngine::new(
        rag,
        agent_system,
        personal_assistant,
        llm_manager,
        memory_system,
    ).await;

    // Wire calendar store path so calendar tools can persist data
    if let Some(ref handle) = app_handle {
        if let Ok(app_dir) = handle.path().app_data_dir() {
            let _ = std::fs::create_dir_all(&app_dir);
            engine.set_calendar_path(app_dir.join("calendar_data.json")).await;
        }
    }

    // Create user message
    let user_msg = UserMessage {
        content: message,
        images: None,
        platform,
        timestamp: chrono::Utc::now(),
    };

    // Process message with optional streaming support via EventEmitter trait
    let emitter = app_handle.map(|h| crate::chat_engine::TauriEventEmitter::new(h));
    let emitter_ref: Option<&dyn shodh_rag::chat::EventEmitter> = emitter.as_ref().map(|e| e as &dyn shodh_rag::chat::EventEmitter);
    let response = engine.process_message(user_msg, context.unwrap_or_default(), emitter_ref).await
        .map_err(|e| format!("Failed to process message: {}", e))?;

    // Store artifacts in artifact store
    if !response.artifacts.is_empty() {
        let mut artifact_store = rag_state.artifact_store.write().await;
        let conversation_id = rag_state.conversation_id.read().await.clone()
            .unwrap_or_else(|| "default".to_string());

        for artifact in &response.artifacts {
            artifact_store.add_artifact(&conversation_id, artifact.clone());
            tracing::info!("📦 Stored artifact: {} ({})", artifact.title, artifact.id);
        }
    }

    Ok(response)
}

/// Unified chat command - single entry point for all chat functionality
#[tauri::command]
pub async fn unified_chat(
    app_handle: tauri::AppHandle,
    state: State<'_, RagState>,
    message: String,
    context: Option<ChatContext>,
) -> Result<AssistantResponse, String> {
    unified_chat_internal(&state, message, context, MessagePlatform::Desktop, Some(app_handle)).await
}

/// Apply artifact content to a file
#[tauri::command]
pub async fn apply_artifact_to_file(
    state: State<'_, RagState>,
    app_handle: tauri::AppHandle,
    artifact_id: String,
    file_path: Option<String>,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;
    use std::io::Write;

    tracing::info!("📝 apply_artifact_to_file: artifact_id={}, file_path={:?}", artifact_id, file_path);

    // Get artifact from store
    let artifact_store = state.artifact_store.read().await;
    let artifact = artifact_store.get_artifact(&artifact_id)
        .ok_or_else(|| format!("Artifact not found: {}", artifact_id))?
        .clone();

    drop(artifact_store);

    // Determine file path
    let target_path = if let Some(path) = file_path {
        path
    } else {
        // Prompt user to select location
        let file_path = app_handle
            .dialog()
            .file()
            .set_title("Save Artifact")
            .add_filter("All Files", &["*"])
            .blocking_save_file();

        match file_path {
            Some(path) => match path {
                tauri_plugin_dialog::FilePath::Path(p) => p.to_string_lossy().to_string(),
                tauri_plugin_dialog::FilePath::Url(u) => u.to_string(),
            },
            None => return Err("No file selected".to_string()),
        }
    };

    // Write artifact content to file
    let mut file = std::fs::File::create(&target_path)
        .map_err(|e| format!("Failed to create file: {}", e))?;

    file.write_all(artifact.content.as_bytes())
        .map_err(|e| format!("Failed to write content: {}", e))?;

    tracing::info!("✅ Artifact applied to: {}", target_path);

    Ok(target_path)
}

/// Update artifact content (creates new version)
#[tauri::command]
pub async fn update_artifact(
    state: State<'_, RagState>,
    artifact_id: String,
    new_content: String,
) -> Result<Artifact, String> {
    tracing::info!("✏️ update_artifact: artifact_id={}", artifact_id);

    let mut artifact_store = state.artifact_store.write().await;

    artifact_store.update_artifact(&artifact_id, new_content)
        .map_err(|e| format!("Failed to update artifact: {}", e))?;

    let updated = artifact_store.get_artifact(&artifact_id)
        .ok_or_else(|| "Artifact not found after update".to_string())?
        .clone();

    Ok(updated)
}

/// Get artifact history
#[tauri::command]
pub async fn get_artifact_history(
    state: State<'_, RagState>,
    artifact_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let artifact_store = state.artifact_store.read().await;

    let history = artifact_store.get_history(&artifact_id)
        .ok_or_else(|| format!("Artifact not found: {}", artifact_id))?;

    let history_json: Vec<serde_json::Value> = history.iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();

    Ok(history_json)
}

/// Get all artifacts for current conversation
#[tauri::command]
pub async fn get_conversation_artifacts(
    state: State<'_, RagState>,
) -> Result<Vec<Artifact>, String> {
    let conversation_id = state.conversation_id.read().await.clone()
        .unwrap_or_else(|| "default".to_string());

    let artifact_store = state.artifact_store.read().await;

    let artifacts = artifact_store.get_conversation_artifacts(&conversation_id)
        .into_iter()
        .cloned()
        .collect();

    Ok(artifacts)
}
