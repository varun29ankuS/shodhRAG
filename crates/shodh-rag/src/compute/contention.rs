//! Work of Shodh's own that competes with the search model for the CPU.
//!
//! OCR of a scan (in this process or in its child) keeps cores busy for
//! minutes. Batches embedded meanwhile run slower without the machine being
//! throttled, so they are not counted towards "slower than usual", and a
//! calibration measured meanwhile is used but not saved.

use std::sync::atomic::{AtomicUsize, Ordering};

static OCR_ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Marks OCR as running while it lives.
#[must_use = "OCR counts as running only while the guard lives"]
pub struct OcrActive(());

impl OcrActive {
    pub fn begin() -> Self {
        OCR_ACTIVE.fetch_add(1, Ordering::SeqCst);
        Self(())
    }
}

impl Default for OcrActive {
    fn default() -> Self {
        Self::begin()
    }
}

impl Drop for OcrActive {
    fn drop(&mut self) {
        OCR_ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether OCR runs now.
pub fn ocr_active() -> bool {
    OCR_ACTIVE.load(Ordering::SeqCst) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_counts_as_running_while_its_guard_lives() {
        // Other tests may read scans at the same time: compare with a base.
        let first = OcrActive::begin();
        assert!(ocr_active());
        let second = OcrActive::begin();
        drop(first);
        assert!(ocr_active(), "the second guard still runs");
        drop(second);
    }
}
