//! How fast the search model embeds on this machine: a short calibration
//! when it loads, a speed class the user can read, and a watch on the
//! batches that follow for a machine that has slowed down (a laptop on
//! battery, a processor throttled by heat).
//!
//! Speeds are in tokens per second: batches differ in size and length, and
//! the model's work grows with the tokens it reads. The speed the user sees
//! is in pages: a page of text is about 650 tokens ([`TOKENS_PER_PAGE`]).

use std::collections::VecDeque;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Tokens of an average page of text (about 500 words; the E5 tokenizer
/// reads about 1.3 tokens a word in English prose).
pub const TOKENS_PER_PAGE: f64 = 650.0;

/// At or above this many pages a minute indexing is fast: a library of 100
/// papers (about 1,500 pages) in under three minutes.
pub const FAST_PAGES_PER_MINUTE: f64 = 600.0;

/// Below this many pages a minute indexing is slow: the same 100 papers take
/// more than ten minutes.
pub const SLOW_PAGES_PER_MINUTE: f64 = 150.0;

/// Below this share of the calibrated speed, measured over
/// [`DEGRADED_WINDOW`] batches, the machine is running degraded.
pub const DEGRADED_SHARE: f64 = 0.5;

/// Batches the degradation watch looks at: one slow batch (another program
/// briefly busy) is not a degraded machine.
pub const DEGRADED_WINDOW: usize = 8;

/// Batches smaller than this are not watched: a single query embeds a few
/// dozen tokens, where fixed overheads dominate and the speed says nothing
/// about the machine.
pub const MIN_WATCHED_TOKENS: usize = 512;

/// A calibration older than this is measured again at the next load.
pub const CALIBRATION_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How fast indexing runs, in the user's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeedClass {
    Fast,
    Ok,
    Slow,
}

/// Pages a minute at `tokens_per_second`.
pub fn pages_per_minute(tokens_per_second: f64) -> f64 {
    tokens_per_second * 60.0 / TOKENS_PER_PAGE
}

/// The speed class of `tokens_per_second`.
pub fn classify(tokens_per_second: f64) -> SpeedClass {
    let pages = pages_per_minute(tokens_per_second);
    if pages >= FAST_PAGES_PER_MINUTE {
        SpeedClass::Fast
    } else if pages >= SLOW_PAGES_PER_MINUTE {
        SpeedClass::Ok
    } else {
        SpeedClass::Slow
    }
}

/// Tokens per second of a batch of `tokens` tokens that took `elapsed`;
/// `None` for a batch too short to time.
pub fn rate(tokens: usize, elapsed: Duration) -> Option<f64> {
    let seconds = elapsed.as_secs_f64();
    (seconds > 0.0 && tokens > 0).then(|| tokens as f64 / seconds)
}

/// Watches embedding batches against the calibrated speed.
#[derive(Debug, Clone)]
pub struct ThroughputWatch {
    calibrated: f64,
    recent: VecDeque<f64>,
}

impl ThroughputWatch {
    /// A watch against `calibrated` tokens per second.
    pub fn new(calibrated: f64) -> Self {
        Self {
            calibrated,
            recent: VecDeque::with_capacity(DEGRADED_WINDOW),
        }
    }

    pub fn calibrated(&self) -> f64 {
        self.calibrated
    }

    /// Forgets the recent batches.
    pub fn clear(&mut self) {
        self.recent.clear();
    }

    /// Records a batch of `tokens` that took `elapsed`. Batches below
    /// [`MIN_WATCHED_TOKENS`] are ignored.
    pub fn record(&mut self, tokens: usize, elapsed: Duration) {
        if tokens < MIN_WATCHED_TOKENS {
            return;
        }
        let Some(rate) = rate(tokens, elapsed) else {
            return;
        };
        if self.recent.len() == DEGRADED_WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(rate);
    }

    /// The median speed of the recent batches, once there are
    /// [`DEGRADED_WINDOW`] of them.
    pub fn recent_rate(&self) -> Option<f64> {
        if self.recent.len() < DEGRADED_WINDOW {
            return None;
        }
        let mut rates: Vec<f64> = self.recent.iter().copied().collect();
        rates.sort_by(f64::total_cmp);
        let mid = rates.len() / 2;
        Some(if rates.len() % 2 == 0 {
            (rates[mid - 1] + rates[mid]) / 2.0
        } else {
            rates[mid]
        })
    }

    /// Whether the recent batches ran well below the calibrated speed.
    pub fn degraded(&self) -> bool {
        self.recent_rate()
            .is_some_and(|recent| recent < self.calibrated * DEGRADED_SHARE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens_per_second_for(pages_per_minute: f64) -> f64 {
        pages_per_minute * TOKENS_PER_PAGE / 60.0
    }

    #[test]
    fn speeds_are_classed_at_the_stated_thresholds() {
        assert_eq!(classify(tokens_per_second_for(1000.0)), SpeedClass::Fast);
        assert_eq!(
            classify(tokens_per_second_for(FAST_PAGES_PER_MINUTE)),
            SpeedClass::Fast
        );
        assert_eq!(
            classify(tokens_per_second_for(FAST_PAGES_PER_MINUTE - 1.0)),
            SpeedClass::Ok
        );
        assert_eq!(
            classify(tokens_per_second_for(SLOW_PAGES_PER_MINUTE)),
            SpeedClass::Ok
        );
        assert_eq!(
            classify(tokens_per_second_for(SLOW_PAGES_PER_MINUTE - 1.0)),
            SpeedClass::Slow
        );
        assert_eq!(classify(0.0), SpeedClass::Slow);
    }

    #[test]
    fn pages_per_minute_converts_tokens_per_second() {
        assert!((pages_per_minute(TOKENS_PER_PAGE / 60.0) - 1.0).abs() < 1e-9);
        assert!((pages_per_minute(TOKENS_PER_PAGE) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn an_untimed_batch_has_no_rate() {
        assert_eq!(rate(1000, Duration::ZERO), None);
        assert_eq!(rate(0, Duration::from_secs(1)), None);
        assert_eq!(rate(2000, Duration::from_secs(2)), Some(1000.0));
    }

    #[test]
    fn a_machine_at_its_calibrated_speed_is_not_degraded() {
        let mut watch = ThroughputWatch::new(1000.0);
        for _ in 0..DEGRADED_WINDOW * 2 {
            watch.record(2000, Duration::from_secs(2));
        }
        assert_eq!(watch.recent_rate(), Some(1000.0));
        assert!(!watch.degraded());
    }

    #[test]
    fn a_sustained_drop_below_half_the_calibrated_speed_is_degraded() {
        let mut watch = ThroughputWatch::new(1000.0);
        // Throttled: 400 tokens a second.
        for _ in 0..DEGRADED_WINDOW - 1 {
            watch.record(2000, Duration::from_secs(5));
        }
        assert!(!watch.degraded(), "too few batches to tell");
        watch.record(2000, Duration::from_secs(5));
        assert!(watch.degraded());
        // Back to full speed: the window refills and the state clears.
        for _ in 0..DEGRADED_WINDOW {
            watch.record(2000, Duration::from_secs(2));
        }
        assert!(!watch.degraded());
    }

    #[test]
    fn a_few_slow_batches_do_not_make_a_degraded_machine() {
        let mut watch = ThroughputWatch::new(1000.0);
        for i in 0..DEGRADED_WINDOW {
            let seconds = if i % 4 == 0 { 10 } else { 2 };
            watch.record(2000, Duration::from_secs(seconds));
        }
        assert!(!watch.degraded());
    }

    #[test]
    fn small_batches_are_not_watched() {
        let mut watch = ThroughputWatch::new(1000.0);
        for _ in 0..DEGRADED_WINDOW * 2 {
            watch.record(MIN_WATCHED_TOKENS - 1, Duration::from_secs(60));
        }
        assert_eq!(watch.recent_rate(), None);
        assert!(!watch.degraded());
    }
}
