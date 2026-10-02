//! Conversation Continuity - Maintains context across sessions using the Memory System

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::memory::{Experience, ExperienceType, Memory, MemorySystem, Query, RetrievalMode};

/// Manages conversation history and context
pub struct ConversationManager {
    memory_system: Arc<RwLock<MemorySystem>>,
    current_conversation: Arc<RwLock<Option<Conversation>>>,
    conversation_stack: Arc<RwLock<Vec<ConversationSnapshot>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub topic: String,
    pub messages: Vec<Message>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub context: ConversationContext,
    pub key_points: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    pub project: Option<String>,
    pub files_discussed: Vec<String>,
    pub concepts_mentioned: Vec<String>,
    pub decisions_made: Vec<Decision>,
    pub tasks_created: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub description: String,
    pub rationale: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub conversation_id: Uuid,
    pub topic: String,
    pub timestamp: DateTime<Utc>,
    pub summary: String,
    pub key_points: Vec<String>,
}

impl ConversationManager {
    pub fn new_with_memory(memory_system: Arc<RwLock<MemorySystem>>) -> Result<Self> {
        Ok(Self {
            memory_system,
            current_conversation: Arc::new(RwLock::new(None)),
            conversation_stack: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub fn new() -> Result<Self> {
        // Create a new memory system if not provided
        let memory_config = crate::memory::MemoryConfig::default();
        let memory_system = Arc::new(RwLock::new(MemorySystem::new(memory_config)?));

        Ok(Self {
            memory_system,
            current_conversation: Arc::new(RwLock::new(None)),
            conversation_stack: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Start a new conversation
    pub async fn start_conversation(&self, topic: String) -> Result<Uuid> {
        let conversation = Conversation {
            id: Uuid::new_v4(),
            topic: topic.clone(),
            messages: Vec::new(),
            started_at: Utc::now(),
            ended_at: None,
            context: ConversationContext {
                project: None,
                files_discussed: Vec::new(),
                concepts_mentioned: Vec::new(),
                decisions_made: Vec::new(),
                tasks_created: Vec::new(),
            },
            key_points: Vec::new(),
        };

        let conversation_id = conversation.id;

        // Store in memory system as an experience
        let experience = Experience {
            experience_type: ExperienceType::Conversation,
            content: topic.clone(),
            context: None, // Will be built by context manager
            entities: Vec::new(),
            metadata: Default::default(),
            embeddings: None,
            related_memories: Vec::new(),
            causal_chain: Vec::new(),
            outcomes: Vec::new(),
        };

        let memory = self.memory_system.write().await;
        memory.record(experience)?;
        drop(memory);

        *self.current_conversation.write().await = Some(conversation);

        Ok(conversation_id)
    }

    /// Add a message to the current conversation
    pub async fn add_message(&self, role: MessageRole, content: String) -> Result<()> {
        let mut current = self.current_conversation.write().await;

        if let Some(ref mut conversation) = *current {
            let message = Message {
                role: role.clone(),
                content: content.clone(),
                timestamp: Utc::now(),
                metadata: serde_json::Value::Null,
            };

            conversation.messages.push(message);

            // Extract key information
            self.extract_context_from_message(&content, &mut conversation.context);

            // Store important messages in memory
            if matches!(role, MessageRole::Assistant) && self.is_important_message(&content) {
                let experience = Experience {
                    experience_type: ExperienceType::Learning,
                    content: content.clone(),
                    context: None, // Will be built by context manager
                    entities: Vec::new(),
                    metadata: Default::default(),
                    embeddings: None,
                    related_memories: Vec::new(),
                    causal_chain: Vec::new(),
                    outcomes: Vec::new(),
                };

                let memory = self.memory_system.write().await;
                memory.record(experience)?;
            }
        }

        Ok(())
    }

    /// End current conversation and create snapshot
    pub async fn end_conversation(&self) -> Result<()> {
        let mut current = self.current_conversation.write().await;

        if let Some(mut conversation) = current.take() {
            conversation.ended_at = Some(Utc::now());

            // Generate summary
            let summary = self.generate_summary(&conversation);

            // Extract key points
            let key_points = self.extract_key_points(&conversation);

            // Create snapshot
            let snapshot = ConversationSnapshot {
                conversation_id: conversation.id,
                topic: conversation.topic.clone(),
                timestamp: conversation.ended_at.unwrap_or_else(Utc::now),
                summary: summary.clone(),
                key_points: key_points.clone(),
            };

            // Save to conversation stack
            let mut stack = self.conversation_stack.write().await;
            stack.push(snapshot);

            // Store final conversation in memory as important experience
            let experience = Experience {
                experience_type: ExperienceType::Decision,
                content: summary,
                context: None, // Will be built by context manager
                entities: Vec::new(),
                metadata: Default::default(),
                embeddings: None,
                related_memories: Vec::new(),
                causal_chain: Vec::new(),
                outcomes: Vec::new(),
            };

            let memory = self.memory_system.write().await;
            memory.record(experience)?;
        }

        Ok(())
    }

    /// Continue from last conversation
    pub async fn continue_last_conversation(&self) -> Result<Option<ContinuationContext>> {
        let stack = self.conversation_stack.read().await;

        if let Some(last_snapshot) = stack.last() {
            // Retrieve full conversation from memory
            let query = Query {
                query_text: Some(format!("conversation_id:{}", last_snapshot.conversation_id)),
                query_embedding: None,
                retrieval_mode: RetrievalMode::Similarity,
                max_results: 10,
                importance_threshold: Some(0.0),
                time_range: None,
                experience_types: None,
            };

            let memory = self.memory_system.read().await;
            let memories = memory.retrieve(&query)?;

            if !memories.is_empty() {
                // Reconstruct conversation context
                let context = self.reconstruct_context(last_snapshot, &memories)?;
                return Ok(Some(context));
            }
        }

        Ok(None)
    }

    /// Get the last conversation
    pub async fn get_last_conversation(&self) -> Result<Option<Conversation>> {
        let current = self.current_conversation.read().await;

        if current.is_some() {
            return Ok(current.clone());
        }

        // If no current conversation, try to get from stack
        let stack = self.conversation_stack.read().await;

        if let Some(last_snapshot) = stack.last() {
            // Retrieve from memory system
            let conversation = self
                .retrieve_conversation(last_snapshot.conversation_id)
                .await?;
            return Ok(conversation);
        }

        Ok(None)
    }

    /// Search conversations by topic or content
    pub async fn search_conversations(&self, query: &str) -> Result<Vec<ConversationSnapshot>> {
        let memory_query = Query {
            query_text: Some(query.to_string()),
            query_embedding: None,
            retrieval_mode: RetrievalMode::Similarity,
            max_results: 20,
            importance_threshold: Some(0.5),
            time_range: None,
            experience_types: None,
        };

        let memory = self.memory_system.read().await;
        let memories = memory.retrieve(&memory_query)?;

        let mut snapshots = Vec::new();

        for memory_item in memories {
            // Extract conversation details from RichContext if available
            if let Some(rich_ctx) = &memory_item.experience.context {
                let conversation_id =
                    if let Some(conv_id_str) = &rich_ctx.conversation.conversation_id {
                        Uuid::parse_str(conv_id_str).unwrap_or(memory_item.id.0)
                    } else {
                        memory_item.id.0
                    };

                let topic = rich_ctx
                    .conversation
                    .topic
                    .clone()
                    .unwrap_or_else(|| memory_item.experience.content.clone());

                // Build summary from conversation context
                let summary = if !rich_ctx.conversation.recent_messages.is_empty() {
                    format!(
                        "Discussed: {}. Key entities: {}",
                        rich_ctx.conversation.recent_messages.join(", "),
                        rich_ctx.conversation.mentioned_entities.join(", ")
                    )
                } else {
                    memory_item.experience.content.clone()
                };

                // Extract key points from entities and intents
                let mut key_points = rich_ctx.conversation.mentioned_entities.clone();
                key_points.extend(rich_ctx.conversation.active_intents.clone());

                snapshots.push(ConversationSnapshot {
                    conversation_id,
                    topic,
                    timestamp: memory_item.created_at,
                    summary,
                    key_points,
                });
            } else {
                // Fallback: create snapshot from basic experience data
                snapshots.push(ConversationSnapshot {
                    conversation_id: memory_item.id.0,
                    topic: memory_item.experience.content.clone(),
                    timestamp: memory_item.created_at,
                    summary: memory_item.experience.content.clone(),
                    key_points: memory_item.experience.entities.clone(),
                });
            }
        }

        Ok(snapshots)
    }

    /// Get conversation suggestions based on context
    pub async fn get_continuation_suggestions(&self) -> Result<Vec<String>> {
        let mut suggestions = Vec::new();

        // Get recent conversations from memory
        let query = Query {
            query_text: Some(String::new()),
            query_embedding: None,
            retrieval_mode: RetrievalMode::Temporal,
            max_results: 5,
            importance_threshold: Some(0.0),
            time_range: Some((Utc::now() - chrono::Duration::days(7), Utc::now())),
            experience_types: None,
        };

        let memory = self.memory_system.read().await;
        let recent_memories = memory.retrieve(&query)?;

        // Extract unfinished topics
        for memory_item in recent_memories {
            if memory_item.experience.experience_type == ExperienceType::Conversation {
                // Check for unresolved questions or tasks
                if let Some(unresolved) =
                    self.find_unresolved_items(&memory_item.experience.content)
                {
                    suggestions.push(format!("Continue discussing: {}", unresolved));
                }
            }
        }

        // Add context-based suggestions
        if let Some(ref conversation) = *self.current_conversation.read().await {
            if let Some(last_task) = conversation.context.tasks_created.last() {
                suggestions.push(format!("Follow up on task: {}", last_task));
            }

            if !conversation.context.decisions_made.is_empty() {
                suggestions.push("Review recent decisions".to_string());
            }
        }

        Ok(suggestions)
    }

    // Helper methods

    fn extract_context_from_message(&self, content: &str, context: &mut ConversationContext) {
        // Extract file mentions
        if content.contains(".rs") || content.contains(".tsx") || content.contains(".ts") {
            // Simple regex-like extraction
            for word in content.split_whitespace() {
                if word.contains('.') && (word.ends_with(".rs") || word.ends_with(".tsx")) {
                    context.files_discussed.push(word.to_string());
                }
            }
        }

        // Extract concepts (simplified)
        let concepts = [
            "memory",
            "assistant",
            "pattern",
            "storage",
            "index",
            "search",
        ];
        for concept in concepts {
            if content.to_lowercase().contains(concept) {
                if !context.concepts_mentioned.contains(&concept.to_string()) {
                    context.concepts_mentioned.push(concept.to_string());
                }
            }
        }

        // Extract decisions (look for decision keywords)
        if content.contains("let's")
            || content.contains("we should")
            || content.contains("decision:")
        {
            // This would need more sophisticated NLP
        }
    }

    fn is_important_message(&self, content: &str) -> bool {
        // Heuristics for importance
        content.len() > 100
            || content.contains("important")
            || content.contains("decision")
            || content.contains("conclusion")
            || content.contains("summary")
    }

    fn generate_summary(&self, conversation: &Conversation) -> String {
        // Simple summary generation
        format!(
            "Discussion about {} with {} messages. Key topics: {}",
            conversation.topic,
            conversation.messages.len(),
            conversation.context.concepts_mentioned.join(", ")
        )
    }

    fn extract_key_points(&self, conversation: &Conversation) -> Vec<String> {
        let mut points = Vec::new();

        // Extract from decisions
        for decision in &conversation.context.decisions_made {
            points.push(format!("Decision: {}", decision.description));
        }

        // Extract from tasks
        for task in &conversation.context.tasks_created {
            points.push(format!("Task: {}", task));
        }

        // Extract important messages (simplified)
        for message in &conversation.messages {
            if message.content.contains("conclusion:") || message.content.contains("summary:") {
                points.push(message.content.clone());
            }
        }

        points
    }

    async fn retrieve_conversation(&self, id: Uuid) -> Result<Option<Conversation>> {
        let query = Query {
            query_text: Some(format!("conversation_id:{}", id)),
            query_embedding: None,
            retrieval_mode: RetrievalMode::Similarity,
            max_results: 1,
            importance_threshold: Some(0.0),
            time_range: None,
            experience_types: None,
        };

        let memory = self.memory_system.read().await;
        let memories = memory.retrieve(&query)?;

        if let Some(memory_item) = memories.first() {
            if let Some(rich_ctx) = &memory_item.experience.context {
                // Reconstruct conversation from RichContext
                if let Some(conv_id_str) = &rich_ctx.conversation.conversation_id {
                    if let Ok(conv_id) = Uuid::parse_str(conv_id_str) {
                        let conversation = Conversation {
                            id: conv_id,
                            topic: rich_ctx.conversation.topic.clone().unwrap_or_default(),
                            messages: Vec::new(), // Messages not stored in context
                            started_at: memory_item.created_at,
                            ended_at: None,
                            context: ConversationContext {
                                project: rich_ctx.project.name.clone(),
                                files_discussed: rich_ctx.code.related_files.clone(),
                                concepts_mentioned: rich_ctx.semantic.concepts.clone(),
                                decisions_made: Vec::new(),
                                tasks_created: Vec::new(),
                            },
                            key_points: rich_ctx.conversation.mentioned_entities.clone(),
                        };
                        return Ok(Some(conversation));
                    }
                }
            }
        }

        Ok(None)
    }

    fn reconstruct_context(
        &self,
        snapshot: &ConversationSnapshot,
        memories: &[crate::memory::Memory],
    ) -> Result<ContinuationContext> {
        Ok(ContinuationContext {
            last_topic: snapshot.topic.clone(),
            last_summary: snapshot.summary.clone(),
            key_points: snapshot.key_points.clone(),
            unresolved_items: self.find_unresolved_from_memories(memories),
            suggested_continuations: vec![
                format!("Continue from: {}", snapshot.topic),
                "Review previous decisions".to_string(),
                "Check task progress".to_string(),
            ],
        })
    }

    fn find_unresolved_items(&self, content: &str) -> Option<String> {
        // Look for question marks or TODO items
        if content.contains('?') {
            Some("unanswered question".to_string())
        } else if content.to_lowercase().contains("todo") {
            Some("incomplete task".to_string())
        } else {
            None
        }
    }

    fn find_unresolved_from_memories(&self, memories: &[crate::memory::Memory]) -> Vec<String> {
        let mut unresolved = Vec::new();

        for memory in memories {
            if let Some(item) = self.find_unresolved_items(&memory.experience.content) {
                unresolved.push(item);
            }
        }

        unresolved
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationContext {
    pub last_topic: String,
    pub last_summary: String,
    pub key_points: Vec<String>,
    pub unresolved_items: Vec<String>,
    pub suggested_continuations: Vec<String>,
}
