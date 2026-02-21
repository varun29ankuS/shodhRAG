//! Space-specific chat history management

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use chrono::Utc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub space_id: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: String,
    pub last_updated: String,
    pub metadata: HashMap<String, String>,
}

pub struct ChatHistoryManager {
    sessions: HashMap<String, ChatSession>, // space_id -> session
    global_session: ChatSession,
    max_messages_per_session: usize,
    storage_path: PathBuf,
}

impl ChatHistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self, String> {
        let app_dir = app_handle.path()
            .app_data_dir()
            .map_err(|e| format!("Failed to get app data directory: {}", e))?;
        
        let storage_path = app_dir.join("chat_history.json");
        
        let mut manager = Self {
            sessions: HashMap::new(),
            global_session: ChatSession {
                space_id: "global".to_string(),
                messages: Vec::new(),
                created_at: Utc::now().to_rfc3339(),
                last_updated: Utc::now().to_rfc3339(),
                metadata: HashMap::new(),
            },
            max_messages_per_session: 100,
            storage_path,
        };
        
        // Load existing chat history
        manager.load_history()?;
        
        Ok(manager)
    }
    
    /// Add a message to chat history
    pub fn add_message(&mut self, space_id: Option<String>, role: MessageRole, content: String) -> Result<(), String> {
        let message = ChatMessage {
            role,
            content,
            timestamp: Utc::now().to_rfc3339(),
            metadata: None,
        };
        
        if let Some(space_id) = space_id {
            // Add to space-specific session
            let session = self.sessions.entry(space_id.clone()).or_insert_with(|| {
                ChatSession {
                    space_id: space_id.clone(),
                    messages: Vec::new(),
                    created_at: Utc::now().to_rfc3339(),
                    last_updated: Utc::now().to_rfc3339(),
                    metadata: HashMap::new(),
                }
            });
            
            session.messages.push(message);
            session.last_updated = Utc::now().to_rfc3339();
            
            // Trim old messages if needed
            if session.messages.len() > self.max_messages_per_session {
                let remove_count = session.messages.len() - self.max_messages_per_session;
                session.messages.drain(0..remove_count);
            }
        } else {
            // Add to global session
            self.global_session.messages.push(message);
            self.global_session.last_updated = Utc::now().to_rfc3339();
            
            if self.global_session.messages.len() > self.max_messages_per_session {
                let remove_count = self.global_session.messages.len() - self.max_messages_per_session;
                self.global_session.messages.drain(0..remove_count);
            }
        }
        
        // Save to disk
        self.save_history()?;
        
        Ok(())
    }
    
    /// Get chat history for a space
    pub fn get_chat_history(&self, space_id: Option<&str>) -> Vec<ChatMessage> {
        if let Some(space_id) = space_id {
            self.sessions.get(space_id)
                .map(|s| s.messages.clone())
                .unwrap_or_default()
        } else {
            self.global_session.messages.clone()
        }
    }
    
    /// Clear chat history for a space
    pub fn clear_chat_history(&mut self, space_id: Option<&str>) -> Result<(), String> {
        if let Some(space_id) = space_id {
            if let Some(session) = self.sessions.get_mut(space_id) {
                session.messages.clear();
                session.last_updated = Utc::now().to_rfc3339();
            }
        } else {
            self.global_session.messages.clear();
            self.global_session.last_updated = Utc::now().to_rfc3339();
        }
        
        self.save_history()?;
        Ok(())
    }
    
    /// Get all sessions summary
    pub fn get_sessions_summary(&self) -> Vec<HashMap<String, String>> {
        let mut summaries = Vec::new();
        
        // Add global session
        summaries.push({
            let mut summary = HashMap::new();
            summary.insert("space_id".to_string(), "global".to_string());
            summary.insert("message_count".to_string(), self.global_session.messages.len().to_string());
            summary.insert("last_updated".to_string(), self.global_session.last_updated.clone());
            summary
        });
        
        // Add space sessions
        for (space_id, session) in &self.sessions {
            let mut summary = HashMap::new();
            summary.insert("space_id".to_string(), space_id.clone());
            summary.insert("message_count".to_string(), session.messages.len().to_string());
            summary.insert("last_updated".to_string(), session.last_updated.clone());
            summaries.push(summary);
        }
        
        summaries
    }
    
    /// Export chat history for a space
    pub fn export_chat_history(&self, space_id: Option<&str>, format: ExportFormat) -> Result<String, String> {
        let messages = self.get_chat_history(space_id);
        
        match format {
            ExportFormat::Json => {
                serde_json::to_string_pretty(&messages)
                    .map_err(|e| format!("Failed to export as JSON: {}", e))
            }
            ExportFormat::Markdown => {
                let mut markdown = String::new();
                markdown.push_str(&format!("# Chat History - {}\n\n", 
                    space_id.unwrap_or("Global")));
                
                for message in messages {
                    let role_emoji = match message.role {
                        MessageRole::User => "👤",
                        MessageRole::Assistant => "🤖",
                        MessageRole::System => "⚙️",
                    };
                    
                    markdown.push_str(&format!("## {} {}\n", role_emoji, 
                        format!("{:?}", message.role)));
                    markdown.push_str(&format!("*{}*\n\n", message.timestamp));
                    markdown.push_str(&format!("{}\n\n---\n\n", message.content));
                }
                
                Ok(markdown)
            }
            ExportFormat::Text => {
                let mut text = String::new();
                for message in messages {
                    text.push_str(&format!("[{:?}] {}\n{}\n\n", 
                        message.role, message.timestamp, message.content));
                }
                Ok(text)
            }
        }
    }
    
    /// Load history from disk
    fn load_history(&mut self) -> Result<(), String> {
        if !self.storage_path.exists() {
            return Ok(());
        }
        
        let content = fs::read_to_string(&self.storage_path)
            .map_err(|e| format!("Failed to read chat history file: {}", e))?;
        
        #[derive(Deserialize)]
        struct HistoryData {
            sessions: HashMap<String, ChatSession>,
            global_session: ChatSession,
        }
        
        let data: HistoryData = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse chat history: {}", e))?;
        
        self.sessions = data.sessions;
        self.global_session = data.global_session;
        
        Ok(())
    }
    
    /// Save history to disk
    fn save_history(&self) -> Result<(), String> {
        #[derive(Serialize)]
        struct HistoryData {
            sessions: HashMap<String, ChatSession>,
            global_session: ChatSession,
        }
        
        let data = HistoryData {
            sessions: self.sessions.clone(),
            global_session: self.global_session.clone(),
        };
        
        let content = serde_json::to_string_pretty(&data)
            .map_err(|e| format!("Failed to serialize chat history: {}", e))?;
        
        // Create parent directory if it doesn't exist
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        
        fs::write(&self.storage_path, content)
            .map_err(|e| format!("Failed to write chat history file: {}", e))?;
        
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportFormat {
    Json,
    Markdown,
    Text,
}