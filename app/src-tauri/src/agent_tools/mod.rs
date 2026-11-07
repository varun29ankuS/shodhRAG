//! App-layer host tools for the agent harness. They implement
//! `shodh_rag::harness::tools::HostTool`; the registry applies schema
//! validation, the profile allowlist, the approval gate and auditing.
//!
//! Tools never hold the Tauri `AppHandle`. They get an [`AgentHost`]: the app
//! data directory, the RAG engine, the audit log and a [`HostEffects`]
//! implementation for everything that must reach the running app (refreshing
//! views, re-indexing, background jobs). Production uses [`TauriEffects`];
//! tests use a recording implementation against temporary storage, so the
//! whole registry can be built and exercised without a window.

mod artifacts;
mod audit;
mod calendar;
mod code_intel;
mod code_review;
pub(crate) mod export;
mod figures;
pub(crate) mod files;
mod graph;
mod history;
mod indexing;
mod memory;
mod papers;
mod research;
mod settings;
mod sources;
mod tauri_host;
mod visuals;
mod web;
mod workspaces;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use shodh_rag::audit::AuditLog;
use shodh_rag::harness::tools::documents::OpenDocumentTool;
use shodh_rag::harness::tools::mode::SuggestModeTool;
use shodh_rag::harness::tools::navigate::{
    OpenViewTool, ShowAuditTool, ShowDocumentTool, ShowSourceTool,
};
use shodh_rag::harness::tools::plan::UpdatePlanTool;
use shodh_rag::harness::tools::search::{DefaultK, SearchDocumentsTool};
use shodh_rag::harness::tools::sources::ListSourcesTool;
use shodh_rag::harness::tools::{RegistryError, ToolContext, ToolError, ToolRegistry};
use shodh_rag::harness::web::SafeClient;
use shodh_rag::RAGEngine;
use tokio::sync::RwLock;

use crate::app_settings::{AppSettings, SettingsStore};
use crate::calendar_store::{CalendarEvent, TodoItem};
use crate::memory_commands::MemoryState;
use crate::research_commands::ResearchState;
use crate::visual_commands::VisualState;
use crate::workspace_commands::WorkspaceState;

pub use artifacts::{creator_tools, CREATOR_TOOL_NAMES};
pub use code_intel::{code_intel_tools, CODE_INTEL_TOOL_NAMES};
pub use code_review::{code_review_tools, CODE_REVIEW_TOOL_NAMES};
pub use files::{IndexedRoots, SourceRoots};
/// For the coverage test, which checks them against the frontend.
#[cfg(test)]
pub(crate) use settings::SETTINGS_SECTIONS;
pub use tauri_host::TauriEffects;
pub use web::web_block_reason;
#[cfg(test)]
pub(crate) use workspaces::TABS as WORKSPACE_TABS;

/// What the agent is deliberately never given a tool for, stated in the
/// system prompt (see `ToolRegistry::capability_manifest`) so the model says
/// it cannot instead of improvising or claiming success.
///
/// Why each is withheld:
/// - Secrets: a prompt-injected document could otherwise exfiltrate or
///   replace a key; keys never enter tool arguments, results or the audit log.
/// - Approvals: an agent that can approve its own actions, or relax what
///   needs approval, makes the approval gate meaningless.
/// - Audit policy: shortening retention or exporting/deleting the log would
///   let the agent erase the record of what it did.
/// - Local-only mode and web access: these decide where the user's data may
///   go; only the user may widen that.
/// - Model/provider switching (including stealth models): changes who
///   receives the user's documents and prompts.
/// - Running programs: no shell or code execution is exposed at all.
pub const AGENT_CANNOT_DO: &[&str] = &[
    "see, set or delete API keys or any other secret",
    "approve its own actions or change which actions need approval",
    "change audit settings (retention, export) or delete audit history",
    "turn Local-only mode or web access on or off",
    "switch the language model or provider, or allow stealth models",
    "run programs, shell commands or code on the user's computer",
    "delete, move or edit the user's existing files on disk",
];

/// A calendar record that was saved or removed.
#[derive(Debug, Clone)]
pub enum CalendarChange {
    TaskSaved(TodoItem),
    TaskRemoved(String),
    EventSaved(CalendarEvent),
    EventRemoved(String),
}

/// A saved conversation the agent renamed or pinned.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationChange {
    pub conversation_id: String,
    pub title: String,
    pub pinned: bool,
    pub updated_at: String,
}

/// The model a code review is sent to: its name and whether it is off this computer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTarget {
    pub model: String,
    pub cloud: bool,
}

/// The model that answers, as the user configured it. Never carries a key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ModelInfo {
    pub provider: String,
    pub model: Option<String>,
    pub cloud: bool,
}

/// A single file to index (a download) into an existing source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIndexJob {
    pub path: String,
    pub source_id: String,
}

/// A background indexing job started by a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexJob {
    /// `add` or `reindex` (recorded in the audit log).
    pub action: &'static str,
    pub folder: String,
    pub source_id: String,
}

/// What the tools need from the running app.
#[async_trait::async_trait]
pub trait HostEffects: Send + Sync {
    /// A calendar record changed: re-index it and refresh open views.
    fn calendar_changed(&self, change: CalendarChange);
    /// Index a folder in the background and audit the outcome in `ctx`'s
    /// run scope.
    fn start_indexing(&self, ctx: &ToolContext, job: IndexJob);
    /// A saved conversation was renamed or pinned: update the open list.
    fn conversation_changed(&self, change: ConversationChange);
    /// Settings changed: apply them in the UI.
    fn settings_changed(&self, settings: &AppSettings);
    /// The configured model, if any.
    fn model_info(&self) -> Option<ModelInfo>;
    /// The user's OpenRouter API key, if one is configured (used only for
    /// web search requests to OpenRouter; never logged or returned).
    fn openrouter_key(&self) -> Option<String>;
    /// The user's Documents folder.
    fn documents_dir(&self) -> Option<PathBuf>;
    /// Index one new file in the background (audited in `ctx`'s scope) and
    /// refresh the Library.
    fn index_file(&self, ctx: &ToolContext, job: FileIndexJob);
    /// Files or folders of a source changed on disk: refresh the Library.
    fn library_changed(&self, source_id: &str);
    /// Remove the source `source_id` as the Library removes one
    /// ([`crate::file_watcher::FolderSyncState::remove_source`]: its sync and
    /// index runs stopped, its chunks and manifest deleted) and tell the
    /// Library, which drops its card. Chunks without a source are removed
    /// only under the folder the Library lists for it
    /// ([`crate::file_watcher::FolderSyncState::library_path`]), never under
    /// a folder inferred from its files. Returns the chunks deleted.
    async fn remove_source(
        &self,
        rag: &RwLock<RAGEngine>,
        source_id: &str,
    ) -> Result<usize, String>;
    /// A gallery visual of `conversation_id` was revised or organised: refresh the gallery.
    fn visuals_changed(&self, conversation_id: &str);
    /// Snippets, results or the citation graph (`kind` `snippet`, `result` or `graph`) of
    /// `file_path` changed: refresh the Library and the gallery.
    fn research_changed(&self, kind: &str, file_path: &str);
    /// A workspace (its fields, instructions or sources) changed: refresh open views.
    fn workspaces_changed(&self, workspace_id: &str);
    /// Whether indexing is paused.
    fn indexing_paused(&self) -> bool;
    /// Pauses (`true`) or resumes every indexing run, saved as the status
    /// strip saves it.
    fn set_indexing_paused(&self, paused: bool);
    /// Cancels the indexing in progress of `source_id` (every run without
    /// one); returns the runs stopped.
    fn cancel_indexing(&self, source_id: Option<&str>) -> usize;
    /// The System status the status strip shows, as JSON.
    async fn system_status(&self) -> serde_json::Value;
    /// Runs the fresh-eyes reviewer on `request` (only what it holds reaches
    /// it) and keeps the review for the code pane
    /// ([`crate::code_review_commands::run_and_store`]).
    async fn run_code_review(
        &self,
        request: shodh_rag::code_review::ReviewRequest,
        workspace_id: Option<String>,
        conversation_id: Option<String>,
    ) -> Result<crate::code_review_commands::StoredReview, String>;
    /// The model a code review would be sent to now, if known.
    fn review_target(&self) -> Option<ReviewTarget>;
}

/// Everything an app tool can reach.
pub struct AgentHost {
    pub data_dir: PathBuf,
    pub rag: Arc<RwLock<RAGEngine>>,
    pub audit: Option<Arc<AuditLog>>,
    pub effects: Arc<dyn HostEffects>,
    /// HTTP client for web tools (SSRF-checked).
    pub web: SafeClient,
    /// The indexed source folders (where file-creating tools may write).
    pub roots: Arc<dyn SourceRoots>,
    /// Long-term memory (opened on first use).
    pub memory: MemoryState,
    /// Generated visuals (the gallery; opened on first use).
    pub visuals: VisualState,
    /// Snippets and Result statements (opened on first use).
    pub research: ResearchState,
    /// Prints documents to PDF (`export_document` with format `pdf`).
    pub pdf: Arc<dyn crate::pdf_export::PdfPrinter>,
    /// Workspaces (opened on first use).
    pub workspaces: WorkspaceState,
    /// The GitHub connector: its sign-in and the user's list, read afresh for each call.
    pub github: Arc<dyn shodh_rag::connectors::github::tools::GitHubHost>,
}

/// Build the registry with every agent tool. Fails if two tools share a
/// name or a schema does not compile.
pub fn build_registry(host: Arc<AgentHost>) -> Result<ToolRegistry, RegistryError> {
    let mut registry = ToolRegistry::new();
    let rag = host.rag.clone();
    let settings = SettingsStore::in_dir(&host.data_dir);
    let default_k: DefaultK = Arc::new(move || {
        let preferred = match settings.load() {
            Ok(s) => s.preferences.search_max_results,
            Err(e) => {
                tracing::warn!(target: "shodh::harness", error = %e, "settings unreadable; default passage count used");
                crate::app_settings::Preferences::default().search_max_results
            }
        };
        usize::try_from(preferred).unwrap_or(usize::MAX)
    });
    registry.register(Arc::new(
        SearchDocumentsTool::new(rag.clone()).with_default_k(default_k),
    ))?;
    registry.register(Arc::new(OpenDocumentTool::new(rag.clone())))?;
    registry.register(Arc::new(ListSourcesTool::new(rag.clone())))?;
    registry.register(Arc::new(UpdatePlanTool))?;
    registry.register(Arc::new(SuggestModeTool))?;
    registry.register(Arc::new(OpenViewTool))?;
    registry.register(Arc::new(ShowDocumentTool))?;
    registry.register(Arc::new(ShowAuditTool::new(host.audit.clone())))?;
    registry.register(Arc::new(ShowSourceTool::new(rag)))?;
    calendar::register(&mut registry, &host)?;
    history::register(&mut registry, &host)?;
    audit::register(&mut registry, &host)?;
    settings::register(&mut registry, &host)?;
    web::register(&mut registry, &host, &host.web)?;
    export::register(&mut registry, &host)?;
    research::register(&mut registry, &host)?;
    sources::register(&mut registry, &host)?;
    indexing::register(&mut registry, &host)?;
    memory::register(&mut registry, &host)?;
    visuals::register(&mut registry, &host)?;
    papers::register(&mut registry, &host)?;
    graph::register(&mut registry, &host)?;
    figures::register(&mut registry, &host)?;
    workspaces::register(&mut registry, &host)?;
    artifacts::register(&mut registry, &host)?;
    code_review::register(&mut registry, &host)?;
    code_intel::register(&mut registry, &host)?;
    shodh_rag::connectors::github::tools::register(&mut registry, host.github.clone())?;
    Ok(registry)
}

/// The parts of the Library a navigation opens (`LIBRARY_SECTIONS` in the frontend's
/// navigation; the coverage test checks the two agree).
pub(crate) const LIBRARY_SECTIONS: [&str; 3] = ["snippets", "graph", "paper"];

/// Show a part of the Library ([`LIBRARY_SECTIONS`]) with `item` (a snippet or paper
/// id) highlighted or opened.
pub(crate) fn show_in_library(
    ctx: &ToolContext,
    section: &'static str,
    item: Option<String>,
    label: &str,
) {
    debug_assert!(LIBRARY_SECTIONS.contains(&section), "{section}");
    shodh_rag::harness::tools::navigate::emit_shown(
        ctx,
        "library",
        item.clone(),
        Some(shodh_rag::harness::events::NavigationTarget::Library {
            section: section.to_string(),
            item,
        }),
        Some(label),
    );
}

pub(crate) fn invalid(tool: &str, reasons: impl Into<String>) -> ToolError {
    ToolError::InvalidArguments {
        tool: tool.to_string(),
        reasons: reasons.into(),
    }
}

pub(crate) fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// `limit` argument clamped to `1..=max`, `default` when absent.
pub(crate) fn limit_arg(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(default)
        .clamp(1, max)
}

#[cfg(test)]
pub(crate) mod testing {
    //! A host over temporary storage that records every effect.

    use super::files::SourceRoot;
    use super::*;
    use std::sync::Mutex;

    pub struct Recorder {
        pub documents: PathBuf,
        pub calendar: Mutex<Vec<CalendarChange>>,
        pub indexing: Mutex<Vec<IndexJob>>,
        pub conversations: Mutex<Vec<ConversationChange>>,
        pub settings: Mutex<Vec<AppSettings>>,
        pub indexed_files: Mutex<Vec<FileIndexJob>>,
        pub library: Mutex<Vec<String>>,
        pub visuals: Mutex<Vec<String>>,
        pub research: Mutex<Vec<(String, String)>>,
        pub workspaces: Mutex<Vec<String>>,
        /// The Library's folder sync state, which `remove_source` acts on.
        pub folder_sync: crate::file_watcher::FolderSyncState,
        pub indexing_state: shodh_rag::indexing::IndexingState,
        /// Sources removed through `remove_source`, as the Library is told.
        pub removed_sources: Mutex<Vec<String>>,
        /// Every review asked for: exactly what the reviewer would get.
        pub reviews: Mutex<Vec<shodh_rag::code_review::ReviewRequest>>,
        /// The model a review would be sent to (none: unknown).
        pub review_target: Mutex<Option<ReviewTarget>>,
    }

    #[async_trait::async_trait]
    impl HostEffects for Recorder {
        fn review_target(&self) -> Option<ReviewTarget> {
            self.review_target
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        async fn run_code_review(
            &self,
            request: shodh_rag::code_review::ReviewRequest,
            workspace_id: Option<String>,
            conversation_id: Option<String>,
        ) -> Result<crate::code_review_commands::StoredReview, String> {
            self.reviews
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.clone());
            Ok(crate::code_review_commands::StoredReview {
                id: "rv_test".to_string(),
                created_at: "2026-11-06T00:00:00Z".to_string(),
                workspace_id,
                conversation_id,
                by_agent: true,
                report: shodh_rag::code_review::ReviewReport {
                    check: request.check,
                    summary: "No problems found.".to_string(),
                    findings: Vec::new(),
                    diagram: None,
                    dropped: 0,
                },
                request,
            })
        }
        fn calendar_changed(&self, change: CalendarChange) {
            self.calendar
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(change);
        }
        fn start_indexing(&self, _ctx: &ToolContext, job: IndexJob) {
            self.indexing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(job);
        }
        fn conversation_changed(&self, change: ConversationChange) {
            self.conversations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(change);
        }
        fn settings_changed(&self, settings: &AppSettings) {
            self.settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(settings.clone());
        }
        fn model_info(&self) -> Option<ModelInfo> {
            None
        }
        fn openrouter_key(&self) -> Option<String> {
            None
        }
        fn documents_dir(&self) -> Option<PathBuf> {
            Some(self.documents.clone())
        }
        fn index_file(&self, _ctx: &ToolContext, job: FileIndexJob) {
            self.indexed_files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(job);
        }
        fn library_changed(&self, source_id: &str) {
            self.library
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(source_id.to_string());
        }
        fn visuals_changed(&self, conversation_id: &str) {
            self.visuals
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(conversation_id.to_string());
        }
        fn research_changed(&self, kind: &str, file_path: &str) {
            self.research
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((kind.to_string(), file_path.to_string()));
        }
        fn workspaces_changed(&self, workspace_id: &str) {
            self.workspaces
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(workspace_id.to_string());
        }
        fn indexing_paused(&self) -> bool {
            self.indexing_state.is_paused()
        }
        fn set_indexing_paused(&self, paused: bool) {
            self.indexing_state.pause_switch().set(paused);
        }
        fn cancel_indexing(&self, source_id: Option<&str>) -> usize {
            match source_id {
                Some(id) => self.indexing_state.cancel_source(id),
                None => self.indexing_state.cancel(),
            }
        }
        async fn system_status(&self) -> serde_json::Value {
            serde_json::json!({
                "indexingPaused": self.indexing_state.is_paused(),
                "running": self.indexing_state.running_paths(),
            })
        }
        async fn remove_source(
            &self,
            rag: &RwLock<RAGEngine>,
            source_id: &str,
        ) -> Result<usize, String> {
            let path = self.folder_sync.library_path(source_id);
            let deleted = self
                .folder_sync
                .remove_source(rag, &self.indexing_state, source_id, path.as_deref())
                .await?;
            self.removed_sources
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(source_id.to_string());
            Ok(deleted)
        }
    }

    pub struct TestHost {
        pub dir: tempfile::TempDir,
        pub host: Arc<AgentHost>,
        pub effects: Arc<Recorder>,
        pub roots: Arc<FixedRoots>,
        pub printer: Arc<crate::pdf_export::testing::FakePrinter>,
    }

    /// A host with an empty RAG engine (no search models) and an audit log
    /// in a fresh temporary directory.
    pub async fn host() -> TestHost {
        let dir = tempfile::tempdir().unwrap();
        let mut config = shodh_rag::config::RAGConfig {
            data_dir: dir.path().join("index"),
            ..shodh_rag::config::RAGConfig::default()
        };
        config.embedding.model_dir = dir.path().join("no-models");
        let rag = RAGEngine::new(config).await.unwrap();
        let audit = Arc::new(AuditLog::open(dir.path().join("shodh.db"), None).unwrap());
        let memory = memory_state(dir.path(), Some(audit.clone())).await;
        let effects = Arc::new(Recorder {
            documents: dir.path().join("Documents"),
            calendar: Mutex::default(),
            indexing: Mutex::default(),
            conversations: Mutex::default(),
            settings: Mutex::default(),
            indexed_files: Mutex::default(),
            library: Mutex::default(),
            visuals: Mutex::default(),
            research: Mutex::default(),
            workspaces: Mutex::default(),
            folder_sync: crate::file_watcher::FolderSyncState::new(dir.path().join("folder_sync")),
            indexing_state: shodh_rag::indexing::IndexingState::default(),
            removed_sources: Mutex::default(),
            reviews: Mutex::default(),
            review_target: Mutex::default(),
        });
        std::fs::create_dir_all(&effects.documents).unwrap();
        let roots = Arc::new(FixedRoots::default());
        let printer = Arc::new(crate::pdf_export::testing::FakePrinter::writing());
        let host = Arc::new(AgentHost {
            data_dir: dir.path().to_path_buf(),
            rag: Arc::new(RwLock::new(rag)),
            audit: Some(audit),
            effects: effects.clone(),
            web: SafeClient::system(),
            roots: roots.clone(),
            research: ResearchState::at(
                memory.clone(),
                Some((dir.path().join("shodh.db"), None)),
                shodh_rag::processing::table_model::shared_table_model(),
                Default::default(),
            ),
            memory,
            visuals: VisualState::at(Some((dir.path().join("shodh.db"), None))),
            pdf: printer.clone(),
            workspaces: WorkspaceState::at(Some((dir.path().join("shodh.db"), None))),
            github: Arc::new(NoGitHub),
        });
        TestHost {
            dir,
            host,
            effects,
            roots,
            printer,
        }
    }

    /// A GitHub connector nobody signed in to (the tests never reach GitHub).
    pub struct NoGitHub;

    #[async_trait::async_trait]
    impl shodh_rag::connectors::github::tools::GitHubHost for NoGitHub {
        async fn session(&self) -> Result<shodh_rag::connectors::github::tools::Session, String> {
            Err("Shodh is not signed in to GitHub.".to_string())
        }
        async fn grant(
            &self,
            _repo: &shodh_rag::connectors::github::remote::RepoRef,
            _write: bool,
        ) -> Result<(), String> {
            Err("Shodh is not signed in to GitHub.".to_string())
        }
        fn notify(&self, _notice: shodh_rag::connectors::github::tools::GitHubNotice) {}
        async fn activity(
            &self,
            _limit: usize,
        ) -> Vec<shodh_rag::connectors::github::tools::ActivityItem> {
            Vec::new()
        }
    }

    /// Indexed source folders for tests.
    #[derive(Default)]
    pub struct FixedRoots {
        pub roots: Mutex<Vec<SourceRoot>>,
    }

    #[async_trait::async_trait]
    impl SourceRoots for FixedRoots {
        async fn roots(&self) -> Result<Vec<SourceRoot>, ToolError> {
            Ok(self.roots.lock().unwrap_or_else(|e| e.into_inner()).clone())
        }
    }

    /// Treat `folder` as an indexed source folder; returns its source id.
    pub async fn index_folder(t: &TestHost, folder: &std::path::Path) -> String {
        let mut roots = t.roots.roots.lock().unwrap_or_else(|e| e.into_inner());
        let source_id = format!("source-{}", roots.len() + 1);
        roots.push(SourceRoot {
            source_id: source_id.clone(),
            folder: std::fs::canonicalize(folder)
                .map(|p| super::sources::strip_verbatim(&p))
                .unwrap_or_else(|_| folder.to_path_buf()),
        });
        source_id
    }

    /// An engine over `dir` that can index: the deterministic [`WordEmbedder`]
    /// stands in for the search models.
    pub async fn engine_with_search_models(dir: &std::path::Path) -> RAGEngine {
        let mut config = shodh_rag::config::RAGConfig {
            data_dir: dir.join("index"),
            ..shodh_rag::config::RAGConfig::default()
        };
        config.embedding.model_dir = dir.join("no-models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = WORD_DIM;
        config.search.min_score_threshold = 0.0;
        let mut rag = RAGEngine::new(config).await.unwrap();
        rag.attach_search_models(shodh_rag::SearchModels::from_embedder(Arc::new(
            WordEmbedder::default(),
        )))
        .unwrap();
        rag
    }

    /// The test host over an engine that can index ([`engine_with_search_models`]).
    pub async fn host_with_search_models() -> TestHost {
        let t = host().await;
        let rag = engine_with_search_models(&t.dir.path().join("searchable")).await;
        let host = Arc::new(AgentHost {
            data_dir: t.host.data_dir.clone(),
            rag: Arc::new(RwLock::new(rag)),
            audit: t.host.audit.clone(),
            effects: t.host.effects.clone(),
            web: t.host.web.clone(),
            roots: t.host.roots.clone(),
            memory: t.host.memory.clone(),
            visuals: t.host.visuals.clone(),
            research: t.host.research.clone(),
            pdf: t.host.pdf.clone(),
            workspaces: t.host.workspaces.clone(),
            github: t.host.github.clone(),
        });
        TestHost { host, ..t }
    }

    /// The test host with another web client (in-process DNS and HTTP).
    pub fn with_web(t: &TestHost, web: SafeClient) -> Arc<AgentHost> {
        Arc::new(AgentHost {
            data_dir: t.host.data_dir.clone(),
            rag: t.host.rag.clone(),
            audit: t.host.audit.clone(),
            effects: t.host.effects.clone(),
            web,
            roots: t.host.roots.clone(),
            memory: t.host.memory.clone(),
            visuals: t.host.visuals.clone(),
            research: t.host.research.clone(),
            pdf: t.host.pdf.clone(),
            workspaces: t.host.workspaces.clone(),
            github: t.host.github.clone(),
        })
    }

    /// The test host with another PDF printer.
    pub fn with_printer(
        t: &TestHost,
        pdf: Arc<dyn crate::pdf_export::PdfPrinter>,
    ) -> Arc<AgentHost> {
        Arc::new(AgentHost {
            data_dir: t.host.data_dir.clone(),
            rag: t.host.rag.clone(),
            audit: t.host.audit.clone(),
            effects: t.host.effects.clone(),
            web: t.host.web.clone(),
            roots: t.host.roots.clone(),
            memory: t.host.memory.clone(),
            visuals: t.host.visuals.clone(),
            research: t.host.research.clone(),
            pdf,
            workspaces: t.host.workspaces.clone(),
            github: t.host.github.clone(),
        })
    }

    /// Gives each distinct lower-cased word its own dimension: texts sharing words are
    /// similar, texts sharing none are orthogonal. Deterministic, no model files.
    #[derive(Default)]
    pub struct WordEmbedder {
        vocabulary: Mutex<std::collections::HashMap<String, usize>>,
    }

    pub const WORD_DIM: usize = 256;

    impl WordEmbedder {
        fn embed(&self, text: &str) -> Vec<f32> {
            let mut v = vec![0.0f32; WORD_DIM];
            let mut vocabulary = self.vocabulary.lock().unwrap_or_else(|e| e.into_inner());
            for word in text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() > 1)
            {
                let next = vocabulary.len() % WORD_DIM;
                let index = *vocabulary.entry(word.to_lowercase()).or_insert(next);
                v[index] += 1.0;
            }
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm == 0.0 {
                v[WORD_DIM - 1] = 1.0;
            } else {
                v.iter_mut().for_each(|x| *x /= norm);
            }
            v
        }
    }

    impl shodh_rag::embeddings::EmbeddingModel for WordEmbedder {
        fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(self.embed(text))
        }
        fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(self.embed(text))
        }
        fn dimension(&self) -> usize {
            WORD_DIM
        }
    }

    /// How the embedder source of a test memory behaves (it can change during a test, so a
    /// memory written with search models can be read as if they were missing, slow or
    /// failing).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Embedder {
        /// Works.
        Ready,
        /// The search models are not installed.
        Missing,
        /// The engine is held (indexing): the embedder never arrives.
        Held,
        /// Arrives, but embedding a query fails.
        Failing,
    }

    /// Switches the embedder of a test memory.
    #[derive(Clone)]
    pub struct EmbedderSwitch(Arc<Mutex<Embedder>>);

    impl EmbedderSwitch {
        pub fn set(&self, mode: Embedder) {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = mode;
        }
        fn get(&self) -> Embedder {
            *self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    /// Delegates to `inner`, except that a query cannot be embedded while `switch` is
    /// [`Embedder::Failing`].
    struct SwitchedModel {
        inner: Arc<dyn shodh_rag::embeddings::EmbeddingModel>,
        switch: EmbedderSwitch,
    }

    impl shodh_rag::embeddings::EmbeddingModel for SwitchedModel {
        fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            if self.switch.get() == Embedder::Failing {
                anyhow::bail!("the embedding model failed");
            }
            self.inner.embed_query(text)
        }
        fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
            self.inner.embed_document(text)
        }
        fn dimension(&self) -> usize {
            self.inner.dimension()
        }
    }

    struct SwitchedEmbedder {
        model: Arc<SwitchedModel>,
    }

    #[async_trait::async_trait]
    impl shodh_rag::statements::EmbedderSource for SwitchedEmbedder {
        async fn embedder(
            &self,
        ) -> shodh_rag::statements::StatementResult<Arc<dyn shodh_rag::embeddings::EmbeddingModel>>
        {
            match self.model.switch.get() {
                Embedder::Missing => {
                    Err(shodh_rag::statements::StatementError::EmbeddingUnavailable(
                        "search models are not installed".into(),
                    ))
                }
                Embedder::Held => std::future::pending().await,
                Embedder::Ready | Embedder::Failing => Ok(self.model.clone()),
            }
        }
    }

    /// A memory service over `dir` (its own LanceDB folder and `dir/shodh.db`), with its
    /// suggestions inbox in `dir/shodh.db`, as the app opens it (`MemoryState::service`).
    pub async fn memory_state(
        dir: &std::path::Path,
        audit: Option<Arc<AuditLog>>,
    ) -> crate::memory_commands::MemoryState {
        memory_state_switched(dir, audit).await.0
    }

    /// [`memory_state`] and the switch of its embedder.
    pub async fn memory_state_switched(
        dir: &std::path::Path,
        audit: Option<Arc<AuditLog>>,
    ) -> (crate::memory_commands::MemoryState, EmbedderSwitch) {
        let switch = EmbedderSwitch(Arc::new(Mutex::new(Embedder::Ready)));
        let model = Arc::new(SwitchedModel {
            inner: Arc::new(WordEmbedder::default()),
            switch: switch.clone(),
        });
        use shodh_rag::statements::{DynamicsStore, StatementStore, SystemClock};
        let dynamics = DynamicsStore::open(&dir.join("shodh.db"), None).unwrap();
        let inbox =
            shodh_rag::user_memory::learn::Inbox::open(&dir.join("shodh.db"), None).unwrap();
        let store = StatementStore::open(
            &dir.join("memory_lance"),
            WORD_DIM,
            Arc::new(crate::memory_commands::memory_ontology().unwrap()),
            Arc::new(dynamics),
            Arc::new(SwitchedEmbedder { model }),
            Arc::new(SystemClock),
        )
        .await
        .unwrap();
        let state = crate::memory_commands::MemoryState::ready(Arc::new(
            shodh_rag::user_memory::MemoryService::new(Arc::new(store), audit, "test")
                .with_inbox(Arc::new(inbox)),
        ));
        (state, switch)
    }

    /// A tool context whose events land in the returned receiver.
    pub fn ctx() -> (
        ToolContext,
        tokio::sync::mpsc::UnboundedReceiver<shodh_rag::harness::AgentEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (ToolContext::new("run-1", "step-1", tx), rx)
    }

    /// A `navigated` event a tool sent, as the tests see it.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Shown {
        pub view: String,
        pub focus: Option<String>,
        pub target: Option<shodh_rag::harness::events::NavigationTarget>,
        pub label: Option<String>,
    }

    /// The navigation map of `agent-coverage.json` (`navigation`).
    fn navigation_map() -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("agent-coverage.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        manifest["navigation"].clone()
    }

    /// The next `navigated` event among `rx`'s events, sent by `tool`. It must be on the
    /// navigation map: `tool` listed under its target's kind (or, without a target,
    /// under its view), so a test of a tool that navigates without being on the map
    /// fails.
    pub fn expect_navigation(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<shodh_rag::harness::AgentEvent>,
        tool: &str,
    ) -> Shown {
        loop {
            match rx.try_recv() {
                Ok(shodh_rag::harness::AgentEvent::Navigated {
                    view,
                    focus,
                    target,
                    label,
                    ..
                }) => {
                    let map = navigation_map();
                    let (section, key) = match &target {
                        Some(t) => ("targets", t.kind().to_string()),
                        None => ("views", view.clone()),
                    };
                    let listed = map[section][&key]
                        .as_array()
                        .is_some_and(|tools| tools.iter().any(|t| t == tool));
                    assert!(
                        listed,
                        "{tool} sent a {key} navigation but is not listed under navigation.{section}.{key} in agent-coverage.json"
                    );
                    return Shown {
                        view,
                        focus,
                        target,
                        label,
                    };
                }
                Ok(_) => continue,
                Err(e) => panic!("{tool} sent no navigated event ({e:?})"),
            }
        }
    }

    /// Whether `rx` holds no further `navigated` event.
    pub fn no_navigation(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<shodh_rag::harness::AgentEvent>,
    ) -> bool {
        while let Ok(event) = rx.try_recv() {
            if matches!(event, shodh_rag::harness::AgentEvent::Navigated { .. }) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::harness::profile::{app_tools, AgentProfile};

    #[tokio::test]
    async fn every_allowed_tool_is_registered_and_every_registered_tool_allowed() {
        let t = testing::host().await;
        let registry = build_registry(t.host.clone()).unwrap();
        let profile = AgentProfile::assistant();
        let mut registered: Vec<&str> = registry.names();
        registered.sort_unstable();
        let mut allowed: Vec<&str> = profile.allowed_tools.iter().map(String::as_str).collect();
        allowed.sort_unstable();
        assert_eq!(registered, allowed);
    }

    /// The tools the modes that accept edits run without asking: only changes that stay
    /// on this computer, in the user's own data or workspace, and can be taken back.
    /// What leaves the computer (downloads, GitHub), what the user approves by design
    /// (memories) and what changes the library or the app (folders to index, settings)
    /// never opts in.
    #[tokio::test]
    async fn only_local_edits_run_without_asking_in_accept_edits_and_auto() {
        use shodh_rag::harness::modes::{decide, AgentMode, Gate, ToolClass};
        use shodh_rag::harness::RiskTier;
        let t = testing::host().await;
        let registry = build_registry(t.host.clone()).unwrap();
        let mut local: Vec<&str> = registry
            .names()
            .into_iter()
            .filter(|name| registry.tool(name).is_some_and(|tool| tool.edits_locally()))
            .collect();
        local.sort_unstable();
        let mut expected = vec![
            app_tools::ORGANIZE_CONVERSATION,
            app_tools::CREATE_FOLDER,
            app_tools::EXPORT_DOCUMENT,
        ];
        expected.sort_unstable();
        assert_eq!(local, expected);
        for name in [
            app_tools::DOWNLOAD_FILE,
            app_tools::REMEMBER,
            app_tools::UPDATE_MEMORY,
            app_tools::FORGET,
            app_tools::UPDATE_SETTING,
            app_tools::ADD_FOLDER,
            app_tools::REMOVE_SOURCE,
            app_tools::DELETE_TASK,
            app_tools::DELETE_EVENT,
            // Workspace instructions steer later answers: changing them always asks.
            app_tools::CREATE_WORKSPACE,
            app_tools::UPDATE_WORKSPACE,
            // A workspace's sources decide what Slack and Telegram answers draw on.
            app_tools::ADD_TO_WORKSPACE,
            app_tools::REMOVE_FROM_WORKSPACE,
            // Tasks, events and snippets are indexed and read by later answers (and
            // remind the user): the model's words there would steer them.
            app_tools::CREATE_TASK,
            app_tools::UPDATE_TASK,
            app_tools::COMPLETE_TASK,
            app_tools::CREATE_EVENT,
            app_tools::UPDATE_EVENT,
            app_tools::CREATE_SNIPPET,
        ] {
            assert!(!local.contains(&name), "{name} must ask in every mode");
        }
        // Every tool, in every mode: a write that is not a local edit and every
        // destructive tool ask (or are hidden in Plan mode); nothing hidden in Plan
        // mode is anything but a change.
        for name in registry.names() {
            let tool = registry.tool(name).unwrap();
            let class = ToolClass::of(
                tool.tier(),
                tool.edits_locally(),
                tool.changes_files_unasked() || tool.changes_state(),
            );
            for mode in AgentMode::ALL {
                let gate = decide(mode, class);
                match tool.tier() {
                    RiskTier::Read if tool.changes_state() && mode == AgentMode::Plan => {
                        assert_eq!(gate, Gate::Hidden, "{name} {mode:?}")
                    }
                    RiskTier::Read => assert_eq!(gate, Gate::Run, "{name} {mode:?}"),
                    // Creator mode runs unasked only inside its creation folder: a
                    // host tool's local edit lands outside it, so it asks.
                    RiskTier::Write if tool.edits_locally() && mode.creates() => {
                        assert_eq!(gate, Gate::Ask, "{name} {mode:?}")
                    }
                    RiskTier::Write if tool.edits_locally() && mode != AgentMode::Ask => {
                        assert_ne!(gate, Gate::Ask, "{name} {mode:?}")
                    }
                    RiskTier::Write | RiskTier::Destructive => {
                        assert_ne!(gate, Gate::Run, "{name} {mode:?}")
                    }
                }
                assert_eq!(
                    registry.offers(
                        name,
                        &AgentProfile {
                            mode,
                            ..AgentProfile::assistant()
                        }
                    ),
                    gate != Gate::Hidden,
                    "{name} {mode:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod no_paths_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::Duration;

    /// Whether `text` holds the folders of `dir` (either separator, any case).
    fn holds_folder(text: &str, dir: &std::path::Path) -> bool {
        let text = text.to_lowercase().replace('\\', "/");
        let dir = dir.display().to_string().to_lowercase().replace('\\', "/");
        let dir = dir.trim_start_matches("//?/");
        text.contains(dir) || text.contains("alice-private")
    }

    /// Every tool the assistant has, called with no arguments and with the arguments a
    /// conversation about an indexed file uses, never puts the user's folders into what
    /// the model reads: its output text or its error. A path holds the user's name and
    /// folders, which an answer could quote.
    #[tokio::test]
    async fn no_model_visible_tool_output_holds_a_local_path() {
        let t = testing::host_with_search_models().await;
        let folder = t
            .dir
            .path()
            .join("Users")
            .join("alice-private")
            .join("Papers");
        std::fs::create_dir_all(folder.join("Drafts")).unwrap();
        let file = folder.join("delta notes.md");
        std::fs::write(
            &file,
            "# Delta\n\nThe chunkwise form is faster than the recurrent form on long sequences.\n",
        )
        .unwrap();
        let source_id = testing::index_folder(&t, &folder).await;
        t.host
            .rag
            .write()
            .await
            .add_document_from_file(
                &file,
                HashMap::from([("space_id".to_string(), source_id.clone())]),
            )
            .await
            .unwrap();
        let registry = build_registry(t.host.clone()).unwrap();
        let (ctx, _rx) = testing::ctx();

        // The calls a conversation about the file makes, in order (the search numbers the
        // passage the later calls name), then every tool with no arguments.
        let mut calls: Vec<(String, Value)> = vec![
            (
                "search_documents".into(),
                json!({"query": "chunkwise form"}),
            ),
            ("open_document".into(), json!({"passage": 1})),
            ("open_document".into(), json!({"path": "delta notes.md"})),
            ("open_document".into(), json!({"path": "missing.md"})),
            ("show_document".into(), json!({"number": 1})),
            ("show_document".into(), json!({"path": "delta notes.md"})),
            ("show_document".into(), json!({"path": "other.pdf"})),
            ("list_sources".into(), json!({})),
            ("list_sources".into(), json!({"source_id": source_id})),
            ("list_sources".into(), json!({"source_id": "nope"})),
            ("show_source".into(), json!({"source_id": source_id})),
            (
                "search_documents".into(),
                json!({"query": "chunkwise", "sources": ["nothing-here"]}),
            ),
            (
                "list_directory".into(),
                json!({"path": "papers", "recursive": true}),
            ),
            (
                "list_directory".into(),
                json!({"path": folder.display().to_string()}),
            ),
            (
                "create_folder".into(),
                json!({"parent": "papers", "name": "Notes"}),
            ),
            (
                "create_folder".into(),
                json!({"parent": "papers", "name": "Notes"}),
            ),
            ("reindex_source".into(), json!({"source_id": source_id})),
        ];
        for name in registry.names() {
            calls.push((name.to_string(), json!({})));
        }

        let mut leaks = Vec::new();
        let mut ran = 0;
        for (name, args) in calls {
            let Some(tool) = registry.tool(&name) else {
                continue;
            };
            let outcome =
                tokio::time::timeout(Duration::from_secs(20), tool.execute(args.clone(), &ctx))
                    .await;
            let text = match outcome {
                Ok(Ok(out)) => out.text_for_model,
                Ok(Err(e)) => e.to_string(),
                // A call that does not finish shows the model nothing.
                Err(_) => continue,
            };
            ran += 1;
            if holds_folder(&text, t.dir.path()) {
                leaks.push(format!("{name} {args}: {text}"));
            }
        }
        assert!(ran > 20, "the sweep ran only {ran} calls");
        assert!(
            leaks.is_empty(),
            "model-visible text with a local path:\n{}",
            leaks.join("\n\n")
        );
    }
}
