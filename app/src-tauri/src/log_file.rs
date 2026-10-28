//! The app's log file. Installed builds have no console, so without it a failure
//! (a Slack connection refused, a model error) would leave nothing to read.
//!
//! Lines go to `<app data>/logs/shodh.log` as well as to stderr. Each run appends to
//! the log (its first line names the version), and a log that grows past
//! [`MAX_BYTES`] rolls over to `shodh.log.1`, so the two files together stay bounded
//! by size, not by the number of starts: a crash followed by two relaunches still
//! leaves the crash in the log. Lines logged before the file is opened (the first
//! moments of start-up) go to stderr only.
//!
//! The log never loses a line to its own housekeeping: a roll-over that fails (another
//! program holds the file) leaves the log growing and is tried again after
//! [`ROLL_RETRY_LINES`] lines, and a last line cut off by a crash is ended before the
//! next run's first line is written after it.

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
/// Lines written after a failed roll-over before it is tried again.
pub const ROLL_RETRY_LINES: u32 = 1000;

type Rename = fn(&Path, &Path) -> io::Result<()>;

/// One open log file that rolls over at a size limit.
pub struct RollingFile {
    dir: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
    /// How the log is moved to `shodh.log.1` (a failing one in tests).
    rename: Rename,
    /// Lines left before a failed roll-over is tried again.
    retry_roll_in: u32,
    /// A failed roll-over was reported on stderr (once per run).
    roll_failure_reported: bool,
}

fn rename_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

impl RollingFile {
    /// Opens the log in `dir` to append to it; a full one rolls over at the next line.
    pub fn open(dir: &Path, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(dir.join(FILE_NAME))?;
        let mut written = file.metadata()?.len();
        // A crash can cut the last line short: end it, so this run's first line
        // starts a line of its own.
        if written > 0 && !ends_with_newline(&mut file)? {
            file.write_all(b"\n")?;
            written += 1;
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            written,
            max_bytes,
            rename: rename_file,
            retry_roll_in: 0,
            roll_failure_reported: false,
        })
    }

    /// Moves the current log to `shodh.log.1` and opens an empty one. On failure the
    /// current file stays open as it was.
    fn roll(&mut self) -> io::Result<()> {
        let current = self.dir.join(FILE_NAME);
        if current.exists() {
            (self.rename)(&current, &self.dir.join(PREVIOUS_FILE_NAME))?;
        }
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(current)?;
        self.written = 0;
        Ok(())
    }

    /// Writes one formatted log line, rolling over first if it would pass the limit.
    /// A roll-over that fails never costs the line: it is written to the current log,
    /// and the roll-over is tried again [`ROLL_RETRY_LINES`] lines later.
    pub fn write_line(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            if self.retry_roll_in == 0 {
                let _ = self.file.flush();
                if let Err(e) = self.roll() {
                    self.retry_roll_in = ROLL_RETRY_LINES;
                    if !self.roll_failure_reported {
                        self.roll_failure_reported = true;
                        let _ = writeln!(
                            io::stderr(),
                            "The log could not roll over ({e}); it keeps growing and is rolled over later."
                        );
                    }
                }
            } else {
                self.retry_roll_in -= 1;
            }
        }
        self.file.write_all(buf)?;
        self.written += buf.len() as u64;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Whether `file`'s last byte is a newline (`file` is not empty).
fn ends_with_newline(file: &mut File) -> io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
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

    /// The live logs: the log rolled at every start, so a 47-second run had pushed out
    /// everything before it while the 10 MB limit was nowhere near.
    #[test]
    fn a_new_run_appends_to_the_log() {
        let dir = tempfile::tempdir().unwrap();
        for run in ["first run\n", "second run\n", "third run\n"] {
            let mut log = RollingFile::open(dir.path(), 1024).unwrap();
            log.write_line(run.as_bytes()).unwrap();
            log.flush().unwrap();
        }
        assert_eq!(
            read(dir.path(), FILE_NAME),
            "first run\nsecond run\nthird run\n"
        );
        assert!(!dir.path().join(PREVIOUS_FILE_NAME).exists());
    }

    #[test]
    fn a_new_run_counts_what_the_log_already_holds() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = RollingFile::open(dir.path(), 20).unwrap();
        first.write_line(b"0123456789\n").unwrap(); // 11 bytes
        first.flush().unwrap();
        drop(first);

        let mut second = RollingFile::open(dir.path(), 20).unwrap();
        second.write_line(b"abcdefghij\n").unwrap(); // 22 > 20 with the first run's line
        second.flush().unwrap();

        assert_eq!(read(dir.path(), FILE_NAME), "abcdefghij\n");
        assert_eq!(read(dir.path(), PREVIOUS_FILE_NAME), "0123456789\n");
    }

    /// A crash cut the last line short; the next run's first line must not be glued
    /// to it.
    #[test]
    fn a_line_cut_off_by_a_crash_is_ended_before_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), b"complete\ncut off mid-li").unwrap();
        let mut log = RollingFile::open(dir.path(), 1024).unwrap();
        log.write_line(b"next run\n").unwrap();
        log.flush().unwrap();
        assert_eq!(
            read(dir.path(), FILE_NAME),
            "complete\ncut off mid-li\nnext run\n"
        );
        // A log that ends properly gets nothing added.
        drop(log);
        let mut again = RollingFile::open(dir.path(), 1024).unwrap();
        again.write_line(b"third\n").unwrap();
        again.flush().unwrap();
        assert_eq!(
            read(dir.path(), FILE_NAME),
            "complete\ncut off mid-li\nnext run\nthird\n"
        );
    }

    static RENAME_TRIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn held_by_another_program(_from: &Path, _to: &Path) -> io::Result<()> {
        RENAME_TRIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(io::Error::other("the file is in use by another program"))
    }

    /// The roll-over failed (an indexer or antivirus held the file): every later line
    /// failed with it and was lost, and each one tried the rename again.
    #[test]
    fn a_failed_roll_over_loses_no_line_and_is_retried_later() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RollingFile::open(dir.path(), 4).unwrap();
        log.rename = held_by_another_program;
        let tries = || RENAME_TRIES.load(std::sync::atomic::Ordering::SeqCst);
        let mut expected = String::new();
        // Two lines fit; the third tries the roll-over, which fails; the next
        // ROLL_RETRY_LINES lines do not try again.
        for n in 0..(ROLL_RETRY_LINES + 3) {
            let line = format!("{n}\n");
            log.write_line(line.as_bytes()).unwrap();
            expected.push_str(&line);
        }
        assert_eq!(tries(), 1);
        log.write_line(b"again\n").unwrap();
        expected.push_str("again\n");
        assert_eq!(tries(), 2, "tried again after ROLL_RETRY_LINES lines");
        log.flush().unwrap();
        assert_eq!(read(dir.path(), FILE_NAME), expected, "every line kept");
        assert!(!dir.path().join(PREVIOUS_FILE_NAME).exists());

        // Once the file is free again, the next try rolls over.
        log.rename = rename_file;
        for _ in 0..ROLL_RETRY_LINES {
            log.write_line(b"x\n").unwrap();
        }
        log.write_line(b"rolled\n").unwrap();
        log.flush().unwrap();
        assert_eq!(read(dir.path(), FILE_NAME), "rolled\n");
        assert!(read(dir.path(), PREVIOUS_FILE_NAME).ends_with("x\n"));
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
