//! The app's log, in Android's logcat under the tag `Shodh` (what `adb
//! logcat -s Shodh` shows, and what CI's launch test reads). Rust's own
//! output goes nowhere on Android, so each `tracing` line, and a panic, is
//! written there.

use std::ffi::CString;
use std::io::{self, Write};
use std::os::raw::{c_char, c_int};

use tracing::{Level, Metadata};
use tracing_subscriber::fmt::MakeWriter;

/// The logcat tag of every line.
const TAG: &[u8] = b"Shodh\0";

// android/log.h priorities.
const DEBUG: c_int = 3;
const INFO: c_int = 4;
const WARN: c_int = 5;
const ERROR: c_int = 6;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// Write one line to logcat.
pub fn write(priority: c_int, text: &str) {
    let text = text.trim_end();
    if text.is_empty() {
        return;
    }
    // A NUL inside would end the line early; there is none in our messages.
    let Ok(line) = CString::new(text.replace('\0', " ")) else {
        return;
    };
    // SAFETY: both pointers are NUL-terminated strings that outlive the call.
    unsafe {
        __android_log_write(priority, TAG.as_ptr().cast(), line.as_ptr());
    }
}

/// One formatted `tracing` event, written to logcat when it is dropped.
pub struct Line {
    priority: c_int,
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

/// The `tracing` writer: a logcat line per event, at the event's priority.
pub struct Logcat;

impl<'a> MakeWriter<'a> for Logcat {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line {
            priority: INFO,
            buffer: Vec::new(),
        }
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Line {
        let priority = match *meta.level() {
            Level::ERROR => ERROR,
            Level::WARN => WARN,
            Level::INFO => INFO,
            Level::DEBUG | Level::TRACE => DEBUG,
        };
        Line {
            priority,
            buffer: Vec::new(),
        }
    }
}

/// Send the app's `tracing` output (info and above) and its panics to logcat.
pub fn init() {
    let subscriber = tracing_subscriber::fmt()
        .with_writer(Logcat)
        .with_ansi(false)
        .without_time()
        .with_max_level(Level::INFO)
        .finish();
    // Only the first call installs it; the app calls this once, first.
    let _ = tracing::subscriber::set_global_default(subscriber);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(ERROR, &format!("Shodh panicked: {info}"));
        default_hook(info);
    }));
}
