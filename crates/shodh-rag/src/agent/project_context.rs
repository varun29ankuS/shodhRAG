//! Project Context Manager

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub session_id: uuid::Uuid,
    pub project: Option<String>,
    pub current_focus: Option<String>,
    pub recent_activities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectContextManager {
    contexts: Arc<RwLock<HashMap<String, ProjectContext>>>,
    current_project: Arc<RwLock<Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    pub name: String,
    pub last_task: String,
    pub open_files: Vec<String>,
    pub recent_searches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectData {
    pub name: String,
    pub last_task: String,
}

impl ProjectContextManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            contexts: Arc::new(RwLock::new(HashMap::new())),
            current_project: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn save_project_state(&self, project: &str) -> Result<()> {
        // Save current project state
        Ok(())
    }

    pub async fn load_project_context(&self, project: &str) -> Result<Context> {
        Ok(Context {
            session_id: uuid::Uuid::new_v4(),
            project: Some(project.to_string()),
            current_focus: None,
            recent_activities: Vec::new(),
        })
    }

    pub async fn get_last_active_project(&self) -> Result<Option<String>> {
        Ok(self.current_project.read().await.clone())
    }

    pub async fn get_project_data(&self, project: &str) -> Result<ProjectData> {
        Ok(ProjectData {
            name: project.to_string(),
            last_task: "Previous task".to_string(),
        })
    }

    pub async fn switch_to(&self, project: &str) -> Result<()> {
        // Save current project state if there is one
        if let Some(current) = self.current_project.read().await.as_ref() {
            self.save_project_state(current).await?;
        }

        // Load new project context
        let _context = self.load_project_context(project).await?;

        // Update current project
        *self.current_project.write().await = Some(project.to_string());

        Ok(())
    }
}