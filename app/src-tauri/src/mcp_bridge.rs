//! MCP → Agent Tool Bridge
//!
//! Converts MCP server tools into DynamicTools that can be used by
//! the agent framework's ToolRegistry and ReAct tool-calling loop.

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::mcp::MCPManager;
use shodh_rag::agent::{DynamicToolDef, ToolCallback, ToolResult};

/// Discover all tools from connected MCP servers and return them as
/// DynamicToolDefs ready for registration in the agent ToolRegistry.
pub async fn mcp_tools_as_dynamic(
    mcp_manager: Arc<RwLock<MCPManager>>,
) -> Vec<DynamicToolDef> {
    let manager = mcp_manager.read().await;
    let tools = match manager.list_tools().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list MCP tools for bridge");
            return Vec::new();
        }
    };
    drop(manager);

    tools
        .into_iter()
        .map(|(tool_name, server_name, tool_def)| {
            let mcp = mcp_manager.clone();
            let call_name = tool_name.clone();

            // Prefix the tool ID with "mcp_" to avoid collisions with built-in tools
            let id = format!("mcp_{}", tool_name);
            let display_name = format!("{} ({})", tool_def.name, server_name);

            let callback: ToolCallback = Arc::new(move |params: serde_json::Value| {
                let mcp = mcp.clone();
                let name = call_name.clone();
                Box::pin(async move {
                    let manager = mcp.read().await;
                    let result = manager.call_tool(&name, params).await;
                    drop(manager);

                    match result {
                        Ok(mcp_result) => {
                            let output = if let Some(ref val) = mcp_result.result {
                                serde_json::to_string_pretty(val)
                                    .unwrap_or_else(|_| format!("{:?}", val))
                            } else if let Some(ref err) = mcp_result.error {
                                err.clone()
                            } else {
                                "Tool executed successfully".to_string()
                            };

                            Ok(ToolResult {
                                success: mcp_result.success,
                                output,
                                data: mcp_result.result.unwrap_or(serde_json::json!({})),
                                error: mcp_result.error,
                            })
                        }
                        Err(e) => Ok(ToolResult {
                            success: false,
                            output: format!("MCP tool call failed: {}", e),
                            data: serde_json::json!({}),
                            error: Some(e.to_string()),
                        }),
                    }
                })
            });

            DynamicToolDef {
                id,
                name: display_name,
                description: tool_def.description.clone(),
                parameters_schema: tool_def.input_schema.clone(),
                callback,
            }
        })
        .collect()
}

/// Register all MCP tools into an agent ToolRegistry.
/// Call this after MCP servers are connected and before starting a chat session.
pub async fn register_mcp_tools(
    registry: &mut shodh_rag::agent::ToolRegistry,
    mcp_manager: Arc<RwLock<MCPManager>>,
) {
    let defs = mcp_tools_as_dynamic(mcp_manager).await;
    let count = defs.len();
    shodh_rag::agent::register_dynamic_tools(registry, defs);
    if count > 0 {
        tracing::info!(count, "Registered MCP tools into agent ToolRegistry");
    }
}
