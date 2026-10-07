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
    /// The workspace the conversation belongs to; `None` is "No workspace". Conversations
    /// saved with a legacy space (`space_id`) are assigned once by the workspace import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// How the conversation's answers work; absent (or a mode this version
    /// does not know) is Research.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "known_mode"
    )]
    pub mode: Option<ConversationMode>,
    /// Side discussions that belong to the conversation but not to one of
    /// its messages (e.g. about a task), as the focus pop-out stores them.
    /// Opaque to the backend. Threads about a message live in that
    /// message's `metadata`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_threads: Option<serde_json::Value>,
}

/// How a conversation's answers work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationMode {
    /// Answers from the library and the web with Shodh's own tools.
    #[default]
    Research,
    /// The agent reads and changes the workspace's code folder (Code mode).
    Code,
}

/// A stored mode; one this version does not know must not stop every
/// conversation from loading.
fn known_mode<'de, D>(deserializer: D) -> Result<Option<ConversationMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let stored: Option<String> = Option::deserialize(deserializer)?;
    Ok(match stored.as_deref() {
        Some("research") => Some(ConversationMode::Research),
        Some("code") => Some(ConversationMode::Code),
        _ => None,
    })
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

/// The saved conversations, pinned first, then most recently updated. Conversations saved
/// with a legacy space are assigned to the workspace made from it first (once), so the UI
/// reads and saves records that already carry their workspace.
#[tauri::command]
pub async fn load_conversations(
    app: AppHandle,
    workspaces: tauri::State<'_, crate::workspace_commands::WorkspaceState>,
    rag: tauri::State<'_, crate::rag_commands::RagState>,
) -> Result<Vec<ConversationRecord>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    match crate::workspace_commands::import_legacy_spaces(&workspaces, &dir, &rag.rag).await {
        Ok(0) => {}
        // Views that listed workspaces before the import refresh.
        Ok(_) => crate::workspace_commands::broadcast_change(&app, None),
        // Retried on the next load; the conversations load either way.
        Err(e) => {
            tracing::warn!(target: "shodh::workspaces", error = %e, "legacy spaces not imported yet")
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_threads_round_trip_and_old_files_load() {
        let old = r#"{"conversations": [{"id": "c1", "title": "Taxes", "messages": [],
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z",
            "pinned": false}]}"#;
        let file: ConversationsFile = serde_json::from_str(old).unwrap();
        assert_eq!(file.conversations[0].focus_threads, None);
        // Not written back when absent.
        let text = serde_json::to_string(&file).unwrap();
        assert!(!text.contains("focusThreads"));

        let mut record = file.conversations[0].clone();
        let threads = serde_json::json!([{"id": "t1", "anchor": {"conversationId": "c1",
            "target": {"kind": "task", "label": "File ITR"}}, "turns": []}]);
        record.focus_threads = Some(threads.clone());
        let text = serde_json::to_string(&record).unwrap();
        assert!(text.contains("\"focusThreads\""));
        let back: ConversationRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back.focus_threads, Some(threads));

        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::in_dir(dir.path());
        store
            .update(|all| {
                all.push(record.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap()[0].focus_threads, record.focus_threads);
    }

    #[test]
    fn the_mode_is_kept_with_the_conversation() {
        let old = r#"{"id": "c1", "title": "Build", "messages": [],
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z",
            "pinned": false}"#;
        let record: ConversationRecord = serde_json::from_str(old).unwrap();
        // Saved before modes existed: Research, and nothing new is written.
        assert_eq!(record.mode.unwrap_or_default(), ConversationMode::Research);
        assert!(!serde_json::to_string(&record).unwrap().contains("mode"));

        let mut code = record.clone();
        code.mode = Some(ConversationMode::Code);
        let text = serde_json::to_string(&code).unwrap();
        assert!(text.contains(r#""mode":"code""#));

        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::in_dir(dir.path());
        store
            .update(|all| {
                all.push(code.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(store.load().unwrap()[0].mode, Some(ConversationMode::Code));

        let unknown = old.replace(r#""pinned": false"#, r#""pinned": false, "mode": "turbo""#);
        let read: ConversationRecord = serde_json::from_str(&unknown).unwrap();
        assert_eq!(read.mode, None);
    }
}
