//! Multi-Agent Orchestrator
//!
//! Enables agents to invoke other agents as tools, forming a hierarchical
//! multi-agent system. A "coordinator" agent can delegate subtasks to
//! specialist agents and synthesize their outputs.
//!
//! Pattern: Agents-as-tools — each registered agent becomes a callable tool
//! that runs the target agent's full execution pipeline and returns its result.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::context::AgentContext;
use super::definition::AgentDefinition;
use super::executor::{AgentExecutor, ExecutionResult};
use super::metrics::AgentMetricsCollector;
use super::monitor::AgentMonitor;
use super::registry::AgentRegistry;
use super::tools::{AgentTool, ToolInput, ToolRegistry, ToolResult};

/// An AgentTool that delegates execution to another agent.
/// When invoked, it runs the target agent with the provided query
/// and returns the agent's response as the tool result.
pub struct AgentDelegateTool {
    agent_id: String,
    agent_name: String,
    agent_description: String,
    registry: Arc<RwLock<AgentRegistry>>,
    tool_registry: Arc<ToolRegistry>,
    metrics: Arc<AgentMetricsCollector>,
    monitor: Arc<AgentMonitor>,
}

impl AgentDelegateTool {
    pub fn new(
        agent_def: &AgentDefinition,
        registry: Arc<RwLock<AgentRegistry>>,
        tool_registry: Arc<ToolRegistry>,
        metrics: Arc<AgentMetricsCollector>,
        monitor: Arc<AgentMonitor>,
    ) -> Self {
        Self {
            agent_id: agent_def.id.clone(),
            agent_name: agent_def.name.clone(),
            agent_description: agent_def.description.clone(),
            registry,
            tool_registry,
            metrics,
            monitor,
        }
    }
}

#[async_trait]
impl AgentTool for AgentDelegateTool {
    fn id(&self) -> &str {
        // Use agent_id as tool id (prefixed to avoid collision)
        // We store it on the struct so we can return a &str
        &self.agent_id
    }

    fn name(&self) -> &str {
        &self.agent_name
    }

    fn description(&self) -> &str {
        &self.agent_description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The task or question to delegate to this agent"
                },
                "context": {
                    "type": "string",
                    "description": "Additional context or instructions for the agent"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: ToolInput, parent_context: AgentContext) -> Result<ToolResult> {
        let query = input.parameters["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;

        let extra_context = input.parameters["context"].as_str().unwrap_or("");

        // Look up the agent definition
        let registry = self.registry.read().await;
        let agent_def = registry.get(&self.agent_id)?;
        drop(registry);

        // Build a child context from the parent, overriding the query
        let mut child_context = parent_context.clone();
        child_context.query = Some(format!("{}\n\n{}", query, extra_context).trim().to_string());

        // Execute the target agent
        let execution_id = uuid::Uuid::new_v4().to_string();
        let executor = AgentExecutor::new(
            agent_def,
            self.tool_registry.clone(),
            self.metrics.clone(),
            self.monitor.clone(),
            execution_id,
        );

        match executor.execute(child_context).await {
            Ok(result) => {
                let tools_used_str = if result.tools_used.is_empty() {
                    String::new()
                } else {
                    format!("\n\nTools used: {}", result.tools_used.join(", "))
                };

                Ok(ToolResult {
                    success: result.success,
                    output: result.response.clone(),
                    data: serde_json::json!({
                        "agent": self.agent_name,
                        "response": result.response,
                        "tools_used": result.tools_used,
                        "steps": result.steps.len(),
                        "execution_time_ms": result.execution_time_ms,
                    }),
                    error: result.error,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: format!("Agent '{}' failed: {}", self.agent_name, e),
                data: serde_json::json!({}),
                error: Some(e.to_string()),
            }),
        }
    }
}

/// Register all enabled agents as delegate tools in a ToolRegistry.
/// This allows a coordinator agent to call specialist agents by name.
///
/// To prevent infinite recursion, `exclude_agent_id` specifies the calling
/// agent's ID — it won't be registered as a tool for itself.
pub async fn register_agent_tools(
    tool_registry: &mut ToolRegistry,
    agent_registry: Arc<RwLock<AgentRegistry>>,
    shared_tool_registry: Arc<ToolRegistry>,
    metrics: Arc<AgentMetricsCollector>,
    monitor: Arc<AgentMonitor>,
    exclude_agent_id: Option<&str>,
) {
    let registry = agent_registry.read().await;
    let agents = registry.list();
    drop(registry);

    for meta in agents {
        // Skip the calling agent to prevent self-invocation loops
        if let Some(exclude) = exclude_agent_id {
            if meta.id == exclude {
                continue;
            }
        }

        // Only register enabled agents
        if !meta.enabled {
            continue;
        }

        let registry_guard = agent_registry.read().await;
        if let Ok(def) = registry_guard.get(&meta.id) {
            let tool = AgentDelegateTool::new(
                &def,
                agent_registry.clone(),
                shared_tool_registry.clone(),
                metrics.clone(),
                monitor.clone(),
            );
            tool_registry.register(Arc::new(tool));
        }
        drop(registry_guard);
    }
}

/// A coordinator agent configuration that automatically gets all other agents as tools.
/// This creates a "meta-agent" that can delegate to specialist agents.
pub fn create_coordinator_agent(
    name: impl Into<String>,
    description: impl Into<String>,
    specialist_names: &[&str],
) -> AgentDefinition {
    let specialists_list = specialist_names.join(", ");
    let system_prompt = format!(
        "You are a coordinator agent that manages a team of specialist agents. \
         Your role is to:\n\
         1. Understand the user's request\n\
         2. Break it down into subtasks if needed\n\
         3. Delegate subtasks to the appropriate specialist agent(s)\n\
         4. Synthesize their outputs into a coherent final response\n\n\
         Available specialists: {}\n\n\
         Guidelines:\n\
         - Use the search_documents tool for knowledge base queries\n\
         - Delegate to specialist agents for domain-specific tasks\n\
         - Combine results from multiple agents when the task spans domains\n\
         - Always cite which agent provided which information\n\
         - If a specialist fails, try an alternative approach or report the limitation",
        specialists_list
    );

    AgentDefinition {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        description: description.into(),
        system_prompt,
        config: super::definition::AgentConfig {
            max_tool_calls: 15,
            timeout_seconds: 120,
            ..Default::default()
        },
        capabilities: vec![
            super::definition::AgentCapability::RAGSearch,
            super::definition::AgentCapability::ExternalAPI,
        ],
        enabled: true,
        tools: vec![],
        metadata: HashMap::new(),
    }
}
