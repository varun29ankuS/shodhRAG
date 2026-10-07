//! The text index's directory: Tantivy's [`MmapDirectory`] with one Windows
//! behaviour handled.
//!
//! Every commit replaces `meta.json` (and `.managed.json`) by writing a temporary
//! file and renaming it over the old one. On Windows that rename fails with
//! "Access is denied" (os error 5) or a sharing violation (os error 32) while any
//! other process has the old file open without delete sharing: the antivirus
//! scanner and the search indexer open every file written under the user's
//! profile, for a few milliseconds each. Tantivy reports that as a failed commit,
//! so a document failed to index for no reason of its own (seen in tests under
//! load as "Tantivy commit failed: ... Access is denied", about once in twenty
//! full runs).
//!
//! The replacement is atomic and idempotent (the same bytes to the same path), so
//! it is attempted again while the error is one of those two, for at most
//! [`REPLACE_PATIENCE`]. Any other error, or the same one lasting longer, is
//! returned as before.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tantivy::directory::error::{DeleteError, LockError, OpenReadError, OpenWriteError};
use tantivy::directory::{
    Directory, DirectoryLock, FileHandle, Lock, MmapDirectory, WatchCallback, WatchHandle, WritePtr,
};

/// Longest a replacement waits for another process to close the file.
pub const REPLACE_PATIENCE: Duration = Duration::from_secs(2);
/// Pause between attempts (doubles up to [`MAX_PAUSE`]).
const FIRST_PAUSE: Duration = Duration::from_millis(5);
const MAX_PAUSE: Duration = Duration::from_millis(100);

/// `ERROR_ACCESS_DENIED` and `ERROR_SHARING_VIOLATION`.
fn held_by_another_process(error: &io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(5) | Some(32))
}

/// Runs `replace` until it succeeds, fails for another reason, or the file stays
/// held for longer than `patience`.
pub fn replace_patiently(
    patience: Duration,
    is_held: impl Fn(&io::Error) -> bool,
    mut replace: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let started = Instant::now();
    let mut pause = FIRST_PAUSE;
    let mut attempts = 1u32;
    loop {
        match replace() {
            Ok(()) => {
                if attempts > 1 {
                    tracing::debug!(attempts, "text index file replaced after it was released");
                }
                return Ok(());
            }
            Err(error) if is_held(&error) && started.elapsed() < patience => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(MAX_PAUSE);
                attempts += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// [`MmapDirectory`] whose atomic writes wait out another process holding the file.
#[derive(Clone, Debug)]
pub struct IndexDirectory {
    inner: MmapDirectory,
}

impl IndexDirectory {
    pub fn open(path: &Path) -> tantivy::Result<Self> {
        Ok(Self {
            inner: MmapDirectory::open(path)?,
        })
    }
}

impl Directory for IndexDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        self.inner.get_file_handle(path)
    }

    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        self.inner.delete(path)
    }

    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        self.inner.exists(path)
    }

    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        self.inner.open_write(path)
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        self.inner.atomic_read(path)
    }

    fn atomic_write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        replace_patiently(REPLACE_PATIENCE, held_by_another_process, || {
            self.inner.atomic_write(path, data)
        })
    }

    fn sync_directory(&self) -> io::Result<()> {
        self.inner.sync_directory()
    }

    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        self.inner.acquire_lock(lock)
    }

    fn watch(&self, watch_callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        self.inner.watch(watch_callback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn denied() -> io::Error {
        io::Error::from_raw_os_error(5)
    }

    fn always_held(_: &io::Error) -> bool {
        true
    }

    #[test]
    fn a_replacement_waits_until_the_file_is_released() {
        let attempts = Cell::new(0);
        let result = replace_patiently(Duration::from_secs(5), always_held, || {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 4 {
                Err(denied())
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(attempts.get(), 4);
    }

    #[test]
    fn other_errors_and_a_file_held_too_long_are_returned() {
        let attempts = Cell::new(0);
        let other = replace_patiently(
            Duration::from_secs(5),
            |_| false,
            || {
                attempts.set(attempts.get() + 1);
                Err(io::Error::new(io::ErrorKind::Other, "disk full"))
            },
        );
        assert_eq!(other.unwrap_err().to_string(), "disk full");
        assert_eq!(attempts.get(), 1);

        let started = Instant::now();
        let held = replace_patiently(Duration::from_millis(50), always_held, || Err(denied()));
        assert_eq!(held.unwrap_err().raw_os_error(), Some(5));
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn only_access_denied_and_sharing_violations_count_as_held_on_windows() {
        assert_eq!(held_by_another_process(&denied()), cfg!(windows));
        assert_eq!(
            held_by_another_process(&io::Error::from_raw_os_error(32)),
            cfg!(windows)
        );
        assert!(!held_by_another_process(&io::Error::from_raw_os_error(2)));
    }

    /// The real case: on Windows, `meta.json` held open by another handle that
    /// does not share delete (as a scanner opens it) blocks the replacement until
    /// the handle closes.
    #[cfg(windows)]
    #[test]
    fn the_index_commits_while_another_handle_briefly_holds_its_metadata() {
        use std::os::windows::fs::OpenOptionsExt;
        use tantivy::schema::{Schema, TEXT};

        let dir = tempfile::tempdir().unwrap();
        let mut schema = Schema::builder();
        let text = schema.add_text_field("text", TEXT);
        let index = tantivy::Index::create(
            IndexDirectory::open(dir.path()).unwrap(),
            schema.build(),
            tantivy::IndexSettings::default(),
        )
        .unwrap();
        let mut writer: tantivy::IndexWriter = index.writer(15_000_000).unwrap();
        writer.add_document(tantivy::doc!(text => "one")).unwrap();

        // FILE_SHARE_READ only: no FILE_SHARE_DELETE, so the rename over it fails
        // until this handle is closed.
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(dir.path().join("meta.json"))
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(holder);
        });
        writer
            .commit()
            .expect("the commit waits for the handle to close");
        release.join().unwrap();
        let reader = index.reader().unwrap();
        assert_eq!(reader.searcher().num_docs(), 1);
    }
}
