//! The app's log file. Installed builds have no console, so without it a failure
//! (a Slack connection refused, a model error) would leave nothing to read.
//!
//! Lines go to `<app data>/logs/shodh.log` as well as to stderr. The previous run's
//! log is kept as `shodh.log.1`, and a log that grows past [`MAX_BYTES`] rolls over
//! to `shodh.log.1` too, so the two files together stay bounded. Lines logged before
//! the file is opened (the first moments of start-up) go to stderr only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Size at which the log rolls over to `shodh.log.1`.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// The log's file name inside the logs folder.
pub const FILE_NAME: &str = "shodh.log";
/// The previous log's file name.
pub const PREVIOUS_FILE_NAME: &str = "shodh.log.1";

/// One open log file that rolls over at a size limit.
pub struct RollingFile {
    dir: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
}

impl RollingFile {
    /// Opens a fresh log in `dir`, keeping the previous one as `shodh.log.1`.
    pub fn open(dir: &Path, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = Self::start(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            written: 0,
            max_bytes,
        })
    }

    /// Moves the current log (if any) to `shodh.log.1` and opens an empty one.
    fn start(dir: &Path) -> io::Result<File> {
        let current = dir.join(FILE_NAME);
        if current.exists() {
            fs::rename(&current, dir.join(PREVIOUS_FILE_NAME))?;
        }
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(current)
    }

    /// Writes one formatted log line, rolling over first if it would pass the limit.
    pub fn write_line(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            self.file.flush()?;
            self.file = Self::start(&self.dir)?;
            self.written = 0;
        }
        self.file.write_all(buf)?;
        self.written += buf.len() as u64;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

static FILE: OnceLock<Mutex<Option<RollingFile>>> = OnceLock::new();

fn shared() -> &'static Mutex<Option<RollingFile>> {
    FILE.get_or_init(|| Mutex::new(None))
}

/// Starts writing the log to `dir`. Returns the log's path.
pub fn open(dir: &Path) -> io::Result<PathBuf> {
    let file = RollingFile::open(dir, MAX_BYTES)?;
    *shared().lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    Ok(dir.join(FILE_NAME))
}

/// The writer handed to `tracing_subscriber`: each line goes to stderr and, once
/// [`open`] has run, to the log file. A failed file write never fails logging.
#[derive(Default)]
pub struct Writer;

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        if let Some(file) = shared().lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = file.write_line(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Some(file) = shared().lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap_or_default()
    }

    #[test]
    fn a_new_run_keeps_the_previous_log() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = RollingFile::open(dir.path(), 1024).unwrap();
        first.write_line(b"first run\n").unwrap();
        first.flush().unwrap();
        drop(first);

        let mut second = RollingFile::open(dir.path(), 1024).unwrap();
        second.write_line(b"second run\n").unwrap();
        second.flush().unwrap();

        assert_eq!(read(dir.path(), FILE_NAME), "second run\n");
        assert_eq!(read(dir.path(), PREVIOUS_FILE_NAME), "first run\n");
    }

    #[test]
    fn a_full_log_rolls_over_and_stays_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RollingFile::open(dir.path(), 20).unwrap();
        log.write_line(b"0123456789\n").unwrap(); // 11 bytes
        log.write_line(b"abcdefghij\n").unwrap(); // would reach 22 > 20: rolls first
        log.write_line(b"KLMNOPQRST\n").unwrap(); // rolls again
        log.flush().unwrap();

        assert_eq!(read(dir.path(), FILE_NAME), "KLMNOPQRST\n");
        assert_eq!(read(dir.path(), PREVIOUS_FILE_NAME), "abcdefghij\n");
        let files = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(files, 2, "only the log and the previous log are kept");
    }

    #[test]
    fn a_line_longer_than_the_limit_is_still_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RollingFile::open(dir.path(), 4).unwrap();
        log.write_line(b"a long line\n").unwrap();
        log.flush().unwrap();
        assert_eq!(read(dir.path(), FILE_NAME), "a long line\n");
    }

    #[test]
    fn the_logs_folder_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let mut log = RollingFile::open(&logs, 1024).unwrap();
        log.write_line(b"x\n").unwrap();
        log.flush().unwrap();
        assert_eq!(read(&logs, FILE_NAME), "x\n");
    }
}
