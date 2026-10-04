//! Snippets, Result statements and the citation graph: managed state and the commands of
//! the PDF viewer, the focus pop-out, the Library's Snippets and Results views and the
//! comparison builder (the graph's commands are in `graph_commands`).
//!
//! Both are research-pack statements in the statement store the memory layer opens (one
//! `StatementStore` per LanceDB table, so its write lock covers every writer), with snippet
//! images, extraction reports and rejections in `shodh.db` (see `shodh_rag::research`).
//! Every write is broadcast as [`RESEARCH_CHANGED_EVENT`], also when the agent made it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::audit::payload::{is_cloud, provider_id};
use shodh_rag::audit::AuditKey;
use shodh_rag::llm::{ApiProvider, LLMMode};
use shodh_rag::processing::table_model::SharedTableModel;
use shodh_rag::research::citations::{CitationService, GraphSlot};
use shodh_rag::research::pdf_text::PageRect;
use shodh_rag::research::results::{
    Comparison, ExtractionReport, PaperResults, ResultFacets, ResultFilter, ResultService,
};
use shodh_rag::research::snippets::{
    NewSnippet, Snippet, SnippetAuthor, SnippetKind, SnippetPatch, SnippetQuery, SnippetService,
    SnippetTable,
};
use shodh_rag::research::vision::{self, VisionModel};
use shodh_rag::research::{ResearchDb, ResearchError};
use shodh_rag::statements::{Scope, StatementError};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::OnceCell;

use crate::app_settings::SettingsStore;
use crate::audit_commands::AuditState;
use crate::llm_commands::LLMState;
use crate::memory_commands::{MemoryState, APP_VERSION};

/// Emitted with `{ "kind": "snippet" | "result" | "graph", "filePath": ... }` after a write.
pub const RESEARCH_CHANGED_EVENT: &str = "research-changed";

/// Error of a research command: `not_found`, `invalid`, `unavailable` or `storage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchCommandError {
    pub code: &'static str,
    pub message: String,
}

impl ResearchCommandError {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "unavailable",
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "invalid",
            message: message.into(),
        }
    }
}

impl From<ResearchError> for ResearchCommandError {
    fn from(error: ResearchError) -> Self {
        let code = match &error {
            ResearchError::Invalid(_) | ResearchError::Pdf(_) => "invalid",
            ResearchError::NotFound(_) => "not_found",
            ResearchError::Statement(StatementError::EmbeddingUnavailable(_))
            | ResearchError::Model(_) => "unavailable",
            ResearchError::Statement(StatementError::Invalid(_)) => "invalid",
            ResearchError::Statement(StatementError::NotFound(_)) => "not_found",
            ResearchError::Statement(_) | ResearchError::Database(_) | ResearchError::Task(_) => {
                "storage"
            }
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for ResearchCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type ResearchCommandResult<T> = Result<T, ResearchCommandError>;

/// The snippet, result and citation graph services over one statement store.
pub struct ResearchServices {
    pub snippets: SnippetService,
    pub results: ResultService,
    pub citations: CitationService,
    /// `shodh.db`'s research tables (the graph's scholarly cache lives there).
    pub db: Arc<ResearchDb>,
}

/// Managed state: the research services, opened on first use (they need the statement
/// store, which needs the search models). Clones share them (the agent tools hold one).
#[derive(Clone)]
pub struct ResearchState {
    inner: Arc<Inner>,
}

struct Inner {
    memory: MemoryState,
    database: Option<(PathBuf, Option<AuditKey>)>,
    services: OnceCell<Arc<ResearchServices>>,
    /// The table model, when installed; result extraction structures tables with it.
    tables: SharedTableModel,
    /// The citation graph snapshot, shared with the document search's ranker.
    graph: GraphSlot,
}

impl ResearchState {
    /// State over the app's memory store and the database the audit log opened. `graph` is
    /// the slot the document search's graph ranker reads.
    pub fn new(
        memory: MemoryState,
        audit: &AuditState,
        tables: SharedTableModel,
        graph: GraphSlot,
    ) -> Self {
        Self::at(memory, audit.database(), tables, graph)
    }

    /// State over `memory`'s statement store and `shodh.db` at `database`.
    pub fn at(
        memory: MemoryState,
        database: Option<(PathBuf, Option<AuditKey>)>,
        tables: SharedTableModel,
        graph: GraphSlot,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                memory,
                database,
                services: OnceCell::new(),
                tables,
                graph,
            }),
        }
    }

    /// The services, opening them if needed. Fails (and retries on the next call) while
    /// the statement store or the database is unavailable.
    pub async fn services(&self) -> ResearchCommandResult<Arc<ResearchServices>> {
        self.inner
            .services
            .get_or_try_init(|| async {
                let service = self
                    .inner
                    .memory
                    .service()
                    .await
                    .map_err(ResearchCommandError::unavailable)?;
                let (path, key) = self.inner.database.clone().ok_or_else(|| {
                    ResearchCommandError::unavailable(
                        "Snippets and results need the app database (shodh.db), which could not be opened; see the audit page.",
                    )
                })?;
                let db = tokio::task::spawn_blocking(move || ResearchDb::open(&path, key.as_ref()))
                    .await
                    .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?
                    .map_err(ResearchCommandError::from)?;
                let db = Arc::new(db);
                let store = service.store().clone();
                Ok(Arc::new(ResearchServices {
                    snippets: SnippetService::new(store.clone(), db.clone(), APP_VERSION),
                    citations: CitationService::new(
                        store.clone(),
                        db.clone(),
                        self.inner.graph.clone(),
                    ),
                    results: ResultService::new(store, db.clone(), APP_VERSION)
                        .with_table_model(self.inner.tables.clone()),
                    db,
                }))
            })
            .await
            .cloned()
    }
}

/// Tells open views that snippets or results (of `file_path`, when known) changed.
pub fn broadcast_change(app: &AppHandle, kind: &str, file_path: Option<&str>) {
    if let Err(e) = app.emit(
        RESEARCH_CHANGED_EVENT,
        json!({ "kind": kind, "filePath": file_path }),
    ) {
        tracing::warn!(target: "shodh::research", error = %e, "research change not broadcast");
    }
}

/// Scopes visible from an optional workspace: the workspace and global ones, or every
/// scope when none is given (the Library shows everything).
pub fn scopes_for(workspace: Option<&str>) -> Vec<Scope> {
    match workspace.map(str::trim).filter(|w| !w.is_empty()) {
        Some(id) => Scope::Workspace(id.to_string()).visible(),
        None => Vec::new(),
    }
}

fn decode_png(base64_png: &str) -> ResearchCommandResult<Vec<u8>> {
    let text = base64_png.trim();
    let text = text.strip_prefix("data:image/png;base64,").unwrap_or(text);
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| ResearchCommandError::invalid("The snippet image is not valid base64."))
}

/// Checks `path` is an existing PDF and returns it as spelled on disk (citations carry
/// the index's lower-cased form; snippets and results show the real file name).
fn require_pdf(path: &str) -> ResearchCommandResult<String> {
    let p = Path::new(path.trim());
    let is_pdf = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return Err(ResearchCommandError::invalid(
            "Snippets and results are taken from PDF files.",
        ));
    }
    if !p.is_file() {
        return Err(ResearchCommandError::invalid(format!(
            "{} does not exist or is not a file.",
            p.display()
        )));
    }
    Ok(std::fs::canonicalize(p)
        .map(|real| {
            let text = real.display().to_string();
            text.strip_prefix(r"\\?\")
                .map(str::to_string)
                .unwrap_or(text)
        })
        .unwrap_or_else(|_| path.trim().to_string()))
}

/// Input of [`snippets_create`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSnippetInput {
    pub file_path: String,
    pub page: u32,
    pub rect: PageRect,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image_png: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub kind: Option<SnippetKind>,
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Input of [`snippets_list`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetListQuery {
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[tauri::command]
pub async fn snippets_create(
    app: AppHandle,
    state: State<'_, ResearchState>,
    input: NewSnippetInput,
) -> ResearchCommandResult<Snippet> {
    let file_path = require_pdf(&input.file_path)?;
    let image = input.image_png.as_deref().map(decode_png).transpose()?;
    let services = state.services().await?;
    let snippet = services
        .snippets
        .create(NewSnippet {
            file_path,
            page: input.page,
            rect: input.rect,
            text: input.text,
            image_png: image,
            title: input.title,
            note: input.note,
            tags: input.tags,
            kind: input.kind.unwrap_or_default(),
            scope: Scope::for_workspace(input.workspace.as_deref()),
            author: SnippetAuthor::User,
        })
        .await?;
    broadcast_change(&app, "snippet", Some(&snippet.file_path));
    Ok(snippet)
}

#[tauri::command]
pub async fn snippets_list(
    state: State<'_, ResearchState>,
    query: SnippetListQuery,
) -> ResearchCommandResult<Vec<Snippet>> {
    let services = state.services().await?;
    Ok(services
        .snippets
        .list(&SnippetQuery {
            file_path: query.file_path,
            text: query.text,
            scopes: scopes_for(query.workspace.as_deref()),
            limit: query.limit,
        })
        .await?)
}

#[tauri::command]
pub async fn snippets_get(
    state: State<'_, ResearchState>,
    id: String,
) -> ResearchCommandResult<Snippet> {
    Ok(state.services().await?.snippets.get(&id).await?)
}

/// The snippet's PNG as base64, or null when it has none yet.
#[tauri::command]
pub async fn snippets_image(
    state: State<'_, ResearchState>,
    id: String,
) -> ResearchCommandResult<Option<String>> {
    let png = state.services().await?.snippets.image(&id).await?;
    Ok(png.map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
pub async fn snippets_set_image(
    app: AppHandle,
    state: State<'_, ResearchState>,
    id: String,
    image_png: String,
) -> ResearchCommandResult<Snippet> {
    let png = decode_png(&image_png)?;
    let snippet = state.services().await?.snippets.set_image(&id, png).await?;
    broadcast_change(&app, "snippet", Some(&snippet.file_path));
    Ok(snippet)
}

#[tauri::command]
pub async fn snippets_update(
    app: AppHandle,
    state: State<'_, ResearchState>,
    id: String,
    patch: SnippetPatch,
) -> ResearchCommandResult<Snippet> {
    let snippet = state.services().await?.snippets.update(&id, patch).await?;
    broadcast_change(&app, "snippet", Some(&snippet.file_path));
    Ok(snippet)
}

#[tauri::command]
pub async fn snippets_delete(
    app: AppHandle,
    state: State<'_, ResearchState>,
    id: String,
) -> ResearchCommandResult<()> {
    let snippet = state.services().await?.snippets.delete(&id).await?;
    broadcast_change(&app, "snippet", Some(&snippet.file_path));
    Ok(())
}

/// The parser's table rows under the snippet, if its PDF has a table block there.
#[tauri::command]
pub async fn snippets_table(
    state: State<'_, ResearchState>,
    id: String,
) -> ResearchCommandResult<Option<SnippetTable>> {
    Ok(state.services().await?.snippets.table(&id).await?)
}

/// Whether a vision-capable model can transcribe equations, from provider metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionCapability {
    pub available: bool,
    pub model: Option<String>,
    pub reason: Option<String>,
}

/// Which vision model the configured provider offers, or why none (pure: the metadata
/// lookups are passed in as their results).
fn vision_choice(mode: &LLMMode, local_only: bool) -> Result<VisionModel, String> {
    match mode {
        LLMMode::Disabled => Err("No model is configured (Settings → Model).".to_string()),
        LLMMode::Local { model_path } => Err(format!(
            "The local model ({}) runs text-only: Shodh's built-in llama.cpp runtime loads no vision projector.",
            model_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "local".to_string())
        )),
        LLMMode::External {
            provider, model, ..
        } => {
            let id = provider_id(provider);
            if local_only && is_cloud(id) {
                return Err(
                    "Local-only mode is on: transcription needs a local vision model (Ollama)."
                        .to_string(),
                );
            }
            match provider {
                ApiProvider::OpenRouter => Ok(VisionModel::OpenRouter {
                    model: model.clone(),
                }),
                ApiProvider::Ollama => Ok(VisionModel::Ollama {
                    model: model.clone(),
                }),
                _ => Err(format!(
                    "{id} does not publish which inputs {model} accepts, so Shodh cannot confirm it reads images. Use an OpenRouter or Ollama model that lists image input."
                )),
            }
        }
    }
}

/// Checks the chosen model's metadata: OpenRouter's input modalities or Ollama's
/// capabilities must include images.
async fn vision_capability_of(mode: &LLMMode, local_only: bool) -> Result<VisionModel, String> {
    let model = vision_choice(mode, local_only)?;
    match &model {
        VisionModel::OpenRouter { model: id } => match vision::openrouter_modalities(id).await {
            Ok(Some(modalities)) if modalities.iter().any(|m| m == "image") => Ok(model),
            Ok(Some(_)) => Err(format!("OpenRouter lists {id} as text-only.")),
            Ok(None) => Err(format!(
                "OpenRouter does not list {id}, so its inputs are unknown."
            )),
            Err(e) => Err(format!("Could not check whether {id} reads images: {e}")),
        },
        VisionModel::Ollama { model: id } => match vision::ollama_capabilities(id).await {
            Ok(capabilities) if capabilities.iter().any(|c| c == "vision") => Ok(model),
            Ok(_) => Err(format!("Ollama reports no vision capability for {id}.")),
            Err(e) => Err(format!("Could not check whether {id} reads images: {e}")),
        },
    }
}

fn current_mode(llm: &LLMState) -> LLMMode {
    llm.config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mode
        .clone()
}

fn local_only(app: &AppHandle) -> bool {
    app.path()
        .app_data_dir()
        .ok()
        .and_then(|dir| SettingsStore::in_dir(&dir).load().ok())
        .map(|s| s.policy.local_only)
        // Unreadable settings: assume the stricter policy.
        .unwrap_or(true)
}

#[tauri::command]
pub async fn vision_capability(
    app: AppHandle,
    llm: State<'_, LLMState>,
) -> ResearchCommandResult<VisionCapability> {
    let mode = current_mode(&llm);
    Ok(match vision_capability_of(&mode, local_only(&app)).await {
        Ok(model) => VisionCapability {
            available: true,
            model: Some(model.model_id().to_string()),
            reason: None,
        },
        Err(reason) => VisionCapability {
            available: false,
            model: None,
            reason: Some(reason),
        },
    })
}

/// Sends the snippet's image to the vision model and stores the LaTeX with the model id.
#[tauri::command]
pub async fn snippets_transcribe_latex(
    app: AppHandle,
    state: State<'_, ResearchState>,
    llm: State<'_, LLMState>,
    id: String,
) -> ResearchCommandResult<Snippet> {
    let mode = current_mode(&llm);
    let model = vision_capability_of(&mode, local_only(&app))
        .await
        .map_err(ResearchCommandError::unavailable)?;
    let services = state.services().await?;
    let png = services.snippets.image(&id).await?.ok_or_else(|| {
        ResearchCommandError::invalid(
            "The snippet's image is not stored yet; open the snippet so it is rendered first.",
        )
    })?;
    let key = match &model {
        VisionModel::OpenRouter { .. } => Some(
            crate::agent_session_commands::resolve_key(&llm, &mode)
                .await
                .ok_or_else(|| {
                    ResearchCommandError::unavailable("No OpenRouter API key is configured.")
                })?,
        ),
        VisionModel::Ollama { .. } => None,
    };
    let latex = vision::transcribe(&model, key.as_deref(), &png).await?;
    let snippet = services
        .snippets
        .set_latex(&id, &latex, model.model_id())
        .await?;
    broadcast_change(&app, "snippet", Some(&snippet.file_path));
    Ok(snippet)
}

/// Extracts the Result statements of one paper. With `use_model`, the configured model
/// is asked to name the dataset or metric of columns the rules could not (never values).
#[tauri::command]
pub async fn results_extract(
    app: AppHandle,
    state: State<'_, ResearchState>,
    llm: State<'_, LLMState>,
    file_path: String,
    workspace: Option<String>,
    use_model: Option<bool>,
) -> ResearchCommandResult<ExtractionReport> {
    let file_path = require_pdf(&file_path)?;
    let model = if use_model.unwrap_or(false) {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| ResearchCommandError::unavailable(e.to_string()))?;
        Some(
            crate::memory_learn::configured_model(&dir, &llm)
                .map_err(ResearchCommandError::unavailable)?,
        )
    } else {
        None
    };
    let services = state.services().await?;
    let report = services
        .results
        .extract(
            &file_path,
            Scope::for_workspace(workspace.as_deref()),
            model,
        )
        .await?;
    broadcast_change(&app, "result", Some(&report.file_path));
    Ok(report)
}

#[tauri::command]
pub async fn results_list(
    state: State<'_, ResearchState>,
    file_path: String,
) -> ResearchCommandResult<PaperResults> {
    Ok(state.services().await?.results.list(&file_path).await?)
}

/// Accepts (used from now on) or rejects (removed and not extracted again) a result.
#[tauri::command]
pub async fn results_review(
    app: AppHandle,
    state: State<'_, ResearchState>,
    id: String,
    accept: bool,
) -> ResearchCommandResult<()> {
    state.services().await?.results.review(&id, accept).await?;
    broadcast_change(&app, "result", None);
    Ok(())
}

/// Filters of [`results_query`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultFilterInput {
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub dataset: Option<String>,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub papers: Option<Vec<String>>,
    #[serde(default)]
    pub workspace: Option<String>,
}

impl ResultFilterInput {
    pub fn into_filter(self, known_papers: Vec<String>) -> ResultFilter {
        ResultFilter {
            method: self.method,
            dataset: self.dataset,
            metric: self.metric,
            papers: self.papers.unwrap_or_default(),
            scopes: scopes_for(self.workspace.as_deref()),
            known_papers,
        }
    }
}

/// Indexed PDF files (the index's stored paths), for "not yet scanned" notes.
pub async fn indexed_pdfs(rag: &tokio::sync::RwLock<shodh_rag::RAGEngine>) -> Vec<String> {
    match rag.read().await.document_sources().await {
        Ok(rows) => rows
            .into_iter()
            .map(|row| row.source)
            .filter(|source| {
                !source.contains("://") && source.to_ascii_lowercase().ends_with(".pdf")
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        Err(e) => {
            tracing::warn!(target: "shodh::research", error = %e, "indexed files could not be listed");
            Vec::new()
        }
    }
}

#[tauri::command]
pub async fn results_query(
    state: State<'_, ResearchState>,
    rag: State<'_, crate::rag_commands::RagState>,
    filter: ResultFilterInput,
) -> ResearchCommandResult<Comparison> {
    let known = indexed_pdfs(&rag.rag).await;
    let services = state.services().await?;
    Ok(services.results.query(&filter.into_filter(known)).await?)
}

#[tauri::command]
pub async fn results_facets(
    state: State<'_, ResearchState>,
    workspace: Option<String>,
) -> ResearchCommandResult<ResultFacets> {
    let services = state.services().await?;
    Ok(services
        .results
        .facets(&scopes_for(workspace.as_deref()))
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_is_offered_only_where_metadata_exists() {
        let external = |provider: ApiProvider| LLMMode::External {
            provider,
            api_key: String::new(),
            model: "m".to_string(),
        };
        assert_eq!(
            vision_choice(&external(ApiProvider::OpenRouter), false),
            Ok(VisionModel::OpenRouter { model: "m".into() })
        );
        assert_eq!(
            vision_choice(&external(ApiProvider::Ollama), true),
            Ok(VisionModel::Ollama { model: "m".into() })
        );
        let refused = vision_choice(&external(ApiProvider::OpenRouter), true).unwrap_err();
        assert!(refused.contains("Local-only"));
        let unknown = vision_choice(&external(ApiProvider::Anthropic), false).unwrap_err();
        assert!(unknown.contains("does not publish"));
        assert!(vision_choice(&LLMMode::Disabled, false).is_err());
        let local = vision_choice(
            &LLMMode::Local {
                model_path: PathBuf::from("C:/models/qwen.gguf"),
            },
            false,
        )
        .unwrap_err();
        assert!(local.contains("qwen.gguf") && local.contains("vision projector"));
    }

    #[test]
    fn errors_map_to_codes_the_ui_knows() {
        let code = |e: ResearchError| ResearchCommandError::from(e).code;
        assert_eq!(code(ResearchError::Invalid("x".into())), "invalid");
        assert_eq!(code(ResearchError::NotFound("x".into())), "not_found");
        assert_eq!(code(ResearchError::Model("x".into())), "unavailable");
        assert_eq!(
            code(ResearchError::Statement(
                StatementError::EmbeddingUnavailable("x".into())
            )),
            "unavailable"
        );
        assert_eq!(code(ResearchError::Database("x".into())), "storage");
        assert!(decode_png("data:image/png;base64,iVBORw0KGgo=").is_ok());
        assert_eq!(decode_png("***").unwrap_err().code, "invalid");
        assert!(scopes_for(None).is_empty());
        assert_eq!(scopes_for(Some("s1")).len(), 2);
    }
}
