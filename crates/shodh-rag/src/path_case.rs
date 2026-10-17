//! The paths of indexed files as the file system spells them, for display.
//!
//! An indexed file's identity is lowercased on Windows
//! ([`normalize_source_path`]). Documents indexed before their spelling was
//! stored with their chunks ([`crate::rag_engine::DISPLAY_PATH_KEY`]) get it
//! here, where paths are shown (the Library's list of indexed files, search
//! passages). Asking the file system once per file would cost a round trip
//! per document on every listing (seconds for a large library on a network
//! share, a timeout per file when the share is offline), so each folder is
//! resolved once per process and the file's name is taken in its original
//! case from what the index keeps (`filename`, the title), falling back to
//! resolving the file itself only when the index keeps no matching name.
//! Results, failures included, are cached for the process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::rag_engine::{canonical_path, normalize_source_path};

/// How a path is resolved on disk: `std::fs::canonicalize`, or a stand-in in
/// tests.
type Canonicalize = dyn Fn(&Path) -> std::io::Result<PathBuf> + Send + Sync;

/// Resolves and caches the on-disk spelling of folders and files, keyed by
/// identity ([`normalize_source_path`]).
pub struct PathSpellings {
    canonicalize: Box<Canonicalize>,
    /// Folder identity to its spelling; `None` when it could not be resolved
    /// or resolved to another identity.
    folders: Mutex<HashMap<String, Option<PathBuf>>>,
    /// File identity to its spelling, for files whose name the index does
    /// not keep in a matching case.
    files: Mutex<HashMap<String, Option<String>>>,
}

impl PathSpellings {
    /// Resolving with the file system.
    pub fn new() -> Self {
        Self::with_canonicalize(|path| std::fs::canonicalize(path))
    }

    /// Resolving with `canonicalize` (tests count and fake the I/O).
    pub fn with_canonicalize(
        canonicalize: impl Fn(&Path) -> std::io::Result<PathBuf> + Send + Sync + 'static,
    ) -> Self {
        Self {
            canonicalize: Box::new(canonicalize),
            folders: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
        }
    }

    /// The resolver shared by everything that shows indexed paths, for the
    /// process's lifetime.
    pub fn shared() -> &'static PathSpellings {
        static SHARED: OnceLock<PathSpellings> = OnceLock::new();
        SHARED.get_or_init(PathSpellings::new)
    }

    /// The path of the indexed file `source` (its identity) as the file
    /// system spells it, in its [`canonical_path`] form. `names` are spellings
    /// of the file's name the index keeps (its `filename`, its title with the
    /// extension); the first that names the same file in another case is
    /// used under the folder's spelling. Elsewhere than Windows paths keep
    /// their case, so `source` is its own spelling and nothing is read.
    /// `None` when the folder (or, without a matching name, the file) cannot
    /// be resolved, or resolves to another identity (a short `8.3` name
    /// expanded, a mapped drive or a junction followed).
    pub fn spelling(&self, source: &str, names: &[&str]) -> Option<String> {
        if !cfg!(windows) {
            return Some(canonical_path(Path::new(source)).display().to_string());
        }
        self.resolve(source, names)
    }

    fn resolve(&self, source: &str, names: &[&str]) -> Option<String> {
        if source.is_empty() || source.contains("://") {
            return None;
        }
        let path = canonical_path(Path::new(source));
        let name = path.file_name()?.to_string_lossy().into_owned();
        let folder = self.folder(path.parent()?)?;
        let key = name.to_lowercase();
        if let Some(original) = names
            .iter()
            .map(|n| n.trim())
            .find(|n| !n.is_empty() && n.to_lowercase() == key)
        {
            return Some(folder.join(original).display().to_string());
        }
        self.file(&path)
    }

    /// The spelling of `folder`, resolved once per process.
    fn folder(&self, folder: &Path) -> Option<PathBuf> {
        let key = normalize_source_path(folder);
        if let Some(cached) = lock(&self.folders).get(&key) {
            return cached.clone();
        }
        // Resolved outside the lock: a slow share must not hold up the cache.
        let resolved = self.same_identity(folder);
        lock(&self.folders).entry(key).or_insert(resolved).clone()
    }

    /// The spelling of the file at `path` when the index keeps no matching
    /// name, resolved once per process.
    fn file(&self, path: &Path) -> Option<String> {
        let key = normalize_source_path(path);
        if let Some(cached) = lock(&self.files).get(&key) {
            return cached.clone();
        }
        let resolved = self.same_identity(path).map(|p| p.display().to_string());
        lock(&self.files).entry(key).or_insert(resolved).clone()
    }

    /// `path` resolved on disk, when the result has `path`'s identity: only
    /// case and separators may differ.
    fn same_identity(&self, path: &Path) -> Option<PathBuf> {
        let resolved = canonical_path(&(self.canonicalize)(path).ok()?);
        (normalize_source_path(&resolved) == normalize_source_path(path)).then_some(resolved)
    }
}

impl Default for PathSpellings {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A resolver over a fake disk: `disk` maps lowercased paths to their
    /// spelling; every call is counted.
    fn fake(disk: &[(&str, &str)]) -> (PathSpellings, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let disk: HashMap<String, PathBuf> = disk
            .iter()
            .map(|(k, v)| (k.to_lowercase(), PathBuf::from(v)))
            .collect();
        let resolver = PathSpellings::with_canonicalize(move |path| {
            counted.fetch_add(1, Ordering::SeqCst);
            disk.get(&path.display().to_string().to_lowercase())
                .cloned()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        (resolver, calls)
    }

    #[cfg(windows)]
    #[test]
    fn many_files_in_one_folder_resolve_the_folder_once() {
        let (resolver, calls) = fake(&[("c:\\research\\papers", "\\\\?\\C:\\Research\\papers")]);
        for i in 0..50 {
            let source = format!("c:/research/papers/paper {i}.pdf");
            let name = format!("Paper {i}.pdf");
            assert_eq!(
                resolver.resolve(&source, &[name.as_str()]).as_deref(),
                Some(format!("C:\\Research\\papers\\Paper {i}.pdf").as_str())
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(windows)]
    #[test]
    fn the_name_is_taken_from_a_matching_title_and_never_another_name() {
        let (resolver, calls) = fake(&[
            ("c:\\research\\papers", "C:\\Research\\papers"),
            (
                "c:\\research\\papers\\spann.pdf",
                "C:\\Research\\papers\\SPANN.pdf",
            ),
        ]);
        // The title with the extension names the file.
        assert_eq!(
            resolver
                .resolve("c:/research/papers/spann.pdf", &["", "SPANN.pdf"])
                .as_deref(),
            Some("C:\\Research\\papers\\SPANN.pdf")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the folder only");
        // A name of another file (a PDF's own title): the file is resolved.
        assert_eq!(
            resolver
                .resolve(
                    "c:/research/papers/spann.pdf",
                    &["Highly-efficient Search.pdf"]
                )
                .as_deref(),
            Some("C:\\Research\\papers\\SPANN.pdf")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Then cached.
        resolver.resolve("c:/research/papers/spann.pdf", &[]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_that_cannot_be_resolved_is_not_asked_again() {
        let (resolver, calls) = fake(&[]);
        for i in 0..10 {
            let source = format!("z:/offline/share/{i}.pdf");
            assert_eq!(resolver.resolve(&source, &["X.pdf"]), None);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_resolving_to_another_identity_is_not_used() {
        // A short 8.3 name expanded: not the same identity.
        let (resolver, _) = fake(&[(
            "c:\\users\\varuns~1\\papers",
            "C:\\Users\\Varun Sharma\\papers",
        )]);
        assert_eq!(
            resolver.resolve("c:/users/varuns~1/papers/a.pdf", &["A.pdf"]),
            None
        );
        assert_eq!(resolver.resolve("calendar://task/1", &[]), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_a_source_is_its_own_spelling_without_io() {
        let (resolver, calls) = fake(&[]);
        assert_eq!(
            resolver.spelling("/home/v/Papers/./A.pdf", &[]).as_deref(),
            Some("/home/v/Papers/A.pdf")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
