//! Dynamic Tool — A generic AgentTool backed by an async callback.
//!
//! Used to bridge external tool systems (MCP servers, custom APIs, etc.)
//! into the agent ToolRegistry without compile-time coupling.

use anyhow::Result;
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::context::AgentContext;
use super::tools::{AgentTool, ToolInput, ToolResult};

/// Type alias for the async callback that executes the tool.
/// Takes (parameters: serde_json::Value) and returns Result<ToolResult>.
pub type ToolCallback = Arc<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send>>
        + Send
        + Sync,
>;

/// A tool whose behavior is defined at runtime via a callback closure.
/// Enables bridging external tool systems (MCP, REST APIs, etc.) into
/// the agent framework without requiring a separate struct per tool.
pub struct DynamicTool {
    id: String,
    name: String,
    description: String,
    parameters_schema: serde_json::Value,
    callback: ToolCallback,
}

impl DynamicTool {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        parameters_schema: serde_json::Value,
        callback: ToolCallback,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            parameters_schema,
            callback,
        }
    }
}

#[async_trait]
impl AgentTool for DynamicTool {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.parameters_schema.clone()
    }

    async fn execute(&self, input: ToolInput, _context: AgentContext) -> Result<ToolResult> {
        (self.callback)(input.parameters).await
    }
}

/// Convenience builder for registering multiple MCP tools at once.
/// Takes a list of tool definitions and a shared caller function,
/// and registers each as a DynamicTool in the given registry.
pub fn register_dynamic_tools(
    registry: &mut super::tools::ToolRegistry,
    tools: Vec<DynamicToolDef>,
) {
    for def in tools {
        registry.register(Arc::new(DynamicTool::new(
            def.id,
            def.name,
            def.description,
            def.parameters_schema,
            def.callback,
        )));
    }
}

/// Definition for a dynamic tool to be registered.
pub struct DynamicToolDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
    pub callback: ToolCallback,
}
