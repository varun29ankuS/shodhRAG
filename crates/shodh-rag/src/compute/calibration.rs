//! The search model's measured speed, kept between loads so it is measured
//! when the model, the target or the machine changes, and once a week
//! ([`super::speed::CALIBRATION_MAX_AGE`]), not every time the model loads.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::provider::ComputePreference;
use super::speed::CALIBRATION_MAX_AGE;

/// One calibration of the search model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    /// The model file and its size: another model is measured again.
    pub model_file: String,
    pub model_size: u64,
    pub preference: ComputePreference,
    /// The graphics card the model started on, when one did.
    pub device: Option<String>,
    /// Tokens a second on the graphics card, when one started.
    pub gpu_tokens_per_second: Option<f64>,
    /// Tokens a second on the CPU.
    pub cpu_tokens_per_second: f64,
    /// The CPU's intra-op threads (another count is another speed).
    pub threads: usize,
    pub measured_at: DateTime<Utc>,
}

impl Calibration {
    /// Whether this calibration holds for a load of `model_file`
    /// (`model_size` bytes) on `device` with `threads`, at `now`.
    pub fn matches(
        &self,
        model_file: &str,
        model_size: u64,
        preference: ComputePreference,
        device: Option<&str>,
        threads: usize,
        now: DateTime<Utc>,
    ) -> bool {
        let fresh = (now - self.measured_at)
            .to_std()
            .map_or(false, |age| age < CALIBRATION_MAX_AGE);
        fresh
            && self.model_file == model_file
            && self.model_size == model_size
            && self.preference == preference
            && self.device.as_deref() == device
            && self.threads == threads
            && self.cpu_tokens_per_second > 0.0
    }

    /// The calibration saved at `path`; `None` when there is none or it
    /// cannot be read (the model is then measured again).
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(calibration) => Some(calibration),
            Err(e) => {
                tracing::warn!(error = %e, "The saved search model speed is unreadable; measuring again");
                None
            }
        }
    }

    /// Saves this calibration at `path` (written beside it, then renamed).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        let partial = path.with_extension("json.part");
        std::fs::write(&partial, json)?;
        std::fs::rename(&partial, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibration(at: DateTime<Utc>) -> Calibration {
        Calibration {
            model_file: "model.onnx".to_string(),
            model_size: 100,
            preference: ComputePreference::Auto,
            device: Some("RTX 4060".to_string()),
            gpu_tokens_per_second: Some(20_000.0),
            cpu_tokens_per_second: 1_500.0,
            threads: 8,
            measured_at: at,
        }
    }

    #[test]
    fn a_calibration_holds_only_for_the_same_model_target_and_week() {
        let at = Utc::now();
        let c = calibration(at);
        let auto = ComputePreference::Auto;
        assert!(c.matches("model.onnx", 100, auto, Some("RTX 4060"), 8, at));
        assert!(!c.matches("model_O4.onnx", 100, auto, Some("RTX 4060"), 8, at));
        assert!(!c.matches("model.onnx", 101, auto, Some("RTX 4060"), 8, at));
        assert!(!c.matches(
            "model.onnx",
            100,
            ComputePreference::Cpu,
            Some("RTX 4060"),
            8,
            at
        ));
        assert!(!c.matches("model.onnx", 100, auto, None, 8, at));
        assert!(!c.matches("model.onnx", 100, auto, Some("RTX 4060"), 4, at));
        let week = chrono::Duration::from_std(CALIBRATION_MAX_AGE).expect("duration");
        assert!(!c.matches("model.onnx", 100, auto, Some("RTX 4060"), 8, at + week));
        assert!(c.matches(
            "model.onnx",
            100,
            auto,
            Some("RTX 4060"),
            8,
            at + week - chrono::Duration::hours(1)
        ));
    }

    #[test]
    fn a_calibration_survives_a_save_and_a_damaged_file_is_ignored() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("search-speed.json");
        assert_eq!(Calibration::load(&path), None);
        let c = calibration(Utc::now());
        c.save(&path).expect("saved");
        assert_eq!(Calibration::load(&path), Some(c));
        std::fs::write(&path, b"{not json").expect("damaged");
        assert_eq!(Calibration::load(&path), None);
    }
}
