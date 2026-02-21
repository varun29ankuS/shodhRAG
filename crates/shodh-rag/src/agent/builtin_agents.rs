//! Built-in AI Agents
//!
//! Agents are created dynamically by the LLM based on user needs.
//! Previously-created agents are persisted as YAML and loaded on restart.
//! This function exists as a hook for any future default agents.

use super::definition::AgentDefinition;

/// Returns an empty list — no hardcoded agents.
/// The LLM creates agents on-the-fly via Intent::AgentCreation,
/// and they persist to the agents/ directory as YAML files.
pub fn create_builtin_agents() -> Vec<AgentDefinition> {
    vec![]
}
