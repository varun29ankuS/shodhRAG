//! What the search model runs on and how fast, as the app shows it: kept
//! by the engine, written when the model loads and after each batch it
//! embeds, and read by the status panel at any time (also while the model
//! is not loaded: the last report stays).

use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use super::provider::{Accelerator, CpuReason};
use super::speed::{self, SpeedClass, ThroughputWatch};

/// The search model's last known state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedderReport {
    pub accelerator: Accelerator,
    /// The graphics card's name, when one runs the model.
    pub device: Option<String>,
    /// Why the CPU runs the model, when it does.
    pub cpu_reason: Option<CpuReason>,
    /// Intra-op threads, when the CPU runs the model.
    pub threads: Option<usize>,
    /// The model file ("model_qint8_avx512_vnni.onnx").
    pub model_file: String,
    /// Calibrated speed.
    pub pages_per_minute: f64,
    pub speed: SpeedClass,
    /// The recent batches' speed, once there are enough of them.
    pub recent_pages_per_minute: Option<f64>,
    /// The recent batches ran well below the calibrated speed.
    pub degraded: bool,
    /// When the speed was calibrated (RFC 3339).
    pub calibrated_at: String,
    /// Whether the model is in memory now.
    pub loaded: bool,
}

/// The engine's compute status, shared with the model loaders.
#[derive(Debug, Default)]
pub struct ComputeStatus {
    embedder: RwLock<Option<EmbedderReport>>,
    watch: Mutex<Option<ThroughputWatch>>,
}

impl ComputeStatus {
    /// The search model loaded: what runs it and its calibrated speed in
    /// tokens a second.
    pub fn embedder_loaded(&self, mut report: EmbedderReport, tokens_per_second: f64) {
        report.loaded = true;
        report.recent_pages_per_minute = None;
        report.degraded = false;
        *self.watch.lock() = Some(ThroughputWatch::new(tokens_per_second));
        *self.embedder.write() = Some(report);
    }

    /// The search model was dropped from memory; its report stays.
    pub fn embedder_unloaded(&self) {
        if let Some(report) = self.embedder.write().as_mut() {
            report.loaded = false;
        }
    }

    /// A batch of `tokens` was embedded in `elapsed`.
    pub fn embedded(&self, tokens: usize, elapsed: Duration) {
        let (recent, degraded) = {
            let mut watch = self.watch.lock();
            let Some(watch) = watch.as_mut() else {
                return;
            };
            watch.record(tokens, elapsed);
            (watch.recent_rate(), watch.degraded())
        };
        if let Some(report) = self.embedder.write().as_mut() {
            let was = report.degraded;
            report.recent_pages_per_minute = recent.map(speed::pages_per_minute);
            report.degraded = degraded;
            if degraded && !was {
                tracing::warn!(
                    calibrated_pages_per_minute = report.pages_per_minute,
                    recent_pages_per_minute = report.recent_pages_per_minute,
                    "The search model runs well below its calibrated speed (throttled or busy)"
                );
            }
        }
    }

    /// The search model's last known state; `None` before it first loaded.
    pub fn embedder(&self) -> Option<EmbedderReport> {
        self.embedder.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> EmbedderReport {
        EmbedderReport {
            accelerator: Accelerator::Cpu,
            device: None,
            cpu_reason: Some(CpuReason::NoGpu),
            threads: Some(8),
            model_file: "model.onnx".to_string(),
            pages_per_minute: speed::pages_per_minute(1000.0),
            speed: speed::classify(1000.0),
            recent_pages_per_minute: None,
            degraded: false,
            calibrated_at: "2026-11-01T00:00:00Z".to_string(),
            loaded: false,
        }
    }

    #[test]
    fn batches_far_below_the_calibrated_speed_mark_the_model_degraded() {
        let status = ComputeStatus::default();
        assert_eq!(status.embedder(), None);
        status.embedder_loaded(report(), 1000.0);
        assert!(status.embedder().is_some_and(|r| r.loaded && !r.degraded));
        for _ in 0..speed::DEGRADED_WINDOW {
            status.embedded(2000, Duration::from_secs(10));
        }
        let degraded = status.embedder().expect("report");
        assert!(degraded.degraded);
        assert_eq!(
            degraded.recent_pages_per_minute,
            Some(speed::pages_per_minute(200.0))
        );
        // A new load is calibrated again and starts clean.
        status.embedder_loaded(report(), 1000.0);
        assert!(status.embedder().is_some_and(|r| !r.degraded));
    }

    #[test]
    fn the_report_outlives_an_unload() {
        let status = ComputeStatus::default();
        status.embedder_loaded(report(), 1000.0);
        status.embedder_unloaded();
        let kept = status.embedder().expect("report kept");
        assert!(!kept.loaded);
        assert_eq!(kept.cpu_reason, Some(CpuReason::NoGpu));
    }

    #[test]
    fn the_report_serializes_in_camel_case() {
        let json = serde_json::to_value(report()).expect("json");
        assert_eq!(json["accelerator"], "cpu");
        assert_eq!(json["cpuReason"]["kind"], "noGpu");
        assert_eq!(json["speed"], "slow");
        assert!(json.get("pagesPerMinute").is_some());
    }
}
