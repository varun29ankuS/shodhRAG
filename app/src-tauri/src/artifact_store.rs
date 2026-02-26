//! Artifact Store - Manages versioning and persistence of artifacts

use crate::chat_engine::{Artifact, ArtifactType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use anyhow::Result;

/// Artifact with version history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactWithHistory {
    pub current: Artifact,
    pub history: Vec<ArtifactVersion>,
}

/// Single version of an artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactVersion {
    pub version: u32,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub diff: Option<String>,
}

/// Artifact store
pub struct ArtifactStore {
    /// conversation_id -> list of artifacts
    artifacts_by_conversation: HashMap<String, Vec<String>>,

    /// artifact_id -> artifact with history
    artifacts: HashMap<String, ArtifactWithHistory>,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self {
            artifacts_by_conversation: HashMap::new(),
            artifacts: HashMap::new(),
        }
    }

    /// Add new artifact
    pub fn add_artifact(
        &mut self,
        conversation_id: &str,
        artifact: Artifact,
    ) -> String {
        let artifact_id = artifact.id.clone();

        // Add to conversation
        self.artifacts_by_conversation
            .entry(conversation_id.to_string())
            .or_insert_with(Vec::new)
            .push(artifact_id.clone());

        // Store artifact with history
        self.artifacts.insert(
            artifact_id.clone(),
            ArtifactWithHistory {
                current: artifact.clone(),
                history: vec![ArtifactVersion {
                    version: 1,
                    content: artifact.content.clone(),
                    timestamp: artifact.created_at,
                    diff: None,
                }],
            },
        );

        artifact_id
    }

    /// Get artifact by ID
    pub fn get_artifact(&self, artifact_id: &str) -> Option<&Artifact> {
        self.artifacts.get(artifact_id).map(|a| &a.current)
    }

    /// Update artifact content (creates new version)
    pub fn update_artifact(
        &mut self,
        artifact_id: &str,
        new_content: String,
    ) -> Result<()> {
        let artifact_with_history = self.artifacts.get_mut(artifact_id)
            .ok_or_else(|| anyhow::anyhow!("Artifact not found: {}", artifact_id))?;

        // Calculate diff (clone old content to avoid borrow issues)
        let old_content = artifact_with_history.current.content.clone();
        let diff = Self::calculate_diff_static(&old_content, &new_content);

        // Create new version
        let new_version = artifact_with_history.current.version + 1;
        artifact_with_history.history.push(ArtifactVersion {
            version: new_version,
            content: new_content.clone(),
            timestamp: Utc::now(),
            diff: Some(diff),
        });

        // Update current
        artifact_with_history.current.content = new_content;
        artifact_with_history.current.version = new_version;

        Ok(())
    }

    /// Static diff calculation helper
    fn calculate_diff_static(old: &str, new: &str) -> String {
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        let mut diff = String::new();

        // Simple line-by-line comparison
        for (i, (old_line, new_line)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
            if old_line != new_line {
                diff.push_str(&format!("Line {}: {} -> {}\n", i + 1, old_line, new_line));
            }
        }

        // Handle length differences
        if old_lines.len() > new_lines.len() {
            diff.push_str(&format!("Removed {} lines\n", old_lines.len() - new_lines.len()));
        } else if new_lines.len() > old_lines.len() {
            diff.push_str(&format!("Added {} lines\n", new_lines.len() - old_lines.len()));
        }

        diff
    }

    /// Get artifact history
    pub fn get_history(&self, artifact_id: &str) -> Option<Vec<&ArtifactVersion>> {
        self.artifacts.get(artifact_id).map(|a| {
            a.history.iter().collect()
        })
    }

    /// Get all artifacts for a conversation
    pub fn get_conversation_artifacts(
        &self,
        conversation_id: &str,
    ) -> Vec<&Artifact> {
        self.artifacts_by_conversation
            .get(conversation_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.get_artifact(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Simple diff calculation (line-based)
    fn calculate_diff(&self, old: &str, new: &str) -> String {
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        let mut diff = String::new();

        // Simple line-by-line comparison
        for (i, (old_line, new_line)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
            if old_line != new_line {
                diff.push_str(&format!("Line {}: {} -> {}\n", i + 1, old_line, new_line));
            }
        }

        // Handle length differences
        if old_lines.len() > new_lines.len() {
            diff.push_str(&format!("Removed {} lines\n", old_lines.len() - new_lines.len()));
        } else if new_lines.len() > old_lines.len() {
            diff.push_str(&format!("Added {} lines\n", new_lines.len() - old_lines.len()));
        }

        diff
    }
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}
