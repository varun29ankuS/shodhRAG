//! Agent Executor - Runtime execution engine for agents

use super::context::AgentContext;
use super::definition::AgentDefinition;
use super::tool_loop::{
    run_tool_loop, tool_descriptions_to_schemas, ToolLoopConfig, ToolLoopResult,
};
use super::tools::{ToolInput, ToolRegistry, ToolResult};
use crate::llm::{ChatMessage, LLMManager};
use anyhow::{Context as AnyhowContext, Result};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::RwLock;

/// Result of agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Final response from the agent
    pub response: String,

    /// Steps taken during execution
    pub steps: Vec<ExecutionStep>,

    /// Tools used during execution
    pub tools_used: Vec<String>,

    /// Total execution time
    pub execution_time_ms: u64,

    /// Whether execution was successful
    pub success: bool,

    /// Error message if execution failed
    pub error: Option<String>,

    /// Metadata about execution
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
}

/// Single step in agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// Step number
    pub step_number: usize,

    /// Type of step
    pub step_type: StepType,

    /// Timestamp when step started
    pub timestamp: u64,

    /// Duration of this step in milliseconds
    pub duration_ms: u64,

    /// Input to this step
    pub input: String,

    /// Output from this step
    pub output: String,

    /// Tool used (if any)
    pub tool_used: Option<String>,

    /// Whether this step was successful
    pub success: bool,
}

/// Type of execution step
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepType {
    /// Initial reasoning about what to do
    Reasoning,

    /// Tool invocation
    ToolCall,

    /// RAG search
    RAGSearch,

    /// LLM generation
    LLMGeneration,

    /// Final response synthesis
    FinalSynthesis,

    /// Error handling
    ErrorRecovery,
}

/// Progress update during agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProgress {
    /// Current step being executed
    pub current_step: usize,

    /// Total steps planned
    pub total_steps: usize,

    /// Current step type
    pub step_type: StepType,

    /// Human-readable description of what's happening
    pub message: String,

    /// Progress percentage (0-100)
    pub percentage: f32,

    /// Time elapsed in milliseconds
    pub elapsed_ms: u64,
}

/// Agent executor that runs agent logic
pub struct AgentExecutor {
    definition: AgentDefinition,
    tool_registry: Arc<ToolRegistry>,
    metrics_collector: Arc<super::metrics::AgentMetricsCollector>,
    monitor: Arc<super::monitor::AgentMonitor>,
    execution_id: String,
    llm_manager_ref: Option<Arc<RwLock<Option<LLMManager>>>>,
}

impl AgentExecutor {
    /// Create a new agent executor
    pub fn new(
        definition: AgentDefinition,
        tool_registry: Arc<ToolRegistry>,
        metrics_collector: Arc<super::metrics::AgentMetricsCollector>,
        monitor: Arc<super::monitor::AgentMonitor>,
        execution_id: String,
    ) -> Self {
        Self {
            definition,
            tool_registry,
            metrics_collector,
            monitor,
            execution_id,
            llm_manager_ref: None,
        }
    }

    /// Set the shared LLM manager reference for real LLM-driven execution
    pub fn with_llm_manager_ref(mut self, llm_ref: Arc<RwLock<Option<LLMManager>>>) -> Self {
        self.llm_manager_ref = Some(llm_ref);
        self
    }

    /// Execute the agent with given context
    pub async fn execute(&self, context: AgentContext) -> Result<ExecutionResult> {
        let start_time = Instant::now();
        let mut steps = Vec::new();
        let mut tools_used = Vec::new();
        let mut metadata = std::collections::HashMap::new();

        // Validate definition
        self.definition
            .validate()
            .context("Invalid agent definition")?;

        // Build execution plan
        let plan = self.build_execution_plan(&context).await?;
        metadata.insert("plan_steps".to_string(), serde_json::json!(plan.len()));

        // Execute plan steps
        let mut current_context = context.clone();
        let total_steps = plan.len();
        for (step_num, step) in plan.into_iter().enumerate() {
            // Send progress update
            let step_type = match &step {
                PlannedStep::Reasoning { .. } => StepType::Reasoning,
                PlannedStep::RAGSearch { .. } => StepType::RAGSearch,
                PlannedStep::LLMGeneration { .. } => StepType::LLMGeneration,
                PlannedStep::FinalSynthesis => StepType::FinalSynthesis,
            };

            let message = match &step {
                PlannedStep::Reasoning { .. } => "Analyzing your request...".to_string(),
                PlannedStep::RAGSearch { query, .. } => format!("Searching for '{}'...", query),
                PlannedStep::LLMGeneration { .. } => "Generating response...".to_string(),
                PlannedStep::FinalSynthesis => "Finalizing answer...".to_string(),
            };

            self.monitor
                .update_progress(
                    &self.execution_id,
                    AgentProgress {
                        current_step: step_num + 1,
                        total_steps,
                        step_type: step_type.clone(),
                        message,
                        percentage: ((step_num + 1) as f32 / total_steps as f32) * 100.0,
                        elapsed_ms: start_time.elapsed().as_millis() as u64,
                    },
                )
                .await;

            let step_result = self
                .execute_step(step_num + 1, step, &mut current_context)
                .await;

            match step_result {
                Ok(execution_step) => {
                    if let Some(ref tool) = execution_step.tool_used {
                        if !tools_used.contains(tool) {
                            tools_used.push(tool.clone());
                        }
                    }
                    steps.push(execution_step);
                }
                Err(e) => {
                    // Error recovery step
                    let error_step = ExecutionStep {
                        step_number: step_num + 1,
                        step_type: StepType::ErrorRecovery,
                        timestamp: current_timestamp(),
                        duration_ms: 0,
                        input: format!("Error: {}", e),
                        output: "Attempting recovery".to_string(),
                        tool_used: None,
                        success: false,
                    };
                    steps.push(error_step);

                    // Decide whether to continue or abort
                    if self.definition.config.max_tool_calls <= steps.len() {
                        return Ok(ExecutionResult {
                            response: format!("Execution failed: {}", e),
                            steps,
                            tools_used,
                            execution_time_ms: start_time.elapsed().as_millis() as u64,
                            success: false,
                            error: Some(e.to_string()),
                            metadata,
                        });
                    }
                }
            }

            // Check timeout
            if start_time.elapsed().as_secs() >= self.definition.config.timeout_seconds {
                metadata.insert("timeout".to_string(), serde_json::json!(true));
                break;
            }

            // Check max tool calls
            if steps.len() >= self.definition.config.max_tool_calls {
                metadata.insert("max_calls_reached".to_string(), serde_json::json!(true));
                break;
            }
        }

        // Synthesize final response
        let final_response = self.synthesize_response(&steps, &current_context).await?;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            response: final_response,
            steps,
            tools_used,
            execution_time_ms,
            success: true,
            error: None,
            metadata,
        })
    }

    /// Execute the agent with progress updates and cancellation support
    pub async fn execute_with_progress(
        &self,
        context: AgentContext,
        progress_tx: Option<mpsc::Sender<AgentProgress>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<ExecutionResult> {
        let start_time = Instant::now();
        let mut steps = Vec::new();
        let mut tools_used = Vec::new();
        let mut metadata = std::collections::HashMap::new();

        // Validate definition
        self.definition
            .validate()
            .context("Invalid agent definition")?;

        // Build execution plan
        let plan = self.build_execution_plan(&context).await?;
        let total_steps = plan.len();
        metadata.insert("plan_steps".to_string(), serde_json::json!(total_steps));

        // Send initial progress
        if let Some(ref tx) = progress_tx {
            let _ = tx
                .send(AgentProgress {
                    current_step: 0,
                    total_steps,
                    step_type: StepType::Reasoning,
                    message: format!("Starting agent '{}'...", self.definition.name),
                    percentage: 0.0,
                    elapsed_ms: 0,
                })
                .await;
        }

        // Execute plan steps
        let mut current_context = context.clone();
        for (step_num, step) in plan.into_iter().enumerate() {
            // Check for cancellation
            if cancel_token.load(Ordering::Relaxed) {
                return Ok(ExecutionResult {
                    response: "Agent execution cancelled by user".to_string(),
                    steps,
                    tools_used,
                    execution_time_ms: start_time.elapsed().as_millis() as u64,
                    success: false,
                    error: Some("Cancelled".to_string()),
                    metadata,
                });
            }

            // Send progress update
            let step_type = match &step {
                PlannedStep::Reasoning { .. } => StepType::Reasoning,
                PlannedStep::RAGSearch { .. } => StepType::RAGSearch,
                PlannedStep::LLMGeneration { .. } => StepType::LLMGeneration,
                PlannedStep::FinalSynthesis => StepType::FinalSynthesis,
            };

            let message = match &step {
                PlannedStep::Reasoning { .. } => "Analyzing your request...".to_string(),
                PlannedStep::RAGSearch { query, .. } => format!("Searching for '{}'...", query),
                PlannedStep::LLMGeneration { .. } => "Generating response...".to_string(),
                PlannedStep::FinalSynthesis => "Finalizing answer...".to_string(),
            };

            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(AgentProgress {
                        current_step: step_num + 1,
                        total_steps,
                        step_type: step_type.clone(),
                        message,
                        percentage: ((step_num + 1) as f32 / total_steps as f32) * 100.0,
                        elapsed_ms: start_time.elapsed().as_millis() as u64,
                    })
                    .await;
            }

            let step_result = self
                .execute_step(step_num + 1, step, &mut current_context)
                .await;

            match step_result {
                Ok(execution_step) => {
                    if let Some(ref tool) = execution_step.tool_used {
                        if !tools_used.contains(tool) {
                            tools_used.push(tool.clone());
                        }
                    }
                    steps.push(execution_step);
                }
                Err(e) => {
                    // Error recovery step
                    let error_step = ExecutionStep {
                        step_number: step_num + 1,
                        step_type: StepType::ErrorRecovery,
                        timestamp: current_timestamp(),
                        duration_ms: 0,
                        input: format!("Error: {}", e),
                        output: "Attempting recovery".to_string(),
                        tool_used: None,
                        success: false,
                    };
                    steps.push(error_step);

                    if self.definition.config.max_tool_calls <= steps.len() {
                        return Ok(ExecutionResult {
                            response: format!("Execution failed: {}", e),
                            steps,
                            tools_used,
                            execution_time_ms: start_time.elapsed().as_millis() as u64,
                            success: false,
                            error: Some(e.to_string()),
                            metadata,
                        });
                    }
                }
            }

            // Check timeout
            if start_time.elapsed().as_secs() >= self.definition.config.timeout_seconds {
                metadata.insert("timeout".to_string(), serde_json::json!(true));
                break;
            }

            // Check max tool calls
            if steps.len() >= self.definition.config.max_tool_calls {
                metadata.insert("max_calls_reached".to_string(), serde_json::json!(true));
                break;
            }
        }

        // Send completion progress
        if let Some(ref tx) = progress_tx {
            let _ = tx
                .send(AgentProgress {
                    current_step: total_steps,
                    total_steps,
                    step_type: StepType::FinalSynthesis,
                    message: "Completed!".to_string(),
                    percentage: 100.0,
                    elapsed_ms: start_time.elapsed().as_millis() as u64,
                })
                .await;
        }

        // Synthesize final response
        let final_response = self.synthesize_response(&steps, &current_context).await?;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            response: final_response,
            steps,
            tools_used,
            execution_time_ms,
            success: true,
            error: None,
            metadata,
        })
    }

    /// Detect if a query is purely visual (about analyzing images/screenshots)
    /// and doesn't need document context from RAG
    fn is_visual_query(query: &str) -> bool {
        let query_lower = query.to_lowercase();

        // Visual query patterns - queries that ask about image content
        let visual_patterns = [
            "what is in this screenshot",
            "what does this image show",
            "what can you see in",
            "describe this image",
            "describe this picture",
            "describe this screenshot",
            "what is in the image",
            "what is in the picture",
            "what's in this screenshot",
            "what's in this image",
            "analyze this screenshot",
            "analyze this image",
            "analyze this picture",
            "tell me about this image",
            "tell me about this screenshot",
            "explain this image",
            "explain this screenshot",
            "what am i looking at",
            "read this screenshot",
            "read this image",
            "ocr this",
            "extract text from",
        ];

        // Check if query matches visual patterns
        for pattern in visual_patterns.iter() {
            if query_lower.contains(pattern) {
                return true;
            }
        }

        // Additional heuristic: if query is very short and contains "screenshot" or "image"
        let words: Vec<&str> = query_lower.split_whitespace().collect();
        if words.len() <= 6
            && (query_lower.contains("screenshot")
                || query_lower.contains("image")
                || query_lower.contains("picture"))
        {
            // Likely a simple visual query like "What's this screenshot about?"
            return true;
        }

        false
    }

    /// Build execution plan based on context and agent capabilities
    async fn build_execution_plan(&self, context: &AgentContext) -> Result<Vec<PlannedStep>> {
        let mut plan = Vec::new();

        // Step 1: Initial reasoning
        plan.push(PlannedStep::Reasoning {
            prompt: self.build_initial_prompt(context),
        });

        // Step 2: RAG search if auto_use_rag is enabled
        // Skip RAG for visual queries that only need image analysis
        if let Some(query) = context
            .query
            .as_ref()
            .filter(|_| self.definition.config.auto_use_rag)
        {
            let is_visual = Self::is_visual_query(query);

            if !is_visual {
                plan.push(PlannedStep::RAGSearch {
                    query: query.clone(),
                    top_k: self.definition.config.rag_top_k,
                });
            } else {
                tracing::debug!(query = %query, "Skipping RAG search for visual query");
            }
        }

        // Step 3: LLM generation with context
        plan.push(PlannedStep::LLMGeneration {
            system_prompt: self.definition.system_prompt.clone(),
            temperature: self.definition.config.temperature,
        });

        // Step 4: Tool calls if needed (determined dynamically)
        // This will be handled during execution based on LLM output

        // Step 5: Final synthesis
        plan.push(PlannedStep::FinalSynthesis);

        Ok(plan)
    }

    /// Execute a single step
    async fn execute_step(
        &self,
        step_number: usize,
        step: PlannedStep,
        context: &mut AgentContext,
    ) -> Result<ExecutionStep> {
        let step_start = Instant::now();
        let timestamp = current_timestamp();

        match step {
            PlannedStep::Reasoning { prompt } => {
                // Reasoning step - analyze what needs to be done
                let output = format!(
                    "Analyzing query: '{}'. Available tools: [{}]",
                    context.query.as_ref().unwrap_or(&"<no query>".to_string()),
                    self.definition
                        .enabled_tools()
                        .iter()
                        .map(|t| t.tool_id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );

                Ok(ExecutionStep {
                    step_number,
                    step_type: StepType::Reasoning,
                    timestamp,
                    duration_ms: step_start.elapsed().as_millis() as u64,
                    input: prompt,
                    output,
                    tool_used: None,
                    success: true,
                })
            }

            PlannedStep::RAGSearch { query, top_k } => {
                // RAG search step
                let tool = self
                    .tool_registry
                    .get("rag_search")
                    .ok_or_else(|| anyhow::anyhow!("RAG search tool not found"))?;

                let tool_input = ToolInput {
                    tool_id: "rag_search".to_string(),
                    parameters: serde_json::json!({
                        "query": query,
                        "top_k": top_k,
                    }),
                };

                let result = tool.execute(tool_input, context.clone()).await?;

                // Add results to context
                if let Some(results) = result.data.get("results") {
                    context.add_variable("rag_results".to_string(), results.clone());
                }

                Ok(ExecutionStep {
                    step_number,
                    step_type: StepType::RAGSearch,
                    timestamp,
                    duration_ms: step_start.elapsed().as_millis() as u64,
                    input: format!("RAG search: '{}'", query),
                    output: result.output,
                    tool_used: Some("rag_search".to_string()),
                    success: result.success,
                })
            }

            PlannedStep::LLMGeneration {
                system_prompt,
                temperature,
            } => {
                // Try to acquire the shared LLM manager
                let llm_guard = if let Some(ref llm_ref) = self.llm_manager_ref {
                    let guard = llm_ref.read().await;
                    if guard.is_some() {
                        Some(guard)
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(ref llm_guard) = llm_guard {
                    let llm = llm_guard.as_ref().unwrap(); // Safe: checked above
                                                           // Build chat messages with system prompt and user query
                    let mut user_content = context.query.clone().unwrap_or_default();

                    // Inject RAG results if available
                    if let Some(rag_results) = context.get_variable("rag_results") {
                        let rag_text = match rag_results {
                            serde_json::Value::String(s) => s.clone(),
                            other => serde_json::to_string_pretty(other).unwrap_or_default(),
                        };
                        if !rag_text.is_empty() {
                            user_content = format!(
                                "{}\n\n---\nRelevant context from knowledge base:\n{}\n---",
                                user_content, rag_text
                            );
                        }
                    }

                    // Inject crew context if available (previous agent outputs)
                    if let Some(crew_ctx) = context.get_variable("crew_previous_outputs") {
                        let crew_text = match crew_ctx {
                            serde_json::Value::String(s) => s.clone(),
                            other => serde_json::to_string_pretty(other).unwrap_or_default(),
                        };
                        if !crew_text.is_empty() {
                            user_content = format!(
                                "{}\n\n---\nPrevious agent outputs:\n{}\n---",
                                user_content, crew_text
                            );
                        }
                    }

                    let mut messages = vec![
                        ChatMessage::system(&system_prompt),
                        ChatMessage::user(&user_content),
                    ];

                    // Add conversation history
                    for turn in context
                        .conversation_history
                        .iter()
                        .rev()
                        .take(6)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        match turn.role.as_str() {
                            "user" => messages
                                .insert(messages.len() - 1, ChatMessage::user(&turn.content)),
                            "assistant" => messages
                                .insert(messages.len() - 1, ChatMessage::assistant(&turn.content)),
                            _ => {}
                        }
                    }

                    // Get tool schemas from registry
                    let tool_descriptions = self.tool_registry.get_tool_descriptions();
                    let tool_schemas = tool_descriptions_to_schemas(&tool_descriptions);

                    let loop_config = ToolLoopConfig {
                        max_iterations: self.definition.config.max_tool_calls.min(10),
                        tool_timeout_secs: 30,
                        streaming: false,
                    };

                    // Run the ReAct tool-calling loop
                    let loop_result = run_tool_loop(
                        llm,
                        &self.tool_registry,
                        &mut messages,
                        &tool_schemas,
                        context,
                        &loop_config,
                        None,
                    )
                    .await
                    .context("Tool loop failed during LLM generation")?;

                    // Record tool invocations in context
                    let tools_used: Vec<String> = loop_result
                        .tool_invocations
                        .iter()
                        .map(|inv| inv.tool_name.clone())
                        .collect();

                    if !tools_used.is_empty() {
                        context.add_variable(
                            "llm_tools_used".to_string(),
                            serde_json::json!(tools_used),
                        );
                    }

                    // Store the LLM response for synthesis
                    context.add_variable(
                        "llm_response".to_string(),
                        serde_json::Value::String(loop_result.content.clone()),
                    );

                    Ok(ExecutionStep {
                        step_number,
                        step_type: StepType::LLMGeneration,
                        timestamp,
                        duration_ms: step_start.elapsed().as_millis() as u64,
                        input: user_content,
                        output: loop_result.content,
                        tool_used: if tools_used.is_empty() {
                            None
                        } else {
                            Some(tools_used.join(", "))
                        },
                        success: true,
                    })
                } else {
                    // Fallback when no LLM manager — descriptive placeholder
                    let output = format!(
                        "Generated response with temperature {} using system prompt: '{}'",
                        temperature,
                        if system_prompt.chars().count() > 50 {
                            format!("{}...", system_prompt.chars().take(50).collect::<String>())
                        } else {
                            system_prompt.clone()
                        }
                    );

                    Ok(ExecutionStep {
                        step_number,
                        step_type: StepType::LLMGeneration,
                        timestamp,
                        duration_ms: step_start.elapsed().as_millis() as u64,
                        input: system_prompt,
                        output,
                        tool_used: None,
                        success: true,
                    })
                }
            }

            PlannedStep::FinalSynthesis => {
                // Synthesis step - combine all information
                let output = "Synthesized final response from all execution steps".to_string();

                Ok(ExecutionStep {
                    step_number,
                    step_type: StepType::FinalSynthesis,
                    timestamp,
                    duration_ms: step_start.elapsed().as_millis() as u64,
                    input: "Synthesize response".to_string(),
                    output,
                    tool_used: None,
                    success: true,
                })
            }
        }
    }

    /// Synthesize final response from all steps
    async fn synthesize_response(
        &self,
        steps: &[ExecutionStep],
        context: &AgentContext,
    ) -> Result<String> {
        // If we have a real LLM response, use it directly
        if let Some(llm_response) = context.get_variable("llm_response") {
            if let Some(text) = llm_response.as_str() {
                if !text.is_empty() {
                    return Ok(text.to_string());
                }
            }
        }

        // Fallback: find the LLMGeneration step output
        if let Some(llm_step) = steps
            .iter()
            .find(|s| s.step_type == StepType::LLMGeneration && s.success)
        {
            if !llm_step.output.is_empty()
                && !llm_step
                    .output
                    .starts_with("Generated response with temperature")
            {
                return Ok(llm_step.output.clone());
            }
        }

        // Legacy fallback for non-LLM execution
        let outputs: Vec<String> = steps
            .iter()
            .filter(|s| s.success && s.step_type != StepType::Reasoning)
            .map(|s| s.output.clone())
            .collect();

        let mut response = String::new();
        if let Some(query) = &context.query {
            response.push_str(&format!("Query: {}\n\n", query));
        }
        response.push_str("Agent Response:\n");
        for (i, output) in outputs.iter().enumerate() {
            response.push_str(&format!("{}. {}\n", i + 1, output));
        }

        Ok(response)
    }

    /// Build initial prompt for reasoning
    fn build_initial_prompt(&self, context: &AgentContext) -> String {
        let mut prompt = self.definition.system_prompt.clone();
        prompt.push_str("\n\n");

        if let Some(query) = &context.query {
            prompt.push_str(&format!("User query: {}\n", query));
        }

        if !context.conversation_history.is_empty() {
            prompt.push_str("\nConversation history:\n");
            for turn in context.conversation_history.iter().take(5) {
                prompt.push_str(&format!("- {}: {}\n", turn.role, turn.content));
            }
        }

        prompt
    }
}

/// Planned step in execution
#[derive(Debug, Clone)]
enum PlannedStep {
    Reasoning {
        prompt: String,
    },
    RAGSearch {
        query: String,
        top_k: usize,
    },
    LLMGeneration {
        system_prompt: String,
        temperature: f32,
    },
    FinalSynthesis,
}

/// Get current timestamp in milliseconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::definition::{AgentConfig, AgentDefinition};

    #[tokio::test]
    async fn test_executor_creation() {
        let definition = AgentDefinition::new(
            "TestAgent".to_string(),
            "You are a test assistant".to_string(),
        );
        let tool_registry = Arc::new(ToolRegistry::new());
        let metrics_collector = Arc::new(super::super::metrics::AgentMetricsCollector::new());
        let monitor = Arc::new(super::super::monitor::AgentMonitor::new());
        let execution_id = uuid::Uuid::new_v4().to_string();
        let executor = AgentExecutor::new(
            definition,
            tool_registry,
            metrics_collector,
            monitor,
            execution_id,
        );

        // Executor created successfully
        assert_eq!(executor.definition.name, "TestAgent");
    }

    #[test]
    fn test_visual_query_detection() {
        // Visual queries - should return true
        assert!(AgentExecutor::is_visual_query(
            "what is in this screenshot?"
        ));
        assert!(AgentExecutor::is_visual_query("What does this image show?"));
        assert!(AgentExecutor::is_visual_query("Describe this picture"));
        assert!(AgentExecutor::is_visual_query(
            "what can you see in this image?"
        ));
        assert!(AgentExecutor::is_visual_query("analyze this screenshot"));
        assert!(AgentExecutor::is_visual_query("tell me about this image"));
        assert!(AgentExecutor::is_visual_query("explain this screenshot"));
        assert!(AgentExecutor::is_visual_query("What am I looking at?"));
        assert!(AgentExecutor::is_visual_query("OCR this image"));
        assert!(AgentExecutor::is_visual_query(
            "extract text from this screenshot"
        ));

        // Short visual queries
        assert!(AgentExecutor::is_visual_query("What's this screenshot?"));
        assert!(AgentExecutor::is_visual_query("Describe this image"));

        // Non-visual queries - should return false
        assert!(!AgentExecutor::is_visual_query(
            "What are active magnetic bearings?"
        ));
        assert!(!AgentExecutor::is_visual_query(
            "How do I implement a hash table in Rust?"
        ));
        assert!(!AgentExecutor::is_visual_query(
            "Tell me about Python generators"
        ));
        assert!(!AgentExecutor::is_visual_query(
            "Search for documents about machine learning"
        ));
        assert!(!AgentExecutor::is_visual_query(
            "What is the capital of France?"
        ));
        assert!(!AgentExecutor::is_visual_query(
            "How does the screenshot feature work in the codebase?"
        ));

        // Edge cases - longer queries with "screenshot" but not visual
        assert!(!AgentExecutor::is_visual_query(
            "How do I take a screenshot programmatically in Rust using the winit library?"
        ));
    }
}
