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
use crate::audit::AuditLog;
use crate::harness::events::{AgentEvent, NavigationTarget, RiskTier, MAX_NAVIGATION_LABEL_CHARS};
use crate::RAGEngine;

pub const OPEN_VIEW: &str = "open_view";
pub const SHOW_DOCUMENT: &str = "show_document";
pub const SHOW_AUDIT: &str = "show_audit";
pub const SHOW_SOURCE: &str = "show_source";

/// Views the agent may open, in the navbar's order.
pub const VIEWS: [&str; 8] = [
    "ask",
    "artifacts",
    "library",
    "workspaces",
    "connectors",
    "tasks",
    "activity",
    "settings",
];

/// Earlier view ids, still accepted: `calendar` is now `tasks` and `audit`
/// is now `activity`.
pub const VIEW_ALIASES: [(&str, &str); 2] = [("calendar", "tasks"), ("audit", "activity")];

/// The current id of `view`, if it is a view the agent may open.
pub fn normalize_view(view: &str) -> Option<&'static str> {
    VIEWS.iter().copied().find(|v| *v == view).or_else(|| {
        VIEW_ALIASES
            .iter()
            .find(|(old, _)| *old == view)
            .map(|(_, new)| *new)
    })
}

fn accepted_views() -> Vec<&'static str> {
    VIEWS
        .iter()
        .copied()
        .chain(VIEW_ALIASES.iter().map(|(old, _)| *old))
        .collect()
}

/// Longest passage text sent to the viewer for highlighting.
const MAX_PASSAGE_CHARS: usize = 500;

/// What a tool that shows something tells the model happened. The app may
/// hold a navigation while the user types (it then offers it with a "Go
/// there" link), so a tool never claims more than this.
pub const SHOWN_TO_USER: &str =
    "It is shown to the user, or offered to them with a Go there link if they are typing.";

/// Emit a `navigated` event for this call's run.
pub fn emit_navigation(
    ctx: &ToolContext,
    view: &str,
    focus: Option<String>,
    target: Option<NavigationTarget>,
) {
    emit_shown(ctx, view, focus, target, None);
}

/// Emit a `navigated` event naming the item shown (`label`, in the user's
/// words: a task's title, a file and page), clipped to
/// [`MAX_NAVIGATION_LABEL_CHARS`]. Call it only after the item passed the
/// tool's scope checks: the label reaches the screen.
pub fn emit_shown(
    ctx: &ToolContext,
    view: &str,
    focus: Option<String>,
    target: Option<NavigationTarget>,
    label: Option<&str>,
) {
    let label = label
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(clip_label);
    ctx.emit(AgentEvent::Navigated {
        run_id: ctx.run_id.clone(),
        view: view.to_string(),
        focus,
        target,
        label,
    });
}

/// `label` cut to [`MAX_NAVIGATION_LABEL_CHARS`], with an ellipsis when cut.
pub fn clip_label(label: &str) -> String {
    if label.chars().count() <= MAX_NAVIGATION_LABEL_CHARS {
        return label.to_string();
    }
    let kept: String = label.chars().take(MAX_NAVIGATION_LABEL_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
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
        "Switch the app to a view so the user can see a result: ask (conversation), artifacts \
         (what Shodh made: creations, diagrams, charts, tables and snippets), library \
         (indexed folders and their files), connectors (Slack and other connected tools; focus \
         `slack`, `telegram` or `github`, optionally with `:<tab>`, opens that connector's \
         page), tasks (the calendar: tasks and events), activity (usage and the audit log), \
         settings (show_setting opens a section). Optionally focus an item by id, e.g. the id \
         of a task you just created."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "view": {"type": "string", "enum": accepted_views()},
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
        let requested = req_str(&args, "view", OPEN_VIEW)?;
        let view = normalize_view(requested).ok_or_else(|| ToolError::InvalidArguments {
            tool: OPEN_VIEW.to_string(),
            reasons: format!("unknown view {requested}"),
        })?;
        let focus = opt_str(&args, "focus").map(str::to_string);
        emit_navigation(ctx, view, focus.clone(), None);
        Ok(ToolOutput {
            text_for_model: format!("The {view} view: {SHOWN_TO_USER}"),
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
        "Showing[ {path}][ the document of passage {number!}]"
    }
    fn description(&self) -> &'static str {
        "Open a document in the app's viewer for the user, optionally at a page and with a \
         passage highlighted. Name the document by the number of one of its passages from \
         search_documents (`number`; its page is used unless you give one), or by its file \
         name (`path`). This only shows the document; use open_document to read it yourself."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "number": {"type": "integer", "minimum": 1},
                "path": {"type": "string", "minLength": 1, "maxLength": 2048},
                "page": {"type": "integer", "minimum": 1, "maximum": 100000},
                "passage": {"type": "string", "minLength": 1, "maxLength": 2000}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let (path, passage_page) = shown_document(&args, ctx)?;
        let path = path.as_str();
        let page = args
            .get("page")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok())
            .or(passage_page);
        let passage =
            opt_str(&args, "passage").map(|p| crate::harness::truncate_chars(p, MAX_PASSAGE_CHARS));
        let name = file_name(path).to_string();
        let at = page.map(|p| format!(" at page {p}")).unwrap_or_default();
        let label = match page {
            Some(p) => format!("{name}, page {p}"),
            None => name.clone(),
        };
        emit_shown(
            ctx,
            "ask",
            None,
            Some(NavigationTarget::Document {
                path: path.to_string(),
                page,
                passage,
            }),
            Some(&label),
        );
        Ok(ToolOutput {
            text_for_model: format!("{name} in the viewer{at}: {SHOWN_TO_USER}"),
            summary_for_ui: format!("Opened {name}{at}"),
            detail: Some(json!({ "path": path, "page": page })),
        })
    }
}

/// The document `show_document` is asked to show, and the page of the passage it was
/// named by: a passage number of this conversation, a file name one of its passages
/// has, or (as the app itself passes) an absolute path or in-app record. Errors name
/// files, never paths.
fn shown_document(args: &Value, ctx: &ToolContext) -> Result<(String, Option<u32>), ToolError> {
    if let Some(n) = args.get("number").and_then(Value::as_u64) {
        let passage = super::document_of_passage(ctx, n)?;
        let page = passage
            .page
            .as_deref()
            .and_then(|p| p.split(['-', '–', ',']).next())
            .and_then(|p| p.trim().parse::<u32>().ok());
        return Ok((passage.path, page));
    }
    let requested = opt_str(args, "path").ok_or_else(|| ToolError::InvalidArguments {
        tool: SHOW_DOCUMENT.to_string(),
        reasons: "pass `number` (one of its passages) or `path` (its file name)".to_string(),
    })?;
    if requested.contains("://") || std::path::Path::new(requested).is_absolute() {
        return Ok((requested.to_string(), None));
    }
    match super::document_named(ctx, requested)? {
        Some(found) => Ok((found.path, None)),
        None => Err(ToolError::NotFound(format!(
            "{} is not a document this conversation found. Search for it first, then name it \
             by the number of one of its passages.",
            file_name(requested)
        ))),
    }
}

/// `show_audit`: open the audit log with filters, or at one entry.
pub struct ShowAuditTool {
    /// The log, to check an `entry_id` against (none: entries cannot be shown).
    audit: Option<Arc<AuditLog>>,
}

impl ShowAuditTool {
    pub fn new(audit: Option<Arc<AuditLog>>) -> Self {
        Self { audit }
    }

    /// The day filter that shows entry `id`, after checking it exists and, in a
    /// workspace chat, that it is one of this conversation's entries (a chat
    /// limited to a workspace never puts another chat's entries on screen).
    /// Errors name nothing about the entry.
    async fn entry_bounds(&self, ctx: &ToolContext, id: u64) -> Result<String, ToolError> {
        let audit = self
            .audit
            .clone()
            .ok_or_else(|| ToolError::Unavailable("The audit log is not available.".to_string()))?;
        let not_found = || ToolError::NotFound(format!("No Activity entry {id}."));
        let row_id = i64::try_from(id).map_err(|_| not_found())?;
        let row = tokio::task::spawn_blocking(move || audit.entry(row_id))
            .await
            .map_err(|e| ToolError::Failed(format!("Reading the audit log failed: {e}")))?
            .map_err(|e| ToolError::Failed(format!("Reading the audit log failed: {e}")))?
            .ok_or_else(not_found)?;
        if ctx.scope().restricted {
            let this = ctx.audit_scope().map(|s| s.conversation_id.as_str());
            if row.conversation_id.as_deref() != this || this.is_none() {
                return Err(ToolError::Forbidden(
                    "In a workspace chat only this conversation's own Activity entries can be \
                     shown."
                        .to_string(),
                ));
            }
        }
        Ok(row.ts)
    }
}

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
        "Open the activity (usage and audit) log for the user with filters applied: event types, a tool \
         name, a time range (RFC 3339 from/to) and free text. Or give only `entry_id` (an id \
         from audit_query) to open the log at that entry, expanded and highlighted. Use \
         audit_query to read the log yourself."
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
                "text": {"type": "string", "minLength": 1, "maxLength": 200},
                "entry_id": {"type": "integer", "minimum": 1}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if let Some(id) = args.get("entry_id").and_then(Value::as_u64) {
            if ["types", "tool", "from", "to", "text"]
                .iter()
                .any(|k| args.get(*k).is_some())
            {
                return Err(ToolError::InvalidArguments {
                    tool: SHOW_AUDIT.to_string(),
                    reasons: "`entry_id` shows one entry; give it without filters".to_string(),
                });
            }
            let ts = self.entry_bounds(ctx, id).await?;
            // The entry's moment as the filter: the log opens on that day with the entry
            // among the rows it loads.
            emit_shown(
                ctx,
                "activity",
                None,
                Some(NavigationTarget::Audit {
                    types: Vec::new(),
                    tool: None,
                    from: Some(ts.clone()),
                    to: Some(ts),
                    text: None,
                    entry_id: Some(id),
                }),
                Some(&format!("entry {id}")),
            );
            return Ok(ToolOutput {
                text_for_model: format!("Activity entry {id}: {SHOWN_TO_USER}"),
                summary_for_ui: format!("Opened Activity entry {id}"),
                detail: Some(json!({ "entryId": id })),
            });
        }
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
            "activity",
            None,
            Some(NavigationTarget::Audit {
                types: types.clone(),
                tool: tool.clone(),
                from,
                to,
                text,
                entry_id: None,
            }),
        );
        Ok(ToolOutput {
            text_for_model: format!("The audit log with those filters: {SHOWN_TO_USER}"),
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
        emit_shown(
            ctx,
            "library",
            Some(source.source_id.clone()),
            Some(NavigationTarget::Source {
                source_id: source.source_id.clone(),
            }),
            Some(&source.name),
        );
        Ok(ToolOutput {
            // The folder's name only: its path never reaches the model.
            text_for_model: format!(
                "The folder \"{}\" in the Library: {SHOWN_TO_USER}",
                source.name
            ),
            summary_for_ui: format!("Opened {folder} in Library"),
            detail: Some(json!({ "sourceId": source.source_id, "folder": folder })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::tools::CitedPassage;
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
        // A file name this conversation has not found is refused, naming only the file.
        match ShowDocumentTool
            .execute(json!({"path": "acme.pdf"}), &ctx)
            .await
        {
            Err(ToolError::NotFound(message)) => {
                assert!(
                    message.starts_with("acme.pdf is not a document"),
                    "{message}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(rx.try_recv().is_err());

        // By a passage number, at the passage's page, or by the file name it has.
        let n = ctx.cite_passage(CitedPassage {
            n: 0,
            file: "acme.pdf".to_string(),
            path: path.to_string(),
            page: Some("7".to_string()),
            web: false,
            text: "Notice is sixty days.".to_string(),
            checkable: true,
        });
        let out = ShowDocumentTool
            .execute(json!({"number": n}), &ctx)
            .await
            .unwrap();
        assert_eq!(out.summary_for_ui, "Opened acme.pdf at page 7");
        assert!(!out.text_for_model.contains(path));
        match rx.recv().await.unwrap() {
            AgentEvent::Navigated { target, .. } => assert_eq!(
                target,
                Some(NavigationTarget::Document {
                    path: path.into(),
                    page: Some(7),
                    passage: None,
                })
            ),
            other => panic!("{other:?}"),
        }
        let by_name = ShowDocumentTool
            .execute(json!({"path": "ACME.pdf", "page": 2}), &ctx)
            .await
            .unwrap();
        assert_eq!(by_name.summary_for_ui, "Opened acme.pdf at page 2");
        assert!(matches!(
            ShowDocumentTool.execute(json!({"number": 99}), &ctx).await,
            Err(ToolError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn show_audit_normalises_bounds_and_rejects_bad_ones() {
        let (ctx, mut rx) = ctx();
        let tool = ShowAuditTool::new(None);
        let out = tool
            .execute(
                json!({"types": ["tool_call"], "tool": "web_search", "from": "2026-10-01T00:00:00+05:30"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.text_for_model.ends_with(SHOWN_TO_USER));
        match rx.recv().await.unwrap() {
            AgentEvent::Navigated { view, target, .. } => {
                assert_eq!(view, "activity");
                match target {
                    Some(NavigationTarget::Audit {
                        types,
                        tool,
                        from,
                        entry_id,
                        ..
                    }) => {
                        assert_eq!(entry_id, None);
                        assert_eq!(types, vec!["tool_call".to_string()]);
                        assert_eq!(tool.as_deref(), Some("web_search"));
                        assert_eq!(from.as_deref(), Some("2026-09-30T18:30:00+00:00"));
                    }
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
        let bad = tool.execute(json!({"from": "last tuesday"}), &ctx).await;
        assert!(matches!(bad, Err(ToolError::InvalidArguments { .. })));
    }

    /// An entry is shown only when it exists, alone (a filter could hide it), and in a
    /// workspace chat only when it is this conversation's. The log opens on the entry's
    /// moment, so the entry is among the rows it loads.
    #[tokio::test]
    async fn show_audit_opens_an_entry_only_after_checking_it() {
        use crate::audit::AuditRecord;
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(AuditLog::open(dir.path().join("shodh.db"), None).unwrap());
        let id = log
            .append(
                AuditRecord::new(AuditEventType::Question, json!({"text": "notice period"}))
                    .conversation("c1"),
            )
            .unwrap();
        let ts = log.entry(id).unwrap().unwrap().ts;
        let tool = ShowAuditTool::new(Some(log));
        let (ctx, mut rx) = ctx();
        let entry = u64::try_from(id).unwrap();
        let out = tool
            .execute(json!({ "entry_id": entry }), &ctx)
            .await
            .unwrap();
        assert!(!out.text_for_model.contains("notice period"));
        match rx.try_recv().unwrap() {
            AgentEvent::Navigated { target, label, .. } => {
                assert_eq!(
                    target,
                    Some(NavigationTarget::Audit {
                        types: Vec::new(),
                        tool: None,
                        from: Some(ts.clone()),
                        to: Some(ts),
                        text: None,
                        entry_id: Some(entry),
                    })
                );
                assert_eq!(label.as_deref(), Some(format!("entry {entry}").as_str()));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            tool.execute(json!({ "entry_id": entry + 100 }), &ctx).await,
            Err(ToolError::NotFound(_))
        ));
        assert!(matches!(
            tool.execute(json!({ "entry_id": entry, "text": "x" }), &ctx)
                .await,
            Err(ToolError::InvalidArguments { .. })
        ));
        // A workspace chat that is not the entry's conversation cannot show it.
        let restricted = ctx.clone().with_scope(Arc::new(super::super::RunScope {
            restricted: true,
            ..Default::default()
        }));
        assert!(matches!(
            tool.execute(json!({ "entry_id": entry }), &restricted)
                .await,
            Err(ToolError::Forbidden(_))
        ));
        assert!(matches!(
            ShowAuditTool::new(None)
                .execute(json!({ "entry_id": entry }), &ctx)
                .await,
            Err(ToolError::Unavailable(_))
        ));
        assert!(rx.try_recv().is_err(), "a refused call shows nothing");
    }

    #[test]
    fn labels_are_clipped_for_the_shell() {
        assert_eq!(clip_label("Renew lease"), "Renew lease");
        let long = "a".repeat(80);
        let clipped = clip_label(&long);
        assert_eq!(clipped.chars().count(), MAX_NAVIGATION_LABEL_CHARS);
        assert!(clipped.ends_with('…'));
    }
}
