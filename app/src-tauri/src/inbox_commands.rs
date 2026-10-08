//! The Inbox: managed state, the commands of the Inbox panel, and [`post`] /
//! [`resolve`] for the parts of the app that create items (approvals in the agent
//! event forwarder, background work when it finishes).
//!
//! Items live in `<app_data_dir>/shodh.db` (see `shodh_rag::inbox`); the store opens
//! on first use with the database path and key the audit log was opened with. Pending
//! memory suggestions are not stored here: [`inbox_list`] reads them from the learner
//! and merges them in. Every change is broadcast as [`INBOX_CHANGED_EVENT`].
//!
//! Desktop notifications (tauri-plugin-notification 2.3) carry a title and a body only
//! on Windows: no action buttons, and a click does not reach the app (see
//! `reminders.rs`). An approval that arrives while the window is not focused is
//! announced that way; Approve / Deny live in the Inbox and in the chat.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use shodh_rag::audit::AuditKey;
use shodh_rag::inbox::{
    aggregate, InboxItem, InboxKind, InboxLink, InboxResult, InboxStatus, InboxStore, NewInboxItem,
};
use shodh_rag::user_memory::learn::{ProposalAction, ProposalStatus, ProposalView};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::OnceCell;

use crate::audit_commands::AuditState;
use crate::calendar_store::{CalendarData, CalendarStore};
use crate::memory_learn::LearnState;

/// Emitted (no payload) after any Inbox item was added, changed or removed.
pub const INBOX_CHANGED_EVENT: &str = "inbox-changed";

/// Most pending memory suggestions shown in the Inbox (the Memory page lists all).
const MAX_MEMORY_ITEMS: usize = 50;

/// Managed state: the Inbox store, opened on first use.
#[derive(Clone)]
pub struct InboxState {
    inner: Arc<Inner>,
}

struct Inner {
    store: OnceCell<Arc<InboxStore>>,
    database: Option<(PathBuf, Option<AuditKey>)>,
}

impl InboxState {
    /// State over the database the audit log opened (`None` when it could not be opened).
    pub fn new(audit: &AuditState) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: OnceCell::new(),
                database: audit.database(),
            }),
        }
    }

    async fn store(&self) -> Result<Arc<InboxStore>, String> {
        self.inner
            .store
            .get_or_try_init(|| async {
                let (path, key) = self.inner.database.clone().ok_or_else(|| {
                    "The Inbox needs the app database (shodh.db), which could not be opened; \
                     see the audit page."
                        .to_string()
                })?;
                let store =
                    tokio::task::spawn_blocking(move || InboxStore::open(&path, key.as_ref()))
                        .await
                        .map_err(|e| format!("Opening the Inbox failed: {e}"))?
                        .map_err(|e| e.to_string())?;
                Ok(Arc::new(store))
            })
            .await
            .cloned()
    }

    /// Runs `call` against the store on the blocking pool.
    async fn run<T, F>(&self, call: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&InboxStore) -> InboxResult<T> + Send + 'static,
    {
        let store = self.store().await?;
        tokio::task::spawn_blocking(move || call(&store))
            .await
            .map_err(|e| format!("The Inbox task failed: {e}"))?
            .map_err(|e| e.to_string())
    }
}

fn broadcast(app: &AppHandle) {
    if let Err(e) = app.emit(INBOX_CHANGED_EVENT, ()) {
        tracing::warn!(target: "shodh::inbox", error = %e, "inbox change not broadcast");
    }
}

/// Adds or updates an item and tells the Inbox. Failures are logged: the Inbox is a
/// view of work that already happened and never fails that work.
pub async fn post(app: &AppHandle, item: NewInboxItem) {
    let state = app.state::<InboxState>().inner().clone();
    let id = item.id.clone();
    match state.run(move |store| store.put(&item, Utc::now())).await {
        Ok(_) => broadcast(app),
        Err(e) => {
            tracing::warn!(target: "shodh::inbox", id = %id, error = %e, "inbox item not saved")
        }
    }
}

/// Removes an item (it was answered elsewhere) and tells the Inbox.
pub async fn resolve(app: &AppHandle, id: String) {
    let state = app.state::<InboxState>().inner().clone();
    match state.run(move |store| store.remove(&id)).await {
        Ok(true) => broadcast(app),
        Ok(false) => {}
        Err(e) => tracing::warn!(target: "shodh::inbox", error = %e, "inbox item not removed"),
    }
}

/// Removes the approvals of a session (its run ended or it closed).
pub async fn resolve_session_approvals(app: &AppHandle, session_id: &str) {
    let state = app.state::<InboxState>().inner().clone();
    let prefix = approval_prefix(session_id);
    match state
        .run(move |store| store.remove_prefixed(InboxKind::Approval, &prefix))
        .await
    {
        Ok(0) => {}
        Ok(_) => broadcast(app),
        Err(e) => tracing::warn!(target: "shodh::inbox", error = %e, "inbox approvals not removed"),
    }
}

/// On launch: drop the previous run's approvals, mark interrupted work, prune.
pub fn recover_on_launch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<InboxState>().inner().clone();
        match state.run(|store| store.recover(Utc::now())).await {
            Ok(0) => {}
            Ok(_) => broadcast(&app),
            Err(e) => tracing::warn!(target: "shodh::inbox", error = %e, "inbox not recovered"),
        }
    });
}

fn approval_prefix(session_id: &str) -> String {
    format!("approval:{session_id}:")
}

/// Open shows the conversation.
pub fn conversation_link(conversation_id: &str) -> InboxLink {
    InboxLink::new(
        "ask",
        Some(json!({ "kind": "conversation", "conversationId": conversation_id })),
    )
}

/// Open shows the file in the Library's viewer.
pub fn document_link(path: &str) -> InboxLink {
    InboxLink::new("library", Some(json!({ "kind": "document", "path": path })))
}

/// The Inbox item of an export: ready (Open shows the file) or failed (Open shows the
/// conversation that asked for it).
pub fn export_item(
    key: &str,
    ok: bool,
    summary: &str,
    path: Option<&str>,
    conversation_id: &str,
) -> NewInboxItem {
    let (status, title, link) = match (ok, path) {
        (true, Some(path)) => (InboxStatus::Done, "Export ready", document_link(path)),
        _ => (
            InboxStatus::Failed,
            "The export failed",
            conversation_link(conversation_id),
        ),
    };
    work_item(
        format!("export:{key}"),
        InboxKind::Export,
        status,
        title,
        Some(path.map_or_else(|| summary.to_string(), str::to_string)),
        Some(link),
    )
}

/// An item of background work (indexing, tables, the graph, an export, an install).
pub fn work_item(
    id: String,
    kind: InboxKind,
    status: InboxStatus,
    title: impl Into<String>,
    detail: Option<String>,
    link: Option<InboxLink>,
) -> NewInboxItem {
    NewInboxItem {
        id,
        kind,
        status,
        title: title.into(),
        detail,
        link,
        data: json!({}),
    }
}

/// Inbox id of an approval.
pub fn approval_id(session_id: &str, step_id: &str) -> String {
    format!("{}{step_id}", approval_prefix(session_id))
}

/// The Inbox item of an approval the assistant is waiting for.
pub fn approval_item(
    session_id: &str,
    conversation_id: &str,
    step_id: &str,
    label: &str,
    tool: &str,
) -> NewInboxItem {
    NewInboxItem {
        id: approval_id(session_id, step_id),
        kind: InboxKind::Approval,
        status: InboxStatus::NeedsYou,
        title: if label.trim().is_empty() {
            format!("Approve {tool}?")
        } else {
            label.to_string()
        },
        detail: Some(format!("The assistant waits for your approval ({tool}).")),
        link: Some(conversation_link(conversation_id)),
        data: json!({ "sessionId": session_id, "stepId": step_id }),
    }
}

/// Shows a desktop notification for an approval when the main window is not focused
/// (minimised, hidden in the tray, or behind another window).
pub fn notify_if_unfocused(app: &AppHandle, title: &str) {
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    if let Err(e) = app
        .notification()
        .builder()
        .title("Shodh needs your approval")
        .body(title)
        .show()
    {
        tracing::warn!(target: "shodh::inbox", error = %e, "approval notification not shown");
    }
}

/// The Inbox item of a pending memory suggestion.
fn memory_item(view: &ProposalView) -> InboxItem {
    let title = match &view.action {
        ProposalAction::Remember { text, .. } => format!("Remember: {text}"),
        ProposalAction::Revise { text, .. } => format!("Update memory: {text}"),
        ProposalAction::Link { a_text, b_text, .. } => {
            format!("Link memories: “{a_text}” and “{b_text}”")
        }
        ProposalAction::Resolve {
            keep_text,
            retire_text,
            ..
        } => format!("Keep “{keep_text}” over “{retire_text}”"),
        ProposalAction::Archive { text, .. } => format!("Archive faded memory: {text}"),
    };
    let at = view
        .created_at
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    InboxItem {
        id: format!("memory:{}", view.id),
        kind: InboxKind::Memory,
        status: InboxStatus::NeedsYou,
        title,
        detail: Some("Memory suggestion".to_string()),
        link: Some(InboxLink::new("settings", None)),
        data: json!({ "suggestionId": view.id, "conversationId": view.conversation_id }),
        created_at: at.clone(),
        updated_at: at,
    }
}

async fn pending_memory_items(learn: &LearnState) -> Vec<InboxItem> {
    // Learning off or its store unavailable: no suggestions to show, not an error.
    let Ok(learner) = learn.learner().await else {
        return Vec::new();
    };
    match learner.list(&[ProposalStatus::Pending], MAX_MEMORY_ITEMS) {
        Ok(views) => views.iter().map(memory_item).collect(),
        Err(e) => {
            tracing::warn!(target: "shodh::inbox", error = %e, "memory suggestions not read");
            Vec::new()
        }
    }
}

/// Everything in the Inbox: stored items and pending memory suggestions, in order.
#[tauri::command]
pub async fn inbox_list(
    app: AppHandle,
    inbox: State<'_, InboxState>,
    learn: State<'_, LearnState>,
) -> Result<Vec<InboxItem>, String> {
    let mut stored = inbox.run(|store| store.list()).await?;
    // A reminder whose task was done or deleted since it rang waits on nothing.
    if stored.iter().any(|i| i.kind == InboxKind::Reminder) {
        if let Some(calendar) = read_calendar(&app).await {
            let settled = settled_reminders(&stored, &calendar);
            if !settled.is_empty() {
                stored.retain(|i| !settled.contains(&i.id));
                inbox
                    .run(move |store| {
                        for id in &settled {
                            store.remove(id)?;
                        }
                        Ok(())
                    })
                    .await?;
            }
        }
    }
    let live = pending_memory_items(&learn).await;
    Ok(aggregate(stored, live))
}

/// The calendar, or `None` when it cannot be read (reminders are then kept).
async fn read_calendar(app: &AppHandle) -> Option<CalendarData> {
    let dir = crate::profile::app_data_dir(app).ok()?;
    let store = CalendarStore::in_dir(&dir);
    match tokio::task::spawn_blocking(move || store.load()).await {
        Ok(Ok(data)) => Some(data),
        Ok(Err(e)) => {
            tracing::debug!(target: "shodh::inbox", error = %e, "calendar not read");
            None
        }
        Err(_) => None,
    }
}

/// Ids of reminder items whose task is completed or no longer exists.
fn settled_reminders(items: &[InboxItem], calendar: &CalendarData) -> Vec<String> {
    items
        .iter()
        .filter(|item| item.kind == InboxKind::Reminder)
        .filter(|item| {
            let task_id = item.data.get("taskId").and_then(|t| t.as_str());
            !calendar
                .tasks
                .iter()
                .any(|t| Some(t.id.as_str()) == task_id && t.status != "completed")
        })
        .map(|item| item.id.clone())
        .collect()
}

/// Removes a stored item (Dismiss on a finished item or a reminder). True when it was
/// there. Memory suggestions are dismissed by rejecting them.
#[tauri::command]
pub async fn inbox_dismiss(
    id: String,
    app: AppHandle,
    inbox: State<'_, InboxState>,
) -> Result<bool, String> {
    let removed = inbox.run(move |store| store.remove(&id)).await?;
    if removed {
        broadcast(&app);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_of_a_session_share_a_prefix_no_other_session_has() {
        let item = approval_item("s1", "conv-1", "step-9", "Write notes.md", "write_file");
        assert_eq!(item.id, "approval:s1:step-9");
        assert!(item.id.starts_with(&approval_prefix("s1")));
        assert!(!approval_id("s11", "x").starts_with(&approval_prefix("s1")));
        assert_eq!(item.status, InboxStatus::NeedsYou);
        assert_eq!(
            item.link,
            Some(InboxLink::new(
                "ask",
                Some(json!({ "kind": "conversation", "conversationId": "conv-1" }))
            ))
        );
        assert_eq!(item.data["stepId"], "step-9");
        let unnamed = approval_item("s1", "c", "a", " ", "run_command");
        assert_eq!(unnamed.title, "Approve run_command?");
    }

    #[test]
    fn reminders_of_done_or_deleted_tasks_are_settled() {
        let task = |id: &str, status: &str| {
            serde_json::from_value::<crate::calendar_store::TodoItem>(json!({
                "id": id, "title": id, "status": status, "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z"
            }))
            .unwrap()
        };
        let calendar = CalendarData {
            tasks: vec![task("open", "pending"), task("done", "completed")],
            events: Vec::new(),
        };
        let reminder = |task_id: &str| InboxItem {
            id: format!("reminder:{task_id}"),
            kind: InboxKind::Reminder,
            status: InboxStatus::NeedsYou,
            title: task_id.into(),
            detail: None,
            link: None,
            data: json!({ "taskId": task_id }),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let mut export = reminder("open");
        export.id = "export:1".into();
        export.kind = InboxKind::Export;
        let items = vec![
            reminder("open"),
            reminder("done"),
            reminder("deleted"),
            export,
        ];
        assert_eq!(
            settled_reminders(&items, &calendar),
            ["reminder:done", "reminder:deleted"]
        );
    }

    #[test]
    fn an_export_opens_its_file_when_ready_and_its_chat_when_it_failed() {
        let ready = export_item("s1:e1", true, "Saved a.pdf", Some("C:/out/a.pdf"), "c1");
        assert_eq!(ready.id, "export:s1:e1");
        assert_eq!(ready.status, InboxStatus::Done);
        assert_eq!(ready.link, Some(document_link("C:/out/a.pdf")));
        assert_eq!(ready.detail.as_deref(), Some("C:/out/a.pdf"));
        let failed = export_item("s1:e2", false, "No folder", None, "c1");
        assert_eq!(failed.status, InboxStatus::Failed);
        assert_eq!(failed.link, Some(conversation_link("c1")));
        assert_eq!(failed.detail.as_deref(), Some("No folder"));
    }
}
