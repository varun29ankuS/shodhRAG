//! Quitting without losing the frontend's waiting saves. The frontend
//! debounces conversation saves; an exit that only tore the WebView down
//! would drop a change made in the last moments. Every exit path first asks
//! the main window to write its waiting saves (`app-quitting`), waits for
//! its `quit_ready`, and goes on anyway after [`SAVE_WAIT`], so a hung or
//! still-loading frontend never blocks quitting:
//! - the tray's Quit ([`quit_after_saves`]);
//! - closing the main window when it does not hide to the tray
//!   ([`close_after_saves`]);
//! - installing an update ([`prepare_exit`] before the installer runs);
//! - any other exit request, e.g. the system's quit command
//!   ([`on_exit_requested`] holds it until the saves are written).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Window};
use tokio::sync::oneshot;

use crate::background::MAIN_WINDOW;

/// Sent to the main window before the app exits; payload [`QuitRequest`].
pub const APP_QUITTING_EVENT: &str = "app-quitting";

/// Longest wait for the frontend's saves before exiting anyway.
pub const SAVE_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuitRequest {
    request_id: u64,
}

/// Exits waiting for the frontend, managed by Tauri.
#[derive(Default)]
pub struct ShutdownState {
    next_id: AtomicU64,
    waiting: Mutex<HashMap<u64, oneshot::Sender<()>>>,
    /// The main window is closing (its saves are being written).
    closing: AtomicBool,
    /// An exit request is held while the saves are written.
    holding_exit: AtomicBool,
    /// The saves were written (or the wait ran out): exits go ahead.
    exit_ready: AtomicBool,
    /// What the process holds was released ([`release_everything`]).
    released: AtomicBool,
}

impl ShutdownState {
    /// A new request the frontend answers with `quit_ready(request_id)`.
    fn register(&self) -> (u64, oneshot::Receiver<()>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        (id, rx)
    }

    /// The frontend wrote its saves for request `id`. False when no exit
    /// waits on `id` (it already went on without it).
    fn ready(&self, id: u64) -> bool {
        let sender = self
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        match sender {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// True the first time only: what the process holds is released once.
    fn first_release(&self) -> bool {
        !self.released.swap(true, Ordering::SeqCst)
    }

    fn forget(&self, id: u64) {
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
}

/// Wait for the frontend's answer at most `limit`. True when it answered
/// in time.
async fn wait_ready(answer: oneshot::Receiver<()>, limit: Duration) -> bool {
    matches!(tokio::time::timeout(limit, answer).await, Ok(Ok(())))
}

/// Ask the main window to write its waiting saves and wait until it says
/// they are written, at most [`SAVE_WAIT`]. Returns at once without a main
/// window.
pub async fn flush_frontend(app: &AppHandle) {
    if app.get_webview_window(MAIN_WINDOW).is_none() {
        return;
    }
    let state = app.state::<ShutdownState>();
    let (request_id, answer) = state.register();
    if let Err(e) = app.emit_to(MAIN_WINDOW, APP_QUITTING_EVENT, QuitRequest { request_id }) {
        tracing::warn!(error = %e, "could not ask the window to save before quitting");
        state.forget(request_id);
        return;
    }
    if !wait_ready(answer, SAVE_WAIT).await {
        tracing::warn!(
            wait_ms = SAVE_WAIT.as_millis() as u64,
            "the window did not confirm its saves in time; quitting anyway"
        );
    }
    state.forget(request_id);
}

/// Have the window write its saves, then let exits go ahead without asking
/// again (the caller exits, closes the window or installs an update).
pub async fn prepare_exit(app: &AppHandle) {
    flush_frontend(app).await;
    app.state::<ShutdownState>()
        .exit_ready
        .store(true, Ordering::SeqCst);
}

/// The exit [`prepare_exit`] was for did not happen (an update failed to
/// install): later exits write the saves again.
pub fn cancel_exit(app: &AppHandle) {
    app.state::<ShutdownState>()
        .exit_ready
        .store(false, Ordering::SeqCst);
}

/// Releases what the process holds before it ends, once: evidence table fills go back
/// to the queue, the connectors stop, Code-mode commands and the agent runtimes end,
/// and the log's last line says the app quit. Called for `RunEvent::Exit`, and before
/// an update's installer ends the process: on Windows the updater ends it with
/// `process::exit`, so `RunEvent::Exit` never comes and nothing else would run this.
pub fn release_everything(app: &AppHandle, reason: &'static str) {
    if !app.state::<ShutdownState>().first_release() {
        return;
    }
    // Evidence table fills held by this process go back to the queue, so the next
    // start takes them up at once.
    crate::table_commands::release_on_exit(app);
    // Slack connections are closed (a close frame, bounded wait).
    crate::connectors::slack::shutdown(app);
    // Telegram polling ends (bounded wait).
    crate::connectors::telegram::shutdown(app);
    // Code-mode commands end with every process they started (on macOS nothing else
    // would end them once Shodh is gone).
    let commands = shodh_rag::harness::run_command::stop_all();
    if commands > 0 {
        tracing::info!(commands, "stopped running commands on exit");
    }
    // Stop every omp sidecar before the process exits.
    let sessions = app.state::<crate::agent_session_commands::AgentSessions>();
    tauri::async_runtime::block_on(sessions.shutdown_all());
    // The last line of a run: without it a clean quit and a crash leave the same log.
    tracing::info!(reason, "Shodh quit: connectors and agent sessions stopped");
}

/// Quit the app once the frontend's saves are written (the tray's Quit).
pub fn quit_after_saves(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        prepare_exit(&app).await;
        app.exit(0);
    });
}

/// What to do with an exit request (`RunEvent::ExitRequested`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitDecision {
    /// Let it exit.
    Allow,
    /// Hold it and write the saves, then exit.
    HoldAndSave,
    /// Hold it: the saves are already being written, the exit follows.
    Hold,
}

/// `restarting`: a restart cannot be held (Tauri ignores `prevent_exit` for it).
fn exit_decision(
    exit_ready: bool,
    restarting: bool,
    has_main_window: bool,
    already_holding: bool,
) -> ExitDecision {
    if exit_ready || restarting || !has_main_window {
        ExitDecision::Allow
    } else if already_holding {
        ExitDecision::Hold
    } else {
        ExitDecision::HoldAndSave
    }
}

/// An exit was requested. Returns true when it must be prevented: the main
/// window still has to write its saves, after which the app exits with the
/// requested code (0 when none was given).
pub fn on_exit_requested(app: &AppHandle, code: Option<i32>) -> bool {
    let state = app.state::<ShutdownState>();
    let has_main_window = app.get_webview_window(MAIN_WINDOW).is_some();
    let decision = exit_decision(
        state.exit_ready.load(Ordering::SeqCst),
        code == Some(tauri::RESTART_EXIT_CODE),
        has_main_window,
        state.holding_exit.load(Ordering::SeqCst),
    );
    match decision {
        ExitDecision::Allow => false,
        ExitDecision::Hold => true,
        ExitDecision::HoldAndSave => {
            if state.holding_exit.swap(true, Ordering::SeqCst) {
                return true;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                prepare_exit(&app).await;
                app.exit(code.unwrap_or(0));
            });
            true
        }
    }
}

/// Close the main window once the frontend's saves are written. The caller
/// has prevented the close; a second close while saving is ignored.
pub fn close_after_saves(window: &Window) {
    let app = window.app_handle().clone();
    if app
        .state::<ShutdownState>()
        .closing
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        // Without the main window there is nothing left to save.
        prepare_exit(&app).await;
        if let Err(e) = window.destroy() {
            tracing::warn!(error = %e, "could not close the main window");
            // The window stays: later closes and exits must save again.
            app.state::<ShutdownState>()
                .closing
                .store(false, Ordering::SeqCst);
            cancel_exit(&app);
        }
    });
}

/// The frontend wrote its waiting saves for an `app-quitting` request.
#[tauri::command]
pub fn quit_ready(state: State<'_, ShutdownState>, request_id: u64) {
    if !state.ready(request_id) {
        tracing::debug!(request_id, "saves confirmed after the exit went on");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The update installer and `RunEvent::Exit` can both ask: what the process holds
    /// is released once.
    #[test]
    fn what_the_process_holds_is_released_once() {
        let state = ShutdownState::default();
        assert!(state.first_release());
        assert!(!state.first_release());
        assert!(!state.first_release());
    }

    #[tokio::test]
    async fn an_answer_in_time_ends_the_wait() {
        let state = ShutdownState::default();
        let (id, answer) = state.register();
        assert!(state.ready(id));
        assert!(wait_ready(answer, Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn no_answer_ends_the_wait_at_the_limit() {
        let state = ShutdownState::default();
        let (_id, answer) = state.register();
        let started = std::time::Instant::now();
        assert!(!wait_ready(answer, Duration::from_millis(50)).await);
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(50), "waited {waited:?}");
        assert!(waited < Duration::from_secs(2), "waited {waited:?}");
    }

    #[tokio::test]
    async fn a_late_or_unknown_answer_is_ignored() {
        let state = ShutdownState::default();
        let (id, answer) = state.register();
        assert!(!wait_ready(answer, Duration::from_millis(10)).await);
        state.forget(id);
        assert!(!state.ready(id), "the exit already went on");
        assert!(!state.ready(9_999));
    }

    #[tokio::test]
    async fn a_forgotten_request_never_reads_as_answered() {
        let state = ShutdownState::default();
        let (id, answer) = state.register();
        state.forget(id);
        assert!(!wait_ready(answer, Duration::from_secs(1)).await);
    }

    #[test]
    fn exits_wait_for_the_saves_once() {
        assert_eq!(
            exit_decision(false, false, true, false),
            ExitDecision::HoldAndSave
        );
        assert_eq!(exit_decision(false, false, true, true), ExitDecision::Hold);
        assert_eq!(exit_decision(true, false, true, false), ExitDecision::Allow);
        assert_eq!(exit_decision(true, false, true, true), ExitDecision::Allow);
        assert_eq!(exit_decision(false, true, true, false), ExitDecision::Allow);
        assert_eq!(
            exit_decision(false, false, false, false),
            ExitDecision::Allow
        );
    }

    #[test]
    fn requests_get_distinct_ids() {
        let state = ShutdownState::default();
        let (a, _ra) = state.register();
        let (b, _rb) = state.register();
        assert_ne!(a, b);
        assert!(state.ready(b));
        assert!(state.ready(a));
    }

    #[test]
    fn the_request_payload_is_camel_case() {
        let json = serde_json::to_value(QuitRequest { request_id: 7 }).unwrap();
        assert_eq!(json, serde_json::json!({ "requestId": 7 }));
    }
}
