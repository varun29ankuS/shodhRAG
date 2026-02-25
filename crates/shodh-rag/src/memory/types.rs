//! Type definitions for the memory system

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Unique identifier for memories
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(pub Uuid);

/// Unique identifier for contexts
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContextId(pub Uuid);

/// Experience types that can be recorded
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExperienceType {
    Conversation,
    Decision,
    Error,
    Learning,
    Discovery,
    Pattern,
    Context,
    Task,
    CodeEdit,
    FileAccess,
    Search,
    Command,
}

/// Rich multi-dimensional context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichContext {
    pub id: ContextId,

    /// Conversation context - what's being discussed
    pub conversation: ConversationContext,

    /// User context - who the user is, their patterns
    pub user: UserContext,

    /// Project context - what they're working on
    pub project: ProjectContext,

    /// Temporal context - when and patterns over time
    pub temporal: TemporalContext,

    /// Semantic context - relationships and meaning
    pub semantic: SemanticContext,

    /// Code context - related code elements
    pub code: CodeContext,

    /// Document context - related documents
    pub document: DocumentContext,

    /// Environment context - system state, location, etc
    pub environment: EnvironmentContext,

    /// Parent context (for hierarchical context)
    pub parent: Option<Box<RichContext>>,

    /// Context embeddings for similarity search
    pub embeddings: Option<Vec<f32>>,

    /// Context decay factor (how relevant this context is over time)
    pub decay_rate: f32,

    /// Created timestamp
    pub created_at: DateTime<Utc>,

    /// Last updated
    pub updated_at: DateTime<Utc>,
}

/// Conversation-specific context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversationContext {
    /// Current conversation ID
    pub conversation_id: Option<String>,

    /// Topic being discussed
    pub topic: Option<String>,

    /// Recent messages (last N turns)
    pub recent_messages: Vec<String>,

    /// Key entities mentioned
    pub mentioned_entities: Vec<String>,

    /// Active questions/intents
    pub active_intents: Vec<String>,

    /// Conversation mood/tone
    pub tone: Option<String>,
}

/// User-specific context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserContext {
    /// User ID
    pub user_id: Option<String>,

    /// User name
    pub name: Option<String>,

    /// User preferences
    pub preferences: HashMap<String, String>,

    /// User's typical working hours
    pub work_patterns: Vec<TimePattern>,

    /// User's expertise areas
    pub expertise: Vec<String>,

    /// User's goals/objectives
    pub goals: Vec<String>,

    /// User's learning style
    pub learning_style: Option<String>,
}

/// Project-specific context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectContext {
    /// Project ID
    pub project_id: Option<String>,

    /// Project name
    pub name: Option<String>,

    /// Project type (web, mobile, ML, etc)
    pub project_type: Option<String>,

    /// Tech stack
    pub technologies: Vec<String>,

    /// Current sprint/milestone
    pub current_phase: Option<String>,

    /// Related files being worked on
    pub active_files: Vec<String>,

    /// Current task/feature
    pub current_task: Option<String>,

    /// Project dependencies
    pub dependencies: Vec<String>,
}

/// Temporal context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TemporalContext {
    /// Time of day
    pub time_of_day: Option<String>,

    /// Day of week
    pub day_of_week: Option<String>,

    /// Session duration
    pub session_duration_minutes: Option<u32>,

    /// Time since last interaction
    pub time_since_last_interaction: Option<i64>,

    /// Recurring patterns detected
    pub patterns: Vec<TimePattern>,

    /// Historical trends
    pub trends: Vec<String>,
}

/// Semantic context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SemanticContext {
    /// Main concepts/topics
    pub concepts: Vec<String>,

    /// Related concepts
    pub related_concepts: Vec<String>,

    /// Concept relationships
    pub relationships: Vec<ConceptRelationship>,

    /// Domain/field
    pub domain: Option<String>,

    /// Abstraction level (high-level vs detailed)
    pub abstraction_level: Option<String>,

    /// Semantic tags
    pub tags: Vec<String>,
}

/// Code-specific context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeContext {
    /// Current file being edited
    pub current_file: Option<String>,

    /// Current function/class
    pub current_scope: Option<String>,

    /// Related files (imports, dependencies)
    pub related_files: Vec<String>,

    /// Recently edited functions
    pub recent_edits: Vec<String>,

    /// Call stack context
    pub call_stack: Vec<String>,

    /// Git branch
    pub git_branch: Option<String>,

    /// Recent commits
    pub recent_commits: Vec<String>,

    /// Code patterns detected
    pub patterns: Vec<String>,
}

/// Document-specific context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentContext {
    /// Current document ID
    pub document_id: Option<String>,

    /// Document type
    pub document_type: Option<String>,

    /// Section/chapter being read
    pub current_section: Option<String>,

    /// Related documents
    pub related_documents: Vec<String>,

    /// Citations/references
    pub citations: Vec<String>,

    /// Document tags/categories
    pub categories: Vec<String>,
}

/// Environment context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnvironmentContext {
    /// Operating system
    pub os: Option<String>,

    /// Device type
    pub device: Option<String>,

    /// Screen resolution/size
    pub screen_size: Option<String>,

    /// Location (if available)
    pub location: Option<String>,

    /// Network status
    pub network: Option<String>,

    /// System resource usage
    pub resources: HashMap<String, String>,
}

/// Time pattern for recurring behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimePattern {
    pub pattern_type: String,
    pub frequency: String,
    pub time_range: Option<(String, String)>,
    pub confidence: f32,
}

/// Relationship between concepts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptRelationship {
    pub from: String,
    pub to: String,
    pub relationship_type: RelationshipType,
    pub strength: f32,
}

/// Types of relationships
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RelationshipType {
    IsA,       // Inheritance
    HasA,      // Composition
    Uses,      // Dependency
    RelatedTo, // General association
    Causes,    // Causation
    PartOf,    // Part-whole
    Similar,   // Similarity
    Opposite,  // Antonym/opposite
}

/// Raw experience data to be stored (ENHANCED)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    pub experience_type: ExperienceType,
    pub content: String,

    /// RICH CONTEXT instead of simple string
    pub context: Option<RichContext>,

    /// Extracted entities
    pub entities: Vec<String>,

    /// Additional metadata
    pub metadata: HashMap<String, String>,

    /// Content embeddings
    pub embeddings: Option<Vec<f32>>,

    /// Related memories
    pub related_memories: Vec<MemoryId>,

    /// Causality chain (what led to this)
    pub causal_chain: Vec<MemoryId>,

    /// Outcome/result (what happened after)
    pub outcomes: Vec<String>,
}

/// Stored memory with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub experience: Experience,
    pub importance: f32,
    pub access_count: u32,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub compressed: bool,
}

/// Query for retrieving memories
#[derive(Debug, Clone)]
pub struct Query {
    pub query_text: Option<String>,
    pub query_embedding: Option<Vec<f32>>,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub experience_types: Option<Vec<ExperienceType>>,
    pub importance_threshold: Option<f32>,
    pub max_results: usize,
    pub retrieval_mode: RetrievalMode,
}

/// Retrieval modes
#[derive(Debug, Clone)]
pub enum RetrievalMode {
    Similarity,  // Vector similarity search
    Temporal,    // Time-based retrieval
    Causal,      // Cause-effect chains
    Associative, // Related memories
    Hybrid,      // Combination of methods
}

/// Criteria for forgetting memories
#[derive(Debug, Clone)]
pub enum ForgetCriteria {
    OlderThan(u32),     // Days
    LowImportance(f32), // Threshold
    Pattern(String),    // Regex pattern
}

/// Working memory - fast access, limited size
pub struct WorkingMemory {
    memories: HashMap<MemoryId, Memory>,
    capacity: usize,
    access_order: Vec<MemoryId>,
}

impl WorkingMemory {
    pub fn new(capacity: usize) -> Self {
        Self {
            memories: HashMap::new(),
            capacity,
            access_order: Vec::new(),
        }
    }

    pub fn add(&mut self, memory: Memory) -> anyhow::Result<()> {
        // Evict LRU if at capacity
        if self.memories.len() >= self.capacity {
            if let Some(oldest) = self.access_order.first().cloned() {
                self.memories.remove(&oldest);
                self.access_order.remove(0);
            }
        }

        let id = memory.id.clone();
        self.memories.insert(id.clone(), memory);
        self.access_order.push(id);
        Ok(())
    }

    pub fn search(&self, query: &Query, limit: usize) -> anyhow::Result<Vec<Memory>> {
        let mut results: Vec<Memory> = self
            .memories
            .values()
            .filter(|m| {
                // Apply filters
                if let Some(threshold) = query.importance_threshold {
                    if m.importance < threshold {
                        return false;
                    }
                }
                if let Some(types) = &query.experience_types {
                    if !types.iter().any(|t| {
                        std::mem::discriminant(&m.experience.experience_type)
                            == std::mem::discriminant(t)
                    }) {
                        return false;
                    }
                }
                if let Some((start, end)) = &query.time_range {
                    if m.created_at < *start || m.created_at > *end {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        // Sort by importance and recency
        results.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.last_accessed.cmp(&a.last_accessed))
        });

        results.truncate(limit);
        Ok(results)
    }

    pub fn size(&self) -> usize {
        self.memories.len()
    }

    pub fn contains(&self, id: &MemoryId) -> bool {
        self.memories.contains_key(id)
    }

    pub fn update_access(&mut self, id: &MemoryId) -> anyhow::Result<()> {
        if let Some(memory) = self.memories.get_mut(id) {
            memory.last_accessed = Utc::now();
            memory.access_count += 1;

            // Update access order
            if let Some(pos) = self.access_order.iter().position(|x| x == id) {
                self.access_order.remove(pos);
                self.access_order.push(id.clone());
            }
        }
        Ok(())
    }

    pub fn get_lru(&self, count: usize) -> anyhow::Result<Vec<Memory>> {
        let mut result = Vec::new();
        for id in self.access_order.iter().take(count) {
            if let Some(memory) = self.memories.get(id) {
                result.push(memory.clone());
            }
        }
        Ok(result)
    }

    pub fn remove(&mut self, id: &MemoryId) -> anyhow::Result<()> {
        self.memories.remove(id);
        self.access_order.retain(|x| x != id);
        Ok(())
    }

    pub fn remove_older_than(&mut self, cutoff: DateTime<Utc>) -> anyhow::Result<()> {
        let to_remove: Vec<MemoryId> = self
            .memories
            .iter()
            .filter(|(_, m)| m.created_at < cutoff)
            .map(|(id, _)| id.clone())
            .collect();

        for id in to_remove {
            self.remove(&id)?;
        }
        Ok(())
    }

    pub fn remove_below_importance(&mut self, threshold: f32) -> anyhow::Result<()> {
        let to_remove: Vec<MemoryId> = self
            .memories
            .iter()
            .filter(|(_, m)| m.importance < threshold)
            .map(|(id, _)| id.clone())
            .collect();

        for id in to_remove {
            self.remove(&id)?;
        }
        Ok(())
    }

    pub fn remove_matching(&mut self, regex: &regex::Regex) -> anyhow::Result<usize> {
        let to_remove: Vec<MemoryId> = self
            .memories
            .iter()
            .filter(|(_, m)| regex.is_match(&m.experience.content))
            .map(|(id, _)| id.clone())
            .collect();

        let count = to_remove.len();
        for id in to_remove {
            self.remove(&id)?;
        }
        Ok(count)
    }
}

/// Session memory - medium-term storage
pub struct SessionMemory {
    memories: HashMap<MemoryId, Memory>,
    max_size_mb: usize,
    current_size_bytes: usize,
}

impl SessionMemory {
    pub fn new(max_size_mb: usize) -> Self {
        Self {
            memories: HashMap::new(),
            max_size_mb,
            current_size_bytes: 0,
        }
    }

    pub fn add(&mut self, memory: Memory) -> anyhow::Result<()> {
        let memory_size = serde_json::to_string(&memory)
            .map(|s| s.len())
            .unwrap_or(1024);

        // Check if adding would exceed limit
        if self.current_size_bytes + memory_size > self.max_size_mb * 1024 * 1024 {
            // Evict lowest importance memories until there's space
            self.evict_to_make_space(memory_size)?;
        }

        self.memories.insert(memory.id.clone(), memory);
        self.current_size_bytes += memory_size;
        Ok(())
    }

    fn evict_to_make_space(&mut self, needed_bytes: usize) -> anyhow::Result<()> {
        let mut sorted: Vec<(MemoryId, f32)> = self
            .memories
            .iter()
            .map(|(id, m)| (id.clone(), m.importance))
            .collect();

        sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        for (id, _) in sorted {
            if self.current_size_bytes + needed_bytes <= self.max_size_mb * 1024 * 1024 {
                break;
            }
            if let Some(memory) = self.memories.remove(&id) {
                let size = serde_json::to_string(&memory)
                    .map(|s| s.len())
                    .unwrap_or(1024);
                self.current_size_bytes -= size;
            }
        }
        Ok(())
    }

    pub fn search(&self, query: &Query, limit: usize) -> anyhow::Result<Vec<Memory>> {
        let mut results: Vec<Memory> = self
            .memories
            .values()
            .filter(|m| {
                if let Some(threshold) = query.importance_threshold {
                    if m.importance < threshold {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        results.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);
        Ok(results)
    }

    pub fn size_mb(&self) -> usize {
        self.current_size_bytes / (1024 * 1024)
    }

    pub fn contains(&self, id: &MemoryId) -> bool {
        self.memories.contains_key(id)
    }

    pub fn update_access(&mut self, id: &MemoryId) -> anyhow::Result<()> {
        if let Some(memory) = self.memories.get_mut(id) {
            memory.last_accessed = Utc::now();
            memory.access_count += 1;
        }
        Ok(())
    }

    pub fn get_important(&self, threshold: f32) -> anyhow::Result<Vec<Memory>> {
        Ok(self
            .memories
            .values()
            .filter(|m| m.importance >= threshold)
            .cloned()
            .collect())
    }

    pub fn remove(&mut self, id: &MemoryId) -> anyhow::Result<()> {
        if let Some(memory) = self.memories.remove(id) {
            let size = serde_json::to_string(&memory)
                .map(|s| s.len())
                .unwrap_or(1024);
            self.current_size_bytes -= size;
        }
        Ok(())
    }

    pub fn remove_older_than(&mut self, cutoff: DateTime<Utc>) -> anyhow::Result<()> {
        let to_remove: Vec<MemoryId> = self
            .memories
            .iter()
            .filter(|(_, m)| m.created_at < cutoff)
            .map(|(id, _)| id.clone())
            .collect();

        for id in to_remove {
            self.remove(&id)?;
        }
        Ok(())
    }

    pub fn remove_below_importance(&mut self, threshold: f32) -> anyhow::Result<()> {
        let to_remove: Vec<MemoryId> = self
            .memories
            .iter()
            .filter(|(_, m)| m.importance < threshold)
            .map(|(id, _)| id.clone())
            .collect();

        for id in to_remove {
            self.remove(&id)?;
        }
        Ok(())
    }

    pub fn remove_matching(&mut self, regex: &regex::Regex) -> anyhow::Result<usize> {
        let to_remove: Vec<MemoryId> = self
            .memories
            .iter()
            .filter(|(_, m)| regex.is_match(&m.experience.content))
            .map(|(id, _)| id.clone())
            .collect();

        let count = to_remove.len();
        for id in to_remove {
            self.remove(&id)?;
        }
        Ok(count)
    }
}
