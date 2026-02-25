//! Personal Assistant Coordinator
//! Lightweight coordinator that brings together all agent components

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{
    Activity, ActivityTracker, AgentSystem, ClickPatternData, ConversationManager, PatternLearner,
    ProjectContextManager, Suggestion,
};
use crate::memory::MemorySystem;

/// Personal Assistant - coordinates all agent components
pub struct PersonalAssistant {
    agent_system: Arc<RwLock<AgentSystem>>,
    activity_tracker: Arc<ActivityTracker>,
    pattern_learner: Arc<PatternLearner>,
    project_context: Arc<ProjectContextManager>,
    conversation_manager: Arc<ConversationManager>,
    memory_system: Arc<RwLock<MemorySystem>>,
}

impl PersonalAssistant {
    pub async fn new(memory_system: Arc<RwLock<MemorySystem>>) -> Result<Self> {
        tracing::info!("Initializing PersonalAssistant with built-in agents");

        // Create agent system with all built-in agents registered
        let agent_system = Arc::new(RwLock::new(AgentSystem::with_builtin_agents().await));

        let activity_tracker = Arc::new(ActivityTracker::new()?);
        let pattern_learner = Arc::new(PatternLearner::new()?);
        let project_context = Arc::new(ProjectContextManager::new()?);
        let conversation_manager =
            Arc::new(ConversationManager::new_with_memory(memory_system.clone())?);

        tracing::info!("PersonalAssistant initialized successfully");

        Ok(Self {
            agent_system,
            activity_tracker,
            pattern_learner,
            project_context,
            conversation_manager,
            memory_system,
        })
    }

    // Accessors
    pub fn get_agent_system(&self) -> Arc<RwLock<AgentSystem>> {
        self.agent_system.clone()
    }

    pub fn get_activity_tracker(&self) -> Arc<ActivityTracker> {
        self.activity_tracker.clone()
    }

    pub fn get_pattern_learner(&self) -> Arc<PatternLearner> {
        self.pattern_learner.clone()
    }

    pub fn get_project_context(&self) -> Arc<ProjectContextManager> {
        self.project_context.clone()
    }

    pub fn get_conversation_manager(&self) -> Arc<ConversationManager> {
        self.conversation_manager.clone()
    }

    pub fn get_memory_system(&self) -> Arc<RwLock<MemorySystem>> {
        self.memory_system.clone()
    }

    // Convenience methods for common operations
    pub async fn track_activity(&self, activity: Activity) -> Result<()> {
        self.activity_tracker.record(activity.clone()).await?;
        self.pattern_learner.learn_from_activity(&activity).await?;
        Ok(())
    }

    pub async fn get_suggestions(&self) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();
        suggestions.extend(self.pattern_learner.get_time_based_suggestions().await?);
        suggestions.extend(self.pattern_learner.get_pattern_suggestions().await?);
        Ok(suggestions)
    }

    pub async fn get_click_patterns(&self, query: &str) -> Result<ClickPatternData> {
        self.pattern_learner
            .get_click_patterns_for_query(query)
            .await
    }

    pub async fn get_all_click_patterns(&self) -> Result<Vec<(String, ClickPatternData)>> {
        let patterns = self.pattern_learner.get_all_click_patterns().await?;
        Ok(patterns.into_iter().collect())
    }

    pub async fn calculate_personalization_boost(
        &self,
        result_id: &str,
        query: &str,
    ) -> Result<f32> {
        self.pattern_learner
            .calculate_personalization_boost(result_id, query)
            .await
    }

    pub async fn list_available_agents(&self) -> Result<Vec<crate::agent::AgentMetadata>> {
        let system = self.agent_system.read().await;
        system.list_agents().await
    }

    pub async fn get_current_agent_info(&self) -> Result<crate::agent::AgentDefinition> {
        // For now, return the first agent or a default
        let system = self.agent_system.read().await;
        let agents = system.list_agents().await?;
        if let Some(first) = agents.first() {
            system.get_agent(&first.id).await
        } else {
            Err(anyhow::anyhow!("No agents available"))
        }
    }

    pub async fn switch_agent(&self, agent_id: &str) -> Result<AssistantResponse> {
        // Switching handled by agent system
        Ok(AssistantResponse {
            message: format!("Switched to agent: {}", agent_id),
            suggestions: vec![],
        })
    }

    pub async fn chat(
        &self,
        message: &str,
        _session_id: Option<&str>,
    ) -> Result<AssistantResponse> {
        // Simplified chat - would integrate with agent execution
        Ok(AssistantResponse {
            message: format!("Processing: {}", message),
            suggestions: self.get_suggestions().await?,
        })
    }

    pub async fn get_greeting(&self) -> Result<AssistantResponse> {
        let suggestions = self.get_suggestions().await?;
        Ok(AssistantResponse {
            message: "Hello! How can I help you today?".to_string(),
            suggestions,
        })
    }

    pub async fn continue_from_last_session(&self) -> Result<AssistantResponse> {
        Ok(AssistantResponse {
            message: "Continuing from where we left off...".to_string(),
            suggestions: self.get_suggestions().await?,
        })
    }

    pub async fn switch_project_context(&self, project_name: &str) -> Result<AssistantResponse> {
        self.project_context.switch_to(project_name).await?;
        Ok(AssistantResponse {
            message: format!("Switched to project: {}", project_name),
            suggestions: vec![],
        })
    }

    pub async fn get_daily_summary(&self) -> Result<DailySummary> {
        // Simplified summary
        Ok(DailySummary {
            total_activities: 0,
            projects_worked_on: vec![],
            key_accomplishments: vec![],
        })
    }
}

#[derive(Debug, Clone)]
pub struct AssistantResponse {
    pub message: String,
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone)]
pub struct DailySummary {
    pub total_activities: usize,
    pub projects_worked_on: Vec<String>,
    pub key_accomplishments: Vec<String>,
}
