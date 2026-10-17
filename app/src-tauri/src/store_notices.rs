//! Notices about the app's own data files: a store that was read from its
//! backup, settings that had to be reset, a calendar that cannot be read.
//!
//! The stores report these where they happen, often without an app handle
//! (the agent's tools and the reminders loop use the stores directly) and
//! often before the window listens (startup). So each notice is kept for
//! the run, sent as [`STORE_NOTICE_EVENT`] once the app handle is known,
//! and listed by [`store_notices`], which the window reads once it has
//! started listening. A notice is reported once per store, kind and file.

use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Emitted with a [`StoreNotice`] when one is reported.
pub const STORE_NOTICE_EVENT: &str = "store-notice";

/// What happened to a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// The file was damaged and its backup was used instead.
    Restored,
    /// Settings: the file and its backup were damaged; defaults are used.
    Reset,
    /// The file and its backup are damaged; the store is not used.
    Unreadable,
}

/// One notice, in plain words for the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreNotice {
    /// Stable id: the same problem is shown once.
    pub id: String,
    /// `conversations`, `calendar` or `settings`.
    pub store: &'static str,
    pub kind: NoticeKind,
    pub title: String,
    pub message: String,
    /// The file the notice is about.
    pub path: String,
}

impl StoreNotice {
    pub fn new(
        store: &'static str,
        kind: NoticeKind,
        path: &std::path::Path,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let path = path.display().to_string();
        let kind_id = match kind {
            NoticeKind::Restored => "restored",
            NoticeKind::Reset => "reset",
            NoticeKind::Unreadable => "unreadable",
        };
        Self {
            id: format!("{store}:{kind_id}:{path}"),
            store,
            kind,
            title: title.into(),
            message: message.into(),
            path,
        }
    }
}

static NOTICES: Mutex<Vec<StoreNotice>> = Mutex::new(Vec::new());
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Send later notices to the window (called once at startup).
pub fn attach(app: &AppHandle) {
    if APP.set(app.clone()).is_err() {
        tracing::debug!("store notices were already attached");
    }
}

/// Add `notice` unless the same one was reported in this run. Returns
/// whether it was new.
fn remember(list: &mut Vec<StoreNotice>, notice: &StoreNotice) -> bool {
    if list.iter().any(|n| n.id == notice.id) {
        return false;
    }
    list.push(notice.clone());
    true
}

/// Report `notice`: logged, kept for [`store_notices`] and sent to the
/// window, once per run.
pub fn report(notice: StoreNotice) {
    let new = remember(
        &mut NOTICES.lock().unwrap_or_else(|e| e.into_inner()),
        &notice,
    );
    if !new {
        return;
    }
    tracing::warn!(store = notice.store, path = %notice.path, "{}: {}", notice.title, notice.message);
    if let Some(app) = APP.get() {
        if let Err(e) = app.emit(STORE_NOTICE_EVENT, &notice) {
            tracing::warn!(error = %e, "could not emit {STORE_NOTICE_EVENT}");
        }
        // The message names the file and its error: the window shows it; the
        // desktop notification only says what happened. Delivered later, off
        // the reporting store's lock.
        crate::notifications::problem(
            app,
            notice.id.clone(),
            notice.title.clone(),
            "Open Shodh to see what happened and what to do.",
        );
    }
}

/// The notices reported in this run, oldest first.
#[tauri::command]
pub fn store_notices() -> Vec<StoreNotice> {
    NOTICES.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_notice_is_kept_once_per_store_kind_and_file() {
        let mut list = Vec::new();
        let restored = StoreNotice::new(
            "calendar",
            NoticeKind::Restored,
            Path::new("C:/data/calendar_data.json"),
            "t",
            "m",
        );
        assert!(remember(&mut list, &restored));
        assert!(!remember(&mut list, &restored));
        let other_file = StoreNotice::new(
            "calendar",
            NoticeKind::Restored,
            Path::new("C:/other/calendar_data.json"),
            "t",
            "m",
        );
        assert!(remember(&mut list, &other_file));
        let unreadable = StoreNotice::new(
            "calendar",
            NoticeKind::Unreadable,
            Path::new("C:/data/calendar_data.json"),
            "t",
            "m",
        );
        assert!(remember(&mut list, &unreadable));
        assert_eq!(list.len(), 3);
        let json = serde_json::to_value(&unreadable).unwrap();
        assert_eq!(json["kind"], "unreadable");
        assert_eq!(json["store"], "calendar");
        assert_eq!(json["id"], "calendar:unreadable:C:/data/calendar_data.json");
    }
}
