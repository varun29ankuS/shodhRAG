//! What the search model's targets were measured to do, kept between loads
//! ([`super::choose::Known`]), so a model that is unloaded when idle and
//! loaded again is not measured, and a card that failed is not tried, every
//! time.
//!
//! Every entry holds only for the same model file and size, thread count and
//! app version (a new ONNX Runtime comes with an app update), and a card's
//! entry only for the same card and driver (Windows) or system build (macOS).
//! Speeds are measured again after a week ([`CALIBRATION_MAX_AGE`]). A card
//! that failed is tried again only when one of those changes, or the GPU
//! setting is changed ([`forget`]).

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::choose::{Known, Learned};
use super::provider::{CpuReason, Device};
use super::speed::CALIBRATION_MAX_AGE;

/// What every entry must match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationKey {
    pub model_file: String,
    pub model_size: u64,
    pub threads: usize,
    pub app_version: String,
}

impl CalibrationKey {
    /// The key of `model_file` (`model_size` bytes) on `threads` threads,
    /// in this build of the app.
    pub fn new(model_file: &str, model_size: u64, threads: usize) -> Self {
        Self {
            model_file: model_file.to_string(),
            model_size,
            threads,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// What an entry is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Target {
    Cpu,
    Card {
        name: String,
        driver: Option<String>,
    },
}

impl Target {
    fn of(device: Option<&Device>) -> Self {
        match device {
            None => Target::Cpu,
            Some(d) => Target::Card {
                name: d.name.clone(),
                driver: d.driver.clone(),
            },
        }
    }
}

/// What was found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Verdict {
    Cpu { tokens_per_second: f64 },
    Kept { tokens_per_second: f64 },
    Rejected { reason: CpuReason },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub key: CalibrationKey,
    pub target: Target,
    pub verdict: Verdict,
    pub measured_at: DateTime<Utc>,
}

impl Entry {
    fn fresh(&self, now: DateTime<Utc>) -> bool {
        // A card that failed stays failed until the key or the driver
        // changes; speeds age.
        if matches!(
            self.verdict,
            Verdict::Rejected {
                reason: CpuReason::Failed { .. }
            }
        ) {
            return true;
        }
        (now - self.measured_at)
            .to_std()
            .is_ok_and(|age| age < CALIBRATION_MAX_AGE)
    }
}

/// The saved entries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calibrations {
    pub entries: Vec<Entry>,
}

impl Calibrations {
    /// The entries saved at `path`; none when there are none or the file
    /// cannot be read (everything is measured again).
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "The saved search model speeds are unreadable; measuring again");
            Self::default()
        })
    }

    /// Saves the entries at `path` (written beside it, then renamed).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        let partial = path.with_extension("json.part");
        std::fs::write(&partial, json)?;
        std::fs::rename(&partial, path)
    }

    /// What is known about `device` (the CPU for `None`) under `key` at `now`.
    pub fn known(
        &self,
        key: &CalibrationKey,
        device: Option<&Device>,
        now: DateTime<Utc>,
    ) -> Option<Known> {
        let target = Target::of(device);
        let entry = self
            .entries
            .iter()
            .find(|e| e.key == *key && e.target == target && e.fresh(now))?;
        Some(match &entry.verdict {
            Verdict::Cpu { tokens_per_second } => Known::Cpu {
                cpu: *tokens_per_second,
            },
            Verdict::Kept { tokens_per_second } => Known::Kept {
                gpu: *tokens_per_second,
            },
            Verdict::Rejected { reason } => Known::Rejected(reason.clone()),
        })
    }

    /// When what is known about `device` (the CPU for `None`) was measured.
    pub fn measured_at(
        &self,
        key: &CalibrationKey,
        device: Option<&Device>,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let target = Target::of(device);
        self.entries
            .iter()
            .find(|e| e.key == *key && e.target == target && e.fresh(now))
            .map(|e| e.measured_at)
    }

    /// Records `learned` under `key` at `now`, replacing what was known
    /// about the same targets; entries of other keys go.
    pub fn learn(&mut self, key: &CalibrationKey, learned: &[Learned], now: DateTime<Utc>) {
        self.entries.retain(|e| e.key == *key);
        for item in learned {
            let (target, verdict) = match item {
                Learned::Cpu { cpu } => (
                    Target::Cpu,
                    Verdict::Cpu {
                        tokens_per_second: *cpu,
                    },
                ),
                Learned::Kept { device, gpu } => (
                    Target::of(Some(device)),
                    Verdict::Kept {
                        tokens_per_second: *gpu,
                    },
                ),
                Learned::Rejected { device, reason } => (
                    Target::of(Some(device)),
                    Verdict::Rejected {
                        reason: reason.clone(),
                    },
                ),
            };
            self.entries.retain(|e| e.target != target);
            self.entries.push(Entry {
                key: key.clone(),
                target,
                verdict,
                measured_at: now,
            });
        }
    }
}

/// Forgets every saved measurement at `path` (the GPU setting changed: try
/// the cards again).
pub fn forget(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(error = %e, "The saved search model speeds were not removed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::provider::Accelerator;

    fn card(driver: &str) -> Device {
        Device {
            accelerator: Accelerator::DirectMl,
            name: "RTX 4060".to_string(),
            index: Some(0),
            driver: Some(driver.to_string()),
        }
    }

    fn key() -> CalibrationKey {
        CalibrationKey::new("model_O4.onnx", 100, 8)
    }

    #[test]
    fn a_measurement_holds_for_the_same_model_threads_app_card_and_driver() {
        let now = Utc::now();
        let mut saved = Calibrations::default();
        saved.learn(
            &key(),
            &[
                Learned::Cpu { cpu: 1500.0 },
                Learned::Kept {
                    device: card("31.0.1"),
                    gpu: 9000.0,
                },
            ],
            now,
        );
        assert_eq!(
            saved.known(&key(), None, now),
            Some(Known::Cpu { cpu: 1500.0 })
        );
        assert_eq!(
            saved.known(&key(), Some(&card("31.0.1")), now),
            Some(Known::Kept { gpu: 9000.0 })
        );
        assert_eq!(
            saved.known(&key(), Some(&card("32.0.0")), now),
            None,
            "a new driver"
        );
        assert_eq!(
            saved.known(&CalibrationKey::new("model_O4.onnx", 101, 8), None, now),
            None
        );
        assert_eq!(
            saved.known(&CalibrationKey::new("model_O4.onnx", 100, 4), None, now),
            None
        );
        let other_app = CalibrationKey {
            app_version: "0.0.1".to_string(),
            ..key()
        };
        assert_eq!(
            saved.known(&other_app, None, now),
            None,
            "another app version"
        );
        let week = chrono::Duration::from_std(CALIBRATION_MAX_AGE).unwrap();
        assert_eq!(saved.known(&key(), None, now + week), None, "a week old");
    }

    #[test]
    fn a_failed_card_stays_failed_until_its_driver_changes_and_a_new_verdict_replaces_the_old() {
        let now = Utc::now();
        let mut saved = Calibrations::default();
        let failed = CpuReason::Failed {
            device: Some("RTX 4060".to_string()),
            error: "887A0005".to_string(),
        };
        saved.learn(
            &key(),
            &[Learned::Rejected {
                device: card("31.0.1"),
                reason: failed.clone(),
            }],
            now,
        );
        let much_later = now + chrono::Duration::days(60);
        assert_eq!(
            saved.known(&key(), Some(&card("31.0.1")), much_later),
            Some(Known::Rejected(failed))
        );
        assert_eq!(saved.known(&key(), Some(&card("31.0.2")), much_later), None);
        saved.learn(
            &key(),
            &[Learned::Kept {
                device: card("31.0.1"),
                gpu: 8000.0,
            }],
            now,
        );
        assert_eq!(saved.entries.len(), 1);
    }

    #[test]
    fn measurements_survive_a_save_and_a_damaged_file_or_forget_measures_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("search-speed.json");
        assert_eq!(Calibrations::load(&path), Calibrations::default());
        let mut saved = Calibrations::default();
        saved.learn(&key(), &[Learned::Cpu { cpu: 1200.0 }], Utc::now());
        saved.save(&path).unwrap();
        assert_eq!(Calibrations::load(&path), saved);
        forget(&path);
        assert_eq!(Calibrations::load(&path), Calibrations::default());
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Calibrations::load(&path), Calibrations::default());
    }
}
