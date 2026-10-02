//! Calendar Indexer — Indexes tasks and events into the RAG pipeline
//!
//! Each task/event becomes a single-chunk "document" in LanceDB + Tantivy,
//! making calendar items discoverable via the same hybrid semantic search
//! that powers document retrieval.
//!
//! Source identifiers: `calendar://task/{id}` and `calendar://event/{id}`
//! enable targeted deletion on update/delete operations.

use anyhow::Result;
use std::collections::HashMap;

use super::calendar_tools::{CalendarEvent, TodoItem};
use crate::rag_engine::RAGEngine;
use crate::types::{Citation, DocumentFormat};

/// Compose rich searchable text for a task.
///
/// Includes a structured context prefix (similar to contextual chunking)
/// followed by the description, so the embedding captures both metadata
/// and semantic content.
fn task_to_indexable_text(task: &TodoItem) -> String {
    let mut parts = Vec::with_capacity(8);

    parts.push(format!("Task: \"{}\".", task.title));

    if !task.priority.is_empty() {
        parts.push(format!("Priority: {}.", task.priority));
    }
    if !task.status.is_empty() {
        parts.push(format!("Status: {}.", task.status));
    }
    if let Some(ref project) = task.project {
        parts.push(format!("Project: {}.", project));
    }
    if let Some(ref due) = task.due_date {
        parts.push(format!("Due: {}.", due));
    }
    if !task.tags.is_empty() {
        parts.push(format!("Tags: {}.", task.tags.join(", ")));
    }
    if !task.source.is_empty() {
        parts.push(format!("Created by: {}.", task.source));
    }

    // Subtasks
    if !task.subtasks.is_empty() {
        let subtask_text: Vec<String> = task
            .subtasks
            .iter()
            .map(|s| {
                let check = if s.completed { "[x]" } else { "[ ]" };
                format!("{} {}", check, s.title)
            })
            .collect();
        parts.push(format!("Subtasks: {}", subtask_text.join("; ")));
    }

    // Description as the main body
    if !task.description.is_empty() {
        parts.push(format!("\n{}", task.description));
    }

    parts.join(" ")
}

/// Compose rich searchable text for an event.
fn event_to_indexable_text(event: &CalendarEvent) -> String {
    let mut parts = Vec::with_capacity(6);

    parts.push(format!("Event: \"{}\".", event.title));
    parts.push(format!("Start: {}.", event.start_time));

    if let Some(ref end) = event.end_time {
        parts.push(format!("End: {}.", end));
    }
    if event.all_day {
        parts.push("All-day event.".to_string());
    }
    if !event.source.is_empty() {
        parts.push(format!("Created by: {}.", event.source));
    }
    if !event.description.is_empty() {
        parts.push(format!("\n{}", event.description));
    }

    parts.join(" ")
}

/// Build metadata HashMap for a task.
fn task_metadata(task: &TodoItem, space_id: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("doc_type".to_string(), "task".to_string());
    m.insert("task_id".to_string(), task.id.clone());
    m.insert("title".to_string(), task.title.clone());
    m.insert("priority".to_string(), task.priority.clone());
    m.insert("status".to_string(), task.status.clone());
    m.insert("source".to_string(), format!("calendar://task/{}", task.id));
    m.insert("space_id".to_string(), space_id.to_string());
    m.insert(
        "file_path".to_string(),
        format!("calendar://task/{}", task.id),
    );
    m.insert("indexed_at".to_string(), chrono::Utc::now().to_rfc3339());

    if let Some(ref project) = task.project {
        m.insert("project".to_string(), project.clone());
    }
    if !task.tags.is_empty() {
        m.insert("tags".to_string(), task.tags.join(","));
    }
    if let Some(ref due) = task.due_date {
        m.insert("due_date".to_string(), due.clone());
    }
    if !task.source.is_empty() {
        m.insert("created_by".to_string(), task.source.clone());
    }

    m
}

/// Build metadata HashMap for an event.
fn event_metadata(event: &CalendarEvent, space_id: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("doc_type".to_string(), "event".to_string());
    m.insert("event_id".to_string(), event.id.clone());
    m.insert("title".to_string(), event.title.clone());
    m.insert("start_time".to_string(), event.start_time.clone());
    m.insert(
        "source".to_string(),
        format!("calendar://event/{}", event.id),
    );
    m.insert("space_id".to_string(), space_id.to_string());
    m.insert(
        "file_path".to_string(),
        format!("calendar://event/{}", event.id),
    );
    m.insert("indexed_at".to_string(), chrono::Utc::now().to_rfc3339());

    if let Some(ref end) = event.end_time {
        m.insert("end_time".to_string(), end.clone());
    }
    if event.all_day {
        m.insert("all_day".to_string(), "true".to_string());
    }
    if !event.source.is_empty() {
        m.insert("created_by".to_string(), event.source.clone());
    }

    m
}

/// Index a task into the RAG engine (LanceDB + Tantivy).
///
/// Idempotent: deletes any existing index for this task before inserting.
pub async fn index_task(rag: &mut RAGEngine, task: &TodoItem, space_id: &str) -> Result<()> {
    let source = format!("calendar://task/{}", task.id);

    // Remove existing index entry (idempotent update)
    rag.delete_by_source(&source).await.ok();

    let content = task_to_indexable_text(task);
    let metadata = task_metadata(task, space_id);
    let citation = Citation {
        title: task.title.clone(),
        authors: Vec::new(),
        source: source.clone(),
        year: task.created_at.chars().take(4).collect(),
        url: None,
        doi: None,
        page_numbers: None,
    };

    rag.add_document(&content, DocumentFormat::TXT, metadata, citation)
        .await?;

    tracing::debug!(task_id = %task.id, title = %task.title, "Indexed task in RAG");
    Ok(())
}

/// Remove a task from the RAG index.
pub async fn deindex_task(rag: &mut RAGEngine, task_id: &str) -> Result<()> {
    let source = format!("calendar://task/{}", task_id);
    rag.delete_by_source(&source).await?;
    tracing::debug!(task_id = %task_id, "Deindexed task from RAG");
    Ok(())
}

/// Index an event into the RAG engine (LanceDB + Tantivy).
///
/// Idempotent: deletes any existing index for this event before inserting.
pub async fn index_event(rag: &mut RAGEngine, event: &CalendarEvent, space_id: &str) -> Result<()> {
    let source = format!("calendar://event/{}", event.id);

    rag.delete_by_source(&source).await.ok();

    let content = event_to_indexable_text(event);
    let metadata = event_metadata(event, space_id);
    let citation = Citation {
        title: event.title.clone(),
        authors: Vec::new(),
        source: source.clone(),
        year: event.created_at.chars().take(4).collect(),
        url: None,
        doi: None,
        page_numbers: None,
    };

    rag.add_document(&content, DocumentFormat::TXT, metadata, citation)
        .await?;

    tracing::debug!(event_id = %event.id, title = %event.title, "Indexed event in RAG");
    Ok(())
}

/// Remove an event from the RAG index.
pub async fn deindex_event(rag: &mut RAGEngine, event_id: &str) -> Result<()> {
    let source = format!("calendar://event/{}", event_id);
    rag.delete_by_source(&source).await?;
    tracing::debug!(event_id = %event_id, "Deindexed event from RAG");
    Ok(())
}

/// Bulk re-index all tasks and events. Used on startup to ensure
/// the RAG index is populated even after a fresh index build.
pub async fn reindex_all(
    rag: &mut RAGEngine,
    tasks: &[TodoItem],
    events: &[CalendarEvent],
    space_id: &str,
) -> Result<(usize, usize)> {
    let mut tasks_indexed = 0usize;
    let mut events_indexed = 0usize;

    for task in tasks {
        if let Err(e) = index_task(rag, task, space_id).await {
            tracing::warn!(task_id = %task.id, error = %e, "Failed to index task during bulk reindex");
        } else {
            tasks_indexed += 1;
        }
    }

    for event in events {
        if let Err(e) = index_event(rag, event, space_id).await {
            tracing::warn!(event_id = %event.id, error = %e, "Failed to index event during bulk reindex");
        } else {
            events_indexed += 1;
        }
    }

    tracing::info!(
        tasks = tasks_indexed,
        events = events_indexed,
        "Bulk re-indexed calendar data into RAG"
    );

    Ok((tasks_indexed, events_indexed))
}
