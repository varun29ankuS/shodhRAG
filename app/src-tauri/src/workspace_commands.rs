//! Workspaces: managed state, the commands of the Workspaces view, the one-time import of
//! conversations saved with a legacy space, and what an answer in a workspace may search.
//!
//! Workspaces live in `<app_data_dir>/shodh.db` (see `shodh_rag::workspaces`); the store
//! opens on first use with the database path and key the audit log was opened with. Every
//! store call runs on the blocking pool (SQLite is synchronous). Conversations stay in the
//! conversation file and name their workspace (`workspaceId`).
//!
//! Changes are broadcast as [`WORKSPACES_CHANGED_EVENT`] so open views refresh, also when
//! the assistant changed a workspace (after the user approved it).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::AuditKey;
use shodh_rag::harness::tools::sources::{load_sources, SourceKind as IndexKind};
use shodh_rag::harness::tools::ScopedSnippet;
use shodh_rag::research::ResearchError;
use shodh_rag::workspaces::{
    AddReport, InstructionVersion, NewSource, NewWorkspace, SourceKind, Workspace, WorkspaceAuthor,
    WorkspaceDetail, WorkspaceError, WorkspacePatch, WorkspaceResult, WorkspaceSource,
    WorkspaceStore, WorkspaceTemplate, TEMPLATES,
};
use shodh_rag::RAGEngine;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{OnceCell, RwLock};

use crate::audit_commands::AuditState;
use crate::conversation_commands::{ConversationRecord, ConversationStore};
use crate::research_commands::ResearchState;

/// Emitted with `{ "workspaceId": ... }` (null when several changed) after a workspace was
/// created, changed, deleted, or its sources or instructions changed.
pub const WORKSPACES_CHANGED_EVENT: &str = "workspaces-changed";

/// `workspace_state` key of the one-time import of legacy spaces.
const LEGACY_IMPORT_KEY: &str = "legacy_spaces_v1";

/// Error of a workspace command. `code` lets the UI pick its message without parsing text:
/// `not_found`, `invalid`, `stale`, `unavailable` or `storage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCommandError {
    pub code: &'static str,
    pub message: String,
}

impl WorkspaceCommandError {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "unavailable",
            message: message.into(),
        }
    }
}

impl From<WorkspaceError> for WorkspaceCommandError {
    fn from(error: WorkspaceError) -> Self {
        let code = match &error {
            WorkspaceError::NotFound(_) => "not_found",
            WorkspaceError::Invalid(_) => "invalid",
            WorkspaceError::Stale { .. } => "stale",
            WorkspaceError::Open(_) => "unavailable",
            WorkspaceError::Sqlite(_) => "storage",
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for WorkspaceCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type WorkspaceCommandResult<T> = Result<T, WorkspaceCommandError>;

/// Managed state: the workspace store, opened on first use. Clones share it (the agent
/// tools and the Ask path hold one).
#[derive(Clone)]
pub struct WorkspaceState {
    inner: Arc<Inner>,
}

struct Inner {
    store: OnceCell<Arc<WorkspaceStore>>,
    database: Option<(PathBuf, Option<AuditKey>)>,
}

impl WorkspaceState {
    /// State over the database the audit log opened (`None` when it could not be opened).
    pub fn new(audit: &AuditState) -> Self {
        Self::at(audit.database())
    }

    /// State over `shodh.db` at `database` (path and key).
    pub fn at(database: Option<(PathBuf, Option<AuditKey>)>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: OnceCell::new(),
                database,
            }),
        }
    }

    /// The store, opening it if needed. Fails (and retries on the next call) while the
    /// database is unavailable.
    pub async fn store(&self) -> WorkspaceCommandResult<Arc<WorkspaceStore>> {
        self.inner
            .store
            .get_or_try_init(|| async {
                let (path, key) = self.inner.database.clone().ok_or_else(|| {
                    WorkspaceCommandError::unavailable(
                        "Workspaces need the app database (shodh.db), which could not be \
                         opened; see the audit page.",
                    )
                })?;
                let store =
                    tokio::task::spawn_blocking(move || WorkspaceStore::open(&path, key.as_ref()))
                        .await
                        .map_err(|e| {
                            WorkspaceCommandError::unavailable(format!(
                                "Opening workspaces failed: {e}"
                            ))
                        })??;
                Ok(Arc::new(store))
            })
            .await
            .cloned()
    }

    /// Runs `call` against the store on the blocking pool.
    pub async fn run<T, F>(&self, call: F) -> WorkspaceCommandResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&WorkspaceStore) -> WorkspaceResult<T> + Send + 'static,
    {
        let store = self.store().await?;
        tokio::task::spawn_blocking(move || call(&store))
            .await
            .map_err(|e| WorkspaceCommandError {
                code: "storage",
                message: format!("The workspace task failed: {e}"),
            })?
            .map_err(WorkspaceCommandError::from)
    }
}

/// Tell open views that a workspace (or several, `None`) changed.
pub fn broadcast_change(app: &AppHandle, workspace_id: Option<&str>) {
    if let Err(e) = app.emit(
        WORKSPACES_CHANGED_EVENT,
        json!({ "workspaceId": workspace_id }),
    ) {
        tracing::warn!(target: "shodh::workspaces", error = %e, "workspace change not broadcast");
    }
}

fn data_dir(app: &AppHandle) -> WorkspaceCommandResult<PathBuf> {
    app.path().app_data_dir().map_err(|e| {
        WorkspaceCommandError::unavailable(format!("App data directory unavailable: {e}"))
    })
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// Chats of one workspace, from the conversation file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStats {
    pub chat_count: u32,
    /// `updatedAt` of the most recent chat.
    pub last_chat_at: Option<String>,
}

/// Chats per workspace id.
pub fn chat_stats(conversations: &[ConversationRecord]) -> HashMap<String, ChatStats> {
    let mut out: HashMap<String, ChatStats> = HashMap::new();
    for c in conversations {
        let Some(id) = c.workspace_id.as_deref().filter(|w| !w.is_empty()) else {
            continue;
        };
        let stats = out.entry(id.to_string()).or_default();
        stats.chat_count += 1;
        if stats
            .last_chat_at
            .as_deref()
            .is_none_or(|t| c.updated_at.as_str() > t)
        {
            stats.last_chat_at = Some(c.updated_at.clone());
        }
    }
    out
}

/// A workspace with its chat statistics, as the Workspaces view lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListing {
    #[serde(flatten)]
    pub workspace: Workspace,
    #[serde(flatten)]
    pub chats: ChatStats,
}

/// Workspaces with chat counts (archived ones when `include_archived`).
pub async fn list_with_chats(
    state: &WorkspaceState,
    data_dir: &Path,
    include_archived: bool,
) -> WorkspaceCommandResult<Vec<WorkspaceListing>> {
    let workspaces = state.run(move |s| s.list(include_archived)).await?;
    let conversations =
        ConversationStore::in_dir(data_dir)
            .load()
            .map_err(|e| WorkspaceCommandError {
                code: "storage",
                message: e,
            })?;
    let stats = chat_stats(&conversations);
    Ok(workspaces
        .into_iter()
        .map(|workspace| {
            let chats = stats.get(&workspace.id).cloned().unwrap_or_default();
            WorkspaceListing { workspace, chats }
        })
        .collect())
}

#[tauri::command]
pub async fn workspaces_list(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    include_archived: Option<bool>,
) -> WorkspaceCommandResult<Vec<WorkspaceListing>> {
    let dir = data_dir(&app)?;
    list_with_chats(&state, &dir, include_archived.unwrap_or(false)).await
}

#[tauri::command]
pub async fn workspaces_templates() -> WorkspaceCommandResult<Vec<WorkspaceTemplate>> {
    Ok(TEMPLATES.to_vec())
}

#[tauri::command]
pub async fn workspaces_get(
    state: State<'_, WorkspaceState>,
    id: String,
) -> WorkspaceCommandResult<WorkspaceDetail> {
    state.run(move |s| s.detail(&id)).await
}

#[tauri::command]
pub async fn workspaces_create(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    workspace: NewWorkspace,
) -> WorkspaceCommandResult<Workspace> {
    let created = state
        .run(move |s| s.create(&workspace, WorkspaceAuthor::User))
        .await?;
    broadcast_change(&app, Some(&created.id));
    Ok(created)
}

#[tauri::command]
pub async fn workspaces_update(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    id: String,
    patch: WorkspacePatch,
) -> WorkspaceCommandResult<Workspace> {
    if patch.archived.is_some() {
        return Err(WorkspaceCommandError {
            code: "invalid",
            message: "Archive a workspace with workspaces_set_archived".to_string(),
        });
    }
    let updated = state.run(move |s| s.update(&id, &patch)).await?;
    broadcast_change(&app, Some(&updated.id));
    Ok(updated)
}

/// Archives a workspace (hidden from the list and the sidebar; its chats and sources stay)
/// or brings it back.
#[tauri::command]
pub async fn workspaces_set_archived(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    id: String,
    archived: bool,
) -> WorkspaceCommandResult<Workspace> {
    let patch = WorkspacePatch {
        archived: Some(archived),
        ..WorkspacePatch::default()
    };
    let updated = state.run(move |s| s.update(&id, &patch)).await?;
    broadcast_change(&app, Some(&updated.id));
    Ok(updated)
}

/// Saves the user's instructions as a new version. `expected_version` is the version the
/// editor opened; an edit over a newer version is refused (`stale`).
#[tauri::command]
pub async fn workspaces_set_instructions(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    id: String,
    text: String,
    expected_version: Option<u32>,
    note: Option<String>,
) -> WorkspaceCommandResult<InstructionVersion> {
    let workspace_id = id.clone();
    let (version, changed) = state
        .run(move |s| {
            s.set_instructions(
                &id,
                &text,
                WorkspaceAuthor::User,
                note.as_deref(),
                expected_version,
            )
        })
        .await?;
    if changed {
        broadcast_change(&app, Some(&workspace_id));
    }
    Ok(version)
}

#[tauri::command]
pub async fn workspaces_instruction_history(
    state: State<'_, WorkspaceState>,
    id: String,
) -> WorkspaceCommandResult<Vec<InstructionVersion>> {
    state.run(move |s| s.instruction_history(&id)).await
}

#[tauri::command]
pub async fn workspaces_add_sources(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    id: String,
    sources: Vec<NewSource>,
) -> WorkspaceCommandResult<AddReport> {
    let workspace_id = id.clone();
    let report = state
        .run(move |s| s.add_sources(&id, &sources, WorkspaceAuthor::User))
        .await?;
    if report.added > 0 {
        broadcast_change(&app, Some(&workspace_id));
    }
    Ok(report)
}

#[tauri::command]
pub async fn workspaces_remove_source(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    id: String,
    kind: SourceKind,
    reference: String,
) -> WorkspaceCommandResult<bool> {
    let workspace_id = id.clone();
    let removed = state
        .run(move |s| s.remove_source(&id, kind, &reference))
        .await?;
    if removed {
        broadcast_change(&app, Some(&workspace_id));
    }
    Ok(removed)
}

/// Deletes a workspace (its instructions history and source list; never the sources).
/// Its chats are kept and move to "No workspace".
#[tauri::command]
pub async fn workspaces_delete(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    id: String,
) -> WorkspaceCommandResult<bool> {
    let dir = data_dir(&app)?;
    let workspace_id = id.clone();
    let removed = state.run(move |s| s.delete(&id)).await?;
    let detached = detach_conversations(&dir, &workspace_id)?;
    if removed || detached > 0 {
        broadcast_change(&app, Some(&workspace_id));
    }
    Ok(removed)
}

/// Clears `workspace_id` from the conversations that name it. Returns how many changed.
pub fn detach_conversations(data_dir: &Path, workspace_id: &str) -> WorkspaceCommandResult<usize> {
    ConversationStore::in_dir(data_dir)
        .update(|conversations| {
            let mut changed = 0;
            for c in conversations
                .iter_mut()
                .filter(|c| c.workspace_id.as_deref() == Some(workspace_id))
            {
                c.workspace_id = None;
                changed += 1;
            }
            Ok(changed)
        })
        .map_err(|e| WorkspaceCommandError {
            code: "storage",
            message: e,
        })
}

// ---------------------------------------------------------------------------
// Source status ("what's in this workspace")
// ---------------------------------------------------------------------------

/// Whether a source can be searched now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// In the index; answers search it.
    Indexed,
    /// A file that is not (or no longer) in the index.
    NotIndexed,
    /// A folder no longer in the Library, or a snippet that was deleted.
    Missing,
    /// A paper outside the library: shown for reference, never searched.
    Reference,
    /// Could not be checked right now (e.g. snippets need the search models).
    Unknown,
}

/// One source with its index state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub kind: SourceKind,
    #[serde(rename = "ref")]
    pub reference: String,
    pub state: SourceState,
    /// Indexed files (folders: files under it; files and library papers: 1).
    pub files: Option<usize>,
    /// Indexed passages.
    pub chunks: Option<usize>,
}

/// Index health of a workspace: what answers search, and what needs attention.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceHealth {
    pub sources: Vec<SourceStatus>,
    /// Files answers search (folder files plus single files and library papers).
    pub searchable_files: usize,
    /// Passages in those files, where known.
    pub chunks: usize,
    /// Sources answers cannot search (missing or not indexed).
    pub problems: usize,
}

/// The index state of each source of `detail`.
pub async fn source_health(
    detail: &WorkspaceDetail,
    rag: &RwLock<RAGEngine>,
    research: &ResearchState,
) -> SourceHealth {
    let engine = rag.read().await;
    let folders: HashMap<String, (usize, usize)> = match load_sources(&engine).await {
        Ok(sources) => sources
            .into_iter()
            .filter(|s| s.kind == IndexKind::Folder)
            .map(|s| (s.source_id, (s.files, s.chunks)))
            .collect(),
        Err(e) => {
            tracing::warn!(target: "shodh::workspaces", error = %e, "indexed sources unreadable");
            HashMap::new()
        }
    };
    let mut health = SourceHealth::default();
    for source in &detail.sources {
        let (state, files, chunks) = match source.kind {
            SourceKind::Folder => match folders.get(&source.reference) {
                Some(&(files, chunks)) => (SourceState::Indexed, Some(files), Some(chunks)),
                None => (SourceState::Missing, None, None),
            },
            SourceKind::File | SourceKind::Paper => match source.path.as_deref() {
                None => (SourceState::Reference, None, None),
                Some(path) => match engine.find_indexed_sources(path).await {
                    Ok(found) if !found.is_empty() => (SourceState::Indexed, Some(1), None),
                    Ok(_) => (SourceState::NotIndexed, None, None),
                    Err(e) => {
                        tracing::warn!(target: "shodh::workspaces", error = %e, "index lookup failed");
                        (SourceState::Unknown, None, None)
                    }
                },
            },
            SourceKind::Snippet => match research.services().await {
                Ok(services) => match services.snippets.get(&source.reference).await {
                    Ok(_) => (SourceState::Indexed, None, None),
                    Err(ResearchError::NotFound(_)) => (SourceState::Missing, None, None),
                    Err(e) => {
                        tracing::warn!(target: "shodh::workspaces", error = %e, "snippet lookup failed");
                        (SourceState::Unknown, None, None)
                    }
                },
                Err(_) => (SourceState::Unknown, None, None),
            },
        };
        if matches!(state, SourceState::Missing | SourceState::NotIndexed) {
            health.problems += 1;
        }
        if state == SourceState::Indexed && source.kind != SourceKind::Snippet {
            health.searchable_files += files.unwrap_or(0);
            health.chunks += chunks.unwrap_or(0);
        }
        health.sources.push(SourceStatus {
            kind: source.kind,
            reference: source.reference.clone(),
            state,
            files,
            chunks,
        });
    }
    health
}

#[tauri::command]
pub async fn workspaces_source_health(
    state: State<'_, WorkspaceState>,
    rag: State<'_, crate::rag_commands::RagState>,
    research: State<'_, ResearchState>,
    id: String,
) -> WorkspaceCommandResult<SourceHealth> {
    let detail = state.run(move |s| s.detail(&id)).await?;
    Ok(source_health(&detail, &rag.rag, &research).await)
}

// ---------------------------------------------------------------------------
// What an answer in a workspace may search
// ---------------------------------------------------------------------------

/// A workspace as one answer uses it: its name and instructions, and the sources the
/// answer is limited to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnswerWorkspace {
    pub id: String,
    pub name: String,
    pub instructions: String,
    /// Folder source ids (`space_id`s).
    pub source_ids: Vec<String>,
    /// Folder paths, for checking a path read outside search.
    pub folders: Vec<String>,
    /// Single files and library papers.
    pub files: Vec<String>,
    pub snippets: Vec<ScopedSnippet>,
    /// Snippets that could not be loaded (deleted, or the store is unavailable).
    pub unavailable_snippets: usize,
}

/// Splits a workspace's sources into what search filters on. Snippets are loaded by the
/// caller (they need the research store).
pub fn answer_sources(detail: &WorkspaceDetail) -> AnswerWorkspace {
    let mut out = AnswerWorkspace {
        id: detail.workspace.id.clone(),
        name: detail.workspace.name.clone(),
        instructions: detail.instructions.clone(),
        ..AnswerWorkspace::default()
    };
    for source in &detail.sources {
        match source.kind {
            SourceKind::Folder => {
                out.source_ids.push(source.reference.clone());
                if let Some(path) = &source.path {
                    out.folders.push(path.clone());
                }
            }
            SourceKind::File | SourceKind::Paper => {
                if let Some(path) = &source.path {
                    out.files.push(path.clone());
                }
            }
            SourceKind::Snippet => {}
        }
    }
    out.files.sort();
    out.files.dedup();
    out
}

/// The workspace `id` as one answer uses it, or `None` when no workspace has that id
/// (a conversation of a deleted workspace, or a legacy space id never imported).
pub async fn answer_workspace(
    state: &WorkspaceState,
    research: &ResearchState,
    id: &str,
) -> WorkspaceCommandResult<Option<AnswerWorkspace>> {
    let lookup = id.to_string();
    let detail = match state.run(move |s| s.detail(&lookup)).await {
        Ok(detail) => detail,
        Err(e) if e.code == "not_found" => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut answer = answer_sources(&detail);
    let snippet_sources: Vec<&WorkspaceSource> = detail
        .sources
        .iter()
        .filter(|s| s.kind == SourceKind::Snippet)
        .collect();
    if !snippet_sources.is_empty() {
        match research.services().await {
            Ok(services) => {
                for source in snippet_sources {
                    match services.snippets.get(&source.reference).await {
                        Ok(snippet) => answer.snippets.push(ScopedSnippet {
                            id: snippet.id,
                            file_path: snippet.file_path,
                            file_name: snippet.file_name,
                            page: snippet.page,
                            title: snippet.title,
                            text: snippet.text,
                        }),
                        Err(e) => {
                            tracing::debug!(target: "shodh::workspaces", error = %e, "workspace snippet unavailable");
                            answer.unavailable_snippets += 1;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::info!(target: "shodh::workspaces", error = %e, "snippets unavailable for a workspace answer");
                answer.unavailable_snippets += snippet_sources.len();
            }
        }
    }
    // Recorded for "last active"; a failure only affects ordering.
    let touched = id.to_string();
    if let Err(e) = state.run(move |s| s.touch(&touched)).await {
        tracing::debug!(target: "shodh::workspaces", error = %e, "workspace activity not recorded");
    }
    Ok(Some(answer))
}

// ---------------------------------------------------------------------------
// Import of conversations saved with a legacy space
// ---------------------------------------------------------------------------

/// A folder source as the index knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownFolder {
    pub source_id: String,
    pub folder: String,
}

/// A legacy space that becomes a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySpace {
    /// The space id, kept as the workspace id.
    pub id: String,
    pub name: String,
    pub folder: String,
}

/// What the import does: the workspaces to create and, per conversation, the workspace it
/// joins. Conversations whose space is not an indexed folder any more stay in "No
/// workspace" (their space fields are kept as they were).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyPlan {
    pub create: Vec<LegacySpace>,
    pub assign: BTreeMap<String, String>,
}

/// Plans the import. `existing` holds the ids of workspaces that already exist (an earlier,
/// interrupted import created them; they are reused, not created again).
pub fn plan_legacy_import(
    conversations: &[ConversationRecord],
    folders: &[KnownFolder],
    existing: &HashSet<String>,
) -> LegacyPlan {
    let by_id: HashMap<&str, &KnownFolder> =
        folders.iter().map(|f| (f.source_id.as_str(), f)).collect();
    let mut plan = LegacyPlan::default();
    let mut planned: HashSet<String> = HashSet::new();
    for c in conversations {
        if c.workspace_id.as_deref().is_some_and(|w| !w.is_empty()) {
            continue;
        }
        let Some(space) = c
            .space_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(folder) = by_id.get(space) else {
            continue;
        };
        if !existing.contains(space) && planned.insert(space.to_string()) {
            let name = c
                .space_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    Path::new(&folder.folder)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| folder.folder.clone());
            plan.create.push(LegacySpace {
                id: space.to_string(),
                name: name
                    .chars()
                    .take(shodh_rag::workspaces::MAX_NAME_CHARS)
                    .collect(),
                folder: folder.folder.clone(),
            });
        }
        plan.assign.insert(c.id.clone(), space.to_string());
    }
    plan
}

/// Runs the import once: creates a workspace per legacy space that is still an indexed
/// folder and assigns its conversations. Idempotent and resumable (a partial run is
/// completed by the next call); marked done only after the conversations were saved.
pub async fn import_legacy_spaces(
    state: &WorkspaceState,
    data_dir: &Path,
    rag: &RwLock<RAGEngine>,
) -> WorkspaceCommandResult<usize> {
    if state.run(|s| s.state(LEGACY_IMPORT_KEY)).await?.is_some() {
        return Ok(0);
    }
    let conversations = ConversationStore::in_dir(data_dir)
        .load()
        .map_err(|message| WorkspaceCommandError {
            code: "storage",
            message,
        })?;
    let needs_import = conversations.iter().any(|c| {
        c.workspace_id.is_none() && c.space_id.as_deref().is_some_and(|s| !s.trim().is_empty())
    });
    if !needs_import {
        state
            .run(|s| s.set_state(LEGACY_IMPORT_KEY, "nothing to import"))
            .await?;
        return Ok(0);
    }
    let folders: Vec<KnownFolder> = {
        let engine = rag.read().await;
        load_sources(&engine)
            .await
            .map_err(|e| WorkspaceCommandError::unavailable(e.to_string()))?
            .into_iter()
            .filter(|s| s.kind == IndexKind::Folder)
            .filter_map(|s| {
                s.folder.map(|folder| KnownFolder {
                    source_id: s.source_id,
                    folder,
                })
            })
            .collect()
    };
    let existing: HashSet<String> = state
        .run(|s| s.list(true))
        .await?
        .into_iter()
        .map(|w| w.id)
        .collect();
    let plan = plan_legacy_import(&conversations, &folders, &existing);
    let create = plan.create.clone();
    state
        .run(move |s| {
            for space in &create {
                s.create_with_id(
                    &space.id,
                    &NewWorkspace {
                        name: space.name.clone(),
                        description: "Chats that were asked about this folder before \
                                      workspaces existed."
                            .to_string(),
                        ..NewWorkspace::default()
                    },
                    WorkspaceAuthor::Migration,
                )?;
                s.add_sources(
                    &space.id,
                    &[NewSource {
                        kind: SourceKind::Folder,
                        reference: space.id.clone(),
                        label: space.name.clone(),
                        path: Some(space.folder.clone()),
                    }],
                    WorkspaceAuthor::Migration,
                )?;
            }
            Ok(())
        })
        .await?;
    let assign = plan.assign.clone();
    let assigned = ConversationStore::in_dir(data_dir)
        .update(move |conversations| {
            let mut n = 0;
            for c in conversations.iter_mut() {
                if c.workspace_id.is_some() {
                    continue;
                }
                if let Some(workspace) = assign.get(&c.id) {
                    c.workspace_id = Some(workspace.clone());
                    n += 1;
                }
            }
            Ok(n)
        })
        .map_err(|message| WorkspaceCommandError {
            code: "storage",
            message,
        })?;
    let note = format!("{} workspaces, {assigned} conversations", plan.create.len());
    state
        .run(move |s| s.set_state(LEGACY_IMPORT_KEY, &note))
        .await?;
    tracing::info!(target: "shodh::workspaces", workspaces = plan.create.len(), conversations = assigned, "legacy spaces imported as workspaces");
    Ok(assigned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(
        id: &str,
        space: Option<(&str, &str)>,
        workspace: Option<&str>,
    ) -> ConversationRecord {
        ConversationRecord {
            id: id.into(),
            title: id.into(),
            messages: Vec::new(),
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: format!("2026-10-0{}T00:00:00Z", id.len()),
            pinned: false,
            space_id: space.map(|s| s.0.to_string()),
            space_name: space.map(|s| s.1.to_string()),
            workspace_id: workspace.map(str::to_string),
            system_prompt: None,
            focus_threads: None,
        }
    }

    fn known(id: &str, folder: &str) -> KnownFolder {
        KnownFolder {
            source_id: id.into(),
            folder: folder.into(),
        }
    }

    #[test]
    fn legacy_spaces_that_are_still_folders_become_workspaces() {
        let conversations = vec![
            conversation("a", Some(("src-1", "Contracts")), None),
            conversation("bb", Some(("src-1", "Contracts")), None),
            conversation("ccc", Some(("src-gone", "Old")), None),
            conversation("dddd", None, None),
            conversation("eeeee", Some(("src-2", "")), None),
            conversation("ffffff", Some(("src-1", "Contracts")), Some("ws-x")),
        ];
        let folders = vec![
            known("src-1", "C:/Docs/Contracts"),
            known("src-2", "C:/Docs/Tax 2026"),
        ];
        let plan = plan_legacy_import(&conversations, &folders, &HashSet::new());
        assert_eq!(
            plan.create,
            vec![
                LegacySpace {
                    id: "src-1".into(),
                    name: "Contracts".into(),
                    folder: "C:/Docs/Contracts".into()
                },
                LegacySpace {
                    id: "src-2".into(),
                    name: "Tax 2026".into(),
                    folder: "C:/Docs/Tax 2026".into()
                },
            ]
        );
        let assigned: Vec<(&str, &str)> = plan
            .assign
            .iter()
            .map(|(c, w)| (c.as_str(), w.as_str()))
            .collect();
        // A space that is no longer indexed, no space, or a workspace already chosen: kept.
        assert_eq!(
            assigned,
            vec![("a", "src-1"), ("bb", "src-1"), ("eeeee", "src-2")]
        );
        // A resumed import reuses workspaces an interrupted one created.
        let existing: HashSet<String> = ["src-1".to_string()].into();
        let resumed = plan_legacy_import(&conversations, &folders, &existing);
        assert_eq!(resumed.create.len(), 1);
        assert_eq!(resumed.assign.len(), 3);
    }

    #[test]
    fn chat_stats_count_chats_and_find_the_latest() {
        let conversations = vec![
            conversation("a", None, Some("ws-1")),
            conversation("bbb", None, Some("ws-1")),
            conversation("cc", None, Some("ws-2")),
            conversation("d", None, None),
        ];
        let stats = chat_stats(&conversations);
        assert_eq!(stats["ws-1"].chat_count, 2);
        assert_eq!(
            stats["ws-1"].last_chat_at.as_deref(),
            Some("2026-10-03T00:00:00Z")
        );
        assert_eq!(stats["ws-2"].chat_count, 1);
        assert_eq!(stats.len(), 2);
    }

    #[tokio::test]
    async fn the_import_runs_once_and_keeps_space_ids() {
        let dir = tempfile::tempdir().unwrap();
        let state = WorkspaceState::at(Some((dir.path().join("shodh.db"), None)));
        let store = ConversationStore::in_dir(dir.path());
        store
            .update(|all| {
                all.push(conversation("a", Some(("src-1", "Contracts")), None));
                all.push(conversation("bb", Some(("src-gone", "Old")), None));
                Ok(())
            })
            .unwrap();
        let mut config = shodh_rag::config::RAGConfig {
            data_dir: dir.path().join("index"),
            ..shodh_rag::config::RAGConfig::default()
        };
        config.embedding.model_dir = dir.path().join("no-models");
        let rag = RwLock::new(RAGEngine::new(config).await.unwrap());
        // No folder is indexed: nothing maps, but the import is complete.
        assert_eq!(
            import_legacy_spaces(&state, dir.path(), &rag)
                .await
                .unwrap(),
            0
        );
        let after = store.load().unwrap();
        assert!(after.iter().all(|c| c.workspace_id.is_none()));
        assert_eq!(after[0].space_id.as_deref(), Some("src-1"));
        assert!(state
            .run(|s| s.state(LEGACY_IMPORT_KEY))
            .await
            .unwrap()
            .is_some());
        // Done: a second run does nothing.
        assert_eq!(
            import_legacy_spaces(&state, dir.path(), &rag)
                .await
                .unwrap(),
            0
        );
    }

    #[test]
    fn answer_sources_split_folders_files_and_papers() {
        let source = |kind, reference: &str, path: Option<&str>| WorkspaceSource {
            kind,
            reference: reference.into(),
            label: reference.into(),
            path: path.map(str::to_string),
            added_by: WorkspaceAuthor::User,
            added_at: "t".into(),
        };
        let detail = WorkspaceDetail {
            workspace: Workspace {
                id: "ws-1".into(),
                name: "Thesis".into(),
                description: String::new(),
                icon: "folder".into(),
                color: "neutral".into(),
                template: "blank".into(),
                pinned: false,
                archived: false,
                created_at: "t".into(),
                updated_at: "t".into(),
                last_active_at: None,
                instructions_version: 1,
                source_counts: Default::default(),
            },
            instructions: "Be brief.".into(),
            sources: vec![
                source(SourceKind::Folder, "src-1", Some("C:/Docs")),
                source(SourceKind::File, "c:/x/a.pdf", Some("C:/x/a.pdf")),
                source(SourceKind::Paper, "paper:doi:1", Some("C:/x/a.pdf")),
                source(SourceKind::Paper, "paper:doi:2", None),
                source(SourceKind::Snippet, "snippet:1", Some("C:/y/b.pdf")),
            ],
        };
        let answer = answer_sources(&detail);
        assert_eq!(answer.source_ids, vec!["src-1"]);
        assert_eq!(answer.folders, vec!["C:/Docs"]);
        // A library paper is searched through its PDF; a reference-only paper is not.
        assert_eq!(answer.files, vec!["C:/x/a.pdf"]);
        assert_eq!(answer.instructions, "Be brief.");
        assert!(answer.snippets.is_empty());
    }
}
