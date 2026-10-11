//! The Inbox: one list of what waits on the user (approvals, memory suggestions,
//! reminders) and of background work that finished or failed (indexing, table
//! refinement, the citation graph, exports, skill and MCP installs).
//!
//! Event-type items (approvals and background work) are kept in `shodh.db`
//! ([`InboxStore`]). Items that have their own store (pending memory
//! suggestions) are not copied: the app derives them when the Inbox is read and
//! [`aggregate`] merges both into one ordered list.
//!
//! Done and failed items are pruned [`DONE_RETENTION_DAYS`] after they last
//! changed. Approvals belong to a running assistant session, which does not
//! survive a restart: [`InboxStore::recover`] drops them on launch and marks
//! work that was still running as stopped.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::audit::{open_shared_connection, AuditError, AuditKey};

/// Days a done or failed item stays in the Inbox after it last changed.
pub const DONE_RETENTION_DAYS: i64 = 30;

/// Longest title and detail kept (characters).
const MAX_TITLE_CHARS: usize = 200;
const MAX_DETAIL_CHARS: usize = 2_000;
const MAX_ID_CHARS: usize = 512;

/// Error of the Inbox store.
#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    /// `shodh.db` could not be opened or migrated.
    #[error("The app database could not be opened: {0}")]
    Open(#[from] AuditError),
    #[error("Inbox database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Invalid inbox item: {0}")]
    Invalid(String),
}

pub type InboxResult<T> = Result<T, InboxError>;

/// Where an item stands. The UI colours it: working blue, needs you amber,
/// done green, failed red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Working,
    NeedsYou,
    Done,
    Failed,
}

impl InboxStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsYou => "needs_you",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "working" => Self::Working,
            "needs_you" => Self::NeedsYou,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    /// Order in the list: what needs the user first, then what runs, then
    /// failures, then what finished.
    fn rank(self) -> u8 {
        match self {
            Self::NeedsYou => 0,
            Self::Working => 1,
            Self::Failed => 2,
            Self::Done => 3,
        }
    }

    fn is_finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// What an item is about; decides its actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    /// A tool call, edit or command waiting for Approve / Deny.
    Approval,
    /// A memory suggestion waiting for Accept / Dismiss (derived, not stored).
    Memory,
    /// A task reminder that rang (or was missed while Shodh was closed).
    Reminder,
    Indexing,
    Tables,
    CitationGraph,
    Export,
    /// A skill or MCP server install.
    Install,
}

impl InboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Memory => "memory",
            Self::Reminder => "reminder",
            Self::Indexing => "indexing",
            Self::Tables => "tables",
            Self::CitationGraph => "citation_graph",
            Self::Export => "export",
            Self::Install => "install",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "approval" => Self::Approval,
            "memory" => Self::Memory,
            "reminder" => Self::Reminder,
            "indexing" => Self::Indexing,
            "tables" => Self::Tables,
            "citation_graph" => Self::CitationGraph,
            "export" => Self::Export,
            "install" => Self::Install,
            _ => return None,
        })
    }
}

/// What Open shows: a view (`ask`, `library`, `tasks`, `settings`, ...) and, optionally,
/// what to show in it, as the app's navigation target (`{"kind": "conversation",
/// "conversationId": ...}`, `{"kind": "document", "path": ...}`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxLink {
    pub view: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
}

impl InboxLink {
    pub fn new(view: &str, target: Option<serde_json::Value>) -> Self {
        Self {
            view: view.to_string(),
            target,
        }
    }
}

/// An item to add or update (by `id`).
#[derive(Debug, Clone, PartialEq)]
pub struct NewInboxItem {
    /// Stable per subject (`approval:<session>:<step>`, `indexing:<folder>`), so a
    /// later state of the same work replaces the earlier one.
    pub id: String,
    pub kind: InboxKind,
    pub status: InboxStatus,
    pub title: String,
    pub detail: Option<String>,
    pub link: Option<InboxLink>,
    /// What the item's actions need.
    pub data: serde_json::Value,
}

/// One Inbox entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub id: String,
    pub kind: InboxKind,
    pub status: InboxStatus,
    pub title: String,
    pub detail: Option<String>,
    pub link: Option<InboxLink>,
    pub data: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

fn format_ts(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn clip(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &trimmed[..cut]),
        None => trimmed.to_string(),
    }
}

fn corrupt(what: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown {what} {value:?}"),
        )),
    )
}

fn read_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<InboxItem> {
    let kind: String = row.get(1)?;
    let status: String = row.get(2)?;
    let link: Option<String> = row.get(5)?;
    let data: String = row.get(6)?;
    Ok(InboxItem {
        id: row.get(0)?,
        kind: InboxKind::parse(&kind).ok_or_else(|| corrupt("kind", &kind))?,
        status: InboxStatus::parse(&status).ok_or_else(|| corrupt("status", &status))?,
        title: row.get(3)?,
        detail: row.get(4)?,
        // A link or data the app can no longer read is dropped, not fatal.
        link: link.and_then(|text| serde_json::from_str(&text).ok()),
        data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const COLUMNS: &str =
    "id, kind, status, title, detail, link_json, data_json, created_at, updated_at";

/// Inbox items in `shodh.db`.
pub struct InboxStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for InboxStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxStore").finish_non_exhaustive()
    }
}

impl InboxStore {
    /// Opens `shodh.db` at `path` with the audit database key (if the database is
    /// encrypted), applying any pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> InboxResult<Self> {
        let conn = open_shared_connection(path, key)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds `item`, or updates the item with its id (keeping when it was created).
    pub fn put(&self, item: &NewInboxItem, now: DateTime<Utc>) -> InboxResult<InboxItem> {
        let id = item.id.trim();
        if id.is_empty() || id.chars().count() > MAX_ID_CHARS {
            return Err(InboxError::Invalid("the id is empty or too long".into()));
        }
        let title = clip(&item.title, MAX_TITLE_CHARS);
        if title.is_empty() {
            return Err(InboxError::Invalid("the title is empty".into()));
        }
        let detail = item
            .detail
            .as_deref()
            .map(|d| clip(d, MAX_DETAIL_CHARS))
            .filter(|d| !d.is_empty());
        let link = item
            .link
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| InboxError::Invalid(e.to_string()))?;
        let data =
            serde_json::to_string(&item.data).map_err(|e| InboxError::Invalid(e.to_string()))?;
        let at = format_ts(now);
        let conn = self.lock();
        conn.execute(
            "INSERT INTO inbox_items(id, kind, status, title, detail, link_json, data_json,
                                     created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, status = excluded.status,
                 title = excluded.title, detail = excluded.detail,
                 link_json = excluded.link_json, data_json = excluded.data_json,
                 updated_at = excluded.updated_at",
            params![
                id,
                item.kind.as_str(),
                item.status.as_str(),
                title,
                detail,
                link,
                data,
                at
            ],
        )?;
        let stored = conn.query_row(
            &format!("SELECT {COLUMNS} FROM inbox_items WHERE id = ?1"),
            params![id],
            read_item,
        )?;
        Ok(stored)
    }

    /// Every stored item, in Inbox order.
    pub fn list(&self) -> InboxResult<Vec<InboxItem>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM inbox_items"))?;
        let items = stmt
            .query_map([], read_item)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(aggregate(items, Vec::new()))
    }

    /// Removes the item `id`. False when there was none.
    pub fn remove(&self, id: &str) -> InboxResult<bool> {
        Ok(self
            .lock()
            .execute("DELETE FROM inbox_items WHERE id = ?1", params![id])?
            > 0)
    }

    /// Removes the items of `kind` whose id starts with `prefix` (the approvals of
    /// one session). Returns how many.
    pub fn remove_prefixed(&self, kind: InboxKind, prefix: &str) -> InboxResult<usize> {
        let pattern = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        Ok(self.lock().execute(
            "DELETE FROM inbox_items WHERE kind = ?1 AND id LIKE ?2 ESCAPE '\\'",
            params![kind.as_str(), pattern],
        )?)
    }

    /// Removes done and failed items that last changed more than
    /// [`DONE_RETENTION_DAYS`] before `now`. Returns how many.
    pub fn purge(&self, now: DateTime<Utc>) -> InboxResult<usize> {
        let cutoff = format_ts(now - Duration::days(DONE_RETENTION_DAYS));
        Ok(self.lock().execute(
            "DELETE FROM inbox_items WHERE status IN ('done', 'failed') AND updated_at < ?1",
            params![cutoff],
        )?)
    }

    /// After a launch: approvals of the previous run cannot be answered any more
    /// (their sessions ended with it) and are removed; work that was still running
    /// is marked failed. Then prunes old items. Returns how many items changed.
    pub fn recover(&self, now: DateTime<Utc>) -> InboxResult<usize> {
        let at = format_ts(now);
        let changed = {
            let conn = self.lock();
            let removed = conn.execute("DELETE FROM inbox_items WHERE kind = 'approval'", [])?;
            let stopped = conn.execute(
                "UPDATE inbox_items SET status = 'failed',
                     detail = 'Stopped when Shodh closed before it finished.', updated_at = ?1
                 WHERE status = 'working'",
                params![at],
            )?;
            removed + stopped
        };
        Ok(changed + self.purge(now)?)
    }
}

/// One Inbox list from the stored items and the derived (`live`) ones: an id
/// in both is the live item; needs-you first, then working, failed and done,
/// each newest first.
pub fn aggregate(stored: Vec<InboxItem>, live: Vec<InboxItem>) -> Vec<InboxItem> {
    let mut by_id: BTreeMap<String, InboxItem> = BTreeMap::new();
    for item in stored.into_iter().chain(live) {
        by_id.insert(item.id.clone(), item);
    }
    let mut items: Vec<InboxItem> = by_id.into_values().collect();
    items.sort_by(|a, b| {
        a.status
            .rank()
            .cmp(&b.status.rank())
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    items
}

/// Whether an item is finished (it may be pruned, and Dismiss removes it).
pub fn is_finished(item: &InboxItem) -> bool {
    item.status.is_finished()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, day, hour, 0, 0).unwrap()
    }

    fn new(id: &str, kind: InboxKind, status: InboxStatus) -> NewInboxItem {
        NewInboxItem {
            id: id.into(),
            kind,
            status,
            title: format!("Item {id}"),
            detail: None,
            link: None,
            data: json!({}),
        }
    }

    fn store(dir: &tempfile::TempDir) -> InboxStore {
        InboxStore::open(&dir.path().join("shodh.db"), None).expect("open")
    }

    fn ids(items: &[InboxItem]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn items_are_kept_and_updated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = store(&dir);
        let mut item = new(
            "indexing:C:/papers",
            InboxKind::Indexing,
            InboxStatus::Working,
        );
        item.link = Some(InboxLink::new(
            "library",
            Some(json!({ "kind": "document", "path": "C:/papers" })),
        ));
        item.data = json!({ "folder": "C:/papers" });
        inbox.put(&item, at(1, 9)).unwrap();
        item.status = InboxStatus::Done;
        item.detail = Some("  12 files indexed  ".into());
        let updated = inbox.put(&item, at(1, 10)).unwrap();
        assert_eq!(updated.status, InboxStatus::Done);
        assert_eq!(updated.detail.as_deref(), Some("12 files indexed"));
        assert_eq!(updated.created_at, "2026-09-01T09:00:00.000Z");
        assert_eq!(updated.updated_at, "2026-09-01T10:00:00.000Z");

        // Persisted: a second store over the same database reads the same item.
        drop(inbox);
        let reopened = store(&dir);
        assert_eq!(reopened.list().unwrap(), vec![updated]);
        assert!(reopened.remove("indexing:C:/papers").unwrap());
        assert!(!reopened.remove("indexing:C:/papers").unwrap());
        assert!(reopened.list().unwrap().is_empty());
    }

    #[test]
    fn invalid_items_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = store(&dir);
        let mut item = new(" ", InboxKind::Export, InboxStatus::Done);
        assert!(matches!(
            inbox.put(&item, at(1, 9)),
            Err(InboxError::Invalid(_))
        ));
        item.id = "export:1".into();
        item.title = "   ".into();
        assert!(matches!(
            inbox.put(&item, at(1, 9)),
            Err(InboxError::Invalid(_))
        ));
        item.title = "x".repeat(MAX_TITLE_CHARS + 50);
        let stored = inbox.put(&item, at(1, 9)).unwrap();
        assert_eq!(stored.title.chars().count(), MAX_TITLE_CHARS + 1);
    }

    #[test]
    fn finished_items_are_pruned_after_thirty_days_and_waiting_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = store(&dir);
        inbox
            .put(
                &new("old-done", InboxKind::Export, InboxStatus::Done),
                at(1, 9),
            )
            .unwrap();
        inbox
            .put(
                &new("old-failed", InboxKind::Tables, InboxStatus::Failed),
                at(1, 9),
            )
            .unwrap();
        inbox
            .put(
                &new("old-reminder", InboxKind::Reminder, InboxStatus::NeedsYou),
                at(1, 9),
            )
            .unwrap();
        inbox
            .put(
                &new("recent-done", InboxKind::Indexing, InboxStatus::Done),
                at(2, 9),
            )
            .unwrap();
        // Exactly 30 days after the first two, but a moment before the third.
        let now = at(1, 9) + Duration::days(DONE_RETENTION_DAYS) + Duration::seconds(1);
        assert_eq!(inbox.purge(now).unwrap(), 2);
        assert_eq!(ids(&inbox.list().unwrap()), ["old-reminder", "recent-done"]);
        // A day later the third one goes too; the reminder still waits.
        assert_eq!(inbox.purge(now + Duration::days(1)).unwrap(), 1);
        assert_eq!(ids(&inbox.list().unwrap()), ["old-reminder"]);
    }

    #[test]
    fn a_relaunch_drops_approvals_and_stops_running_work() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = store(&dir);
        inbox
            .put(
                &new("approval:s1:a", InboxKind::Approval, InboxStatus::NeedsYou),
                at(3, 9),
            )
            .unwrap();
        inbox
            .put(
                &new("indexing:x", InboxKind::Indexing, InboxStatus::Working),
                at(3, 9),
            )
            .unwrap();
        inbox
            .put(
                &new("reminder:t1", InboxKind::Reminder, InboxStatus::NeedsYou),
                at(3, 9),
            )
            .unwrap();
        assert_eq!(inbox.recover(at(3, 10)).unwrap(), 2);
        let items = inbox.list().unwrap();
        assert_eq!(ids(&items), ["reminder:t1", "indexing:x"]);
        assert_eq!(items[1].status, InboxStatus::Failed);
        assert!(items[1].detail.as_deref().unwrap().contains("Stopped"));
    }

    #[test]
    fn the_approvals_of_one_session_are_removed_together() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = store(&dir);
        for id in ["approval:s_1:a", "approval:s_1:b", "approval:s21:c"] {
            inbox
                .put(
                    &new(id, InboxKind::Approval, InboxStatus::NeedsYou),
                    at(3, 9),
                )
                .unwrap();
        }
        // `_` is literal, not a LIKE wildcard: s21 is another session.
        assert_eq!(
            inbox
                .remove_prefixed(InboxKind::Approval, "approval:s_1:")
                .unwrap(),
            2
        );
        assert_eq!(ids(&inbox.list().unwrap()), ["approval:s21:c"]);
    }

    #[test]
    fn stored_and_derived_items_merge_into_one_ordered_list() {
        let item = |id: &str, status: InboxStatus, updated: &str| InboxItem {
            id: id.into(),
            kind: InboxKind::Indexing,
            status,
            title: id.into(),
            detail: None,
            link: None,
            data: json!(null),
            created_at: updated.into(),
            updated_at: updated.into(),
        };
        let stored = vec![
            item("done-new", InboxStatus::Done, "2026-09-03T00:00:00.000Z"),
            item("done-old", InboxStatus::Done, "2026-09-01T00:00:00.000Z"),
            item("failed", InboxStatus::Failed, "2026-09-01T00:00:00.000Z"),
            item("running", InboxStatus::Working, "2026-09-01T00:00:00.000Z"),
            item("memory:m1", InboxStatus::Done, "2026-09-01T00:00:00.000Z"),
        ];
        let live = vec![
            item(
                "memory:m1",
                InboxStatus::NeedsYou,
                "2026-09-02T00:00:00.000Z",
            ),
            item(
                "memory:m2",
                InboxStatus::NeedsYou,
                "2026-09-04T00:00:00.000Z",
            ),
        ];
        let merged = aggregate(stored, live);
        assert_eq!(
            ids(&merged),
            [
                "memory:m2",
                "memory:m1",
                "running",
                "failed",
                "done-new",
                "done-old"
            ]
        );
        assert_eq!(merged[1].status, InboxStatus::NeedsYou);
        assert!(is_finished(&merged[4]) && !is_finished(&merged[0]));
    }

    #[test]
    fn items_serialize_for_the_app() {
        let value = serde_json::to_value(InboxItem {
            id: "a".into(),
            kind: InboxKind::CitationGraph,
            status: InboxStatus::NeedsYou,
            title: "t".into(),
            detail: None,
            link: Some(InboxLink::new("research", None)),
            data: json!({}),
            created_at: "c".into(),
            updated_at: "u".into(),
        })
        .unwrap();
        assert_eq!(value["kind"], "citation_graph");
        assert_eq!(value["status"], "needs_you");
        assert_eq!(value["link"], json!({ "view": "research" }));
        assert_eq!(value["updatedAt"], "u");
    }
}
