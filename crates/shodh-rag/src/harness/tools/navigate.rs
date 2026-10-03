//! UI actions: tools that change what the user sees, never their data.
//! Each emits a `navigated` event; the conversation keeps streaming in the
//! dock (spec §10a, Conversation dock).
//!
//! - `open_view` switches the app tab.
//! - `show_document` opens a document in the viewer at a page or passage.
//! - `show_audit` opens the audit log with filters applied.
//! - `show_source` opens an indexed folder source in the Library.
//!
//! The app adds `show_calendar` and `open_conversation`, which check their
//! ids against app storage.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::sources::find_folder_source;
use super::{opt_str, req_str, HostTool, ToolContext, ToolError, ToolOutput};
use crate::audit::AuditEventType;
use crate::harness::events::{AgentEvent, NavigationTarget, RiskTier};
use crate::RAGEngine;

pub const OPEN_VIEW: &str = "open_view";
pub const SHOW_DOCUMENT: &str = "show_document";
pub const SHOW_AUDIT: &str = "show_audit";
pub const SHOW_SOURCE: &str = "show_source";

/// Views the agent may open.
pub const VIEWS: [&str; 4] = ["ask", "library", "calendar", "settings"];

/// Longest passage text sent to the viewer for highlighting.
const MAX_PASSAGE_CHARS: usize = 500;

/// Emit a `navigated` event for this call's run.
pub fn emit_navigation(
    ctx: &ToolContext,
    view: &str,
    focus: Option<String>,
    target: Option<NavigationTarget>,
) {
    ctx.emit(AgentEvent::Navigated {
        run_id: ctx.run_id.clone(),
        view: view.to_string(),
        focus,
        target,
    });
}

pub struct OpenViewTool;

#[async_trait]
impl HostTool for OpenViewTool {
    fn name(&self) -> &'static str {
        OPEN_VIEW
    }
    fn label(&self) -> &'static str {
        "Open view"
    }
    fn label_template(&self) -> &'static str {
        "Opening {view}"
    }
    fn description(&self) -> &'static str {
        "Switch the app to a view so the user can see a result: ask (conversation), library \
         (indexed folders), calendar (tasks and events), settings. Optionally focus an item by id, \
         e.g. the id of a task you just created."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "view": {"type": "string", "enum": VIEWS},
                "focus": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "required": ["view"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let view = req_str(&args, "view", OPEN_VIEW)?;
        if !VIEWS.contains(&view) {
            return Err(ToolError::InvalidArguments {
                tool: OPEN_VIEW.to_string(),
                reasons: format!("unknown view {view}"),
            });
        }
        let focus = opt_str(&args, "focus").map(str::to_string);
        emit_navigation(ctx, view, focus.clone(), None);
        Ok(ToolOutput {
            text_for_model: format!("The {view} view is now open for the user."),
            summary_for_ui: format!("Opened {view}"),
            detail: focus.map(|f| json!({ "focus": f })),
        })
    }
}

/// `show_document`: open a document in the viewer.
pub struct ShowDocumentTool;

/// The file name of a path or URI, for labels.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
}

#[async_trait]
impl HostTool for ShowDocumentTool {
    fn name(&self) -> &'static str {
        SHOW_DOCUMENT
    }
    fn label(&self) -> &'static str {
        "Show document"
    }
    fn label_template(&self) -> &'static str {
        "Showing {path}"
    }
    fn description(&self) -> &'static str {
        "Open a document in the app's viewer for the user, optionally at a page and with a \
         passage highlighted. Use the path from search_documents. This only shows the document; \
         use open_document to read it yourself."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 2048},
                "page": {"type": "integer", "minimum": 1, "maximum": 100000},
                "passage": {"type": "string", "minLength": 1, "maxLength": 2000}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let path = req_str(&args, "path", SHOW_DOCUMENT)?;
        let is_record = path.contains("://");
        if !is_record && !std::path::Path::new(path).is_absolute() {
            return Err(ToolError::InvalidArguments {
                tool: SHOW_DOCUMENT.to_string(),
                reasons: "`path` must be the absolute path returned by search_documents"
                    .to_string(),
            });
        }
        let page = args
            .get("page")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok());
        let passage =
            opt_str(&args, "passage").map(|p| crate::harness::truncate_chars(p, MAX_PASSAGE_CHARS));
        let name = file_name(path).to_string();
        let at = page.map(|p| format!(" at page {p}")).unwrap_or_default();
        emit_navigation(
            ctx,
            "ask",
            None,
            Some(NavigationTarget::Document {
                path: path.to_string(),
                page,
                passage,
            }),
        );
        Ok(ToolOutput {
            text_for_model: format!("{name} is now open in the viewer{at}."),
            summary_for_ui: format!("Opened {name}{at}"),
            detail: Some(json!({ "path": path, "page": page })),
        })
    }
}

/// `show_audit`: open the audit log with filters.
pub struct ShowAuditTool;

fn audit_type_names() -> Vec<&'static str> {
    AuditEventType::ALL.iter().map(|t| t.as_str()).collect()
}

#[async_trait]
impl HostTool for ShowAuditTool {
    fn name(&self) -> &'static str {
        SHOW_AUDIT
    }
    fn label(&self) -> &'static str {
        "Show audit log"
    }
    fn label_template(&self) -> &'static str {
        "Opening the audit log"
    }
    fn description(&self) -> &'static str {
        "Open Settings → Usage & Audit for the user with filters applied: event types, a tool \
         name, a time range (RFC 3339 from/to) and free text. Use audit_query to read the log \
         yourself."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "types": {
                    "type": "array",
                    "items": {"type": "string", "enum": audit_type_names()},
                    "maxItems": AuditEventType::ALL.len(),
                    "uniqueItems": true
                },
                "tool": {"type": "string", "minLength": 1, "maxLength": 100},
                "from": {"type": "string", "minLength": 10, "maxLength": 40},
                "to": {"type": "string", "minLength": 10, "maxLength": 40},
                "text": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let types: Vec<String> = args
            .get("types")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut bounds = [None, None];
        for (slot, key) in bounds.iter_mut().zip(["from", "to"]) {
            if let Some(raw) = opt_str(&args, key) {
                let parsed = chrono::DateTime::parse_from_rfc3339(raw).map_err(|_| {
                    ToolError::InvalidArguments {
                        tool: SHOW_AUDIT.to_string(),
                        reasons: format!("`{key}` {raw:?} is not an RFC 3339 timestamp"),
                    }
                })?;
                *slot = Some(parsed.with_timezone(&chrono::Utc).to_rfc3339());
            }
        }
        let [from, to] = bounds;
        let tool = opt_str(&args, "tool").map(str::to_string);
        let text = opt_str(&args, "text").map(str::to_string);
        emit_navigation(
            ctx,
            "settings",
            None,
            Some(NavigationTarget::Audit {
                types: types.clone(),
                tool: tool.clone(),
                from,
                to,
                text,
            }),
        );
        Ok(ToolOutput {
            text_for_model: "The audit log is now open for the user with those filters."
                .to_string(),
            summary_for_ui: "Opened the audit log".to_string(),
            detail: Some(json!({ "types": types, "tool": tool })),
        })
    }
}

/// `show_source`: open an indexed folder in the Library.
pub struct ShowSourceTool {
    rag: Arc<RwLock<RAGEngine>>,
}

impl ShowSourceTool {
    pub fn new(rag: Arc<RwLock<RAGEngine>>) -> Self {
        Self { rag }
    }
}

#[async_trait]
impl HostTool for ShowSourceTool {
    fn name(&self) -> &'static str {
        SHOW_SOURCE
    }
    fn label(&self) -> &'static str {
        "Show source"
    }
    fn label_template(&self) -> &'static str {
        "Showing source {source_id}"
    }
    fn description(&self) -> &'static str {
        "Open an indexed folder source (id from list_sources) in the Library so the user can see \
         its files."
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
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let source_id = req_str(&args, "source_id", SHOW_SOURCE)?;
        let source = {
            let engine = self.rag.read().await;
            find_folder_source(&engine, source_id).await?
        };
        let folder = source
            .folder
            .clone()
            .unwrap_or_else(|| source.source_id.clone());
        emit_navigation(
            ctx,
            "library",
            Some(source.source_id.clone()),
            Some(NavigationTarget::Source {
                source_id: source.source_id.clone(),
            }),
        );
        Ok(ToolOutput {
            text_for_model: format!("{folder} is now shown in the Library."),
            summary_for_ui: format!("Opened {folder} in Library"),
            detail: Some(json!({ "sourceId": source.source_id, "folder": folder })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn ctx() -> (ToolContext, mpsc::UnboundedReceiver<AgentEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (ToolContext::new("run-1", "step-1", tx), rx)
    }

    #[tokio::test]
    async fn show_document_emits_a_document_target() {
        let (ctx, mut rx) = ctx();
        let path = if cfg!(windows) {
            "C:/docs/acme.pdf"
        } else {
            "/docs/acme.pdf"
        };
        let out = ShowDocumentTool
            .execute(
                json!({"path": path, "page": 4, "passage": "notice period"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.summary_for_ui, "Opened acme.pdf at page 4");
        match rx.recv().await.unwrap() {
            AgentEvent::Navigated { view, target, .. } => {
                assert_eq!(view, "ask");
                assert_eq!(
                    target,
                    Some(NavigationTarget::Document {
                        path: path.into(),
                        page: Some(4),
                        passage: Some("notice period".into()),
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        let relative = ShowDocumentTool
            .execute(json!({"path": "acme.pdf"}), &ctx)
            .await;
        assert!(matches!(relative, Err(ToolError::InvalidArguments { .. })));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn show_audit_normalises_bounds_and_rejects_bad_ones() {
        let (ctx, mut rx) = ctx();
        ShowAuditTool
            .execute(
                json!({"types": ["tool_call"], "tool": "web_search", "from": "2026-10-01T00:00:00+05:30"}),
                &ctx,
            )
            .await
            .unwrap();
        match rx.recv().await.unwrap() {
            AgentEvent::Navigated { view, target, .. } => {
                assert_eq!(view, "settings");
                match target {
                    Some(NavigationTarget::Audit {
                        types, tool, from, ..
                    }) => {
                        assert_eq!(types, vec!["tool_call".to_string()]);
                        assert_eq!(tool.as_deref(), Some("web_search"));
                        assert_eq!(from.as_deref(), Some("2026-09-30T18:30:00+00:00"));
                    }
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
        let bad = ShowAuditTool
            .execute(json!({"from": "last tuesday"}), &ctx)
            .await;
        assert!(matches!(bad, Err(ToolError::InvalidArguments { .. })));
    }
}
