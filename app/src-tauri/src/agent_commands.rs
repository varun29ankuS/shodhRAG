use crate::rag_commands::RagState;
use serde::{Deserialize, Serialize};
use shodh_rag::agent::{
    AgentCapability, AgentConfig, AgentDefinition, ToolConfig,
    AgentContext, ConversationTurn,
    ExecutionResult,
};
use tauri::State;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub execution_count: u64,
    pub avg_execution_time_ms: u64,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDashboard {
    pub agents: Vec<AgentInfo>,
    pub total_runs: usize,
    pub successful_runs: usize,
    pub failed_runs: usize,
    pub success_rate: f32,
    pub active_now: usize,
    pub recent_executions: Vec<ExecutionLogEntry>,
    pub health: Vec<AgentHealthEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLogEntry {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub query: String,
    pub response: Option<String>,
    pub status: String,
    pub execution_time_ms: Option<u64>,
    pub steps_count: usize,
    pub tools_used: Vec<String>,
    pub success: bool,
    pub error_message: Option<String>,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHealthEntry {
    pub agent_id: String,
    pub status: String,
    pub health_score: f32,
    pub consecutive_failures: usize,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
}

#[tauri::command]
pub async fn get_agent_dashboard(
    rag_state: State<'_, RagState>,
) -> Result<AgentDashboard, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = match agent_system_guard.as_ref() {
        Some(arc) => arc.clone(),
        None => {
            return Ok(AgentDashboard {
                agents: vec![],
                total_runs: 0,
                successful_runs: 0,
                failed_runs: 0,
                success_rate: 0.0,
                active_now: 0,
                recent_executions: vec![],
                health: vec![],
            });
        }
    };
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;

    // Get agent list
    let agent_metas = system.list_agents().await.map_err(|e| e.to_string())?;
    let agents: Vec<AgentInfo> = agent_metas
        .iter()
        .map(|m| {
            let caps: Vec<String> = m.tags.clone();
            AgentInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                description: m.description.clone(),
                enabled: m.enabled,
                execution_count: m.execution_count,
                avg_execution_time_ms: m.avg_execution_time_ms,
                capabilities: caps,
            }
        })
        .collect();

    // Get dashboard summary
    let summary = system
        .metrics_collector
        .get_dashboard_summary()
        .await
        .unwrap_or_else(|_| shodh_rag::agent::DashboardSummary {
            total_runs: 0,
            successful_runs: 0,
            failed_runs: 0,
            success_rate: 0.0,
            active_now: 0,
        });

    // Get active count from monitor
    let active_now = system.monitor.get_active_count().await;

    // Get recent executions
    let recent = system
        .metrics_collector
        .get_all_recent_executions(50)
        .await
        .unwrap_or_default();

    let recent_executions: Vec<ExecutionLogEntry> = recent
        .into_iter()
        .map(|r| ExecutionLogEntry {
            id: r.id,
            agent_id: r.agent_id,
            agent_name: r.agent_name,
            query: r.query,
            response: r.response,
            status: format!("{:?}", r.status).to_lowercase(),
            execution_time_ms: r.execution_time_ms,
            steps_count: r.steps_count,
            tools_used: r.tools_used,
            success: r.success,
            error_message: r.error_message,
            started_at: r.started_at.to_rfc3339(),
        })
        .collect();

    // Get health for all agents
    let health_map = system
        .metrics_collector
        .get_all_agents_health()
        .await
        .unwrap_or_default();

    let health: Vec<AgentHealthEntry> = health_map
        .into_iter()
        .map(|(_, h)| AgentHealthEntry {
            agent_id: h.agent_id,
            status: format!("{:?}", h.status).to_lowercase(),
            health_score: h.health_score,
            consecutive_failures: h.consecutive_failures,
            last_success_at: h.last_success_at.map(|t| t.to_rfc3339()),
            last_failure_at: h.last_failure_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    Ok(AgentDashboard {
        agents,
        total_runs: summary.total_runs,
        successful_runs: summary.successful_runs,
        failed_runs: summary.failed_runs,
        success_rate: summary.success_rate,
        active_now,
        recent_executions,
        health,
    })
}

#[tauri::command]
pub async fn get_active_executions(
    rag_state: State<'_, RagState>,
) -> Result<Vec<serde_json::Value>, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = match agent_system_guard.as_ref() {
        Some(arc) => arc.clone(),
        None => return Ok(vec![]),
    };
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    let active = system.monitor.get_active_executions().await;

    let entries: Vec<serde_json::Value> = active
        .into_iter()
        .map(|a| {
            serde_json::json!({
                "execution_id": a.execution_id,
                "agent_id": a.agent_id,
                "agent_name": a.agent_name,
                "query": a.query,
                "started_at": a.started_at.to_rfc3339(),
                "elapsed_ms": a.elapsed_ms,
                "current_step": a.current_step,
                "total_steps": a.total_steps,
                "current_step_type": a.current_step_type,
                "current_message": a.current_message,
                "progress_percentage": a.progress_percentage,
            })
        })
        .collect();

    Ok(entries)
}

#[tauri::command]
pub async fn toggle_agent(
    agent_id: String,
    enabled: bool,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    system
        .toggle_agent_enabled(&agent_id, enabled)
        .await
        .map_err(|e| e.to_string())
}

/// Frontend agent definition (matches TypeScript AgentDefinition in AgentBuilder.tsx)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendAgentDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub enabled: bool,
    pub config: FrontendAgentConfig,
    pub capabilities: Vec<String>,
    pub tools: Vec<String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendAgentConfig {
    pub temperature: f32,
    pub max_tokens: usize,
    pub top_p: f32,
    pub stream: bool,
    pub max_tool_calls: usize,
    pub timeout_seconds: u64,
    pub auto_use_rag: bool,
    pub rag_top_k: usize,
}

fn parse_capability(s: &str) -> AgentCapability {
    match s {
        "RAGSearch" => AgentCapability::RAGSearch,
        "CodeAnalysis" | "CodeGeneration" => AgentCapability::CodeAnalysis,
        "DocumentGeneration" | "Summarization" => AgentCapability::DocumentGeneration,
        "ConversationMemory" => AgentCapability::ConversationMemory,
        "PatternLearning" => AgentCapability::PatternLearning,
        "CodeExecution" => AgentCapability::CodeExecution,
        "ExternalAPI" | "ToolUse" => AgentCapability::ExternalAPI,
        "FileManagement" => AgentCapability::FileManagement,
        "WebSearch" => AgentCapability::WebSearch,
        "Analysis" => AgentCapability::Custom("Analysis".to_string()),
        "Creative" => AgentCapability::Custom("Creative".to_string()),
        other => AgentCapability::Custom(other.to_string()),
    }
}

fn capability_to_string(cap: &AgentCapability) -> String {
    match cap {
        AgentCapability::RAGSearch => "RAGSearch".to_string(),
        AgentCapability::CodeAnalysis => "CodeAnalysis".to_string(),
        AgentCapability::DocumentGeneration => "DocumentGeneration".to_string(),
        AgentCapability::ConversationMemory => "ConversationMemory".to_string(),
        AgentCapability::PatternLearning => "PatternLearning".to_string(),
        AgentCapability::CodeExecution => "CodeExecution".to_string(),
        AgentCapability::ExternalAPI => "ExternalAPI".to_string(),
        AgentCapability::FileManagement => "FileManagement".to_string(),
        AgentCapability::WebSearch => "WebSearch".to_string(),
        AgentCapability::Custom(s) => s.clone(),
    }
}

fn frontend_to_backend(def: &FrontendAgentDefinition) -> AgentDefinition {
    AgentDefinition {
        id: def.id.clone(),
        name: def.name.clone(),
        description: def.description.clone(),
        system_prompt: def.system_prompt.clone(),
        config: AgentConfig {
            temperature: def.config.temperature,
            max_tokens: def.config.max_tokens,
            top_p: def.config.top_p,
            stream: def.config.stream,
            max_tool_calls: def.config.max_tool_calls,
            timeout_seconds: def.config.timeout_seconds,
            auto_use_rag: def.config.auto_use_rag,
            rag_top_k: def.config.rag_top_k,
        },
        capabilities: def.capabilities.iter().map(|c| parse_capability(c)).collect(),
        tools: def.tools.iter().map(|t| ToolConfig {
            tool_id: t.clone(),
            enabled: true,
            config: HashMap::new(),
            description: None,
        }).collect(),
        enabled: def.enabled,
        metadata: def.metadata.clone(),
    }
}

fn backend_to_frontend(def: &AgentDefinition) -> FrontendAgentDefinition {
    FrontendAgentDefinition {
        id: def.id.clone(),
        name: def.name.clone(),
        description: def.description.clone(),
        system_prompt: def.system_prompt.clone(),
        enabled: def.enabled,
        config: FrontendAgentConfig {
            temperature: def.config.temperature,
            max_tokens: def.config.max_tokens,
            top_p: def.config.top_p,
            stream: def.config.stream,
            max_tool_calls: def.config.max_tool_calls,
            timeout_seconds: def.config.timeout_seconds,
            auto_use_rag: def.config.auto_use_rag,
            rag_top_k: def.config.rag_top_k,
        },
        capabilities: def.capabilities.iter().map(|c| capability_to_string(c)).collect(),
        tools: def.tools.iter().map(|t| t.tool_id.clone()).collect(),
        metadata: def.metadata.clone(),
    }
}

/// Create a new agent
#[tauri::command]
pub async fn create_agent(
    definition: FrontendAgentDefinition,
    rag_state: State<'_, RagState>,
) -> Result<String, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let backend_def = frontend_to_backend(&definition);

    let system = agent_system_arc.read().await;
    let agent_id = system
        .register_agent(backend_def)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Agent created: {} ({})", definition.name, agent_id);
    Ok(agent_id)
}

/// Update an existing agent
#[tauri::command]
pub async fn update_agent(
    agent_id: String,
    definition: FrontendAgentDefinition,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let mut backend_def = frontend_to_backend(&definition);
    backend_def.id = agent_id.clone();

    let system = agent_system_arc.read().await;
    system
        .update_agent(&agent_id, backend_def)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Agent updated: {}", agent_id);
    Ok(())
}

/// Delete an agent
#[tauri::command]
pub async fn delete_agent(
    agent_id: String,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    system
        .delete_agent(&agent_id)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Agent deleted: {}", agent_id);
    Ok(())
}

/// Get a single agent's full definition
#[tauri::command]
pub async fn get_agent(
    agent_id: String,
    rag_state: State<'_, RagState>,
) -> Result<FrontendAgentDefinition, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    let def = system
        .get_agent(&agent_id)
        .await
        .map_err(|e| e.to_string())?;

    Ok(backend_to_frontend(&def))
}

/// List all agents (full definitions)
#[tauri::command]
pub async fn list_agents(
    rag_state: State<'_, RagState>,
) -> Result<Vec<FrontendAgentDefinition>, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = match agent_system_guard.as_ref() {
        Some(arc) => arc.clone(),
        None => return Ok(vec![]),
    };
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    let metas = system.list_agents().await.map_err(|e| e.to_string())?;

    let mut results = Vec::new();
    for meta in &metas {
        if let Ok(def) = system.get_agent(&meta.id).await {
            results.push(backend_to_frontend(&def));
        }
    }

    Ok(results)
}

/// Execute an agent with a query
#[tauri::command]
pub async fn execute_agent(
    agent_id: String,
    query: String,
    space_id: Option<String>,
    conversation_history: Option<Vec<serde_json::Value>>,
    rag_state: State<'_, RagState>,
) -> Result<ExecutionResult, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let mut context = AgentContext::with_query(query.clone());

    if let Some(sid) = space_id {
        context.space_id = Some(sid);
    }

    if let Some(history) = conversation_history {
        for entry in history {
            let role = entry.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = entry.get("content").and_then(|v| v.as_str()).unwrap_or("");
            context.add_conversation_turn(match role {
                "assistant" => ConversationTurn::assistant(content.to_string()),
                "system" => ConversationTurn::system(content.to_string()),
                _ => ConversationTurn::user(content.to_string()),
            });
        }
    }

    tracing::info!("Executing agent {} with query: {}", agent_id, query.chars().take(50).collect::<String>());

    let system = agent_system_arc.read().await;
    let result = system
        .execute_agent(&agent_id, context)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!(
        "Agent {} execution complete: success={}, time={}ms, tools={:?}",
        agent_id, result.success, result.execution_time_ms, result.tools_used
    );

    Ok(result)
}

// ============================================================================
// Crew Commands
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendCrewDefinition {
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub agents: Vec<FrontendCrewMember>,
    pub process: String, // "sequential" | "hierarchical"
    pub coordinator_id: Option<String>,
    #[serde(default)]
    pub config: FrontendCrewConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendCrewMember {
    pub agent_id: String,
    pub role: String,
    pub goal: String,
    pub order: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendCrewConfig {
    #[serde(default = "default_crew_timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub verbose: bool,
}

fn default_crew_timeout() -> u64 { 300 }

impl Default for FrontendCrewConfig {
    fn default() -> Self {
        Self { timeout_seconds: 300, verbose: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendCrewExecutionResult {
    pub success: bool,
    pub final_output: String,
    pub agent_outputs: Vec<FrontendCrewAgentOutput>,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendCrewAgentOutput {
    pub agent_id: String,
    pub agent_name: String,
    pub role: String,
    pub output: String,
    pub execution_time_ms: u64,
    pub tools_used: Vec<String>,
}

fn frontend_crew_to_backend(fc: &FrontendCrewDefinition) -> shodh_rag::agent::CrewDefinition {
    let process = match fc.process.as_str() {
        "hierarchical" => {
            let coord = fc.coordinator_id.clone().unwrap_or_else(|| {
                fc.agents.first().map(|a| a.agent_id.clone()).unwrap_or_default()
            });
            shodh_rag::agent::CrewProcess::Hierarchical { coordinator_id: coord }
        }
        _ => shodh_rag::agent::CrewProcess::Sequential,
    };

    shodh_rag::agent::CrewDefinition {
        id: fc.id.clone().unwrap_or_default(),
        name: fc.name.clone(),
        description: fc.description.clone(),
        agents: fc.agents.iter().map(|m| shodh_rag::agent::CrewMember {
            agent_id: m.agent_id.clone(),
            role: m.role.clone(),
            goal: m.goal.clone(),
            order: m.order,
        }).collect(),
        process,
        config: shodh_rag::agent::CrewConfig {
            timeout_seconds: fc.config.timeout_seconds,
            verbose: fc.config.verbose,
        },
    }
}

fn backend_crew_to_frontend(bc: &shodh_rag::agent::CrewDefinition) -> FrontendCrewDefinition {
    let (process_str, coordinator_id) = match &bc.process {
        shodh_rag::agent::CrewProcess::Sequential => ("sequential".to_string(), None),
        shodh_rag::agent::CrewProcess::Hierarchical { coordinator_id } => {
            ("hierarchical".to_string(), Some(coordinator_id.clone()))
        }
    };

    FrontendCrewDefinition {
        id: Some(bc.id.clone()),
        name: bc.name.clone(),
        description: bc.description.clone(),
        agents: bc.agents.iter().map(|m| FrontendCrewMember {
            agent_id: m.agent_id.clone(),
            role: m.role.clone(),
            goal: m.goal.clone(),
            order: m.order,
        }).collect(),
        process: process_str,
        coordinator_id,
        config: FrontendCrewConfig {
            timeout_seconds: bc.config.timeout_seconds,
            verbose: bc.config.verbose,
        },
    }
}

/// Create a new crew
#[tauri::command]
pub async fn create_crew(
    crew: FrontendCrewDefinition,
    rag_state: State<'_, RagState>,
) -> Result<String, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let backend_crew = frontend_crew_to_backend(&crew);
    let system = agent_system_arc.read().await;
    let crew_id = system.register_crew(backend_crew).await.map_err(|e| e.to_string())?;

    tracing::info!("Created crew: {} ({})", crew.name, crew_id);
    Ok(crew_id)
}

/// Get a crew by ID
#[tauri::command]
pub async fn get_crew(
    crew_id: String,
    rag_state: State<'_, RagState>,
) -> Result<FrontendCrewDefinition, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    let crew = system.get_crew(&crew_id).await.map_err(|e| e.to_string())?;
    Ok(backend_crew_to_frontend(&crew))
}

/// List all crews
#[tauri::command]
pub async fn list_crews(
    rag_state: State<'_, RagState>,
) -> Result<Vec<FrontendCrewDefinition>, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = match agent_system_guard.as_ref() {
        Some(arc) => arc.clone(),
        None => return Ok(vec![]),
    };
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    let crews = system.list_crews().await;
    Ok(crews.iter().map(backend_crew_to_frontend).collect())
}

/// Delete a crew
#[tauri::command]
pub async fn delete_crew(
    crew_id: String,
    rag_state: State<'_, RagState>,
) -> Result<(), String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    let system = agent_system_arc.read().await;
    system.delete_crew(&crew_id).await.map_err(|e| e.to_string())?;
    tracing::info!("Deleted crew: {}", crew_id);
    Ok(())
}

/// Execute a crew with a task (with streaming progress events)
#[tauri::command]
pub async fn execute_crew(
    crew_id: String,
    task: String,
    space_id: Option<String>,
    app_handle: tauri::AppHandle,
    rag_state: State<'_, RagState>,
) -> Result<FrontendCrewExecutionResult, String> {
    let agent_system_guard = rag_state.agent_system.read().await;
    let agent_system_arc = agent_system_guard
        .as_ref()
        .ok_or("Agent system not initialized")?
        .clone();
    drop(agent_system_guard);

    tracing::info!("Executing crew {} with task: {}", crew_id, task.chars().take(80).collect::<String>());

    let emitter = crate::chat_engine::TauriEventEmitter::new(app_handle);
    let emitter_ref: Option<&dyn shodh_rag::chat::EventEmitter> = Some(&emitter);

    let system = agent_system_arc.read().await;
    let result = system
        .execute_crew(&crew_id, &task, space_id.as_deref(), emitter_ref)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!(
        "Crew {} execution complete: success={}, time={}ms, agents={}",
        crew_id, result.success, result.execution_time_ms, result.agent_outputs.len()
    );

    Ok(FrontendCrewExecutionResult {
        success: result.success,
        final_output: result.final_output,
        agent_outputs: result.agent_outputs.iter().map(|o| FrontendCrewAgentOutput {
            agent_id: o.agent_id.clone(),
            agent_name: o.agent_name.clone(),
            role: o.role.clone(),
            output: o.output.clone(),
            execution_time_ms: o.execution_time_ms,
            tools_used: o.tools_used.clone(),
        }).collect(),
        execution_time_ms: result.execution_time_ms,
        error: result.error,
    })
}
