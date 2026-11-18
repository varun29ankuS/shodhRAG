//! Work of Shodh's own that competes with the search model for the machine.
//!
//! OCR of a scan (in this process or in its child) keeps cores busy for
//! minutes. A llama.cpp generation in this process runs on the graphics card,
//! where Core ML embedding halves its tokens per second. Batches embedded
//! meanwhile run slower without the machine being throttled, so they are not
//! counted towards "slower than usual", and a calibration measured meanwhile
//! is used but not saved.

use std::sync::atomic::{AtomicUsize, Ordering};

static OCR_ACTIVE: AtomicUsize = AtomicUsize::new(0);
static GENERATION_ACTIVE: AtomicUsize = AtomicUsize::new(0);

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

/// Marks a llama.cpp generation in this process as running while it lives.
#[must_use = "the generation counts as running only while the guard lives"]
pub struct GenerationActive(());

impl GenerationActive {
    pub fn begin() -> Self {
        GENERATION_ACTIVE.fetch_add(1, Ordering::SeqCst);
        Self(())
    }
}

impl Drop for GenerationActive {
    fn drop(&mut self) {
        GENERATION_ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether a llama.cpp generation in this process runs now.
pub fn generation_active() -> bool {
    GENERATION_ACTIVE.load(Ordering::SeqCst) > 0
}

/// Whether any work of Shodh's own competes with the search model now.
pub fn competing() -> bool {
    ocr_active() || generation_active()
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

    #[test]
    fn a_generation_counts_as_competing_while_its_guard_lives() {
        let generating = GenerationActive::begin();
        assert!(generation_active());
        assert!(competing());
        drop(generating);
    }
}
