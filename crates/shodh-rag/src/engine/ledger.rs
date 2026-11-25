//! The ledger of model calls: how many calls each part of Shodh (a *spender*) made per
//! UTC day, and against which quota pool, in `shodh.db` (`model_calls`).
//!
//! - **The day is the UTC date**, the window of the provider's own daily quota
//!   (OpenRouter's free quota resets at 00:00 UTC); the interface shows the reset in local
//!   time.
//! - **The pool** is the provider limit the calls count against
//!   ([`crate::harness::model_catalog::ModelRef::quota_pool`]): OpenRouter limits its free
//!   models per account, so two free models share one pool. `''` when it is not known.
//! - **Chat is counted, never refused**: [`Ledger::record`] always records. A spender with
//!   a cap (the evidence tables) reserves through its own counter
//!   ([`crate::research::tables::calls::CallCounter`]), which writes the same rows.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::clock::day_key;
use super::store::{EngineError, EngineResult};
use crate::audit::{open_shared_connection, AuditKey, SharedConnection};

/// What spent a model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Spender {
    /// The user's own answers: Ask, its side threads and the phone.
    Chat,
    Tables,
    Automations,
    Review,
    Slack,
    Telegram,
    Learning,
}

impl Spender {
    /// Every spender, in the order the counter names them.
    pub const ALL: [Spender; 7] = [
        Spender::Chat,
        Spender::Tables,
        Spender::Automations,
        Spender::Review,
        Spender::Slack,
        Spender::Telegram,
        Spender::Learning,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Spender::Chat => "chat",
            Spender::Tables => "tables",
            Spender::Automations => "automations",
            Spender::Review => "review",
            Spender::Slack => "slack",
            Spender::Telegram => "telegram",
            Spender::Learning => "learning",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }
}

/// Adds `n` calls of `spender` in `pool` to the day `day` (a [`day_key`]).
pub(crate) fn record_in(
    conn: &Connection,
    day: &str,
    spender: Spender,
    pool: &str,
    n: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO model_calls(utc_day, spender, pool, calls) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(utc_day, spender, pool) DO UPDATE SET calls = calls + excluded.calls",
        params![day, spender.as_str(), pool, n],
    )?;
    Ok(())
}

/// The calls of `spender` on the day `day` (a [`day_key`]), over every pool.
pub(crate) fn spender_count_in(
    conn: &Connection,
    day: &str,
    spender: Spender,
) -> rusqlite::Result<i64> {
    Ok(conn
        .query_row(
            "SELECT SUM(calls) FROM model_calls WHERE utc_day = ?1 AND spender = ?2",
            params![day, spender.as_str()],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten()
        .unwrap_or(0))
}

fn as_count(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// One spender's calls of a day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpenderCalls {
    pub spender: Spender,
    pub calls: u32,
}

/// A day's calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayCalls {
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    pub total: u32,
    /// The spenders that made calls, in [`Spender::ALL`]'s order.
    pub by_spender: Vec<SpenderCalls>,
    /// Calls per quota pool (`''`: not known).
    pub by_pool: BTreeMap<String, u32>,
}

/// The ledger of model calls.
pub struct Ledger {
    conn: SharedConnection,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ledger").finish_non_exhaustive()
    }
}

impl Ledger {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> EngineResult<Self> {
        Ok(Self {
            conn: open_shared_connection(path, key)?,
        })
    }

    /// Records `n` calls of `spender` in `pool` on `day`, without any cap. Returns the
    /// spender's total of the day.
    pub fn record(
        &self,
        day: NaiveDate,
        spender: Spender,
        pool: &str,
        n: u32,
    ) -> EngineResult<u32> {
        let key = day_key(day);
        let total = self.conn.write(|tx| {
            record_in(tx, &key, spender, pool, i64::from(n))?;
            spender_count_in(tx, &key, spender)
        })?;
        Ok(as_count(total))
    }

    /// The calls of `spender` on `day`, over every pool.
    pub fn count(&self, day: NaiveDate, spender: Spender) -> EngineResult<u32> {
        let conn = self.conn.read()?;
        Ok(as_count(spender_count_in(&conn, &day_key(day), spender)?))
    }

    /// Every call of `day`, by spender and by pool.
    pub fn day(&self, day: NaiveDate) -> EngineResult<DayCalls> {
        let key = day_key(day);
        let conn = self.conn.read()?;
        let mut statement = conn.prepare(
            "SELECT spender, pool, calls FROM model_calls WHERE utc_day = ?1 AND calls > 0",
        )?;
        let rows = statement
            .query_map(params![key], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut spenders: BTreeMap<Spender, u32> = BTreeMap::new();
        let mut by_pool: BTreeMap<String, u32> = BTreeMap::new();
        let mut total: u32 = 0;
        for (spender, pool, calls) in rows {
            let spender = Spender::parse(&spender)
                .ok_or_else(|| EngineError::Invalid(format!("unknown spender {spender}")))?;
            let calls = as_count(calls);
            *spenders.entry(spender).or_default() += calls;
            *by_pool.entry(pool).or_default() += calls;
            total = total.saturating_add(calls);
        }
        Ok(DayCalls {
            day: key,
            total,
            by_spender: Spender::ALL
                .into_iter()
                .filter_map(|spender| {
                    spenders
                        .get(&spender)
                        .map(|&calls| SpenderCalls { spender, calls })
                })
                .collect(),
            by_pool,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    fn ledger() -> (tempfile::TempDir, Ledger) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&dir.path().join("shodh.db"), None).unwrap();
        (dir, ledger)
    }

    #[test]
    fn spenders_and_pools_add_up_per_utc_day() {
        let (_dir, ledger) = ledger();
        assert_eq!(
            ledger
                .record(date(25), Spender::Chat, "openrouter:free", 2)
                .unwrap(),
            2
        );
        assert_eq!(
            ledger
                .record(date(25), Spender::Chat, "lmstudio", 1)
                .unwrap(),
            3
        );
        ledger
            .record(date(25), Spender::Tables, "openrouter:free", 4)
            .unwrap();
        ledger
            .record(date(26), Spender::Chat, "openrouter:free", 5)
            .unwrap();
        assert_eq!(ledger.count(date(25), Spender::Chat).unwrap(), 3);
        assert_eq!(ledger.count(date(25), Spender::Automations).unwrap(), 0);
        let day = ledger.day(date(25)).unwrap();
        assert_eq!(day.day, "2026-11-25");
        assert_eq!(day.total, 7);
        assert_eq!(
            day.by_spender,
            vec![
                SpenderCalls {
                    spender: Spender::Chat,
                    calls: 3
                },
                SpenderCalls {
                    spender: Spender::Tables,
                    calls: 4
                },
            ]
        );
        assert_eq!(
            day.by_pool.into_iter().collect::<Vec<_>>(),
            vec![
                ("lmstudio".to_string(), 1),
                ("openrouter:free".to_string(), 6)
            ]
        );
        assert_eq!(ledger.day(date(24)).unwrap().total, 0, "an empty day");
    }

    #[test]
    fn chat_is_never_refused() {
        let (_dir, ledger) = ledger();
        for expected in 1..=1_200 {
            assert_eq!(
                ledger.record(date(25), Spender::Chat, "", 1).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn spenders_round_trip_as_text() {
        for spender in Spender::ALL {
            assert_eq!(Spender::parse(spender.as_str()), Some(spender));
        }
        assert_eq!(Spender::parse("cron"), None);
    }
}
