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
mod history;
mod settings;
mod sources;
mod tauri_host;
mod web;

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
}

/// Everything an app tool can reach.
pub struct AgentHost {
    pub data_dir: PathBuf,
    pub rag: Arc<RwLock<RAGEngine>>,
    pub audit: Option<Arc<AuditLog>>,
    pub effects: Arc<dyn HostEffects>,
    /// HTTP client for web tools (SSRF-checked).
    pub web: SafeClient,
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
    sources::register(&mut registry, &host)?;
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

    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Recorder {
        pub calendar: Mutex<Vec<CalendarChange>>,
        pub indexing: Mutex<Vec<IndexJob>>,
        pub conversations: Mutex<Vec<ConversationChange>>,
        pub settings: Mutex<Vec<AppSettings>>,
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
    }

    pub struct TestHost {
        pub dir: tempfile::TempDir,
        pub host: Arc<AgentHost>,
        pub effects: Arc<Recorder>,
    }

    /// A host with an empty RAG engine (no search models) and an audit log
    /// in a fresh temporary directory.
    pub async fn host() -> TestHost {
        let dir = tempfile::tempdir().unwrap();
        let mut config = shodh_rag::config::RAGConfig::default();
        config.data_dir = dir.path().join("index");
        config.embedding.model_dir = dir.path().join("no-models");
        let rag = RAGEngine::new(config).await.unwrap();
        let audit = AuditLog::open(dir.path().join("shodh.db"), None).unwrap();
        let effects = Arc::new(Recorder::default());
        let host = Arc::new(AgentHost {
            data_dir: dir.path().to_path_buf(),
            rag: Arc::new(RwLock::new(rag)),
            audit: Some(Arc::new(audit)),
            effects: effects.clone(),
            web: SafeClient::system(),
        });
        TestHost { dir, host, effects }
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
