use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_results: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    pub id: String,
    pub title: String,
    pub messages: Vec<ConversationMessage>,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationsFile {
    conversations: Vec<ConversationRecord>,
}

fn conversations_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;
    fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create app dir: {}", e))?;
    Ok(app_dir.join("conversations.json"))
}

fn read_conversations(app: &AppHandle) -> Result<Vec<ConversationRecord>, String> {
    let path = conversations_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read conversations: {}", e))?;
    let file: ConversationsFile =
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse conversations: {}", e))?;
    Ok(file.conversations)
}

fn write_conversations(
    app: &AppHandle,
    conversations: &[ConversationRecord],
) -> Result<(), String> {
    let path = conversations_path(app)?;
    let tmp_path = path.with_extension("json.tmp");
    let file = ConversationsFile {
        conversations: conversations.to_vec(),
    };
    let data = serde_json::to_string_pretty(&file)
        .map_err(|e| format!("Failed to serialize conversations: {}", e))?;
    fs::write(&tmp_path, &data).map_err(|e| format!("Failed to write temp file: {}", e))?;
    fs::rename(&tmp_path, &path).map_err(|e| format!("Failed to rename temp file: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn load_conversations(app: AppHandle) -> Result<Vec<ConversationRecord>, String> {
    let mut conversations = read_conversations(&app)?;
    // Sort by updated_at descending, pinned first
    conversations.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    Ok(conversations)
}

#[tauri::command]
pub async fn save_conversation(
    app: AppHandle,
    conversation: ConversationRecord,
) -> Result<(), String> {
    let mut conversations = read_conversations(&app)?;
    if let Some(existing) = conversations.iter_mut().find(|c| c.id == conversation.id) {
        *existing = conversation;
    } else {
        conversations.push(conversation);
    }
    write_conversations(&app, &conversations)
}

#[tauri::command]
pub async fn delete_conversation(app: AppHandle, conversation_id: String) -> Result<(), String> {
    let mut conversations = read_conversations(&app)?;
    conversations.retain(|c| c.id != conversation_id);
    write_conversations(&app, &conversations)
}

#[tauri::command]
pub async fn rename_conversation(
    app: AppHandle,
    conversation_id: String,
    new_title: String,
) -> Result<(), String> {
    let mut conversations = read_conversations(&app)?;
    if let Some(conv) = conversations.iter_mut().find(|c| c.id == conversation_id) {
        conv.title = new_title;
        conv.updated_at = Utc::now().to_rfc3339();
    }
    write_conversations(&app, &conversations)
}

#[tauri::command]
pub async fn pin_conversation(
    app: AppHandle,
    conversation_id: String,
    pinned: bool,
) -> Result<(), String> {
    let mut conversations = read_conversations(&app)?;
    if let Some(conv) = conversations.iter_mut().find(|c| c.id == conversation_id) {
        conv.pinned = pinned;
        conv.updated_at = Utc::now().to_rfc3339();
    }
    write_conversations(&app, &conversations)
}
