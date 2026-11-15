//! CI's screenshot tour, in a debug build only (lib.rs declares this module
//! under `cfg(debug_assertions)`, so a release build has none of it).
//!
//! The tour shows every screen in one run of the app: relaunching the app
//! for each picture killed the emulator itself. It asks for a screen by
//! setting one system property, `debug.shodh.tour`, to
//! `<request> <screen> <theme>` ("7 demo-library dark"): a new request
//! number for each, so asking for the same screen twice is still a change,
//! and the screen and theme in one value, so the window never sees one
//! without the other ("start" asks for the app's own start, as a tester
//! first sees it). The window opens that screen again from the start
//! (src/lib/tour.ts) and writes its ready line for it, as at launch.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::link::{is_screen_name, system_property};

const TOUR_PROPERTY: &std::ffi::CStr = c"debug.shodh.tour";

/// The window's event for each request (src/lib/tour.ts).
const TOUR_EVENT: &str = "shodh://tour";

/// How often the property is read. A request is answered within this, and
/// reading a property is a read of shared memory, so it costs nothing.
const POLL: Duration = Duration::from_millis(200);

/// A screen the tour asks for, and the theme to show it in ("phone" follows
/// the phone's own setting, as the app does by default).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Request {
    screen: String,
    theme: &'static str,
}

/// A request from the property's value, or `None` when it is not one: only
/// a number, a screen's name and a theme of the closed set reach a log line
/// or the window.
fn parse(value: &str) -> Option<Request> {
    let mut parts = value.split(' ');
    let (number, screen, theme) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some()
        || number.is_empty()
        || number.len() > 9
        || !number.bytes().all(|b| b.is_ascii_digit())
        || !is_screen_name(screen)
    {
        return None;
    }
    let theme = match theme {
        "phone" => "phone",
        "light" => "light",
        "dark" => "dark",
        _ => return None,
    };
    Some(Request {
        screen: screen.to_string(),
        theme,
    })
}

/// Watch the property while the app runs. Its value at start is the last
/// run's (properties last until the phone restarts), so only a change is a
/// request, and the app opens as it always does.
pub fn follow(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("shodh-tour".to_string())
        .spawn(move || {
            let mut last = system_property(TOUR_PROPERTY);
            loop {
                std::thread::sleep(POLL);
                let value = system_property(TOUR_PROPERTY);
                if value == last {
                    continue;
                }
                last = value;
                let Some(request) = parse(&last) else {
                    tracing::warn!("Shodh tour: not a request, so it is ignored");
                    continue;
                };
                tracing::info!(
                    "Shodh tour: the {} screen in the {} theme (debug build)",
                    request.screen,
                    request.theme
                );
                if let Err(e) = app.emit(TOUR_EVENT, &request) {
                    tracing::error!("The tour's request did not reach the window: {e}");
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!("The screenshot tour could not start: {e}");
    }
}
