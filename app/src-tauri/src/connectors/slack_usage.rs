//! The count of Slack answers per UTC day, against the connector's own daily cap.
//!
//! Same day and reset as the evidence tables' cap (`research::tables::calls`): the UTC
//! date, the provider's own daily window, and an answer is counted before its model call
//! is made, so a crash over-counts by at most one and never under-counts.
//!
//! Kept in a small file next to the settings (`connectors/slack_usage.json`, written
//! atomically with a backup) rather than in `shodh.db`: a new `shodh.db` table needs a
//! schema migration, and the database's writers are being moved behind one write gate
//! (#95); the Slack tables of part B go there after it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use shodh_rag::connectors::slack::respond::AnswerQuota;
use shodh_rag::research::tables::calls::day_key;

use crate::durable_file::{self, Loaded, Origin, ReadFailure};

/// The usage file, under the app data directory.
pub const USAGE_FILE: &str = "connectors/slack_usage.json";
/// Days kept in the file (older ones are dropped on write).
const DAYS_KEPT: usize = 7;

/// Slack answers per day, by `YYYY-MM-DD` (UTC).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Usage {
    #[serde(default)]
    days: BTreeMap<String, usize>,
}

/// The daily count of Slack answers, shared by every connection of this process.
pub struct SlackUsage {
    path: PathBuf,
    cap: Arc<AtomicUsize>,
    lock: Mutex<()>,
}

impl std::fmt::Debug for SlackUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackUsage")
            .field("path", &self.path)
            .field("cap", &self.cap())
            .finish()
    }
}

impl SlackUsage {
    /// The counter of the app data directory `data_dir`, with the cap `cap` (shared: a
    /// change in the card applies from the next question).
    pub fn new(data_dir: &Path, cap: Arc<AtomicUsize>) -> Self {
        Self {
            path: data_dir.join(USAGE_FILE),
            cap,
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> Result<Loaded<Usage>, String> {
        durable_file::read_with_backup(
            &self.path,
            |text| serde_json::from_str::<Usage>(text).map_err(|e| e.to_string()),
            Usage::default,
        )
        .or_else(|failure| match failure {
            // A damaged count with no usable backup starts again from the cap, not from
            // zero: the day's calls may already be spent.
            ReadFailure::Damaged { problem, .. } => {
                tracing::warn!(target: "shodh::slack", path = %self.path.display(), %problem, "the Slack answer count was damaged; today counts as used up");
                let mut usage = Usage::default();
                usage.days.insert(
                    day_key(chrono::Utc::now().date_naive()),
                    self.cap.load(Ordering::SeqCst),
                );
                // Saving moves the damaged files aside, never over them.
                Ok(Loaded {
                    value: usage,
                    origin: Origin::Damaged { problem },
                })
            }
            ReadFailure::Io { message, .. } => Err(message),
        })
    }

    fn write(&self, usage: &mut Usage, origin: &Origin) -> Result<(), String> {
        while usage.days.len() > DAYS_KEPT {
            let Some(oldest) = usage.days.keys().next().cloned() else {
                break;
            };
            usage.days.remove(&oldest);
        }
        let text = serde_json::to_string_pretty(usage).map_err(|e| e.to_string())?;
        durable_file::write_with_backup(&self.path, text.as_bytes(), origin)
            .map_err(|e| format!("The Slack answer count could not be saved: {e}"))
    }
}

impl AnswerQuota for SlackUsage {
    fn cap(&self) -> usize {
        self.cap.load(Ordering::SeqCst)
    }

    fn used(&self, day: NaiveDate) -> Result<usize, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self
            .read()?
            .value
            .days
            .get(&day_key(day))
            .copied()
            .unwrap_or(0))
    }

    fn try_reserve(&self, day: NaiveDate) -> Result<bool, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Loaded { mut value, origin } = self.read()?;
        let key = day_key(day);
        let used = value.days.get(&key).copied().unwrap_or(0);
        if used >= self.cap() {
            return Ok(false);
        }
        value.days.insert(key, used + 1);
        self.write(&mut value, &origin)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    #[test]
    fn answers_are_counted_per_utc_day_up_to_the_cap_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let cap = Arc::new(AtomicUsize::new(2));
        let usage = SlackUsage::new(dir.path(), cap.clone());
        assert_eq!(usage.used(day(23)).unwrap(), 0);
        assert!(usage.try_reserve(day(23)).unwrap());
        assert!(usage.try_reserve(day(23)).unwrap());
        assert!(!usage.try_reserve(day(23)).unwrap(), "the cap holds");
        assert_eq!(
            usage.used(day(23)).unwrap(),
            2,
            "a refused answer is not counted"
        );
        assert!(
            usage.try_reserve(day(24)).unwrap(),
            "a new day starts at zero"
        );
        // A raised cap applies at once; the count survives a restart.
        cap.store(3, Ordering::SeqCst);
        let again = SlackUsage::new(dir.path(), cap);
        assert_eq!(again.used(day(23)).unwrap(), 2);
        assert!(again.try_reserve(day(23)).unwrap());
        assert_eq!(again.used(day(24)).unwrap(), 1);
    }

    #[test]
    fn only_the_last_week_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let usage = SlackUsage::new(dir.path(), Arc::new(AtomicUsize::new(5)));
        for d in 1..=10 {
            assert!(usage.try_reserve(day(d)).unwrap());
        }
        let text = std::fs::read_to_string(dir.path().join(USAGE_FILE)).unwrap();
        let kept: Usage = serde_json::from_str(&text).unwrap();
        assert_eq!(kept.days.len(), DAYS_KEPT);
        assert!(kept.days.contains_key("2026-10-10"));
        assert!(!kept.days.contains_key("2026-10-03"));
    }

    #[test]
    fn a_damaged_count_without_backup_spends_nothing_today() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(USAGE_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        let usage = SlackUsage::new(dir.path(), Arc::new(AtomicUsize::new(4)));
        let today = chrono::Utc::now().date_naive();
        assert_eq!(usage.used(today).unwrap(), 4);
        assert!(!usage.try_reserve(today).unwrap());
    }
}
