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

use super::calendar::{CalendarEvent, TodoItem};
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
    if let Some(location) = event.location.as_deref().filter(|l| !l.trim().is_empty()) {
        parts.push(format!("Location: {}.", location.trim()));
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

/// The calendar as it holds an item now, read under the lock that keeps the
/// writes of calendar index entries one at a time. Every writer of an entry
/// ([`reconcile_task`], [`reconcile_event`], [`index_missing`]) takes `lock`,
/// reads the item, and writes what it read, so the entry always ends as the
/// calendar's latest version, whatever order the writers run in.
pub struct CalendarNow<'a> {
    /// Taken while one item's entry is read and written.
    pub lock: &'a tokio::sync::Mutex<()>,
    /// The task `id` as the calendar holds it now: `None` when it is gone, an
    /// error when the calendar cannot be read (the entry is left as it is).
    pub task: &'a (dyn Fn(&str) -> std::result::Result<Option<TodoItem>, String> + Sync),
    /// The event `id`, as `task` gives a task.
    pub event: &'a (dyn Fn(&str) -> std::result::Result<Option<CalendarEvent>, String> + Sync),
}

/// What [`reconcile_task`] or [`reconcile_event`] did to an item's entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciled {
    /// Indexed as the calendar holds the item now.
    Indexed,
    /// Removed: the item is gone from the calendar.
    Removed,
    /// Left as it is: the calendar could not be read.
    Unread,
}

/// Brings the index entry of the task `id` in line with the calendar now
/// (see [`CalendarNow`]): the stored task is indexed, or its entry removed
/// when it is gone.
pub async fn reconcile_task(
    rag: &tokio::sync::RwLock<RAGEngine>,
    now: &CalendarNow<'_>,
    id: &str,
    space_id: &str,
) -> Result<Reconciled> {
    let _turn = now.lock.lock().await;
    match (now.task)(id) {
        Ok(Some(task)) => {
            index_task(&mut *rag.write().await, &task, space_id).await?;
            Ok(Reconciled::Indexed)
        }
        Ok(None) => {
            deindex_task(&mut *rag.write().await, id).await?;
            Ok(Reconciled::Removed)
        }
        Err(error) => {
            tracing::warn!(task_id = %id, %error, "calendar unreadable; the task's index entry is left as it is");
            Ok(Reconciled::Unread)
        }
    }
}

/// [`reconcile_task`] for the event `id`.
pub async fn reconcile_event(
    rag: &tokio::sync::RwLock<RAGEngine>,
    now: &CalendarNow<'_>,
    id: &str,
    space_id: &str,
) -> Result<Reconciled> {
    let _turn = now.lock.lock().await;
    match (now.event)(id) {
        Ok(Some(event)) => {
            index_event(&mut *rag.write().await, &event, space_id).await?;
            Ok(Reconciled::Indexed)
        }
        Ok(None) => {
            deindex_event(&mut *rag.write().await, id).await?;
            Ok(Reconciled::Removed)
        }
        Err(error) => {
            tracing::warn!(event_id = %id, %error, "calendar unreadable; the event's index entry is left as it is");
            Ok(Reconciled::Unread)
        }
    }
}

/// Index the tasks and events of `tasks` and `events` (the calendar as it
/// was read when this started) that have no entry in the index yet, or one
/// of an older index format ([`crate::rag_engine::INDEX_FORMAT`]) or chunker
/// version ([`crate::rag_engine::CHUNKER_VERSION`]). Run at startup and after
/// the search models are installed, so the calendar is searchable after a
/// fresh index build. Items already indexed in the current format are left
/// as they are: edits are indexed when they are made (see
/// [`reconcile_task`]), and re-embedding the whole calendar at every start
/// would load the embedding model even when nothing is searched.
///
/// Each item is written as [`reconcile_task`] writes it: under `now.lock`,
/// read again from the calendar, so one deleted meanwhile is not indexed
/// back and one edited meanwhile is indexed as it is now; the engine is held
/// for one item at a time. One that fails is logged, and the others go on.
/// Returns how many tasks and events were indexed.
pub async fn index_missing(
    rag: &tokio::sync::RwLock<RAGEngine>,
    now: &CalendarNow<'_>,
    tasks: &[TodoItem],
    events: &[CalendarEvent],
    space_id: &str,
) -> Result<(usize, usize)> {
    // Items indexed in an older index format or chunker version count as
    // missing: indexing one again chunks it anew and replaces its chunks.
    let indexed: std::collections::HashSet<String> = {
        let engine = rag.read().await;
        let stale: std::collections::HashSet<String> = engine
            .outdated_sources()
            .await?
            .into_iter()
            .map(|row| row.source)
            .collect();
        engine
            .document_sources()
            .await?
            .into_iter()
            .map(|row| row.source)
            .filter(|source| !stale.contains(source))
            .collect()
    };
    let mut tasks_indexed = 0usize;
    let mut events_indexed = 0usize;

    for task in tasks
        .iter()
        .filter(|t| !indexed.contains(&format!("calendar://task/{}", t.id)))
    {
        match reconcile_task(rag, now, &task.id, space_id).await {
            Ok(Reconciled::Indexed) => tasks_indexed += 1,
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "Failed to index a calendar task")
            }
        }
    }

    for event in events
        .iter()
        .filter(|e| !indexed.contains(&format!("calendar://event/{}", e.id)))
    {
        match reconcile_event(rag, now, &event.id, space_id).await {
            Ok(Reconciled::Indexed) => events_indexed += 1,
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(event_id = %event.id, error = %e, "Failed to index a calendar event")
            }
        }
    }

    Ok((tasks_indexed, events_indexed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_text_includes_the_location() {
        let mut event: CalendarEvent = serde_json::from_str(
            r#"{"id": "e1", "title": "Board review", "startTime": "2026-10-10T10:00",
                "createdAt": "2026-10-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(!event_to_indexable_text(&event).contains("Location"));
        event.location = Some(" Room 4B ".into());
        assert!(event_to_indexable_text(&event).contains("Location: Room 4B."));
    }

    /// Counts the texts it embeds.
    struct CountingEmbedder(std::sync::atomic::AtomicUsize);

    impl crate::embeddings::EmbeddingModel for CountingEmbedder {
        fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            crate::statements::testing::HashEmbedder.embed_query(text)
        }
        fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            crate::statements::testing::HashEmbedder.embed_document(text)
        }
        fn dimension(&self) -> usize {
            crate::statements::testing::DIM
        }
    }

    fn task(id: &str, title: &str) -> TodoItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title,
            "description": format!("{title}: a task on the weekly list, with notes on what to bring, who to call first and when the office opens."),
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn startup_indexing_embeds_only_items_missing_from_the_index() {
        use crate::embeddings::EmbeddingModel as _;
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        let mut engine = RAGEngine::new(config).await.unwrap();
        let embedder = std::sync::Arc::new(CountingEmbedder(Default::default()));
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(
                embedder.clone(),
            ))
            .unwrap();
        let embedded = || embedder.0.load(std::sync::atomic::Ordering::SeqCst);
        let engine = tokio::sync::RwLock::new(engine);
        let lock = tokio::sync::Mutex::new(());
        let calendar = std::sync::Mutex::new(Vec::<TodoItem>::new());
        let find = |id: &str| -> std::result::Result<Option<TodoItem>, String> {
            Ok(calendar
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.id == id)
                .cloned())
        };
        let no_events =
            |_: &str| -> std::result::Result<Option<CalendarEvent>, String> { Ok(None) };
        let now = CalendarNow {
            lock: &lock,
            task: &find,
            event: &no_events,
        };

        let first = [task("t1", "Renew the library card")];
        *calendar.lock().unwrap() = first.to_vec();
        assert_eq!(
            index_missing(&engine, &now, &first, &[], "calendar")
                .await
                .unwrap(),
            (1, 0)
        );
        let after_first = embedded();
        assert!(after_first > 0);

        // The next start: nothing new, nothing embedded.
        assert_eq!(
            index_missing(&engine, &now, &first, &[], "calendar")
                .await
                .unwrap(),
            (0, 0)
        );
        assert_eq!(embedded(), after_first);

        // A task added while the index did not have it is indexed; the known one is not.
        let both = [
            task("t1", "Renew the library card"),
            task("t2", "Water the plants"),
        ];
        *calendar.lock().unwrap() = both.to_vec();
        assert_eq!(
            index_missing(&engine, &now, &both, &[], "calendar")
                .await
                .unwrap(),
            (1, 0)
        );
        assert_eq!(
            engine.read().await.document_sources().await.unwrap().len(),
            2
        );
    }

    /// The startup indexing reads each item again as it writes it: a task
    /// deleted after the calendar was read is not indexed back, and one
    /// edited meanwhile ends indexed as edited, as an edit's own indexing
    /// (under the same lock) leaves it.
    #[tokio::test]
    async fn startup_indexing_writes_the_calendar_as_it_is_now() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = crate::statements::testing::DIM;
        let mut engine = RAGEngine::new(config).await.unwrap();
        engine
            .attach_search_models(crate::rag_engine::SearchModels::from_embedder(
                std::sync::Arc::new(crate::statements::testing::HashEmbedder),
            ))
            .unwrap();
        let engine = tokio::sync::RwLock::new(engine);
        let snapshot = vec![
            task("t1", "Renew the library card"),
            task("t2", "Water the plants"),
            task("t3", "Call the plumber"),
        ];
        let calendar = std::sync::Mutex::new(snapshot.clone());
        let lock = tokio::sync::Mutex::new(());
        // While the first task is written, t2 is deleted and t3 edited.
        let find = |id: &str| -> std::result::Result<Option<TodoItem>, String> {
            let mut tasks = calendar.lock().unwrap();
            if id == "t1" {
                tasks.retain(|t| t.id != "t2");
                if let Some(t3) = tasks.iter_mut().find(|t| t.id == "t3") {
                    *t3 = task("t3", "Call the electrician");
                }
            }
            Ok(tasks.iter().find(|t| t.id == id).cloned())
        };
        let no_events =
            |_: &str| -> std::result::Result<Option<CalendarEvent>, String> { Ok(None) };
        let now = CalendarNow {
            lock: &lock,
            task: &find,
            event: &no_events,
        };
        assert_eq!(
            index_missing(&engine, &now, &snapshot, &[], "calendar")
                .await
                .unwrap(),
            (2, 0)
        );
        let engine = engine.read().await;
        let sources: std::collections::BTreeSet<String> = engine
            .document_sources()
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.source)
            .collect();
        assert_eq!(
            sources,
            std::collections::BTreeSet::from([
                "calendar://task/t1".to_string(),
                "calendar://task/t3".to_string()
            ])
        );
        let t3 = engine
            .document_metadata("calendar://task/t3")
            .await
            .unwrap();
        assert_eq!(
            t3.get("title").map(String::as_str),
            Some("Call the electrician")
        );
    }
}
