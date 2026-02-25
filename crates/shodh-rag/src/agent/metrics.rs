//! Agent Metrics Collection and Analysis
//!
//! Tracks execution history, performance metrics, and health status for all agents.

use super::context::AgentContext;
use super::executor::{ExecutionResult, ExecutionStep};
use anyhow::{Context as AnyhowContext, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Core Types
// ============================================================================

/// Complete record of an agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Unique execution ID
    pub id: String,

    /// Agent that was executed
    pub agent_id: String,
    pub agent_name: String,

    /// User/session info
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub space_id: Option<String>,

    /// Query and response
    pub query: String,
    pub response: Option<String>,

    /// Status
    pub status: ExecutionStatus,

    /// Timing
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub execution_time_ms: Option<u64>,

    /// Metrics
    pub steps_count: usize,
    pub tools_used: Vec<String>,
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub total_tokens: Option<usize>,

    /// Result
    pub success: bool,
    pub error_message: Option<String>,

    /// Detailed steps (optional, for drill-down)
    pub steps: Option<Vec<ExecutionStep>>,

    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Execution status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Aggregated metrics for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetrics {
    pub agent_id: String,
    pub date: NaiveDate,

    // Usage stats
    pub total_executions: usize,
    pub successful_executions: usize,
    pub failed_executions: usize,
    pub cancelled_executions: usize,

    // Performance
    pub avg_execution_time_ms: u64,
    pub p95_execution_time_ms: u64,
    pub p99_execution_time_ms: u64,
    pub min_execution_time_ms: u64,
    pub max_execution_time_ms: u64,

    // Tokens
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub avg_tokens_per_execution: usize,

    // Tools
    pub tool_usage: HashMap<String, usize>,
}

/// Agent health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHealthStatus {
    pub agent_id: String,
    pub status: HealthState,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub consecutive_failures: usize,
    pub health_score: f32, // 0.0 - 1.0
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Healthy,
    Degraded,
    Down,
}

// ============================================================================
// Metrics Collector
// ============================================================================

/// Collects and stores agent execution metrics
pub struct AgentMetricsCollector {
    /// In-memory cache of recent executions
    recent_executions: Arc<RwLock<Vec<ExecutionRecord>>>,

    /// In-memory cache of metrics by agent and date
    metrics_cache: Arc<RwLock<HashMap<String, AgentMetrics>>>,

    /// Health status cache
    health_cache: Arc<RwLock<HashMap<String, AgentHealthStatus>>>,

    /// Maximum number of recent executions to keep in memory
    max_recent_executions: usize,
}

impl AgentMetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            recent_executions: Arc::new(RwLock::new(Vec::new())),
            metrics_cache: Arc::new(RwLock::new(HashMap::new())),
            health_cache: Arc::new(RwLock::new(HashMap::new())),
            max_recent_executions: 1000, // Keep last 1000 executions in memory
        }
    }

    /// Record an agent execution
    pub async fn record_execution(
        &self,
        agent_id: &str,
        agent_name: &str,
        context: &AgentContext,
        result: &ExecutionResult,
        started_at: DateTime<Utc>,
    ) -> Result<String> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let completed_at = Utc::now();

        let record = ExecutionRecord {
            id: execution_id.clone(),
            agent_id: agent_id.to_string(),
            agent_name: agent_name.to_string(),
            user_id: context.user_info.as_ref().map(|u| u.user_id.clone()),
            session_id: Some(context.session_id.clone()),
            space_id: context.space_id.clone(),
            query: context.query.clone().unwrap_or_default(),
            response: Some(result.response.clone()),
            status: if result.success {
                ExecutionStatus::Completed
            } else {
                ExecutionStatus::Failed
            },
            started_at,
            completed_at: Some(completed_at),
            execution_time_ms: Some(result.execution_time_ms),
            steps_count: result.steps.len(),
            tools_used: result.tools_used.clone(),
            input_tokens: result
                .metadata
                .get("input_tokens")
                .and_then(|v| v.as_u64().map(|n| n as usize)),
            output_tokens: result
                .metadata
                .get("output_tokens")
                .and_then(|v| v.as_u64().map(|n| n as usize)),
            total_tokens: result
                .metadata
                .get("total_tokens")
                .and_then(|v| v.as_u64().map(|n| n as usize)),
            success: result.success,
            error_message: result.error.clone(),
            steps: Some(result.steps.clone()),
            metadata: result.metadata.clone(),
        };

        // Add to recent executions
        {
            let mut recent = self.recent_executions.write().await;
            recent.push(record.clone());

            // Keep only last N executions
            if recent.len() > self.max_recent_executions {
                recent.remove(0);
            }
        }

        // Update metrics
        self.update_metrics(&record).await?;

        // Update health status
        self.update_health(agent_id, result.success).await?;

        Ok(execution_id)
    }

    /// Update metrics for an agent
    async fn update_metrics(&self, record: &ExecutionRecord) -> Result<()> {
        let date = record.started_at.date_naive();
        let cache_key = format!("{}:{}", record.agent_id, date);

        let mut metrics_cache = self.metrics_cache.write().await;

        let metrics = metrics_cache
            .entry(cache_key.clone())
            .or_insert_with(|| AgentMetrics {
                agent_id: record.agent_id.clone(),
                date,
                total_executions: 0,
                successful_executions: 0,
                failed_executions: 0,
                cancelled_executions: 0,
                avg_execution_time_ms: 0,
                p95_execution_time_ms: 0,
                p99_execution_time_ms: 0,
                min_execution_time_ms: u64::MAX,
                max_execution_time_ms: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                avg_tokens_per_execution: 0,
                tool_usage: HashMap::new(),
            });

        // Update counts
        metrics.total_executions += 1;
        match record.status {
            ExecutionStatus::Completed => metrics.successful_executions += 1,
            ExecutionStatus::Failed => metrics.failed_executions += 1,
            ExecutionStatus::Cancelled => metrics.cancelled_executions += 1,
            _ => {}
        }

        // Update timing
        if let Some(exec_time) = record.execution_time_ms {
            metrics.min_execution_time_ms = metrics.min_execution_time_ms.min(exec_time);
            metrics.max_execution_time_ms = metrics.max_execution_time_ms.max(exec_time);

            // Recalculate average
            let total_time =
                (metrics.avg_execution_time_ms * (metrics.total_executions - 1) as u64) + exec_time;
            metrics.avg_execution_time_ms = total_time / metrics.total_executions as u64;
        }

        // Update token counts
        if let Some(input) = record.input_tokens {
            metrics.total_input_tokens += input;
        }
        if let Some(output) = record.output_tokens {
            metrics.total_output_tokens += output;
        }
        let total_tokens = metrics.total_input_tokens + metrics.total_output_tokens;
        if metrics.total_executions > 0 {
            metrics.avg_tokens_per_execution = total_tokens / metrics.total_executions;
        }

        // Update tool usage
        for tool in &record.tools_used {
            *metrics.tool_usage.entry(tool.clone()).or_insert(0) += 1;
        }

        Ok(())
    }

    /// Update health status for an agent
    async fn update_health(&self, agent_id: &str, success: bool) -> Result<()> {
        let mut health_cache = self.health_cache.write().await;

        let health =
            health_cache
                .entry(agent_id.to_string())
                .or_insert_with(|| AgentHealthStatus {
                    agent_id: agent_id.to_string(),
                    status: HealthState::Healthy,
                    last_success_at: None,
                    last_failure_at: None,
                    consecutive_failures: 0,
                    health_score: 1.0,
                    checked_at: Utc::now(),
                });

        let now = Utc::now();

        if success {
            health.last_success_at = Some(now);
            health.consecutive_failures = 0;
        } else {
            health.last_failure_at = Some(now);
            health.consecutive_failures += 1;
        }

        // Calculate health score (last 100 executions)
        health.health_score = self.calculate_health_score(agent_id).await;

        // Determine health state
        health.status = if health.health_score >= 0.95 {
            HealthState::Healthy
        } else if health.health_score >= 0.7 {
            HealthState::Degraded
        } else {
            HealthState::Down
        };

        health.checked_at = now;

        Ok(())
    }

    /// Calculate health score for an agent (0.0 - 1.0)
    async fn calculate_health_score(&self, agent_id: &str) -> f32 {
        let recent = self.recent_executions.read().await;

        let agent_executions: Vec<_> = recent
            .iter()
            .rev()
            .filter(|e| e.agent_id == agent_id)
            .take(100)
            .collect();

        if agent_executions.is_empty() {
            return 1.0;
        }

        let successful = agent_executions.iter().filter(|e| e.success).count();
        successful as f32 / agent_executions.len() as f32
    }

    /// Get recent executions for an agent
    pub async fn get_agent_history(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ExecutionRecord>> {
        let recent = self.recent_executions.read().await;

        let agent_executions: Vec<ExecutionRecord> = recent
            .iter()
            .rev()
            .filter(|e| e.agent_id == agent_id)
            .take(limit)
            .cloned()
            .collect();

        Ok(agent_executions)
    }

    /// Get all recent executions
    pub async fn get_all_recent_executions(&self, limit: usize) -> Result<Vec<ExecutionRecord>> {
        let recent = self.recent_executions.read().await;

        Ok(recent.iter().rev().take(limit).cloned().collect())
    }

    /// Get metrics for an agent on a specific date
    pub async fn get_agent_metrics(
        &self,
        agent_id: &str,
        date: NaiveDate,
    ) -> Result<Option<AgentMetrics>> {
        let cache_key = format!("{}:{}", agent_id, date);
        let metrics_cache = self.metrics_cache.read().await;

        Ok(metrics_cache.get(&cache_key).cloned())
    }

    /// Get aggregated metrics for all agents
    pub async fn get_all_agents_metrics(&self) -> Result<HashMap<String, AgentMetrics>> {
        let metrics_cache = self.metrics_cache.read().await;

        // Group by agent_id, taking the most recent date
        let mut result = HashMap::new();

        for (_, metrics) in metrics_cache.iter() {
            result
                .entry(metrics.agent_id.clone())
                .and_modify(|existing: &mut AgentMetrics| {
                    // Update if this date is more recent
                    if metrics.date > existing.date {
                        *existing = metrics.clone();
                    }
                })
                .or_insert_with(|| metrics.clone());
        }

        Ok(result)
    }

    /// Get health status for an agent
    pub async fn get_agent_health(&self, agent_id: &str) -> Result<Option<AgentHealthStatus>> {
        let health_cache = self.health_cache.read().await;
        Ok(health_cache.get(agent_id).cloned())
    }

    /// Get health status for all agents
    pub async fn get_all_agents_health(&self) -> Result<HashMap<String, AgentHealthStatus>> {
        let health_cache = self.health_cache.read().await;
        Ok(health_cache.clone())
    }

    /// Get dashboard summary data
    pub async fn get_dashboard_summary(&self) -> Result<DashboardSummary> {
        let recent = self.recent_executions.read().await;
        let now = Utc::now();
        let last_24h = now - chrono::Duration::hours(24);

        let recent_24h: Vec<_> = recent.iter().filter(|e| e.started_at >= last_24h).collect();

        let total_runs = recent_24h.len();
        let successful_runs = recent_24h.iter().filter(|e| e.success).count();
        let failed_runs = recent_24h.iter().filter(|e| !e.success).count();

        let success_rate = if total_runs > 0 {
            (successful_runs as f32 / total_runs as f32) * 100.0
        } else {
            0.0
        };

        Ok(DashboardSummary {
            total_runs,
            successful_runs,
            failed_runs,
            success_rate,
            active_now: 0, // Will be set by monitor
        })
    }
}

impl Default for AgentMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Dashboard summary data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub total_runs: usize,
    pub successful_runs: usize,
    pub failed_runs: usize,
    pub success_rate: f32,
    pub active_now: usize,
}
