//! Agent Definition - Configuration and metadata for AI agents

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete definition of an AI agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Unique identifier for the agent
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Description of what the agent does
    pub description: String,

    /// System prompt that defines the agent's behavior
    pub system_prompt: String,

    /// Configuration parameters
    pub config: AgentConfig,

    /// Capabilities the agent has
    pub capabilities: Vec<AgentCapability>,

    /// Tools the agent can use
    pub tools: Vec<ToolConfig>,

    /// Whether this agent is currently enabled
    #[serde(default)]
    pub enabled: bool,

    /// Custom metadata (tags, version, author, etc.)
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Configuration for agent behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Temperature for LLM generation (0.0 - 2.0)
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Maximum tokens to generate
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,

    /// Top-p sampling parameter
    #[serde(default = "default_top_p")]
    pub top_p: f32,

    /// Whether to stream responses
    #[serde(default)]
    pub stream: bool,

    /// Maximum number of tool calls per execution
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: usize,

    /// Timeout for agent execution in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Whether to use RAG context automatically
    #[serde(default)]
    pub auto_use_rag: bool,

    /// Number of RAG results to retrieve
    #[serde(default = "default_rag_results")]
    pub rag_top_k: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            top_p: default_top_p(),
            stream: false,
            max_tool_calls: default_max_tool_calls(),
            timeout_seconds: default_timeout(),
            auto_use_rag: true,
            rag_top_k: default_rag_results(),
        }
    }
}

fn default_temperature() -> f32 {
    0.7
}
fn default_max_tokens() -> usize {
    2048
}
fn default_top_p() -> f32 {
    0.9
}
fn default_max_tool_calls() -> usize {
    10
}
fn default_timeout() -> u64 {
    300
} // 5 minutes
fn default_rag_results() -> usize {
    5
}

/// Capabilities an agent can have
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentCapability {
    /// Can search and retrieve documents
    RAGSearch,

    /// Can analyze code structure and semantics
    CodeAnalysis,

    /// Can generate documents in various formats
    DocumentGeneration,

    /// Can maintain conversation context
    ConversationMemory,

    /// Can learn from user feedback
    PatternLearning,

    /// Can execute code (with sandboxing)
    CodeExecution,

    /// Can access external APIs
    ExternalAPI,

    /// Can manage files
    FileManagement,

    /// Can perform web searches
    WebSearch,

    /// Custom capability defined by user
    Custom(String),
}

/// Configuration for a tool that an agent can use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Tool identifier (matches ToolRegistry)
    pub tool_id: String,

    /// Whether this tool is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Custom configuration for this tool
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,

    /// Description override (if different from tool default)
    pub description: Option<String>,
}

fn default_true() -> bool {
    true
}

impl AgentDefinition {
    /// Create a new agent definition with minimal configuration
    pub fn new(name: String, system_prompt: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            description: String::new(),
            system_prompt,
            config: AgentConfig::default(),
            capabilities: vec![],
            tools: vec![],
            enabled: true,
            metadata: HashMap::new(),
        }
    }

    /// Builder pattern: Add a description
    pub fn with_description(mut self, description: String) -> Self {
        self.description = description;
        self
    }

    /// Builder pattern: Add a capability
    pub fn with_capability(mut self, capability: AgentCapability) -> Self {
        self.capabilities.push(capability);
        self
    }

    /// Builder pattern: Add a tool
    pub fn with_tool(mut self, tool: ToolConfig) -> Self {
        self.tools.push(tool);
        self
    }

    /// Builder pattern: Set configuration
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Builder pattern: Add metadata
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Check if agent has a specific capability
    pub fn has_capability(&self, capability: &AgentCapability) -> bool {
        self.capabilities.contains(capability)
    }

    /// Get enabled tools
    pub fn enabled_tools(&self) -> Vec<&ToolConfig> {
        self.tools.iter().filter(|t| t.enabled).collect()
    }

    /// Validate the agent definition
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("Agent name cannot be empty");
        }
        if self.system_prompt.is_empty() {
            anyhow::bail!("Agent system prompt cannot be empty");
        }
        if self.config.temperature < 0.0 || self.config.temperature > 2.0 {
            anyhow::bail!("Temperature must be between 0.0 and 2.0");
        }
        if self.config.max_tokens == 0 {
            anyhow::bail!("Max tokens must be greater than 0");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_definition_builder() {
        let agent = AgentDefinition::new(
            "TestAgent".to_string(),
            "You are a test assistant".to_string(),
        )
        .with_description("A test agent".to_string())
        .with_capability(AgentCapability::RAGSearch)
        .with_capability(AgentCapability::CodeAnalysis)
        .with_metadata("author".to_string(), "system".to_string());

        assert_eq!(agent.name, "TestAgent");
        assert_eq!(agent.capabilities.len(), 2);
        assert!(agent.has_capability(&AgentCapability::RAGSearch));
        assert_eq!(agent.metadata.get("author").unwrap(), "system");
    }

    #[test]
    fn test_agent_validation() {
        let valid_agent = AgentDefinition::new("Valid".to_string(), "Valid prompt".to_string());
        assert!(valid_agent.validate().is_ok());

        let invalid_agent = AgentDefinition::new("".to_string(), "Valid prompt".to_string());
        assert!(invalid_agent.validate().is_err());
    }
}
