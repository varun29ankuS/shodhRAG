//! "Update Shodh on your phone", on iPhone. iOS installs an app only from
//! the App Store or TestFlight, so there is nothing for Shodh to download or
//! verify itself (the one-tap update of update.rs is Android's): when the
//! computer runs another version, the Update screen says where the matching
//! app is, and the user updates it there.

use serde::Serialize;
use tauri::State;

use crate::link::{Link, Status};

/// Nothing is kept between a check and an install on iPhone; the state is
/// here so the app manages the same type on both phones.
#[derive(Default)]
pub struct Updates;

/// What a check found, for the Update screen (the same shape as Android's).
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UpdateCheck {
    /// Nothing for Shodh to install itself: the message says what to do.
    Unavailable { message: String },
}

/// Where the matching app is: TestFlight or the App Store updates it.
#[tauri::command]
pub async fn update_check(link: State<'_, Link>) -> Result<UpdateCheck, String> {
    let message = match link.status() {
        // Which side must change: a phone newer than the computer waits for
        // the computer (`needed` says so in its own words).
        Status::Update { desktop, .. } => {
            match shodh_link::update::needed(env!("CARGO_PKG_VERSION"), &desktop) {
                Ok(version) => format!(
                    "Update Shodh to {version} in TestFlight or the App Store, then open it again."
                ),
                Err(e) => e.to_string(),
            }
        }
        _ => "Your phone and your computer run the same Shodh.".to_string(),
    };
    Ok(UpdateCheck::Unavailable { message })
}

/// iOS installs apps itself; the window never offers this on iPhone.
#[tauri::command]
pub async fn update_install() -> Result<bool, String> {
    Err("Update Shodh in TestFlight or the App Store.".to_string())
}
