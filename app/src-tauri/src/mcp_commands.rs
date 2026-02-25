//! Tauri commands for MCP (Model Context Protocol) operations

use crate::mcp::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock as AsyncRwLock;

/// MCP State managed by Tauri
pub struct MCPState {
    pub manager: Arc<AsyncRwLock<MCPManager>>,
    pub registry: Arc<AsyncRwLock<registry::MCPRegistry>>,
}

/// Tool information for frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub server: String,
    pub category: Option<ToolCategory>,
    pub input_schema: Value,
}

/// Server information for frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub command: String,
    pub connected: bool,
    pub tool_count: usize,
}

/// Connect to an MCP server
#[tauri::command]
pub async fn mcp_connect_server(
    server_name: String,
    state: State<'_, MCPState>,
) -> Result<String, String> {
    tracing::info!("🔌 Connecting to MCP server: {}", server_name);

    // Get server config from registry
    let registry = state.registry.read().await;
    let config = registry
        .get(&server_name)
        .ok_or_else(|| format!("Server not found in registry: {}", server_name))?
        .clone();
    drop(registry);

    // Validate environment variables are set
    for (key, value) in &config.env {
        if value.is_empty() {
            return Err(format!("Environment variable not set: {}", key));
        }
    }

    // Connect
    let manager = state.manager.read().await;
    manager
        .connect_server(config)
        .await
        .map_err(|e| format!("Failed to connect: {}", e))?;

    Ok(format!("Connected to {}", server_name))
}

/// Disconnect from an MCP server
#[tauri::command]
pub async fn mcp_disconnect_server(
    server_name: String,
    state: State<'_, MCPState>,
) -> Result<String, String> {
    tracing::info!("🔌 Disconnecting from MCP server: {}", server_name);

    let manager = state.manager.read().await;
    manager
        .disconnect_server(&server_name)
        .await
        .map_err(|e| format!("Failed to disconnect: {}", e))?;

    Ok(format!("Disconnected from {}", server_name))
}

/// List all available tools across all connected servers
#[tauri::command]
pub async fn mcp_list_tools(state: State<'_, MCPState>) -> Result<Vec<ToolInfo>, String> {
    let manager = state.manager.read().await;
    let tools = manager
        .list_tools()
        .await
        .map_err(|e| format!("Failed to list tools: {}", e))?;

    Ok(tools
        .into_iter()
        .map(|(name, server, def)| ToolInfo {
            name,
            description: def.description.clone(),
            server,
            category: def.category.clone(),
            input_schema: def.input_schema.clone(),
        })
        .collect())
}

/// Search for tools by query
#[tauri::command]
pub async fn mcp_search_tools(
    query: String,
    state: State<'_, MCPState>,
) -> Result<Vec<ToolInfo>, String> {
    let manager = state.manager.read().await;
    let tools = manager
        .search_tools(&query)
        .await
        .map_err(|e| format!("Failed to search tools: {}", e))?;

    // Get server names for each tool
    let all_tools = manager.list_tools().await.map_err(|e| e.to_string())?;
    let tool_to_server: std::collections::HashMap<_, _> = all_tools
        .into_iter()
        .map(|(name, server, _)| (name, server))
        .collect();

    Ok(tools
        .into_iter()
        .map(|(name, def)| {
            let server = tool_to_server.get(&name).cloned().unwrap_or_default();
            ToolInfo {
                name,
                description: def.description.clone(),
                server,
                category: def.category.clone(),
                input_schema: def.input_schema.clone(),
            }
        })
        .collect())
}

/// Call a specific tool
#[tauri::command]
pub async fn mcp_call_tool(
    tool_name: String,
    params: Value,
    state: State<'_, MCPState>,
) -> Result<Value, String> {
    tracing::info!(
        "🔧 Calling MCP tool: {} with params: {:?}",
        tool_name,
        params
    );

    let manager = state.manager.read().await;
    let result = manager
        .call_tool(&tool_name, params)
        .await
        .map_err(|e| format!("Tool call failed: {}", e))?;

    if result.success {
        Ok(result.result.unwrap_or(Value::Null))
    } else {
        Err(result.error.unwrap_or_else(|| "Unknown error".to_string()))
    }
}

/// List all configured servers
#[tauri::command]
pub async fn mcp_list_servers(state: State<'_, MCPState>) -> Result<Vec<ServerInfo>, String> {
    let registry = state.registry.read().await;
    let manager = state.manager.read().await;

    let connected_servers = manager
        .list_servers()
        .await
        .map_err(|e| format!("Failed to list servers: {}", e))?;

    let all_tools = manager
        .list_tools()
        .await
        .map_err(|e| format!("Failed to list tools: {}", e))?;

    // Count tools per server
    let tool_counts: std::collections::HashMap<String, usize> = all_tools
        .iter()
        .map(|(_, server, _)| server.clone())
        .fold(std::collections::HashMap::new(), |mut acc, server| {
            *acc.entry(server).or_insert(0) += 1;
            acc
        });

    Ok(registry
        .list()
        .iter()
        .map(|config| ServerInfo {
            name: config.name.clone(),
            command: format!("{} {}", config.command, config.args.join(" ")),
            connected: connected_servers.contains(&config.name),
            tool_count: tool_counts.get(&config.name).copied().unwrap_or(0),
        })
        .collect())
}

/// Add or update an MCP server configuration
#[tauri::command]
pub async fn mcp_upsert_server(
    config: MCPServerConfig,
    state: State<'_, MCPState>,
) -> Result<String, String> {
    let mut registry = state.registry.write().await;
    registry.upsert(config.clone());
    registry
        .save()
        .await
        .map_err(|e| format!("Failed to save: {}", e))?;

    Ok(format!("Server '{}' configuration saved", config.name))
}

/// Remove an MCP server configuration
#[tauri::command]
pub async fn mcp_remove_server(
    server_name: String,
    state: State<'_, MCPState>,
) -> Result<String, String> {
    // Disconnect first if connected
    let manager = state.manager.read().await;
    let _ = manager.disconnect_server(&server_name).await;
    drop(manager);

    // Remove from registry
    let mut registry = state.registry.write().await;
    registry
        .remove(&server_name)
        .ok_or_else(|| format!("Server not found: {}", server_name))?;
    registry
        .save()
        .await
        .map_err(|e| format!("Failed to save: {}", e))?;

    Ok(format!("Server '{}' removed", server_name))
}

/// Update environment variable for a server
#[tauri::command]
pub async fn mcp_update_server_env(
    server_name: String,
    env_key: String,
    env_value: String,
    state: State<'_, MCPState>,
) -> Result<String, String> {
    let mut registry = state.registry.write().await;

    let mut config = registry
        .get(&server_name)
        .ok_or_else(|| format!("Server not found: {}", server_name))?
        .clone();

    config.env.insert(env_key.clone(), env_value);
    registry.upsert(config);
    registry
        .save()
        .await
        .map_err(|e| format!("Failed to save: {}", e))?;

    Ok(format!("Updated {} for server '{}'", env_key, server_name))
}
