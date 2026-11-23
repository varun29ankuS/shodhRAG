//! The clock: injected so that days, resets and waits are deterministic in tests, and the
//! UTC day that call counters count on.

use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};

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
