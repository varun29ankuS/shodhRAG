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

mod audit;
mod calendar;
mod export;
mod figures;
pub(crate) mod files;
mod graph;
mod history;
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

pub use files::{IndexedRoots, SourceRoot, SourceRoots};
pub use research::{list_directory_in, DirEntry, Listing};
pub use tauri_host::TauriEffects;
pub use web::web_block_reason;

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
    /// A gallery visual of `conversation_id` was revised or organised: refresh the gallery.
    fn visuals_changed(&self, conversation_id: &str);
    /// Snippets, results or the citation graph (`kind` `snippet`, `result` or `graph`) of
    /// `file_path` changed: refresh the Library and the gallery.
    fn research_changed(&self, kind: &str, file_path: &str);
    /// A workspace (its fields, instructions or sources) changed: refresh open views.
    fn workspaces_changed(&self, workspace_id: &str);
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
    registry.register(Arc::new(OpenViewTool))?;
    registry.register(Arc::new(ShowDocumentTool))?;
    registry.register(Arc::new(ShowAuditTool))?;
    registry.register(Arc::new(ShowSourceTool::new(rag)))?;
    calendar::register(&mut registry, &host)?;
    history::register(&mut registry, &host)?;
    audit::register(&mut registry, &host)?;
    settings::register(&mut registry, &host)?;
    web::register(&mut registry, &host, &host.web)?;
    export::register(&mut registry, &host)?;
    research::register(&mut registry, &host)?;
    sources::register(&mut registry, &host)?;
    memory::register(&mut registry, &host)?;
    visuals::register(&mut registry, &host)?;
    papers::register(&mut registry, &host)?;
    graph::register(&mut registry, &host)?;
    figures::register(&mut registry, &host)?;
    workspaces::register(&mut registry, &host)?;
    Ok(registry)
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
    }

    impl HostEffects for Recorder {
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
        });
        TestHost {
            dir,
            host,
            effects,
            roots,
            printer,
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

    struct FixedEmbedder(Arc<dyn shodh_rag::embeddings::EmbeddingModel>);

    #[async_trait::async_trait]
    impl shodh_rag::statements::EmbedderSource for FixedEmbedder {
        async fn embedder(
            &self,
        ) -> shodh_rag::statements::StatementResult<Arc<dyn shodh_rag::embeddings::EmbeddingModel>>
        {
            Ok(self.0.clone())
        }
    }

    /// A memory service over `dir` (its own LanceDB folder and `dir/shodh.db`).
    pub async fn memory_state(
        dir: &std::path::Path,
        audit: Option<Arc<AuditLog>>,
    ) -> crate::memory_commands::MemoryState {
        use shodh_rag::statements::{DynamicsStore, StatementStore, SystemClock};
        let dynamics = DynamicsStore::open(&dir.join("shodh.db"), None).unwrap();
        let store = StatementStore::open(
            &dir.join("memory_lance"),
            WORD_DIM,
            Arc::new(crate::memory_commands::memory_ontology().unwrap()),
            Arc::new(dynamics),
            Arc::new(FixedEmbedder(Arc::new(WordEmbedder::default()))),
            Arc::new(SystemClock),
        )
        .await
        .unwrap();
        crate::memory_commands::MemoryState::ready(Arc::new(
            shodh_rag::user_memory::MemoryService::new(Arc::new(store), audit, "test"),
        ))
    }

    /// A tool context whose events land in the returned receiver.
    pub fn ctx() -> (
        ToolContext,
        tokio::sync::mpsc::UnboundedReceiver<shodh_rag::harness::AgentEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (ToolContext::new("run-1", "step-1", tx), rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::harness::profile::AgentProfile;

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
}
