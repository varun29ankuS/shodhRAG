//! Multi-Agent Crew System
//!
//! Enables assembling teams of agents that collaborate on tasks, inspired by
//! CrewAI. Supports two execution processes:
//!
//! - **Sequential**: Agents execute in order, each receiving previous outputs.
//! - **Hierarchical**: A coordinator agent delegates to specialists using the
//!   existing AgentDelegateTool pattern from `orchestrator.rs`.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::time::Instant;

use super::context::AgentContext;
use crate::chat::EventEmitter;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A crew is a team of agents that work together on a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub agents: Vec<CrewMember>,
    pub process: CrewProcess,
    pub config: CrewConfig,
}

/// A member of a crew with a specific role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewMember {
    /// ID of a registered agent in AgentSystem
    pub agent_id: String,
    /// Role name (e.g., "researcher", "writer", "reviewer")
    pub role: String,
    /// What this agent should accomplish within the crew
    pub goal: String,
    /// Execution order for sequential process (0-indexed)
    pub order: usize,
}

/// How agents in the crew collaborate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CrewProcess {
    /// Agents execute in order. Each agent receives the accumulated outputs
    /// from previous agents as context.
    Sequential,
    /// A coordinator agent delegates tasks to specialist agents using tool
    /// calls. The coordinator decides which specialists to invoke and how
    /// to synthesize their outputs.
    Hierarchical {
        /// Agent ID of the coordinator (must be in crew.agents).
        coordinator_id: String,
    },
}

/// Configuration for crew execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewConfig {
    /// Maximum wall-clock time for the entire crew execution.
    #[serde(default = "default_crew_timeout")]
    pub timeout_seconds: u64,
    /// Whether to log detailed per-agent progress.
    #[serde(default)]
    pub verbose: bool,
}

fn default_crew_timeout() -> u64 {
    300
}

impl Default for CrewConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_crew_timeout(),
            verbose: false,
        }
    }
}

/// Result of executing a crew task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewExecutionResult {
    pub success: bool,
    pub final_output: String,
    pub agent_outputs: Vec<CrewAgentOutput>,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

/// Output from a single agent within a crew execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewAgentOutput {
    pub agent_id: String,
    pub agent_name: String,
    pub role: String,
    pub output: String,
    pub execution_time_ms: u64,
    pub tools_used: Vec<String>,
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute a crew task using the given AgentSystem.
///
/// This is the main entry point — it dispatches to sequential or hierarchical
/// execution based on the crew's process type. Pass an `emitter` to stream
/// per-agent progress to the frontend in real time.
pub async fn execute_crew(
    crew: &CrewDefinition,
    task: &str,
    space_id: Option<&str>,
    agent_system: &super::AgentSystem,
    emitter: Option<&dyn EventEmitter>,
) -> Result<CrewExecutionResult> {
    let start = Instant::now();

    if crew.agents.is_empty() {
        bail!("Crew '{}' has no agents", crew.name);
    }

    let result = match &crew.process {
        CrewProcess::Sequential => {
            execute_sequential(crew, task, space_id, agent_system, emitter).await
        }
        CrewProcess::Hierarchical { coordinator_id } => {
            execute_hierarchical(crew, task, space_id, agent_system, coordinator_id, emitter).await
        }
    };

    match result {
        Ok(mut r) => {
            r.execution_time_ms = start.elapsed().as_millis() as u64;
            Ok(r)
        }
        Err(e) => Ok(CrewExecutionResult {
            success: false,
            final_output: format!("Crew execution failed: {}", e),
            agent_outputs: vec![],
            execution_time_ms: start.elapsed().as_millis() as u64,
            error: Some(e.to_string()),
        }),
    }
}

/// Emit a helper for crew progress.
fn emit_event(emitter: Option<&dyn EventEmitter>, event: &str, data: serde_json::Value) {
    if let Some(em) = emitter {
        em.emit(event, data);
    }
}

/// Sequential execution: run agents in order, passing accumulated outputs forward.
/// Streams per-agent progress via `chat_token` and `tool_call_start`/`tool_call_complete`.
async fn execute_sequential(
    crew: &CrewDefinition,
    task: &str,
    space_id: Option<&str>,
    agent_system: &super::AgentSystem,
    emitter: Option<&dyn EventEmitter>,
) -> Result<CrewExecutionResult> {
    let mut agent_outputs: Vec<CrewAgentOutput> = Vec::new();
    let mut accumulated_context = String::new();
    let mut accumulated_stream = String::new(); // what the user sees (streamed)
    let total_agents = crew.agents.len();

    // Sort agents by order
    let mut sorted_agents = crew.agents.clone();
    sorted_agents.sort_by_key(|a| a.order);

    for (idx, member) in sorted_agents.iter().enumerate() {
        tracing::info!(
            crew = %crew.name,
            agent = %member.agent_id,
            role = %member.role,
            "Crew sequential: executing agent {}/{}",
            idx + 1,
            total_agents
        );

        let agent_start = Instant::now();

        // Get agent name for logging
        let agent_name = match agent_system.get_agent(&member.agent_id).await {
            Ok(def) => def.name,
            Err(e) => {
                tracing::error!(agent_id = %member.agent_id, error = %e, "Agent not found in registry");
                bail!("Agent '{}' not found: {}", member.agent_id, e);
            }
        };

        let tool_label = format!("{} ({})", agent_name, member.role);

        // Emit tool_call_start so the frontend shows a spinner bubble
        emit_event(
            emitter,
            "tool_call_start",
            serde_json::json!({
                "tool_name": tool_label,
                "arguments": serde_json::json!({
                    "role": member.role,
                    "goal": member.goal,
                    "step": format!("{}/{}", idx + 1, total_agents),
                }).to_string(),
            }),
        );

        // Stream a section header so the user sees progress immediately
        let header = format!(
            "---\n### Agent {}: {}\n*Role: {} | Goal: {}*\n\n",
            idx + 1,
            agent_name,
            member.role,
            member.goal,
        );
        accumulated_stream.push_str(&header);
        emit_event(
            emitter,
            "chat_token",
            serde_json::json!({
                "token": header,
                "accumulated": accumulated_stream,
            }),
        );

        // Build context with role, goal, and previous outputs
        let mut ctx = AgentContext::with_query(task.to_string());
        if let Some(sid) = space_id {
            ctx = ctx.with_space_id(sid.to_string());
        }

        // Inject previous agent outputs so this agent can build on them
        if !accumulated_context.is_empty() {
            ctx.add_variable(
                "crew_previous_outputs".to_string(),
                serde_json::Value::String(accumulated_context.clone()),
            );
        }

        // Inject role and goal into context metadata
        ctx.add_metadata("crew_role".to_string(), member.role.clone());
        ctx.add_metadata("crew_goal".to_string(), member.goal.clone());
        ctx.add_metadata("crew_name".to_string(), crew.name.clone());

        // Augment the query with role instructions
        let augmented_query = format!(
            "{}\n\nYour role: {}\nYour goal: {}{}",
            task,
            member.role,
            member.goal,
            if accumulated_context.is_empty() {
                String::new()
            } else {
                format!("\n\nPrevious team member outputs:\n{}", accumulated_context)
            }
        );
        ctx.query = Some(augmented_query);

        // Execute agent
        let result = agent_system.execute_agent(&member.agent_id, ctx).await?;

        let duration_ms = agent_start.elapsed().as_millis() as u64;

        let agent_output = CrewAgentOutput {
            agent_id: member.agent_id.clone(),
            agent_name: agent_name.clone(),
            role: member.role.clone(),
            output: result.response.clone(),
            execution_time_ms: duration_ms,
            tools_used: result.tools_used.clone(),
        };

        // Emit tool_call_complete so the bubble shows success + duration
        emit_event(
            emitter,
            "tool_call_complete",
            serde_json::json!({
                "tool_name": tool_label,
                "result": if result.response.len() > 200 {
                    format!("{}...", &result.response[..200])
                } else {
                    result.response.clone()
                },
                "success": result.success,
                "duration_ms": duration_ms,
            }),
        );

        // Stream the agent's output
        accumulated_stream.push_str(&result.response);
        accumulated_stream.push_str("\n\n");
        emit_event(
            emitter,
            "chat_token",
            serde_json::json!({
                "token": format!("{}\n\n", result.response),
                "accumulated": accumulated_stream,
            }),
        );

        // Accumulate context for next agent
        accumulated_context.push_str(&format!(
            "\n--- {} ({}) ---\n{}\n",
            agent_name, member.role, result.response
        ));

        agent_outputs.push(agent_output);
    }

    // Final output is the last agent's response
    let final_output = agent_outputs
        .last()
        .map(|o| o.output.clone())
        .unwrap_or_else(|| "No output produced".to_string());

    Ok(CrewExecutionResult {
        success: true,
        final_output,
        agent_outputs,
        execution_time_ms: 0, // Will be set by caller
        error: None,
    })
}

/// Hierarchical execution: coordinator delegates to specialists via tool calls.
///
/// Uses the existing `orchestrator.rs` patterns:
/// - `register_agent_tools()` to make crew members callable as tools
/// - The coordinator agent uses the tool-calling loop to decide which
///   specialists to invoke and how to synthesize their outputs.
async fn execute_hierarchical(
    crew: &CrewDefinition,
    task: &str,
    space_id: Option<&str>,
    agent_system: &super::AgentSystem,
    coordinator_id: &str,
    emitter: Option<&dyn EventEmitter>,
) -> Result<CrewExecutionResult> {
    // Verify coordinator exists in crew
    if !crew.agents.iter().any(|a| a.agent_id == coordinator_id) {
        bail!(
            "Coordinator '{}' is not a member of crew '{}'",
            coordinator_id,
            crew.name
        );
    }

    tracing::info!(
        crew = %crew.name,
        coordinator = %coordinator_id,
        members = crew.agents.len(),
        "Crew hierarchical: coordinator delegating to specialists"
    );

    // Get specialist names for the coordinator's system prompt
    let specialist_names: Vec<String> = crew
        .agents
        .iter()
        .filter(|a| a.agent_id != coordinator_id)
        .map(|a| format!("{} ({})", a.role, a.agent_id))
        .collect();

    let coordinator_name = agent_system
        .get_agent(coordinator_id)
        .await
        .map(|d| d.name)
        .unwrap_or_else(|_| "Coordinator".to_string());

    // Emit tool_call_start for coordinator
    emit_event(
        emitter,
        "tool_call_start",
        serde_json::json!({
            "tool_name": format!("{} (coordinator)", coordinator_name),
            "arguments": serde_json::json!({
                "specialists": specialist_names,
            }).to_string(),
        }),
    );

    // Build context for coordinator
    let mut ctx = AgentContext::with_query(task.to_string());
    if let Some(sid) = space_id {
        ctx = ctx.with_space_id(sid.to_string());
    }

    // Add crew info to context
    ctx.add_metadata("crew_name".to_string(), crew.name.clone());
    ctx.add_metadata("crew_role".to_string(), "coordinator".to_string());

    // Augment query with delegation instructions
    let augmented_query = format!(
        "{}\n\nYou are the coordinator of crew '{}'. \
         You have the following specialist agents available as tools: {}. \
         Delegate subtasks to specialists as needed, then synthesize a final response.",
        task,
        crew.name,
        specialist_names.join(", ")
    );
    ctx.query = Some(augmented_query);

    let coord_start = Instant::now();

    // Execute the coordinator — it will use AgentDelegateTool to call specialists
    let result = agent_system.execute_agent(coordinator_id, ctx).await?;

    let duration_ms = coord_start.elapsed().as_millis() as u64;

    // Emit tool_call_complete
    emit_event(
        emitter,
        "tool_call_complete",
        serde_json::json!({
            "tool_name": format!("{} (coordinator)", coordinator_name),
            "result": if result.response.len() > 200 {
                format!("{}...", &result.response[..200])
            } else {
                result.response.clone()
            },
            "success": result.success,
            "duration_ms": duration_ms,
        }),
    );

    // Stream the full output
    emit_event(
        emitter,
        "chat_token",
        serde_json::json!({
            "token": result.response.clone(),
            "accumulated": result.response.clone(),
        }),
    );

    let agent_outputs = vec![CrewAgentOutput {
        agent_id: coordinator_id.to_string(),
        agent_name: coordinator_name,
        role: "coordinator".to_string(),
        output: result.response.clone(),
        execution_time_ms: duration_ms,
        tools_used: result.tools_used.clone(),
    }];

    Ok(CrewExecutionResult {
        success: result.success,
        final_output: result.response,
        agent_outputs,
        execution_time_ms: 0, // Will be set by caller
        error: result.error,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crew_definition_serialization() {
        let crew = CrewDefinition {
            id: "crew-1".to_string(),
            name: "Research Team".to_string(),
            description: "A team that researches and writes".to_string(),
            agents: vec![
                CrewMember {
                    agent_id: "agent-1".to_string(),
                    role: "researcher".to_string(),
                    goal: "Find relevant information".to_string(),
                    order: 0,
                },
                CrewMember {
                    agent_id: "agent-2".to_string(),
                    role: "writer".to_string(),
                    goal: "Write a comprehensive summary".to_string(),
                    order: 1,
                },
            ],
            process: CrewProcess::Sequential,
            config: CrewConfig::default(),
        };

        let json = serde_json::to_string(&crew).unwrap();
        let deserialized: CrewDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "Research Team");
        assert_eq!(deserialized.agents.len(), 2);
    }

    #[test]
    fn test_hierarchical_process_serialization() {
        let process = CrewProcess::Hierarchical {
            coordinator_id: "coord-1".to_string(),
        };
        let json = serde_json::to_string(&process).unwrap();
        assert!(json.contains("hierarchical"));
        assert!(json.contains("coord-1"));
    }

    #[test]
    fn test_crew_config_defaults() {
        let config = CrewConfig::default();
        assert_eq!(config.timeout_seconds, 300);
        assert!(!config.verbose);
    }
}
