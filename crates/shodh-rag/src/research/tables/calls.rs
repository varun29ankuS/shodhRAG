//! The model calls evidence tables made per UTC day, and the clock. The daily cap
//! (Settings, [`DEFAULT_DAILY_CAP`] by default) is checked against this count. Only table
//! fills count here: chat and agents do not, so a table fill can never silently spend the
//! quota a conversation needs, and the fill dialog says "Tables have made N of 30 calls
//! today".
//!
//! The day is the UTC date, because that is the window of the provider's own daily quota
//! (OpenRouter's free quota resets at 00:00 UTC): a cap counted per local date would let a
//! user east or west of UTC spend two caps inside one provider day. The interface shows
//! when the day resets in local time ("resets at 5:30 am").
//!
//! A call is counted before it is made ([`CallCounter::try_record`] reserves it), so a
//! crash between the two over-counts by at most one call and never under-counts: on a
//! free quota of about 50 calls a day an over-count pauses a job one call early, while an
//! under-count could run into the provider's own daily limit.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::budget::MAX_CALLS_PER_PAPER;
use crate::audit::{open_shared_connection, AuditKey, SharedConnection};
use crate::research::{ResearchError, ResearchResult};

/// The daily cap of table calls when the user has not set one.
pub const DEFAULT_DAILY_CAP: usize = 30;
/// The most calls one paper can cost: two planned calls and the one retry. Under a
/// smaller cap a two-call paper could never start.
pub const MAX_CALLS_PER_PAPER_WITH_RETRY: usize = MAX_CALLS_PER_PAPER + 1;
/// The smallest cap a runner accepts.
pub const MIN_DAILY_CAP: usize = MAX_CALLS_PER_PAPER_WITH_RETRY;
/// The largest cap Settings accepts: OpenRouter's daily request limit for a key with
/// credits. A larger number could not be spent, and a typo of one more digit should not
/// lift the guard that keeps a table fill from emptying the day's quota.
pub const MAX_DAILY_CAP: usize = 1_000;

/// The day's cap of table calls, shared by every runner and gate that holds a clone. It is
/// read at every check (each paper and each call), so a change in Settings applies from
/// the next check, also to a run in flight.
#[derive(Debug, Clone)]
pub struct DailyCap(Arc<AtomicUsize>);

impl DailyCap {
    /// A cap of `calls`, within [`MIN_DAILY_CAP`]..=[`MAX_DAILY_CAP`].
    pub fn new(calls: usize) -> ResearchResult<Self> {
        check_cap(calls)?;
        Ok(Self(Arc::new(AtomicUsize::new(calls))))
    }

    /// The cap now.
    pub fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    /// Changes the cap for every holder; a value out of range is refused and changes
    /// nothing.
    pub fn set(&self, calls: usize) -> ResearchResult<()> {
        check_cap(calls)?;
        self.0.store(calls, Ordering::SeqCst);
        Ok(())
    }
}

impl Default for DailyCap {
    /// [`DEFAULT_DAILY_CAP`], which is within the bounds.
    fn default() -> Self {
        Self(Arc::new(AtomicUsize::new(DEFAULT_DAILY_CAP)))
    }
}

/// Whether `calls` is a cap Settings may hold.
pub fn check_cap(calls: usize) -> ResearchResult<()> {
    if (MIN_DAILY_CAP..=MAX_DAILY_CAP).contains(&calls) {
        Ok(())
    } else {
        Err(ResearchError::Invalid(format!(
            "The daily cap of table calls is between {MIN_DAILY_CAP} and {MAX_DAILY_CAP}."
        )))
    }
}

/// The time. Injected so that days, resets and waits are deterministic in tests.
pub trait Clock: Send + Sync {
    /// Now, in UTC.
    fn now(&self) -> DateTime<Utc>;
}

/// The computer's clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The UTC date at `at`: the day the counter counts on.
pub fn utc_day(at: DateTime<Utc>) -> NaiveDate {
    at.date_naive()
}

/// The next 00:00 UTC after `at`: when the counter's day, and the provider's daily quota,
/// start again.
pub fn next_utc_midnight(at: DateTime<Utc>) -> DateTime<Utc> {
    let next = utc_day(at)
        .succ_opt()
        .map(|day| Utc.from_utc_datetime(&day.and_time(chrono::NaiveTime::default())));
    next.unwrap_or_else(|| at + TimeDelta::days(1))
}

/// The stored form of a day: `YYYY-MM-DD`.
pub fn day_key(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

fn as_count(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn as_sql(n: usize) -> ResearchResult<i64> {
    i64::try_from(n).map_err(|_| ResearchError::Invalid(format!("{n} calls is too many.")))
}

/// The day's count on `conn`.
fn count_in(conn: &Connection, day: &str) -> rusqlite::Result<i64> {
    Ok(conn
        .query_row(
            "SELECT calls FROM table_calls WHERE utc_day = ?1",
            params![day],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0))
}

/// Table model calls per UTC day, in `shodh.db`.
pub struct CallCounter {
    conn: SharedConnection,
}

impl std::fmt::Debug for CallCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallCounter").finish_non_exhaustive()
    }
}

impl CallCounter {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> ResearchResult<Self> {
        Ok(Self {
            conn: open_shared_connection(path, key)?,
        })
    }

    /// How long a write waits for a writer of another process (tests make it short to meet
    /// a lock).
    #[cfg(test)]
    pub(crate) fn set_busy_timeout(&self, timeout: std::time::Duration) {
        self.conn.read().busy_timeout(timeout).unwrap();
    }

    /// Adds `n` calls to `day`. Returns the day's total.
    pub fn record(&self, day: NaiveDate, n: usize) -> ResearchResult<usize> {
        let n = as_sql(n)?;
        let key = day_key(day);
        let total = self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO table_calls(utc_day, calls) VALUES (?1, ?2)
                 ON CONFLICT(utc_day) DO UPDATE SET calls = calls + excluded.calls",
                params![key, n],
            )?;
            count_in(tx, &key)
        })?;
        Ok(as_count(total))
    }

    /// Adds `n` calls to `day` only if the day's total stays within `cap`. Returns the new
    /// total, or `None` (nothing recorded) when the calls would pass the cap. The read and
    /// the write are one immediate transaction, so two jobs reserving at once can never
    /// pass the cap together.
    pub fn try_record(
        &self,
        day: NaiveDate,
        n: usize,
        cap: usize,
    ) -> ResearchResult<Option<usize>> {
        let n_sql = as_sql(n)?;
        let key = day_key(day);
        self.conn.write(|tx| {
            let current = as_count(count_in(tx, &key)?);
            if current.saturating_add(n) > cap {
                return Ok(None);
            }
            tx.execute(
                "INSERT INTO table_calls(utc_day, calls) VALUES (?1, ?2)
                 ON CONFLICT(utc_day) DO UPDATE SET calls = calls + excluded.calls",
                params![key, n_sql],
            )?;
            Ok(Some(as_count(count_in(tx, &key)?)))
        })
    }

    /// The calls recorded for `day`.
    pub fn count(&self, day: NaiveDate) -> ResearchResult<usize> {
        let conn = self.conn.read();
        Ok(as_count(count_in(&conn, &day_key(day))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research::tables::testkit::utc;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn counter() -> (tempfile::TempDir, std::path::PathBuf, CallCounter) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let counter = CallCounter::open(&path, None).unwrap();
        (dir, path, counter)
    }

    #[test]
    fn counts_add_up_per_day_and_a_new_day_starts_at_zero() {
        let (_dir, _path, counter) = counter();
        let monday = date(2026, 10, 19);
        let tuesday = date(2026, 10, 20);
        assert_eq!(counter.count(monday).unwrap(), 0);
        assert_eq!(counter.record(monday, 2).unwrap(), 2);
        assert_eq!(counter.record(monday, 1).unwrap(), 3);
        assert_eq!(counter.record(monday, 0).unwrap(), 3);
        assert_eq!(counter.count(monday).unwrap(), 3);
        assert_eq!(counter.count(tuesday).unwrap(), 0);
        assert_eq!(counter.record(tuesday, 1).unwrap(), 1);
        assert_eq!(counter.count(monday).unwrap(), 3);
        assert_eq!(day_key(tuesday), "2026-10-20");
    }

    #[test]
    fn two_connections_see_the_same_count() {
        let (_dir, path, one) = counter();
        let two = CallCounter::open(&path, None).unwrap();
        let day = date(2026, 10, 22);
        one.record(day, 2).unwrap();
        assert_eq!(two.count(day).unwrap(), 2);
        assert_eq!(two.record(day, 3).unwrap(), 5);
        assert_eq!(one.count(day).unwrap(), 5);
        assert_eq!(one.try_record(day, 1, 5).unwrap(), None);
        assert_eq!(two.try_record(day, 1, 6).unwrap(), Some(6));
    }

    #[test]
    fn try_record_never_passes_the_cap() {
        let (_dir, _path, counter) = counter();
        let day = date(2026, 10, 22);
        for expected in 1..=4 {
            assert_eq!(counter.try_record(day, 1, 4).unwrap(), Some(expected));
        }
        assert_eq!(counter.try_record(day, 1, 4).unwrap(), None);
        assert_eq!(
            counter.count(day).unwrap(),
            4,
            "a refused call is not recorded"
        );
        assert_eq!(counter.try_record(day, 3, 6).unwrap(), None);
        assert_eq!(counter.try_record(day, 2, 6).unwrap(), Some(6));
        assert_eq!(
            counter.try_record(date(2026, 10, 23), 3, 4).unwrap(),
            Some(3)
        );
    }

    #[test]
    fn threads_reserving_at_once_never_pass_the_cap() {
        let (_dir, path, first) = counter();
        let day = date(2026, 10, 22);
        let cap = 10;
        let counters: Vec<CallCounter> = (0..4)
            .map(|_| CallCounter::open(&path, None).unwrap())
            .collect();
        let granted: usize = std::thread::scope(|scope| {
            let handles: Vec<_> = counters
                .iter()
                .map(|counter| {
                    scope.spawn(move || {
                        (0..20)
                            .filter(|_| counter.try_record(day, 1, cap).unwrap().is_some())
                            .count()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        });
        assert_eq!(granted, cap, "exactly the cap was granted");
        assert_eq!(first.count(day).unwrap(), cap);
    }

    #[test]
    fn a_shared_cap_changes_for_every_holder_and_refuses_values_out_of_range() {
        assert!(DailyCap::new(MIN_DAILY_CAP - 1).is_err());
        assert!(DailyCap::new(MAX_DAILY_CAP + 1).is_err());
        let cap = DailyCap::new(DEFAULT_DAILY_CAP).unwrap();
        let held = cap.clone();
        cap.set(MIN_DAILY_CAP).unwrap();
        assert_eq!(held.get(), MIN_DAILY_CAP);
        assert!(held.set(0).is_err());
        assert!(held.set(MAX_DAILY_CAP + 1).is_err());
        assert_eq!(cap.get(), MIN_DAILY_CAP, "a refused value changes nothing");
        held.set(MAX_DAILY_CAP).unwrap();
        assert_eq!(cap.get(), MAX_DAILY_CAP);
        let default = DailyCap::default();
        assert_eq!(default.get(), DEFAULT_DAILY_CAP);
        assert!(
            check_cap(default.get()).is_ok(),
            "the default is within the bounds"
        );
    }

    #[test]
    fn the_day_is_the_utc_date_and_resets_at_utc_midnight() {
        assert_eq!(utc_day(utc(2026, 10, 22, 23, 59)), date(2026, 10, 22));
        assert_eq!(utc_day(utc(2026, 10, 23, 0, 0)), date(2026, 10, 23));
        assert_eq!(
            next_utc_midnight(utc(2026, 10, 22, 18, 30)),
            utc(2026, 10, 23, 0, 0)
        );
        assert_eq!(
            next_utc_midnight(utc(2026, 10, 23, 0, 0)),
            utc(2026, 10, 24, 0, 0),
            "at midnight the next one is a day away"
        );
        // A DST switch (29 March in Europe) does not move the day: it is UTC.
        assert_eq!(
            next_utc_midnight(utc(2026, 3, 29, 0, 30)) - utc(2026, 3, 29, 0, 30),
            TimeDelta::minutes(23 * 60 + 30)
        );
    }
}
