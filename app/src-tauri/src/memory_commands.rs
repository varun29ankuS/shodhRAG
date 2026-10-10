//! Long-term memory: managed state, the Settings → Memory commands and the recall
//! injected at the start of an agent run.
//!
//! Memories are typed statements in the LanceDB table `statements` (next to the document
//! index in `<app_data_dir>/lance_data`) with their dynamics in `shodh.db`. The store
//! opens on first use: it needs the search models (the embedder), which may be installed
//! after the app starts.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use shodh_rag::audit::{AuditKey, AuditLog};
use shodh_rag::embeddings::{EmbeddingModel, SearchModelsMissing};
use shodh_rag::statements::{
    DynamicsStore, EmbedderSource, Scope, StatementError, StatementResult, StatementStore,
    SystemClock,
};
use shodh_rag::user_memory::{
    render_injection_counted, Actor, ListRequest, MemoryContent, MemoryError, MemoryRecord,
    MemoryService, Origin, RecallMode, RecallRequest, RememberOutcome, MAX_INJECTION_CHARS,
};
use shodh_rag::RAGEngine;
use tauri::State;
use tokio::sync::{OnceCell, RwLock as AsyncRwLock};

use crate::audit_commands::AuditState;

/// Memories recalled into one agent run.
pub const INJECTED_MEMORIES: usize = 6;

/// The app version recorded on memories the user states.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The ontology memories are typed against: the core plus the built-in research pack.
pub fn memory_ontology() -> Result<shodh_ontology::Ontology, String> {
    shodh_ontology::Ontology::builtin_with_packs(&["research"]).map_err(|e| e.to_string())
}

/// The engine's embedding model, looked up per call (models can be attached later).
struct EngineEmbedder(Arc<AsyncRwLock<RAGEngine>>);

#[async_trait::async_trait]
impl EmbedderSource for EngineEmbedder {
    async fn embedder(&self) -> StatementResult<Arc<dyn EmbeddingModel>> {
        self.0
            .read()
            .await
            .shared_embeddings()
            .ok_or_else(|| StatementError::EmbeddingUnavailable(SearchModelsMissing.to_string()))
    }
}

struct Deps {
    lance_dir: PathBuf,
    rag: Arc<AsyncRwLock<RAGEngine>>,
    audit: Option<Arc<AuditLog>>,
    database: Option<(PathBuf, Option<AuditKey>)>,
}

/// Managed state: the memory service, opened on first use. Clones share it (the agent
/// tools hold one).
#[derive(Clone)]
pub struct MemoryState {
    inner: Arc<Inner>,
}

struct Inner {
    service: OnceCell<Arc<MemoryService>>,
    deps: Option<Deps>,
}

impl MemoryState {
    /// State that opens the store under `app_data_dir` on first use.
    pub fn new(app_data_dir: &Path, rag: Arc<AsyncRwLock<RAGEngine>>, audit: &AuditState) -> Self {
        Self {
            inner: Arc::new(Inner {
                service: OnceCell::new(),
                deps: Some(Deps {
                    lance_dir: app_data_dir.join("lance_data"),
                    rag,
                    audit: audit.log(),
                    database: audit.database(),
                }),
            }),
        }
    }

    /// State over an already open service (tests).
    #[cfg(test)]
    pub fn ready(service: Arc<MemoryService>) -> Self {
        Self {
            inner: Arc::new(Inner {
                service: OnceCell::new_with(Some(service)),
                deps: None,
            }),
        }
    }

    /// The memory service, opening it if needed. Fails (and retries on the next call)
    /// while the audit database is unavailable or the store cannot open.
    pub async fn service(&self) -> Result<Arc<MemoryService>, String> {
        self.inner
            .service
            .get_or_try_init(|| async {
                let deps = self
                    .inner
                    .deps
                    .as_ref()
                    .ok_or_else(|| "Memory is not available in this build.".to_string())?;
                let (db_path, key) = deps.database.clone().ok_or_else(|| {
                    "Memory needs the app database (shodh.db), which could not be opened; see the audit page."
                        .to_string()
                })?;
                let dynamics = tokio::task::spawn_blocking(move || {
                    DynamicsStore::open(&db_path, key.as_ref())
                })
                .await
                .map_err(|e| format!("Opening memory failed: {e}"))?
                .map_err(|e| format!("Opening memory failed: {e}"))?;
                let dimension = deps.rag.read().await.config().embedding.dimension;
                let ontology = Arc::new(memory_ontology()?);
                let store = StatementStore::open(
                    &deps.lance_dir,
                    dimension,
                    ontology,
                    Arc::new(dynamics),
                    Arc::new(EngineEmbedder(deps.rag.clone())),
                    Arc::new(SystemClock),
                )
                .await
                .map_err(|e| format!("Opening memory failed: {e}"))?;
                Ok(Arc::new(MemoryService::new(
                    Arc::new(store),
                    deps.audit.clone(),
                    APP_VERSION,
                )))
            })
            .await
            .cloned()
    }
}

/// Text of a memory error for the UI.
pub fn error_text(error: MemoryError) -> String {
    error.to_string()
}

/// Longest the memory recall may delay the start of an answer (it waits for the engine
/// while indexing holds it). On timeout the answer starts without memories.
pub const RECALL_TIMEOUT: Duration = Duration::from_millis(1_500);

/// The memory block for the start of an agent run: memories relevant to `text`. The
/// read-only ranking is bounded by [`RECALL_TIMEOUT`]; the memories it returns are then
/// recorded as used (reinforced, linked, audited as `memory_use`). `None` when nothing is
/// relevant or memory is unavailable; failures never block the answer.
pub async fn recall_for_run(
    memory: &MemoryState,
    text: &str,
    workspace: Option<&str>,
    actor: &Actor,
) -> Option<String> {
    let request = RecallRequest::new(
        text,
        Scope::for_workspace(workspace),
        INJECTED_MEMORIES,
        RecallMode::Inspect,
    );
    let ranked = tokio::time::timeout(RECALL_TIMEOUT, async {
        let service = memory.service().await.map_err(RecallFailure::Unavailable)?;
        let recalled = service
            .recall(&request, actor)
            .await
            .map_err(RecallFailure::Memory)?;
        Ok::<_, RecallFailure>((service, recalled))
    })
    .await;
    let (service, recalled) = match ranked {
        Ok(Ok(ranked)) => ranked,
        Ok(Err(RecallFailure::Unavailable(e))) => {
            tracing::debug!(target: "shodh::memory", error = %e, "memory unavailable; nothing injected");
            return None;
        }
        Ok(Err(RecallFailure::Memory(MemoryError::Statement(
            StatementError::EmbeddingUnavailable(_),
        )))) => {
            tracing::debug!(target: "shodh::memory", "search models not installed; no memories recalled");
            return None;
        }
        Ok(Err(RecallFailure::Memory(e))) => {
            tracing::warn!(target: "shodh::memory", error = %e, "memory recall failed; nothing injected");
            return None;
        }
        Err(_) => {
            tracing::warn!(target: "shodh::memory", timeout_ms = RECALL_TIMEOUT.as_millis(), "memory recall timed out; nothing injected");
            return None;
        }
    };
    let (block, injected) = render_injection_counted(&recalled, MAX_INJECTION_CHARS)?;
    if let Err(e) = service
        .record_use(&recalled[..injected], &request, actor)
        .await
    {
        tracing::warn!(target: "shodh::memory", error = %e, "recording memory use failed");
    }
    Some(block)
}

enum RecallFailure {
    Unavailable(String),
    Memory(MemoryError),
}

/// `message` with the memory block in front of it, delimited from the user's words.
pub fn with_memories(block: Option<&str>, message: &str) -> String {
    match block {
        Some(block) => format!("{block}\n\nCurrent message:\n{message}"),
        None => message.to_string(),
    }
}

/// Memories for Settings → Memory: current ones (and history on request), optionally
/// searched. Listing is not use: nothing is reinforced.
#[tauri::command]
pub async fn memory_list(
    request: Option<ListRequest>,
    memory: State<'_, MemoryState>,
) -> Result<Vec<MemoryRecord>, String> {
    let service = memory.service().await?;
    service
        .list(&request.unwrap_or_default())
        .await
        .map_err(error_text)
}

/// Every version of the fact a memory belongs to, oldest first.
#[tauri::command]
pub async fn memory_history(
    id: String,
    memory: State<'_, MemoryState>,
) -> Result<Vec<MemoryRecord>, String> {
    let service = memory.service().await?;
    service.history(&id).await.map_err(error_text)
}

/// Edit a memory in Settings: `content` is the complete new fact. The old version is
/// kept as history. Audited as `memory_write`.
#[tauri::command]
pub async fn memory_update(
    id: String,
    content: MemoryContent,
    memory: State<'_, MemoryState>,
) -> Result<RememberOutcome, String> {
    let service = memory.service().await?;
    service
        .update(
            &id,
            content,
            false,
            None,
            &Origin::user_interface(APP_VERSION),
            &Actor::ui(),
        )
        .await
        .map_err(error_text)
}

/// Pin (exempt from decay) or unpin a memory. Audited as `memory_write`.
#[tauri::command]
pub async fn memory_set_pinned(
    id: String,
    pinned: bool,
    memory: State<'_, MemoryState>,
) -> Result<MemoryRecord, String> {
    let service = memory.service().await?;
    service
        .set_pinned(&id, pinned, &Actor::ui())
        .await
        .map_err(error_text)
}

/// Forget a memory and all its versions. Audited as `memory_forget`.
#[tauri::command]
pub async fn memory_forget(
    id: String,
    memory: State<'_, MemoryState>,
) -> Result<Vec<String>, String> {
    let service = memory.service().await?;
    service
        .forget(&id, None, &Actor::ui())
        .await
        .map_err(error_text)
}

/// Where an export was written and how many memories it holds.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryExport {
    pub path: String,
    pub memories: usize,
}

/// Export every memory (with history) as JSON to `path` (from the save dialog).
#[tauri::command]
pub async fn memory_export(
    path: String,
    memory: State<'_, MemoryState>,
) -> Result<MemoryExport, String> {
    let path = export_path(&path)?;
    let service = memory.service().await?;
    let document = service.export().await.map_err(error_text)?;
    let memories = document["memories"].as_array().map_or(0, Vec::len);
    let text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
    let target = path.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&target, text))
        .await
        .map_err(|e| format!("Export failed: {e}"))?
        .map_err(|e| format!("Export to {} failed: {e}", path.display()))?;
    Ok(MemoryExport {
        path: path.display().to_string(),
        memories,
    })
}

fn export_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err("Choose where to save the export".to_string());
    }
    match path.parent() {
        Some(parent) if parent.is_dir() => {}
        _ => return Err(format!("The folder for {} does not exist", path.display())),
    }
    if path.is_dir() {
        return Err(format!("{} is a folder", path.display()));
    }
    let is_json = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    Ok(if is_json {
        path
    } else {
        path.with_extension("json")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_is_kept_apart_from_the_users_words() {
        assert_eq!(with_memories(None, "hello"), "hello");
        let message = with_memories(Some("<memory>\n- x\n</memory>"), "What's next?");
        assert!(message.starts_with("<memory>"));
        assert!(message.ends_with("Current message:\nWhat's next?"));
    }

    #[test]
    fn exports_are_json_files_in_existing_folders() {
        let dir = tempfile::tempdir().unwrap();
        let p = export_path(&dir.path().join("mem.txt").display().to_string()).unwrap();
        assert_eq!(p.extension().unwrap(), "json");
        let p = export_path(&dir.path().join("mem.JSON").display().to_string()).unwrap();
        assert_eq!(p.file_name().unwrap(), "mem.JSON");
        assert!(export_path("relative.json").is_err());
        assert!(export_path(&dir.path().display().to_string()).is_err());
        assert!(export_path(
            &dir.path()
                .join("missing")
                .join("m.json")
                .display()
                .to_string()
        )
        .is_err());
    }

    #[tokio::test]
    async fn a_run_gets_relevant_memories_and_records_only_their_use() {
        use shodh_rag::audit::{AuditEventType, AuditQuery, AuditRecord};
        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(AuditLog::open(dir.path().join("shodh.db"), None).unwrap());
        let memory =
            crate::agent_tools::testing::memory_state(dir.path(), Some(audit.clone())).await;
        let service = memory.service().await.unwrap();
        for text in ["Invoices go to Priya", "Gym on Tuesdays"] {
            service
                .remember(
                    MemoryContent::Note { text: text.into() },
                    Scope::Global,
                    &Origin::user_interface("t"),
                    &Actor::ui(),
                )
                .await
                .unwrap();
        }
        let actor = Actor::agent("local-owner", "conv-1", "assistant", "run-1");
        // The default floor is tuned for E5; the test embedder scores identical words 1.
        let block = recall_for_run(&memory, "Invoices go to Priya?", None, &actor)
            .await
            .unwrap();
        assert!(block.contains("Invoices go to Priya"));
        assert!(!block.contains("Gym"));
        let listed = service.list(&ListRequest::default()).await.unwrap();
        let uses = |needle: &str| {
            listed
                .iter()
                .find(|m| m.text.contains(needle))
                .unwrap()
                .use_count
        };
        assert_eq!(uses("Priya"), 1);
        assert_eq!(uses("Gym"), 0);
        assert!(
            recall_for_run(&memory, "Explain gradient descent", None, &actor)
                .await
                .is_none()
        );
        audit
            .append(AuditRecord::new(
                AuditEventType::Question,
                serde_json::json!({}),
            ))
            .unwrap();
        let used = audit
            .query(&AuditQuery {
                types: vec![AuditEventType::MemoryUse],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(used.len(), 1);
        assert_eq!(used[0].conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(used[0].run_id.as_deref(), Some("run-1"));
    }

    #[test]
    fn the_memory_ontology_loads() {
        let ontology = memory_ontology().unwrap();
        for class in [
            "Note",
            "Preference",
            "Person",
            "Project",
            "Decision",
            "Concept",
        ] {
            assert!(ontology.class(class).is_some(), "{class}");
        }
    }
}
