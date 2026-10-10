//! Space management for organizing documents
//!
//! Provides CRUD operations for knowledge spaces with JSON-based persistence.
//! No Tauri dependency — pure business logic.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// Space structure representing a knowledge space
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub document_count: usize,
    pub last_active: String,
    pub is_shared: bool,
    pub new_insights: usize,
    pub folder_path: Option<String>,
    pub watching_changes: bool,
    pub documents: Vec<String>,
    pub metadata: HashMap<String, String>,
}

/// Space manager — CRUD operations with JSON persistence
pub struct SpaceManager {
    pub spaces: Mutex<Vec<Space>>,
    pub space_documents: Mutex<HashMap<String, Vec<String>>>,
    pub document_spaces: Mutex<HashMap<String, String>>,
    data_dir: PathBuf,
    /// Whether clearing also removes the legacy `vectora/spaces.json` files
    /// in the user's config and home folders.
    legacy_cleanup: bool,
}

impl SpaceManager {
    pub fn new() -> Self {
        Self::with_data_dir(PathBuf::from("./data"))
    }

    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self::migrate_old_data(&data_dir);

        let spaces = Self::load_spaces_from_dir(&data_dir).unwrap_or_else(|_| Vec::new());

        // Rebuild in-memory indexes from loaded space data
        let mut space_documents = HashMap::new();
        let mut document_spaces = HashMap::new();
        for space in &spaces {
            for doc_id in &space.documents {
                space_documents
                    .entry(space.id.clone())
                    .or_insert_with(Vec::new)
                    .push(doc_id.clone());
                document_spaces.insert(doc_id.clone(), space.id.clone());
            }
        }

        SpaceManager {
            spaces: Mutex::new(spaces),
            space_documents: Mutex::new(space_documents),
            document_spaces: Mutex::new(document_spaces),
            data_dir,
            legacy_cleanup: true,
        }
    }

    /// Never touch the legacy files outside `data_dir` (for a profile kept in
    /// its own folder, which does not own them).
    pub fn without_legacy_cleanup(mut self) -> Self {
        self.legacy_cleanup = false;
        self
    }

    fn migrate_old_data(new_data_dir: &PathBuf) {
        let old_data_dir = PathBuf::from("./data");
        let old_spaces_file = old_data_dir.join("spaces.json");

        if old_spaces_file.exists() {
            let new_spaces_file = new_data_dir.join("spaces.json");

            if !new_spaces_file.exists() {
                if let Err(e) = fs::create_dir_all(new_data_dir) {
                    tracing::warn!(error = %e, "Failed to create new data directory");
                    return;
                }

                if let Err(e) = fs::copy(&old_spaces_file, &new_spaces_file) {
                    tracing::warn!(error = %e, "Failed to migrate spaces data");
                    return;
                }

                let _ = fs::remove_file(&old_spaces_file);
            }
        }
    }

    fn load_spaces_from_dir(data_dir: &PathBuf) -> Result<Vec<Space>, std::io::Error> {
        if !data_dir.exists() {
            fs::create_dir_all(data_dir)?;
        }

        let spaces_file = data_dir.join("spaces.json");

        if spaces_file.exists() {
            let data = fs::read_to_string(spaces_file)?;
            let spaces: Vec<Space> = serde_json::from_str(&data)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(spaces)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Spaces file not found",
            ))
        }
    }

    pub fn save_spaces(&self) -> Result<(), String> {
        let spaces = self.spaces.lock().map_err(|e| e.to_string())?;

        fs::create_dir_all(&self.data_dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let spaces_file = self.data_dir.join("spaces.json");
        let data = serde_json::to_string_pretty(&*spaces)
            .map_err(|e| format!("Failed to serialize spaces: {}", e))?;

        fs::write(&spaces_file, data).map_err(|e| format!("Failed to write spaces file: {}", e))?;

        Ok(())
    }

    pub fn clear_all_spaces(&self) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        let mut space_docs = self.space_documents.lock().map_err(|e| e.to_string())?;
        let mut doc_spaces = self.document_spaces.lock().map_err(|e| e.to_string())?;

        spaces.clear();
        space_docs.clear();
        doc_spaces.clear();

        drop(spaces);
        drop(space_docs);
        drop(doc_spaces);

        // Delete the primary data file
        let primary_file = self.data_dir.join("spaces.json");
        if primary_file.exists() {
            let _ = std::fs::remove_file(&primary_file);
        }

        if !self.legacy_cleanup {
            return Ok(());
        }

        // Also clean up legacy locations
        if let Some(config_dir) = dirs::config_dir() {
            let spaces_file = config_dir.join("vectora").join("spaces.json");
            if spaces_file.exists() {
                let _ = std::fs::remove_file(&spaces_file);
            }
        }

        if let Some(home_dir) = dirs::home_dir() {
            let spaces_file = home_dir.join(".vectora").join("spaces.json");
            if spaces_file.exists() {
                let _ = std::fs::remove_file(&spaces_file);
            }
        }

        Ok(())
    }

    pub fn add_document_to_space(&self, space_id: &str, document_id: String) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        let mut space_docs = self.space_documents.lock().map_err(|e| e.to_string())?;
        let mut doc_spaces = self.document_spaces.lock().map_err(|e| e.to_string())?;

        let space = spaces
            .iter_mut()
            .find(|s| s.id == space_id)
            .ok_or_else(|| "Space not found".to_string())?;

        if !space.documents.contains(&document_id) {
            space.documents.push(document_id.clone());
            space.document_count = space.documents.len();
            space.last_active = Utc::now().to_rfc3339();
        }

        space_docs
            .entry(space_id.to_string())
            .or_default()
            .push(document_id.clone());

        doc_spaces.insert(document_id, space_id.to_string());

        drop(spaces);
        drop(space_docs);
        drop(doc_spaces);

        self.save_spaces()?;
        Ok(())
    }

    pub fn get_spaces(&self) -> Result<Vec<Space>, String> {
        let spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        Ok(spaces.clone())
    }
}

impl Default for SpaceManager {
    fn default() -> Self {
        Self::new()
    }
}
