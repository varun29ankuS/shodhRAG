//! The What's new feed in `shodh.db` (`feed_item`, migration 20).
//!
//! - **One row per file or paper.** `(kind, source_ref)` is unique and rows are only
//!   inserted when absent, so a file reported twice (a sync retried, two syncs racing) is
//!   listed once.
//! - **Posted once.** The job that found a row posts one Inbox item for the rows it has not
//!   told of yet ([`FeedStore::unposted`]), then marks them with its run
//!   ([`FeedStore::mark_posted`]).
//! - **Seen and dismissed are the user's.** Dismissed rows stay until they are pruned, so
//!   what the user set aside is not found again.
//! - **Kept [`FEED_RETENTION_DAYS`] days**, then pruned, dismissed or not.

use std::path::Path;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, params_from_iter, Row};
use serde::Serialize;
use serde_json::Value;

use crate::audit::{open_shared_connection, AuditError, AuditKey, SharedConnection};
use crate::engine::lease::timestamp;

/// Rows older than this many days are pruned.
pub const FEED_RETENTION_DAYS: i64 = 90;

/// Error of the feed's store.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    /// `shodh.db` could not be opened or migrated.
    #[error("The app database could not be opened: {0}")]
    Open(#[from] AuditError),
    #[error("What's new database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type FeedResult<T> = Result<T, FeedError>;

/// What a row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    /// A file new in a watched folder.
    File,
    /// A paper a paper alert found.
    Paper,
}

impl FeedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Paper => "paper",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "file" => Some(Self::File),
            "paper" => Some(Self::Paper),
            _ => None,
        }
    }
}

/// A row to insert.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFeedItem {
    pub kind: FeedKind,
    /// A file's index identity; a paper's strongest identifier.
    pub source_ref: String,
    /// The engine job that found it.
    pub job_id: String,
    pub title: String,
    pub detail: Option<String>,
    pub link: Value,
    pub data: Value,
}

/// A row of the feed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedItem {
    pub id: String,
    pub kind: FeedKind,
    pub source_ref: String,
    pub job_id: String,
    pub title: String,
    pub detail: Option<String>,
    /// When it was found (UTC, RFC 3339).
    pub found_at: String,
    /// The run whose Inbox item told of it.
    pub posted_run_id: Option<String>,
    pub seen_at: Option<String>,
    pub dismissed_at: Option<String>,
    pub link: Value,
    pub data: Value,
}

const COLS: &str = "id, kind, source_ref, job_id, title, detail, found_at, posted_run_id, \
     seen_at, dismissed_at, link_json, data_json";

fn bad_column(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, message.into())
}

fn json_col(index: usize, text: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&text).map_err(|e| bad_column(index, e.to_string()))
}

fn item_from(r: &Row<'_>) -> rusqlite::Result<FeedItem> {
    let kind: String = r.get(1)?;
    Ok(FeedItem {
        id: r.get(0)?,
        kind: FeedKind::parse(&kind)
            .ok_or_else(|| bad_column(1, format!("unknown kind {kind}")))?,
        source_ref: r.get(2)?,
        job_id: r.get(3)?,
        title: r.get(4)?,
        detail: r.get(5)?,
        found_at: r.get(6)?,
        posted_run_id: r.get(7)?,
        seen_at: r.get(8)?,
        dismissed_at: r.get(9)?,
        link: json_col(10, r.get(10)?)?,
        data: json_col(11, r.get(11)?)?,
    })
}

/// `?, ?, …` for `n` values.
fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// The What's new feed in `shodh.db`.
pub struct FeedStore {
    conn: SharedConnection,
}

impl std::fmt::Debug for FeedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedStore").finish_non_exhaustive()
    }
}

impl FeedStore {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> FeedResult<Self> {
        Ok(Self {
            conn: open_shared_connection(path, key)?,
        })
    }

    /// Inserts the rows not in the feed yet, found at `now`; returns those inserted, in
    /// the order given.
    pub fn insert_if_absent(
        &self,
        items: &[NewFeedItem],
        now: DateTime<Utc>,
    ) -> FeedResult<Vec<FeedItem>> {
        let found_at = timestamp(now);
        self.conn.write(|tx| -> FeedResult<Vec<FeedItem>> {
            let mut inserted = Vec::new();
            for item in items {
                let id = uuid::Uuid::new_v4().to_string();
                let changed = tx.execute(
                    "INSERT OR IGNORE INTO feed_item (id, kind, source_ref, job_id, title,
                         detail, found_at, link_json, data_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        id,
                        item.kind.as_str(),
                        item.source_ref,
                        item.job_id,
                        item.title,
                        item.detail,
                        found_at,
                        item.link.to_string(),
                        item.data.to_string(),
                    ],
                )?;
                if changed > 0 {
                    inserted.push(FeedItem {
                        id,
                        kind: item.kind,
                        source_ref: item.source_ref.clone(),
                        job_id: item.job_id.clone(),
                        title: item.title.clone(),
                        detail: item.detail.clone(),
                        found_at: found_at.clone(),
                        posted_run_id: None,
                        seen_at: None,
                        dismissed_at: None,
                        link: item.link.clone(),
                        data: item.data.clone(),
                    });
                }
            }
            Ok(inserted)
        })
    }

    /// The rows of `job_id` no Inbox item has told of yet, oldest first.
    pub fn unposted(&self, job_id: &str) -> FeedResult<Vec<FeedItem>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM feed_item WHERE job_id = ?1 AND posted_run_id IS NULL
              ORDER BY found_at, rowid"
        ))?;
        let rows = stmt.query_map(params![job_id], item_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Marks `ids` as told of by `run_id`.
    pub fn mark_posted(&self, ids: &[String], run_id: &str) -> FeedResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.conn.write(|tx| -> FeedResult<()> {
            let sql = format!(
                "UPDATE feed_item SET posted_run_id = ? WHERE id IN ({})",
                placeholders(ids.len())
            );
            let values = std::iter::once(run_id.to_string()).chain(ids.iter().cloned());
            tx.execute(&sql, params_from_iter(values))?;
            Ok(())
        })
    }

    /// The rows of `job_id` found at or after `since` and not dismissed, oldest first.
    pub fn of_job_since(&self, job_id: &str, since: DateTime<Utc>) -> FeedResult<Vec<FeedItem>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM feed_item
              WHERE job_id = ?1 AND found_at >= ?2 AND dismissed_at IS NULL
              ORDER BY found_at, rowid"
        ))?;
        let rows = stmt.query_map(params![job_id, timestamp(since)], item_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// At most `limit` rows not dismissed, newest first.
    pub fn list(&self, limit: usize) -> FeedResult<Vec<FeedItem>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM feed_item WHERE dismissed_at IS NULL
              ORDER BY found_at DESC, rowid DESC LIMIT ?1"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![limit], item_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Marks `ids` seen at `now` (those not seen before); returns how many.
    pub fn mark_seen(&self, ids: &[String], now: DateTime<Utc>) -> FeedResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.conn.write(|tx| -> FeedResult<usize> {
            let sql = format!(
                "UPDATE feed_item SET seen_at = ? WHERE seen_at IS NULL AND id IN ({})",
                placeholders(ids.len())
            );
            let values = std::iter::once(timestamp(now)).chain(ids.iter().cloned());
            Ok(tx.execute(&sql, params_from_iter(values))?)
        })
    }

    /// Dismisses the row `id` at `now`, or brings it back (`dismissed` false); returns
    /// whether there is such a row.
    pub fn set_dismissed(&self, id: &str, dismissed: bool, now: DateTime<Utc>) -> FeedResult<bool> {
        let at = dismissed.then(|| timestamp(now));
        self.conn.write(|tx| -> FeedResult<bool> {
            Ok(tx.execute(
                "UPDATE feed_item SET dismissed_at = ?2 WHERE id = ?1",
                params![id, at],
            )? > 0)
        })
    }

    /// Deletes the rows of `job_id` (its job is deleted); returns how many.
    pub fn delete_job(&self, job_id: &str) -> FeedResult<usize> {
        self.conn.write(|tx| -> FeedResult<usize> {
            Ok(tx.execute("DELETE FROM feed_item WHERE job_id = ?1", params![job_id])?)
        })
    }

    /// Deletes the rows found more than [`FEED_RETENTION_DAYS`] days before `now`;
    /// returns how many.
    pub fn prune(&self, now: DateTime<Utc>) -> FeedResult<usize> {
        let cutoff = timestamp(now - TimeDelta::days(FEED_RETENTION_DAYS));
        self.conn.write(|tx| -> FeedResult<usize> {
            Ok(tx.execute("DELETE FROM feed_item WHERE found_at < ?1", params![cutoff])?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn store(dir: &tempfile::TempDir) -> FeedStore {
        FeedStore::open(&dir.path().join("shodh.db"), None).unwrap()
    }

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap()
    }

    fn file(job: &str, path: &str) -> NewFeedItem {
        NewFeedItem {
            kind: FeedKind::File,
            source_ref: path.to_string(),
            job_id: job.to_string(),
            title: path.rsplit('/').next().unwrap_or(path).to_string(),
            detail: Some("Research Papers".into()),
            link: serde_json::json!({ "view": "library" }),
            data: serde_json::json!({ "sourceId": "s1" }),
        }
    }

    #[test]
    fn a_second_item_with_the_same_ref_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let feed = store(&dir);
        let first = feed
            .insert_if_absent(&[file("j1", "c:/r/a.pdf")], at(23, 9))
            .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].title, "a.pdf");
        let again = feed
            .insert_if_absent(
                &[file("j1", "c:/r/a.pdf"), file("j1", "c:/r/b.pdf")],
                at(23, 10),
            )
            .unwrap();
        assert_eq!(
            again
                .iter()
                .map(|i| i.source_ref.as_str())
                .collect::<Vec<_>>(),
            ["c:/r/b.pdf"]
        );
        assert_eq!(feed.list(10).unwrap().len(), 2);
        // The row inserted is the row stored.
        let stored = feed.list(10).unwrap();
        assert!(stored.contains(&again[0]));
    }

    #[test]
    fn unposted_rows_are_posted_once() {
        let dir = tempfile::tempdir().unwrap();
        let feed = store(&dir);
        feed.insert_if_absent(
            &[file("j1", "c:/r/a.pdf"), file("j2", "c:/s/x.pdf")],
            at(23, 9),
        )
        .unwrap();
        let unposted = feed.unposted("j1").unwrap();
        assert_eq!(unposted.len(), 1);
        let ids: Vec<String> = unposted.iter().map(|i| i.id.clone()).collect();
        feed.mark_posted(&ids, "run-1").unwrap();
        assert!(feed.unposted("j1").unwrap().is_empty());
        assert_eq!(
            feed.unposted("j2").unwrap().len(),
            1,
            "another job's rows stay"
        );
        let posted = feed.list(10).unwrap();
        let a = posted
            .iter()
            .find(|i| i.source_ref == "c:/r/a.pdf")
            .unwrap();
        assert_eq!(a.posted_run_id.as_deref(), Some("run-1"));
    }

    #[test]
    fn rows_of_a_job_since_a_time_leave_out_dismissed_ones() {
        let dir = tempfile::tempdir().unwrap();
        let feed = store(&dir);
        feed.insert_if_absent(&[file("j1", "c:/r/old.pdf")], at(22, 9))
            .unwrap();
        let today = feed
            .insert_if_absent(
                &[file("j1", "c:/r/a.pdf"), file("j1", "c:/r/b.pdf")],
                at(23, 9),
            )
            .unwrap();
        assert!(feed.set_dismissed(&today[1].id, true, at(23, 10)).unwrap());
        let since = feed.of_job_since("j1", at(23, 0)).unwrap();
        assert_eq!(
            since
                .iter()
                .map(|i| i.source_ref.as_str())
                .collect::<Vec<_>>(),
            ["c:/r/a.pdf"]
        );
    }

    #[test]
    fn dismissed_rows_stay_as_tombstones_until_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let feed = store(&dir);
        let rows = feed
            .insert_if_absent(&[file("j1", "c:/r/a.pdf")], at(1, 9))
            .unwrap();
        assert!(feed.set_dismissed(&rows[0].id, true, at(1, 10)).unwrap());
        assert!(feed.list(10).unwrap().is_empty(), "the list hides it");
        // Still there: found again, it is not inserted again.
        assert!(feed
            .insert_if_absent(&[file("j1", "c:/r/a.pdf")], at(2, 9))
            .unwrap()
            .is_empty());
        // Undo brings it back.
        assert!(feed.set_dismissed(&rows[0].id, false, at(1, 11)).unwrap());
        assert_eq!(feed.list(10).unwrap().len(), 1);
        assert!(!feed.set_dismissed("missing", true, at(1, 11)).unwrap());
        assert_eq!(
            feed.prune(at(1, 9) + TimeDelta::days(FEED_RETENTION_DAYS - 1))
                .unwrap(),
            0
        );
        assert_eq!(
            feed.prune(at(1, 9) + TimeDelta::days(FEED_RETENTION_DAYS + 1))
                .unwrap(),
            1
        );
        assert!(feed.list(10).unwrap().is_empty());
    }

    #[test]
    fn seen_is_set_once() {
        let dir = tempfile::tempdir().unwrap();
        let feed = store(&dir);
        let rows = feed
            .insert_if_absent(&[file("j1", "c:/r/a.pdf")], at(23, 9))
            .unwrap();
        let ids = vec![rows[0].id.clone()];
        assert_eq!(feed.mark_seen(&ids, at(23, 10)).unwrap(), 1);
        assert_eq!(feed.mark_seen(&ids, at(23, 11)).unwrap(), 0);
        assert_eq!(
            feed.list(1).unwrap()[0].seen_at.as_deref(),
            Some(timestamp(at(23, 10)).as_str())
        );
    }

    #[test]
    fn deleting_a_job_deletes_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let feed = store(&dir);
        feed.insert_if_absent(
            &[
                file("j1", "c:/r/a.pdf"),
                file("j1", "c:/r/b.pdf"),
                file("j2", "c:/s/x.pdf"),
            ],
            at(23, 9),
        )
        .unwrap();
        assert_eq!(feed.delete_job("j1").unwrap(), 2);
        assert_eq!(feed.list(10).unwrap().len(), 1);
    }

    #[test]
    fn the_list_is_newest_first_and_limited() {
        let dir = tempfile::tempdir().unwrap();
        let feed = store(&dir);
        feed.insert_if_absent(&[file("j1", "c:/r/a.pdf")], at(21, 9))
            .unwrap();
        feed.insert_if_absent(&[file("j1", "c:/r/b.pdf")], at(22, 9))
            .unwrap();
        feed.insert_if_absent(&[file("j1", "c:/r/c.pdf")], at(23, 9))
            .unwrap();
        let list = feed.list(2).unwrap();
        assert_eq!(
            list.iter()
                .map(|i| i.source_ref.as_str())
                .collect::<Vec<_>>(),
            ["c:/r/c.pdf", "c:/r/b.pdf"]
        );
    }
}
