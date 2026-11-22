//! The floating chat over the embedded Artifacts preview.
//!
//! A native child webview paints above the main webview's page, so while
//! the embedded preview ([`crate::creations::preview_embed`]) is up, the
//! chat the main page draws in that corner would be under it. The chat then
//! shows in a child webview of its own, `dock`, added above the preview:
//!
//! - it loads the app's `/dock-view` page in the main window's own browser
//!   environment ([`crate::profile::child_webview_defaults`]);
//! - it is hidden by default and shown exactly while the embedded preview is
//!   shown ([`follow`]), sized to the chat's own box (the pill or the open
//!   panel) and nothing larger, so it never catches clicks meant for the
//!   page behind it ([`dock_bounds`]);
//! - it runs no chat of its own: the main webview publishes a snapshot of
//!   what the chat shows ([`dock_layer_publish`], to `dock` only), and the
//!   dock sends the user's clicks and keys back ([`dock_layer_action`], to
//!   `main` only), where the real session acts on them after checking them;
//! - its capability (`dock-window`) grants only those relay commands; every
//!   permission request (microphone, camera and the rest) is denied: voice
//!   runs in `main`, which holds the microphone ([`deny_permissions`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::Value;
use tauri::webview::{PermissionKind, PermissionResponse, WebviewBuilder};
use tauri::{
    AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, Rect, Runtime,
    WebviewUrl,
};

use crate::background::MAIN_WINDOW;
use crate::creations::preview_embed::{CssRect, Visibility};

/// The floating chat's webview label.
pub const DOCK_WEBVIEW: &str = "dock";

/// Main's snapshot of the chat, relayed to `dock` only.
pub const DOCK_SNAPSHOT_EVENT: &str = "dock-snapshot";

/// The dock's user actions, relayed to `main` only.
pub const DOCK_ACTION_EVENT: &str = "dock-action";

/// The largest snapshot or action relayed, in bytes of JSON (the frontend's
/// `MAX_SNAPSHOT_BYTES` is the same).
pub const MAX_RELAY_BYTES: usize = 262_144;

/// The gap between the chat and the window's bottom and right edges, in the
/// dock page's CSS pixels (as the in-page dock's `bottom-4 right-4`).
pub const DOCK_EDGE: f64 = 16.0;

/// The chat's size before the dock page reports it: the pill's.
const FIRST_SIZE: (f64, f64) = (240.0, 40.0);

/// The dock page's chat box, in its CSS pixels, as it last reported it.
static PANEL: Mutex<Option<(f64, f64)>> = Mutex::new(None);

/// Main's latest snapshot, sent again whenever the dock page reports in (a
/// page that just loaded has missed the ones before).
static SNAPSHOT: Mutex<Option<Value>> = Mutex::new(None);

/// Whether the dock should be up: the embedded preview is shown.
static UP: AtomicBool = AtomicBool::new(false);

/// Where the dock is up now (`None`: hidden or not made).
static APPLIED: Mutex<Option<CssRect>> = Mutex::new(None);

/// The dock page has reported its size since it was made (it is listening and drawn).
static SIZED: AtomicBool = AtomicBool::new(false);

/// Where the dock is up now, in logical pixels of the main window (`None`: hidden).
pub fn shown_at() -> Option<CssRect> {
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the dock page has reported its size since the dock was made.
pub fn sized() -> bool {
    SIZED.load(Ordering::SeqCst)
}

/// Where the chat's box goes: its `panel` size, at the bottom right of a
/// `window` of that logical size, `edge` from its edges, and never larger
/// than the window leaves room for.
pub fn dock_bounds(panel: (f64, f64), window: (f64, f64), edge: f64) -> CssRect {
    let sane = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let (window_w, window_h, edge) = (sane(window.0), sane(window.1), sane(edge));
    let width = sane(panel.0).min((window_w - 2.0 * edge).max(0.0));
    let height = sane(panel.1).min((window_h - 2.0 * edge).max(0.0));
    CssRect {
        x: (window_w - edge - width).max(0.0),
        y: (window_h - edge - height).max(0.0),
        width,
        height,
    }
}

/// Whether a relay from `caller` may go on: only from the `expected`
/// webview, and only up to [`MAX_RELAY_BYTES`].
pub fn relay_allowed(caller: &str, expected: &str, bytes: usize) -> Result<(), String> {
    if caller != expected {
        return Err(format!("Only the {expected} webview may send this."));
    }
    if bytes > MAX_RELAY_BYTES {
        return Err(format!(
            "That is {bytes} bytes; at most {MAX_RELAY_BYTES} are relayed."
        ));
    }
    Ok(())
}

fn json_size(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

/// Every permission request of the dock page is denied: it never gets the
/// microphone, camera, location or the rest. Dictation runs in `main`.
fn deny_permissions<R: Runtime>(
    _webview: tauri::Webview<R>,
    _kind: PermissionKind,
) -> PermissionResponse {
    PermissionResponse::Deny
}

/// Add the dock to the main window, hidden, above the child webviews added
/// before it. Never call on the main thread (adding a child waits for it).
fn build<R: Runtime>(app: &AppHandle<R>) -> Result<tauri::Webview<R>, String> {
    let main = crate::background::main_window(app)
        .ok_or_else(|| "The main window is not open.".to_string())?;
    let builder = crate::profile::child_webview_defaults(WebviewBuilder::new(
        DOCK_WEBVIEW,
        WebviewUrl::App("dock-view".into()),
    ))
    .transparent(true)
    .focused(false)
    .devtools(cfg!(debug_assertions))
    .on_permission_request(deny_permissions);
    let dock = main
        .add_child(
            builder,
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(FIRST_SIZE.0, FIRST_SIZE.1),
        )
        .map_err(|e| format!("The floating chat could not open ({e})"))?;
    if let Err(e) = dock.hide() {
        if let Err(close) = dock.close() {
            tracing::warn!(error = %close, "the floating chat could not be closed");
        }
        return Err(format!("The floating chat could not be hidden ({e})"));
    }
    Ok(dock)
}

/// The dock's place over the main window now, in logical pixels.
fn bounds_now<R: Runtime>(app: &AppHandle<R>) -> Result<CssRect, String> {
    let main = crate::background::main_window(app)
        .ok_or_else(|| "The main window is not open.".to_string())?;
    let scale = main.scale_factor().map_err(|e| e.to_string())?;
    let size = main
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale);
    // The dock page is drawn at the main page's text size.
    let zoom = crate::app_settings::main_zoom();
    let panel = PANEL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unwrap_or(FIRST_SIZE);
    Ok(dock_bounds(
        (panel.0 * zoom, panel.1 * zoom),
        (size.width, size.height),
        DOCK_EDGE * zoom,
    ))
}

fn place<R: Runtime>(dock: &tauri::Webview<R>, rect: CssRect) -> tauri::Result<()> {
    dock.set_bounds(Rect {
        position: LogicalPosition::new(rect.x, rect.y).into(),
        size: LogicalSize::new(rect.width, rect.height).into(),
    })
}

/// The embedded preview's visibility changed, or main reported the pane
/// again: the dock is up exactly while the preview is shown, at the chat's
/// box. It is made the first time it is needed.
pub fn follow<R: Runtime>(app: &AppHandle<R>, visibility: Visibility) {
    let up = matches!(visibility, Visibility::Shown(_));
    UP.store(up, Ordering::SeqCst);
    if !up {
        let was_up = APPLIED.lock().unwrap_or_else(|e| e.into_inner()).take();
        if was_up.is_some() {
            if let Some(dock) = app.get_webview(DOCK_WEBVIEW) {
                if let Err(e) = dock.hide() {
                    tracing::warn!(error = %e, "the floating chat could not be hidden");
                }
            }
        }
        return;
    }
    if let Err(e) = show(app) {
        tracing::warn!(error = %e, "the floating chat could not be shown");
    }
}

/// Put the dock up at the chat's box now, unless it is there already.
fn show<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let rect = bounds_now(app)?;
    if *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) == Some(rect) {
        return Ok(());
    }
    let dock = match app.get_webview(DOCK_WEBVIEW) {
        Some(dock) => dock,
        None => build(app)?,
    };
    place(&dock, rect)
        .and_then(|()| dock.show())
        .map_err(|e| e.to_string())?;
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = Some(rect);
    Ok(())
}

/// A new embedded preview was added above the dock: the dock goes, and is
/// made again above it when the preview shows.
pub fn below_new_preview<R: Runtime>(app: &AppHandle<R>) {
    UP.store(false, Ordering::SeqCst);
    SIZED.store(false, Ordering::SeqCst);
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some(dock) = app.get_webview(DOCK_WEBVIEW) {
        if let Err(e) = dock.close() {
            tracing::warn!(error = %e, "the floating chat could not be closed");
        }
    }
}

/// Relay main's snapshot to the dock alone.
pub fn relay_snapshot<R: Runtime>(
    app: &AppHandle<R>,
    caller: &str,
    snapshot: Value,
) -> Result<(), String> {
    relay_allowed(caller, MAIN_WINDOW, json_size(&snapshot))?;
    let sent = app.emit_to(
        EventTarget::Webview {
            label: DOCK_WEBVIEW.to_string(),
        },
        DOCK_SNAPSHOT_EVENT,
        &snapshot,
    );
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot);
    sent.map_err(|e| e.to_string())
}

/// Relay the dock's action to main alone. It is not read here: main checks
/// it before acting on it.
pub fn relay_action<R: Runtime>(
    app: &AppHandle<R>,
    caller: &str,
    action: Value,
) -> Result<(), String> {
    relay_allowed(caller, DOCK_WEBVIEW, json_size(&action))?;
    app.emit_to(
        EventTarget::Webview {
            label: MAIN_WINDOW.to_string(),
        },
        DOCK_ACTION_EVENT,
        action,
    )
    .map_err(|e| e.to_string())
}

/// The dock page's chat box changed size (CSS pixels), or the page just
/// started listening: place it again and send it the latest snapshot.
pub fn resized<R: Runtime>(
    app: &AppHandle<R>,
    caller: &str,
    width: f64,
    height: f64,
) -> Result<(), String> {
    relay_allowed(caller, DOCK_WEBVIEW, 0)?;
    if !(width.is_finite() && height.is_finite() && width >= 0.0 && height >= 0.0) {
        return Err("The floating chat's size is not a size.".to_string());
    }
    *PANEL.lock().unwrap_or_else(|e| e.into_inner()) = Some((width, height));
    SIZED.store(true, Ordering::SeqCst);
    let snapshot = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(snapshot) = snapshot {
        app.emit_to(
            EventTarget::Webview {
                label: DOCK_WEBVIEW.to_string(),
            },
            DOCK_SNAPSHOT_EVENT,
            snapshot,
        )
        .map_err(|e| e.to_string())?;
    }
    if UP.load(Ordering::SeqCst) {
        show(app)?;
    }
    Ok(())
}

/// Main publishes what the floating chat shows (granted to `main-window`).
#[tauri::command]
pub async fn dock_layer_publish(
    webview: tauri::Webview,
    app: AppHandle,
    snapshot: Value,
) -> Result<(), String> {
    relay_snapshot(&app, webview.label(), snapshot)
}

/// The floating chat sends one of the user's actions to main (granted to
/// `dock-window`).
#[tauri::command]
pub async fn dock_layer_action(
    webview: tauri::Webview,
    app: AppHandle,
    action: Value,
) -> Result<(), String> {
    relay_action(&app, webview.label(), action)
}

/// The commands are async so they run off the event loop: showing the dock
/// may add it to the main window, which waits for the event loop.
///
/// The floating chat's box is now `width` × `height` CSS pixels (granted to
/// `dock-window`).
#[tauri::command]
pub async fn dock_layer_size(
    webview: tauri::Webview,
    app: AppHandle,
    width: f64,
    height: f64,
) -> Result<(), String> {
    resized(&app, webview.label(), width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tauri::{Listener, WebviewWindowBuilder};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CssRect {
        CssRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_dock_sits_bottom_right_inside_the_window() {
        // The open panel.
        assert_eq!(
            dock_bounds((380., 470.), (1400., 900.), 16.),
            rect(1004., 414., 380., 470.)
        );
        // The pill: only its own box, so a click just left of it or above it
        // reaches the page behind.
        assert_eq!(
            dock_bounds((236., 40.), (1400., 900.), 16.),
            rect(1148., 844., 236., 40.)
        );
        // A panel taller (or wider) than the window leaves the edges free.
        assert_eq!(
            dock_bounds((380., 2000.), (1400., 900.), 16.),
            rect(1004., 16., 380., 868.)
        );
        assert_eq!(
            dock_bounds((2000., 40.), (300., 900.), 16.),
            rect(16., 844., 268., 40.)
        );
        // Nonsense sizes give an empty box, never one outside the window.
        let empty = dock_bounds((f64::NAN, -5.), (1400., 900.), 16.);
        assert_eq!((empty.width, empty.height), (0., 0.));
        assert!(empty.x <= 1400. && empty.y <= 900.);
    }

    #[test]
    fn relay_refuses_the_wrong_caller_and_big_payloads() {
        assert!(relay_allowed("main", "main", 10).is_ok());
        assert!(relay_allowed("dock", "dock", MAX_RELAY_BYTES).is_ok());
        assert!(relay_allowed("dock", "main", 10).is_err());
        assert!(relay_allowed("preview-0123", "dock", 10).is_err());
        assert!(relay_allowed("main", "main", MAX_RELAY_BYTES + 1).is_err());
        assert_eq!(MAX_RELAY_BYTES, 262_144);
    }

    /// What main alone should hear (the preview's events, the dock's
    /// actions) never reaches the dock webview; main does hear it.
    #[test]
    fn main_events_reach_main_alone() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("the mock app builds");
        WebviewWindowBuilder::new(&app, MAIN_WINDOW, WebviewUrl::App("index.html".into()))
            .build()
            .expect("the main window builds");
        let main = crate::background::main_window(&app).expect("the main window");
        let dock = main
            .add_child(
                WebviewBuilder::new(DOCK_WEBVIEW, WebviewUrl::App("dock-view".into())),
                LogicalPosition::new(0., 0.),
                LogicalSize::new(100., 100.),
            )
            .expect("the dock child");
        let main_webview = crate::background::main_webview(&app).expect("main's webview");
        let heard = |webview: &tauri::Webview<tauri::test::MockRuntime>, event: &str| {
            let count = Arc::new(AtomicUsize::new(0));
            let counter = count.clone();
            webview.listen(event.to_string(), move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            });
            count
        };
        let dock_preview = heard(&dock, crate::creations::preview_window::PREVIEW_EVENT);
        let dock_action = heard(&dock, DOCK_ACTION_EVENT);
        let main_preview = heard(
            &main_webview,
            crate::creations::preview_window::PREVIEW_EVENT,
        );
        let main_action = heard(&main_webview, DOCK_ACTION_EVENT);
        let main_snapshot = heard(&main_webview, DOCK_SNAPSHOT_EVENT);
        let dock_snapshot = heard(&dock, DOCK_SNAPSHOT_EVENT);

        crate::creations::preview_window::tell_main(
            app.handle(),
            "preview-0123",
            crate::creations::preview_window::Place::Embedded,
            crate::creations::preview_window::PreviewState::Loaded,
        );
        relay_action(
            app.handle(),
            DOCK_WEBVIEW,
            serde_json::json!({ "kind": "send" }),
        )
        .expect("the dock's action is relayed");
        relay_snapshot(app.handle(), MAIN_WINDOW, serde_json::json!({ "v": 1 }))
            .expect("main's snapshot is relayed");
        // The wrong callers are refused.
        assert!(relay_action(app.handle(), MAIN_WINDOW, serde_json::json!({})).is_err());
        assert!(relay_snapshot(app.handle(), DOCK_WEBVIEW, serde_json::json!({})).is_err());

        assert_eq!(dock_preview.load(Ordering::SeqCst), 0);
        assert_eq!(dock_action.load(Ordering::SeqCst), 0);
        assert_eq!(main_snapshot.load(Ordering::SeqCst), 0);
        assert_eq!(main_preview.load(Ordering::SeqCst), 1);
        assert_eq!(main_action.load(Ordering::SeqCst), 1);
        assert_eq!(dock_snapshot.load(Ordering::SeqCst), 1);
    }
}
