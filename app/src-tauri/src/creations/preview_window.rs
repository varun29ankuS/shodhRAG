//! The window a creation's live preview shows in.
//!
//! Agent-built code never runs in the app's webview. The Artifacts tab asks
//! for a preview and Rust opens a borderless window of its own label
//! (`preview-<id>`, [`preview::is_preview_webview`]) over the tab's preview
//! area, owned by the main window (an owned window on Windows, a child
//! window on macOS), so it stays above Shodh, follows it when it moves, and
//! goes when it goes. No capability names that label, so Tauri's ACL refuses
//! the page every app command, plugin command and event, on any origin
//! (`acl_manifest.rs`).
//!
//! It is a top-level window, not a webview inside the main window: Tauri
//! 2.9's child webviews need its `unstable` feature, which also changes how
//! every other window's webview is sized (each gets bounds that follow the
//! window by ratio), on an API without semver promises.
//!
//! The window is locked down:
//! - it navigates only within its own token's path ([`navigation_allowed`]),
//!   opens no other window and starts no download;
//! - it is private (incognito): nothing it stores is kept, or shared with
//!   the app's storage;
//! - it runs the WebRTC guard ([`preview::GUARD_SCRIPT`]) before every
//!   document's own scripts, and keeps the app's browser arguments (WebRTC
//!   gets no direct UDP on Windows);
//! - devtools only in debug builds.
//!
//! The page cannot reach the app: it has no handle to the main window (Rust
//! opened it), and it may not emit events. What the tab learns about it,
//! that it loaded or tried to leave, Rust sends to the main window alone
//! ([`PREVIEW_EVENT`]), at most once each per window.
//!
//! One preview shows at a time. It is created already shown and never shown
//! again later (that would take the keyboard focus from Shodh), so hiding
//! it means closing it: on leaving the pane or the tab, on another version,
//! when the internet switch changes, when a dialog opens over it, when the
//! creation is deleted, and when the app's page reloads or its window closes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent};
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, PhysicalPosition, PhysicalSize, Runtime, Url,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window, WindowEvent,
};

use super::preview;
use crate::background::MAIN_WINDOW;

/// What the main window hears about its preview ([`PreviewEvent`]).
pub const PREVIEW_EVENT: &str = "creation-preview";

/// Largest coordinate an area may name, in CSS pixels.
const MAX_CSS_PIXELS: f64 = 16_384.0;

/// The tab's preview area in the main window: its box in CSS pixels from
/// the top left of the app's page, and the page's own size in CSS pixels.
/// The ratio of the page's size to the webview's size on screen turns one
/// into the other, whatever the display scale and the text size zoom.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
}

impl Area {
    /// The area, when it is a box inside the app's page.
    pub fn checked(self) -> Result<Self, String> {
        let numbers = [
            self.x,
            self.y,
            self.width,
            self.height,
            self.viewport_width,
            self.viewport_height,
        ];
        let inside = numbers
            .iter()
            .all(|n| n.is_finite() && (0.0..=MAX_CSS_PIXELS).contains(n))
            && self.width >= 1.0
            && self.height >= 1.0
            && self.viewport_width >= 1.0
            && self.viewport_height >= 1.0
            && self.x + self.width <= self.viewport_width + 1.0
            && self.y + self.height <= self.viewport_height + 1.0;
        if inside {
            Ok(self)
        } else {
            Err("That is not a place inside Shodh's window.".into())
        }
    }

    /// Where the area is on screen, given where the app's page is and how
    /// large it is there (the main window's inner position and size).
    pub fn on_screen(
        &self,
        page_origin: PhysicalPosition<i32>,
        page_size: PhysicalSize<u32>,
    ) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
        let scale_x = f64::from(page_size.width) / self.viewport_width;
        let scale_y = f64::from(page_size.height) / self.viewport_height;
        let left = (self.x * scale_x).round();
        let top = (self.y * scale_y).round();
        let right = ((self.x + self.width) * scale_x).round();
        let bottom = ((self.y + self.height) * scale_y).round();
        (
            PhysicalPosition::new(page_origin.x + left as i32, page_origin.y + top as i32),
            PhysicalSize::new(
                (right - left).max(1.0) as u32,
                (bottom - top).max(1.0) as u32,
            ),
        )
    }
}

/// What the main window hears about its preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewState {
    /// The first page finished loading.
    Loaded,
    /// The page tried to go somewhere other than its own files, or to open
    /// a window or a download; it was stopped.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEvent {
    pub label: String,
    pub state: PreviewState,
}

/// The preview window shown now, and where.
struct Shown {
    label: String,
    area: Area,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

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

/// The preview window of `token` showing `url`, with every lock on and not
/// yet placed.
fn locked_builder<'a, R: Runtime, M: Manager<R>>(
    manager: &'a M,
    app: &AppHandle<R>,
    label: &str,
    token: &str,
    url: Url,
) -> WebviewWindowBuilder<'a, R, M> {
    let blocked = Arc::new(AtomicBool::new(false));
    let loaded = Arc::new(AtomicBool::new(false));
    let (on_navigation, on_new_window, on_download, on_load) = {
        let nav = (
            app.clone(),
            label.to_string(),
            token.to_string(),
            blocked.clone(),
        );
        let new = (app.clone(), label.to_string(), blocked.clone());
        let download = (app.clone(), label.to_string(), blocked);
        let load = (app.clone(), label.to_string(), loaded);
        (nav, new, download, load)
    };
    // The app's browser arguments (WebView2 shares one browser process per
    // data folder and argument list), then a private profile.
    crate::profile::webview_defaults(WebviewWindowBuilder::new(
        manager,
        label,
        WebviewUrl::CustomProtocol(url),
    ))
    .incognito(true)
    .devtools(cfg!(debug_assertions))
    .initialization_script(preview::GUARD_SCRIPT)
    .on_navigation(move |url| {
        let (app, label, token, blocked) = &on_navigation;
        let allowed = navigation_allowed(url, token);
        if !allowed {
            tracing::info!(window = %label, "a preview tried to leave its own files; stopped");
            if !blocked.swap(true, Ordering::SeqCst) {
                tell_main(app, label, PreviewState::Blocked);
            }
        }
        allowed
    })
    .on_new_window(move |url, _features| {
        let (app, label, blocked) = &on_new_window;
        if new_window_allowed(&url) {
            return NewWindowResponse::Allow;
        }
        tracing::info!(window = %label, "a preview tried to open a window; stopped");
        if !blocked.swap(true, Ordering::SeqCst) {
            tell_main(app, label, PreviewState::Blocked);
        }
        NewWindowResponse::Deny
    })
    .on_download(move |_webview, event| {
        let (app, label, blocked) = &on_download;
        let allowed = download_allowed(&event);
        if !allowed && matches!(event, DownloadEvent::Requested { .. }) {
            tracing::info!(window = %label, "a preview tried to download a file; stopped");
            if !blocked.swap(true, Ordering::SeqCst) {
                tell_main(app, label, PreviewState::Blocked);
            }
        }
        allowed
    })
    .on_page_load(move |_window, payload| {
        let (app, label, loaded) = &on_load;
        if payload.event() == PageLoadEvent::Finished && !loaded.swap(true, Ordering::SeqCst) {
            tell_main(app, label, PreviewState::Loaded);
        }
    })
    .title("Preview")
    .decorations(false)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .skip_taskbar(true)
    .accept_first_mouse(true)
    .background_color(Color(255, 255, 255, 255))
    // Shown once, at creation, without taking the focus from Shodh.
    .visible(true)
    .focused(false)
}

fn place_window<R: Runtime>(
    main: &WebviewWindow<R>,
    preview: &WebviewWindow<R>,
    area: &Area,
) -> tauri::Result<()> {
    let (position, size) = area.on_screen(main.inner_position()?, main.inner_size()?);
    preview.set_position(position)?;
    preview.set_size(size)
}

fn destroy<R: Runtime>(window: &WebviewWindow<R>) {
    if let Err(e) = window.destroy() {
        tracing::warn!(window = %window.label(), error = %e, "a preview window could not be closed");
    }
}

/// Open the preview window `label` on `url` (of `token`) over `area`. Call
/// with [`changing`] held, after [`close`] ended the one before.
pub fn open<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    token: &str,
    url: &str,
    area: Area,
) -> Result<(), String> {
    let area = area.checked()?;
    let url = Url::parse(url).map_err(|e| format!("The preview address is not valid ({e})"))?;
    let main = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("Shodh's window is not open.")?;
    let window_error = |e: tauri::Error| format!("Shodh's window could not be measured ({e})");
    let (position, size) = area.on_screen(
        main.inner_position().map_err(window_error)?,
        main.inner_size().map_err(window_error)?,
    );
    let scale = main.scale_factor().map_err(window_error)?;
    let at = position.to_logical::<f64>(scale);
    let extent = size.to_logical::<f64>(scale);
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Shown {
        label: label.to_string(),
        area,
    });
    let opened = locked_builder(app, app, label, token, url)
        .position(at.x, at.y)
        .inner_size(extent.width, extent.height)
        .parent(&main)
        .and_then(|builder| builder.build());
    let window = match opened {
        Ok(window) => window,
        Err(e) => {
            end_shown(label);
            return Err(format!("The preview window could not open ({e})"));
        }
    };
    // Exactly over the area, in the screen's own pixels.
    if let Err(e) = place_window(&main, &window, &area) {
        tracing::debug!(error = %e, "the preview window could not be placed exactly");
    }
    // A close that came while this one was opening wins.
    let still_shown = SHOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|shown| shown.label == label);
    if !still_shown {
        preview::PREVIEWS.end(label);
        destroy(&window);
        return Err("The preview was closed while it opened.".into());
    }
    Ok(())
}

fn end_shown(label: &str) {
    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    if shown.as_ref().is_some_and(|s| s.label == label) {
        *shown = None;
    }
}

/// Move the preview `label` over `area` (the tab's layout changed).
pub fn place<R: Runtime>(app: &AppHandle<R>, label: &str, area: Area) -> Result<(), String> {
    let area = area.checked()?;
    {
        let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        match shown.as_mut() {
            Some(s) if s.label == label => s.area = area,
            _ => return Err("This preview has closed.".into()),
        }
    }
    reposition(app);
    Ok(())
}

/// Load the preview `label` again (the files changed, or the user asked).
pub fn reload<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<(), String> {
    let window = app
        .get_webview_window(label)
        .filter(|w| preview::is_preview_webview(w.label()))
        .ok_or("This preview has closed.")?;
    window
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

/// [`close`] off the event loop's thread (from a window or page event).
fn close_later<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _changing = changing().await;
        close(&app, None);
    });
}

/// Put the shown preview over its area again (the main window moved or
/// changed size).
fn reposition<R: Runtime>(app: &AppHandle<R>) {
    let Some((label, area)) = SHOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|s| (s.label.clone(), s.area))
    else {
        return;
    };
    let (Some(main), Some(window)) = (
        app.get_webview_window(MAIN_WINDOW),
        app.get_webview_window(&label),
    ) else {
        return;
    };
    // A minimized window has no size to follow; it is placed again on restore.
    if main.is_minimized().unwrap_or(false) {
        return;
    }
    if let Err(e) = place_window(&main, &window, &area) {
        tracing::debug!(error = %e, "the preview window could not follow Shodh's window");
    }
}

/// Window events: the preview follows the main window, closes with it, and
/// is not closed by a key press of its own (the tab decides when it goes).
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let label = window.label();
    if preview::is_preview_webview(label) {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
        }
        return;
    }
    if label != MAIN_WINDOW {
        return;
    }
    match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. } => reposition(window.app_handle()),
        WindowEvent::Destroyed => close_later(window.app_handle()),
        _ => {}
    }
}

/// The app's page started loading (a reload): its tab is gone, so is the
/// preview it showed.
pub fn on_main_page_load<R: Runtime>(webview: &tauri::Webview<R>, event: PageLoadEvent) {
    if webview.label() == MAIN_WINDOW && event == PageLoadEvent::Started {
        close_later(webview.app_handle());
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
    let label = SHOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|s| s.label.clone())?;
    app.get_webview_window(&label)
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
    fn the_area_is_checked_and_scaled_to_the_screen() {
        let area = Area {
            x: 100.0,
            y: 50.0,
            width: 600.0,
            height: 400.0,
            viewport_width: 1000.0,
            viewport_height: 800.0,
        };
        assert_eq!(area.checked(), Ok(area));
        // A 150% display with the text size at 125%: the page is 1000 CSS
        // pixels wide on 1875 screen pixels.
        let (at, size) = area.on_screen(
            PhysicalPosition::new(300, 200),
            PhysicalSize::new(1875, 1500),
        );
        assert_eq!(at, PhysicalPosition::new(300 + 188, 200 + 94));
        assert_eq!(size, PhysicalSize::new(1313 - 188, 844 - 94));
        for bad in [
            Area { x: -1.0, ..area },
            Area { width: 0.0, ..area },
            Area { x: 500.0, ..area },
            Area { y: 500.0, ..area },
            Area {
                viewport_width: 0.0,
                ..area
            },
            Area {
                height: f64::NAN,
                ..area
            },
            Area {
                width: f64::INFINITY,
                ..area
            },
            Area {
                viewport_width: 1e9,
                width: 1e9 - 200.0,
                ..area
            },
        ] {
            assert!(bad.checked().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn new_labels_are_preview_labels() {
        let first = new_label();
        assert!(preview::is_preview_webview(&first), "{first}");
        assert_ne!(first, new_label());
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
        let window = locked_builder(&app, app.handle(), &label, TOKEN, url(&address))
            .build()
            .expect("the preview window builds");
        assert_eq!(window.label(), label);
        assert_eq!(window.url().expect("its address"), url(&address));
    }
}
