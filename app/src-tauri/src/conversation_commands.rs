use chrono::Utc;
use serde::{Deserialize, Serialize};
use shodh_rag::harness::modes::AgentMode;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::AppHandle;

use crate::durable_file::{self, Loaded, Origin};
use crate::store_notices::{self, NoticeKind, StoreNotice};

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
    /// Set on a conversation that happened in a connector (a Slack thread, a Telegram
    /// chat): Shodh's answers there and the questions they answered, kept by the connector
    /// ([`crate::connectors::conversations`]). The app shows it read-only; only the
    /// connector adds to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ConversationSource>,
    /// A conversation started with "Continue in Ask" from a connector conversation: what
    /// it carries as context (opaque here; written and read by the Ask view, which attaches
    /// it to the first question as quoted, untrusted text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<serde_json::Value>,
    /// The file an "About <file>" chat is limited to (`{path, name}`), kept for the rest
    /// of the chat until the user removes it. Opaque here; written and read by the Ask
    /// view, which sends the path as each answer's search limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about_file: Option<serde_json::Value>,
    /// The mode the user picked for the conversation ([`AgentMode`]); absent
    /// follows the default in Settings. A mode this version does not know reads as
    /// Ask, never as a looser one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "known_agent_mode"
    )]
    pub agent_mode: Option<AgentMode>,
}

/// Where a connector conversation happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSource {
    /// `slack` or `telegram`.
    pub connector: String,
    /// The chat within the connector (a stable key: Slack workspace, channel and thread;
    /// Telegram chat and conversation).
    pub chat: String,
    /// What the sidebar groups it under: "Slack · #deals", "Telegram · you".
    pub label: String,
    /// The chat in the connector itself (a Slack permalink, a t.me link), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
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

/// A stored agent mode: one this version does not know (from a newer version) is
/// Ask, the mode that asks for every change, and never stops the conversations from
/// loading.
fn known_agent_mode<'de, D>(deserializer: D) -> Result<Option<AgentMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let stored: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    Ok(stored.map(|value| {
        value
            .as_str()
            .and_then(AgentMode::parse)
            .unwrap_or(AgentMode::Ask)
    }))
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

    /// The saved conversations and the file they came from: the backup
    /// when the file is damaged (reported as a notice). Fails when neither
    /// can be read; nothing is written then.
    fn read_unlocked(&self) -> Result<Loaded<Vec<ConversationRecord>>, String> {
        let loaded = durable_file::read_with_backup(
            &self.path,
            |text| {
                serde_json::from_str::<ConversationsFile>(text)
                    .map(|f| f.conversations)
                    .map_err(|e| e.to_string())
            },
            Vec::new,
        )
        .map_err(|e| format!("The saved conversations could not be read: {e}"))?;
        if let Origin::Backup { problem } = &loaded.origin {
            store_notices::report(StoreNotice::new(
                "conversations",
                NoticeKind::Restored,
                &self.path,
                "Conversations restored from the backup",
                format!(
                    "The conversations file could not be read ({problem}), so the copy kept before \
                     the last save was loaded. The latest change may be missing. The damaged file \
                     is kept next to it ({CONVERSATIONS_FILE}.damaged-…) when you next save."
                ),
            ));
        }
        Ok(loaded)
    }

    fn write_unlocked(
        &self,
        conversations: Vec<ConversationRecord>,
        origin: &Origin,
    ) -> Result<(), String> {
        let data = serde_json::to_string_pretty(&ConversationsFile { conversations })
            .map_err(|e| format!("Failed to serialize conversations: {e}"))?;
        durable_file::write_with_backup(&self.path, data.as_bytes(), origin).map_err(|e| {
            format!(
                "The conversations could not be saved to {}: {e}",
                self.path.display()
            )
        })
    }

    pub fn load(&self) -> Result<Vec<ConversationRecord>, String> {
        let _guard = CONVERSATIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.read_unlocked().map(|loaded| loaded.value)
    }

    /// Apply `change` and save. Nothing is written when it fails, or when
    /// the saved conversations cannot be read.
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut Vec<ConversationRecord>) -> Result<R, String>,
    ) -> Result<R, String> {
        self.update_if(|conversations| change(conversations).map(Some))?
            .ok_or_else(|| "The conversations were not changed.".to_string())
    }

    /// [`Self::update`] where `change` may decide to change nothing (`None`): then
    /// nothing is written.
    fn update_if<R>(
        &self,
        change: impl FnOnce(&mut Vec<ConversationRecord>) -> Result<Option<R>, String>,
    ) -> Result<Option<R>, String> {
        let _guard = CONVERSATIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Loaded {
            value: mut conversations,
            origin,
        } = self.read_unlocked()?;
        let Some(result) = change(&mut conversations)? else {
            return Ok(None);
        };
        self.write_unlocked(conversations, &origin)?;
        Ok(Some(result))
    }
}

/// Emitted with [`ConversationChanged`] after the messages of a conversation were written
/// ([`ConversationStore::write_turns`]): by the agent session (a question it accepted, an
/// answer as it is checked and when it ends) or by a window's message edit. Every open
/// view of the conversation reloads it from the store (`load_conversation`).
pub const CONVERSATION_CHANGED_EVENT: &str = "conversation_changed";

/// The payload of [`CONVERSATION_CHANGED_EVENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationChanged {
    pub id: String,
}

/// The title of a conversation nobody named yet.
pub const NEW_CHAT_TITLE: &str = "New Chat";

/// The title of a conversation from its first question: its first six words, with `...`
/// when there are more (as `autoTitle` in `hooks/useConversations.ts` named it).
pub fn auto_title(first_message: &str) -> String {
    let words: Vec<&str> = first_message.split_whitespace().collect();
    let mut title = words.iter().take(6).copied().collect::<Vec<_>>().join(" ");
    if words.len() > 6 {
        title.push_str("...");
    }
    if title.is_empty() {
        NEW_CHAT_TITLE.to_string()
    } else {
        title
    }
}

/// Now, as the window writes times (`new Date().toISOString()`).
fn now_text() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// How one message is written into a conversation ([`ConversationStore::write_turns`]).
#[derive(Debug, Clone)]
pub enum TurnWrite {
    /// Added at the end unless the conversation has a message with its id already (a
    /// question, or an answer as it starts): what is there is never set back.
    Insert(ConversationMessage),
    /// The answer of an agent run: its text and transcript replace those of the message
    /// with its id, keeping whatever else was added to it since (side threads, edited
    /// artifacts); added at the end when there is none.
    Answer(ConversationMessage),
    /// A message as a window wrote it (a notice, a moved citation, an edited artifact, a
    /// side thread, an answer that failed before the session took it): it replaces the
    /// message with its id, or is added at the end.
    Whole(ConversationMessage),
}

impl TurnWrite {
    fn message(&self) -> &ConversationMessage {
        match self {
            TurnWrite::Insert(m) | TurnWrite::Answer(m) | TurnWrite::Whole(m) => m,
        }
    }

    fn into_message(self) -> ConversationMessage {
        match self {
            TurnWrite::Insert(m) | TurnWrite::Answer(m) | TurnWrite::Whole(m) => m,
        }
    }
}

/// Applies `writes`, then `removed`, to the messages of `record`, and names it from its
/// first question while it is still unnamed.
fn apply_turns(record: &mut ConversationRecord, writes: Vec<TurnWrite>, removed: &[String]) {
    for write in writes {
        let at = record
            .messages
            .iter()
            .position(|m| m.id == write.message().id);
        match (write, at) {
            (TurnWrite::Insert(_), Some(_)) => {}
            (TurnWrite::Answer(answer), Some(index)) => {
                let existing = &mut record.messages[index];
                existing.content = answer.content;
                existing.transcript = answer.transcript;
            }
            (TurnWrite::Whole(message), Some(index)) => record.messages[index] = message,
            (write, None) => record.messages.push(write.into_message()),
        }
    }
    if !removed.is_empty() {
        record.messages.retain(|m| !removed.contains(&m.id));
    }
    if record.title == NEW_CHAT_TITLE {
        if let Some(first) = record.messages.iter().find(|m| m.role == "user") {
            record.title = auto_title(&first.content);
        }
    }
    record.updated_at = now_text();
}

/// A conversation created by its first message (the window's own save of the new chat
/// may not have reached the store yet).
fn new_record(id: &str) -> ConversationRecord {
    let now = now_text();
    ConversationRecord {
        id: id.to_string(),
        title: NEW_CHAT_TITLE.to_string(),
        messages: Vec::new(),
        created_at: now.clone(),
        updated_at: now,
        pinned: false,
        space_id: None,
        space_name: None,
        workspace_id: None,
        system_prompt: None,
        mode: None,
        focus_threads: None,
        source: None,
        seed: None,
        about_file: None,
        agent_mode: None,
    }
}

/// Why a turn is not written into a connector conversation.
const CONNECTOR_KEEPS_IT: &str = "This conversation happened in a connector (Slack or Telegram): \
     only the connector adds to it.";

impl ConversationStore {
    /// Writes messages of one conversation in a single read-modify-write under the
    /// conversations lock, so writers in different tasks (the agent session, a window,
    /// another device) never lose each other's turns: `writes` in order, then `removed`.
    /// A conversation not saved yet is created with `create` (a question, which may
    /// reach the store before the window's save of the new chat); without it nothing is
    /// written and `None` returned (a late answer of a conversation deleted meanwhile). A
    /// connector conversation is refused (its connector keeps it). Returns the
    /// conversation as it is now.
    pub fn write_turns(
        &self,
        conversation_id: &str,
        writes: Vec<TurnWrite>,
        removed: &[String],
        create: bool,
    ) -> Result<Option<ConversationRecord>, String> {
        if crate::connectors::conversations::is_connector_id(conversation_id) {
            return Err(CONNECTOR_KEEPS_IT.to_string());
        }
        self.update_if(|conversations| {
            let index = match conversations.iter().position(|c| c.id == conversation_id) {
                Some(index) => index,
                // Deleted meanwhile: a late answer does not bring it back.
                None if !create => return Ok(None),
                None => {
                    conversations.push(new_record(conversation_id));
                    conversations.len() - 1
                }
            };
            let record = &mut conversations[index];
            if record.source.is_some() {
                return Err(CONNECTOR_KEEPS_IT.to_string());
            }
            apply_turns(record, writes, removed);
            Ok(Some(record.clone()))
        })
    }

    /// Adds one message to a conversation atomically, or replaces the one with its id
    /// (an answer saved again as it progresses keeps its place): [`Self::write_turns`]
    /// with one whole message, as the tests' concurrent senders write.
    #[cfg(test)]
    pub fn append_turn(
        &self,
        conversation_id: &str,
        message: ConversationMessage,
    ) -> Result<ConversationRecord, String> {
        self.write_turns(conversation_id, vec![TurnWrite::Whole(message)], &[], true)?
            .ok_or_else(|| format!("The conversation {conversation_id} was not saved."))
    }

    /// The saved conversation with `id`, if there is one.
    pub fn get(&self, id: &str) -> Result<Option<ConversationRecord>, String> {
        Ok(self.load()?.into_iter().find(|c| c.id == id))
    }
}

/// Tells every open view that the messages of conversation `id` changed.
pub fn emit_changed(app: &AppHandle, id: &str) {
    use tauri::Emitter;
    if let Err(e) = app.emit(
        CONVERSATION_CHANGED_EVENT,
        ConversationChanged { id: id.to_string() },
    ) {
        tracing::warn!(error = %e, "emitting conversation_changed failed");
    }
}

pub(crate) fn store(app: &AppHandle) -> Result<ConversationStore, String> {
    let dir = crate::profile::app_data_dir(app)
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    Ok(ConversationStore::in_dir(&dir))
}

/// The saved conversations, pinned first, then most recently updated. Conversations saved
/// with a legacy space are assigned to the workspace made from it first (once), so the UI
/// reads and saves records that already carry their workspace. Once that import is done,
/// statements no workspace run can see are made global in the background
/// ([`crate::workspace_commands::settle_statement_scopes`]).
#[tauri::command]
pub async fn load_conversations(
    app: AppHandle,
    workspaces: tauri::State<'_, crate::workspace_commands::WorkspaceState>,
    rag: tauri::State<'_, crate::rag_commands::RagState>,
    memory: tauri::State<'_, crate::memory_commands::MemoryState>,
) -> Result<Vec<ConversationRecord>, String> {
    let dir = crate::profile::app_data_dir(&app)
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    let imported =
        crate::workspace_commands::import_legacy_spaces(&workspaces, &dir, &rag.rag).await;
    if imported.is_ok() {
        crate::workspace_commands::settle_statement_scopes_in_background(
            workspaces.inner().clone(),
            memory.inner().clone(),
        );
    }
    match imported {
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

/// Stores what the UI owns of `conversation` in the saved one with its id; a new one is
/// stored as sent.
///
/// - **An Ask chat's messages are never taken from a whole record.** They are written
///   message by message ([`ConversationStore::write_turns`]): the agent session saves its
///   turns itself, and a window or another device holding an older copy must never
///   overwrite turns it has not seen. Its title and pin have their own commands
///   (`rename_conversation`, `pin_conversation`, the agent's `organize_conversation`) and
///   the first question names it, so a copy still called "New Chat" never undoes a name.
///   Only the chat's settings are taken: its workspace, mode, instructions, side threads,
///   carried seed, the file it is about, and when it last changed.
/// - **A connector conversation is the connector's**: only its title, pin and side
///   threads are taken, and one that is no longer saved (forgotten) is never brought
///   back by a late save.
pub fn save_record(conversations: &mut Vec<ConversationRecord>, conversation: ConversationRecord) {
    match conversations.iter_mut().find(|c| c.id == conversation.id) {
        Some(existing) if existing.source.is_some() => {
            existing.title = conversation.title;
            existing.pinned = conversation.pinned;
            existing.focus_threads = conversation.focus_threads;
        }
        Some(existing) => {
            // An Ask chat never becomes a connector conversation through the UI (its
            // `source` is not taken either).
            existing.space_id = conversation.space_id;
            existing.space_name = conversation.space_name;
            existing.workspace_id = conversation.workspace_id;
            existing.system_prompt = conversation.system_prompt;
            existing.mode = conversation.mode;
            existing.focus_threads = conversation.focus_threads;
            existing.seed = conversation.seed;
            existing.about_file = conversation.about_file;
            existing.agent_mode = conversation.agent_mode;
            if conversation.updated_at > existing.updated_at {
                existing.updated_at = conversation.updated_at;
            }
        }
        None if crate::connectors::conversations::is_connector_id(&conversation.id)
            || conversation.source.is_some() =>
        {
            tracing::debug!(target: "shodh::connectors", "a save of a connector conversation that is no longer kept was ignored");
        }
        None => conversations.push(conversation),
    }
}

#[tauri::command]
pub async fn save_conversation(
    app: AppHandle,
    conversation: ConversationRecord,
) -> Result<(), String> {
    let store = store(&app)?;
    tokio::task::spawn_blocking(move || {
        store.update(|conversations| {
            save_record(conversations, conversation);
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One saved conversation, as it is stored now (`None` when there is none): what a view
/// reloads on [`CONVERSATION_CHANGED_EVENT`].
#[tauri::command]
pub async fn load_conversation(
    app: AppHandle,
    conversation_id: String,
) -> Result<Option<ConversationRecord>, String> {
    let store = store(&app)?;
    tokio::task::spawn_blocking(move || store.get(&conversation_id))
        .await
        .map_err(|e| e.to_string())?
}

/// Writes messages a window changed, each in place of the one with its id (or at the
/// end), and removes `removed`: a notice, an edited artifact or citation, a side thread,
/// a question and answer that failed before the agent session took them. Never the
/// whole list: turns written meanwhile by the agent session or another device stay.
#[tauri::command]
pub async fn save_conversation_messages(
    app: AppHandle,
    conversation_id: String,
    messages: Vec<ConversationMessage>,
    removed: Option<Vec<String>>,
) -> Result<(), String> {
    let store = store(&app)?;
    let id = conversation_id.clone();
    let removed = removed.unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        store.write_turns(
            &id,
            messages.into_iter().map(TurnWrite::Whole).collect(),
            &removed,
            true,
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    emit_changed(&app, &conversation_id);
    Ok(())
}

#[tauri::command]
pub async fn delete_conversation(app: AppHandle, conversation_id: String) -> Result<(), String> {
    store(&app)?.update(|conversations| {
        conversations.retain(|c| c.id != conversation_id);
        Ok(())
    })?;
    // Code mode's scratch folder for the conversation (its commands' home
    // and temporary files) goes with it. A file a running program still
    // holds can keep it from going; the conversation is deleted anyway.
    if let Ok(dir) = crate::profile::app_data_dir(&app) {
        let removed = tokio::task::spawn_blocking(move || {
            // Its kept command outputs (memory only) go now.
            shodh_rag::harness::command_output::forget(&conversation_id);
            // Its tokens-saved record goes too; the running total keeps them.
            if let Err(e) = shodh_rag::harness::compress::savings::SavingsStore::in_dir(&dir)
                .forget(&conversation_id)
            {
                tracing::warn!(error = %e, "a deleted conversation's tokens-saved record was not removed");
            }
            shodh_rag::harness::run_command::remove_scratch(&dir, &conversation_id)
        })
        .await;
        match removed {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "a deleted conversation's scratch folder was not removed")
            }
            Err(e) => {
                tracing::warn!(error = %e, "removing a deleted conversation's scratch folder did not finish")
            }
        }
    }
    Ok(())
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

    #[test]
    fn a_conversation_keeps_its_agent_mode_and_unknown_ones_ask() {
        let old = r#"{"id": "c1", "title": "Build", "messages": [],
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z",
            "pinned": false}"#;
        let record: ConversationRecord = serde_json::from_str(old).unwrap();
        assert_eq!(record.agent_mode, None);
        assert!(!serde_json::to_string(&record)
            .unwrap()
            .contains("agentMode"));
        for mode in AgentMode::ALL {
            let mut picked = record.clone();
            picked.agent_mode = Some(mode);
            let text = serde_json::to_string(&picked).unwrap();
            let read: ConversationRecord = serde_json::from_str(&text).unwrap();
            assert_eq!(read.agent_mode, Some(mode));
        }
        // Creator is a known mode now (kept above); these never were.
        for stored in [r#""bypass""#, r#""Creator""#, r#""Auto""#, "7", "{}"] {
            let text = old.replace(
                r#""pinned": false"#,
                &format!(r#""pinned": false, "agentMode": {stored}"#),
            );
            let read: ConversationRecord = serde_json::from_str(&text).unwrap();
            assert_eq!(read.agent_mode, Some(AgentMode::Ask), "{stored}");
        }
    }

    fn record(id: &str) -> ConversationRecord {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": id, "messages": [],
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z",
            "pinned": false
        }))
        .unwrap()
    }

    fn ids(records: &[ConversationRecord]) -> Vec<&str> {
        records.iter().map(|c| c.id.as_str()).collect()
    }

    fn add(store: &ConversationStore, id: &str) -> Result<(), String> {
        store.update(|all| {
            all.push(record(id));
            Ok(())
        })
    }

    #[test]
    fn a_truncated_file_loads_the_backup_with_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::in_dir(dir.path());
        add(&store, "c1").unwrap();
        add(&store, "c2").unwrap();
        let path = dir.path().join(CONVERSATIONS_FILE);
        let full = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, &full.as_bytes()[..full.len() / 2]).unwrap();

        assert_eq!(ids(&store.load().unwrap()), ["c1"]);
        let notice = store_notices::store_notices()
            .into_iter()
            .find(|n| n.store == "conversations" && n.path == path.display().to_string())
            .expect("a notice says the backup was used");
        assert_eq!(notice.kind, NoticeKind::Restored);

        // The next save starts from the backup and keeps the damaged file aside.
        add(&store, "c3").unwrap();
        assert_eq!(ids(&store.load().unwrap()), ["c1", "c3"]);
        let kept_aside = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(".damaged-"))
            })
            .expect("the damaged file is kept");
        assert_eq!(
            std::fs::read(kept_aside).unwrap(),
            &full.as_bytes()[..full.len() / 2]
        );
    }

    #[test]
    fn a_damaged_file_without_a_backup_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::in_dir(dir.path());
        let path = dir.path().join(CONVERSATIONS_FILE);
        std::fs::write(&path, r#"{"conversations": [{"id": "c1", "#).unwrap();

        let error = store.load().unwrap_err();
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(add(&store, "c2").is_err(), "saving refuses");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"conversations": [{"id": "c1", "#
        );
        assert!(!durable_file::backup_path(&path).exists());
        // Turns are refused too: nothing overwrites the file the history is still in.
        assert!(store
            .append_turn("c1", message("q1", "user", "Hello"))
            .is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"conversations": [{"id": "c1", "#
        );
    }

    fn message(id: &str, role: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: "2026-11-13T10:00:00.000Z".to_string(),
            artifacts: None,
            search_results: None,
            metadata: None,
            run: None,
            transcript: None,
        }
    }

    #[test]
    fn a_whole_record_save_keeps_the_stored_messages_and_name() {
        let mut all = vec![record("c1")];
        all[0].messages = vec![
            message("q1", "user", "Notice period?"),
            message("a1", "assistant", "60 days"),
        ];
        all[0].title = "Notice period?".to_string();
        // A window's older copy: no answer yet, still unnamed, with a new workspace.
        let mut stale = record("c1");
        stale.title = NEW_CHAT_TITLE.to_string();
        stale.messages = vec![message("q1", "user", "Notice period?")];
        stale.workspace_id = Some("w1".to_string());
        stale.mode = Some(ConversationMode::Code);
        stale.updated_at = "2026-11-13T11:00:00.000Z".to_string();
        save_record(&mut all, stale);
        assert_eq!(ids(&all), ["c1"]);
        let kept: Vec<&str> = all[0].messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(kept, ["q1", "a1"], "no turn is lost to a whole-record save");
        assert_eq!(all[0].title, "Notice period?");
        assert_eq!(
            all[0].workspace_id.as_deref(),
            Some("w1"),
            "settings are taken"
        );
        assert_eq!(all[0].mode, Some(ConversationMode::Code));
        assert_eq!(all[0].updated_at, "2026-11-13T11:00:00.000Z");
        // A new conversation is stored as sent.
        save_record(&mut all, record("c2"));
        assert_eq!(ids(&all), ["c1", "c2"]);
    }

    #[test]
    fn turns_are_written_by_id_and_name_the_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::in_dir(dir.path());
        // The first question reaches the store before the window's save of the new chat.
        let written = store
            .write_turns(
                "c1",
                vec![
                    TurnWrite::Insert(message("q1", "user", "one two three four five six seven")),
                    TurnWrite::Insert(message("a1", "assistant", "")),
                ],
                &[],
                true,
            )
            .unwrap()
            .expect("created");
        assert_eq!(written.title, "one two three four five six...");
        // An insert never sets back what is there; an answer keeps what was added to it.
        let mut edited = message("a1", "assistant", "draft");
        edited.metadata = Some(serde_json::json!({ "focusThreads": [] }));
        store.append_turn("c1", edited).unwrap();
        let mut answer = message("a1", "assistant", "Final.");
        answer.transcript = Some(serde_json::json!({ "status": "completed" }));
        store
            .write_turns(
                "c1",
                vec![
                    TurnWrite::Insert(message("q1", "user", "changed?")),
                    TurnWrite::Answer(answer),
                ],
                &[],
                false,
            )
            .unwrap();
        let saved = store.get("c1").unwrap().unwrap();
        assert_eq!(
            saved.messages[0].content,
            "one two three four five six seven"
        );
        assert_eq!(saved.messages[1].content, "Final.");
        assert!(saved.messages[1].metadata.is_some());
        assert!(saved.messages[1].transcript.is_some());
        // Removed by id; a conversation that is gone is not brought back.
        store
            .write_turns("c1", vec![], &["a1".to_string()], false)
            .unwrap();
        assert_eq!(store.get("c1").unwrap().unwrap().messages.len(), 1);
        assert!(store
            .write_turns(
                "gone",
                vec![TurnWrite::Answer(message("a9", "assistant", "x"))],
                &[],
                false
            )
            .unwrap()
            .is_none());
        assert!(store.get("gone").unwrap().is_none());
        // A connector's conversation is its connector's.
        assert!(store
            .append_turn("connector-slack-T1", message("q", "user", "x"))
            .is_err());
    }
}
