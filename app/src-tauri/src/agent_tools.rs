//! App-layer host tools for the agent harness: calendar tasks and events, and
//! folder sources. They implement `shodh_rag::harness::tools::HostTool`; the
//! registry applies schema validation, the profile allowlist, the approval
//! gate (all of these are `write` or `destructive`) and auditing.
//!
//! Folder indexing holds the RAG engine's write lock for the whole job, so
//! `add_folder` and `reindex_source` start a background job and return at
//! once instead of blocking the agent (and every search) until it finishes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime};
use serde_json::{json, Value};
use shodh_rag::audit::payload::{indexing_outcome, source_change, ChangeOrigin};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::documents::OpenDocumentTool;
use shodh_rag::harness::tools::navigate::OpenViewTool;
use shodh_rag::harness::tools::plan::UpdatePlanTool;
use shodh_rag::harness::tools::search::SearchDocumentsTool;
use shodh_rag::harness::tools::sources::{find_folder_source, ListSourcesTool, SourceSummary};
use shodh_rag::harness::tools::{
    ApprovalPreview, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;
use shodh_rag::indexing::{IndexingOptions, IndexingState};
use shodh_rag::RAGEngine;
use tauri::{AppHandle, Manager};
use tokio::sync::RwLock;

use crate::calendar_commands::{insert_event, insert_task, NewEvent, NewTask};
use crate::event_emitter::TauriEventEmitter;
use crate::rag_commands::RagState;

/// Build the registry with every v1 tool.
pub fn build_registry(
    app: &AppHandle,
    rag: Arc<RwLock<RAGEngine>>,
) -> Result<ToolRegistry, RegistryError> {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SearchDocumentsTool::new(rag.clone())))?;
    registry.register(Arc::new(OpenDocumentTool::new(rag.clone())))?;
    registry.register(Arc::new(ListSourcesTool::new(rag)))?;
    registry.register(Arc::new(UpdatePlanTool))?;
    registry.register(Arc::new(OpenViewTool))?;
    registry.register(Arc::new(CreateTaskTool { app: app.clone() }))?;
    registry.register(Arc::new(CreateEventTool { app: app.clone() }))?;
    registry.register(Arc::new(AddFolderTool { app: app.clone() }))?;
    registry.register(Arc::new(ReindexSourceTool { app: app.clone() }))?;
    registry.register(Arc::new(RemoveSourceTool { app: app.clone() }))?;
    Ok(registry)
}

fn invalid(tool: &str, reasons: impl Into<String>) -> ToolError {
    ToolError::InvalidArguments {
        tool: tool.to_string(),
        reasons: reasons.into(),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A calendar moment: a date (all-day) or a date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Moment {
    Date(NaiveDate),
    DateTime(NaiveDateTime),
}

impl Moment {
    fn as_datetime(self) -> NaiveDateTime {
        match self {
            Moment::Date(d) => d.and_hms_opt(0, 0, 0).unwrap_or_default(),
            Moment::DateTime(dt) => dt,
        }
    }
}

/// Accept `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS]` or RFC 3339.
fn parse_moment(value: &str) -> Option<Moment> {
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(Moment::Date(d));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(Moment::DateTime(dt.naive_local()));
    }
    [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(value, f).ok())
    .map(Moment::DateTime)
}

const MOMENT_HINT: &str = "use YYYY-MM-DD, YYYY-MM-DDTHH:MM or an RFC 3339 timestamp";

// ── create_task ────────────────────────────────────────────────────────────

pub struct CreateTaskTool {
    app: AppHandle,
}

#[async_trait]
impl HostTool for CreateTaskTool {
    fn name(&self) -> &'static str {
        app_tools::CREATE_TASK
    }
    fn label(&self) -> &'static str {
        "Create task"
    }
    fn label_template(&self) -> &'static str {
        "Creating task {title}"
    }
    fn description(&self) -> &'static str {
        "Create a calendar task (a to-do with an optional due date). Needs the user's approval. \
         source_ref can hold the file the task came from, e.g. a contract path."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "due": {"type": "string", "minLength": 10, "maxLength": 40},
                "source_ref": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["title"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        Ok(ApprovalPreview {
            label: None,
            details: json!({
                "title": str_arg(args, "title"),
                "due": str_arg(args, "due"),
                "sourceRef": str_arg(args, "source_ref"),
            }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CREATE_TASK;
        let title = str_arg(&args, "title").ok_or_else(|| invalid(tool, "`title` is required"))?;
        let due = match str_arg(&args, "due") {
            Some(due) => {
                parse_moment(due)
                    .ok_or_else(|| invalid(tool, format!("`due` {due:?}: {MOMENT_HINT}")))?;
                Some(due.to_string())
            }
            None => None,
        };
        let task = insert_task(
            &self.app,
            NewTask {
                title: title.to_string(),
                due_date: due.clone(),
                source: Some("agent".to_string()),
                source_ref: str_arg(&args, "source_ref").map(str::to_string),
                ..NewTask::default()
            },
        )
        .map_err(ToolError::Failed)?;
        let due_text = due
            .as_deref()
            .map(|d| format!(", due {d}"))
            .unwrap_or_default();
        Ok(ToolOutput {
            text_for_model: format!(
                "Created task \"{}\" (id {}{due_text}). Use open_view with view calendar and this id to show it.",
                task.title, task.id
            ),
            summary_for_ui: format!("Created task “{}”{due_text}", task.title),
            detail: Some(json!({ "id": task.id, "title": task.title, "due": due })),
        })
    }
}

// ── create_event ───────────────────────────────────────────────────────────

pub struct CreateEventTool {
    app: AppHandle,
}

#[async_trait]
impl HostTool for CreateEventTool {
    fn name(&self) -> &'static str {
        app_tools::CREATE_EVENT
    }
    fn label(&self) -> &'static str {
        "Create event"
    }
    fn label_template(&self) -> &'static str {
        "Creating event {title}"
    }
    fn description(&self) -> &'static str {
        "Create a calendar event. A date without a time makes an all-day event. Needs the user's approval."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 200},
                "start": {"type": "string", "minLength": 10, "maxLength": 40},
                "end": {"type": "string", "minLength": 10, "maxLength": 40}
            },
            "required": ["title", "start"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        Ok(ApprovalPreview {
            label: None,
            details: json!({
                "title": str_arg(args, "title"),
                "start": str_arg(args, "start"),
                "end": str_arg(args, "end"),
            }),
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CREATE_EVENT;
        let title = str_arg(&args, "title").ok_or_else(|| invalid(tool, "`title` is required"))?;
        let start = str_arg(&args, "start").ok_or_else(|| invalid(tool, "`start` is required"))?;
        let start_moment = parse_moment(start)
            .ok_or_else(|| invalid(tool, format!("`start` {start:?}: {MOMENT_HINT}")))?;
        let end = match str_arg(&args, "end") {
            Some(end) => {
                let end_moment = parse_moment(end)
                    .ok_or_else(|| invalid(tool, format!("`end` {end:?}: {MOMENT_HINT}")))?;
                if end_moment.as_datetime() < start_moment.as_datetime() {
                    return Err(invalid(tool, "`end` is before `start`"));
                }
                Some(end.to_string())
            }
            None => None,
        };
        let all_day = matches!(start_moment, Moment::Date(_));
        let event = insert_event(
            &self.app,
            NewEvent {
                title: title.to_string(),
                start_time: start.to_string(),
                end_time: end.clone(),
                all_day: Some(all_day),
                source: Some("agent".to_string()),
                ..NewEvent::default()
            },
        )
        .map_err(ToolError::Failed)?;
        Ok(ToolOutput {
            text_for_model: format!(
                "Created event \"{}\" (id {}) starting {start}. Use open_view with view calendar and this id to show it.",
                event.title, event.id
            ),
            summary_for_ui: format!("Created event “{}” on {start}", event.title),
            detail: Some(json!({ "id": event.id, "title": event.title, "start": start, "end": end, "allDay": all_day })),
        })
    }
}

// ── folder sources ─────────────────────────────────────────────────────────

fn agent_indexing_options() -> IndexingOptions {
    IndexingOptions {
        skip_indexed: false,
        watch_changes: false,
        process_subdirs: true,
        priority: "normal".to_string(),
        // Empty means every supported file type.
        file_types: Vec::new(),
    }
}

/// Start indexing `folder` into `space_id` in the background.
/// Index `folder` as `space_id` in the background and audit the outcome
/// (`action` is `add` or `reindex`) in the calling run's scope.
fn spawn_index_job(
    app: &AppHandle,
    ctx: &ToolContext,
    action: &'static str,
    folder: String,
    space_id: String,
) {
    let app = app.clone();
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let rag = app.state::<RagState>().rag.clone();
        let indexing_state = app.state::<IndexingState>();
        let emitter = TauriEventEmitter::new(app.clone());
        let mut engine = rag.write().await;
        let result = shodh_rag::indexing::index_folder(
            &folder,
            &space_id,
            &agent_indexing_options(),
            &mut engine,
            &indexing_state,
            Some(&emitter as &dyn shodh_rag::chat::EventEmitter),
        )
        .await;
        drop(engine);
        if let Err(e) = &result {
            tracing::warn!(target: "shodh::harness", source_id = %space_id, error = %e, "agent-started indexing failed");
        }
        ctx.audit(
            AuditEventType::SourceChange,
            source_change(
                action,
                ChangeOrigin::Agent,
                Some(&space_id),
                Some(&folder),
                indexing_outcome(&result),
            ),
        );
    });
}

fn validate_folder(tool: &str, raw: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(invalid(tool, "`path` must be an absolute folder path"));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ToolError::Forbidden(
            "Paths containing '..' are not allowed".to_string(),
        ));
    }
    if !path.is_dir() {
        return Err(ToolError::NotFound(format!(
            "{raw} is not an existing folder"
        )));
    }
    std::fs::canonicalize(path)
        .map(|p| {
            // Strip the Windows verbatim prefix so paths match the index.
            let text = p.display().to_string();
            PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
        })
        .map_err(|e| ToolError::Unavailable(format!("Cannot open {raw}: {e}")))
}

/// Resolve the `source_id` argument to an indexed folder source.
async fn resolve_source(
    app: &AppHandle,
    tool: &str,
    args: &Value,
) -> Result<SourceSummary, ToolError> {
    let source_id =
        str_arg(args, "source_id").ok_or_else(|| invalid(tool, "`source_id` is required"))?;
    let rag = app.state::<RagState>().rag.clone();
    let engine = rag.read().await;
    find_folder_source(&engine, source_id).await
}

fn source_preview(source: &SourceSummary) -> Value {
    json!({
        "sourceId": source.source_id,
        "folder": source.folder,
        "files": source.files,
        "chunks": source.chunks,
    })
}

pub struct AddFolderTool {
    app: AppHandle,
}

#[async_trait]
impl HostTool for AddFolderTool {
    fn name(&self) -> &'static str {
        app_tools::ADD_FOLDER
    }
    fn label(&self) -> &'static str {
        "Add folder"
    }
    fn label_template(&self) -> &'static str {
        "Adding folder {path}"
    }
    fn description(&self) -> &'static str {
        "Index a folder on this computer so its documents become searchable. Needs the user's \
         approval. Indexing runs in the background; the call returns once it has started."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let raw = str_arg(args, "path")
            .ok_or_else(|| invalid(app_tools::ADD_FOLDER, "`path` is required"))?;
        let folder = validate_folder(app_tools::ADD_FOLDER, raw)?;
        let folder = folder.display().to_string();
        Ok(ApprovalPreview {
            label: Some(format!("Add {folder} to the index")),
            details: json!({ "path": folder }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::ADD_FOLDER;
        let raw = str_arg(&args, "path").ok_or_else(|| invalid(tool, "`path` is required"))?;
        let folder = validate_folder(tool, raw)?;
        let folder_text = folder.display().to_string();
        let source_id = uuid::Uuid::new_v4().to_string();
        spawn_index_job(
            &self.app,
            ctx,
            "add",
            folder_text.clone(),
            source_id.clone(),
        );
        Ok(ToolOutput {
            text_for_model: format!(
                "Started indexing {folder_text} as source {source_id}. It runs in the background; \
                 its documents become searchable as files finish."
            ),
            summary_for_ui: format!("Indexing {folder_text}"),
            detail: Some(json!({ "sourceId": source_id, "path": folder_text })),
        })
    }
}

pub struct ReindexSourceTool {
    app: AppHandle,
}

#[async_trait]
impl HostTool for ReindexSourceTool {
    fn name(&self) -> &'static str {
        app_tools::REINDEX_SOURCE
    }
    fn label(&self) -> &'static str {
        "Re-index source"
    }
    fn label_template(&self) -> &'static str {
        "Re-indexing source {source_id}"
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let source = resolve_source(&self.app, app_tools::REINDEX_SOURCE, args).await?;
        let folder = source
            .folder
            .clone()
            .unwrap_or_else(|| source.source_id.clone());
        Ok(ApprovalPreview {
            label: Some(format!("Re-index {folder}")),
            details: source_preview(&source),
        })
    }
    fn description(&self) -> &'static str {
        "Re-index a folder source (id from list_sources) to pick up new and changed files. Needs \
         the user's approval. Runs in the background."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_id": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "required": ["source_id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let source = resolve_source(&self.app, app_tools::REINDEX_SOURCE, &args).await?;
        let source_id = source.source_id.as_str();
        let folder = source.folder.ok_or_else(|| {
            ToolError::NotFound(format!("The folder of source {source_id} is unknown"))
        })?;
        if !Path::new(&folder).is_dir() {
            return Err(ToolError::NotFound(format!(
                "{folder} no longer exists on disk"
            )));
        }
        spawn_index_job(
            &self.app,
            ctx,
            "reindex",
            folder.clone(),
            source_id.to_string(),
        );
        Ok(ToolOutput {
            text_for_model: format!(
                "Started re-indexing {folder} (source {source_id}) in the background."
            ),
            summary_for_ui: format!("Re-indexing {folder}"),
            detail: Some(json!({ "sourceId": source_id, "path": folder })),
        })
    }
}

pub struct RemoveSourceTool {
    app: AppHandle,
}

#[async_trait]
impl HostTool for RemoveSourceTool {
    fn name(&self) -> &'static str {
        app_tools::REMOVE_SOURCE
    }
    fn label(&self) -> &'static str {
        "Remove source"
    }
    fn label_template(&self) -> &'static str {
        "Removing source {source_id}"
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let source = resolve_source(&self.app, app_tools::REMOVE_SOURCE, args).await?;
        let folder = source
            .folder
            .clone()
            .unwrap_or_else(|| source.source_id.clone());
        Ok(ApprovalPreview {
            label: Some(format!(
                "Remove {folder} ({} files) from the index",
                source.files
            )),
            details: source_preview(&source),
        })
    }
    fn description(&self) -> &'static str {
        "Remove a folder source (id from list_sources) from the index. Files on disk are not \
         touched. Always needs the user's approval."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_id": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "required": ["source_id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Destructive
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let source = resolve_source(&self.app, app_tools::REMOVE_SOURCE, &args).await?;
        let source_id = source.source_id.as_str();
        let rag = self.app.state::<RagState>().rag.clone();
        let deleted = {
            let mut engine = rag.write().await;
            engine
                .delete_by_space_id(&source.source_id)
                .await
                .map_err(|e| ToolError::Failed(format!("Removing the source failed: {e}")))?
        };
        let folder = source.folder.unwrap_or_else(|| source.source_id.clone());
        ctx.audit(
            AuditEventType::SourceChange,
            source_change(
                "remove",
                ChangeOrigin::Agent,
                Some(source_id),
                Some(&folder),
                json!({"ok": true, "files": source.files, "chunks": deleted}),
            ),
        );
        Ok(ToolOutput {
            text_for_model: format!(
                "Removed {folder} (source {source_id}, {} files, {deleted} chunks) from the index. Files on disk were not touched.",
                source.files
            ),
            summary_for_ui: format!("Removed {folder} from the index"),
            detail: Some(json!({ "sourceId": source_id, "path": folder, "files": source.files, "chunks": deleted })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_parse_dates_and_times() {
        assert!(matches!(parse_moment("2026-10-30"), Some(Moment::Date(_))));
        assert!(matches!(
            parse_moment("2026-10-30T09:30"),
            Some(Moment::DateTime(_))
        ));
        assert!(matches!(
            parse_moment("2026-10-30T09:30:00+05:30"),
            Some(Moment::DateTime(_))
        ));
        assert_eq!(parse_moment("next friday"), None);
        assert_eq!(parse_moment("2026-13-01"), None);
    }
}
