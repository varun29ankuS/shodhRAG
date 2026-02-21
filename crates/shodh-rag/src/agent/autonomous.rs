//! Autonomous Agent - Task execution with retry logic and self-correction

use anyhow::{Result, Context as AnyhowContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// Task to be executed autonomously
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub description: String,
    pub context: TaskContext,
    pub constraints: TaskConstraints,
}

/// Context for task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub user_intent: String,
    pub available_tools: Vec<String>,
    pub relevant_documents: Vec<String>,
    pub conversation_history: Vec<String>,
    pub current_state: HashMap<String, serde_json::Value>,
}

/// Constraints and limits for task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConstraints {
    pub max_iterations: usize,
    pub timeout_seconds: u64,
    pub require_human_approval: bool,
    pub allowed_tool_categories: Vec<String>,
    pub max_cost_usd: Option<f64>,
}

impl Default for TaskConstraints {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            timeout_seconds: 300,
            require_human_approval: false,
            allowed_tool_categories: vec![
                "filesystem".to_string(),
                "search".to_string(),
                "rag_system".to_string(),
            ],
            max_cost_usd: Some(1.0),
        }
    }
}

/// Decomposed task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub task_id: String,
    pub steps: Vec<PlanStep>,
    pub estimated_duration_seconds: u64,
    pub requires_approval: bool,
    pub created_at: DateTime<Utc>,
}

/// Individual step in a task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub action: StepAction,
    pub expected_output: String,
    pub dependencies: Vec<String>, // Step IDs that must complete first
    pub retry_count: usize,
    pub max_retries: usize,
    pub status: StepStatus,
}

/// Action to perform in a step
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StepAction {
    ToolCall {
        tool_name: String,
        parameters: serde_json::Value,
    },
    CodeExecution {
        code: String,
        language: String,
    },
    RagSearch {
        query: String,
        filters: Option<HashMap<String, String>>,
    },
    LlmQuery {
        prompt: String,
        context: Vec<String>,
    },
    HumanApproval {
        question: String,
        options: Vec<String>,
    },
}

/// Status of a plan step
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Blocked, // Waiting for dependencies
}

/// Result of task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub success: bool,
    pub final_output: String,
    pub steps_completed: usize,
    pub total_steps: usize,
    pub execution_time_seconds: u64,
    pub cost_usd: f64,
    pub error: Option<String>,
    pub artifacts: Vec<TaskArtifact>,
}

/// Artifact produced during task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub name: String,
    pub artifact_type: ArtifactType,
    pub content: serde_json::Value,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactType {
    File,
    Code,
    Data,
    Report,
    Visualization,
}

/// Execution progress update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionProgress {
    pub task_id: String,
    pub current_step: usize,
    pub total_steps: usize,
    pub current_step_description: String,
    pub progress_percentage: f32,
    pub elapsed_seconds: u64,
    pub estimated_remaining_seconds: u64,
}

/// Autonomous agent that can execute tasks with retry logic
pub struct AutonomousAgent {
    task_history: Vec<TaskResult>,
    current_execution: Option<String>, // Current task ID
}

impl AutonomousAgent {
    pub fn new() -> Self {
        Self {
            task_history: Vec::new(),
            current_execution: None,
        }
    }

    /// Decompose a task into executable steps
    pub async fn decompose_task(
        &self,
        task: &Task,
        llm_prompt_generator: impl Fn(&str, &TaskContext) -> String,
    ) -> Result<TaskPlan> {
        tracing::info!(task = %task.description, "Decomposing task");

        // Generate decomposition prompt
        let prompt = llm_prompt_generator(&task.description, &task.context);

        // In a real implementation, call LLM to decompose
        // For now, create a simple plan structure
        let steps = self.generate_plan_steps(task)?;

        let requires_approval = steps.iter().any(|s| {
            matches!(s.action, StepAction::CodeExecution { .. })
                || task.constraints.require_human_approval
        });

        Ok(TaskPlan {
            task_id: task.id.clone(),
            steps,
            estimated_duration_seconds: 60,
            requires_approval,
            created_at: Utc::now(),
        })
    }

    /// Execute a task plan with retry logic
    pub async fn execute_task_plan(
        &mut self,
        plan: &mut TaskPlan,
        executor: impl TaskExecutor,
        on_progress: impl Fn(ExecutionProgress),
    ) -> Result<TaskResult> {
        tracing::info!(task_id = %plan.task_id, steps = plan.steps.len(), "Executing task plan");

        self.current_execution = Some(plan.task_id.clone());
        let start_time = std::time::Instant::now();
        let mut steps_completed = 0;
        let mut artifacts = Vec::new();
        let total_cost = 0.0;

        // Execute steps in dependency order
        let total_steps = plan.steps.len();
        for i in 0..total_steps {
            // Check dependencies first (immutable borrow)
            let dependencies_met = {
                let step = &plan.steps[i];
                self.are_dependencies_met(&plan.steps, step)
            };

            if !dependencies_met {
                plan.steps[i].status = StepStatus::Blocked;
                continue;
            }

            // Get step description for progress (before mutable borrow)
            let step_description = plan.steps[i].description.clone();

            // Update status
            plan.steps[i].status = StepStatus::InProgress;

            // Emit progress
            on_progress(ExecutionProgress {
                task_id: plan.task_id.clone(),
                current_step: i + 1,
                total_steps,
                current_step_description: step_description.clone(),
                progress_percentage: (i as f32 / total_steps as f32) * 100.0,
                elapsed_seconds: start_time.elapsed().as_secs(),
                estimated_remaining_seconds: 60,
            });

            // Execute step with retry logic (mutable borrow)
            let step_result = {
                let step = &mut plan.steps[i];
                self.execute_step_with_retry(step, &executor).await
            };

            match step_result {
                Ok(artifact) => {
                    plan.steps[i].status = StepStatus::Completed;
                    steps_completed += 1;
                    if let Some(art) = artifact {
                        artifacts.push(art);
                    }
                    tracing::info!(step = i + 1, description = %step_description, "Step completed");
                }
                Err(e) => {
                    plan.steps[i].status = StepStatus::Failed;
                    tracing::error!(step = i + 1, error = %e, "Step failed");

                    // Get retry count for error message
                    let max_retries = plan.steps[i].max_retries;
                    let retry_count = plan.steps[i].retry_count;

                    // Decide whether to continue or abort
                    if retry_count >= max_retries {
                        let elapsed = start_time.elapsed().as_secs();
                        self.current_execution = None;

                        return Ok(TaskResult {
                            task_id: plan.task_id.clone(),
                            success: false,
                            final_output: String::new(),
                            steps_completed,
                            total_steps,
                            execution_time_seconds: elapsed,
                            cost_usd: total_cost,
                            error: Some(format!("Step {} failed after {} retries: {}", i + 1, max_retries, e)),
                            artifacts,
                        });
                    }
                }
            }
        }

        let elapsed = start_time.elapsed().as_secs();
        self.current_execution = None;

        // Collect final output from completed steps
        let final_output = self.synthesize_final_output(&artifacts);

        let result = TaskResult {
            task_id: plan.task_id.clone(),
            success: steps_completed == plan.steps.len(),
            final_output,
            steps_completed,
            total_steps: plan.steps.len(),
            execution_time_seconds: elapsed,
            cost_usd: total_cost,
            error: None,
            artifacts,
        };

        self.task_history.push(result.clone());
        tracing::info!(completed = steps_completed, total = plan.steps.len(), duration_seconds = elapsed, "Task completed");

        Ok(result)
    }

    /// Execute a single step with retry logic and self-correction
    async fn execute_step_with_retry(
        &self,
        step: &mut PlanStep,
        executor: &impl TaskExecutor,
    ) -> Result<Option<TaskArtifact>> {
        let mut last_error: Option<String> = None;

        while step.retry_count <= step.max_retries {
            match executor.execute_step(&step.action).await {
                Ok(artifact) => {
                    return Ok(Some(artifact));
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                    step.retry_count += 1;

                    if step.retry_count <= step.max_retries {
                        tracing::warn!(retry = step.retry_count, max_retries = step.max_retries, error = %e, "Retrying step");

                        // Self-correction: Adjust action based on error
                        self.attempt_self_correction(step, &e.to_string()).await?;

                        // Wait before retry (exponential backoff)
                        let delay = std::time::Duration::from_secs(2u64.pow(step.retry_count as u32));
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        anyhow::bail!("Step failed after {} retries: {}", step.max_retries, last_error.unwrap_or_default())
    }

    /// Attempt to correct the step based on error
    async fn attempt_self_correction(&self, step: &mut PlanStep, error: &str) -> Result<()> {
        // Simple correction strategies
        if error.contains("timeout") {
            // Increase timeout or simplify task
            tracing::debug!("Self-correction: Detected timeout, will retry with adjusted parameters");
        } else if error.contains("permission") {
            // Check permissions or use alternative approach
            tracing::debug!("Self-correction: Permission issue detected");
        } else if error.contains("not found") {
            // Verify resource exists or find alternative
            tracing::debug!("Self-correction: Resource not found, attempting alternative");
        }

        // In a real implementation, use LLM to suggest corrections
        // For now, keep the action the same for retry
        Ok(())
    }

    /// Check if all dependencies for a step are met
    fn are_dependencies_met(&self, steps: &[PlanStep], step: &PlanStep) -> bool {
        for dep_id in &step.dependencies {
            if let Some(dep_step) = steps.iter().find(|s| &s.id == dep_id) {
                if dep_step.status != StepStatus::Completed {
                    return false;
                }
            }
        }
        true
    }

    /// Generate plan steps (placeholder - real impl uses LLM)
    fn generate_plan_steps(&self, task: &Task) -> Result<Vec<PlanStep>> {
        // This is a simplified version
        // Real implementation would use LLM to analyze task and generate steps
        Ok(vec![
            PlanStep {
                id: "step_1".to_string(),
                description: format!("Analyze requirement: {}", task.description),
                action: StepAction::RagSearch {
                    query: task.description.clone(),
                    filters: None,
                },
                expected_output: "Relevant context from knowledge base".to_string(),
                dependencies: vec![],
                retry_count: 0,
                max_retries: 3,
                status: StepStatus::Pending,
            },
        ])
    }

    /// Synthesize final output from artifacts
    fn synthesize_final_output(&self, artifacts: &[TaskArtifact]) -> String {
        if artifacts.is_empty() {
            return "Task completed with no output artifacts.".to_string();
        }

        let mut output = String::from("Task completed successfully.\n\n");
        for (i, artifact) in artifacts.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", i + 1, artifact.name));
        }
        output
    }

    /// Get task execution history
    pub fn get_history(&self) -> &[TaskResult] {
        &self.task_history
    }

    /// Check if agent is currently executing a task
    pub fn is_busy(&self) -> bool {
        self.current_execution.is_some()
    }
}

/// Trait for task executors (implemented by different execution backends)
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute_step(&self, action: &StepAction) -> Result<TaskArtifact>;
}

impl Default for AutonomousAgent {
    fn default() -> Self {
        Self::new()
    }
}
