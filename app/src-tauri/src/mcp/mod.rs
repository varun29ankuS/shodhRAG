//! Model Context Protocol (MCP) - Tool Integration Framework
//!
//! MCP provides a standardized protocol for integrating external tools and resources
//! into the RAG system. Following the specification from Anthropic/ModelContextProtocol.

use anyhow::{Result, Context as AnyhowContext};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub mod client;
pub mod transport;
pub mod registry;
pub mod builtin_tools;

/// Tool definition following MCP spec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value, // JSON Schema
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<ToolCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Filesystem,
    Database,
    Api,
    Search,
    Cloud,
    Development,
    Communication,
    RagSystem, // Our own RAG tools
    Custom,
}

/// Resource definition (files, databases, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(rename = "type")]
    pub resource_type: ResourceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    File,
    Directory,
    Database,
    Api,
    Web,
}

/// Resource content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub text: Option<String>,
    pub blob: Option<Vec<u8>>,
}

/// Tool call result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub success: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub content: Value,
    #[serde(rename = "type")]
    pub artifact_type: String,
}

/// MCP Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub transport: TransportType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Stdio,   // Most common - communicate via stdin/stdout
    Http,    // HTTP/REST API
    WebSocket,
}

/// MCP Client trait - implemented by different transport types
#[async_trait]
pub trait MCPClient: Send + Sync {
    /// Discover available tools from the server
    async fn discover_tools(&mut self) -> Result<Vec<ToolDefinition>>;

    /// Call a specific tool with parameters
    async fn call_tool(&self, name: &str, params: Value) -> Result<ToolCallResult>;

    /// List available resources
    async fn list_resources(&self) -> Result<Vec<Resource>>;

    /// Read a specific resource
    async fn read_resource(&self, uri: &str) -> Result<ResourceContent>;

    /// Check if client is connected
    fn is_connected(&self) -> bool;

    /// Get server information
    fn server_info(&self) -> &MCPServerConfig;
}

/// MCP Manager - manages multiple MCP clients
pub struct MCPManager {
    clients: Arc<RwLock<HashMap<String, Arc<dyn MCPClient>>>>,
    tool_registry: Arc<RwLock<HashMap<String, (String, ToolDefinition)>>>, // tool_name -> (client_name, definition)
}

impl MCPManager {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            tool_registry: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to an MCP server
    pub async fn connect_server(&self, config: MCPServerConfig) -> Result<()> {
        tracing::info!("🔌 Connecting to MCP server: {}", config.name);

        let client: Arc<dyn MCPClient> = match config.transport {
            TransportType::Stdio => {
                Arc::new(transport::StdioMCPClient::new(config.clone()).await?)
            },
            TransportType::Http => {
                anyhow::bail!("HTTP transport not yet implemented")
            },
            TransportType::WebSocket => {
                anyhow::bail!("WebSocket transport not yet implemented")
            },
        };

        // Discover tools — we're the sole owner so get_mut is safe
        let mut client_owned = client;
        let tools = Arc::get_mut(&mut client_owned)
            .ok_or_else(|| anyhow::anyhow!("Failed to get mutable access to MCP client"))?
            .discover_tools()
            .await?;
        let client = client_owned;

        tracing::info!("  ✓ Discovered {} tools from {}", tools.len(), config.name);

        // Register tools
        let mut registry = self.tool_registry.write().await;
        for tool in tools {
            tracing::info!("    - {}: {}", tool.name, tool.description);
            registry.insert(
                tool.name.clone(),
                (config.name.clone(), tool.clone())
            );
        }

        // Store client
        let mut clients = self.clients.write().await;
        clients.insert(config.name.clone(), client);

        tracing::info!("  ✓ Connected successfully");
        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect_server(&self, name: &str) -> Result<()> {
        let mut clients = self.clients.write().await;
        clients.remove(name)
            .ok_or_else(|| anyhow::anyhow!("Server not found: {}", name))?;

        // Remove tools from registry
        let mut registry = self.tool_registry.write().await;
        registry.retain(|_, (server_name, _)| server_name != name);

        tracing::info!("🔌 Disconnected from MCP server: {}", name);
        Ok(())
    }

    /// List all available tools
    pub async fn list_tools(&self) -> Result<Vec<(String, String, ToolDefinition)>> {
        let registry = self.tool_registry.read().await;
        Ok(registry.iter()
            .map(|(name, (server, def))| (name.clone(), server.clone(), def.clone()))
            .collect())
    }

    /// Call a tool
    pub async fn call_tool(&self, tool_name: &str, params: Value) -> Result<ToolCallResult> {
        let registry = self.tool_registry.read().await;
        let (server_name, _) = registry.get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", tool_name))?;

        let clients = self.clients.read().await;
        let client = clients.get(server_name)
            .ok_or_else(|| anyhow::anyhow!("Server not connected: {}", server_name))?;

        tracing::info!("🔧 Calling tool: {} (server: {})", tool_name, server_name);
        let result = client.call_tool(tool_name, params).await?;

        if result.success {
            tracing::info!("  ✓ Tool call successful");
        } else {
            tracing::info!("  ✗ Tool call failed: {:?}", result.error);
        }

        Ok(result)
    }

    /// Search for tools by query
    pub async fn search_tools(&self, query: &str) -> Result<Vec<(String, ToolDefinition)>> {
        let registry = self.tool_registry.read().await;
        let query_lower = query.to_lowercase();

        Ok(registry.iter()
            .filter(|(name, (_, def))| {
                name.to_lowercase().contains(&query_lower) ||
                def.description.to_lowercase().contains(&query_lower)
            })
            .map(|(name, (_, def))| (name.clone(), def.clone()))
            .collect())
    }

    /// Get connected servers
    pub async fn list_servers(&self) -> Result<Vec<String>> {
        let clients = self.clients.read().await;
        Ok(clients.keys().cloned().collect())
    }
}

impl Default for MCPManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_manager_creation() {
        let manager = MCPManager::new();
        let servers = manager.list_servers().await.unwrap();
        assert_eq!(servers.len(), 0);
    }
}
