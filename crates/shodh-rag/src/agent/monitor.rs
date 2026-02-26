//! Real-time Agent Execution Monitoring
//!
//! Tracks currently running agents and provides progress updates.

use super::executor::{ExecutionResult, AgentProgress};
use super::context::AgentContext;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::mpsc;

// ============================================================================
// Active Execution Tracking
// ============================================================================

/// Information about a currently running agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveExecution {
    /// Unique execution ID
    pub execution_id: String,

    /// Agent being executed
    pub agent_id: String,
    pub agent_name: String,

    /// User/session info
    pub user_id: Option<String>,
    pub session_id: String,
    pub space_id: Option<String>,

    /// Query being processed
    pub query: String,

    /// Timing
    pub started_at: DateTime<Utc>,
    pub elapsed_ms: u64,

    /// Progress
    pub current_step: usize,
    pub total_steps: usize,
    pub progress_percentage: f32,
    pub current_step_type: String,
    pub current_message: String,

    /// Can be cancelled
    pub can_cancel: bool,
}

/// Real-time agent monitor
pub struct AgentMonitor {
    /// Map of execution_id -> ActiveExecution
    active_executions: Arc<DashMap<String, ActiveExecution>>,

    /// Map of execution_id -> cancel_token
    cancel_tokens: Arc<DashMap<String, Arc<AtomicBool>>>,

    /// Map of execution_id -> progress subscribers
    subscribers: Arc<DashMap<String, Vec<mpsc::UnboundedSender<AgentProgress>>>>,
}

impl AgentMonitor {
    /// Create a new agent monitor
    pub fn new() -> Self {
        Self {
            active_executions: Arc::new(DashMap::new()),
            cancel_tokens: Arc::new(DashMap::new()),
            subscribers: Arc::new(DashMap::new()),
        }
    }

    /// Start tracking a new execution
    pub async fn start_tracking(
        &self,
        execution_id: String,
        agent_id: String,
        agent_name: String,
        context: &AgentContext,
    ) -> Arc<AtomicBool> {
        let cancel_token = Arc::new(AtomicBool::new(false));

        let active = ActiveExecution {
            execution_id: execution_id.clone(),
            agent_id,
            agent_name,
            user_id: context.user_info.as_ref().map(|u| u.user_id.clone()),
            session_id: context.session_id.clone(),
            space_id: context.space_id.clone(),
            query: context.query.clone().unwrap_or_default(),
            started_at: Utc::now(),
            elapsed_ms: 0,
            current_step: 0,
            total_steps: 0,
            progress_percentage: 0.0,
            current_step_type: "Initializing".to_string(),
            current_message: "Starting agent execution...".to_string(),
            can_cancel: true,
        };

        self.active_executions.insert(execution_id.clone(), active);
        self.cancel_tokens.insert(execution_id, cancel_token.clone());

        cancel_token
    }

    /// Update execution progress
    pub async fn update_progress(&self, execution_id: &str, progress: AgentProgress) {
        if let Some(mut entry) = self.active_executions.get_mut(execution_id) {
            entry.current_step = progress.current_step;
            entry.total_steps = progress.total_steps;
            entry.progress_percentage = progress.percentage;
            entry.current_step_type = format!("{:?}", progress.step_type);
            entry.current_message = progress.message.clone();
            entry.elapsed_ms = progress.elapsed_ms;
        }

        // Notify all subscribers
        if let Some(subs) = self.subscribers.get(execution_id) {
            let subs_clone = subs.value().clone();
            drop(subs); // Release the lock

            for sender in subs_clone {
                let _ = sender.send(progress.clone());
            }
        }
    }

    /// Subscribe to progress updates for an execution
    pub async fn subscribe_to_execution(&self, execution_id: &str) -> Result<mpsc::UnboundedReceiver<AgentProgress>> {
        let (tx, rx) = mpsc::unbounded_channel();

        if let Some(mut subs) = self.subscribers.get_mut(execution_id) {
            subs.push(tx);
        } else {
            self.subscribers.insert(execution_id.to_string(), vec![tx]);
        }

        Ok(rx)
    }

    /// Mark execution as complete and remove from tracking
    pub async fn complete_execution(&self, execution_id: &str) {
        self.active_executions.remove(execution_id);
        self.cancel_tokens.remove(execution_id);
        self.subscribers.remove(execution_id);
    }

    /// Cancel a running execution
    pub fn cancel_execution(&self, execution_id: &str) -> Result<()> {
        if let Some(cancel_token) = self.cancel_tokens.get(execution_id) {
            cancel_token.store(true, Ordering::Relaxed);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Execution not found or already completed"))
        }
    }

    /// Get all currently active executions
    pub async fn get_active_executions(&self) -> Vec<ActiveExecution> {
        self.active_executions
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get specific execution status
    pub async fn get_execution_status(&self, execution_id: &str) -> Option<ActiveExecution> {
        self.active_executions.get(execution_id).map(|e| e.value().clone())
    }

    /// Get all active executions for a specific agent
    pub async fn get_agent_active_executions(&self, agent_id: &str) -> Vec<ActiveExecution> {
        self.active_executions
            .iter()
            .filter(|entry| entry.value().agent_id == agent_id)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get count of active executions
    pub async fn get_active_count(&self) -> usize {
        self.active_executions.len()
    }

    /// Check if execution was cancelled
    pub fn is_cancelled(&self, execution_id: &str) -> bool {
        if let Some(cancel_token) = self.cancel_tokens.get(execution_id) {
            cancel_token.load(Ordering::Relaxed)
        } else {
            false
        }
    }
}

impl Default for AgentMonitor {
    fn default() -> Self {
        Self::new()
    }
}
