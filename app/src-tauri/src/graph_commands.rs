//! The citation graph's commands: build it (Library → Graph), read its status, the graph
//! view, a paper page, method and dataset pages, and paper filters.
//!
//! Building scans the indexed PDFs and, only when the user's privacy policy allows the web
//! (Local-only mode off and web access on, read fail-closed) and the user asked for it,
//! looks papers up on OpenAlex. Otherwise the build reads only OpenAlex answers already
//! cached on this computer and sends nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shodh_rag::harness::web::SafeClient;
use shodh_rag::research::citations::graph::GraphSize;
use shodh_rag::research::citations::views::{
    concept_view, find, graph_view, paper_view, ConceptView, GraphView, PaperView,
};
use shodh_rag::research::citations::{
    BuildProgress, BuildReport, PaperFilter, PaperNode, Resolver,
};
use shodh_rag::research::results::ResultRecord;
use shodh_rag::research::snippets::{Snippet, SnippetQuery};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::agent_tools::web_block_reason;
use crate::research_commands::{
    broadcast_change, indexed_pdfs, ResearchCommandError, ResearchCommandResult, ResearchState,
};

/// Emitted with a [`BuildProgress`] while the graph builds.
pub const GRAPH_PROGRESS_EVENT: &str = "citation-graph-progress";
/// Most papers a filter returns.
const MAX_FOUND: usize = 500;

/// Whether a build is running (one at a time).
static BUILDING: AtomicBool = AtomicBool::new(false);

struct BuildingGuard;

impl BuildingGuard {
    fn acquire() -> Option<Self> {
        BUILDING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| BuildingGuard)
    }
}

impl Drop for BuildingGuard {
    fn drop(&mut self) {
        BUILDING.store(false, Ordering::SeqCst);
    }
}

fn data_dir(app: &AppHandle) -> ResearchCommandResult<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| ResearchCommandError::unavailable(e.to_string()))
}

/// The graph's state for the Library.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphStatus {
    pub report: Option<BuildReport>,
    pub size: GraphSize,
    /// Whether a build may look papers up online now.
    pub online_allowed: bool,
    /// Why it may not, when it may not.
    pub online_blocked_reason: Option<String>,
    pub building: bool,
}

/// Loads the graph of an earlier session in the background, so search can rank with it
/// from the start. Silent when the services cannot open yet (no search models).
pub fn warm(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let state = app.state::<ResearchState>();
        let Ok(services) = state.services().await else {
            return;
        };
        if let Err(e) = services.citations.graph(&services.results).await {
            tracing::warn!(target: "shodh::research", error = %e, "citation graph not loaded");
        }
    });
}

#[tauri::command]
pub async fn paper_graph_status(
    app: AppHandle,
    state: State<'_, ResearchState>,
) -> ResearchCommandResult<GraphStatus> {
    let services = state.services().await?;
    let graph = services.citations.graph(&services.results).await?;
    let blocked = web_block_reason(&data_dir(&app)?);
    Ok(GraphStatus {
        report: services.citations.report().await?,
        size: graph.size(),
        online_allowed: blocked.is_none(),
        online_blocked_reason: blocked,
        building: BUILDING.load(Ordering::SeqCst),
    })
}

/// Builds the graph of the indexed PDFs. With `online`, papers are looked up on OpenAlex
/// when the privacy policy allows the web; otherwise only cached answers are used.
#[tauri::command]
pub async fn paper_graph_build(
    app: AppHandle,
    state: State<'_, ResearchState>,
    rag: State<'_, crate::rag_commands::RagState>,
    online: Option<bool>,
) -> ResearchCommandResult<BuildReport> {
    let _guard = BuildingGuard::acquire().ok_or_else(|| ResearchCommandError {
        code: "conflict",
        message: "The graph is already being built.".to_string(),
    })?;
    let services = state.services().await?;
    let resolver = resolver_for(
        &data_dir(&app)?,
        online.unwrap_or(false),
        services.db.clone(),
        Arc::new(SafeClient::system()),
    );
    let files: Vec<String> = indexed_pdfs(&rag.rag)
        .await
        .into_iter()
        .map(|f| disk_spelling(&f))
        .collect();
    let emitter = app.clone();
    let progress = move |p: BuildProgress| {
        if let Err(e) = emitter.emit(GRAPH_PROGRESS_EVENT, &p) {
            tracing::debug!(target: "shodh::research", error = %e, "graph progress not sent");
        }
    };
    let report = services
        .citations
        .build(&files, &services.results, &resolver, &progress)
        .await?;
    broadcast_change(&app, "graph", None);
    Ok(report)
}

/// The resolver of a build: online only when the user asked for it and the stored
/// privacy policy allows the web (Local-only mode off, web access on); an unreadable
/// policy counts as not allowing it. Otherwise only answers cached on this computer are
/// read and nothing is sent.
pub fn resolver_for(
    data_dir: &std::path::Path,
    online: bool,
    db: Arc<shodh_rag::research::ResearchDb>,
    transport: Arc<dyn shodh_rag::research::citations::ScholarlyTransport>,
) -> Resolver {
    if online && web_block_reason(data_dir).is_none() {
        Resolver::online(transport, db)
    } else {
        Resolver::cache_only(db)
    }
}

/// The file as spelled on disk (the index stores a lower-cased spelling on Windows).
pub fn disk_spelling(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|real| {
            let text = real.display().to_string();
            text.strip_prefix(r"\\?\")
                .map(str::to_string)
                .unwrap_or(text)
        })
        .unwrap_or_else(|_| path.to_string())
}

#[tauri::command]
pub async fn paper_graph_view(state: State<'_, ResearchState>) -> ResearchCommandResult<GraphView> {
    let services = state.services().await?;
    let graph = services.citations.graph(&services.results).await?;
    Ok(graph_view(&graph))
}

/// A paper page: the graph's view of the paper, and for a library paper its results and
/// snippets.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperDetail {
    #[serde(flatten)]
    pub view: PaperView,
    pub results: Vec<ResultRecord>,
    pub snippets: Vec<Snippet>,
}

/// The page of a paper named by graph id, file path, arXiv id, DOI or title.
pub async fn paper_detail(
    state: &ResearchState,
    paper: &str,
) -> ResearchCommandResult<PaperDetail> {
    let services = state.services().await?;
    let graph = services.citations.graph(&services.results).await?;
    let node = graph.find(paper).ok_or_else(|| ResearchCommandError {
        code: "not_found",
        message: if graph.papers().is_empty() {
            "The paper graph has not been built yet (Library → Graph → Build).".to_string()
        } else {
            format!("No paper in the graph matches \"{paper}\".")
        },
    })?;
    let view = paper_view(&graph, &node.id).ok_or_else(|| ResearchCommandError {
        code: "not_found",
        message: format!("No paper in the graph matches \"{paper}\"."),
    })?;
    let (results, snippets) = match &view.paper.file_path {
        Some(file) => {
            let results = services.results.list(file).await?.results;
            let snippets = services
                .snippets
                .list(&SnippetQuery {
                    file_path: Some(file.clone()),
                    text: None,
                    scopes: Vec::new(),
                    limit: Some(100),
                })
                .await?;
            (results, snippets)
        }
        None => (Vec::new(), Vec::new()),
    };
    Ok(PaperDetail {
        view,
        results,
        snippets,
    })
}

#[tauri::command]
pub async fn paper_get(
    state: State<'_, ResearchState>,
    paper: String,
) -> ResearchCommandResult<PaperDetail> {
    paper_detail(&state, &paper).await
}

#[tauri::command]
pub async fn papers_find(
    state: State<'_, ResearchState>,
    filter: PaperFilter,
) -> ResearchCommandResult<Vec<PaperNode>> {
    let services = state.services().await?;
    let graph = services.citations.graph(&services.results).await?;
    Ok(find(&graph, &filter, MAX_FOUND))
}

/// Input of [`paper_concept`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConceptInput {
    /// `method` or `dataset`.
    pub kind: String,
    /// Canonical id (`method:deltanet`) or label as printed.
    pub id: String,
}

#[tauri::command]
pub async fn paper_concept(
    state: State<'_, ResearchState>,
    concept: ConceptInput,
) -> ResearchCommandResult<ConceptView> {
    let services = state.services().await?;
    let graph = services.citations.graph(&services.results).await?;
    concept_view(&graph, &concept.kind, &concept.id).ok_or_else(|| ResearchCommandError {
        code: "not_found",
        message: format!(
            "The {} \"{}\" is not in the graph (methods and datasets come from results extracted from papers' tables).",
            concept.kind, concept.id
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_users_policy_lets_a_build_look_papers_up_online() {
        use crate::app_settings::{SettingsStore, SETTINGS_FILE};
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(
            shodh_rag::research::ResearchDb::open(&dir.path().join("shodh.db"), None).unwrap(),
        );
        let transport: Arc<dyn shodh_rag::research::citations::ScholarlyTransport> =
            Arc::new(SafeClient::system());
        let online = |asked: bool| {
            resolver_for(dir.path(), asked, db.clone(), transport.clone()).is_online()
        };
        // Default policy: web access on, Local-only off.
        assert!(online(true));
        assert!(!online(false), "not asked: cache only");
        let store = SettingsStore::in_dir(dir.path());
        store
            .update(|s| {
                s.policy.local_only = true;
                Ok(())
            })
            .unwrap();
        assert!(!online(true), "Local-only mode sends nothing");
        store
            .update(|s| {
                s.policy.local_only = false;
                s.policy.web_access = false;
                Ok(())
            })
            .unwrap();
        assert!(!online(true), "web access off sends nothing");
        std::fs::write(dir.path().join(SETTINGS_FILE), "{not json").unwrap();
        assert!(!online(true), "an unreadable policy fails closed");
    }

    #[test]
    fn only_one_build_runs_at_a_time() {
        let first = BuildingGuard::acquire();
        assert!(first.is_some());
        assert!(BuildingGuard::acquire().is_none());
        drop(first);
        assert!(BuildingGuard::acquire().is_some());
    }
}
