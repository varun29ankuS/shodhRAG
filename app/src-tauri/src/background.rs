//! Background mode: closing the main window hides it to the tray (a
//! setting, on by default, explained the first time), so reminders keep
//! ringing; the tray menu opens the window, pauses background work or quits.
//! Starting with Windows is optional and off by default.
//!
//! The tray's pause is the indexing pause of the status strip and the
//! Library ([`crate::indexing_control`]): every indexing run waits before its
//! next file or batch, jobs the agent starts and folder syncs wait before
//! they take the index lock, and the pause is kept across restarts. Agent
//! answers and reminders are not paused.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use shodh_rag::audit::{AuditEventType, AuditRecord};
use shodh_rag::indexing::PauseSwitch;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, Window, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::app_settings::{broadcast, AppSettings, SettingsStore};
use crate::audit_commands::AuditState;

/// The window that hides to the tray. Other windows (daily brief, map,
/// floating widget) close normally.
pub const MAIN_WINDOW: &str = "main";

/// Name of the autostart entry (the Windows Run value, the macOS launch
/// agent). Fixed rather than taken from the product name, which the plugin
/// would otherwise use: an entry made before the app was renamed to Shodh
/// keeps being found, so it is never orphaned.
pub const AUTOSTART_ENTRY: &str = "shodh-rag-app";

/// Command-line flag the autostart entry passes: start hidden in the tray.
pub const BACKGROUND_FLAG: &str = "--background";

const MENU_OPEN: &str = "tray-open";
const MENU_PAUSE: &str = "tray-pause";
const MENU_QUIT: &str = "tray-quit";

const PAUSE_LABEL: &str = "Pause indexing";
const RESUME_LABEL: &str = "Resume indexing";

/// Tray and pause state, managed by Tauri.
#[derive(Default)]
pub struct BackgroundState {
    /// Quit was chosen: let the main window close.
    quitting: AtomicBool,
    /// The tray icon exists. Without it a hidden window could not be
    /// brought back, so closing then quits as usual.
    tray_ready: AtomicBool,
    /// Whether indexing is paused: the switch the engine's indexing state
    /// shares ([`shodh_rag::indexing::PauseSwitch`]).
    pause: PauseSwitch,
    pause_item: Mutex<Option<MenuItem<Wry>>>,
}

impl BackgroundState {
    /// Tray state over `pause`, the indexing pause switch.
    pub fn with_pause(pause: PauseSwitch) -> Self {
        Self {
            quitting: AtomicBool::new(false),
            tray_ready: AtomicBool::new(false),
            pause,
            pause_item: Mutex::new(None),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.pause.is_paused()
    }

    /// Pauses or resumes indexing and relabels the tray item; returns
    /// whether anything changed. [`crate::indexing_control::set_paused`]
    /// also saves it and tells the window.
    pub(crate) fn set_paused(&self, paused: bool) -> bool {
        let changed = self.pause.set(paused);
        self.relabel();
        changed
    }

    /// The tray item says what choosing it does now.
    fn relabel(&self) {
        let paused = self.is_paused();
        let item = self.pause_item.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = item.as_ref() {
            let label = if paused { RESUME_LABEL } else { PAUSE_LABEL };
            if let Err(e) = item.set_text(label) {
                tracing::warn!(error = %e, "could not relabel the tray pause item");
            }
        }
    }

    /// Wait while indexing is paused. Call before taking the index lock for
    /// an agent-started job or a folder sync.
    pub async fn wait_until_resumed(&self) {
        self.pause.wait_until_resumed().await;
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
            let paused = !app.state::<BackgroundState>().is_paused();
            crate::indexing_control::set_paused(app, paused);
            tracing::info!(paused, "indexing paused or resumed from the tray");
        }
        MENU_QUIT => {
            let state = app.state::<BackgroundState>();
            if state.quitting.swap(true, Ordering::SeqCst) {
                return;
            }
            // The window writes its waiting saves first (bounded wait).
            crate::shutdown::quit_after_saves(app);
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
    // Paused in an earlier run: the item offers to resume.
    app.state::<BackgroundState>().relabel();

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

/// macOS: the Dock icon (or Finder) opened Shodh while it runs. macOS hands
/// that to the running process as a reopen, never as a second launch, so
/// with the window closed to the tray this is the only way back to it.
/// Nothing happens while a window is visible or the app is quitting.
#[cfg(target_os = "macos")]
pub fn on_reopen(app: &AppHandle, has_visible_windows: bool) {
    let quitting = app
        .state::<BackgroundState>()
        .quitting
        .load(Ordering::SeqCst);
    if has_visible_windows || quitting {
        return;
    }
    show_main(app);
}

/// Whether this launch came from the autostart entry.
pub fn started_in_background() -> bool {
    std::env::args().any(|a| a == BACKGROUND_FLAG)
}

/// Where closing the main window takes it: the tray (with the settings
/// store and settings, to explain it the first time), or closed.
fn close_to_tray(app: &AppHandle, state: &BackgroundState) -> Option<(SettingsStore, AppSettings)> {
    if !state.tray_ready.load(Ordering::SeqCst) {
        return None;
    }
    let store = match crate::profile::app_data_dir(app) {
        Ok(dir) => SettingsStore::in_dir(&dir),
        Err(e) => {
            tracing::warn!(error = %e, "no app data directory; closing normally");
            return None;
        }
    };
    let settings = match store.load() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "settings unreadable; closing normally");
            return None;
        }
    };
    settings
        .background
        .close_to_tray
        .then_some((store, settings))
}

/// Window events: closing the main window hides it to the tray when the
/// setting is on. The first time, a notification explains where it went.
/// Otherwise the window closes once its waiting saves are written.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if window.label() != MAIN_WINDOW {
        return;
    }
    let app = window.app_handle();
    let state = app.state::<BackgroundState>();
    // Quit already had the window write its saves.
    if state.quitting.load(Ordering::SeqCst) {
        return;
    }
    let Some((store, settings)) = close_to_tray(app, &state) else {
        api.prevent_close();
        crate::shutdown::close_after_saves(window);
        return;
    };
    api.prevent_close();
    if let Err(e) = window.hide() {
        tracing::warn!(error = %e, "could not hide the main window");
    }
    if !settings.background.close_to_tray_explained {
        // Belongs to no notification category: it explains, once, the close the
        // user just made (a failure is logged by the notification service).
        crate::notifications::app_notice(
            app,
            "Shodh is still running",
            "It stays in the tray so reminders can ring. Use the tray icon to open or quit \
             Shodh; turn this off in Settings → General.",
        );
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
    /// See [`crate::app_settings::BackgroundPrefs::agent_idle_minutes`].
    pub agent_idle_minutes: u32,
}

fn settings_store(app: &AppHandle) -> Result<SettingsStore, String> {
    crate::profile::app_data_dir(app)
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
        agent_idle_minutes: settings.background.agent_idle_minutes,
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

/// How many minutes an idle agent runtime is kept (`0`: until the app exits).
/// User-only.
#[tauri::command]
pub async fn set_agent_idle_minutes(
    app: AppHandle,
    minutes: u32,
    audit: State<'_, AuditState>,
) -> Result<BackgroundStatus, String> {
    if minutes > crate::app_settings::MAX_AGENT_IDLE_MINUTES {
        return Err(format!(
            "The idle period is at most {} minutes",
            crate::app_settings::MAX_AGENT_IDLE_MINUTES
        ));
    }
    let (settings, before) = settings_store(&app)?
        .update(|s| {
            Ok(std::mem::replace(
                &mut s.background.agent_idle_minutes,
                minutes,
            ))
        })
        .map_err(|e| e.to_string())?;
    if before != minutes {
        audit.record(AuditRecord::new(
            AuditEventType::SettingsChange,
            json!({"action": "agent_idle_minutes", "old": before, "new": minutes, "via": "ui"}),
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
    // The startup entry is one per app; a separate profile must not change it.
    if crate::profile::active().is_separate() {
        return Err(format!(
            "The startup entry belongs to the normal profile; it cannot be changed while {} is set.",
            crate::profile::DATA_DIR_ENV
        ));
    }
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
