//! The window a creation's live preview shows in.
//!
//! Agent-built code never runs in the app's webview. When the user opens a
//! preview (or the agent shows one with `open_artifact`), Rust opens a normal
//! window of its own label (`preview-<id>`, [`preview::is_preview_webview`]):
//! titled, resizable, placed where the user last left it. No capability names
//! that label, so Tauri's ACL refuses the page every app command, plugin
//! command and event, on any origin (`acl_manifest.rs`).
//!
//! The window is locked down:
//! - it navigates only within its own token's path ([`navigation_allowed`]),
//!   opens no other window and starts no download;
//! - it is private (incognito), and on Windows it has a browser process of
//!   its own (its own data folder and arguments): while the internet switch
//!   is off its traffic goes to a dead proxy and WebRTC may use no direct
//!   UDP, so nothing it sends leaves the computer, WebRTC included
//!   ([`browser_args`]);
//! - on Windows it denies every permission request (camera, microphone,
//!   location and the rest) and reports a crashed page; macOS's webview
//!   exposes neither, so there the page guard removes media capture and the
//!   preview's text says what remains ([`preview::GUARD_SCRIPT`]);
//! - the WebRTC and media guard runs before every frame's own scripts;
//! - devtools only in debug builds.
//!
//! The page cannot reach the app: it has no handle to the main window (Rust
//! opened it), and it may not emit events. What the tab learns about it, that
//! it loaded, was stopped, closed or crashed, Rust sends to the main window
//! alone ([`PREVIEW_EVENT`]).
//!
//! One preview shows at a time. It stays open across Shodh's dialogs and tab
//! changes, and closes when the user (or the page) closes it, when the tab
//! opens another version or turns the internet switch, when the creation is
//! deleted, and when Shodh quits. It hides and comes back with the main
//! window when Shodh goes to the tray.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent};
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, PhysicalPosition, PhysicalSize, Runtime, Theme, Url,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window, WindowEvent,
};

use super::preview;
use crate::background::MAIN_WINDOW;

/// What the main window hears about its preview ([`PreviewEvent`]).
pub const PREVIEW_EVENT: &str = "creation-preview";

/// Where the preview's own browser data lives, under the app's data folder
/// (one folder per internet-switch state: WebView2 runs one browser process
/// per data folder, and its arguments must not change while it runs).
const DATA_FOLDER: &str = "preview-webview";

/// Where the window's last place on screen is kept.
const BOUNDS_FILE: &str = "preview-window.json";

/// While the internet switch is off, every request the page's browser
/// process would send goes to this closed local port, so it fails at once.
pub const DEAD_PROXY: &str = "--proxy-server=127.0.0.1:9";

/// The window's size the first time, in logical pixels.
const FIRST_SIZE: (f64, f64) = (1000.0, 760.0);

/// Least part of a remembered window that must be on a screen to be put back
/// there (physical pixels).
const MIN_VISIBLE: (i64, i64) = (120, 80);

/// The surface colour shown before the page's first paint (the app's own
/// `--c-surface`), so a dark theme gets no white flash.
const DARK_SURFACE: Color = Color(18, 18, 21, 255);
const LIGHT_SURFACE: Color = Color(255, 255, 255, 255);

/// What the main window hears about its preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewState {
    /// The first page finished loading.
    Loaded,
    /// The page tried to go somewhere other than its own files, or to open
    /// a window or a download; it was stopped.
    Blocked,
    /// The window closed (the user, the page itself, or Shodh closed it).
    Closed,
    /// The page's browser process stopped (a crash or a hang). Only WebView2
    /// reports it; macOS's webview tells Tauri nothing.
    #[cfg_attr(not(windows), allow(dead_code))]
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEvent {
    pub label: String,
    pub state: PreviewState,
}

/// The preview showing now, as the tab sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shown {
    pub label: String,
    pub creation_id: String,
    /// A saved version, or the folder as it is now (`None`).
    pub version: Option<u32>,
    pub network: bool,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

/// Where the user last left the window, while it is open.
static BOUNDS: Mutex<Option<Bounds>> = Mutex::new(None);

/// Opening and closing happen one at a time (never held by the event loop).
static CHANGING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Held while a preview opens or closes.
pub async fn changing() -> tokio::sync::MutexGuard<'static, ()> {
    CHANGING.lock().await
}

/// A fresh preview window label.
pub fn new_label() -> String {
    format!(
        "{}{}",
        preview::WEBVIEW_PREFIX,
        uuid::Uuid::new_v4().simple()
    )
}

/// The preview showing now.
pub fn shown() -> Option<Shown> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether the preview window of `token` may load `url`: only pages of this
/// app's preview scheme, exactly its origin (no port, no user name), under
/// this token's own path.
pub fn navigation_allowed(url: &Url, token: &str) -> bool {
    let (scheme, host) = preview::origin_parts();
    if url.scheme() != scheme
        || url.host_str() != Some(host)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    // No `.` or `..` part, even escaped, whether or not the URL parser
    // already resolved them.
    let dotted = url.path_segments().is_some_and(|mut parts| {
        parts.any(|part| {
            let part = part.to_ascii_lowercase().replace("%2e", ".");
            part == "." || part == ".."
        })
    });
    let own = format!("/{token}");
    let path = url.path();
    !dotted
        && (path == own
            || path
                .strip_prefix(&own)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// A preview opens no other window (`window.open`, links with a target).
pub fn new_window_allowed(_url: &Url) -> bool {
    false
}

/// A preview downloads nothing (a refused request starts nothing, so no
/// other download report follows).
pub fn download_allowed(_event: &DownloadEvent<'_>) -> bool {
    false
}

/// The preview's browser arguments: the app's own (`base`: WebRTC may use no
/// direct UDP, the language), and with the internet switch off a dead proxy
/// for everything, so WebRTC's only remaining route (TCP through a proxy)
/// leads nowhere either.
pub fn browser_args(base: &str, network: bool) -> String {
    if network {
        base.to_string()
    } else if base.is_empty() {
        DEAD_PROXY.to_string()
    } else {
        format!("{base} {DEAD_PROXY}")
    }
}

/// The preview's own browser data folder for this switch state.
pub fn data_folder(app_data: &Path, network: bool) -> PathBuf {
    app_data
        .join(DATA_FOLDER)
        .join(if network { "online" } else { "offline" })
}

/// The window's title.
pub fn window_title(title: &str, version: Option<u32>) -> String {
    let title: String = title.chars().take(80).collect();
    match version {
        Some(number) => format!("Preview — {title} · Version {number}"),
        None => format!("Preview — {title} · Now"),
    }
}

/// Where the window was, in physical pixels: its outer top left and its
/// inner size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A screen's area, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Whether remembered `bounds` can be put back: a sane size, and enough of
/// it on one of `screens` to grab (a screen may have gone since).
pub fn restorable(bounds: &Bounds, screens: &[Screen]) -> bool {
    let sane = (200..=16_384).contains(&bounds.width) && (150..=16_384).contains(&bounds.height);
    sane && screens.iter().any(|screen| {
        let left = i64::from(bounds.x).max(i64::from(screen.x));
        let top = i64::from(bounds.y).max(i64::from(screen.y));
        let right = (i64::from(bounds.x) + i64::from(bounds.width))
            .min(i64::from(screen.x) + i64::from(screen.width));
        let bottom = (i64::from(bounds.y) + i64::from(bounds.height))
            .min(i64::from(screen.y) + i64::from(screen.height));
        right - left >= MIN_VISIBLE.0 && bottom - top >= MIN_VISIBLE.1
    })
}

fn bounds_file<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    crate::profile::app_data_dir(app)
        .ok()
        .map(|dir| dir.join(BOUNDS_FILE))
}

fn saved_bounds<R: Runtime>(app: &AppHandle<R>) -> Option<Bounds> {
    let text = std::fs::read_to_string(bounds_file(app)?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_bounds<R: Runtime>(app: &AppHandle<R>) {
    let Some(bounds) = *BOUNDS.lock().unwrap_or_else(|e| e.into_inner()) else {
        return;
    };
    let Some(path) = bounds_file(app) else {
        return;
    };
    match serde_json::to_string(&bounds) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                tracing::debug!(error = %e, "the preview window's place could not be saved");
            }
        }
        Err(e) => tracing::debug!(error = %e, "the preview window's place could not be written"),
    }
}

fn note_bounds<R: Runtime>(window: &Window<R>) {
    if window.is_minimized().unwrap_or(true) || window.is_maximized().unwrap_or(true) {
        return;
    }
    if let (Ok(at), Ok(size)) = (window.outer_position(), window.inner_size()) {
        *BOUNDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Bounds {
            x: at.x,
            y: at.y,
            width: size.width,
            height: size.height,
        });
    }
}

fn tell_main<R: Runtime>(app: &AppHandle<R>, label: &str, state: PreviewState) {
    let event = PreviewEvent {
        label: label.to_string(),
        state,
    };
    if let Err(e) = app.emit_to(
        EventTarget::WebviewWindow {
            label: MAIN_WINDOW.to_string(),
        },
        PREVIEW_EVENT,
        event,
    ) {
        tracing::debug!(error = %e, "the preview event could not be sent");
    }
}

/// What a preview window shows and how.
pub struct Opening<'a> {
    pub label: &'a str,
    pub token: &'a str,
    pub url: Url,
    pub title: &'a str,
    pub version: Option<u32>,
    pub network: bool,
    /// The app's theme is dark (no white flash before the first paint).
    pub dark: bool,
    /// The preview's own browser data folder ([`data_folder`]).
    pub data_dir: PathBuf,
}

/// The preview window, with every lock on, not yet shown.
fn locked_builder<'a, R: Runtime, M: Manager<R>>(
    manager: &'a M,
    app: &AppHandle<R>,
    opening: &Opening<'_>,
) -> WebviewWindowBuilder<'a, R, M> {
    let label = opening.label.to_string();
    let blocked = Arc::new(AtomicBool::new(false));
    let loaded = Arc::new(AtomicBool::new(false));
    let on_navigation = (
        app.clone(),
        label.clone(),
        opening.token.to_string(),
        blocked.clone(),
    );
    let on_new_window = (app.clone(), label.clone(), blocked.clone());
    let on_download = (app.clone(), label.clone(), blocked);
    let on_load = (app.clone(), label, loaded);
    WebviewWindowBuilder::new(
        manager,
        opening.label,
        WebviewUrl::CustomProtocol(opening.url.clone()),
    )
    // Its own browser process (Windows), arguments and private profile.
    .additional_browser_args(&browser_args(
        crate::profile::process_browser_args(),
        opening.network,
    ))
    .data_directory(opening.data_dir.clone())
    .incognito(true)
    .devtools(cfg!(debug_assertions))
    .initialization_script_for_all_frames(preview::GUARD_SCRIPT)
    .on_navigation(move |url| {
        let (app, label, token, blocked) = &on_navigation;
        let allowed = navigation_allowed(url, token);
        // Logged and reported once per window: a page looping on it floods nothing.
        if !allowed && !blocked.swap(true, Ordering::SeqCst) {
            tracing::info!(window = %label, "a preview tried to leave its own files; stopped");
            tell_main(app, label, PreviewState::Blocked);
        }
        allowed
    })
    .on_new_window(move |url, _features| {
        let (app, label, blocked) = &on_new_window;
        if new_window_allowed(&url) {
            return NewWindowResponse::Allow;
        }
        if !blocked.swap(true, Ordering::SeqCst) {
            tracing::info!(window = %label, "a preview tried to open a window; stopped");
            tell_main(app, label, PreviewState::Blocked);
        }
        NewWindowResponse::Deny
    })
    .on_download(move |_webview, event| {
        let (app, label, blocked) = &on_download;
        let allowed = download_allowed(&event);
        if !allowed
            && matches!(event, DownloadEvent::Requested { .. })
            && !blocked.swap(true, Ordering::SeqCst)
        {
            tracing::info!(window = %label, "a preview tried to download a file; stopped");
            tell_main(app, label, PreviewState::Blocked);
        }
        allowed
    })
    .on_page_load(move |window, payload| {
        let (app, label, loaded) = &on_load;
        if payload.event() == PageLoadEvent::Finished && !loaded.swap(true, Ordering::SeqCst) {
            // A page with no background of its own is white, as in a browser.
            if let Err(e) = window.set_background_color(Some(LIGHT_SURFACE)) {
                tracing::debug!(error = %e, "the preview's background could not be set");
            }
            tell_main(app, label, PreviewState::Loaded);
        }
    })
    .title(window_title(opening.title, opening.version))
    .theme(Some(if opening.dark {
        Theme::Dark
    } else {
        Theme::Light
    }))
    .background_color(if opening.dark {
        DARK_SURFACE
    } else {
        LIGHT_SURFACE
    })
    .decorations(true)
    .resizable(true)
    .min_inner_size(320.0, 240.0)
    // Placed first, then shown (the user asked for it, so it takes the focus).
    .visible(false)
}

fn destroy<R: Runtime>(window: &WebviewWindow<R>) {
    if let Err(e) = window.destroy() {
        tracing::warn!(window = %window.label(), error = %e, "a preview window could not be closed");
    }
}

/// Open the preview window described by `opening`. Call with [`changing`]
/// held, after [`close`] ended the one before.
pub fn open<R: Runtime>(
    app: &AppHandle<R>,
    opening: Opening<'_>,
    creation_id: &str,
) -> Result<(), String> {
    if let Err(e) = std::fs::create_dir_all(&opening.data_dir) {
        return Err(format!("The preview's data folder could not be made ({e})"));
    }
    let screens: Vec<Screen> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| Screen {
            x: m.position().x,
            y: m.position().y,
            width: m.size().width,
            height: m.size().height,
        })
        .collect();
    let remembered = saved_bounds(app).filter(|b| restorable(b, &screens));
    let mut builder = locked_builder(app, app, &opening);
    if remembered.is_none() {
        builder = builder.inner_size(FIRST_SIZE.0, FIRST_SIZE.1).center();
    }
    let window = builder
        .build()
        .map_err(|e| format!("The preview window could not open ({e})"))?;
    if let Some(bounds) = remembered {
        let placed = window
            .set_size(PhysicalSize::new(bounds.width, bounds.height))
            .and_then(|()| window.set_position(PhysicalPosition::new(bounds.x, bounds.y)));
        if let Err(e) = placed {
            tracing::debug!(error = %e, "the preview window could not go back to its place");
        }
    }
    #[cfg(windows)]
    {
        let app = app.clone();
        let label = opening.label.to_string();
        let locked = window.with_webview(move |webview| {
            let stopped = Arc::new(AtomicBool::new(false));
            let on_failed = Box::new(move || {
                if !stopped.swap(true, Ordering::SeqCst) {
                    tracing::warn!(window = %label, "a preview's page stopped");
                    tell_main(&app, &label, PreviewState::Stopped);
                }
            });
            if let Err(e) = webview2::lock(&webview, on_failed) {
                tracing::warn!(error = %e, "the preview's permission and crash handlers could not be set");
            }
        });
        if let Err(e) = locked {
            destroy(&window);
            return Err(format!("The preview window could not be locked ({e})"));
        }
    }
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Shown {
        label: opening.label.to_string(),
        creation_id: creation_id.to_string(),
        version: opening.version,
        network: opening.network,
    });
    *BOUNDS.lock().unwrap_or_else(|e| e.into_inner()) = remembered;
    if let Err(e) = window.show().and_then(|()| window.set_focus()) {
        tracing::debug!(error = %e, "the preview window could not be brought up");
    }
    Ok(())
}

fn preview_window<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<WebviewWindow<R>, String> {
    app.get_webview_window(label)
        .filter(|w| preview::is_preview_webview(w.label()))
        .ok_or_else(|| "This preview has closed.".to_string())
}

/// Bring the preview `label` to the front, with the keyboard focus.
pub fn focus<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<(), String> {
    let window = preview_window(app, label)?;
    window
        .unminimize()
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus())
        .map_err(|e| format!("The preview could not come to the front ({e})"))
}

/// Load the preview `label` again (the files changed, or the user asked).
pub fn reload<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<(), String> {
    preview_window(app, label)?
        .reload()
        .map_err(|e| format!("The preview could not reload ({e})"))
}

/// Close the preview `label`, or the one shown now (`None`). Its token
/// stops working at once.
pub fn close<R: Runtime>(app: &AppHandle<R>, label: Option<&str>) {
    let shown = {
        let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        let matches = match label {
            None => true,
            Some(label) => shown.as_ref().is_some_and(|s| s.label == label),
        };
        if matches {
            shown.take().map(|s| s.label)
        } else {
            None
        }
    };
    save_bounds(app);
    // The shown one, and the named one even when another is shown now.
    let mut labels: Vec<&str> = shown.iter().map(String::as_str).chain(label).collect();
    labels.dedup();
    for label in labels {
        if !preview::is_preview_webview(label) {
            continue;
        }
        preview::PREVIEWS.end(label);
        if let Some(window) = app.get_webview_window(label) {
            destroy(&window);
        }
    }
}

/// The preview window is gone, however it went (closed by the user, by the
/// page itself with `window.close()`, or by Shodh): its token ends and the
/// tab is told.
fn on_preview_destroyed<R: Runtime>(app: &AppHandle<R>, label: &str) {
    save_bounds(app);
    {
        let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        if shown.as_ref().is_some_and(|s| s.label == label) {
            *shown = None;
        }
    }
    preview::PREVIEWS.end(label);
    tell_main(app, label, PreviewState::Closed);
}

/// Window events: the preview remembers where the user puts it, and its end
/// is noticed however it comes; it closes when the main window does (quit).
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let label = window.label();
    if preview::is_preview_webview(label) {
        match event {
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => note_bounds(window),
            WindowEvent::Destroyed => on_preview_destroyed(window.app_handle(), label),
            _ => {}
        }
        return;
    }
    if label == MAIN_WINDOW && matches!(event, WindowEvent::Destroyed) {
        let app = window.app_handle().clone();
        tauri::async_runtime::spawn(async move {
            let _changing = changing().await;
            close(&app, None);
        });
    }
}

/// The main window went to the tray: so does the preview.
pub fn hide_with_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = shown_window(app) {
        if let Err(e) = window.hide() {
            tracing::warn!(error = %e, "the preview window could not be hidden");
        }
    }
}

/// The main window came back from the tray: so does the preview. Call
/// before the main window takes the focus.
pub fn show_with_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = shown_window(app) {
        if let Err(e) = window.show() {
            tracing::warn!(error = %e, "the preview window could not be shown");
        }
    }
}

fn shown_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let label = shown()?.label;
    app.get_webview_window(&label)
}

#[cfg(windows)]
mod webview2 {
    //! WebView2 hooks wry does not offer: deny every permission request, and
    //! notice a crashed page.

    use tauri::webview::PlatformWebview;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_PROCESS_FAILED_KIND,
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    };
    use webview2_com::{PermissionRequestedEventHandler, ProcessFailedEventHandler};

    /// Whether a failed process leaves the page dead (not a GPU or utility
    /// process WebView2 restarts by itself).
    pub fn stops_the_page(kind: COREWEBVIEW2_PROCESS_FAILED_KIND) -> bool {
        [
            COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
            COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
        ]
        .contains(&kind)
    }

    /// Runs on the UI thread (inside `with_webview`).
    pub fn lock(webview: &PlatformWebview, on_failed: Box<dyn Fn()>) -> windows_core::Result<()> {
        // Every permission (camera, microphone, location, notifications,
        // clipboard and the rest) is denied without asking.
        let deny = PermissionRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                // SAFETY: a COM call on the event's own arguments, on the UI thread.
                unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
            }
            Ok(())
        }));
        let failed = ProcessFailedEventHandler::create(Box::new(move |_, args| {
            let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
            if let Some(args) = args {
                // SAFETY: a COM call on the event's own arguments, on the UI thread.
                unsafe { args.ProcessFailedKind(&mut kind)? };
            }
            if stops_the_page(kind) {
                on_failed();
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: COM calls on the webview's own UI thread (with_webview runs
        // the closure there); WebView2 holds its own references to the handlers.
        unsafe {
            let core = webview.controller().CoreWebView2()?;
            core.add_PermissionRequested(&deny, &mut token)?;
            core.add_ProcessFailed(&failed, &mut token)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn a_preview_navigates_only_within_its_own_files() {
        let own = format!("{}/{TOKEN}", preview::origin());
        for allowed in [
            format!("{own}/index.html"),
            format!("{own}/"),
            own.clone(),
            format!("{own}/docs/page.html?x=1#top"),
            format!("{own}/my%20page.html"),
        ] {
            assert!(navigation_allowed(&url(&allowed), TOKEN), "{allowed}");
        }
        let other = "f".repeat(32);
        let (scheme, host) = preview::origin_parts();
        for refused in [
            // Another preview, or this token as a mere prefix.
            format!("{}/{other}/index.html", preview::origin()),
            format!("{own}0/index.html"),
            format!("{own}abc"),
            format!("{own}/../{other}/index.html"),
            format!("{own}/%2e%2e/{other}/index.html"),
            format!("{}/", preview::origin()),
            // Lookalike hosts, a port, a user name.
            format!("{scheme}://{host}.evil.example/{TOKEN}/index.html"),
            format!("{scheme}://{host}:8080/{TOKEN}/index.html"),
            format!("{scheme}://user@{host}/{TOKEN}/index.html"),
            format!("{scheme}://user:pw@{host}/{TOKEN}/index.html"),
            // The other platform's form, and the https form on Windows.
            format!("https://shodh-preview.localhost/{TOKEN}/index.html"),
            format!("shodh-preview://evil/{TOKEN}/index.html"),
            // The app, its IPC, the web, and documents of no address.
            "http://tauri.localhost/".to_string(),
            "tauri://localhost/".to_string(),
            "http://ipc.localhost/get_app_settings".to_string(),
            format!("https://example.com/{TOKEN}/index.html"),
            "about:blank".to_string(),
            "data:text/html,<p>hi</p>".to_string(),
            format!("blob:{}/{TOKEN}/0a1b", preview::origin()),
            "javascript:alert(1)".to_string(),
            "file:///C:/Users/".to_string(),
        ] {
            assert!(!navigation_allowed(&url(&refused), TOKEN), "{refused}");
        }
        // Windows' form is the only one there, and the other only elsewhere.
        let windows_form = url(&format!(
            "http://shodh-preview.localhost/{TOKEN}/index.html"
        ));
        let other_form = url(&format!("shodh-preview://localhost/{TOKEN}/index.html"));
        assert_eq!(navigation_allowed(&windows_form, TOKEN), cfg!(windows));
        assert_eq!(navigation_allowed(&other_form, TOKEN), !cfg!(windows));
    }

    #[test]
    fn a_preview_opens_no_window_and_downloads_nothing() {
        let own = format!("{}/{TOKEN}/index.html", preview::origin());
        for target in ["https://example.com/", "about:blank", own.as_str()] {
            assert!(!new_window_allowed(&url(target)), "{target}");
        }
        let mut destination = std::env::temp_dir().join("x.zip");
        let requested = DownloadEvent::Requested {
            url: url(&format!("{}/{TOKEN}/x.zip", preview::origin())),
            destination: &mut destination,
        };
        assert!(!download_allowed(&requested));
    }

    #[test]
    fn offline_previews_send_everything_to_a_dead_proxy_in_their_own_folder() {
        let base = "--force-webrtc-ip-handling-policy=disable_non_proxied_udp --disable-gpu";
        let offline = browser_args(base, false);
        assert!(offline.starts_with(base));
        assert!(offline.ends_with(DEAD_PROXY));
        assert_eq!(DEAD_PROXY, "--proxy-server=127.0.0.1:9");
        assert_eq!(browser_args(base, true), base);
        assert_eq!(browser_args("", false), DEAD_PROXY);
        // The app's own arguments keep WebRTC off direct UDP in every window.
        assert!(crate::profile::process_browser_args()
            .contains("--force-webrtc-ip-handling-policy=disable_non_proxied_udp"));
        // One folder per switch state (a browser process's arguments are
        // fixed while it runs), apart from the app's own storage.
        let data = Path::new("data");
        assert_eq!(
            data_folder(data, false),
            data.join("preview-webview").join("offline")
        );
        assert_eq!(
            data_folder(data, true),
            data.join("preview-webview").join("online")
        );
        assert_ne!(data_folder(data, false), data_folder(data, true));
    }

    #[test]
    fn the_window_is_titled_with_its_creation_and_version() {
        assert_eq!(
            window_title("Snake", Some(3)),
            "Preview — Snake · Version 3"
        );
        assert_eq!(window_title("Snake", None), "Preview — Snake · Now");
        assert_eq!(
            window_title(&"x".repeat(200), None).chars().count(),
            "Preview — ".chars().count() + 80 + " · Now".chars().count()
        );
    }

    #[test]
    fn a_remembered_place_comes_back_only_onto_a_screen() {
        let screens = [
            Screen {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            Screen {
                x: 1920,
                y: 0,
                width: 2560,
                height: 1440,
            },
        ];
        let on_second = Bounds {
            x: 2400,
            y: 100,
            width: 1000,
            height: 760,
        };
        assert!(restorable(&on_second, &screens));
        // That screen is gone.
        assert!(!restorable(&on_second, &screens[..1]));
        // Only a sliver on screen, or no sane size.
        assert!(!restorable(
            &Bounds {
                x: 1900,
                y: 1060,
                ..on_second
            },
            &screens[..1]
        ));
        assert!(!restorable(
            &Bounds {
                width: 10,
                ..on_second
            },
            &screens
        ));
        assert!(!restorable(&on_second, &[]));
        // Negative coordinates (a screen left of the main one) are fine.
        let left = [Screen {
            x: -1280,
            y: 0,
            width: 1280,
            height: 1024,
        }];
        assert!(restorable(
            &Bounds {
                x: -1200,
                y: 50,
                width: 900,
                height: 700
            },
            &left
        ));
        let text = serde_json::to_string(&on_second).unwrap();
        assert_eq!(serde_json::from_str::<Bounds>(&text).unwrap(), on_second);
    }

    #[test]
    fn new_labels_are_preview_labels() {
        let first = new_label();
        assert!(preview::is_preview_webview(&first), "{first}");
        assert_ne!(first, new_label());
    }

    #[test]
    fn the_tab_hears_four_plain_states() {
        let event = |state| {
            serde_json::to_value(PreviewEvent {
                label: "preview-x".into(),
                state,
            })
            .unwrap()
        };
        for (state, word) in [
            (PreviewState::Loaded, "loaded"),
            (PreviewState::Blocked, "blocked"),
            (PreviewState::Closed, "closed"),
            (PreviewState::Stopped, "stopped"),
        ] {
            assert_eq!(
                event(state),
                serde_json::json!({ "label": "preview-x", "state": word })
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_crashed_or_hung_page_counts_as_stopped() {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        assert!(webview2::stops_the_page(
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
        ));
        assert!(webview2::stops_the_page(
            COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED
        ));
        assert!(webview2::stops_the_page(
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
        ));
        assert!(!webview2::stops_the_page(
            COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED
        ));
    }

    /// The locked builder builds, with its label and address, in the mock
    /// runtime (which runs no navigation, so the predicates above are the
    /// locks' tests).
    #[test]
    fn the_locked_window_builds_on_its_own_address() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("the mock app builds");
        let label = new_label();
        let address = format!("{}/{TOKEN}/index.html", preview::origin());
        let opening = Opening {
            label: &label,
            token: TOKEN,
            url: url(&address),
            title: "Snake",
            version: Some(2),
            network: false,
            dark: true,
            data_dir: std::env::temp_dir().join("shodh-preview-test"),
        };
        let window = locked_builder(&app, app.handle(), &opening)
            .build()
            .expect("the preview window builds");
        assert_eq!(window.label(), label);
        assert_eq!(window.url().expect("its address"), url(&address));
    }
}
