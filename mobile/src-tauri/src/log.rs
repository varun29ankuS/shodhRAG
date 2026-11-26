//! The app's log, where each phone keeps one. Rust's own output goes nowhere
//! on a phone, so each `tracing` line, and a panic, is written there:
//!
//! - **Android:** logcat, under the tag `Shodh` (what `adb logcat -s Shodh`
//!   shows, and what CI's launch test reads).
//! - **iPhone:** the unified log, through `NSLog` (what Console and `log
//!   stream` show, in a debug build and a release one, and what CI's launch
//!   test reads from the simulator).

use std::io::{self, Write};

use tracing::{Level, Metadata};
use tracing_subscriber::fmt::MakeWriter;

/// How much a line matters, as each platform's log orders it.
#[derive(Clone, Copy)]
enum Priority {
    Debug,
    Info,
    Warn,
    Error,
}

#[cfg(target_os = "android")]
mod sink {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};

    use super::Priority;

    /// The logcat tag of every line.
    const TAG: &[u8] = b"Shodh\0";

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    /// Write one line to logcat.
    pub fn write(priority: Priority, text: &str) {
        // android/log.h priorities.
        let priority: c_int = match priority {
            Priority::Debug => 3,
            Priority::Info => 4,
            Priority::Warn => 5,
            Priority::Error => 6,
        };
        // A NUL inside would end the line early; there is none in our messages.
        let Ok(line) = CString::new(text.replace('\0', " ")) else {
            return;
        };
        // SAFETY: both pointers are NUL-terminated strings that outlive the call.
        unsafe {
            __android_log_write(priority, TAG.as_ptr().cast(), line.as_ptr());
        }
    }
}

#[cfg(target_os = "ios")]
mod sink {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    use super::Priority;

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {
        /// Foundation's `NSLog(NSString *format, ...)`; a `CFString` is an
        /// `NSString` (toll-free bridged).
        fn NSLog(format: CFStringRef, ...);
    }

    /// Write one line to the unified log. `NSLog` has no priorities, so a
    /// warning or an error says so in its own words.
    pub fn write(priority: Priority, text: &str) {
        let line = match priority {
            Priority::Debug | Priority::Info => CFString::new(text),
            Priority::Warn => CFString::new(&format!("warning: {text}")),
            Priority::Error => CFString::new(&format!("error: {text}")),
        };
        let format = CFString::from_static_string("%@");
        // SAFETY: the format takes one object, and both strings outlive the call.
        unsafe {
            NSLog(format.as_concrete_TypeRef(), line.as_concrete_TypeRef());
        }
    }
}

/// Write one line to the phone's log.
fn write(priority: Priority, text: &str) {
    let text = text.trim_end();
    if !text.is_empty() {
        sink::write(priority, text);
    }
}

/// One formatted `tracing` event, written to the log when it is dropped.
pub struct Line {
    priority: Priority,
    buffer: Vec<u8>,
}

impl Write for Line {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        write(self.priority, &String::from_utf8_lossy(&self.buffer));
    }
}

/// The `tracing` writer: a log line per event, at the event's priority.
pub struct PhoneLog;

impl<'a> MakeWriter<'a> for PhoneLog {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line {
            priority: Priority::Info,
            buffer: Vec::new(),
        }
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Line {
        let priority = match *meta.level() {
            Level::ERROR => Priority::Error,
            Level::WARN => Priority::Warn,
            Level::INFO => Priority::Info,
            Level::DEBUG | Level::TRACE => Priority::Debug,
        };
        Line {
            priority,
            buffer: Vec::new(),
        }
    }
}

/// Send the app's `tracing` output (info and above) and its panics to the
/// phone's log.
pub fn init() {
    let subscriber = tracing_subscriber::fmt()
        .with_writer(PhoneLog)
        .with_ansi(false)
        .without_time()
        .with_max_level(Level::INFO)
        .finish();
    // Only the first call installs it; the app calls this once, first.
    let _ = tracing::subscriber::set_global_default(subscriber);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(Priority::Error, &format!("Shodh panicked: {info}"));
        default_hook(info);
    }));
}
