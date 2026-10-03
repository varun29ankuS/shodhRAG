use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
    /// Response metadata reported by the chat engine (model, tokens, timings,
    /// search queries). Opaque to the backend; stored for the Ask view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Run record (status, wall time, steps observed from streaming events).
    /// Opaque to the backend; stored so the run summary survives reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<serde_json::Value>,
    /// Reduced agent transcript (steps, task list, usage, cited passages)
    /// of an answer produced by an agent session. Opaque to the backend;
    /// stored so the transcript survives reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<serde_json::Value>,
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

pub const CONVERSATIONS_FILE: &str = "conversations.json";

/// Serialises every read-modify-write of the conversations file (UI saves
/// and the agent's conversation tools).
static CONVERSATIONS_LOCK: Mutex<()> = Mutex::new(());

/// The saved conversations of one app data directory.
#[derive(Debug, Clone)]
pub struct ConversationStore {
    path: PathBuf,
}

impl ConversationStore {
    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(CONVERSATIONS_FILE),
        }
    }

    fn read_unlocked(&self) -> Result<Vec<ConversationRecord>, String> {
        match fs::read_to_string(&self.path) {
            Ok(data) => serde_json::from_str::<ConversationsFile>(&data)
                .map(|f| f.conversations)
                .map_err(|e| format!("Failed to parse conversations: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("Failed to read conversations: {e}")),
        }
    }

    fn write_unlocked(&self, conversations: Vec<ConversationRecord>) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("Failed to create app dir: {e}"))?;
        }
        let tmp_path = self.path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(&ConversationsFile { conversations })
            .map_err(|e| format!("Failed to serialize conversations: {e}"))?;
        fs::write(&tmp_path, &data).map_err(|e| format!("Failed to write temp file: {e}"))?;
        fs::rename(&tmp_path, &self.path).map_err(|e| format!("Failed to rename temp file: {e}"))
    }

    pub fn load(&self) -> Result<Vec<ConversationRecord>, String> {
        let _guard = CONVERSATIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.read_unlocked()
    }

    /// Apply `change` and save. Nothing is written when it fails.
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut Vec<ConversationRecord>) -> Result<R, String>,
    ) -> Result<R, String> {
        let _guard = CONVERSATIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conversations = self.read_unlocked()?;
        let result = change(&mut conversations)?;
        self.write_unlocked(conversations)?;
        Ok(result)
    }
}

fn store(app: &AppHandle) -> Result<ConversationStore, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    Ok(ConversationStore::in_dir(&dir))
}

#[tauri::command]
pub async fn load_conversations(app: AppHandle) -> Result<Vec<ConversationRecord>, String> {
    let mut conversations = store(&app)?.load()?;
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
    store(&app)?.update(|conversations| {
        if let Some(existing) = conversations.iter_mut().find(|c| c.id == conversation.id) {
            *existing = conversation;
        } else {
            conversations.push(conversation);
        }
        Ok(())
    })
}

#[tauri::command]
pub async fn delete_conversation(app: AppHandle, conversation_id: String) -> Result<(), String> {
    store(&app)?.update(|conversations| {
        conversations.retain(|c| c.id != conversation_id);
        Ok(())
    })
}

#[tauri::command]
pub async fn rename_conversation(
    app: AppHandle,
    conversation_id: String,
    new_title: String,
) -> Result<(), String> {
    store(&app)?.update(|conversations| {
        if let Some(conv) = conversations.iter_mut().find(|c| c.id == conversation_id) {
            conv.title = new_title;
            conv.updated_at = Utc::now().to_rfc3339();
        }
        Ok(())
    })
}

#[tauri::command]
pub async fn pin_conversation(
    app: AppHandle,
    conversation_id: String,
    pinned: bool,
) -> Result<(), String> {
    store(&app)?.update(|conversations| {
        if let Some(conv) = conversations.iter_mut().find(|c| c.id == conversation_id) {
            conv.pinned = pinned;
            conv.updated_at = Utc::now().to_rfc3339();
        }
        Ok(())
    })
}
