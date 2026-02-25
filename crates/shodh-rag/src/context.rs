//! Context accumulation and tracking.
//!
//! Provides helpers for tracking user interactions (searches, document views,
//! task starts) in the memory system, building LLM context from conversation
//! history + memory, and managing sessions.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock as AsyncRwLock;

use crate::agent::conversation_continuity::ConversationManager;
use crate::memory::{self, Experience, ExperienceType, MemorySystem, Query, RetrievalMode};

// ── Tracking helpers ───────────────────────────────────────────────────────

/// Record a search query as a Search experience in memory.
pub async fn track_search(
    query: &str,
    results: &[String],
    memory: &Arc<AsyncRwLock<MemorySystem>>,
) -> Result<(), String> {
    let mut mem = memory.write().await;

    let experience = Experience {
        experience_type: ExperienceType::Search,
        content: format!("Search query: {}", query),
        context: None,
        entities: results.iter().flat_map(|r| extract_entities(r)).collect(),
        metadata: {
            let mut map = HashMap::new();
            map.insert("query".to_string(), query.to_string());
            map.insert("result_count".to_string(), results.len().to_string());
            map
        },
        embeddings: None,
        related_memories: Vec::new(),
        causal_chain: Vec::new(),
        outcomes: results.iter().take(3).cloned().collect(),
    };

    mem.record(experience)
        .map(|_| ())
        .map_err(|e| format!("Failed to record search: {}", e))
}

/// Record a document view in memory.
pub async fn track_document_view(
    doc_id: &str,
    memory: &Arc<AsyncRwLock<MemorySystem>>,
) -> Result<(), String> {
    let mut mem = memory.write().await;

    let experience = Experience {
        experience_type: ExperienceType::FileAccess,
        content: format!("Viewed document: {}", doc_id),
        context: None,
        entities: vec![doc_id.to_string()],
        metadata: {
            let mut map = HashMap::new();
            map.insert("doc_id".to_string(), doc_id.to_string());
            map.insert("action".to_string(), "view".to_string());
            map
        },
        embeddings: None,
        related_memories: Vec::new(),
        causal_chain: Vec::new(),
        outcomes: Vec::new(),
    };

    mem.record(experience)
        .map(|_| ())
        .map_err(|e| format!("Failed to record document view: {}", e))
}

/// Record a search refinement (old query → new query) as a Learning experience.
pub async fn track_search_refinement(
    old_query: &str,
    new_query: &str,
    memory: &Arc<AsyncRwLock<MemorySystem>>,
) -> Result<(), String> {
    let mut mem = memory.write().await;

    let experience = Experience {
        experience_type: ExperienceType::Learning,
        content: format!("Refined search from '{}' to '{}'", old_query, new_query),
        context: None,
        entities: vec![old_query.to_string(), new_query.to_string()],
        metadata: {
            let mut map = HashMap::new();
            map.insert("old_query".to_string(), old_query.to_string());
            map.insert("new_query".to_string(), new_query.to_string());
            map.insert("action".to_string(), "refinement".to_string());
            map
        },
        embeddings: None,
        related_memories: Vec::new(),
        causal_chain: Vec::new(),
        outcomes: Vec::new(),
    };

    mem.record(experience)
        .map(|_| ())
        .map_err(|e| format!("Failed to record refinement: {}", e))
}

/// Record a filter application in memory.
pub async fn track_filter(
    filter_type: &str,
    filter_value: &str,
    memory: &Arc<AsyncRwLock<MemorySystem>>,
) -> Result<(), String> {
    let mut mem = memory.write().await;

    let experience = Experience {
        experience_type: ExperienceType::Search,
        content: format!("Applied filter: {} = {}", filter_type, filter_value),
        context: None,
        entities: vec![filter_type.to_string(), filter_value.to_string()],
        metadata: {
            let mut map = HashMap::new();
            map.insert("filter_type".to_string(), filter_type.to_string());
            map.insert("filter_value".to_string(), filter_value.to_string());
            map
        },
        embeddings: None,
        related_memories: Vec::new(),
        causal_chain: Vec::new(),
        outcomes: Vec::new(),
    };

    mem.record(experience)
        .map(|_| ())
        .map_err(|e| format!("Failed to record filter: {}", e))
}

/// Record a task start in memory.
pub async fn track_task_start(
    name: &str,
    category: &str,
    memory: &Arc<AsyncRwLock<MemorySystem>>,
) -> Result<(), String> {
    let mut mem = memory.write().await;

    let experience = Experience {
        experience_type: ExperienceType::Task,
        content: format!("Task: {}", name),
        context: None,
        entities: vec![name.to_string(), category.to_string()],
        metadata: {
            let mut map = HashMap::new();
            map.insert("task_name".to_string(), name.to_string());
            map.insert("category".to_string(), category.to_string());
            map.insert("status".to_string(), "started".to_string());
            map
        },
        embeddings: None,
        related_memories: Vec::new(),
        causal_chain: Vec::new(),
        outcomes: Vec::new(),
    };

    mem.record(experience)
        .map(|_| ())
        .map_err(|e| format!("Failed to record task: {}", e))
}

// ── Context building ───────────────────────────────────────────────────────

/// Build a context summary string from conversation + memory.
pub async fn build_context_summary(
    session_id: &str,
    conversation_manager: Option<&ConversationManager>,
    memory: Option<&Arc<AsyncRwLock<MemorySystem>>>,
) -> Result<String, String> {
    let mut summary = String::new();

    if let Some(conv_mgr) = conversation_manager {
        if let Some(conversation) = conv_mgr
            .get_last_conversation()
            .await
            .map_err(|e| format!("Failed to get conversation: {}", e))?
        {
            summary.push_str(&format!("Conversation: {}\n", conversation.topic));
            summary.push_str(&format!("Messages: {}\n", conversation.messages.len()));
            summary.push_str(&format!(
                "Key concepts: {}\n",
                conversation.context.concepts_mentioned.join(", ")
            ));
        }
    }

    if let Some(mem_arc) = memory {
        let mem = mem_arc.read().await;
        if let Ok(stats) = mem.stats() {
            summary.push_str(&format!(
                "\nMemory: {} working memories\n",
                stats.working_memory_count
            ));
        }
    }

    Ok(summary)
}

/// Build rich LLM context from conversation history + recent memories.
pub async fn build_llm_context(
    conversation_manager: Option<&ConversationManager>,
    memory: Option<&Arc<AsyncRwLock<MemorySystem>>>,
) -> Result<String, String> {
    let mut context_parts = Vec::new();

    // Recent conversation messages
    if let Some(conv_mgr) = conversation_manager {
        if let Some(conversation) = conv_mgr
            .get_last_conversation()
            .await
            .map_err(|e| format!("Failed to get conversation: {}", e))?
        {
            let recent: Vec<String> = conversation
                .messages
                .iter()
                .rev()
                .take(5)
                .rev()
                .map(|m| format!("{:?}: {}", m.role, m.content))
                .collect();

            if !recent.is_empty() {
                context_parts.push(format!("Recent conversation:\n{}", recent.join("\n")));
            }
        }
    }

    // Recent memories
    if let Some(mem_arc) = memory {
        let mem = mem_arc.read().await;

        let query = Query {
            query_text: None,
            query_embedding: None,
            retrieval_mode: RetrievalMode::Temporal,
            max_results: 5,
            importance_threshold: Some(0.6),
            time_range: Some((
                chrono::Utc::now() - chrono::Duration::hours(24),
                chrono::Utc::now(),
            )),
            experience_types: None,
        };

        if let Ok(memories) = mem.retrieve(&query) {
            let memory_context: Vec<String> = memories
                .iter()
                .map(|m| format!("Memory: {}", m.experience.content))
                .collect();

            if !memory_context.is_empty() {
                context_parts.push(format!("Relevant memories:\n{}", memory_context.join("\n")));
            }
        }
    }

    Ok(context_parts.join("\n\n"))
}

/// Get full context dump as JSON (for debugging/inspection).
pub async fn get_full_context(
    conversation_manager: Option<&ConversationManager>,
    memory: Option<&Arc<AsyncRwLock<MemorySystem>>>,
) -> Result<serde_json::Value, String> {
    let mut context = serde_json::json!({});

    if let Some(mem_arc) = memory {
        let mem = mem_arc.read().await;
        if let Ok(stats) = mem.stats() {
            context["memory"] = serde_json::json!({
                "total_memories": stats.working_memory_count,
                "working_count": stats.working_memory_count,
                "session_count": stats.session_memory_count,
                "long_term_count": stats.long_term_count
            });
        }
    }

    if let Some(conv_mgr) = conversation_manager {
        if let Some(conversation) = conv_mgr.get_last_conversation().await.unwrap_or(None) {
            context["conversation"] = serde_json::json!({
                "topic": conversation.topic,
                "message_count": conversation.messages.len(),
                "concepts": conversation.context.concepts_mentioned,
                "files": conversation.context.files_discussed
            });
        }
    }

    Ok(context)
}

/// Search memory semantically.
pub async fn search_memory(
    query: &str,
    max_results: usize,
    memory: &Arc<AsyncRwLock<MemorySystem>>,
) -> Result<Vec<String>, String> {
    let mem = memory.read().await;

    let memory_query = Query {
        query_text: Some(query.to_string()),
        query_embedding: None,
        retrieval_mode: RetrievalMode::Hybrid,
        max_results,
        time_range: None,
        importance_threshold: None,
        experience_types: None,
    };

    let results = mem
        .retrieve(&memory_query)
        .map_err(|e| format!("Memory search failed: {}", e))?;

    Ok(results
        .iter()
        .map(|m| {
            format!(
                "[{:?}] {} (importance: {:.2})",
                m.experience.experience_type,
                m.experience.content.chars().take(100).collect::<String>(),
                m.importance
            )
        })
        .collect())
}

// ── Utilities ──────────────────────────────────────────────────────────────

/// Extract entities (file names, quoted strings) from text.
pub fn extract_entities(text: &str) -> Vec<String> {
    let mut entities = Vec::new();

    for word in text.split_whitespace() {
        if word.contains('.')
            && (word.ends_with(".rs")
                || word.ends_with(".ts")
                || word.ends_with(".tsx")
                || word.ends_with(".pdf")
                || word.ends_with(".docx"))
        {
            entities.push(word.to_string());
        }
    }

    let mut in_quote = false;
    let mut current_entity = String::new();
    for ch in text.chars() {
        if ch == '"' || ch == '\'' {
            if in_quote && !current_entity.is_empty() {
                entities.push(current_entity.clone());
                current_entity.clear();
            }
            in_quote = !in_quote;
        } else if in_quote {
            current_entity.push(ch);
        }
    }

    entities
}
