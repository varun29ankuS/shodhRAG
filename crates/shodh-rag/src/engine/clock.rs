//! The clock: injected so that days, resets and waits are deterministic in tests, and the
//! UTC day that call counters count on.

use chrono::{DateTime, LocalResult, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Utc};

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

/// The instant a local wall-clock time means in `tz`. An ambiguous time
/// (clocks went back, the hour happens twice) is its first occurrence; a
/// time that does not exist (clocks went forward) is the first minute after
/// the gap, so a reminder in the skipped hour rings as soon as it ends
/// rather than never.
pub fn resolve_local<Tz: TimeZone>(tz: &Tz, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    let mut probe = local;
    // Real gaps are at most a few hours; a day bounds the search.
    for _ in 0..=24 * 60 {
        match tz.from_local_datetime(&probe) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => {
                return Some(t.with_timezone(&Utc))
            }
            LocalResult::None => probe += chrono::Duration::minutes(1),
        }
    }
    None
}
