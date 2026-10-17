//! Desktop notifications: the one service every part of the app uses to tell
//! the user something while Shodh is not in front.
//!
//! **Categories** ([`Category`]), each switched in Settings → Notifications
//! ([`NotificationPrefs`], user-only):
//! - reminders: a task reminder rang, or reminders were missed while Shodh
//!   was closed;
//! - needs you: the assistant waits for an approval; memory suggestions are
//!   a separate switch inside it, off by default;
//! - work finished: an answer, a folder's indexing or sync, or a result
//!   extraction finished;
//! - problems: indexing or a sync failed, a background model call for the
//!   app's own work failed (learning memories), or a data file had to be
//!   restored or reset.
//!
//! **When**: only while the main window is not focused, minimised or hidden
//! in the tray ([`should_alert`]). While the user looks at the window, the
//! in-app toasts, banners and the Inbox already say it; a desktop
//! notification on top would be a second alert for the same thing.
//!
//! **Inbox**: reminders, approvals, memory suggestions and indexing results
//! already have Inbox items, which stay the lasting record (posted by their
//! senders as before). Finished answers and result extractions get none: the
//! conversation and the paper show them, and an item per answer would bury
//! the items that wait for the user. Problems of data files and of learning
//! are shown in the window (store notices, the memory page) instead.
//!
//! **Rate**: work-finished notifications are at most one a minute
//! ([`WorkBatch`]): the first is shown at once, what finishes in the next
//! minute is shown together at its end ("3 folders finished indexing"), so
//! one source is never announced twice within a minute. A problem from the
//! same source is shown at most once in [`PROBLEM_COOLDOWN`].
//!
//! **Text**: a short title and a body naming what it is about (a task, a
//! folder, a paper, a conversation title) and counts; never document text,
//! answer text, memory text or error messages, which can quote either.
//!
//! **Delivery** (tauri-plugin-notification 2.3 on desktop): title and body
//! only. A click does not reach the app, so no notification offers an
//! action. The plugin hands the notification to the system on a background
//! task and drops that task's result: a failure it reports synchronously is
//! logged at warn; a toast the system blocks is not seen by the app, which is
//! why Settings has a test button and a link to the system's settings.
//!
//! **Automations** (and any later sender) call [`notify`] with a category,
//! a title and a body; the gating above applies.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::harness::{AgentEvent, RunStatus};
use shodh_rag::inbox::{InboxKind, InboxStatus, NewInboxItem};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use crate::app_settings::{broadcast, AppSettings, NotificationPrefs, SettingsStore};
use crate::audit_commands::AuditState;
use crate::background::MAIN_WINDOW;
use crate::conversation_commands::ConversationStore;

/// What a notification is about; each has its switch in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Reminders,
    NeedsYou,
    /// Part of "needs you", with its own switch.
    MemorySuggestions,
    WorkFinished,
    Problems,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Reminders => "reminders",
            Category::NeedsYou => "needs_you",
            Category::MemorySuggestions => "memory_suggestions",
            Category::WorkFinished => "work_finished",
            Category::Problems => "problems",
        }
    }
}

/// Whether the user's settings allow `category`.
pub fn allows(prefs: &NotificationPrefs, category: Category) -> bool {
    match category {
        Category::Reminders => prefs.reminders,
        Category::NeedsYou => prefs.needs_you,
        Category::MemorySuggestions => prefs.needs_you && prefs.memory_suggestions,
        Category::WorkFinished => prefs.work_finished,
        Category::Problems => prefs.problems,
    }
}

/// Where the main window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    /// Shown and focused: the user is looking at Shodh.
    Focused,
    /// Shown, behind another window.
    Unfocused,
    /// Minimised, hidden in the tray, or not open.
    Hidden,
}

/// Whether a notification of `category` is shown now.
pub fn should_alert(prefs: &NotificationPrefs, category: Category, window: WindowState) -> bool {
    allows(prefs, category) && window != WindowState::Focused
}

/// The main window's state. A state that cannot be read counts as not
/// focused: a notification too many is better than a reminder missed.
fn window_state(app: &AppHandle) -> WindowState {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return WindowState::Hidden;
    };
    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    if !visible || minimized {
        return WindowState::Hidden;
    }
    if window.is_focused().unwrap_or(false) {
        WindowState::Focused
    } else {
        WindowState::Unfocused
    }
}

/// Longest body text taken from a name or label.
const MAX_NAME_CHARS: usize = 80;

/// `text` on one line, at most [`MAX_NAME_CHARS`] characters.
pub fn one_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= MAX_NAME_CHARS {
        return joined;
    }
    let cut: String = joined.chars().take(MAX_NAME_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// A notification's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

// ── Work finished: one a minute, batched ─────────────────────────

/// How long after a work-finished notification the next one waits; what
/// finishes meanwhile is shown together.
pub const WORK_WINDOW: Duration = Duration::from_secs(60);

/// What kind of work finished (for the batched text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    /// A folder was indexed or synced.
    Indexing,
    /// Results were extracted from a paper.
    Results,
    /// An answer finished (or stopped with an error).
    Answer,
}

impl WorkKind {
    fn counted(self, n: usize) -> String {
        match (self, n) {
            (WorkKind::Indexing, 1) => "1 folder finished indexing".to_string(),
            (WorkKind::Indexing, n) => format!("{n} folders finished indexing"),
            (WorkKind::Results, 1) => "Results extracted from 1 paper".to_string(),
            (WorkKind::Results, n) => format!("Results extracted from {n} papers"),
            (WorkKind::Answer, 1) => "1 answer finished".to_string(),
            (WorkKind::Answer, n) => format!("{n} answers finished"),
        }
    }
}

/// One piece of finished work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkDone {
    /// What finished (an Inbox id, a paper, a conversation): within a batch
    /// only its latest outcome is kept.
    pub source: String,
    pub kind: WorkKind,
    /// Shown when it is the only one.
    pub notice: Notice,
    /// Names it in a batch ("Papers is indexed").
    pub summary: String,
}

/// What to do with finished work now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkDecision {
    /// Show this now.
    Show(Notice),
    /// Held for the batch; `flush_in` is set when the caller must flush the
    /// batch after that long (once per batch).
    Hold { flush_in: Option<Duration> },
}

/// The pacing of work-finished notifications. Shows happen only when no
/// notification was shown in the last [`WORK_WINDOW`] (at once) or at the
/// end of a window (the batch), so two are never less than a window apart
/// and one source is never announced twice within a minute.
#[derive(Debug, Default)]
pub struct WorkBatch {
    window_until: Option<Instant>,
    pending: Vec<WorkDone>,
}

impl WorkBatch {
    pub fn add(&mut self, now: Instant, done: WorkDone) -> WorkDecision {
        let open = self.window_until.is_some_and(|until| now < until);
        // A batch waiting for its flush keeps collecting even if its window
        // just ended, so it is shown once, after the flush.
        if !open && self.pending.is_empty() {
            self.window_until = Some(now + WORK_WINDOW);
            return WorkDecision::Show(done.notice);
        }
        let first = self.pending.is_empty();
        match self.pending.iter_mut().find(|p| p.source == done.source) {
            Some(existing) => *existing = done,
            None => self.pending.push(done),
        }
        let flush_in = first.then(|| {
            self.window_until
                .map_or(Duration::ZERO, |until| until.saturating_duration_since(now))
        });
        WorkDecision::Hold { flush_in }
    }

    /// The batch to show at the end of the window, if anything waits. A new
    /// window starts with it.
    pub fn flush(&mut self, now: Instant) -> Option<Notice> {
        if self.pending.is_empty() {
            return None;
        }
        let items = std::mem::take(&mut self.pending);
        self.window_until = Some(now + WORK_WINDOW);
        Some(batch_notice(items))
    }
}

/// Most names listed in a batch's body.
const MAX_BATCH_NAMES: usize = 3;

fn batch_notice(mut items: Vec<WorkDone>) -> Notice {
    if items.len() == 1 {
        return items.remove(0).notice;
    }
    let kinds: Vec<WorkKind> = items.iter().fold(Vec::new(), |mut kinds, item| {
        if !kinds.contains(&item.kind) {
            kinds.push(item.kind);
        }
        kinds
    });
    let title = match kinds.as_slice() {
        [kind] => kind.counted(items.len()),
        _ => kinds
            .iter()
            .map(|kind| kind.counted(items.iter().filter(|i| i.kind == *kind).count()))
            .collect::<Vec<_>>()
            .join(", "),
    };
    let mut names: Vec<String> = items
        .iter()
        .take(MAX_BATCH_NAMES)
        .map(|i| i.summary.clone())
        .collect();
    if items.len() > MAX_BATCH_NAMES {
        names.push(format!("{} more", items.len() - MAX_BATCH_NAMES));
    }
    Notice {
        title,
        body: names.join(" · "),
    }
}

// ── Problems: once per source in a while ─────────────────────────

/// A problem from the same source is shown at most once in this long (a
/// provider that is down would otherwise fail every turn's learning).
pub const PROBLEM_COOLDOWN: Duration = Duration::from_secs(10 * 60);

/// When each source last showed a problem.
#[derive(Debug, Default)]
pub struct ProblemGate {
    last: HashMap<String, Instant>,
}

impl ProblemGate {
    /// Whether a problem from `source` may be shown at `now` (recorded when so).
    pub fn admit(&mut self, now: Instant, source: &str) -> bool {
        self.last
            .retain(|_, at| now.saturating_duration_since(*at) < PROBLEM_COOLDOWN);
        if self.last.contains_key(source) {
            return false;
        }
        self.last.insert(source.to_string(), now);
        true
    }
}

// ── State and delivery ───────────────────────────────────────────

/// Managed state: the settings (kept here, so a sender never reads the
/// settings file, which can be locked by the code that reports a problem)
/// and the pacing.
pub struct NotificationState {
    prefs: Mutex<NotificationPrefs>,
    work: Mutex<WorkBatch>,
    problems: Mutex<ProblemGate>,
}

impl NotificationState {
    pub fn new(prefs: NotificationPrefs) -> Self {
        Self {
            prefs: Mutex::new(prefs),
            work: Mutex::new(WorkBatch::default()),
            problems: Mutex::new(ProblemGate::default()),
        }
    }

    /// State with the saved settings of `data_dir` (the defaults when they
    /// cannot be read).
    pub fn load(data_dir: &std::path::Path) -> Self {
        let prefs = match SettingsStore::in_dir(data_dir).load() {
            Ok(settings) => settings.notifications,
            Err(e) => {
                tracing::warn!(target: "shodh::notify", error = %e, "settings unreadable; default notification settings apply");
                NotificationPrefs::default()
            }
        };
        Self::new(prefs)
    }

    pub fn prefs(&self) -> NotificationPrefs {
        *self.prefs.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_prefs(&self, prefs: NotificationPrefs) {
        *self.prefs.lock().unwrap_or_else(|e| e.into_inner()) = prefs;
    }
}

/// The settings changed (called by [`crate::app_settings::broadcast`]).
pub fn settings_changed(app: &AppHandle, prefs: &NotificationPrefs) {
    if let Some(state) = app.try_state::<NotificationState>() {
        state.set_prefs(*prefs);
    }
}

/// Where notifications go: the system's notification centre, or a recorder
/// in tests.
pub trait Deliver {
    fn show(&self, title: &str, body: &str) -> Result<(), String>;
}

/// The desktop, through tauri-plugin-notification.
struct Desktop<'a>(&'a AppHandle);

impl Deliver for Desktop<'_> {
    fn show(&self, title: &str, body: &str) -> Result<(), String> {
        self.0
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())
    }
}

/// Hands `notice` to `sink`; a failure is logged at warn and returned.
fn deliver(sink: &dyn Deliver, label: &str, notice: &Notice) -> Result<(), String> {
    sink.show(&notice.title, &notice.body).map_err(|e| {
        tracing::warn!(target: "shodh::notify", category = label, error = %e, "desktop notification not shown");
        e
    })
}

/// Runs `job` off the async runtime's workers and off the caller's locks:
/// reading the window's state waits for the main thread, which may itself
/// wait for a lock the caller holds (a data file reporting a problem).
fn in_background(app: &AppHandle, job: impl FnOnce(&AppHandle) + Send + 'static) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || job(&app));
}

/// Shows `title` / `body` as a notification of `category` if the user's
/// settings allow it and the main window is not focused. The entry point for
/// every sender, automations included; returns at once.
pub fn notify(
    app: &AppHandle,
    category: Category,
    title: impl Into<String>,
    body: impl Into<String>,
) {
    let notice = Notice {
        title: title.into(),
        body: body.into(),
    };
    in_background(app, move |app| {
        let Some(state) = app.try_state::<NotificationState>() else {
            tracing::debug!(target: "shodh::notify", "notifications not ready; not shown");
            return;
        };
        if should_alert(&state.prefs(), category, window_state(app)) {
            let _ = deliver(&Desktop(app), category.as_str(), &notice);
        }
    });
}

/// A problem from `source` (a folder, a store, the learner): shown as
/// [`Category::Problems`], at most once per [`PROBLEM_COOLDOWN`] per source.
pub fn problem(
    app: &AppHandle,
    source: impl Into<String>,
    title: impl Into<String>,
    body: impl Into<String>,
) {
    let source = source.into();
    let notice = Notice {
        title: title.into(),
        body: body.into(),
    };
    in_background(app, move |app| {
        let Some(state) = app.try_state::<NotificationState>() else {
            return;
        };
        let window = window_state(app);
        if !should_alert(&state.prefs(), Category::Problems, window) {
            return;
        }
        let admitted = state
            .problems
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(Instant::now(), &source);
        if admitted {
            let _ = deliver(&Desktop(app), Category::Problems.as_str(), &notice);
        }
    });
}

/// Finished work: shown as [`Category::WorkFinished`], paced by [`WorkBatch`].
pub fn work_finished(app: &AppHandle, done: WorkDone) {
    in_background(app, move |app| submit_work(app, done));
}

fn submit_work(app: &AppHandle, done: WorkDone) {
    let Some(state) = app.try_state::<NotificationState>() else {
        return;
    };
    if !should_alert(&state.prefs(), Category::WorkFinished, window_state(app)) {
        return;
    }
    let decision = state
        .work
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .add(Instant::now(), done);
    match decision {
        WorkDecision::Show(notice) => {
            let _ = deliver(&Desktop(app), Category::WorkFinished.as_str(), &notice);
        }
        WorkDecision::Hold {
            flush_in: Some(delay),
        } => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(delay).await;
                in_background(&app, flush_work);
            });
        }
        WorkDecision::Hold { flush_in: None } => {}
    }
}

/// End of a batch window: shows the batch unless the user is back at the
/// window (or turned these off), when the window and the Inbox show it.
fn flush_work(app: &AppHandle) {
    let Some(state) = app.try_state::<NotificationState>() else {
        return;
    };
    let window = window_state(app);
    let batch = state
        .work
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .flush(Instant::now());
    let Some(notice) = batch else {
        return;
    };
    if should_alert(&state.prefs(), Category::WorkFinished, window) {
        let _ = deliver(&Desktop(app), Category::WorkFinished.as_str(), &notice);
    }
}

/// A notice about the app itself that belongs to no category (where the
/// window went when it was closed to the tray): shown once, as asked.
pub fn app_notice(app: &AppHandle, title: &str, body: &str) {
    let notice = Notice {
        title: title.to_string(),
        body: body.to_string(),
    };
    let _ = deliver(&Desktop(app), "app", &notice);
}

// ── Senders' outcomes ────────────────────────────────────────────

/// What an Inbox item that was just posted announces, if anything. Only a
/// folder's indexing or sync that ended: done is finished work, failed a
/// problem (titled by the item, which names the folder; its detail can be
/// an error message, so it is only used for counts of a success).
pub fn inbox_outcome(item: &NewInboxItem) -> Option<Outcome> {
    if item.kind != InboxKind::Indexing {
        return None;
    }
    match item.status {
        InboxStatus::Done => {
            let summary = one_line(&item.title);
            let body = match item.detail.as_deref().map(str::trim) {
                Some(detail) if !detail.is_empty() => {
                    format!("{summary} · {}", one_line(detail))
                }
                _ => summary.clone(),
            };
            Some(Outcome::Finished(WorkDone {
                source: item.id.clone(),
                kind: WorkKind::Indexing,
                notice: Notice {
                    title: "Indexing finished".to_string(),
                    body,
                },
                summary,
            }))
        }
        InboxStatus::Failed => Some(Outcome::Problem {
            source: item.id.clone(),
            notice: Notice {
                title: "Indexing problem".to_string(),
                body: format!("{}. The Inbox has the details.", one_line(&item.title)),
            },
        }),
        InboxStatus::Working | InboxStatus::NeedsYou => None,
    }
}

/// What a sender's result announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Finished(WorkDone),
    Problem { source: String, notice: Notice },
}

/// Announces `outcome`.
pub fn announce(app: &AppHandle, outcome: Outcome) {
    match outcome {
        Outcome::Finished(done) => work_finished(app, done),
        Outcome::Problem { source, notice } => problem(app, source, notice.title, notice.body),
    }
}

/// Called by [`crate::inbox_commands::post`] for every item it saves.
pub fn on_inbox_item(app: &AppHandle, item: &NewInboxItem) {
    if let Some(outcome) = inbox_outcome(item) {
        announce(app, outcome);
    }
}

/// What a finished run of a conversation announces: its title names it
/// (`None`: not saved yet). An aborted run was stopped by the user.
pub fn answer_outcome(
    conversation_id: &str,
    status: RunStatus,
    title: Option<&str>,
) -> Option<WorkDone> {
    let name = title
        .map(one_line)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Your question".to_string());
    let (heading, body) = match status {
        RunStatus::Completed => ("Answer ready", name.clone()),
        RunStatus::Error => (
            "The answer stopped with an error",
            format!("{name} · open it in Shodh to try again"),
        ),
        RunStatus::Aborted => return None,
    };
    Some(WorkDone {
        source: format!("answer:{conversation_id}"),
        kind: WorkKind::Answer,
        notice: Notice {
            title: heading.to_string(),
            body,
        },
        summary: name,
    })
}

/// A run of a main conversation (not a side thread) finished.
pub fn answer_finished(app: &AppHandle, conversation_id: &str, event: &AgentEvent) {
    let AgentEvent::RunFinished { status, .. } = event else {
        return;
    };
    if *status == RunStatus::Aborted {
        return;
    }
    let status = *status;
    let conversation_id = conversation_id.to_string();
    in_background(app, move |app| {
        let Some(state) = app.try_state::<NotificationState>() else {
            return;
        };
        // Checked before reading the conversations, which is only needed for
        // a notification that will be shown.
        if !should_alert(&state.prefs(), Category::WorkFinished, window_state(app)) {
            return;
        }
        let title = conversation_title(app, &conversation_id);
        if let Some(done) = answer_outcome(&conversation_id, status, title.as_deref()) {
            submit_work(app, done);
        }
    });
}

/// The assistant of `conversation_id` waits for an approval. Named by the
/// conversation's title, never by the action's label, which the model writes
/// and which can quote a command line or a file's text.
pub fn approval_requested(app: &AppHandle, conversation_id: &str) {
    let conversation_id = conversation_id.to_string();
    in_background(app, move |app| {
        let Some(state) = app.try_state::<NotificationState>() else {
            return;
        };
        if !should_alert(&state.prefs(), Category::NeedsYou, window_state(app)) {
            return;
        }
        let title = conversation_title(app, &conversation_id);
        let _ = deliver(
            &Desktop(app),
            Category::NeedsYou.as_str(),
            &approval_notice(title.as_deref()),
        );
    });
}

/// The text of an approval request in the conversation titled `title`.
pub fn approval_notice(title: Option<&str>) -> Notice {
    let body = match title.map(one_line).filter(|t| !t.is_empty()) {
        Some(title) => format!("{title} · an action waits for Approve or Deny"),
        None => "An action waits for Approve or Deny.".to_string(),
    };
    Notice {
        title: "Shodh needs your approval".to_string(),
        body,
    }
}

fn conversation_title(app: &AppHandle, conversation_id: &str) -> Option<String> {
    let dir = crate::profile::app_data_dir(app).ok()?;
    match ConversationStore::in_dir(&dir).load() {
        Ok(conversations) => conversations
            .into_iter()
            .find(|c| c.id == conversation_id)
            .map(|c| c.title),
        Err(e) => {
            tracing::debug!(target: "shodh::notify", error = %e, "conversation title not read");
            None
        }
    }
}

/// What a result extraction announces: done, with counts, or failed.
pub fn results_outcome(file_path: &str, file_name: &str, added: Option<usize>) -> Outcome {
    let name = one_line(file_name);
    let source = format!("results:{file_path}");
    match added {
        Some(added) => Outcome::Finished(WorkDone {
            source,
            kind: WorkKind::Results,
            notice: Notice {
                title: "Results extracted".to_string(),
                body: match added {
                    0 => format!("{name} · no new results"),
                    1 => format!("{name} · 1 new result"),
                    n => format!("{name} · {n} new results"),
                },
            },
            summary: name,
        }),
        None => Outcome::Problem {
            source,
            notice: Notice {
                title: "Results were not extracted".to_string(),
                body: format!("{name}. Open the paper in Shodh to see why."),
            },
        },
    }
}

/// `waiting` new memory suggestions wait for the user.
pub fn memory_suggestions(app: &AppHandle, waiting: usize) {
    if waiting == 0 {
        return;
    }
    let body = if waiting == 1 {
        "1 new suggestion waits for you in the Inbox.".to_string()
    } else {
        format!("{waiting} new suggestions wait for you in the Inbox.")
    };
    notify(app, Category::MemorySuggestions, "Memory suggestions", body);
}

/// The learning model could not be used for the app's own work.
pub fn learning_failed(app: &AppHandle) {
    problem(
        app,
        "memory-learning",
        "Learning from a conversation failed",
        "The learning model could not be reached. Shodh tries again after your next answer.",
    );
}

// ── Commands ─────────────────────────────────────────────────────

pub const TEST_TITLE: &str = "Notifications work";
pub const TEST_BODY: &str =
    "This is how Shodh tells you about reminders, approvals and finished work.";

/// Shows the test notification through `sink`, regardless of the settings
/// and of the window being focused (the user just clicked the button).
pub fn send_test(sink: &dyn Deliver) -> Result<(), String> {
    deliver(
        sink,
        "test",
        &Notice {
            title: TEST_TITLE.to_string(),
            body: TEST_BODY.to_string(),
        },
    )
    .map_err(|e| format!("The system did not accept the notification: {e}"))
}

/// Settings → Notifications → "Send a test notification".
#[tauri::command]
pub async fn send_test_notification(app: AppHandle) -> Result<(), String> {
    send_test(&Desktop(&app))
}

/// The system's notification settings page.
pub fn system_settings_url() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("ms-settings:notifications")
    } else if cfg!(target_os = "macos") {
        Some("x-apple.systempreferences:com.apple.preference.notifications")
    } else {
        None
    }
}

/// Settings → Notifications → "Open system notification settings". The page
/// is chosen here, so the window cannot ask the opener for any other link.
#[tauri::command]
pub async fn open_notification_settings(app: AppHandle) -> Result<(), String> {
    let url = system_settings_url().ok_or_else(|| {
        "This system has no notification settings page Shodh can open.".to_string()
    })?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("The system notification settings could not be opened: {e}"))
}

/// Changes which notifications are shown. User-only: no agent tool calls
/// this, and the agent's settings tools refuse `notifications`.
#[tauri::command]
pub async fn set_notification_preferences(
    app: AppHandle,
    notifications: NotificationPrefs,
    audit: State<'_, AuditState>,
) -> Result<AppSettings, String> {
    let dir = crate::profile::app_data_dir(&app)
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;
    let (settings, before) = SettingsStore::in_dir(&dir)
        .update(|s| Ok(std::mem::replace(&mut s.notifications, notifications)))
        .map_err(|e| e.to_string())?;
    if before != settings.notifications {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({
                "action": "notification_preferences_change",
                "old": before,
                "new": settings.notifications,
                "via": "ui",
            }),
        ));
    }
    broadcast(&app, &settings);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn all_on() -> NotificationPrefs {
        NotificationPrefs {
            reminders: true,
            needs_you: true,
            memory_suggestions: true,
            work_finished: true,
            problems: true,
        }
    }

    const CATEGORIES: [Category; 5] = [
        Category::Reminders,
        Category::NeedsYou,
        Category::MemorySuggestions,
        Category::WorkFinished,
        Category::Problems,
    ];

    #[test]
    fn each_category_follows_its_switch() {
        let defaults = NotificationPrefs::default();
        assert!(allows(&defaults, Category::Reminders));
        assert!(allows(&defaults, Category::NeedsYou));
        assert!(allows(&defaults, Category::WorkFinished));
        assert!(allows(&defaults, Category::Problems));
        assert!(
            !allows(&defaults, Category::MemorySuggestions),
            "off by default"
        );
        let off = |change: fn(&mut NotificationPrefs)| {
            let mut prefs = all_on();
            change(&mut prefs);
            prefs
        };
        let cases: [(NotificationPrefs, Category); 5] = [
            (off(|p| p.reminders = false), Category::Reminders),
            (off(|p| p.needs_you = false), Category::NeedsYou),
            (
                off(|p| p.memory_suggestions = false),
                Category::MemorySuggestions,
            ),
            (off(|p| p.work_finished = false), Category::WorkFinished),
            (off(|p| p.problems = false), Category::Problems),
        ];
        for (prefs, silenced) in cases {
            for category in CATEGORIES {
                assert_eq!(
                    allows(&prefs, category),
                    category != silenced,
                    "{} with {} off",
                    category.as_str(),
                    silenced.as_str()
                );
            }
        }
        // Memory suggestions are part of "needs you".
        let mut prefs = all_on();
        prefs.needs_you = false;
        assert!(!allows(&prefs, Category::MemorySuggestions));
    }

    #[test]
    fn nothing_is_shown_while_the_user_looks_at_the_window() {
        let prefs = all_on();
        for category in CATEGORIES {
            assert!(!should_alert(&prefs, category, WindowState::Focused));
            assert!(should_alert(&prefs, category, WindowState::Unfocused));
            assert!(should_alert(&prefs, category, WindowState::Hidden));
        }
        let mut prefs = all_on();
        prefs.reminders = false;
        assert!(!should_alert(
            &prefs,
            Category::Reminders,
            WindowState::Hidden
        ));
    }

    fn done(source: &str, kind: WorkKind, summary: &str) -> WorkDone {
        WorkDone {
            source: source.to_string(),
            kind,
            notice: Notice {
                title: format!("{summary} title"),
                body: format!("{summary} body"),
            },
            summary: summary.to_string(),
        }
    }

    #[test]
    fn the_first_finished_work_is_shown_at_once_and_the_rest_of_the_minute_together() {
        let t0 = Instant::now();
        let mut batch = WorkBatch::default();
        assert_eq!(
            batch.add(t0, done("indexing:a", WorkKind::Indexing, "Papers")),
            WorkDecision::Show(Notice {
                title: "Papers title".into(),
                body: "Papers body".into()
            })
        );
        // Within the minute: held; the first held one asks for a flush at
        // the end of the window.
        assert_eq!(
            batch.add(
                t0 + Duration::from_secs(10),
                done("indexing:b", WorkKind::Indexing, "Notes")
            ),
            WorkDecision::Hold {
                flush_in: Some(Duration::from_secs(50))
            }
        );
        assert_eq!(
            batch.add(
                t0 + Duration::from_secs(20),
                done("indexing:c", WorkKind::Indexing, "Code")
            ),
            WorkDecision::Hold { flush_in: None }
        );
        assert_eq!(
            batch.add(
                t0 + Duration::from_secs(30),
                done("indexing:d", WorkKind::Indexing, "Thesis")
            ),
            WorkDecision::Hold { flush_in: None }
        );
        let flushed = batch.flush(t0 + WORK_WINDOW).unwrap();
        assert_eq!(flushed.title, "3 folders finished indexing");
        assert_eq!(flushed.body, "Notes · Code · Thesis");
        assert_eq!(batch.flush(t0 + WORK_WINDOW), None, "nothing left");
    }

    #[test]
    fn one_source_is_announced_at_most_once_a_minute() {
        let t0 = Instant::now();
        let mut batch = WorkBatch::default();
        assert!(matches!(
            batch.add(t0, done("indexing:a", WorkKind::Indexing, "Papers")),
            WorkDecision::Show(_)
        ));
        // The same folder again, twice, within the minute: one entry, the latest.
        let mut again = done("indexing:a", WorkKind::Indexing, "Papers");
        again.notice.body = "second".into();
        assert!(matches!(
            batch.add(t0 + Duration::from_secs(5), again),
            WorkDecision::Hold { flush_in: Some(_) }
        ));
        let mut third = done("indexing:a", WorkKind::Indexing, "Papers");
        third.notice.body = "third".into();
        assert_eq!(
            batch.add(t0 + Duration::from_secs(6), third),
            WorkDecision::Hold { flush_in: None }
        );
        let flushed = batch.flush(t0 + WORK_WINDOW).unwrap();
        assert_eq!(flushed.body, "third", "a batch of one is shown as itself");
        // Ten seconds after that batch: held for the end of its window.
        let later = t0 + WORK_WINDOW + Duration::from_secs(10);
        assert!(matches!(
            batch.add(later, done("indexing:a", WorkKind::Indexing, "Papers")),
            WorkDecision::Hold {
                flush_in: Some(d)
            } if d == Duration::from_secs(50)
        ));
        // Once that window was flushed and another passed in silence, the
        // next is shown at once.
        let after = t0 + WORK_WINDOW * 3;
        assert!(batch.flush(t0 + WORK_WINDOW * 2).is_some());
        assert!(matches!(
            batch.add(after, done("indexing:a", WorkKind::Indexing, "Papers")),
            WorkDecision::Show(_)
        ));
    }

    #[test]
    fn a_held_batch_is_shown_once_even_if_work_arrives_as_its_window_ends() {
        let t0 = Instant::now();
        let mut batch = WorkBatch::default();
        batch.add(t0, done("a", WorkKind::Indexing, "A"));
        batch.add(
            t0 + Duration::from_secs(1),
            done("b", WorkKind::Indexing, "B"),
        );
        // The window ended but its flush has not run yet: still held.
        assert_eq!(
            batch.add(t0 + WORK_WINDOW, done("c", WorkKind::Answer, "C")),
            WorkDecision::Hold { flush_in: None }
        );
        let flushed = batch.flush(t0 + WORK_WINDOW).unwrap();
        assert_eq!(
            flushed.title,
            "1 folder finished indexing, 1 answer finished"
        );
        assert_eq!(flushed.body, "B · C");
    }

    #[test]
    fn a_large_batch_names_three_and_counts_the_rest() {
        let items: Vec<WorkDone> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| done(n, WorkKind::Results, n))
            .collect();
        let notice = batch_notice(items);
        assert_eq!(notice.title, "Results extracted from 5 papers");
        assert_eq!(notice.body, "A · B · C · 2 more");
    }

    #[test]
    fn a_problem_from_one_source_is_shown_once_in_ten_minutes() {
        let t0 = Instant::now();
        let mut gate = ProblemGate::default();
        assert!(gate.admit(t0, "memory-learning"));
        assert!(!gate.admit(t0 + Duration::from_secs(60), "memory-learning"));
        assert!(gate.admit(t0 + Duration::from_secs(60), "indexing:a"));
        assert!(gate.admit(t0 + PROBLEM_COOLDOWN, "memory-learning"));
    }

    fn item(status: InboxStatus, title: &str, detail: Option<&str>) -> NewInboxItem {
        NewInboxItem {
            id: "indexing:src-1".into(),
            kind: InboxKind::Indexing,
            status,
            title: title.into(),
            detail: detail.map(str::to_string),
            link: None,
            data: json!({}),
        }
    }

    #[test]
    fn indexing_items_announce_finished_work_or_a_problem_without_error_text() {
        let finished = inbox_outcome(&item(
            InboxStatus::Done,
            "Papers is indexed",
            Some("120 files indexed"),
        ));
        let work = match finished {
            Some(Outcome::Finished(work)) => work,
            other => panic!("finished work: {other:?}"),
        };
        assert_eq!(work.source, "indexing:src-1");
        assert_eq!(work.notice.title, "Indexing finished");
        assert_eq!(work.notice.body, "Papers is indexed · 120 files indexed");
        assert_eq!(work.summary, "Papers is indexed");

        let failed = inbox_outcome(&item(
            InboxStatus::Failed,
            "Papers was not synced",
            Some("C:\\Users\\me\\secret.pdf: permission denied"),
        ));
        let (source, notice) = match failed {
            Some(Outcome::Problem { source, notice }) => (source, notice),
            other => panic!("a problem: {other:?}"),
        };
        assert_eq!(source, "indexing:src-1");
        assert_eq!(notice.title, "Indexing problem");
        assert_eq!(
            notice.body,
            "Papers was not synced. The Inbox has the details."
        );
        assert!(!notice.body.contains("secret"), "no error text");

        assert_eq!(
            inbox_outcome(&item(InboxStatus::Working, "Indexing Papers", None)),
            None
        );
        let mut reminder = item(InboxStatus::NeedsYou, "Call the bank", None);
        reminder.kind = InboxKind::Reminder;
        assert_eq!(
            inbox_outcome(&reminder),
            None,
            "reminders notify themselves"
        );
        let mut tables = item(InboxStatus::Done, "Tables refined", None);
        tables.kind = InboxKind::Tables;
        assert_eq!(inbox_outcome(&tables), None);
    }

    #[test]
    fn answers_are_named_by_their_conversation_title_only() {
        let ready = answer_outcome(
            "c1",
            RunStatus::Completed,
            Some("Transformer\nscaling laws"),
        )
        .unwrap();
        assert_eq!(ready.source, "answer:c1");
        assert_eq!(ready.notice.title, "Answer ready");
        assert_eq!(ready.notice.body, "Transformer scaling laws");
        let unsaved = answer_outcome("c2", RunStatus::Completed, None).unwrap();
        assert_eq!(unsaved.notice.body, "Your question");
        let failed = answer_outcome("c1", RunStatus::Error, Some("Taxes")).unwrap();
        assert_eq!(failed.notice.title, "The answer stopped with an error");
        assert_eq!(failed.notice.body, "Taxes · open it in Shodh to try again");
        assert_eq!(
            answer_outcome("c1", RunStatus::Aborted, Some("Taxes")),
            None
        );
    }

    #[test]
    fn approvals_name_the_conversation_not_the_action() {
        let notice = approval_notice(Some("Lab notes"));
        assert_eq!(notice.title, "Shodh needs your approval");
        assert_eq!(
            notice.body,
            "Lab notes · an action waits for Approve or Deny"
        );
        assert_eq!(
            approval_notice(None).body,
            "An action waits for Approve or Deny."
        );
        assert_eq!(
            approval_notice(Some("  ")).body,
            "An action waits for Approve or Deny."
        );
    }

    #[test]
    fn result_extractions_name_the_paper_and_count() {
        let Outcome::Finished(done) = results_outcome("C:/p/a.pdf", "a.pdf", Some(4)) else {
            panic!("finished");
        };
        assert_eq!(done.notice.body, "a.pdf · 4 new results");
        assert_eq!(done.source, "results:C:/p/a.pdf");
        let Outcome::Problem { notice, .. } = results_outcome("C:/p/a.pdf", "a.pdf", None) else {
            panic!("problem");
        };
        assert_eq!(notice.title, "Results were not extracted");
    }

    #[test]
    fn names_are_one_short_line() {
        assert_eq!(one_line("  Write\n notes.md  "), "Write notes.md");
        let long = "x".repeat(200);
        let cut = one_line(&long);
        assert_eq!(cut.chars().count(), MAX_NAME_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[derive(Default)]
    struct Recorder {
        shown: RefCell<Vec<(String, String)>>,
        fail: Option<String>,
    }

    impl Deliver for Recorder {
        fn show(&self, title: &str, body: &str) -> Result<(), String> {
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            self.shown
                .borrow_mut()
                .push((title.to_string(), body.to_string()));
            Ok(())
        }
    }

    #[test]
    fn the_test_notification_is_sent_as_is_and_a_refusal_is_reported() {
        let recorder = Recorder::default();
        send_test(&recorder).unwrap();
        assert_eq!(
            recorder.shown.borrow().as_slice(),
            &[(TEST_TITLE.to_string(), TEST_BODY.to_string())]
        );
        let refusing = Recorder {
            fail: Some("no executable path".into()),
            ..Recorder::default()
        };
        let error = send_test(&refusing).unwrap_err();
        assert!(error.contains("did not accept"), "{error}");
        assert!(error.contains("no executable path"), "{error}");
    }

    #[test]
    fn the_system_settings_page_is_the_platforms_own() {
        let url = system_settings_url();
        if cfg!(target_os = "windows") {
            assert_eq!(url, Some("ms-settings:notifications"));
        } else if cfg!(target_os = "macos") {
            assert_eq!(
                url,
                Some("x-apple.systempreferences:com.apple.preference.notifications")
            );
        } else {
            assert_eq!(url, None);
        }
    }
}
