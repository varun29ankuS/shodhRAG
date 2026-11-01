//! What the search model runs on and how fast, as the app shows it: kept
//! by the engine, written when the model loads and after each batch it
//! embeds, and read by the status panel at any time (also while the model
//! is not loaded: the last report stays, marked not loaded).
//!
//! "Slower than usual" ([`EmbedderReport::degraded`]) is judged only on
//! batches that had the model to themselves: their time is the inference
//! alone (not the wait for the session), no other batch ran meanwhile, and
//! no OCR competed for the cores ([`super::contention`]). It clears when the
//! model is unloaded and after [`IDLE_CLEAR`] without batches.

use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use super::provider::{Accelerator, CpuReason};
use super::speed::{self, SpeedClass, ThroughputWatch};

/// Without a batch for this long the recent speed is forgotten: an idle
/// model is not running slowly.
pub const IDLE_CLEAR: Duration = Duration::from_secs(60);

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
    /// The model file ("model_O4.onnx").
    pub model_file: String,
    /// Calibrated speed; `None` when it could not be measured.
    pub pages_per_minute: Option<f64>,
    pub speed: Option<SpeedClass>,
    /// The recent batches' speed, once there are enough of them.
    pub recent_pages_per_minute: Option<f64>,
    /// The recent batches ran well below the calibrated speed.
    pub degraded: bool,
    /// When the speed was calibrated (RFC 3339), when it was.
    pub calibrated_at: Option<String>,
    /// Whether the model is in memory now.
    pub loaded: bool,
}

struct Watch {
    throughput: ThroughputWatch,
    last_batch: Option<Instant>,
}

/// The engine's compute status, shared with the model loaders.
pub struct ComputeStatus {
    embedder: RwLock<Option<EmbedderReport>>,
    watch: Mutex<Option<Watch>>,
    /// The load the report is of: an older model dropped after a newer one
    /// loaded does not mark the report unloaded.
    generation: Mutex<u64>,
    /// Counts the changes worth showing (a load, an unload, a change of
    /// the degraded state), for [`Self::subscribe`].
    changes: tokio::sync::watch::Sender<u64>,
}

impl std::fmt::Debug for ComputeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputeStatus")
            .field("embedder", &*self.embedder.read())
            .finish_non_exhaustive()
    }
}

impl Default for ComputeStatus {
    fn default() -> Self {
        Self {
            embedder: RwLock::new(None),
            watch: Mutex::new(None),
            generation: Mutex::new(0),
            changes: tokio::sync::watch::Sender::new(0),
        }
    }
}

impl ComputeStatus {
    /// Wakes on every change worth showing: the model loaded or was
    /// unloaded, or its batches became slower than calibrated, or recovered.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// The search model loaded: what runs it and its calibrated speed in
    /// tokens a second (`None`: not measured). Returns the load's generation,
    /// for [`Self::embedder_unloaded`].
    pub fn embedder_loaded(
        &self,
        mut report: EmbedderReport,
        tokens_per_second: Option<f64>,
    ) -> u64 {
        report.loaded = true;
        report.recent_pages_per_minute = None;
        report.degraded = false;
        *self.watch.lock() = tokens_per_second.map(|rate| Watch {
            throughput: ThroughputWatch::new(rate),
            last_batch: None,
        });
        let generation = {
            let mut generation = self.generation.lock();
            *generation += 1;
            *generation
        };
        *self.embedder.write() = Some(report);
        self.changed();
        generation
    }

    /// What runs the model changed while it was loaded (a graphics card
    /// failed and the CPU took over): the report says so, speed unmeasured.
    pub fn embedder_moved(
        &self,
        generation: u64,
        accelerator: Accelerator,
        reason: CpuReason,
        threads: usize,
    ) {
        if *self.generation.lock() != generation {
            return;
        }
        *self.watch.lock() = None;
        if let Some(report) = self.embedder.write().as_mut() {
            report.accelerator = accelerator;
            report.device = None;
            report.cpu_reason = Some(reason);
            report.threads = Some(threads);
            report.pages_per_minute = None;
            report.speed = None;
            report.recent_pages_per_minute = None;
            report.degraded = false;
            report.calibrated_at = None;
        }
        self.changed();
    }

    /// The model of load `generation` was dropped from memory; its report
    /// stays, not loaded and with the recent speed forgotten. A model of an
    /// older load changes nothing.
    pub fn embedder_unloaded(&self, generation: u64) {
        if *self.generation.lock() != generation {
            return;
        }
        if let Some(watch) = self.watch.lock().as_mut() {
            watch.throughput.clear();
            watch.last_batch = None;
        }
        if let Some(report) = self.embedder.write().as_mut() {
            report.loaded = false;
            report.degraded = false;
            report.recent_pages_per_minute = None;
        }
        self.changed();
    }

    /// A batch of `tokens` was embedded in `elapsed` of inference;
    /// `contended` when another batch or OCR competed with it.
    pub fn embedded(&self, tokens: usize, elapsed: Duration, contended: bool) {
        self.embedded_at(Instant::now(), tokens, elapsed, contended);
    }

    fn embedded_at(&self, now: Instant, tokens: usize, elapsed: Duration, contended: bool) {
        let (recent, degraded) = {
            let mut watch = self.watch.lock();
            let Some(watch) = watch.as_mut() else {
                return;
            };
            if watch
                .last_batch
                .is_some_and(|at| now.saturating_duration_since(at) >= IDLE_CLEAR)
            {
                watch.throughput.clear();
            }
            watch.last_batch = Some(now);
            if !contended {
                watch.throughput.record(tokens, elapsed);
            }
            (watch.throughput.recent_rate(), watch.throughput.degraded())
        };
        self.set_recent(recent, degraded);
    }

    fn set_recent(&self, recent: Option<f64>, degraded: bool) {
        let flipped = match self.embedder.write().as_mut() {
            Some(report) => {
                let was = report.degraded;
                report.recent_pages_per_minute = recent.map(speed::pages_per_minute);
                report.degraded = degraded;
                if degraded && !was {
                    tracing::warn!(
                        calibrated_pages_per_minute = report.pages_per_minute,
                        recent_pages_per_minute = report.recent_pages_per_minute,
                        "The search model runs well below its calibrated speed"
                    );
                }
                was != degraded
            }
            None => false,
        };
        if flipped {
            self.changed();
        }
    }

    /// The search model's last known state; `None` before it first loaded.
    pub fn embedder(&self) -> Option<EmbedderReport> {
        self.embedder_at(Instant::now())
    }

    fn embedder_at(&self, now: Instant) -> Option<EmbedderReport> {
        let idle = self.watch.lock().as_mut().is_some_and(|watch| {
            let idle = watch
                .last_batch
                .is_some_and(|at| now.saturating_duration_since(at) >= IDLE_CLEAR);
            if idle {
                watch.throughput.clear();
                watch.last_batch = None;
            }
            idle
        });
        if idle {
            self.set_recent(None, false);
        }
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
            model_file: "model_O4.onnx".to_string(),
            pages_per_minute: Some(speed::pages_per_minute(1000.0)),
            speed: Some(speed::classify(1000.0)),
            recent_pages_per_minute: None,
            degraded: false,
            calibrated_at: Some("2026-11-01T00:00:00Z".to_string()),
            loaded: false,
        }
    }

    fn slow_batches(status: &ComputeStatus, start: Instant) -> Instant {
        let mut now = start;
        for _ in 0..speed::DEGRADED_WINDOW {
            now += Duration::from_secs(1);
            status.embedded_at(now, 2000, Duration::from_secs(10), false);
        }
        now
    }

    #[test]
    fn batches_far_below_the_calibrated_speed_mark_the_model_degraded() {
        let status = ComputeStatus::default();
        assert_eq!(status.embedder(), None);
        status.embedder_loaded(report(), Some(1000.0));
        let now = slow_batches(&status, Instant::now());
        let degraded = status.embedder_at(now).expect("report");
        assert!(degraded.degraded);
        assert_eq!(
            degraded.recent_pages_per_minute,
            Some(speed::pages_per_minute(200.0))
        );
        // A new load is calibrated again and starts clean.
        status.embedder_loaded(report(), Some(1000.0));
        assert!(status.embedder().is_some_and(|r| !r.degraded));
    }

    #[test]
    fn contended_batches_are_not_judged() {
        let status = ComputeStatus::default();
        status.embedder_loaded(report(), Some(1000.0));
        let mut now = Instant::now();
        for _ in 0..speed::DEGRADED_WINDOW * 2 {
            now += Duration::from_secs(1);
            // Waiting for the session, or sharing the cores with OCR.
            status.embedded_at(now, 2000, Duration::from_secs(10), true);
        }
        let report = status.embedder_at(now).expect("report");
        assert!(!report.degraded);
        assert_eq!(report.recent_pages_per_minute, None);
    }

    #[test]
    fn an_idle_model_is_not_slow_and_an_unloaded_one_forgets_its_batches() {
        let status = ComputeStatus::default();
        let generation = status.embedder_loaded(report(), Some(1000.0));
        let now = slow_batches(&status, Instant::now());
        assert!(status.embedder_at(now).is_some_and(|r| r.degraded));
        // A minute without batches: no longer slow.
        let later = now + IDLE_CLEAR;
        assert!(status.embedder_at(later).is_some_and(|r| !r.degraded));
        // Slow again, then unloaded: the report stays, not slow.
        let now = slow_batches(&status, later);
        assert!(status.embedder_at(now).is_some_and(|r| r.degraded));
        status.embedder_unloaded(generation);
        let unloaded = status.embedder_at(now).expect("report kept");
        assert!(!unloaded.loaded && !unloaded.degraded);
        assert_eq!(unloaded.cpu_reason, Some(CpuReason::NoGpu));
    }

    #[test]
    fn an_older_model_dropped_late_does_not_unload_the_new_one() {
        let status = ComputeStatus::default();
        let old = status.embedder_loaded(report(), Some(1000.0));
        let new = status.embedder_loaded(report(), Some(1000.0));
        status.embedder_unloaded(old);
        assert!(status.embedder().is_some_and(|r| r.loaded));
        status.embedder_unloaded(new);
        assert!(status.embedder().is_some_and(|r| !r.loaded));
    }

    #[test]
    fn a_card_that_fails_while_loaded_moves_the_report_to_the_cpu() {
        let status = ComputeStatus::default();
        let mut gpu = report();
        gpu.accelerator = Accelerator::DirectMl;
        gpu.device = Some("RTX 4060".to_string());
        gpu.cpu_reason = None;
        let generation = status.embedder_loaded(gpu, Some(20_000.0));
        let reason = CpuReason::Failed {
            device: Some("RTX 4060".to_string()),
            error: "device removed".to_string(),
        };
        status.embedder_moved(generation, Accelerator::Cpu, reason.clone(), 8);
        let moved = status.embedder().expect("report");
        assert_eq!(moved.accelerator, Accelerator::Cpu);
        assert_eq!(moved.cpu_reason, Some(reason));
        assert_eq!(
            moved.pages_per_minute, None,
            "the CPU's speed is not measured"
        );
        assert!(moved.loaded);
    }

    #[test]
    fn an_unmeasured_model_is_never_judged() {
        let status = ComputeStatus::default();
        let mut unmeasured = report();
        unmeasured.pages_per_minute = None;
        unmeasured.speed = None;
        status.embedder_loaded(unmeasured, None);
        slow_batches(&status, Instant::now());
        assert!(status.embedder().is_some_and(|r| !r.degraded));
    }

    #[test]
    fn loads_and_slowdowns_are_announced_but_steady_batches_are_not() {
        let status = ComputeStatus::default();
        let mut changes = status.subscribe();
        assert!(!changes.has_changed().expect("open"));
        let generation = status.embedder_loaded(report(), Some(1000.0));
        assert!(changes.has_changed().expect("open"));
        changes.borrow_and_update();
        let mut now = Instant::now();
        for _ in 0..speed::DEGRADED_WINDOW {
            now += Duration::from_secs(1);
            status.embedded_at(now, 2000, Duration::from_secs(2), false);
        }
        assert!(
            !changes.has_changed().expect("open"),
            "steady batches are quiet"
        );
        slow_batches(&status, now);
        assert!(
            changes.has_changed().expect("open"),
            "the slowdown is announced"
        );
        changes.borrow_and_update();
        status.embedder_unloaded(generation);
        assert!(changes.has_changed().expect("open"));
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
