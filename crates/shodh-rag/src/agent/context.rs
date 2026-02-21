//! Agent Context - Execution context for agent runs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Context for agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContext {
    /// Current user query (if any)
    pub query: Option<String>,

    /// Conversation history
    pub conversation_history: Vec<ConversationTurn>,

    /// Context variables (can be set by tools or user)
    pub variables: HashMap<String, serde_json::Value>,

    /// User information
    pub user_info: Option<UserInfo>,

    /// Space/project context
    pub space_id: Option<String>,

    /// Session ID for tracking
    pub session_id: String,

    /// Custom metadata
    pub metadata: HashMap<String, String>,
}

impl AgentContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self {
            query: None,
            conversation_history: Vec::new(),
            variables: HashMap::new(),
            user_info: None,
            space_id: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            metadata: HashMap::new(),
        }
    }

    /// Create context with a query
    pub fn with_query(query: String) -> Self {
        Self {
            query: Some(query),
            ..Self::new()
        }
    }

    /// Add a conversation turn
    pub fn add_conversation_turn(&mut self, turn: ConversationTurn) {
        self.conversation_history.push(turn);
    }

    /// Add a variable to context
    pub fn add_variable(&mut self, key: String, value: serde_json::Value) {
        self.variables.insert(key, value);
    }

    /// Get a variable from context
    pub fn get_variable(&self, key: &str) -> Option<&serde_json::Value> {
        self.variables.get(key)
    }

    /// Set user info
    pub fn with_user_info(mut self, user_info: UserInfo) -> Self {
        self.user_info = Some(user_info);
        self
    }

    /// Set space ID
    pub fn with_space_id(mut self, space_id: String) -> Self {
        self.space_id = Some(space_id);
        self
    }

    /// Add metadata
    pub fn add_metadata(&mut self, key: String, value: String) {
        self.metadata.insert(key, value);
    }

    /// Get recent conversation (last N turns)
    pub fn recent_conversation(&self, n: usize) -> Vec<&ConversationTurn> {
        self.conversation_history
            .iter()
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Format conversation history as string
    pub fn format_conversation(&self, max_turns: Option<usize>) -> String {
        let turns = if let Some(max) = max_turns {
            self.recent_conversation(max)
        } else {
            self.conversation_history.iter().collect()
        };

        turns
            .iter()
            .map(|turn| format!("{}: {}", turn.role, turn.content))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A single turn in conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// Role (user, assistant, system)
    pub role: String,

    /// Content of the message
    pub content: String,

    /// Timestamp in milliseconds
    pub timestamp: u64,

    /// Optional metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl ConversationTurn {
    /// Create a user message
    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
            timestamp: current_timestamp(),
            metadata: HashMap::new(),
        }
    }

    /// Create an assistant message
    pub fn assistant(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            timestamp: current_timestamp(),
            metadata: HashMap::new(),
        }
    }

    /// Create a system message
    pub fn system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content,
            timestamp: current_timestamp(),
            metadata: HashMap::new(),
        }
    }
}

/// User information for personalization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    /// User ID
    pub user_id: String,

    /// User name
    pub name: Option<String>,

    /// User preferences
    pub preferences: HashMap<String, serde_json::Value>,

    /// User role/permissions
    pub role: Option<String>,
}

impl UserInfo {
    /// Create new user info
    pub fn new(user_id: String) -> Self {
        Self {
            user_id,
            name: None,
            preferences: HashMap::new(),
            role: None,
        }
    }

    /// Set a preference
    pub fn set_preference(&mut self, key: String, value: serde_json::Value) {
        self.preferences.insert(key, value);
    }

    /// Get a preference
    pub fn get_preference(&self, key: &str) -> Option<&serde_json::Value> {
        self.preferences.get(key)
    }
}

/// Context variable that can be used in prompts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextVariable {
    /// Variable name
    pub name: String,

    /// Variable value
    pub value: serde_json::Value,

    /// Description of what this variable represents
    pub description: Option<String>,
}

/// Get current timestamp in milliseconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let context = AgentContext::new();
        assert!(context.query.is_none());
        assert_eq!(context.conversation_history.len(), 0);
        assert!(!context.session_id.is_empty());
    }

    #[test]
    fn test_context_with_query() {
        let context = AgentContext::with_query("test query".to_string());
        assert_eq!(context.query.unwrap(), "test query");
    }

    #[test]
    fn test_conversation_management() {
        let mut context = AgentContext::new();
        context.add_conversation_turn(ConversationTurn::user("Hello".to_string()));
        context.add_conversation_turn(ConversationTurn::assistant("Hi there!".to_string()));

        assert_eq!(context.conversation_history.len(), 2);

        let recent = context.recent_conversation(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].role, "assistant");
    }

    #[test]
    fn test_variables() {
        let mut context = AgentContext::new();
        context.add_variable("test_var".to_string(), serde_json::json!({"key": "value"}));

        let var = context.get_variable("test_var").unwrap();
        assert_eq!(var["key"], "value");
    }
}
