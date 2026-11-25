//! The launch test's check of the embedded preview, in the binary testers get.
//!
//! It runs only when both are true: `SHODH_LAUNCH_PROBE=embedded-preview`, and
//! the instance is a separate profile (`SHODH_DATA_DIR` names its own folder),
//! so a user's normal profile and real data never run it ([`probe_requested`]).
//! Once the main window is ready it:
//! 1. makes a sample creation (a solid red page) in that profile;
//! 2. embeds its preview through the same code as `creation_preview_embed`,
//!    places it with a fixed rectangle, and publishes a chat snapshot so the
//!    floating chat draws over the page's bottom-right corner;
//! 3. checks, on Windows and macOS, which webview is under four points: where
//!    the chat covers the page (the chat), just left of the chat's box (the
//!    page: the chat's webview is only as large as its box), the page alone
//!    (the page) and above it (the app); and logs the screen points, which
//!    the launch job reads the screen's pixels at;
//! 4. waits for the job to say it took its screenshot, then reports the pane
//!    gone, as leaving Artifacts does, and checks both layers hid;
//! 5. then, through the main window's own frontend ([`creator_finish`]): a
//!    Creator build's moves as the assistant sends them (the page opening in
//!    Artifacts, then the design check Creator runs before finishing, a real
//!    one of the sample page). The page must come back and stay shown, with
//!    no pane opened over it. Then the user opens the check's findings: they
//!    dock beside the page, which narrows and stays on top where it is, and
//!    the job reads the pixels of both.
//!
//! Each fact is one `Launch probe:` log line, then `Launch probe: PASS` or
//! `Launch probe: FAIL <what>`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use shodh_rag::harness::events::NavigationTarget;
use tauri::{AppHandle, Emitter, Listener, LogicalPosition, LogicalSize, Manager};

use crate::creations::preview_embed::{self, CssRect, Placement, Visibility};
use crate::creations::preview_window::{Place, PREVIEW_EVENT};
use crate::creations::CreationStore;
use crate::profile::Profile;

/// The variable naming the probe to run.
pub const PROBE_ENV: &str = "SHODH_LAUNCH_PROBE";
/// The one probe there is.
pub const EMBEDDED_PREVIEW: &str = "embedded-preview";
/// The file the launch job writes into the profile's folder once it has its
/// screenshot; the probe then hides the layers.
pub const CAPTURED_FILE: &str = "launch-probe-captured";

/// The sample page: solid red, so the job can tell it from the app and the chat.
const PAGE: &str = include_str!("../tests/fixtures/launch_probe.html");

/// The main window's size during the probe (logical), small enough for a
/// runner's screen, so every point is on it.
const WINDOW: (f64, f64) = (1000.0, 700.0);
/// The pane's rectangle the probe reports (CSS pixels of the main page).
const PANE: CssRect = CssRect {
    x: 40.0,
    y: 120.0,
    width: 640.0,
    height: 420.0,
};

static STARTED: AtomicBool = AtomicBool::new(false);
static LOADED: AtomicBool = AtomicBool::new(false);

/// Whether this instance runs the probe: by name, and only on a separate profile.
pub fn probe_requested(env_value: Option<&OsStr>, profile: &Profile) -> bool {
    profile.is_separate() && env_value == Some(OsStr::new(EMBEDDED_PREVIEW))
}

/// Called when the main window reports ready.
pub fn start(app: &AppHandle) {
    if !probe_requested(
        std::env::var_os(PROBE_ENV).as_deref(),
        crate::profile::active(),
    ) || STARTED.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tracing::info!("Launch probe: start");
        match run(&app).await {
            Ok(()) => tracing::info!("Launch probe: PASS"),
            Err(e) => tracing::error!("Launch probe: FAIL {e}"),
        }
    });
}

async fn wait_for(what: &str, seconds: u64, done: impl Fn() -> bool) -> Result<(), String> {
    for _ in 0..(seconds * 10) {
        if done() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("{what} did not happen within {seconds} s"))
}

/// A chat snapshot as the main window publishes one (`lib/dockProtocol.ts`):
/// the open panel of a conversation with nothing in it yet.
fn snapshot() -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "conversationId": null,
        "title": "Launch probe",
        "lastUser": null,
        "transcript": null,
        "trimmed": false,
        "runtime": {
            "status": null,
            "statusError": null,
            "installed": null,
            "installing": false,
            "progress": null,
            "installError": null,
            "installOffline": false,
            "justInstalled": null,
            "installedAt": null
        },
        "isStreaming": false,
        "busyElsewhere": false,
        "sideRunLabel": null,
        "readOnlyConnector": null,
        "draft": "",
        "dictating": false,
        "voiceSupported": false,
        "open": true,
        "unread": 0,
        "theme": "dark",
        "work": "research",
        "permission": "ask",
        "codeAvailable": false,
        "model": { "label": "Launch probe", "ready": false },
        "sendKey": "enter"
    })
}

/// The points checked, in logical pixels of the main window's content area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Points {
    /// Where the chat covers the page.
    pub overlap: (f64, f64),
    /// Just left of the chat's box, on the page.
    pub outside: (f64, f64),
    /// On the page alone.
    pub preview: (f64, f64),
    /// Above the page, on the app.
    pub main: (f64, f64),
}

/// The points for a page at `preview` and the chat at `dock`, or why they cannot
/// be chosen (the chat does not cover the page's corner, or leaves no room
/// beside it).
pub fn points(preview: CssRect, dock: CssRect) -> Result<Points, String> {
    let right = (preview.x + preview.width).min(dock.x + dock.width);
    let bottom = (preview.y + preview.height).min(dock.y + dock.height);
    let left = preview.x.max(dock.x);
    let top = preview.y.max(dock.y);
    if right - left < 30.0 || bottom - top < 60.0 {
        return Err(format!(
            "the chat {dock:?} does not cover the page {preview:?} enough to check"
        ));
    }
    // Low in the overlap, clear of a system dialog in the screen's middle.
    let y = bottom - 40.0;
    let outside = (dock.x - 4.0, y);
    if outside.0 <= preview.x + 4.0 {
        return Err("no page is left of the chat's box".to_string());
    }
    Ok(Points {
        overlap: (right - 20.0, y),
        outside,
        preview: (preview.x + 160.0, preview.y + 130.0),
        main: (preview.x + 260.0, (preview.y - 40.0).max(1.0)),
    })
}

async fn run(app: &AppHandle) -> Result<(), String> {
    let window = crate::background::main_window(app).ok_or_else(|| "no main window".to_string())?;
    window
        .set_size(LogicalSize::new(WINDOW.0, WINDOW.1))
        .and_then(|()| window.set_position(LogicalPosition::new(0.0, 0.0)))
        .map_err(|e| format!("the main window could not be placed: {e}"))?;

    // A real creation in the profile's folder, served through the preview scheme.
    let data_dir = crate::profile::app_data_dir(app).map_err(|e| format!("data dir: {e}"))?;
    let root = crate::creation_commands::root(app).map_err(|e| format!("creations root: {e}"))?;
    let store = CreationStore::in_dir(&data_dir);
    let creation = store
        .prepare(
            &root,
            "launch-probe",
            "Launch probe",
            &chrono::Utc::now().to_rfc3339(),
        )
        .map_err(|e| format!("the sample creation: {e}"))?;
    std::fs::write(Path::new(&creation.folder).join("index.html"), PAGE)
        .map_err(|e| format!("the sample page: {e}"))?;

    let main = crate::background::main_webview(app).ok_or("no main webview")?;
    main.listen(PREVIEW_EVENT, |event| {
        if event.payload().contains("\"loaded\"") {
            LOADED.store(true, Ordering::SeqCst);
        }
    });
    // What the chat shows, sent first: the dock page asks for it when it is up.
    crate::dock_layer::relay_snapshot(app, crate::background::MAIN_WINDOW, snapshot())?;
    let info = crate::creation_commands::show_preview(
        app,
        &creation.id,
        None,
        false,
        true,
        Place::Embedded,
    )
    .await
    .map_err(|e| format!("embedding: {e:?}"))?;
    let label = info.label;
    preview_embed::place(
        app,
        &label,
        Placement {
            rect: Some(PANE),
            covered: false,
            right_inset: 0.0,
        },
    )?;
    let (current, visibility) = preview_embed::current();
    let shown =
        current.as_deref() == Some(label.as_str()) && matches!(visibility, Visibility::Shown(_));
    tracing::info!("Launch probe: preview embedded label={label} shown={shown}");
    let Visibility::Shown(preview) = visibility else {
        return Err("the placed preview is not shown".to_string());
    };

    wait_for("the page's first load", 30, || {
        LOADED.load(Ordering::SeqCst)
    })
    .await?;
    wait_for("the chat's first size report", 30, crate::dock_layer::sized).await?;
    // The size report moved the chat; let both paint.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let dock = crate::dock_layer::shown_at().ok_or("the chat is not up over the page")?;
    tracing::info!("Launch probe: page at {preview:?}, chat at {dock:?}");
    let at = points(preview, dock)?;

    let hits = hit_test(app, &window, &label, at).await?;
    tracing::info!(
        "Launch probe: hit-test overlap={} outside={} preview={} main={}",
        hits[0],
        hits[1],
        hits[2],
        hits[3]
    );

    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let inner = window.inner_position().map_err(|e| e.to_string())?;
    let screen = |p: (f64, f64)| {
        (
            inner.x + (p.0 * scale).round() as i32,
            inner.y + (p.1 * scale).round() as i32,
        )
    };
    tracing::info!(
        "Launch probe: screen points overlap={:?} outside={:?} preview={:?} main={:?} scale={scale}",
        screen(at.overlap),
        screen(at.outside),
        screen(at.preview),
        screen(at.main)
    );
    let expected = ["dock", "preview", "preview", "main"];
    if hits != expected {
        return Err(format!(
            "under the overlap, beside the chat, on the page and above it: {hits:?}, expected {expected:?}"
        ));
    }

    // The job takes its screenshot, then says so.
    let captured: PathBuf = data_dir.join(CAPTURED_FILE);
    wait_for("the launch job's screenshot", 90, || captured.exists()).await?;

    // Leaving Artifacts: the pane reports itself gone.
    preview_embed::place(
        app,
        &label,
        Placement {
            rect: None,
            covered: true,
            right_inset: 0.0,
        },
    )?;
    let hidden = matches!(preview_embed::current().1, Visibility::Hidden);
    let dock_hidden = crate::dock_layer::shown_at().is_none();
    tracing::info!("Launch probe: preview hidden={hidden} chat hidden={dock_hidden}");
    if !hidden || !dock_hidden {
        return Err("a layer stayed up after the pane went".to_string());
    }

    creator_finish(app, &window, &data_dir, &creation).await
}

/// The event main's frontend acts on for the probe (`features/launch/LaunchProbeHost`).
/// Only Rust can send it: no webview may emit events.
pub const LAUNCH_PROBE_EVENT: &str = "launch-probe";
/// The file the launch job writes once it has its screenshot of the Creator finish.
pub const CREATOR_CAPTURED_FILE: &str = "launch-probe-captured-creator";

/// How long the page must stay shown after the Creator finish's moves, in
/// tenths of a second: the old design_check pane opened within milliseconds.
const STAYS_SHOWN_TENTHS: u32 = 30;

/// A `navigated` event as the assistant's run sends it (`AgentEvent::Navigated`).
fn navigated(
    view: &str,
    target: &NavigationTarget,
    label: &str,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "view": view,
        "focus": null,
        "target": serde_json::to_value(target).map_err(|e| e.to_string())?,
        "label": label,
    }))
}

/// A point on the page clear of the floating chat over it (`dock`): near the
/// page's top left, or in the strip of the page above the chat when it
/// reaches up there.
pub fn page_point(page: CssRect, dock: Option<CssRect>) -> Result<(f64, f64), String> {
    let inside = |p: (f64, f64), r: CssRect| {
        p.0 >= r.x && p.0 <= r.x + r.width && p.1 >= r.y && p.1 <= r.y + r.height
    };
    let corner = (page.x + 20.0, page.y + 20.0);
    let Some(dock) = dock.filter(|d| inside(corner, *d)) else {
        return Ok(corner);
    };
    // The strip of the page above the chat, at its middle.
    let strip = dock.y - page.y;
    if strip >= 8.0 {
        Ok((corner.0, page.y + strip / 2.0))
    } else {
        Err(format!(
            "the chat {dock:?} leaves no strip of the page {page:?} to check"
        ))
    }
}

/// The embedded page now: its rectangle while shown.
fn shown_now(label: &str) -> Option<CssRect> {
    match preview_embed::current() {
        (Some(current), Visibility::Shown(rect)) if current == label => Some(rect),
        _ => None,
    }
}

/// The probe's second part, through main's own frontend: a Creator build's
/// moves as the assistant makes them (the page opens in Artifacts once its
/// folder has index.html; then design_check, as Creator runs it before
/// finish_creation), then the user opening the check's findings. The page
/// must stay shown throughout: the check is the header's badge, and its
/// findings dock beside the page, which narrows, never covered (where
/// PR #198's page stayed hidden after every Creator build).
async fn creator_finish(
    app: &AppHandle,
    window: &tauri::Window,
    data_dir: &Path,
    creation: &crate::creations::Creation,
) -> Result<(), String> {
    use crate::design_check_commands::{keep_and_announce, run_detached, Scope, Subject};
    let label = preview_embed::current()
        .0
        .ok_or("no embedded preview to show again")?;
    let subject = Subject {
        root: PathBuf::from(&creation.folder),
        scope: Scope::Path(".".into()),
        workspace_id: None,
        creation_id: Some(creation.id.clone()),
        label: creation.title.clone(),
    };
    let check = run_detached(subject, Some(creation.conversation_id.clone()), true).await?;
    keep_and_announce(app, &check);
    let shown = crate::agent_tools::design_check::shown_for(&check);
    let open = NavigationTarget::Artifact {
        artifact_id: creation.id.clone(),
        version: None,
        pane: None,
    };
    let to_main = || tauri::EventTarget::Webview {
        label: crate::background::MAIN_WINDOW.to_string(),
    };
    app.emit_to(
        to_main(),
        LAUNCH_PROBE_EVENT,
        serde_json::json!({
            "kind": "follow",
            "navigations": [
                navigated("artifacts", &open, &creation.title)?,
                navigated(shown.view, &shown.target, &shown.label)?,
            ],
        }),
    )
    .map_err(|e| format!("the Creator finish could not reach main: {e}"))?;

    // Main shows the page in Artifacts, then keeps it shown: nothing covers it.
    wait_for("the page in Artifacts after the Creator finish", 30, || {
        shown_now(&label).is_some()
    })
    .await?;
    for _ in 0..STAYS_SHOWN_TENTHS {
        if shown_now(&label).is_none() {
            return Err(
                "the page hid after the design check: something of the app's covers it".to_string(),
            );
        }
        if crate::dock_layer::right_inset() > 0.0 {
            return Err("the assistant's design check of a creation opened a pane".to_string());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let page = shown_now(&label).ok_or("the page hid after the design check")?;
    let at_page = page_point(page, crate::dock_layer::shown_at())?;
    let hits = hit_test(
        app,
        window,
        &label,
        Points {
            overlap: at_page,
            outside: at_page,
            preview: at_page,
            main: at_page,
        },
    )
    .await?;
    tracing::info!(
        "Launch probe: creator finish page at {page:?} hit-test={}",
        hits[0]
    );
    if hits[0] != "preview" {
        return Err(format!(
            "after the Creator finish the page is not on top at {at_page:?}: {}",
            hits[0]
        ));
    }

    // The user opens the findings from the badge: they dock beside the page.
    app.emit_to(
        to_main(),
        LAUNCH_PROBE_EVENT,
        serde_json::json!({ "kind": "openDesign", "checkId": check.id }),
    )
    .map_err(|e| format!("the findings could not be opened: {e}"))?;
    wait_for("the findings docked beside the page", 30, || {
        crate::dock_layer::right_inset() > 0.0 && shown_now(&label).is_some()
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let page = shown_now(&label).ok_or("the page hid when the findings opened beside it")?;
    let inset = crate::dock_layer::right_inset();
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let width = window
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale)
        .width;
    if page.x + page.width > width - inset + 1.0 {
        return Err(format!(
            "the page {page:?} runs under the findings pane ({inset} wide)"
        ));
    }
    let at_page = page_point(page, crate::dock_layer::shown_at())?;
    let at_pane = (width - inset / 2.0, page.y + 20.0);
    let hits = hit_test(
        app,
        window,
        &label,
        Points {
            overlap: at_page,
            outside: at_pane,
            preview: at_page,
            main: at_pane,
        },
    )
    .await?;
    tracing::info!(
        "Launch probe: creator design docked page at {page:?} pane={inset} hit-test page={} pane={}",
        hits[0],
        hits[1]
    );
    if hits[0] != "preview" || hits[1] != "main" {
        return Err(format!(
            "with the findings open: page={} pane={}, expected preview and main",
            hits[0], hits[1]
        ));
    }
    let inner = window.inner_position().map_err(|e| e.to_string())?;
    let screen = |p: (f64, f64)| {
        (
            inner.x + (p.0 * scale).round() as i32,
            inner.y + (p.1 * scale).round() as i32,
        )
    };
    tracing::info!(
        "Launch probe: creator screen points preview={:?} pane={:?}",
        screen(at_page),
        screen(at_pane)
    );
    let captured: PathBuf = data_dir.join(CREATOR_CAPTURED_FILE);
    wait_for(
        "the launch job's screenshot of the Creator finish",
        90,
        || captured.exists(),
    )
    .await?;
    tracing::info!("Launch probe: creator finish shown=true docked=true");
    Ok(())
}

/// Which webview is under each point (overlap, outside, preview, main):
/// `dock`, `preview`, `main` or what else it is.
#[cfg(windows)]
async fn hit_test(
    app: &AppHandle,
    window: &tauri::Window,
    label: &str,
    at: Points,
) -> Result<[String; 4], String> {
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::UI::WindowsAndMessaging::{ChildWindowFromPointEx, CWP_SKIPINVISIBLE};

    /// The webview's container window.
    async fn container(webview: &tauri::Webview) -> Result<usize, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        webview
            .with_webview(move |platform| {
                let mut parent = Default::default();
                // SAFETY: a COM call on the webview's own UI thread (with_webview).
                let result = unsafe { platform.controller().ParentWindow(&mut parent) };
                let _ = tx.send(
                    result
                        .map(|()| parent.0 as usize)
                        .map_err(|e| e.to_string()),
                );
            })
            .map_err(|e| e.to_string())?;
        tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| "no answer from the webview".to_string())?
            .map_err(|e| e.to_string())?
    }

    let webview = |name: &str| app.get_webview(name).ok_or(format!("no {name} webview"));
    let main = container(&webview(crate::background::MAIN_WINDOW)?).await?;
    let preview = container(&webview(label)?).await?;
    let dock = container(&webview(crate::dock_layer::DOCK_WEBVIEW)?).await?;
    let top = window.hwnd().map_err(|e| e.to_string())?;
    let top = HWND(top.0 as *mut core::ffi::c_void);
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let name = |hwnd: usize| match hwnd {
        h if h == dock => "dock".to_string(),
        h if h == preview => "preview".to_string(),
        h if h == main => "main".to_string(),
        other => format!("other({other:#x})"),
    };
    let hit = |p: (f64, f64)| {
        let point = POINT {
            x: (p.0 * scale).round() as i32,
            y: (p.1 * scale).round() as i32,
        };
        // SAFETY: a plain Win32 query on this app's own top-level window.
        let child = unsafe { ChildWindowFromPointEx(top, point, CWP_SKIPINVISIBLE) };
        name(child.0 as usize)
    };
    Ok([
        hit(at.overlap),
        hit(at.outside),
        hit(at.preview),
        hit(at.main),
    ])
}

/// Which webview is under each point (overlap, outside, preview, main), by
/// AppKit's own hit test on the window's content view: the view it returns is
/// inside one webview's WKWebView.
#[cfg(target_os = "macos")]
async fn hit_test(
    app: &AppHandle,
    window: &tauri::Window,
    label: &str,
    at: Points,
) -> Result<[String; 4], String> {
    use std::ffi::{c_char, c_void};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NsPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }

    /// The WKWebView of `webview`, as an address.
    async fn view_of(webview: &tauri::Webview) -> Result<usize, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        webview
            .with_webview(move |platform| {
                let _ = tx.send(platform.inner() as usize);
            })
            .map_err(|e| e.to_string())?;
        tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| "no answer from the webview".to_string())?
            .map_err(|e| e.to_string())
    }

    let webview = |name: &str| app.get_webview(name).ok_or(format!("no {name} webview"));
    let main = view_of(&webview(crate::background::MAIN_WINDOW)?).await?;
    let preview = view_of(&webview(label)?).await?;
    let dock = view_of(&webview(crate::dock_layer::DOCK_WEBVIEW)?).await?;
    let content = window.ns_view().map_err(|e| e.to_string())? as usize;
    let height = {
        let scale = window.scale_factor().map_err(|e| e.to_string())?;
        window
            .inner_size()
            .map_err(|e| e.to_string())?
            .to_logical::<f64>(scale)
            .height
    };
    let points = [at.overlap, at.outside, at.preview, at.main];
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .run_on_main_thread(move || {
            // SAFETY: Objective-C messages to the window's own content view and
            // the views AppKit returns, on the main thread; each selector's
            // signature is cast exactly (hitTest: takes an NSPoint, superview
            // takes nothing; both return an object or nil).
            let found = unsafe {
                let hit_test: extern "C" fn(*mut c_void, *const c_void, NsPoint) -> *mut c_void =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let superview: extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let hit_sel = sel_registerName(c"hitTest:".as_ptr());
                let super_sel = sel_registerName(c"superview".as_ptr());
                points.map(|(x, y)| {
                    // The content view is not flipped: its origin is the bottom left.
                    let mut view = hit_test(
                        content as *mut c_void,
                        hit_sel,
                        NsPoint { x, y: height - y },
                    );
                    while !view.is_null() {
                        let address = view as usize;
                        if address == dock {
                            return "dock".to_string();
                        }
                        if address == preview {
                            return "preview".to_string();
                        }
                        if address == main {
                            return "main".to_string();
                        }
                        view = superview(view, super_sel);
                    }
                    "nothing".to_string()
                })
            };
            let _ = tx.send(found);
        })
        .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .map_err(|_| "no answer from the main thread".to_string())?
        .map_err(|e| e.to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
async fn hit_test(
    _app: &AppHandle,
    _window: &tauri::Window,
    _label: &str,
    _at: Points,
) -> Result<[String; 4], String> {
    Err("the hit test runs on Windows and macOS only".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn the_probe_runs_only_on_a_separate_profile_and_by_name() {
        let separate = Profile::from_env_value(Some(absolute("shodh-probe").as_os_str())).unwrap();
        let normal = Profile::from_env_value(None).unwrap();
        let name = Some(OsStr::new("embedded-preview"));
        assert!(probe_requested(name, &separate));
        assert!(!probe_requested(name, &normal));
        assert!(!probe_requested(Some(OsStr::new("1")), &separate));
        assert!(!probe_requested(Some(OsStr::new("")), &separate));
        assert!(!probe_requested(None, &separate));
        assert!(!probe_requested(None, &normal));
    }

    #[test]
    fn the_creator_finish_checks_the_page_clear_of_the_chat() {
        let page = CssRect {
            x: 280.,
            y: 150.,
            width: 250.,
            height: 530.,
        };
        // No chat, or a chat away from the corner: near the page's top left.
        assert_eq!(page_point(page, None), Ok((300., 170.)));
        // The open chat left of a 450 px docked pane in the probe's window
        // spans the narrowed page: the point goes just above it.
        let dock = crate::dock_layer::dock_bounds_beside(
            (380., 470.),
            WINDOW,
            crate::dock_layer::DOCK_EDGE,
            450.,
        );
        let high = CssRect { y: 200., ..page };
        assert_eq!(
            page_point(high, Some(dock)),
            Ok((300., 200. + (dock.y - 200.) / 2.))
        );
        // A chat over the whole page leaves nothing to check: said, not guessed.
        let low = CssRect {
            y: dock.y + 10.,
            ..page
        };
        assert!(page_point(low, Some(dock)).is_err());
    }

    #[test]
    fn the_points_fall_where_the_job_reads_them() {
        let page = PANE;
        // The open chat at the bottom right of the probe's window (dock_layer::dock_bounds).
        let dock =
            crate::dock_layer::dock_bounds((380., 470.), WINDOW, crate::dock_layer::DOCK_EDGE);
        let at = points(page, dock).expect("the chat covers the page's corner");
        let inside = |p: (f64, f64), r: CssRect| {
            p.0 > r.x && p.0 < r.x + r.width && p.1 > r.y && p.1 < r.y + r.height
        };
        assert!(inside(at.overlap, page) && inside(at.overlap, dock));
        // Just outside the chat's box, still on the page: the click goes to the page.
        assert!(inside(at.outside, page) && !inside(at.outside, dock));
        assert!(dock.x - at.outside.0 <= 4.0);
        assert!(inside(at.preview, page) && !inside(at.preview, dock));
        assert!(!inside(at.main, page) && !inside(at.main, dock));
        // A chat away from the page cannot be checked.
        let away = CssRect {
            x: 900.,
            y: 600.,
            width: 80.,
            height: 40.,
        };
        assert!(points(page, away).is_err());
    }
}
