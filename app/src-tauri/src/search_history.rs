//! Search history and suggestions management

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEntry {
    pub query: String,
    pub timestamp: String,
    pub space_id: Option<String>,
    pub result_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSuggestion {
    pub text: String,
    pub suggestion_type: SuggestionType,
    pub score: f32,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionType {
    History,
    Document,
    Query,
    Semantic,
}

pub struct SearchHistoryManager {
    history: HashMap<String, VecDeque<SearchEntry>>, // space_id -> history
    global_history: VecDeque<SearchEntry>,
    max_history_per_space: usize,
    storage_path: PathBuf,
}

impl SearchHistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self, String> {
        let app_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|e| format!("Failed to get app data directory: {}", e))?;

        let storage_path = app_dir.join("search_history.json");

        let mut manager = Self {
            history: HashMap::new(),
            global_history: VecDeque::new(),
            max_history_per_space: 50,
            storage_path,
        };

        // Load existing history
        manager.load_history()?;

        Ok(manager)
    }

    /// Add a search to history
    pub fn add_search(
        &mut self,
        query: String,
        space_id: Option<String>,
        result_count: usize,
    ) -> Result<(), String> {
        let entry = SearchEntry {
            query: query.clone(),
            timestamp: Utc::now().to_rfc3339(),
            space_id: space_id.clone(),
            result_count,
        };

        // Add to space-specific history
        if let Some(space_id) = space_id {
            let space_history = self.history.entry(space_id).or_insert_with(VecDeque::new);

            // Remove duplicate if exists
            space_history.retain(|e| e.query != query);

            // Add to front
            space_history.push_front(entry.clone());

            // Trim to max size
            while space_history.len() > self.max_history_per_space {
                space_history.pop_back();
            }
        }

        // Add to global history
        self.global_history.retain(|e| e.query != query);
        self.global_history.push_front(entry);

        while self.global_history.len() > self.max_history_per_space * 2 {
            self.global_history.pop_back();
        }

        // Save to disk
        self.save_history()?;

        Ok(())
    }

    /// Get search history for a space
    pub fn get_history(&self, space_id: Option<&str>, limit: usize) -> Vec<SearchEntry> {
        if let Some(space_id) = space_id {
            self.history
                .get(space_id)
                .map(|h| h.iter().take(limit).cloned().collect())
                .unwrap_or_default()
        } else {
            self.global_history.iter().take(limit).cloned().collect()
        }
    }

    /// Clear history for a space
    pub fn clear_history(&mut self, space_id: Option<&str>) -> Result<(), String> {
        if let Some(space_id) = space_id {
            self.history.remove(space_id);
        } else {
            self.global_history.clear();
        }

        self.save_history()?;
        Ok(())
    }

    /// Generate suggestions based on query
    pub fn get_suggestions(
        &self,
        query: &str,
        space_id: Option<&str>,
        limit: usize,
    ) -> Vec<SearchSuggestion> {
        let mut suggestions = Vec::new();
        let query_lower = query.to_lowercase();

        // Get history-based suggestions
        let history = if let Some(space_id) = space_id {
            self.history
                .get(space_id)
                .map(|h| h.iter().collect::<Vec<_>>())
                .unwrap_or_default()
        } else {
            self.global_history.iter().collect()
        };

        for entry in history.iter().take(20) {
            if entry.query.to_lowercase().contains(&query_lower) {
                let score = calculate_similarity(&entry.query, query);
                suggestions.push(SearchSuggestion {
                    text: entry.query.clone(),
                    suggestion_type: SuggestionType::History,
                    score,
                    metadata: Some({
                        let mut meta = HashMap::new();
                        meta.insert("timestamp".to_string(), entry.timestamp.clone());
                        meta.insert("result_count".to_string(), entry.result_count.to_string());
                        meta
                    }),
                });
            }
        }

        // Sort by score and take top N
        suggestions.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        suggestions.truncate(limit);

        suggestions
    }

    /// Load history from disk
    fn load_history(&mut self) -> Result<(), String> {
        if !self.storage_path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&self.storage_path)
            .map_err(|e| format!("Failed to read history file: {}", e))?;

        #[derive(Deserialize)]
        struct HistoryData {
            history: HashMap<String, Vec<SearchEntry>>,
            global_history: Vec<SearchEntry>,
        }

        let data: HistoryData = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse history: {}", e))?;

        self.history = data
            .history
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();

        self.global_history = data.global_history.into_iter().collect();

        Ok(())
    }

    /// Save history to disk
    fn save_history(&self) -> Result<(), String> {
        #[derive(Serialize)]
        struct HistoryData {
            history: HashMap<String, Vec<SearchEntry>>,
            global_history: Vec<SearchEntry>,
        }

        let data = HistoryData {
            history: self
                .history
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            global_history: self.global_history.iter().cloned().collect(),
        };

        let content = serde_json::to_string_pretty(&data)
            .map_err(|e| format!("Failed to serialize history: {}", e))?;

        fs::write(&self.storage_path, content)
            .map_err(|e| format!("Failed to write history file: {}", e))?;

        Ok(())
    }
}

/// Calculate similarity score between two strings
fn calculate_similarity(s1: &str, s2: &str) -> f32 {
    let s1_lower = s1.to_lowercase();
    let s2_lower = s2.to_lowercase();

    if s1_lower == s2_lower {
        return 1.0;
    }

    if s1_lower.starts_with(&s2_lower) {
        return 0.9;
    }

    if s1_lower.contains(&s2_lower) {
        return 0.7 + (s2_lower.len() as f32 / s1_lower.len() as f32) * 0.2;
    }

    // Calculate Levenshtein-like similarity
    let common_chars = s1_lower.chars().filter(|c| s2_lower.contains(*c)).count();

    let max_len = s1_lower.len().max(s2_lower.len());
    if max_len > 0 {
        common_chars as f32 / max_len as f32
    } else {
        0.0
    }
}
