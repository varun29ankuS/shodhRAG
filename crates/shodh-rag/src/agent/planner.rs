//! Advanced Planning Layer - Parallel execution, dependency resolution, cost optimization

use super::autonomous::*;
use anyhow::{Result, Context as AnyhowContext};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use chrono::{DateTime, Utc};

/// Enhanced task plan with parallel execution support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedTaskPlan {
    pub task_id: String,
    pub execution_graph: ExecutionGraph,
    pub estimated_duration_seconds: u64,
    pub estimated_cost_usd: f64,
    pub risk_level: RiskLevel,
    pub requires_approval: bool,
    pub alternative_plans: Vec<AlternativePlan>,
    pub created_at: DateTime<Utc>,
}

/// Execution graph with parallel execution support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub nodes: Vec<ExecutionNode>,
    pub edges: Vec<(String, String)>, // (from_id, to_id)
    pub parallel_groups: Vec<ParallelGroup>,
}

/// Execution node (enhanced step with metadata)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: String,
    pub step: PlanStep,
    pub level: usize, // Execution level (0 = first, higher = later)
    pub estimated_duration_seconds: u64,
    pub estimated_cost_usd: f64,
    pub risk_score: f64, // 0.0-1.0
    pub can_run_parallel: bool,
}

/// Group of steps that can run in parallel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelGroup {
    pub id: String,
    pub node_ids: Vec<String>,
    pub level: usize,
}

/// Risk level assessment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Alternative plan (fallback strategy)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativePlan {
    pub id: String,
    pub description: String,
    pub steps: Vec<PlanStep>,
    pub estimated_cost_usd: f64,
    pub use_case: String, // When to use this plan
}

/// Planning result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningResult {
    pub primary_plan: EnhancedTaskPlan,
    pub alternatives: Vec<AlternativePlan>,
    pub analysis: PlanningAnalysis,
}

/// Analysis of the planning process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningAnalysis {
    pub complexity_score: f64, // 0.0-1.0
    pub parallelization_factor: f64, // How much speedup from parallelization
    pub estimated_serial_duration_seconds: u64,
    pub estimated_parallel_duration_seconds: u64,
    pub cost_breakdown: HashMap<String, f64>,
    pub risk_factors: Vec<String>,
}

/// Advanced planner with parallel execution
pub struct AdvancedPlanner {
    cost_calculator: CostCalculator,
    risk_assessor: RiskAssessor,
}

impl AdvancedPlanner {
    pub fn new() -> Self {
        Self {
            cost_calculator: CostCalculator::new(),
            risk_assessor: RiskAssessor::new(),
        }
    }

    /// Create an enhanced plan with parallel execution
    pub fn create_enhanced_plan(
        &self,
        task: &Task,
        base_steps: Vec<PlanStep>,
    ) -> Result<EnhancedTaskPlan> {
        tracing::info!("Creating enhanced execution plan");

        // Build dependency graph
        let execution_graph = self.build_execution_graph(&base_steps)?;
        tracing::info!(nodes = execution_graph.nodes.len(), "Built execution graph");
        tracing::info!(parallel_groups = execution_graph.parallel_groups.len(), "Identified parallel groups");

        // Calculate costs and duration
        let (estimated_cost, cost_breakdown) = self.calculate_total_cost(&execution_graph);
        let serial_duration = self.calculate_serial_duration(&execution_graph);
        let parallel_duration = self.calculate_parallel_duration(&execution_graph);

        tracing::info!(estimated_cost_usd = %estimated_cost, "Estimated cost");
        tracing::info!(
            serial_seconds = serial_duration,
            parallel_seconds = parallel_duration,
            speedup = serial_duration as f64 / parallel_duration as f64,
            "Duration estimates"
        );

        // Assess risk
        let (risk_level, risk_factors) = self.assess_overall_risk(&execution_graph);
        tracing::info!(risk_level = ?risk_level, "Risk assessment complete");

        // Check if approval is needed
        let requires_approval = task.constraints.require_human_approval
            || risk_level >= RiskLevel::High
            || estimated_cost > task.constraints.max_cost_usd.unwrap_or(f64::MAX);

        // Generate alternative plans
        let alternatives = self.generate_alternatives(task, &base_steps)?;
        if !alternatives.is_empty() {
            tracing::info!(count = alternatives.len(), "Generated alternative plans");
        }

        Ok(EnhancedTaskPlan {
            task_id: task.id.clone(),
            execution_graph,
            estimated_duration_seconds: parallel_duration,
            estimated_cost_usd: estimated_cost,
            risk_level,
            requires_approval,
            alternative_plans: alternatives,
            created_at: Utc::now(),
        })
    }

    /// Build execution graph with dependency resolution
    fn build_execution_graph(&self, steps: &[PlanStep]) -> Result<ExecutionGraph> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut step_levels: HashMap<String, usize> = HashMap::new();

        // First pass: Calculate execution levels using topological sort
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for step in steps {
            in_degree.insert(step.id.clone(), step.dependencies.len());
        }

        let mut queue: VecDeque<String> = steps.iter()
            .filter(|s| s.dependencies.is_empty())
            .map(|s| s.id.clone())
            .collect();

        let mut level = 0;
        while !queue.is_empty() {
            let level_size = queue.len();
            for _ in 0..level_size {
                if let Some(step_id) = queue.pop_front() {
                    step_levels.insert(step_id.clone(), level);

                    // Add dependent steps to queue
                    for step in steps {
                        if step.dependencies.contains(&step_id) {
                            if let Some(count) = in_degree.get_mut(&step.id) {
                                *count -= 1;
                                if *count == 0 {
                                    queue.push_back(step.id.clone());
                                }
                            }
                        }
                    }
                }
            }
            level += 1;
        }

        // Create nodes with metadata
        for step in steps {
            let level = *step_levels.get(&step.id).unwrap_or(&0);
            let (cost, duration) = self.cost_calculator.estimate_step(&step.action);
            let risk = self.risk_assessor.assess_step(&step.action);

            nodes.push(ExecutionNode {
                id: step.id.clone(),
                step: step.clone(),
                level,
                estimated_duration_seconds: duration,
                estimated_cost_usd: cost,
                risk_score: risk,
                can_run_parallel: true, // Can be refined based on action type
            });

            // Add edges
            for dep_id in &step.dependencies {
                edges.push((dep_id.clone(), step.id.clone()));
            }
        }

        // Identify parallel groups
        let parallel_groups = self.identify_parallel_groups(&nodes);

        Ok(ExecutionGraph {
            nodes,
            edges,
            parallel_groups,
        })
    }

    /// Identify groups of steps that can run in parallel
    fn identify_parallel_groups(&self, nodes: &[ExecutionNode]) -> Vec<ParallelGroup> {
        let mut groups = Vec::new();
        let mut level_groups: HashMap<usize, Vec<String>> = HashMap::new();

        // Group nodes by level
        for node in nodes {
            level_groups.entry(node.level)
                .or_insert_with(Vec::new)
                .push(node.id.clone());
        }

        // Create parallel groups for levels with multiple nodes
        for (level, node_ids) in level_groups {
            if node_ids.len() > 1 {
                groups.push(ParallelGroup {
                    id: format!("parallel_group_{}", level),
                    node_ids,
                    level,
                });
            }
        }

        groups
    }

    /// Calculate total cost
    fn calculate_total_cost(&self, graph: &ExecutionGraph) -> (f64, HashMap<String, f64>) {
        let mut total = 0.0;
        let mut breakdown = HashMap::new();

        for node in &graph.nodes {
            total += node.estimated_cost_usd;
            breakdown.insert(node.id.clone(), node.estimated_cost_usd);
        }

        (total, breakdown)
    }

    /// Calculate serial duration (if executed sequentially)
    fn calculate_serial_duration(&self, graph: &ExecutionGraph) -> u64 {
        graph.nodes.iter().map(|n| n.estimated_duration_seconds).sum()
    }

    /// Calculate parallel duration (with parallel execution)
    fn calculate_parallel_duration(&self, graph: &ExecutionGraph) -> u64 {
        let mut level_durations: HashMap<usize, u64> = HashMap::new();

        for node in &graph.nodes {
            let current_max = level_durations.get(&node.level).copied().unwrap_or(0);
            level_durations.insert(node.level, current_max.max(node.estimated_duration_seconds));
        }

        level_durations.values().sum()
    }

    /// Assess overall risk
    fn assess_overall_risk(&self, graph: &ExecutionGraph) -> (RiskLevel, Vec<String>) {
        let mut max_risk = 0.0;
        let mut risk_factors = Vec::new();

        for node in &graph.nodes {
            if node.risk_score > max_risk {
                max_risk = node.risk_score;
            }

            if node.risk_score > 0.7 {
                risk_factors.push(format!("High-risk step: {}", node.step.description));
            }
        }

        let risk_level = if max_risk >= 0.8 {
            RiskLevel::Critical
        } else if max_risk >= 0.6 {
            RiskLevel::High
        } else if max_risk >= 0.3 {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };

        (risk_level, risk_factors)
    }

    /// Generate alternative plans
    fn generate_alternatives(&self, task: &Task, steps: &[PlanStep]) -> Result<Vec<AlternativePlan>> {
        let mut alternatives = Vec::new();

        // Alternative 1: Cost-optimized plan (use cheaper tools/methods)
        if let Ok(alt) = self.create_cost_optimized_plan(steps) {
            alternatives.push(alt);
        }

        // Alternative 2: Fast plan (trade cost for speed)
        if let Ok(alt) = self.create_fast_plan(steps) {
            alternatives.push(alt);
        }

        // Alternative 3: Safe plan (minimize risks)
        if let Ok(alt) = self.create_safe_plan(steps) {
            alternatives.push(alt);
        }

        Ok(alternatives)
    }

    fn create_cost_optimized_plan(&self, steps: &[PlanStep]) -> Result<AlternativePlan> {
        Ok(AlternativePlan {
            id: uuid::Uuid::new_v4().to_string(),
            description: "Cost-optimized plan using cheaper tools and methods".to_string(),
            steps: steps.to_vec(), // In real impl, replace expensive tools
            estimated_cost_usd: 0.5, // Placeholder
            use_case: "When cost is the primary concern".to_string(),
        })
    }

    fn create_fast_plan(&self, steps: &[PlanStep]) -> Result<AlternativePlan> {
        Ok(AlternativePlan {
            id: uuid::Uuid::new_v4().to_string(),
            description: "Fast plan prioritizing speed over cost".to_string(),
            steps: steps.to_vec(), // In real impl, use faster but more expensive tools
            estimated_cost_usd: 2.0, // Placeholder
            use_case: "When time is critical".to_string(),
        })
    }

    fn create_safe_plan(&self, steps: &[PlanStep]) -> Result<AlternativePlan> {
        Ok(AlternativePlan {
            id: uuid::Uuid::new_v4().to_string(),
            description: "Safe plan with minimal risks and human oversight".to_string(),
            steps: steps.to_vec(), // In real impl, add approval steps
            estimated_cost_usd: 1.0, // Placeholder
            use_case: "When safety is paramount".to_string(),
        })
    }
}

impl Default for AdvancedPlanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Cost calculator for different action types
struct CostCalculator;

impl CostCalculator {
    fn new() -> Self {
        Self
    }

    fn estimate_step(&self, action: &StepAction) -> (f64, u64) {
        // Returns (cost_usd, duration_seconds)
        match action {
            StepAction::RagSearch { .. } => (0.001, 2),
            StepAction::CodeExecution { .. } => (0.01, 10),
            StepAction::ToolCall { .. } => (0.002, 3),
            StepAction::LlmQuery { .. } => (0.05, 5),
            StepAction::HumanApproval { .. } => (0.0, 60), // Human time is free but slow
        }
    }
}

/// Risk assessor for different action types
struct RiskAssessor;

impl RiskAssessor {
    fn new() -> Self {
        Self
    }

    fn assess_step(&self, action: &StepAction) -> f64 {
        // Returns risk score 0.0-1.0
        match action {
            StepAction::RagSearch { .. } => 0.1, // Low risk
            StepAction::CodeExecution { .. } => 0.7, // High risk (arbitrary code)
            StepAction::ToolCall { tool_name, .. } => {
                if tool_name.contains("delete") || tool_name.contains("remove") {
                    0.8 // Very high risk
                } else if tool_name.contains("write") || tool_name.contains("create") {
                    0.5 // Medium risk
                } else {
                    0.2 // Low risk (read operations)
                }
            }
            StepAction::LlmQuery { .. } => 0.2, // Low risk
            StepAction::HumanApproval { .. } => 0.0, // No risk (human oversight)
        }
    }
}
