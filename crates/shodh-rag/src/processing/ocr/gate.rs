//! Pause and cancel between the pages of a scan.
//!
//! OCR reads a scan page by page for minutes. The indexer installs a gate on
//! the thread that reads a file ([`with_gate`]); before each page the OCR
//! loop asks it ([`before_page`]):
//! - in this process, a paused indexer holds the loop until it is resumed,
//!   and a cancelled one ends it ([`OcrInterrupted`]);
//! - in the OCR child process ([`super::worker`]), which cannot be held, the
//!   parent stops the child on a pause or a cancel; a paused file is read
//!   again from its first page once indexing resumes.
//!
//! Without a gate (searches, tests, the child itself) every page is read.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

/// What the indexer says before a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageGate {
    Go,
    Pause,
    Stop,
}

/// The gate a reading thread asks.
pub type Gate = Arc<dyn Fn() -> PageGate + Send + Sync>;

/// OCR of a file ended before its last page: the indexer paused (the file
/// is read again after the resume) or cancelled it (the file is dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("reading the scan was {}", if *.paused { "paused" } else { "cancelled" })]
pub struct OcrInterrupted {
    pub paused: bool,
}

thread_local! {
    static GATE: RefCell<Option<Gate>> = const { RefCell::new(None) };
}

/// How often a held page asks again.
const HOLD_POLL: Duration = Duration::from_millis(200);

/// Runs `read` with `gate` installed on this thread (restored after).
pub fn with_gate<R>(gate: Gate, read: impl FnOnce() -> R) -> R {
    let previous = GATE.with(|g| g.replace(Some(gate)));
    struct Restore(Option<Gate>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            GATE.with(|g| *g.borrow_mut() = previous);
        }
    }
    let _restore = Restore(previous);
    read()
}

/// The gate's answer now, `Go` without one.
pub fn current() -> PageGate {
    GATE.with(|g| g.borrow().as_ref().map_or(PageGate::Go, |gate| gate()))
}

/// Before a page read in this process: holds while paused, fails when
/// cancelled.
pub fn before_page() -> Result<(), OcrInterrupted> {
    loop {
        match current() {
            PageGate::Go => return Ok(()),
            PageGate::Stop => return Err(OcrInterrupted { paused: false }),
            PageGate::Pause => std::thread::sleep(HOLD_POLL),
        }
    }
}

/// Before a page read by the child process: a pause or a cancel ends the
/// read (the child is stopped by the caller).
pub fn before_child_page() -> Result<(), OcrInterrupted> {
    match current() {
        PageGate::Go => Ok(()),
        PageGate::Pause => Err(OcrInterrupted { paused: true }),
        PageGate::Stop => Err(OcrInterrupted { paused: false }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn without_a_gate_every_page_is_read() {
        assert_eq!(current(), PageGate::Go);
        assert_eq!(before_page(), Ok(()));
        assert_eq!(before_child_page(), Ok(()));
    }

    #[test]
    fn a_paused_gate_holds_until_resumed_and_a_stop_ends_the_read() {
        let asked = Arc::new(AtomicUsize::new(0));
        let gate: Gate = {
            let asked = asked.clone();
            Arc::new(move || match asked.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => PageGate::Pause,
                _ => PageGate::Go,
            })
        };
        with_gate(gate, || assert_eq!(before_page(), Ok(())));
        assert_eq!(asked.load(Ordering::SeqCst), 3, "held twice, then read");
        let stop: Gate = Arc::new(|| PageGate::Stop);
        with_gate(stop.clone(), || {
            assert_eq!(before_page(), Err(OcrInterrupted { paused: false }))
        });
        let pause: Gate = Arc::new(|| PageGate::Pause);
        with_gate(pause, || {
            assert_eq!(before_child_page(), Err(OcrInterrupted { paused: true }))
        });
        // The gate is gone after the read.
        assert_eq!(current(), PageGate::Go);
    }
}
