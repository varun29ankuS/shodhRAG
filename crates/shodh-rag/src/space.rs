//! Space management for organizing documents
//!
//! Provides CRUD operations for knowledge spaces with JSON-based persistence.
//! No Tauri dependency — pure business logic.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;
use chrono::Utc;

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
                space_documents.entry(space.id.clone())
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
        }
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
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Spaces file not found"))
        }
    }

    pub fn save_spaces(&self) -> Result<(), String> {
        let spaces = self.spaces.lock().map_err(|e| e.to_string())?;

        fs::create_dir_all(&self.data_dir)
            .map_err(|e| format!("Failed to create data directory: {}", e))?;

        let spaces_file = self.data_dir.join("spaces.json");
        let data = serde_json::to_string_pretty(&*spaces)
            .map_err(|e| format!("Failed to serialize spaces: {}", e))?;

        fs::write(&spaces_file, data)
            .map_err(|e| format!("Failed to write spaces file: {}", e))?;

        Ok(())
    }

    pub fn create_space(&self, name: String, emoji: String) -> Result<Space, String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;

        if spaces.iter().any(|s| s.name == name) {
            return Err(format!("A space with the name '{}' already exists", name));
        }

        let space = Space {
            id: Uuid::new_v4().to_string(),
            name: name.clone(),
            emoji: emoji.clone(),
            document_count: 0,
            last_active: Utc::now().to_rfc3339(),
            is_shared: false,
            new_insights: 0,
            folder_path: None,
            watching_changes: false,
            documents: Vec::new(),
            metadata: HashMap::new(),
        };

        spaces.push(space.clone());
        drop(spaces);

        self.save_spaces()?;
        Ok(space)
    }

    pub fn delete_space(&self, space_id: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;

        let index = spaces.iter().position(|s| s.id == space_id)
            .ok_or_else(|| "Space not found".to_string())?;

        spaces.remove(index);
        drop(spaces);

        let mut space_docs = self.space_documents.lock().map_err(|e| e.to_string())?;
        let mut doc_spaces = self.document_spaces.lock().map_err(|e| e.to_string())?;

        if let Some(doc_ids) = space_docs.remove(space_id) {
            for doc_id in doc_ids {
                doc_spaces.remove(&doc_id);
            }
        }

        drop(space_docs);
        drop(doc_spaces);

        self.save_spaces()?;
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

    pub fn rename_space(&self, space_id: &str, new_name: String) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;

        let space = spaces.iter_mut()
            .find(|s| s.id == space_id)
            .ok_or_else(|| "Space not found".to_string())?;

        space.name = new_name;
        space.last_active = Utc::now().to_rfc3339();

        drop(spaces);
        self.save_spaces()?;
        Ok(())
    }

    pub fn update_space_folder(&self, space_id: &str, folder_path: Option<String>) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;

        let space = spaces.iter_mut()
            .find(|s| s.id == space_id)
            .ok_or_else(|| "Space not found".to_string())?;

        space.folder_path = folder_path;
        space.last_active = Utc::now().to_rfc3339();

        drop(spaces);
        self.save_spaces()?;
        Ok(())
    }

    pub fn add_document_to_space(&self, space_id: &str, document_id: String) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        let mut space_docs = self.space_documents.lock().map_err(|e| e.to_string())?;
        let mut doc_spaces = self.document_spaces.lock().map_err(|e| e.to_string())?;

        let space = spaces.iter_mut()
            .find(|s| s.id == space_id)
            .ok_or_else(|| "Space not found".to_string())?;

        if !space.documents.contains(&document_id) {
            space.documents.push(document_id.clone());
            space.document_count = space.documents.len();
            space.last_active = Utc::now().to_rfc3339();
        }

        space_docs.entry(space_id.to_string())
            .or_default()
            .push(document_id.clone());

        doc_spaces.insert(document_id, space_id.to_string());

        drop(spaces);
        drop(space_docs);
        drop(doc_spaces);

        self.save_spaces()?;
        Ok(())
    }

    pub fn remove_document_from_space(&self, space_id: &str, document_id: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        let mut space_docs = self.space_documents.lock().map_err(|e| e.to_string())?;
        let mut doc_spaces = self.document_spaces.lock().map_err(|e| e.to_string())?;

        let space = spaces.iter_mut()
            .find(|s| s.id == space_id)
            .ok_or_else(|| "Space not found".to_string())?;

        space.documents.retain(|id| id != document_id);
        space.document_count = space.documents.len();
        space.last_active = Utc::now().to_rfc3339();

        if let Some(docs) = space_docs.get_mut(space_id) {
            docs.retain(|id| id != document_id);
        }

        doc_spaces.remove(document_id);

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

    pub fn get_space(&self, space_id: &str) -> Result<Space, String> {
        let spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        spaces.iter()
            .find(|s| s.id == space_id)
            .cloned()
            .ok_or_else(|| "Space not found".to_string())
    }

    pub fn get_space_documents(&self, space_id: &str) -> Result<Vec<String>, String> {
        let space_docs = self.space_documents.lock().map_err(|e| e.to_string())?;
        Ok(space_docs.get(space_id).cloned().unwrap_or_default())
    }

    pub fn set_space_metadata(&self, space_id: &str, key: &str, value: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        let space = spaces.iter_mut().find(|s| s.id == space_id)
            .ok_or_else(|| format!("Space '{}' not found", space_id))?;
        space.metadata.insert(key.to_string(), value.to_string());
        drop(spaces);
        self.save_spaces()
    }

    pub fn get_space_metadata(&self, space_id: &str, key: &str) -> Option<String> {
        let spaces = self.spaces.lock().ok()?;
        spaces.iter()
            .find(|s| s.id == space_id)
            .and_then(|s| s.metadata.get(key).cloned())
    }

    pub fn remove_space_metadata(&self, space_id: &str, key: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().map_err(|e| e.to_string())?;
        if let Some(space) = spaces.iter_mut().find(|s| s.id == space_id) {
            space.metadata.remove(key);
        }
        drop(spaces);
        self.save_spaces()
    }
}

impl Default for SpaceManager {
    fn default() -> Self {
        Self::new()
    }
}
