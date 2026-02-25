//! Agent Registry - Storage and management of agent definitions

use super::definition::AgentDefinition;
use anyhow::{Context as AnyhowContext, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Metadata about a registered agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    /// Agent ID
    pub id: String,

    /// Agent name
    pub name: String,

    /// Short description
    pub description: String,

    /// When agent was created
    pub created_at: u64,

    /// When agent was last modified
    pub modified_at: u64,

    /// Number of times agent has been executed
    pub execution_count: u64,

    /// Average execution time in milliseconds
    pub avg_execution_time_ms: u64,

    /// Tags for categorization
    pub tags: Vec<String>,

    /// Whether agent is enabled
    pub enabled: bool,
}

/// Agent registry for managing agent definitions
pub struct AgentRegistry {
    agents: HashMap<String, AgentDefinition>,
    metadata: HashMap<String, AgentMetadata>,
    storage_path: Option<PathBuf>,
}

impl AgentRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            metadata: HashMap::new(),
            storage_path: None,
        }
    }

    /// Create registry with persistent storage
    pub fn with_storage(storage_path: PathBuf) -> Result<Self> {
        let mut registry = Self {
            agents: HashMap::new(),
            metadata: HashMap::new(),
            storage_path: Some(storage_path.clone()),
        };

        // Load existing agents if storage exists
        if storage_path.exists() {
            registry.load_from_storage()?;
        } else {
            // Create storage directory
            fs::create_dir_all(&storage_path)
                .context("Failed to create agent storage directory")?;
        }

        Ok(registry)
    }

    /// Register a new agent
    pub fn register(&mut self, definition: AgentDefinition) -> Result<String> {
        // Validate definition
        definition.validate()?;

        let agent_id = definition.id.clone();

        // Create metadata
        let metadata = AgentMetadata {
            id: agent_id.clone(),
            name: definition.name.clone(),
            description: definition.description.clone(),
            created_at: current_timestamp(),
            modified_at: current_timestamp(),
            execution_count: 0,
            avg_execution_time_ms: 0,
            tags: extract_tags(&definition),
            enabled: definition.enabled,
        };

        // Store agent and metadata
        self.agents.insert(agent_id.clone(), definition.clone());
        self.metadata.insert(agent_id.clone(), metadata);

        // Persist if storage is configured
        if self.storage_path.is_some() {
            self.save_agent_to_storage(&agent_id)?;
        }

        Ok(agent_id)
    }

    /// Get an agent by ID
    pub fn get(&self, agent_id: &str) -> Result<AgentDefinition> {
        self.agents
            .get(agent_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Agent not found: {}", agent_id))
    }

    /// List all agents
    pub fn list(&self) -> Vec<AgentMetadata> {
        self.metadata.values().cloned().collect()
    }

    /// Update an agent
    pub fn update(&mut self, agent_id: &str, definition: AgentDefinition) -> Result<()> {
        // Validate definition
        definition.validate()?;

        if !self.agents.contains_key(agent_id) {
            anyhow::bail!("Agent not found: {}", agent_id);
        }

        // Update metadata
        if let Some(metadata) = self.metadata.get_mut(agent_id) {
            metadata.name = definition.name.clone();
            metadata.description = definition.description.clone();
            metadata.modified_at = current_timestamp();
            metadata.tags = extract_tags(&definition);
        }

        // Update agent
        self.agents.insert(agent_id.to_string(), definition);

        // Persist if storage is configured
        if self.storage_path.is_some() {
            self.save_agent_to_storage(agent_id)?;
        }

        Ok(())
    }

    /// Delete an agent
    pub fn delete(&mut self, agent_id: &str) -> Result<()> {
        if !self.agents.contains_key(agent_id) {
            anyhow::bail!("Agent not found: {}", agent_id);
        }

        self.agents.remove(agent_id);
        self.metadata.remove(agent_id);

        // Delete from storage if configured
        if let Some(ref storage_path) = self.storage_path {
            let agent_file = storage_path.join(format!("{}.json", agent_id));
            if agent_file.exists() {
                fs::remove_file(agent_file).context("Failed to delete agent file")?;
            }
        }

        Ok(())
    }

    /// Track agent execution
    pub fn track_execution(&mut self, agent_id: &str, execution_time_ms: u64) -> Result<()> {
        if let Some(metadata) = self.metadata.get_mut(agent_id) {
            metadata.execution_count += 1;

            // Update average execution time
            let total_time =
                metadata.avg_execution_time_ms * (metadata.execution_count - 1) + execution_time_ms;
            metadata.avg_execution_time_ms = total_time / metadata.execution_count;
        }

        Ok(())
    }

    /// Toggle agent enabled status
    pub fn toggle_enabled(&mut self, agent_id: &str, enabled: bool) -> Result<()> {
        if !self.agents.contains_key(agent_id) {
            anyhow::bail!("Agent not found: {}", agent_id);
        }

        // Update metadata
        if let Some(metadata) = self.metadata.get_mut(agent_id) {
            metadata.enabled = enabled;
            metadata.modified_at = current_timestamp();
        }

        // Update agent definition
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.enabled = enabled;
        }

        // Persist if storage is configured
        if self.storage_path.is_some() {
            self.save_agent_to_storage(agent_id)?;
        }

        Ok(())
    }

    /// Load agents from a directory
    pub async fn load_from_directory(&mut self, dir_path: &str) -> Result<Vec<String>> {
        let path = Path::new(dir_path);
        if !path.exists() {
            anyhow::bail!("Directory does not exist: {}", dir_path);
        }

        let mut loaded_ids = Vec::new();

        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_path = entry.path();

            if file_path.is_file() {
                let extension = file_path.extension().and_then(|s| s.to_str());

                if extension == Some("json") || extension == Some("yaml") {
                    match self.load_agent_from_file(&file_path) {
                        Ok(agent_id) => loaded_ids.push(agent_id),
                        Err(e) => {
                            tracing::error!(path = ?file_path, error = %e, "Failed to load agent from file");
                        }
                    }
                }
            }
        }

        Ok(loaded_ids)
    }

    /// Save all agents to a directory
    pub async fn save_to_directory(&self, dir_path: &str) -> Result<()> {
        let path = Path::new(dir_path);
        fs::create_dir_all(path)?;

        for (agent_id, _) in &self.agents {
            let file_path = path.join(format!("{}.json", agent_id));
            self.save_agent_to_file(agent_id, &file_path)?;
        }

        Ok(())
    }

    /// Load agent from file and register it
    fn load_agent_from_file(&mut self, file_path: &Path) -> Result<String> {
        let content = fs::read_to_string(file_path).context("Failed to read agent file")?;

        let definition: AgentDefinition =
            serde_json::from_str(&content).context("Failed to parse agent definition")?;

        let id = self.register(definition)?;
        Ok(id)
    }

    /// Save agent to file
    fn save_agent_to_file(&self, agent_id: &str, file_path: &Path) -> Result<()> {
        let definition = self.get(agent_id)?;
        let content =
            serde_json::to_string_pretty(&definition).context("Failed to serialize agent")?;

        fs::write(file_path, content).context("Failed to write agent file")?;

        Ok(())
    }

    /// Load agents from persistent storage
    fn load_from_storage(&mut self) -> Result<()> {
        if let Some(ref storage_path) = self.storage_path {
            for entry in fs::read_dir(storage_path)? {
                let entry = entry?;
                let file_path = entry.path();

                if file_path.extension().and_then(|s| s.to_str()) == Some("json") {
                    let content = fs::read_to_string(&file_path)?;
                    let definition: AgentDefinition = serde_json::from_str(&content)?;

                    let agent_id = definition.id.clone();

                    // Load metadata if exists
                    let metadata_path = storage_path.join(format!("{}.metadata.json", agent_id));
                    let metadata = if metadata_path.exists() {
                        let metadata_content = fs::read_to_string(&metadata_path)?;
                        serde_json::from_str(&metadata_content)?
                    } else {
                        // Create default metadata
                        AgentMetadata {
                            id: agent_id.clone(),
                            name: definition.name.clone(),
                            description: definition.description.clone(),
                            created_at: current_timestamp(),
                            modified_at: current_timestamp(),
                            execution_count: 0,
                            avg_execution_time_ms: 0,
                            tags: extract_tags(&definition),
                            enabled: true,
                        }
                    };

                    self.agents.insert(agent_id.clone(), definition);
                    self.metadata.insert(agent_id, metadata);
                }
            }
        }

        Ok(())
    }

    /// Save agent to persistent storage
    fn save_agent_to_storage(&self, agent_id: &str) -> Result<()> {
        if let Some(ref storage_path) = self.storage_path {
            let definition = self.get(agent_id)?;
            let metadata = self
                .metadata
                .get(agent_id)
                .ok_or_else(|| anyhow::anyhow!("Metadata not found for agent: {}", agent_id))?;

            // Save definition
            let agent_file = storage_path.join(format!("{}.json", agent_id));
            let agent_content = serde_json::to_string_pretty(&definition)?;
            fs::write(agent_file, agent_content)?;

            // Save metadata
            let metadata_file = storage_path.join(format!("{}.metadata.json", agent_id));
            let metadata_content = serde_json::to_string_pretty(metadata)?;
            fs::write(metadata_file, metadata_content)?;
        }

        Ok(())
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract tags from agent definition
fn extract_tags(definition: &AgentDefinition) -> Vec<String> {
    let mut tags = Vec::new();

    // Add capability-based tags
    for capability in &definition.capabilities {
        tags.push(format!("{:?}", capability).to_lowercase());
    }

    // Add custom tags from metadata
    if let Some(custom_tags) = definition.metadata.get("tags") {
        if let Ok(tag_vec) = serde_json::from_str::<Vec<String>>(custom_tags) {
            tags.extend(tag_vec);
        }
    }

    tags
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
    use crate::agent::definition::AgentDefinition;

    #[test]
    fn test_registry_registration() {
        let mut registry = AgentRegistry::new();

        let definition = AgentDefinition::new(
            "TestAgent".to_string(),
            "You are a test assistant".to_string(),
        );

        let agent_id = registry.register(definition).unwrap();
        assert!(!agent_id.is_empty());

        let agents = registry.list();
        assert_eq!(agents.len(), 1);
    }

    #[test]
    fn test_registry_update() {
        let mut registry = AgentRegistry::new();

        let definition = AgentDefinition::new(
            "TestAgent".to_string(),
            "You are a test assistant".to_string(),
        );

        let agent_id = registry.register(definition.clone()).unwrap();

        let mut updated = definition.clone();
        updated.description = "Updated description".to_string();

        registry.update(&agent_id, updated).unwrap();

        let retrieved = registry.get(&agent_id).unwrap();
        assert_eq!(retrieved.description, "Updated description");
    }

    #[test]
    fn test_registry_delete() {
        let mut registry = AgentRegistry::new();

        let definition = AgentDefinition::new(
            "TestAgent".to_string(),
            "You are a test assistant".to_string(),
        );

        let agent_id = registry.register(definition).unwrap();
        assert_eq!(registry.list().len(), 1);

        registry.delete(&agent_id).unwrap();
        assert_eq!(registry.list().len(), 0);
    }
}
