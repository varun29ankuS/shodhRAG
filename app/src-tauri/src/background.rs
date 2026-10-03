//! Background mode: closing the main window hides it to the tray (a
//! setting, on by default, explained the first time), so reminders keep
//! ringing; the tray menu opens the window, pauses background work or quits.
//! Starting with Windows is optional and off by default.
//!
//! "Background work" is indexing the agent starts on its own (folders it
//! adds or re-indexes, files it downloads). Pausing holds those jobs before
//! they take the index lock, so searches never wait on a paused job; a job
//! already running finishes. Indexing the user starts, agent answers and
//! reminders are not paused.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, Window, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Notify;

use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;

/// The window that hides to the tray. Other windows (daily brief, map,
/// floating widget) close normally.
pub const MAIN_WINDOW: &str = "main";

/// Command-line flag the autostart entry passes: start hidden in the tray.
pub const BACKGROUND_FLAG: &str = "--background";

const MENU_OPEN: &str = "tray-open";
const MENU_PAUSE: &str = "tray-pause";
const MENU_QUIT: &str = "tray-quit";

const PAUSE_LABEL: &str = "Pause background work";
const RESUME_LABEL: &str = "Resume background work";

/// Tray and pause state, managed by Tauri.
#[derive(Default)]
pub struct BackgroundState {
    /// Quit was chosen: let the main window close.
    quitting: AtomicBool,
    /// The tray icon exists. Without it a hidden window could not be
    /// brought back, so closing then quits as usual.
    tray_ready: AtomicBool,
    paused: AtomicBool,
    resumed: Notify,
    pause_item: Mutex<Option<MenuItem<Wry>>>,
}

impl BackgroundState {
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        if !paused {
            self.resumed.notify_waiters();
        }
        let item = self.pause_item.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = item.as_ref() {
            let label = if paused { RESUME_LABEL } else { PAUSE_LABEL };
            if let Err(e) = item.set_text(label) {
                tracing::warn!(error = %e, "could not relabel the tray pause item");
            }
        }
    }

    /// Wait while background work is paused. Call before taking the index
    /// lock for an agent-started job.
    pub async fn wait_until_resumed(&self) {
        loop {
            let resumed = self.resumed.notified();
            tokio::pin!(resumed);
            resumed.as_mut().enable();
            if !self.is_paused() {
                return;
            }
            resumed.await;
        }
    }
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        if let Err(e) = window.show().and_then(|()| window.unminimize()) {
            tracing::warn!(error = %e, "could not show the main window");
        }
        if let Err(e) = window.set_focus() {
            tracing::debug!(error = %e, "could not focus the main window");
        }
    }
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        MENU_OPEN => show_main(app),
        MENU_PAUSE => {
            let state = app.state::<BackgroundState>();
            let paused = !state.is_paused();
            state.set_paused(paused);
            tracing::info!(paused, "background work paused from the tray");
        }
        MENU_QUIT => {
            app.state::<BackgroundState>()
                .quitting
                .store(true, Ordering::SeqCst);
            app.exit(0);
        }
        _ => {}
    }
}

/// Create the tray icon and its menu. Call once in setup, after
/// [`BackgroundState`] is managed.
pub fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Shodh", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, MENU_PAUSE, PAUSE_LABEL, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit Shodh", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &pause, &separator, &quit])?;
    *app.state::<BackgroundState>()
        .pause_item
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(pause);

    let mut tray = TrayIconBuilder::with_id("shodh")
        .tooltip("Shodh")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.state::<BackgroundState>()
        .tray_ready
        .store(true, Ordering::SeqCst);
    Ok(())
}

/// Shodh was started again while running (often: hidden in the tray, the
/// user opened it from the Start menu). Show the running window, unless the
/// second launch was the autostart entry, which must stay in the tray.
pub fn on_second_launch(app: &AppHandle, args: &[String]) {
    if args.iter().any(|a| a == BACKGROUND_FLAG) {
        return;
    }
    show_main(app);
}

/// Whether this launch came from the autostart entry.
pub fn started_in_background() -> bool {
    std::env::args().any(|a| a == BACKGROUND_FLAG)
}

/// Window events: closing the main window hides it to the tray when the
/// setting is on. The first time, a notification explains where it went.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if window.label() != MAIN_WINDOW {
        return;
    }
    let app = window.app_handle();
    let state = app.state::<BackgroundState>();
    if state.quitting.load(Ordering::SeqCst) || !state.tray_ready.load(Ordering::SeqCst) {
        return;
    }
    let store = match app.path().app_data_dir() {
        Ok(dir) => SettingsStore::in_dir(&dir),
        Err(e) => {
            tracing::warn!(error = %e, "no app data directory; closing normally");
            return;
        }
    };
    let settings = match store.load() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "settings unreadable; closing normally");
            return;
        }
    };
    if !settings.background.close_to_tray {
        return;
    }
    api.prevent_close();
    if let Err(e) = window.hide() {
        tracing::warn!(error = %e, "could not hide the main window");
    }
    if !settings.background.close_to_tray_explained {
        let shown = app
            .notification()
            .builder()
            .title("Shodh is still running")
            .body(
                "It stays in the tray so reminders can ring. Use the tray icon to open or quit \
                 Shodh; turn this off in Settings → General.",
            )
            .show();
        if let Err(e) = shown {
            tracing::warn!(error = %e, "could not explain close-to-tray");
        }
        match store.update(|s| {
            s.background.close_to_tray_explained = true;
            Ok(())
        }) {
            Ok((saved, ())) => broadcast(app, &saved),
            Err(e) => {
                tracing::warn!(error = %e, "could not record that close-to-tray was explained")
            }
        }
    }
}

// ── Commands ─────────────────────────────────────────────────────

/// What Settings shows about background mode.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundStatus {
    pub close_to_tray: bool,
    /// Read from the operating system (the user can also turn it off in
    /// Task Manager), not from the settings file.
    pub start_with_windows: bool,
    pub paused: bool,
}

fn settings_store(app: &AppHandle) -> Result<SettingsStore, String> {
    app.path()
        .app_data_dir()
        .map(|dir| SettingsStore::in_dir(&dir))
        .map_err(|e| format!("Failed to get app data directory: {e}"))
}

fn status(app: &AppHandle, settings: &AppSettings) -> BackgroundStatus {
    let start_with_windows = app.autolaunch().is_enabled().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "could not read the startup entry");
        false
    });
    BackgroundStatus {
        close_to_tray: settings.background.close_to_tray,
        start_with_windows,
        paused: app.state::<BackgroundState>().is_paused(),
    }
}

#[tauri::command]
pub async fn get_background_status(app: AppHandle) -> Result<BackgroundStatus, String> {
    let settings = settings_store(&app)?.load().map_err(|e| e.to_string())?;
    Ok(status(&app, &settings))
}

/// Whether closing the main window hides it to the tray. User-only.
#[tauri::command]
pub async fn set_close_to_tray(
    app: AppHandle,
    enabled: bool,
    audit: State<'_, AuditState>,
) -> Result<BackgroundStatus, String> {
    let (settings, before) = settings_store(&app)?
        .update(|s| {
            let before = s.background.close_to_tray;
            s.background.close_to_tray = enabled;
            // Choosing it in Settings is its own explanation.
            s.background.close_to_tray_explained = true;
            Ok(before)
        })
        .map_err(|e| e.to_string())?;
    if before != enabled {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({"action": "close_to_tray", "old": before, "new": enabled, "via": "ui"}),
        ));
    }
    broadcast(&app, &settings);
    Ok(status(&app, &settings))
}

/// Add or remove the "start with Windows" entry. User-only.
#[tauri::command]
pub async fn set_start_with_windows(
    app: AppHandle,
    enabled: bool,
    audit: State<'_, AuditState>,
) -> Result<BackgroundStatus, String> {
    let launcher = app.autolaunch();
    let before = launcher.is_enabled().unwrap_or(false);
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| format!("Could not change the startup entry: {e}"))?;
    if before != enabled {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({"action": "start_with_windows", "old": before, "new": enabled, "via": "ui"}),
        ));
    }
    let settings = settings_store(&app)?.load().map_err(|e| e.to_string())?;
    Ok(status(&app, &settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn paused_work_waits_until_resumed() {
        let state = Arc::new(BackgroundState::default());
        // Not paused: returns at once.
        tokio::time::timeout(Duration::from_millis(100), state.wait_until_resumed())
            .await
            .unwrap();
        state.set_paused(true);
        let waiter = {
            let state = state.clone();
            tokio::spawn(async move { state.wait_until_resumed().await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "held while paused");
        state.set_paused(false);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
    }
}
