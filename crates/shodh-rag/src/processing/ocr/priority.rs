//! OCR runs behind the user's work: its threads are scheduled below normal
//! (Windows `THREAD_PRIORITY_BELOW_NORMAL`, macOS QoS "utility"), and ONNX
//! Runtime gets half the logical cores ([`ocr_threads`]). Only OCR threads are
//! lowered, never the process: the thread that reads a scan is lowered while it
//! reads ([`BelowNormal`]) and the threads ONNX Runtime starts for the OCR
//! sessions are lowered for their life ([`BackgroundThreads`]).

/// Intra-op threads of each OCR session: half the logical cores, at least
/// one. The thread that runs a session counts as one of them.
pub fn ocr_threads() -> usize {
    threads_for(std::thread::available_parallelism().map_or(2, |n| n.get()))
}

fn threads_for(logical_cores: usize) -> usize {
    (logical_cores / 2).max(1)
}

/// Lowers the current thread to below-normal scheduling until dropped, then
/// restores what it had. Where the platform has no per-thread setting, or it
/// fails, nothing changes.
#[must_use = "the thread is restored when the guard is dropped"]
pub struct BelowNormal {
    previous: Option<platform::Level>,
    // The setting belongs to this thread: the guard must be dropped on it.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl BelowNormal {
    pub fn new() -> Self {
        Self {
            previous: platform::lower(),
            _not_send: std::marker::PhantomData,
        }
    }

    /// Whether the thread was lowered.
    pub fn lowered(&self) -> bool {
        self.previous.is_some()
    }
}

impl Default for BelowNormal {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BelowNormal {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            platform::restore(previous);
        }
    }
}

/// Starts the threads ONNX Runtime uses for an OCR session, lowered for their
/// life (they are joined when the session is dropped).
pub struct BackgroundThreads;

impl ort::environment::ThreadManager for BackgroundThreads {
    type Thread = std::thread::JoinHandle<()>;

    fn create(&self, work: impl FnOnce() + Send + 'static) -> ort::Result<Self::Thread> {
        std::thread::Builder::new()
            .name("shodh-ocr".to_string())
            .spawn(move || {
                // Kept for the thread's life.
                let _lowered = BelowNormal::new();
                work();
            })
            .map_err(|e| ort::Error::new(format!("starting an OCR thread: {e}")))
    }

    fn join(thread: Self::Thread) -> ort::Result<()> {
        thread
            .join()
            .map_err(|_| ort::Error::new("an OCR thread panicked"))
    }
}

#[cfg(windows)]
mod platform {
    /// A thread priority (`GetThreadPriority`).
    pub type Level = i32;

    const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
    const THREAD_PRIORITY_ERROR_RETURN: i32 = 0x7fff_ffff;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn GetThreadPriority(thread: isize) -> i32;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }

    pub fn current() -> Option<Level> {
        // SAFETY: the pseudo handle of the current thread is always valid.
        let level = unsafe { GetThreadPriority(GetCurrentThread()) };
        (level != THREAD_PRIORITY_ERROR_RETURN).then_some(level)
    }

    fn set(level: Level) -> bool {
        // SAFETY: as above; SetThreadPriority only reads its arguments.
        unsafe { SetThreadPriority(GetCurrentThread(), level) != 0 }
    }

    /// Whether a thread at `level` runs above below-normal.
    pub fn can_lower(level: Level) -> bool {
        level > THREAD_PRIORITY_BELOW_NORMAL
    }

    /// Lowers the current thread; the level it had, when it was lowered.
    /// A thread already at or below below-normal is left as it is.
    pub fn lower() -> Option<Level> {
        let previous = current().filter(|&l| can_lower(l))?;
        set(THREAD_PRIORITY_BELOW_NORMAL).then_some(previous)
    }

    pub fn restore(previous: Level) {
        if !set(previous) {
            tracing::debug!(previous, "thread priority not restored");
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    /// A quality-of-service class (`qos_class_t`).
    pub type Level = u32;

    const QOS_CLASS_UTILITY: u32 = 0x11;
    const QOS_CLASS_DEFAULT: u32 = 0x15;
    const QOS_CLASS_UNSPECIFIED: u32 = 0x00;

    extern "C" {
        fn qos_class_self() -> u32;
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    }

    pub fn current() -> Option<Level> {
        // SAFETY: reads the calling thread's QoS class.
        Some(unsafe { qos_class_self() })
    }

    fn set(level: Level) -> bool {
        // SAFETY: sets the calling thread's own QoS class.
        unsafe { pthread_set_qos_class_self_np(level, 0) == 0 }
    }

    /// Whether a thread of class `level` runs above the utility class (a
    /// thread without a class runs at the default class).
    pub fn can_lower(level: Level) -> bool {
        level == QOS_CLASS_UNSPECIFIED || level > QOS_CLASS_UTILITY
    }

    /// Lowers the current thread to the utility class; the class it had, when
    /// it was lowered. A thread already at utility or below is left as it is.
    pub fn lower() -> Option<Level> {
        let previous = current().filter(|&l| can_lower(l))?;
        set(QOS_CLASS_UTILITY).then_some(previous)
    }

    /// A thread that had no class goes back to the default class, which is
    /// how such a thread is scheduled.
    pub fn restore(previous: Level) {
        let level = if previous == QOS_CLASS_UNSPECIFIED {
            QOS_CLASS_DEFAULT
        } else {
            previous
        };
        if !set(level) {
            tracing::debug!(level, "thread QoS class not restored");
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    /// No per-thread setting is used on other platforms.
    pub type Level = ();

    #[cfg(test)]
    pub fn current() -> Option<Level> {
        None
    }

    #[cfg(test)]
    pub fn can_lower(_level: Level) -> bool {
        false
    }

    pub fn lower() -> Option<Level> {
        None
    }

    pub fn restore(_previous: Level) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_gets_half_the_cores_and_at_least_one() {
        assert_eq!(threads_for(12), 6);
        assert_eq!(threads_for(4), 2);
        assert_eq!(threads_for(3), 1);
        assert_eq!(threads_for(1), 1);
        assert!(ocr_threads() >= 1);
    }

    /// The guard lowers only its own thread and restores it on drop.
    #[test]
    fn ocr_priority_is_lowered_for_the_guard_and_restored_after() {
        std::thread::spawn(|| {
            let before = platform::current();
            if cfg!(any(windows, target_os = "macos")) {
                assert!(before.is_some(), "the thread's scheduling is readable");
            }
            {
                let guard = BelowNormal::new();
                assert_eq!(guard.lowered(), before.is_some_and(platform::can_lower));
                if guard.lowered() {
                    assert!(platform::current().is_some_and(|l| !platform::can_lower(l)));
                    // A nested guard leaves the lowered thread alone.
                    assert!(!BelowNormal::new().lowered());
                }
            }
            let after = platform::current();
            if cfg!(target_os = "macos") {
                // A thread without a class comes back at the default class.
                assert_eq!(
                    after.is_some_and(platform::can_lower),
                    before.is_some_and(platform::can_lower)
                );
            } else {
                assert_eq!(after, before);
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn ocr_runtime_threads_run_lowered() {
        use ort::environment::ThreadManager;
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = BackgroundThreads
            .create(move || {
                tx.send(platform::current()).unwrap();
            })
            .unwrap();
        BackgroundThreads::join(thread).unwrap();
        let level = rx.recv().unwrap();
        if cfg!(any(windows, target_os = "macos")) {
            assert!(level.is_some_and(|l| !platform::can_lower(l)), "{level:?}");
        } else {
            assert_eq!(level, None);
        }
    }
}
