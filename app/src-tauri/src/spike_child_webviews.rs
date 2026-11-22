//! SPIKE (throwaway, never merged): child webviews in the main window.
//!
//! With `SHODH_SPIKE_CHILD_WEBVIEWS=1`, once the main window reports ready,
//! this adds to `main`:
//! - a `preview-<id>` child built by `preview_window::locked_webview` (its own
//!   data folder, incognito, dead proxy, guard script, navigation, window and
//!   download guards), showing a real creation's red page through the
//!   preview scheme;
//! - a `dock` child loading `/dock-view` in main's own environment, over the
//!   preview's bottom-right corner.
//!
//! Every fact is one `SPIKE` log line, then `SPIKE PASS` or `SPIKE FAIL`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, Listener, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use crate::creations::preview::{Grant, PREVIEWS};
use crate::creations::preview_window;
use crate::creations::CreationStore;

pub const ENV: &str = "SHODH_SPIKE_CHILD_WEBVIEWS";
const DOCK: &str = "dock";

/// Logical layout inside the main window.
const PREVIEW_AT: (f64, f64) = (40.0, 120.0);
const PREVIEW_SIZE: (f64, f64) = (640.0, 420.0);
const DOCK_AT: (f64, f64) = (480.0, 400.0);
const DOCK_SIZE: (f64, f64) = (320.0, 240.0);
/// Points to check: the overlap (dock's opaque part), the preview only, the
/// main page only, and the dock's transparent left column over the preview.
const OVERLAP: (f64, f64) = (600.0, 470.0);
const PREVIEW_ONLY: (f64, f64) = (200.0, 250.0);
const MAIN_ONLY: (f64, f64) = (300.0, 80.0);
const TRANSPARENT: (f64, f64) = (500.0, 470.0);

const RED_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>spike</title>\
<style>html,body{margin:0;height:100%;background:#ff0000}</style></head>\
<body><p style=\"font:14px sans-serif;color:#fff;padding:8px;margin:0\">Spike preview</p></body></html>";

static STARTED: AtomicBool = AtomicBool::new(false);
static DOCK_REPORTED: AtomicBool = AtomicBool::new(false);
static DOCK_OK: AtomicBool = AtomicBool::new(false);
static PREVIEW_LOADED: AtomicBool = AtomicBool::new(false);

fn enabled() -> bool {
    std::env::var_os(ENV).is_some_and(|v| v == "1")
}

/// Called when the main window reports ready.
pub fn start(app: &AppHandle) {
    if !enabled() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        tracing::info!("SPIKE start");
        match run(&app) {
            Ok(()) => tracing::info!("SPIKE PASS"),
            Err(e) => tracing::error!("SPIKE FAIL {e}"),
        }
    });
}

/// What the dock page reports: how a main-only command and a main-only
/// plugin command went when it called them.
#[tauri::command]
pub fn spike_dock_report(webview: tauri::Webview, main_only: String, plugin: String) {
    let refused = |s: &str| s.starts_with("refused");
    tracing::info!(
        "SPIKE dock report from={} main-only={} plugin={}",
        webview.label(),
        main_only,
        plugin
    );
    DOCK_OK.store(
        webview.label() == DOCK && refused(&main_only) && refused(&plugin),
        Ordering::SeqCst,
    );
    DOCK_REPORTED.store(true, Ordering::SeqCst);
}

fn wait_for(flag: &AtomicBool, what: &str, seconds: u64) -> Result<(), String> {
    for _ in 0..(seconds * 10) {
        if flag.load(Ordering::SeqCst) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("{what} did not happen within {seconds} s"))
}

fn run(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_window(crate::background::MAIN_WINDOW)
        .ok_or("no main window")?;

    // A real creation, served through the preview scheme.
    let data_dir = crate::profile::app_data_dir(app).map_err(|e| format!("data dir: {e}"))?;
    let root = crate::creation_commands::root(app).map_err(|e| format!("creations root: {e}"))?;
    let store = CreationStore::in_dir(&data_dir);
    let creation = store
        .prepare(
            &root,
            "spike-child-webviews",
            "Spike child webviews",
            &chrono::Utc::now().to_rfc3339(),
        )
        .map_err(|e| format!("prepare: {e}"))?;
    std::fs::write(Path::new(&creation.folder).join("index.html"), RED_PAGE)
        .map_err(|e| format!("index.html: {e}"))?;
    let label = preview_window::new_label();
    let (token, url) = PREVIEWS.grant(Grant {
        creation_id: creation.id.clone(),
        version: None,
        network: false,
        data_dir: data_dir.clone(),
        root,
        webview: label.clone(),
    });
    let opening = preview_window::Opening {
        label: &label,
        token: &token,
        url: Url::parse(&url).map_err(|e| format!("url: {e}"))?,
        title: "Spike",
        version: None,
        network: false,
        dark: false,
        data_dir: preview_window::data_folder(&data_dir, false),
    };
    std::fs::create_dir_all(&opening.data_dir).map_err(|e| format!("preview data dir: {e}"))?;
    tracing::info!(
        "SPIKE preview data dir {:?}, main webview data dir {:?}",
        opening.data_dir,
        crate::profile::active().webview_data_dir()
    );
    // The locked builder reports its first load to the main window alone.
    let main_window = app
        .get_webview_window(crate::background::MAIN_WINDOW)
        .ok_or("no main webview window")?;
    main_window.listen(preview_window::PREVIEW_EVENT, |event| {
        tracing::info!("SPIKE main heard {}", event.payload());
        if event.payload().contains("\"loaded\"") {
            PREVIEW_LOADED.store(true, Ordering::SeqCst);
        }
    });
    let builder = preview_window::locked_webview(app, &opening);
    let preview = window
        .add_child(
            builder,
            LogicalPosition::new(PREVIEW_AT.0, PREVIEW_AT.1),
            LogicalSize::new(PREVIEW_SIZE.0, PREVIEW_SIZE.1),
        )
        .map_err(|e| format!("preview add_child: {e}"))?;
    tracing::info!("SPIKE preview child added label={}", preview.label());
    #[cfg(windows)]
    preview_window::lock_platform(app, &label, &preview).map_err(|e| format!("lock: {e}"))?;

    // The dock: main's environment (same arguments and data folder).
    let profile = crate::profile::active();
    let mut dock = WebviewBuilder::new(DOCK, WebviewUrl::App("dock-view".into()))
        .additional_browser_args(crate::profile::process_browser_args())
        .transparent(true)
        .focused(false);
    if let Some(dir) = profile.webview_data_dir() {
        dock = dock
            .data_directory(dir)
            .data_store_identifier(profile.folder_id());
    }
    let dock = window
        .add_child(
            dock,
            LogicalPosition::new(DOCK_AT.0, DOCK_AT.1),
            LogicalSize::new(DOCK_SIZE.0, DOCK_SIZE.1),
        )
        .map_err(|e| format!("dock add_child: {e}"))?;
    tracing::info!("SPIKE dock child added label={}", dock.label());

    wait_for(&PREVIEW_LOADED, "the preview's page load", 30)?;
    wait_for(&DOCK_REPORTED, "the dock's report", 30)?;
    if !DOCK_OK.load(Ordering::SeqCst) {
        return Err("the dock was allowed a main-only command".into());
    }
    let labels: Vec<String> = window
        .webviews()
        .iter()
        .map(|w| w.label().to_string())
        .collect();
    tracing::info!("SPIKE webviews in main: {labels:?}");

    // The top left of the screen, so every checked point is on it.
    window
        .set_position(tauri::PhysicalPosition::new(0, 0))
        .map_err(|e| format!("set_position: {e}"))?;
    std::thread::sleep(Duration::from_secs(1));
    let scale = window.scale_factor().map_err(|e| format!("scale: {e}"))?;
    let inner = window
        .inner_position()
        .map_err(|e| format!("inner position: {e}"))?;
    let screen = |p: (f64, f64)| {
        (
            inner.x + (p.0 * scale).round() as i32,
            inner.y + (p.1 * scale).round() as i32,
        )
    };
    tracing::info!(
        "SPIKE screen points overlap={:?} preview={:?} main={:?} transparent={:?} scale={scale}",
        screen(OVERLAP),
        screen(PREVIEW_ONLY),
        screen(MAIN_ONLY),
        screen(TRANSPARENT)
    );

    #[cfg(windows)]
    windows_checks(app, &window, &preview, &dock, scale)?;
    Ok(())
}

#[cfg(windows)]
fn windows_checks(
    app: &AppHandle,
    window: &tauri::Window,
    preview: &tauri::Webview,
    dock: &tauri::Webview,
    scale: f64,
) -> Result<(), String> {
    use std::sync::mpsc;
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::UI::WindowsAndMessaging::{ChildWindowFromPointEx, CWP_SKIPINVISIBLE};

    /// The webview's browser process id and its container window.
    fn facts(webview: &tauri::Webview) -> Result<(u32, usize), String> {
        let (tx, rx) = mpsc::channel();
        webview
            .with_webview(move |platform| {
                let controller = platform.controller();
                let mut pid = 0u32;
                let mut parent = Default::default();
                // SAFETY: COM calls on the webview's own UI thread (with_webview).
                let result = unsafe {
                    controller
                        .CoreWebView2()
                        .and_then(|core| core.BrowserProcessId(&mut pid))
                        .and_then(|()| controller.ParentWindow(&mut parent))
                };
                let _ = tx.send(
                    result
                        .map(|()| (pid, parent.0 as usize))
                        .map_err(|e| e.to_string()),
                );
            })
            .map_err(|e| format!("with_webview: {e}"))?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|e| format!("no answer from the webview: {e}"))?
    }

    let main = app
        .get_webview(crate::background::MAIN_WINDOW)
        .ok_or("no main webview")?;
    let (main_pid, main_hwnd) = facts(&main)?;
    let (preview_pid, preview_hwnd) = facts(preview)?;
    let (dock_pid, dock_hwnd) = facts(dock)?;
    tracing::info!("SPIKE browser pids main={main_pid} preview={preview_pid} dock={dock_pid}");
    tracing::info!(
        "SPIKE containers main={main_hwnd:#x} preview={preview_hwnd:#x} dock={dock_hwnd:#x}"
    );

    // Container windows are placed asynchronously (SWP_ASYNCWINDOWPOS).
    std::thread::sleep(Duration::from_secs(2));
    let top = window.hwnd().map_err(|e| format!("hwnd: {e}"))?;
    let top = HWND(top.0 as *mut core::ffi::c_void);
    let name = |hwnd: usize| {
        if hwnd == dock_hwnd {
            "dock".to_string()
        } else if hwnd == preview_hwnd {
            "preview".to_string()
        } else if hwnd == main_hwnd {
            "main".to_string()
        } else {
            format!("other({hwnd:#x})")
        }
    };
    let hit = |p: (f64, f64)| {
        let point = POINT {
            x: (p.0 * scale).round() as i32,
            y: (p.1 * scale).round() as i32,
        };
        // SAFETY: a plain Win32 query on our own top-level window.
        let child = unsafe { ChildWindowFromPointEx(top, point, CWP_SKIPINVISIBLE) };
        name(child.0 as usize)
    };
    let (overlap, preview_only, main_only) = (hit(OVERLAP), hit(PREVIEW_ONLY), hit(MAIN_ONLY));
    tracing::info!(
        "SPIKE hit-test overlap={overlap} preview-only={preview_only} elsewhere={main_only}"
    );

    if preview_pid == main_pid {
        return Err("the preview shares main's browser process".into());
    }
    if dock_pid != main_pid {
        return Err("the dock is not in main's browser process".into());
    }
    if (overlap.as_str(), preview_only.as_str(), main_only.as_str()) != ("dock", "preview", "main")
    {
        return Err("the z-order is not main < preview < dock".into());
    }
    Ok(())
}
