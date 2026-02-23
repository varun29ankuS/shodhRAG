//! Agent Framework - User-defined AI agents with custom behaviors
//!
//! This module provides a flexible agent framework that allows users to:
//! - Define custom AI agents with specific prompts and behaviors
//! - Equip agents with tools (RAG search, code analysis, document generation)
//! - Execute agents with context from conversations and knowledge base
//! - Manage agent lifecycles (create, update, delete, persist)
//!
//! Architecture:
//! - AgentDefinition: User-defined agent configuration (serializable)
//! - AgentExecutor: Runtime execution engine for agents
//! - AgentTools: Available tools that agents can use
//! - AgentContext: Execution context (conversation history, user data, etc.)

use anyhow::{Result, Context as AnyhowContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ============================================================================
// Common Types
// ============================================================================

/// Suggestion for user actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub confidence: f32,
    pub reason: String,
}

mod definition;
mod executor;
mod tools;
mod filesystem_tools;
mod context;
mod registry;
mod activity_tracker;
mod pattern_learner;
mod project_context;
pub mod conversation_continuity;
mod personal_assistant;
mod builtin_agents;
mod autonomous;
mod metrics;
mod monitor;
mod code_executor;
pub mod tool_loop;
pub mod rag_tools;
pub mod dynamic_tool;
pub mod orchestrator;
pub mod crew;
pub mod calendar_tools;
pub mod calendar_indexer;

pub use definition::{AgentDefinition, AgentConfig, AgentCapability, ToolConfig};
pub use executor::{AgentExecutor, ExecutionResult, ExecutionStep, AgentProgress, StepType};
pub use tools::{AgentTool, ToolRegistry, ToolResult, ToolInput, ToolDescription};
pub use filesystem_tools::{
    PermissionManager, FilePermission, PermissionRequest, PermissionDecision, PermissionScope,
    ReadFileTool, WriteFileTool, ListDirectoryTool, AuditEntry,
};
pub use context::{AgentContext, ConversationTurn, ContextVariable, UserInfo};
pub use registry::{AgentRegistry, AgentMetadata};
pub use activity_tracker::{
    Activity, ActivityType, ActivityTracker,
    ActivityStats, ActivityPatterns, ActivitySequence
};
pub use pattern_learner::{PatternLearner, ClickPatternData};
pub use project_context::ProjectContextManager;
pub use conversation_continuity::{ConversationManager, Conversation, Message, MessageRole};
pub use personal_assistant::{PersonalAssistant, AssistantResponse, DailySummary};
pub use builtin_agents::create_builtin_agents;
pub use autonomous::{
    AutonomousAgent, Task, TaskContext, TaskConstraints, TaskPlan, PlanStep,
    StepAction, StepStatus, TaskResult, TaskArtifact, ArtifactType,
    ExecutionProgress, TaskExecutor,
};
pub use metrics::{
    AgentMetricsCollector, ExecutionRecord, ExecutionStatus, AgentMetrics,
    AgentHealthStatus, HealthState, DashboardSummary,
};
pub use monitor::{AgentMonitor, ActiveExecution};
pub use code_executor::{
    CodeExecutor, CodeLanguage, ExecutionConfig, CodeExecutionResult,
    validate_code_safety,
};
pub use tool_loop::{
    run_tool_loop, run_tool_loop_stream, tool_descriptions_to_schemas,
    ToolLoopConfig, ToolLoopResult, ToolLoopEvent, ToolInvocation, ToolLoopEmitter,
};
pub use rag_tools::register_rag_tools;
pub use dynamic_tool::{DynamicTool, DynamicToolDef, ToolCallback, register_dynamic_tools};
pub use orchestrator::{AgentDelegateTool, register_agent_tools, create_coordinator_agent};
pub use crew::{
    CrewDefinition, CrewMember, CrewProcess, CrewConfig,
    CrewExecutionResult, CrewAgentOutput, execute_crew,
};

use crate::llm::LLMManager;
use std::sync::atomic::{AtomicBool, Ordering};
use dashmap::DashMap;

/// Main agent system that ties everything together
pub struct AgentSystem {
    registry: Arc<RwLock<AgentRegistry>>,
    tool_registry: Arc<ToolRegistry>,
    /// Track running agent execution cancellation tokens
    running_agents: Arc<DashMap<String, Arc<AtomicBool>>>,
    /// Metrics collector for execution history and performance
    pub metrics_collector: Arc<AgentMetricsCollector>,
    /// Real-time execution monitor
    pub monitor: Arc<AgentMonitor>,
    /// Shared reference to the LLM manager for real LLM-driven agent execution.
    /// This is the same Arc that RagState holds — no cloning of LLMManager needed.
    llm_manager_ref: Option<Arc<RwLock<Option<LLMManager>>>>,
    /// Registered crews (multi-agent teams)
    crews: Arc<RwLock<HashMap<String, crew::CrewDefinition>>>,
}

impl AgentSystem {
    /// Create a new agent system
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(AgentRegistry::new())),
            tool_registry: Arc::new(ToolRegistry::new()),
            running_agents: Arc::new(DashMap::new()),
            metrics_collector: Arc::new(AgentMetricsCollector::new()),
            monitor: Arc::new(AgentMonitor::new()),
            llm_manager_ref: None,
            crews: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check if the LLM manager reference has been set
    pub fn llm_manager_ref_is_none(&self) -> bool {
        self.llm_manager_ref.is_none()
    }

    /// Set the shared LLM manager reference so agents can make real LLM-driven decisions.
    /// Pass the same `Arc<RwLock<Option<LLMManager>>>` that your app state holds.
    pub fn set_llm_manager_ref(&mut self, llm_ref: Arc<RwLock<Option<LLMManager>>>) {
        self.llm_manager_ref = Some(llm_ref);
        tracing::info!("AgentSystem: LLM manager reference set, agents can now use tool-calling loop");
    }

    /// Get the LLM manager reference (for crew execution and other subsystems)
    pub fn get_llm_manager_ref(&self) -> Option<Arc<RwLock<Option<LLMManager>>>> {
        self.llm_manager_ref.clone()
    }

    /// Inject the live RAG engine into the tool registry so RAGSearchTool performs real searches.
    pub async fn set_rag_engine(&self, engine: Arc<RwLock<crate::rag_engine::RAGEngine>>) {
        self.tool_registry.set_rag_engine(engine).await;
        tracing::info!("AgentSystem: RAG engine injected into tool registry — agents can now search documents");
    }

    /// Cancel a running agent execution
    pub fn cancel_agent(&self, execution_id: &str) -> Result<()> {
        if let Some(cancel_token) = self.running_agents.get(execution_id) {
            cancel_token.store(true, Ordering::Relaxed);
            tracing::info!(execution_id = %execution_id, "Cancelled agent execution");
            Ok(())
        } else {
            anyhow::bail!("No running agent found with ID: {}", execution_id)
        }
    }

    /// Check if an agent is currently running
    pub fn is_agent_running(&self, execution_id: &str) -> bool {
        self.running_agents.contains_key(execution_id)
    }

    /// Get list of currently running agent IDs
    pub fn get_running_agents(&self) -> Vec<String> {
        self.running_agents.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Create a new agent system with built-in agents pre-registered
    pub async fn with_builtin_agents() -> Self {
        let system = Self::new();

        // Register all built-in agents
        let builtin = create_builtin_agents();
        for agent in builtin {
            if let Err(e) = system.register_agent(agent.clone()).await {
                tracing::error!(name = %agent.name, error = %e, "Failed to register built-in agent");
            } else {
                tracing::info!(name = %agent.name, "Registered agent");
            }
        }

        system
    }

    /// Register a new agent
    pub async fn register_agent(&self, definition: AgentDefinition) -> Result<String> {
        let mut registry = self.registry.write().await;
        registry.register(definition)
    }

    /// Get an agent by ID
    pub async fn get_agent(&self, agent_id: &str) -> Result<AgentDefinition> {
        let registry = self.registry.read().await;
        registry.get(agent_id)
    }

    /// List all registered agents
    pub async fn list_agents(&self) -> Result<Vec<AgentMetadata>> {
        let registry = self.registry.read().await;
        Ok(registry.list())
    }

    /// Update an agent
    pub async fn update_agent(&self, agent_id: &str, definition: AgentDefinition) -> Result<()> {
        let mut registry = self.registry.write().await;
        registry.update(agent_id, definition)
    }

    /// Delete an agent
    pub async fn delete_agent(&self, agent_id: &str) -> Result<()> {
        let mut registry = self.registry.write().await;
        registry.delete(agent_id)
    }

    /// Toggle agent enabled status
    pub async fn toggle_agent_enabled(&self, agent_id: &str, enabled: bool) -> Result<()> {
        let mut registry = self.registry.write().await;
        registry.toggle_enabled(agent_id, enabled)
    }

    /// Execute an agent with given context
    pub async fn execute_agent(
        &self,
        agent_id: &str,
        context: AgentContext,
    ) -> Result<ExecutionResult> {
        let definition = self.get_agent(agent_id).await?;
        let started_at = chrono::Utc::now();

        // Start tracking
        let execution_id = uuid::Uuid::new_v4().to_string();
        let cancel_token = self.monitor.start_tracking(
            execution_id.clone(),
            agent_id.to_string(),
            definition.name.clone(),
            &context,
        ).await;

        // Store cancel token for this execution
        self.running_agents.insert(execution_id.clone(), cancel_token.clone());

        // Execute agent with LLM manager if available
        let mut executor = AgentExecutor::new(
            definition.clone(),
            self.tool_registry.clone(),
            self.metrics_collector.clone(),
            self.monitor.clone(),
            execution_id.clone(),
        );
        if let Some(ref llm_ref) = self.llm_manager_ref {
            executor = executor.with_llm_manager_ref(llm_ref.clone());
        }
        let result = executor.execute(context.clone()).await;

        // Clean up tracking
        self.monitor.complete_execution(&execution_id).await;
        self.running_agents.remove(&execution_id);

        // Record metrics
        if let Ok(ref exec_result) = result {
            let _ = self.metrics_collector.record_execution(
                agent_id,
                &definition.name,
                &context,
                exec_result,
                started_at,
            ).await;
        }

        result
    }

    /// Get the tool registry for adding custom tools
    pub fn tool_registry(&self) -> Arc<ToolRegistry> {
        self.tool_registry.clone()
    }

    /// Load agents from a directory (YAML/JSON files)
    pub async fn load_agents_from_directory(&self, dir_path: &str) -> Result<Vec<String>> {
        let registry = self.registry.write().await;
        registry.load_from_directory(dir_path).await
    }

    /// Save all agents to a directory
    pub async fn save_agents_to_directory(&self, dir_path: &str) -> Result<()> {
        let registry = self.registry.read().await;
        registry.save_to_directory(dir_path).await
    }

    // ======================================================================
    // Crew Management
    // ======================================================================

    /// Register a new crew
    pub async fn register_crew(&self, mut crew_def: crew::CrewDefinition) -> Result<String> {
        if crew_def.id.is_empty() {
            crew_def.id = Uuid::new_v4().to_string();
        }
        let id = crew_def.id.clone();

        // Validate all agent IDs exist
        for member in &crew_def.agents {
            self.get_agent(&member.agent_id).await
                .map_err(|_| anyhow::anyhow!("Agent '{}' not found in registry", member.agent_id))?;
        }

        // Validate hierarchical coordinator
        if let crew::CrewProcess::Hierarchical { ref coordinator_id } = crew_def.process {
            if !crew_def.agents.iter().any(|a| a.agent_id == *coordinator_id) {
                anyhow::bail!("Coordinator '{}' must be a member of the crew", coordinator_id);
            }
        }

        let mut crews = self.crews.write().await;
        crews.insert(id.clone(), crew_def);
        tracing::info!(crew_id = %id, "Registered crew");
        Ok(id)
    }

    /// Get a crew by ID
    pub async fn get_crew(&self, crew_id: &str) -> Result<crew::CrewDefinition> {
        let crews = self.crews.read().await;
        crews.get(crew_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Crew '{}' not found", crew_id))
    }

    /// List all registered crews
    pub async fn list_crews(&self) -> Vec<crew::CrewDefinition> {
        let crews = self.crews.read().await;
        crews.values().cloned().collect()
    }

    /// Delete a crew
    pub async fn delete_crew(&self, crew_id: &str) -> Result<()> {
        let mut crews = self.crews.write().await;
        if crews.remove(crew_id).is_none() {
            anyhow::bail!("Crew '{}' not found", crew_id);
        }
        tracing::info!(crew_id = %crew_id, "Deleted crew");
        Ok(())
    }

    /// Execute a crew task with optional streaming progress
    pub async fn execute_crew(
        &self,
        crew_id: &str,
        task: &str,
        space_id: Option<&str>,
        emitter: Option<&dyn crate::chat::EventEmitter>,
    ) -> Result<crew::CrewExecutionResult> {
        let crew_def = self.get_crew(crew_id).await?;
        tracing::info!(
            crew = %crew_def.name,
            task = %task.chars().take(80).collect::<String>(),
            "Starting crew execution"
        );
        crew::execute_crew(&crew_def, task, space_id, self, emitter).await
    }
}

impl Default for AgentSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_system_creation() {
        let system = AgentSystem::new();
        let agents = system.list_agents().await.unwrap();
        assert_eq!(agents.len(), 0);
    }

    #[tokio::test]
    async fn test_agent_registration() {
        let system = AgentSystem::new();

        let definition = AgentDefinition {
            id: Uuid::new_v4().to_string(),
            name: "TestAgent".to_string(),
            description: "A test agent".to_string(),
            system_prompt: "You are a helpful assistant".to_string(),
            config: AgentConfig::default(),
            capabilities: vec![],
            tools: vec![],
            metadata: HashMap::new(),
        };

        let agent_id = system.register_agent(definition.clone()).await.unwrap();
        assert!(!agent_id.is_empty());

        let retrieved = system.get_agent(&agent_id).await.unwrap();
        assert_eq!(retrieved.name, "TestAgent");
    }
}
