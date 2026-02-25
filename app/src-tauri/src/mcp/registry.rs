//! MCP Server Registry - Store and manage MCP server configurations

use super::*;
use std::path::PathBuf;
use tokio::fs;

/// Registry for managing MCP server configurations
pub struct MCPRegistry {
    servers: HashMap<String, MCPServerConfig>,
    config_file: PathBuf,
}

impl MCPRegistry {
    pub fn new(config_dir: PathBuf) -> Self {
        let config_file = config_dir.join("mcp_servers.json");
        Self {
            servers: HashMap::new(),
            config_file,
        }
    }

    /// Load server configurations from disk
    pub async fn load(&mut self) -> Result<()> {
        if !self.config_file.exists() {
            // Create default configuration with built-in servers
            self.servers = builtin_tools::get_default_mcp_servers()
                .into_iter()
                .map(|config| (config.name.clone(), config))
                .collect();
            self.save().await?;
            return Ok(());
        }

        let content = fs::read_to_string(&self.config_file).await?;
        let servers: Vec<MCPServerConfig> = serde_json::from_str(&content)?;
        self.servers = servers
            .into_iter()
            .map(|config| (config.name.clone(), config))
            .collect();

        Ok(())
    }

    /// Save server configurations to disk
    pub async fn save(&self) -> Result<()> {
        let servers: Vec<_> = self.servers.values().cloned().collect();
        let content = serde_json::to_string_pretty(&servers)?;
        fs::write(&self.config_file, content).await?;
        Ok(())
    }

    /// Add or update a server configuration
    pub fn upsert(&mut self, config: MCPServerConfig) {
        self.servers.insert(config.name.clone(), config);
    }

    /// Remove a server configuration
    pub fn remove(&mut self, name: &str) -> Option<MCPServerConfig> {
        self.servers.remove(name)
    }

    /// Get a server configuration
    pub fn get(&self, name: &str) -> Option<&MCPServerConfig> {
        self.servers.get(name)
    }

    /// List all server configurations
    pub fn list(&self) -> Vec<&MCPServerConfig> {
        self.servers.values().collect()
    }
}
