//! The phone's own calendar, for calendar sync (mobile/src/lib/calsync.ts):
//! which calendars the user chose, and reads and writes of those only
//! (`shodh_link::phone_calendar`). The choice is kept sealed on the phone;
//! a calendar that is not chosen (and is not Shodh's own) is never read or
//! written, whatever the window asks.
//!
//! Also the phone's sealed JSON files for the window: the offline copy of the
//! agenda and tasks, the calendar sync's state, and the phone's settings.

use serde::Deserialize;
use serde_json::{json, Value};
use shodh_link::phone_calendar::CalendarChoice;
use tauri::{AppHandle, Manager, State};

use crate::native::Native;
use crate::store::Store;

const CHOICE_FILE: &str = "calendar-choice.sealed";

/// The sealed files the window may read and write, by name.
const WINDOW_FILES: &[&str] = &["agenda", "calendar-sync", "settings"];

fn native(app: &AppHandle) -> tauri::State<'_, Native<tauri::Wry>> {
    app.state::<Native<tauri::Wry>>()
}

async fn choice(app: &AppHandle, store: &Store) -> Result<CalendarChoice, String> {
    let storage = store.get(app).await?;
    crate::link::blocking(move || {
        storage
            .sealed(CHOICE_FILE)
            .read::<CalendarChoice>()
            .map(Option::unwrap_or_default)
            .map_err(|e| e.to_string())
    })
    .await
}

/// Ask the phone for the calendar permission (once; the user can say no).
#[tauri::command]
pub async fn calendar_access(app: AppHandle) -> Result<bool, String> {
    let reply: Value = native(&app).call("calendarAccess", json!({})).await?;
    Ok(reply["granted"].as_bool().unwrap_or(false))
}

/// The phone's calendars (names and colours), to choose from.
#[tauri::command]
pub async fn calendar_list(app: AppHandle) -> Result<Value, String> {
    let reply: Value = native(&app).call("calendarList", json!({})).await?;
    Ok(reply["calendars"].clone())
}

#[tauri::command]
pub async fn calendar_choice(
    app: AppHandle,
    store: State<'_, Store>,
) -> Result<CalendarChoice, String> {
    choice(&app, &store).await
}

/// Save the user's choice. Turning sync on makes Shodh's own calendar (once).
#[tauri::command]
pub async fn calendar_choose(
    app: AppHandle,
    mut choice: CalendarChoice,
    store: State<'_, Store>,
) -> Result<CalendarChoice, String> {
    let kept = self::choice(&app, &store).await?;
    // Shodh's calendar is the phone's to say, never the window's.
    choice.shodh_calendar = kept.shodh_calendar;
    if choice.enabled && choice.shodh_calendar.is_none() {
        let reply: Value = native(&app).call("calendarEnsureShodh", json!({})).await?;
        choice.shodh_calendar = reply["id"].as_i64();
    }
    if let Some(own) = choice.shodh_calendar {
        choice.chosen.retain(|id| *id != own);
    }
    choice.chosen.sort_unstable();
    choice.chosen.dedup();
    let storage = store.get(&app).await?;
    let saved = choice.clone();
    crate::link::blocking(move || storage.sealed(CHOICE_FILE).write(&saved)).await?;
    tracing::info!(
        "Calendar sync {}: {} calendars chosen, use with Shodh {}",
        if choice.enabled { "on" } else { "off" },
        choice.chosen.len(),
        choice.use_with_shodh
    );
    Ok(choice)
}

/// The single events of the calendars Shodh may read, between two instants.
#[tauri::command]
pub async fn calendar_events(
    app: AppHandle,
    from_ms: i64,
    to_ms: i64,
    store: State<'_, Store>,
) -> Result<Value, String> {
    let choice = choice(&app, &store).await?;
    let ids = choice.reachable();
    if ids.is_empty() {
        return Ok(json!([]));
    }
    let reply: Value = native(&app)
        .call(
            "calendarEvents",
            json!({ "calendarIds": ids, "fromMs": from_ms, "toMs": to_ms }),
        )
        .await?;
    Ok(reply["events"].clone())
}

/// One write to the phone's calendar.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CalendarWrite {
    Insert {
        calendar_id: i64,
        fields: Value,
    },
    Update {
        calendar_id: i64,
        event_id: i64,
        fields: Value,
    },
    Delete {
        calendar_id: i64,
        event_id: i64,
    },
}

/// Write to a calendar Shodh may touch; any other is refused.
#[tauri::command]
pub async fn calendar_write(
    app: AppHandle,
    write: CalendarWrite,
    store: State<'_, Store>,
) -> Result<Value, String> {
    let choice = choice(&app, &store).await?;
    let calendar = match &write {
        CalendarWrite::Insert { calendar_id, .. }
        | CalendarWrite::Update { calendar_id, .. }
        | CalendarWrite::Delete { calendar_id, .. } => *calendar_id,
    };
    if !choice.may_touch(calendar) {
        return Err(
            "Shodh may change only the calendars you chose for it, and its own.".to_string(),
        );
    }
    match write {
        CalendarWrite::Insert {
            calendar_id,
            fields,
        } => {
            native(&app)
                .call(
                    "calendarInsert",
                    json!({ "calendarId": calendar_id, "fields": fields }),
                )
                .await
        }
        CalendarWrite::Update {
            calendar_id,
            event_id,
            fields,
        } => {
            native(&app)
                .call(
                    "calendarUpdate",
                    json!({ "calendarId": calendar_id, "eventId": event_id, "fields": fields }),
                )
                .await
        }
        CalendarWrite::Delete {
            calendar_id,
            event_id,
        } => {
            native(&app)
                .call(
                    "calendarDelete",
                    json!({ "calendarId": calendar_id, "eventId": event_id }),
                )
                .await
        }
    }
}

fn window_file(name: &str) -> Result<String, String> {
    if WINDOW_FILES.contains(&name) {
        Ok(format!("{name}.sealed"))
    } else {
        Err("This is not one of Shodh's files.".to_string())
    }
}

/// One of the window's sealed files (`null` when it was never written).
#[tauri::command]
pub async fn sealed_read(
    app: AppHandle,
    name: String,
    store: State<'_, Store>,
) -> Result<Value, String> {
    let file = window_file(&name)?;
    let storage = store.get(&app).await?;
    crate::link::blocking(move || {
        storage
            .sealed(&file)
            .read::<Value>()
            .map(|v| v.unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn sealed_write(
    app: AppHandle,
    name: String,
    value: Value,
    store: State<'_, Store>,
) -> Result<(), String> {
    let file = window_file(&name)?;
    let storage = store.get(&app).await?;
    crate::link::blocking(move || storage.sealed(&file).write(&value)).await
}
