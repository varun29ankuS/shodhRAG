//! Files the app must not lose to a crash or a power cut.
//!
//! [`write_atomic`] writes a temporary file, flushes it to disk, renames it
//! over the old one and (where the platform allows) flushes the folder, so
//! a kill at any point leaves either the previous contents or the new ones.
//!
//! The JSON stores (conversations, calendar, settings) also keep the
//! previous good version as `<name>.bak`: [`write_with_backup`] copies the
//! file there, flushed, before replacing it, and [`read_with_backup`] reads
//! the backup when the file itself is damaged or missing. A damaged file is
//! never overwritten: the next save first moves it aside as
//! `<name>.damaged-<UTC time>`, where the user (or support) can still find
//! it. Locking is the caller's: each store serialises its own
//! read-modify-write.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Where a store's contents came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The file itself.
    Main,
    /// Neither the file nor its backup exists yet.
    Missing,
    /// The file was damaged (or missing) and its backup was read.
    /// `problem` says what was wrong with the file.
    Backup { problem: String },
    /// The file and its backup were both unusable and the caller chose to
    /// go on without them (settings fall back to defaults). Saving moves
    /// both aside, never over them.
    Damaged { problem: String },
}

/// A store's contents and where they came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded<T> {
    pub value: T,
    pub origin: Origin,
}

/// Why a store could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// The file could not be opened or read (permissions, a locked file);
    /// nothing says it is damaged.
    Io { path: PathBuf, message: String },
    /// The file is damaged (or missing while a backup exists) and there is
    /// no usable backup either.
    Damaged {
        path: PathBuf,
        problem: String,
        backup_problem: String,
    },
}

impl std::fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadFailure::Io { path, message } => {
                write!(f, "{} could not be read: {message}", path.display())
            }
            ReadFailure::Damaged {
                path,
                problem,
                backup_problem,
            } => write!(
                f,
                "{} could not be read ({problem}) and its backup could not be used either ({backup_problem})",
                path.display()
            ),
        }
    }
}

/// `<path>` with `suffix` appended to its file name (`a.json` → `a.json.bak`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// The backup of `path`: `<name>.bak` next to it.
pub fn backup_path(path: &Path) -> PathBuf {
    with_suffix(path, ".bak")
}

/// What reading one file gave.
enum Attempt<T> {
    Parsed(T),
    Missing,
    Damaged(String),
    Unreadable(String),
}

fn attempt<T>(path: &Path, parse: &impl Fn(&str) -> Result<T, String>) -> Attempt<T> {
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => match parse(&text) {
                Ok(value) => Attempt::Parsed(value),
                Err(problem) => Attempt::Damaged(if text.trim().is_empty() {
                    "the file is empty".to_string()
                } else {
                    problem
                }),
            },
            Err(_) => Attempt::Damaged("the file is not valid text".to_string()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Attempt::Missing,
        Err(e) => Attempt::Unreadable(e.to_string()),
    }
}

/// Read the store at `path` with `parse`, falling back to its backup when
/// the file is damaged, or missing while a backup exists. A store that was
/// never saved (neither exists) is `empty()`.
pub fn read_with_backup<T>(
    path: &Path,
    parse: impl Fn(&str) -> Result<T, String>,
    empty: impl FnOnce() -> T,
) -> Result<Loaded<T>, ReadFailure> {
    let problem = match attempt(path, &parse) {
        Attempt::Parsed(value) => {
            return Ok(Loaded {
                value,
                origin: Origin::Main,
            })
        }
        Attempt::Unreadable(message) => {
            return Err(ReadFailure::Io {
                path: path.to_path_buf(),
                message,
            })
        }
        Attempt::Missing => None,
        Attempt::Damaged(problem) => Some(problem),
    };
    let backup = backup_path(path);
    let backup_problem = match attempt(&backup, &parse) {
        Attempt::Parsed(value) => {
            return Ok(Loaded {
                value,
                origin: Origin::Backup {
                    problem: problem.unwrap_or_else(|| "the file was missing".to_string()),
                },
            })
        }
        Attempt::Missing => match problem {
            // A store that was never saved.
            None => {
                return Ok(Loaded {
                    value: empty(),
                    origin: Origin::Missing,
                })
            }
            Some(_) => "there is no backup".to_string(),
        },
        Attempt::Damaged(p) | Attempt::Unreadable(p) => p,
    };
    Err(ReadFailure::Damaged {
        path: path.to_path_buf(),
        problem: problem.unwrap_or_else(|| "the file is missing".to_string()),
        backup_problem,
    })
}

/// Write `bytes` to `path` durably: a temporary file is written, flushed to
/// disk and renamed over `path`, then the folder is flushed where the
/// platform allows it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = parent_dir(path);
    fs::create_dir_all(&dir)?;
    let tmp = with_suffix(path, ".tmp");
    let written = (|| {
        write_synced(&tmp, bytes)?;
        fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    sync_dir(&dir)
}

/// Replace the store at `path` with `bytes`, keeping what was there: the
/// file it was read from (`origin`) becomes `<name>.bak`; a damaged file
/// (the store was read from its backup) is moved aside as
/// `<name>.damaged-<UTC time>` and the backup is kept; when the backup was
/// unusable too ([`Origin::Damaged`]) it is moved aside as
/// `<name>.bak.damaged-<UTC time>`, so neither is ever written over.
pub fn write_with_backup(path: &Path, bytes: &[u8], origin: &Origin) -> io::Result<()> {
    let dir = parent_dir(path);
    fs::create_dir_all(&dir)?;
    match origin {
        Origin::Main => {
            let backup = backup_path(path);
            let tmp = with_suffix(&backup, ".tmp");
            let copied = (|| {
                fs::copy(path, &tmp)?;
                fs::OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
                fs::rename(&tmp, &backup)
            })();
            match copied {
                Ok(()) => {}
                // Removed since it was read: nothing to keep.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let _ = fs::remove_file(&tmp);
                }
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
            }
        }
        Origin::Backup { .. } => {
            if path.exists() {
                fs::rename(path, damaged_path(path))?;
            }
        }
        Origin::Damaged { .. } => {
            let backup = backup_path(path);
            if backup.exists() {
                fs::rename(&backup, damaged_path(&backup))?;
            }
            if path.exists() {
                fs::rename(path, damaged_path(path))?;
            }
        }
        Origin::Missing => {}
    }
    write_atomic(path, bytes)
}

/// A free `<name>.damaged-<UTC time>` name next to `path`. The time has no
/// `:`, which Windows file names cannot hold.
pub fn damaged_path(path: &Path) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let first = with_suffix(path, &format!(".damaged-{stamp}"));
    if !first.exists() {
        return first;
    }
    (1u32..)
        .map(|n| with_suffix(path, &format!(".damaged-{stamp}-{n}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flush a folder's entries (the rename) to disk. Windows cannot open a
/// folder as a file; NTFS journals the rename itself.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<serde_json::Value, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    fn read(path: &Path) -> Result<Loaded<serde_json::Value>, ReadFailure> {
        read_with_backup(path, parse, || serde_json::json!("empty"))
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Save `text` the way the stores do: read, then write keeping a backup.
    fn save(path: &Path, text: &str) {
        let origin = read(path).unwrap().origin;
        write_with_backup(path, text.as_bytes(), &origin).unwrap();
    }

    #[test]
    fn each_save_keeps_the_previous_version_as_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        assert_eq!(
            read(&path).unwrap(),
            Loaded {
                value: serde_json::json!("empty"),
                origin: Origin::Missing
            }
        );
        save(&path, "1");
        assert!(!backup_path(&path).exists(), "nothing to keep yet");
        save(&path, "2");
        save(&path, "3");
        assert_eq!(fs::read_to_string(&path).unwrap(), "3");
        assert_eq!(fs::read_to_string(backup_path(&path)).unwrap(), "2");
        assert_eq!(read(&path).unwrap().origin, Origin::Main);
        // No temporary file is left behind.
        assert_eq!(names(dir.path()), ["store.json", "store.json.bak"]);
    }

    #[test]
    fn a_truncated_file_reads_its_backup_and_is_kept_aside_at_the_next_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        save(&path, r#"{"n": 1}"#);
        save(&path, r#"{"n": 2}"#);
        // A power cut left the file half written.
        fs::write(&path, r#"{"n": "#).unwrap();
        let loaded = read(&path).unwrap();
        assert_eq!(loaded.value, serde_json::json!({"n": 1}));
        assert!(matches!(loaded.origin, Origin::Backup { .. }));
        // Reading changes nothing on disk.
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"n": "#);

        save(&path, r#"{"n": 3}"#);
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"n": 3}"#);
        assert_eq!(
            fs::read_to_string(backup_path(&path)).unwrap(),
            r#"{"n": 1}"#,
            "the good backup is kept, not replaced by the damaged file"
        );
        let damaged: Vec<String> = names(dir.path())
            .into_iter()
            .filter(|n| n.starts_with("store.json.damaged-"))
            .collect();
        assert_eq!(damaged.len(), 1);
        assert!(!damaged[0].contains(':'), "a valid Windows file name");
        assert_eq!(
            fs::read_to_string(dir.path().join(&damaged[0])).unwrap(),
            r#"{"n": "#
        );
        assert_eq!(read(&path).unwrap().origin, Origin::Main);
    }

    #[test]
    fn an_empty_file_without_a_backup_is_an_error_not_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        fs::write(&path, "").unwrap();
        match read(&path) {
            Err(ReadFailure::Damaged {
                problem,
                backup_problem,
                ..
            }) => {
                assert_eq!(problem, "the file is empty");
                assert_eq!(backup_problem, "there is no backup");
            }
            other => panic!("expected a damaged store, got {other:?}"),
        }
        let message = read(&path).unwrap_err().to_string();
        assert!(message.contains("store.json"), "{message}");
        // Both damaged.
        fs::write(backup_path(&path), "{").unwrap();
        assert!(matches!(read(&path), Err(ReadFailure::Damaged { .. })));
    }

    #[test]
    fn a_damaged_backup_is_moved_aside_too_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        fs::write(&path, "{").unwrap();
        fs::write(backup_path(&path), "[").unwrap();
        let problem = match read(&path) {
            Err(ReadFailure::Damaged { problem, .. }) => problem,
            other => panic!("expected a damaged store, got {other:?}"),
        };
        // The caller goes on with defaults and saves.
        write_with_backup(&path, b"1", &Origin::Damaged { problem }).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "1");
        let all = names(dir.path());
        let kept = |prefix: &str, text: &str| {
            all.iter()
                .filter(|n| n.starts_with(prefix))
                .any(|n| fs::read_to_string(dir.path().join(n)).unwrap() == text)
        };
        assert!(kept("store.json.damaged-", "{"), "{all:?}");
        assert!(kept("store.json.bak.damaged-", "["), "{all:?}");
        assert!(!backup_path(&path).exists());
        // The next save starts a new backup from the good file.
        save(&path, "2");
        assert_eq!(fs::read_to_string(backup_path(&path)).unwrap(), "1");
    }

    #[test]
    fn a_missing_file_with_a_backup_reads_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        fs::write(backup_path(&path), "7").unwrap();
        let loaded = read(&path).unwrap();
        assert_eq!(loaded.value, serde_json::json!(7));
        assert_eq!(
            loaded.origin,
            Origin::Backup {
                problem: "the file was missing".to_string()
            }
        );
    }

    #[test]
    fn write_atomic_replaces_the_file_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("value.json");
        write_atomic(&path, b"a").unwrap();
        write_atomic(&path, b"b").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "b");
        assert_eq!(names(&dir.path().join("nested")), ["value.json"]);
    }
}
