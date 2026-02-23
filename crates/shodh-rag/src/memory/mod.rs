//! Simplified Memory System for conversation context
//!
//! Provides in-memory conversation history with JSON file persistence.
//! Replaces the heavyweight RocksDB/Vamana-based system.

pub mod types;

use anyhow::Result;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::path::PathBuf;
use uuid::Uuid;
use chrono::Utc;

pub use types::*;

/// Configuration for the memory system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub storage_path: PathBuf,
    pub working_memory_size: usize,
    pub session_memory_size_mb: usize,
    pub auto_compress: bool,
    pub compression_age_days: u32,
    pub importance_threshold: f32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            storage_path: PathBuf::from("./memory_store"),
            working_memory_size: 100,
            session_memory_size_mb: 100,
            auto_compress: false,
            compression_age_days: 7,
            importance_threshold: 0.7,
        }
    }
}

/// Memory statistics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryStats {
    pub working_memory_count: usize,
    pub session_memory_count: usize,
    pub long_term_count: usize,
    pub total_size_bytes: u64,
}

/// Main memory system — simplified file-based implementation
pub struct MemorySystem {
    config: MemoryConfig,
    memories: Arc<RwLock<Vec<Memory>>>,
    stats: Arc<RwLock<MemoryStats>>,
}

impl MemorySystem {
    pub fn new(config: MemoryConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.storage_path)?;

        let system = Self {
            config: config.clone(),
            memories: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(MemoryStats::default())),
        };

        system.load_from_disk()?;
        Ok(system)
    }

    /// Record an experience
    pub fn record(&self, experience: Experience) -> Result<MemoryId> {
        let id = MemoryId(Uuid::new_v4());
        let importance = Self::calculate_importance(&experience);
        let memory = Memory {
            id: id.clone(),
            experience,
            importance,
            access_count: 0,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            compressed: false,
        };

        let mut memories = self.memories.write().map_err(|e| anyhow::anyhow!("Lock: {}", e))?;
        memories.push(memory);

        let max = self.config.working_memory_size;
        if memories.len() > max * 2 {
            memories.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
            memories.truncate(max);
        }

        drop(memories);
        if let Err(e) = self.persist_to_disk() {
            tracing::warn!("Memory persist failed: {}", e);
        }
        Ok(id)
    }

    /// Retrieve memories matching a query, respecting the requested retrieval mode.
    pub fn retrieve(&self, query: &Query) -> Result<Vec<Memory>> {
        let memories = self.memories.read().map_err(|e| anyhow::anyhow!("Lock: {}", e))?;

        // Phase 1: filter by hard constraints (type, time, importance)
        let mut candidates: Vec<Memory> = memories.iter()
            .filter(|m| {
                if let Some(threshold) = query.importance_threshold {
                    if m.importance < threshold { return false; }
                }
                if let Some(types) = &query.experience_types {
                    if !types.iter().any(|t| {
                        std::mem::discriminant(&m.experience.experience_type) == std::mem::discriminant(t)
                    }) { return false; }
                }
                if let Some((start, end)) = &query.time_range {
                    if m.created_at < *start || m.created_at > *end { return false; }
                }
                true
            })
            .cloned()
            .collect();

        // Phase 2: score and sort by retrieval mode
        match query.retrieval_mode {
            RetrievalMode::Temporal => {
                // Most recent first
                candidates.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            }
            RetrievalMode::Similarity | RetrievalMode::Hybrid => {
                // Text relevance scoring + recency boost
                if let Some(ref text) = query.query_text {
                    let query_lower = text.to_lowercase();
                    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

                    candidates = candidates.into_iter()
                        .map(|mut m| {
                            let content_lower = m.experience.content.to_lowercase();
                            let entity_lower: Vec<String> = m.experience.entities.iter()
                                .map(|e| e.to_lowercase())
                                .collect();

                            // Word overlap score
                            let word_hits = query_words.iter()
                                .filter(|w| content_lower.contains(*w) || entity_lower.iter().any(|e| e.contains(*w)))
                                .count();
                            let text_score = if query_words.is_empty() { 0.0 }
                                else { word_hits as f32 / query_words.len() as f32 };

                            // Recency decay (halve score per 7 days)
                            let age_days = (Utc::now() - m.created_at).num_days().max(0) as f32;
                            let recency = 0.5f32.powf(age_days / 7.0);

                            // Combine: 60% text relevance, 25% recency, 15% importance
                            let combined = 0.60 * text_score + 0.25 * recency + 0.15 * m.importance;
                            m.importance = combined; // reuse field for ranking
                            m
                        })
                        .filter(|m| m.importance > 0.05) // drop irrelevant
                        .collect();

                    candidates.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
                } else {
                    // No query text — fall back to importance sort
                    candidates.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
                }
            }
            RetrievalMode::Causal | RetrievalMode::Associative => {
                // Entity overlap — find memories that share entities with the query
                if let Some(ref text) = query.query_text {
                    let query_lower = text.to_lowercase();
                    candidates.sort_by(|a, b| {
                        let a_hits = a.experience.entities.iter()
                            .filter(|e| query_lower.contains(&e.to_lowercase()))
                            .count();
                        let b_hits = b.experience.entities.iter()
                            .filter(|e| query_lower.contains(&e.to_lowercase()))
                            .count();
                        b_hits.cmp(&a_hits)
                            .then_with(|| b.created_at.cmp(&a.created_at))
                    });
                } else {
                    candidates.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                }
            }
        }

        candidates.truncate(query.max_results);
        Ok(candidates)
    }

    /// Calculate importance based on experience content and type.
    fn calculate_importance(experience: &Experience) -> f32 {
        let mut score: f32 = 0.3; // base

        // Type-based boost
        match experience.experience_type {
            ExperienceType::Task => score += 0.3,
            ExperienceType::Learning => score += 0.25,
            ExperienceType::Search => score += 0.1,
            ExperienceType::FileAccess => score += 0.05,
            _ => score += 0.15,
        }

        // Content length signal (longer = more substance)
        let len = experience.content.len();
        if len > 200 { score += 0.15; }
        else if len > 50 { score += 0.05; }

        // Entity richness
        if experience.entities.len() > 3 { score += 0.1; }
        else if !experience.entities.is_empty() { score += 0.05; }

        // Metadata richness
        if experience.metadata.len() > 2 { score += 0.05; }

        score.min(1.0)
    }

    pub fn stats(&self) -> Result<MemoryStats> {
        let memories = self.memories.read().map_err(|e| anyhow::anyhow!("Lock: {}", e))?;
        Ok(MemoryStats {
            working_memory_count: memories.len(),
            ..Default::default()
        })
    }

    pub fn forget(&self, criteria: ForgetCriteria) -> Result<usize> {
        let mut memories = self.memories.write().map_err(|e| anyhow::anyhow!("Lock: {}", e))?;
        let before = memories.len();

        match criteria {
            ForgetCriteria::OlderThan(days) => {
                let cutoff = Utc::now() - chrono::Duration::days(days as i64);
                memories.retain(|m| m.created_at >= cutoff);
            }
            ForgetCriteria::LowImportance(threshold) => {
                memories.retain(|m| m.importance >= threshold);
            }
            ForgetCriteria::Pattern(pattern) => {
                if let Ok(re) = regex::Regex::new(&pattern) {
                    memories.retain(|m| !re.is_match(&m.experience.content));
                }
            }
        }

        let removed = before - memories.len();
        drop(memories);
        if let Err(e) = self.persist_to_disk() {
            tracing::warn!("Memory persist after forget failed: {}", e);
        }
        Ok(removed)
    }

    pub fn count(&self) -> usize {
        self.memories.read().map(|m| m.len()).unwrap_or(0)
    }

    fn persist_to_disk(&self) -> Result<()> {
        let memories = self.memories.read().map_err(|e| anyhow::anyhow!("Lock: {}", e))?;
        let json = serde_json::to_string(&*memories)?;
        std::fs::write(self.config.storage_path.join("memories.json"), json)?;
        Ok(())
    }

    fn load_from_disk(&self) -> Result<()> {
        let path = self.config.storage_path.join("memories.json");
        if path.exists() {
            let json = match std::fs::read_to_string(&path) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!("Failed to read memories.json, starting fresh: {}", e);
                    return Ok(());
                }
            };
            let data: Vec<Memory> = match serde_json::from_str(&json) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("Corrupt memories.json, starting fresh: {}", e);
                    return Ok(());
                }
            };
            let mut memories = self.memories.write()
                .map_err(|e| anyhow::anyhow!("Memory lock poisoned: {}", e))?;
            *memories = data;
        }
        Ok(())
    }
}
