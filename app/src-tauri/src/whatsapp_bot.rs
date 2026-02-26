//! WhatsApp Bot integration with RAG
//! Provides personal assistant capabilities through WhatsApp messaging

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppMessage {
    pub id: String,
    pub from: String,           // Phone number
    pub from_name: String,       // Contact name
    pub body: String,            // Message text
    pub timestamp: DateTime<Utc>,
    pub chat_id: String,         // Individual or group chat ID
    pub is_group: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppContact {
    pub phone: String,
    pub name: String,
    pub assigned_space: Option<String>,  // Which knowledge space to use
    pub is_authorized: bool,              // Whether they can access the bot
    pub conversation_id: Option<String>,  // Memory conversation ID
    pub preferences: ContactPreferences,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactPreferences {
    pub language: String,
    pub response_style: ResponseStyle,  // Formal, casual, technical
    pub max_response_length: usize,
    pub include_sources: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseStyle {
    Formal,
    Casual,
    Technical,
    Concise,
}

impl Default for ContactPreferences {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            response_style: ResponseStyle::Casual,
            max_response_length: 500,
            include_sources: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotResponse {
    pub message: String,
    pub sources: Vec<String>,
    pub confidence: f32,
    pub used_space: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub contact: String,
    pub messages: Vec<WhatsAppMessage>,
    pub responses: Vec<BotResponse>,
    pub started_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

/// WhatsApp Bot State
pub struct WhatsAppBot {
    pub contacts: Arc<RwLock<HashMap<String, WhatsAppContact>>>,
    pub conversations: Arc<RwLock<HashMap<String, Conversation>>>,
    pub webhook_token: Arc<RwLock<Option<String>>>,
    pub is_active: Arc<RwLock<bool>>,
    pub bot_phone: Arc<RwLock<Option<String>>>,
}

impl WhatsAppBot {
    pub fn new() -> Self {
        Self {
            contacts: Arc::new(RwLock::new(HashMap::new())),
            conversations: Arc::new(RwLock::new(HashMap::new())),
            webhook_token: Arc::new(RwLock::new(None)),
            is_active: Arc::new(RwLock::new(false)),
            bot_phone: Arc::new(RwLock::new(None)),
        }
    }

    /// Add or update a contact
    pub async fn add_contact(&self, contact: WhatsAppContact) {
        let mut contacts = self.contacts.write().await;
        contacts.insert(contact.phone.clone(), contact);
    }

    /// Get contact by phone number
    pub async fn get_contact(&self, phone: &str) -> Option<WhatsAppContact> {
        let contacts = self.contacts.read().await;
        contacts.get(phone).cloned()
    }

    /// Check if contact is authorized to use the bot
    pub async fn is_authorized(&self, phone: &str) -> bool {
        let contacts = self.contacts.read().await;
        contacts.get(phone)
            .map(|c| c.is_authorized)
            .unwrap_or(false)
    }

    /// Get or create conversation for a contact
    pub async fn get_conversation(&self, contact_phone: &str) -> Option<Conversation> {
        let conversations = self.conversations.read().await;
        conversations.values()
            .find(|c| c.contact == contact_phone)
            .cloned()
    }

    /// Start a new conversation
    pub async fn start_conversation(&self, contact_phone: String) -> String {
        let conversation_id = uuid::Uuid::new_v4().to_string();
        let conversation = Conversation {
            id: conversation_id.clone(),
            contact: contact_phone,
            messages: Vec::new(),
            responses: Vec::new(),
            started_at: Utc::now(),
            last_activity: Utc::now(),
        };

        let mut conversations = self.conversations.write().await;
        conversations.insert(conversation_id.clone(), conversation);
        conversation_id
    }

    /// Add message to conversation
    pub async fn add_message(&self, conversation_id: &str, message: WhatsAppMessage) {
        let mut conversations = self.conversations.write().await;
        if let Some(conv) = conversations.get_mut(conversation_id) {
            conv.messages.push(message);
            conv.last_activity = Utc::now();
        }
    }

    /// Add bot response to conversation
    pub async fn add_response(&self, conversation_id: &str, response: BotResponse) {
        let mut conversations = self.conversations.write().await;
        if let Some(conv) = conversations.get_mut(conversation_id) {
            conv.responses.push(response);
            conv.last_activity = Utc::now();
        }
    }

    /// Get conversation history for context
    pub async fn get_conversation_context(&self, conversation_id: &str, limit: usize) -> String {
        let conversations = self.conversations.read().await;
        if let Some(conv) = conversations.get(conversation_id) {
            let mut context = String::new();
            let start = conv.messages.len().saturating_sub(limit);

            for i in start..conv.messages.len() {
                if let (Some(msg), Some(resp)) = (conv.messages.get(i), conv.responses.get(i)) {
                    context.push_str(&format!("User: {}\n", msg.body));
                    context.push_str(&format!("Assistant: {}\n\n", resp.message));
                }
            }
            context
        } else {
            String::new()
        }
    }

    /// Set bot as active/inactive
    pub async fn set_active(&self, active: bool) {
        let mut is_active = self.is_active.write().await;
        *is_active = active;
    }

    /// Check if bot is active
    pub async fn is_bot_active(&self) -> bool {
        *self.is_active.read().await
    }

    /// List all authorized contacts
    pub async fn list_authorized_contacts(&self) -> Vec<WhatsAppContact> {
        let contacts = self.contacts.read().await;
        contacts.values()
            .filter(|c| c.is_authorized)
            .cloned()
            .collect()
    }

    /// Get statistics
    pub async fn get_stats(&self) -> BotStats {
        let contacts = self.contacts.read().await;
        let conversations = self.conversations.read().await;

        let total_messages: usize = conversations.values()
            .map(|c| c.messages.len())
            .sum();

        let total_responses: usize = conversations.values()
            .map(|c| c.responses.len())
            .sum();

        BotStats {
            total_contacts: contacts.len(),
            authorized_contacts: contacts.values().filter(|c| c.is_authorized).count(),
            total_conversations: conversations.len(),
            total_messages,
            total_responses,
            active: *self.is_active.read().await,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotStats {
    pub total_contacts: usize,
    pub authorized_contacts: usize,
    pub total_conversations: usize,
    pub total_messages: usize,
    pub total_responses: usize,
    pub active: bool,
}

impl Default for WhatsAppBot {
    fn default() -> Self {
        Self::new()
    }
}
