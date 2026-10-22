//! The model calls evidence tables made per local day, and the clock that says which day
//! it is. The daily cap (Settings, [`DEFAULT_DAILY_CAP`] by default) is checked against
//! this count. Only table fills count here: chat and agents do not, so a table fill can
//! never silently spend the quota a conversation needs, and the fill dialog says "Tables
//! have made N of 30 calls today".
//!
//! A call is counted before it is made ([`CallCounter::try_record`] reserves it), so a
//! crash between the two over-counts by at most one call and never under-counts: on a
//! free quota of about 50 calls a day an over-count pauses a job one call early, while an
//! under-count could run into the provider's own daily limit.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeDelta, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::budget::MAX_CALLS_PER_PAPER;
use crate::audit::{open_shared_connection, AuditKey};
use crate::research::{ResearchError, ResearchResult};

/// The daily cap of table calls when the user has not set one.
pub const DEFAULT_DAILY_CAP: usize = 30;
/// Calls one paper can cost: its planned calls and the one retry. A paper starts only
/// when this many are left under the cap.
pub const CALLS_PER_PAPER_RESERVE: usize = MAX_CALLS_PER_PAPER + 1;
/// The smallest cap under which a paper can start at all.
pub const MIN_DAILY_CAP: usize = CALLS_PER_PAPER_RESERVE;

/// The time, and the local time zone at a given instant. Injected so that days, midnights
/// and waits are deterministic in tests.
pub trait Clock: Send + Sync {
    /// Now, in UTC.
    fn now(&self) -> DateTime<Utc>;
    /// The local offset from UTC at `at`.
    fn offset_at(&self, at: DateTime<Utc>) -> FixedOffset;
}

/// The computer's clock and time zone. The offset is read at every call, so a time-zone
/// change or a daylight-saving switch applies from the next check.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn offset_at(&self, at: DateTime<Utc>) -> FixedOffset {
        Local.offset_from_utc_datetime(&at.naive_utc())
    }
}

/// The local date at `at`.
pub fn local_day(clock: &dyn Clock, at: DateTime<Utc>) -> NaiveDate {
    at.with_timezone(&clock.offset_at(at)).date_naive()
}

/// The instant local midnight `day` 00:00 would be under `offset`.
fn midnight_under(day: NaiveDate, offset: FixedOffset) -> DateTime<Utc> {
    let local = day.and_time(chrono::NaiveTime::default());
    let utc = local - TimeDelta::seconds(i64::from(offset.local_minus_utc()));
    Utc.from_utc_datetime(&utc)
}

/// The first instant after `at` whose local date is the next one: the next local midnight,
/// or, on a day whose midnight is skipped by a clock change, the instant the day starts.
/// The offset is read at `at` and again at each candidate, so a 23- or 25-hour day ends
/// when the local clock says it does.
pub fn next_local_midnight(clock: &dyn Clock, at: DateTime<Utc>) -> DateTime<Utc> {
    let today = local_day(clock, at);
    let Some(next) = today.succ_opt() else {
        return at + TimeDelta::days(1);
    };
    let mut candidates = vec![midnight_under(next, clock.offset_at(at))];
    for _ in 0..3 {
        let Some(last) = candidates.last().copied() else {
            break;
        };
        let refined = midnight_under(next, clock.offset_at(last));
        if candidates.contains(&refined) {
            break;
        }
        candidates.push(refined);
    }
    candidates
        .into_iter()
        .filter(|&c| c > at && local_day(clock, c) >= next)
        .min()
        .unwrap_or_else(|| at + TimeDelta::days(1))
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
            "SELECT calls FROM model_calls WHERE day = ?1",
            params![day],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0))
}

/// Table model calls per local day, in `shodh.db`.
pub struct CallCounter {
    conn: Mutex<Connection>,
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
            conn: Mutex::new(open_shared_connection(path, key)?),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds `n` calls to `day`. Returns the day's total.
    pub fn record(&self, day: NaiveDate, n: usize) -> ResearchResult<usize> {
        let n = as_sql(n)?;
        let key = day_key(day);
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO model_calls(day, calls) VALUES (?1, ?2)
             ON CONFLICT(day) DO UPDATE SET calls = calls + excluded.calls",
            params![key, n],
        )?;
        let total = count_in(&tx, &key)?;
        tx.commit()?;
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
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = as_count(count_in(&tx, &key)?);
        if current.saturating_add(n) > cap {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO model_calls(day, calls) VALUES (?1, ?2)
             ON CONFLICT(day) DO UPDATE SET calls = calls + excluded.calls",
            params![key, n_sql],
        )?;
        let total = count_in(&tx, &key)?;
        tx.commit()?;
        Ok(Some(as_count(total)))
    }

    /// The calls recorded for `day`.
    pub fn count(&self, day: NaiveDate) -> ResearchResult<usize> {
        let conn = self.lock();
        Ok(as_count(count_in(&conn, &day_key(day))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research::tables::testkit::{utc, ManualClock};

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
    fn the_day_and_the_next_midnight_follow_the_local_clock_across_dst() {
        // Spring forward (Europe style): +01:00 until 01:00 UTC on 29 March, then +02:00.
        // 29 March is 23 hours long.
        let spring = ManualClock::new(
            utc(2026, 3, 28, 22, 30),
            &[
                (utc(2000, 1, 1, 0, 0), 3_600),
                (utc(2026, 3, 29, 1, 0), 7_200),
            ],
        );
        let at = spring.now();
        assert_eq!(local_day(&spring, at), date(2026, 3, 28));
        let midnight = next_local_midnight(&spring, at);
        assert_eq!(midnight, utc(2026, 3, 28, 23, 0));
        assert_eq!(local_day(&spring, midnight), date(2026, 3, 29));
        let after = next_local_midnight(&spring, midnight);
        assert_eq!(after, utc(2026, 3, 29, 22, 0), "a 23-hour day");
        assert_eq!(after - midnight, TimeDelta::hours(23));

        // Fall back: +02:00 until 01:00 UTC on 25 October, then +01:00. 25 hours long.
        let fall = ManualClock::new(
            utc(2026, 10, 24, 21, 0),
            &[
                (utc(2000, 1, 1, 0, 0), 7_200),
                (utc(2026, 10, 25, 1, 0), 3_600),
            ],
        );
        let start = next_local_midnight(&fall, fall.now());
        assert_eq!(start, utc(2026, 10, 24, 22, 0));
        assert_eq!(local_day(&fall, start), date(2026, 10, 25));
        assert_eq!(
            next_local_midnight(&fall, start) - start,
            TimeDelta::hours(25),
            "a 25-hour day"
        );

        // Midnight skipped: -03:00 until local 00:00 on 1 November (03:00 UTC), then
        // -02:00, so the day starts at 01:00 local, which is 03:00 UTC.
        let skipped = ManualClock::new(
            utc(2026, 10, 31, 20, 0),
            &[
                (utc(2000, 1, 1, 0, 0), -10_800),
                (utc(2026, 11, 1, 3, 0), -7_200),
            ],
        );
        let at = skipped.now();
        assert_eq!(local_day(&skipped, at), date(2026, 10, 31));
        let first = next_local_midnight(&skipped, at);
        assert_eq!(first, utc(2026, 11, 1, 3, 0));
        assert_eq!(local_day(&skipped, first), date(2026, 11, 1));
        assert_eq!(
            local_day(&skipped, first - TimeDelta::seconds(1)),
            date(2026, 10, 31)
        );

        // East of UTC, 23:30 UTC is already the next local date.
        let east = ManualClock::new(utc(2026, 10, 22, 23, 30), &[(utc(2000, 1, 1, 0, 0), 7_200)]);
        assert_eq!(local_day(&east, east.now()), date(2026, 10, 23));
        assert_eq!(
            next_local_midnight(&east, east.now()),
            utc(2026, 10, 23, 22, 0)
        );
    }
}
