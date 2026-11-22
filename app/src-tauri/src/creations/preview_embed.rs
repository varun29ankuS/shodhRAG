//! The creation's live preview embedded in the main window, over the
//! Artifacts preview pane.
//!
//! The page runs in a child webview of the main window (`preview-<id>`),
//! built with exactly the preview window's locks
//! ([`preview_window::locked_webview`]): its own browser process and data
//! folder on Windows, private, offline unless the user turned the internet
//! switch on, navigation only within its own token's path, no new windows
//! or downloads, every permission denied. No capability names its label, so
//! the ACL refuses it every command and event, as it does the window.
//!
//! A native child webview paints above the main webview's page, so it must
//! never stay up over something of the app's own. It is **hidden by
//! default**, and shown only while the latest report from the main webview
//! ([`Placement`]) says the pane is on screen and nothing of main's is above
//! it. Rust scales the reported rectangle by the main page's text-size zoom
//! ([`to_logical`]) and clamps it inside the window ([`clamp`]). A reload of
//! the main page, a minimise and a trip to the tray all hide it until main
//! reports again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Rect, Runtime};

use super::preview_window::{self, Opening, Place, Shown};

/// The least size, in logical pixels, worth showing a page at: anything
/// smaller after clamping is hidden rather than shown as a sliver.
pub const MIN_SIDE: f64 = 40.0;

/// Logical pixels kept clear at the top of the main window's content area.
/// The title bar is the platform's own (decorations on), outside the content
/// area on both platforms, so nothing inside it needs reserving.
pub const TOP_RESERVE: f64 = 0.0;

/// A rectangle in the main page's CSS pixels (as reported) or in logical
/// pixels of the main window (after [`to_logical`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CssRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What the main webview reports about the pane: its rectangle (`None`: not
/// on screen) and whether something of main's own is above it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub rect: Option<CssRect>,
    pub covered: bool,
}

/// Whether the embedded page is up, and where.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Visibility {
    Shown(CssRect),
    #[default]
    Hidden,
}

/// The main page's CSS-pixel rectangle in logical pixels: the text-size
/// setting zooms the main webview, so one CSS pixel is `zoom` logical ones.
/// A zoom that is not a positive number counts as 1.
pub fn to_logical(rect: CssRect, zoom: f64) -> CssRect {
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    CssRect {
        x: rect.x * zoom,
        y: rect.y * zoom,
        width: rect.width * zoom,
        height: rect.height * zoom,
    }
}

/// `rect` kept inside `[0, width] × [top_reserve, height]` of a window of
/// `window` logical size, or `None` (hide the page) for a rectangle that is
/// not a real one (NaN, infinite, a negative size) or that leaves less than
/// [`MIN_SIDE`] on either side.
pub fn clamp(rect: CssRect, window: (f64, f64), top_reserve: f64) -> Option<CssRect> {
    let finite = [
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        window.0,
        window.1,
        top_reserve,
    ]
    .iter()
    .all(|v| v.is_finite());
    if !finite || rect.width < 0.0 || rect.height < 0.0 {
        return None;
    }
    let left = rect.x.max(0.0);
    let top = rect.y.max(top_reserve);
    let right = (rect.x + rect.width).min(window.0);
    let bottom = (rect.y + rect.height).min(window.1);
    let (width, height) = (right - left, bottom - top);
    (width >= MIN_SIDE && height >= MIN_SIDE).then_some(CssRect {
        x: left,
        y: top,
        width,
        height,
    })
}

/// The embedded preview's visibility, decided from the main webview's
/// reports alone. Hidden is the default and every doubt goes to it.
#[derive(Debug, Default)]
pub struct EmbedState {
    /// The embedded preview now, if any.
    label: Option<String>,
    visibility: Visibility,
    minimised: bool,
}

impl EmbedState {
    pub const fn new() -> Self {
        Self {
            label: None,
            visibility: Visibility::Hidden,
            minimised: false,
        }
    }

    /// The embedded preview now, if any.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// A new embedded preview `label`: hidden until main places it.
    pub fn embedded(&mut self, label: &str) -> Visibility {
        self.label = Some(label.to_string());
        self.visibility = Visibility::Hidden;
        self.visibility
    }

    /// The embedded preview `label` closed. Returns whether it was this one.
    pub fn closed(&mut self, label: &str) -> bool {
        if self.label.as_deref() != Some(label) {
            return false;
        }
        self.label = None;
        self.visibility = Visibility::Hidden;
        true
    }

    /// Main's report about `label`'s pane (its rectangle already in logical
    /// pixels and clamped). A report about another preview changes nothing.
    pub fn place(&mut self, label: &str, placement: Placement) -> Visibility {
        if self.label.as_deref() != Some(label) {
            return self.visibility;
        }
        self.visibility = match placement.rect {
            Some(rect) if !placement.covered && !self.minimised => Visibility::Shown(rect),
            _ => Visibility::Hidden,
        };
        self.visibility
    }

    /// The main page started loading again: nothing will report for the old
    /// page, so the preview hides until the new one does.
    pub fn main_reloaded(&mut self) -> Visibility {
        self.visibility = Visibility::Hidden;
        self.visibility
    }

    /// The main window was minimised (`true`) or came back (`false`). Either
    /// change hides the page: coming back, it waits for main's next report,
    /// since the pane may have moved meanwhile.
    pub fn minimised(&mut self, yes: bool) -> Visibility {
        if self.minimised != yes {
            self.minimised = yes;
            self.visibility = Visibility::Hidden;
        }
        self.visibility
    }

    pub fn visibility(&self) -> Visibility {
        self.visibility
    }
}

static STATE: Mutex<EmbedState> = Mutex::new(EmbedState::new());

/// What was last applied to the child webview, so a report that changes
/// nothing sends nothing to it.
static APPLIED: Mutex<Option<(String, Visibility)>> = Mutex::new(None);

/// The embedded preview's theme, for the window it pops out to.
static DARK: AtomicBool = AtomicBool::new(false);

/// The embedded preview now and its visibility.
pub fn current() -> (Option<String>, Visibility) {
    let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    (state.label().map(str::to_string), state.visibility())
}

/// The embedded preview's theme (the app's, when it was embedded).
pub fn dark() -> bool {
    DARK.load(Ordering::SeqCst)
}

/// Put `visibility` on the child webview `label`, unless it is on already.
fn apply<R: Runtime>(app: &AppHandle<R>, label: &str, visibility: Visibility) {
    {
        let applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
        if applied
            .as_ref()
            .is_some_and(|(l, v)| l == label && *v == visibility)
        {
            return;
        }
    }
    let Some(webview) = app.get_webview(label) else {
        return;
    };
    let result = match visibility {
        Visibility::Shown(rect) => webview
            .set_bounds(Rect {
                position: LogicalPosition::new(rect.x, rect.y).into(),
                size: LogicalSize::new(rect.width, rect.height).into(),
            })
            .and_then(|()| webview.show()),
        Visibility::Hidden => webview.hide(),
    };
    match result {
        Ok(()) => {
            *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((label.to_string(), visibility));
        }
        Err(e) => {
            tracing::warn!(window = %label, error = %e, "the embedded preview could not be placed")
        }
    }
}

/// Embed the preview described by `opening` in the main window, hidden.
/// Call with [`preview_window::changing`] held, after
/// [`preview_window::close`] ended the one before.
pub fn embed<R: Runtime>(
    app: &AppHandle<R>,
    opening: Opening<'_>,
    creation_id: &str,
) -> Result<(), String> {
    if opening.place != Place::Embedded {
        return Err("This preview opens in its own window.".to_string());
    }
    std::fs::create_dir_all(&opening.data_dir)
        .map_err(|e| format!("The preview's data folder could not be made ({e})"))?;
    let main = crate::background::main_window(app)
        .ok_or_else(|| "The main window is not open.".to_string())?;
    let webview = main
        .add_child(
            preview_window::locked_webview(app, &opening),
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(MIN_SIDE, MIN_SIDE),
        )
        .map_err(|e| format!("The preview could not open in Artifacts ({e})"))?;
    let discard = |why: String| {
        if let Err(e) = webview.close() {
            tracing::warn!(window = %opening.label, error = %e, "an embedded preview could not be closed");
        }
        why
    };
    webview
        .hide()
        .map_err(|e| discard(format!("The preview could not be hidden ({e})")))?;
    #[cfg(windows)]
    preview_window::lock_platform(app, opening.label, Place::Embedded, &webview)
        .map_err(|e| discard(format!("The preview could not be locked ({e})")))?;
    DARK.store(opening.dark, Ordering::SeqCst);
    preview_window::set_shown(Shown {
        label: opening.label.to_string(),
        creation_id: creation_id.to_string(),
        version: opening.version,
        network: opening.network,
        place: Place::Embedded,
    });
    let visibility = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .embedded(opening.label);
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((opening.label.to_string(), visibility));
    Ok(())
}

/// Main's report about the pane of `label`: scaled by the main page's zoom,
/// clamped inside the main window, then shown or hidden.
pub fn place<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    placement: Placement,
) -> Result<(), String> {
    let main = crate::background::main_window(app)
        .ok_or_else(|| "The main window is not open.".to_string())?;
    let scale = main.scale_factor().map_err(|e| e.to_string())?;
    let size = main
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale);
    let zoom = crate::app_settings::main_zoom();
    let rect = placement
        .rect
        .map(|rect| to_logical(rect, zoom))
        .and_then(|rect| clamp(rect, (size.width, size.height), TOP_RESERVE));
    let (current, visibility) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let visibility = state.place(
            label,
            Placement {
                rect,
                covered: placement.covered,
            },
        );
        (state.label().map(str::to_string), visibility)
    };
    if let Some(current) = current {
        apply(app, &current, visibility);
    }
    Ok(())
}

/// The main page started loading again: hide the embedded preview.
pub fn main_reloaded<R: Runtime>(app: &AppHandle<R>) {
    let (current, visibility) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let visibility = state.main_reloaded();
        (state.label().map(str::to_string), visibility)
    };
    if let Some(current) = current {
        apply(app, &current, visibility);
    }
}

/// The main window was minimised or hidden (`true`), or came back.
pub fn minimised<R: Runtime>(app: &AppHandle<R>, yes: bool) {
    let (current, visibility) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let visibility = state.minimised(yes);
        (state.label().map(str::to_string), visibility)
    };
    if let Some(current) = current {
        apply(app, &current, visibility);
    }
}

/// Close the embedded preview `label` (its token is ended by the caller).
/// Returns whether there was one to close.
pub fn closed<R: Runtime>(app: &AppHandle<R>, label: &str) -> bool {
    let was_current = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .closed(label);
    {
        let mut applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
        if applied.as_ref().is_some_and(|(l, _)| l == label) {
            *applied = None;
        }
    }
    let Some(webview) = app.get_webview(label) else {
        return was_current;
    };
    if let Err(e) = webview.close() {
        tracing::warn!(window = %label, error = %e, "an embedded preview could not be closed");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CssRect {
        CssRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_rect_is_scaled_by_the_text_zoom() {
        assert_eq!(
            to_logical(rect(10., 20., 300., 200.), 1.25),
            rect(12.5, 25., 375., 250.)
        );
        assert_eq!(
            to_logical(rect(10., 20., 300., 200.), 0.9),
            rect(9., 18., 270., 180.)
        );
        // A zoom that is not a positive number leaves the rectangle as it is.
        for zoom in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                to_logical(rect(10., 20., 300., 200.), zoom),
                rect(10., 20., 300., 200.)
            );
        }
    }

    #[test]
    fn a_rect_is_clamped_inside_the_window_below_the_title_bar() {
        assert_eq!(
            clamp(rect(-50., 0., 5000., 5000.), (1400., 900.), 40.),
            Some(rect(0., 40., 1400., 860.))
        );
        // Inside already: unchanged.
        assert_eq!(
            clamp(rect(300., 120., 800., 600.), (1400., 900.), 40.),
            Some(rect(300., 120., 800., 600.))
        );
    }

    #[test]
    fn a_bad_rect_hides_the_page() {
        let window = (1400., 900.);
        for bad in [
            rect(f64::NAN, 0., 300., 300.),
            rect(0., 0., f64::INFINITY, 300.),
            rect(0., 0., -300., 300.),
            rect(0., 0., 300., -1.),
            rect(100., 100., 30., 30.),
            // Wholly outside the window.
            rect(2000., 100., 300., 300.),
        ] {
            assert_eq!(clamp(bad, window, 40.), None, "{bad:?}");
        }
        assert_eq!(clamp(rect(0., 0., 300., 300.), (f64::NAN, 900.), 0.), None);
    }

    fn shown(at: CssRect) -> Placement {
        Placement {
            rect: Some(at),
            covered: false,
        }
    }

    #[test]
    fn hidden_until_main_says_visible() {
        let mut state = EmbedState::new();
        let at = rect(0., 40., 800., 600.);
        assert_eq!(state.embedded("preview-a"), Visibility::Hidden);
        assert_eq!(state.visibility(), Visibility::Hidden);
        assert_eq!(state.place("preview-a", shown(at)), Visibility::Shown(at));
        assert_eq!(
            state.place(
                "preview-a",
                Placement {
                    rect: Some(at),
                    covered: true
                }
            ),
            Visibility::Hidden
        );
        assert_eq!(state.place("preview-a", shown(at)), Visibility::Shown(at));
        assert_eq!(
            state.place(
                "preview-a",
                Placement {
                    rect: None,
                    covered: false
                }
            ),
            Visibility::Hidden
        );
    }

    #[test]
    fn a_reload_of_main_hides_the_page() {
        let mut state = EmbedState::new();
        let at = rect(0., 40., 800., 600.);
        state.embedded("preview-a");
        state.place("preview-a", shown(at));
        assert_eq!(state.main_reloaded(), Visibility::Hidden);
        assert_eq!(state.visibility(), Visibility::Hidden);
        assert_eq!(state.place("preview-a", shown(at)), Visibility::Shown(at));
    }

    #[test]
    fn minimising_hides_and_restoring_waits_for_main() {
        let mut state = EmbedState::new();
        let at = rect(0., 40., 800., 600.);
        state.embedded("preview-a");
        state.place("preview-a", shown(at));
        assert_eq!(state.minimised(true), Visibility::Hidden);
        // Main may still report while minimised: the page stays hidden.
        assert_eq!(state.place("preview-a", shown(at)), Visibility::Hidden);
        assert_eq!(state.minimised(false), Visibility::Hidden);
        assert_eq!(state.visibility(), Visibility::Hidden);
        assert_eq!(state.place("preview-a", shown(at)), Visibility::Shown(at));
        // A resize that is no minimise changes nothing.
        assert_eq!(state.minimised(false), Visibility::Shown(at));
    }

    #[test]
    fn a_stale_label_is_ignored() {
        let mut state = EmbedState::new();
        let at = rect(0., 40., 800., 600.);
        state.embedded("preview-a");
        assert_eq!(state.place("preview-old", shown(at)), Visibility::Hidden);
        state.place("preview-a", shown(at));
        assert_eq!(
            state.place(
                "preview-old",
                Placement {
                    rect: None,
                    covered: true
                }
            ),
            Visibility::Shown(at)
        );
        assert!(!state.closed("preview-old"));
        assert_eq!(state.label(), Some("preview-a"));
        assert!(state.closed("preview-a"));
        assert_eq!(state.label(), None);
        assert_eq!(state.place("preview-a", shown(at)), Visibility::Hidden);
    }

    #[test]
    fn the_pane_reports_in_the_shape_main_sends() {
        let placement: Placement = serde_json::from_value(serde_json::json!({
            "rect": { "x": 1, "y": 2, "width": 3, "height": 4 },
            "covered": false
        }))
        .expect("a placement");
        assert_eq!(placement, shown(rect(1., 2., 3., 4.)));
        let hidden: Placement =
            serde_json::from_value(serde_json::json!({ "rect": null, "covered": true }))
                .expect("a hidden placement");
        assert_eq!(
            hidden,
            Placement {
                rect: None,
                covered: true
            }
        );
    }

    /// Once the main window holds a child webview, Tauri no longer counts it
    /// as a webview window: `get_webview_window("main")` finds nothing. The
    /// app's own lookups (tray, quit, notifications, text size) go through
    /// the helpers, which still find it.
    #[test]
    fn the_main_window_is_found_while_it_holds_child_webviews() {
        use tauri::{WebviewUrl, WebviewWindowBuilder};
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("the mock app builds");
        WebviewWindowBuilder::new(
            &app,
            crate::background::MAIN_WINDOW,
            WebviewUrl::App("index.html".into()),
        )
        .build()
        .expect("the main window builds");
        assert!(app
            .get_webview_window(crate::background::MAIN_WINDOW)
            .is_some());
        let main = crate::background::main_window(&app).expect("the main window");
        main.add_child(
            tauri::webview::WebviewBuilder::new("child", WebviewUrl::App("index.html".into())),
            LogicalPosition::new(0., 0.),
            LogicalSize::new(100., 100.),
        )
        .expect("a child webview");
        assert!(
            app.get_webview_window(crate::background::MAIN_WINDOW)
                .is_none(),
            "Tauri counts the main window as a webview window again: the helpers may be simplified"
        );
        assert!(crate::background::main_window(&app).is_some());
        assert_eq!(
            crate::background::main_webview(&app).map(|w| w.label().to_string()),
            Some(crate::background::MAIN_WINDOW.to_string())
        );
        let mut zoomed: Vec<String> = crate::app_settings::zoomed_webviews(&app)
            .iter()
            .map(|w| w.label().to_string())
            .collect();
        zoomed.sort();
        assert_eq!(zoomed, ["child", crate::background::MAIN_WINDOW]);
    }
}
