//! File-system rules shared by the tools that create files and folders
//! (`export_document`, `create_folder`, `download_file`, `list_directory`):
//! safe names, no-overwrite creation, and "is this inside an indexed
//! source folder" checks that survive case, separators, `\\?\` prefixes and
//! symlinks.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

use shodh_rag::harness::tools::sources::{load_sources, SourceKind};
use shodh_rag::harness::tools::ToolError;
use shodh_rag::RAGEngine;

use super::sources::strip_verbatim;

/// Longest file or folder name produced.
pub const MAX_NAME_CHARS: usize = 120;
/// Suffixes tried before giving up on a free name.
const MAX_SUFFIX: u32 = 999;

const RESERVED_WINDOWS_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `name` is a reserved Windows device name (with any extension).
pub fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim();
    RESERVED_WINDOWS_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(stem))
}

/// A file or folder name safe on Windows, macOS and Linux: path separators,
/// reserved and control characters become `_`, trailing dots and spaces are
/// removed, reserved device names are prefixed, and the length is capped.
/// Returns `fallback` when nothing usable is left.
pub fn sanitize_name(raw: &str, fallback: &str) -> String {
    let mut name: String = raw
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    name = name.chars().take(MAX_NAME_CHARS).collect();
    let trimmed = name.trim_end_matches(['.', ' ']).trim_start();
    let mut name = trimmed.to_string();
    if name.is_empty() || name.chars().all(|c| c == '.' || c == '_') {
        name = fallback.to_string();
    }
    if is_reserved_windows_name(&name) {
        name.insert(0, '_');
    }
    name
}

fn candidate(dir: &Path, stem: &str, ext: &str, n: u32) -> PathBuf {
    let base = if n == 1 {
        stem.to_string()
    } else {
        format!("{stem} ({n})")
    };
    if ext.is_empty() {
        dir.join(base)
    } else {
        dir.join(format!("{base}.{ext}"))
    }
}

/// The first `stem.ext`, `stem (2).ext`, … that does not exist yet.
pub fn next_free_path(dir: &Path, stem: &str, ext: &str) -> io::Result<PathBuf> {
    (1..=MAX_SUFFIX)
        .map(|n| candidate(dir, stem, ext, n))
        .find(|p| !p.exists())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("no free name for {stem} in {}", dir.display()),
            )
        })
}

/// Create `stem.ext` (or the first free suffixed name) without ever
/// replacing an existing file: each attempt is an exclusive create, so a
/// file that appears concurrently is skipped, not overwritten.
pub fn create_unique(dir: &Path, stem: &str, ext: &str) -> io::Result<(PathBuf, File)> {
    for n in 1..=MAX_SUFFIX {
        let path = candidate(dir, stem, ext, n);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("no free name for {stem} in {}", dir.display()),
    ))
}

/// Create exactly `path`, failing if anything exists there.
pub fn create_exact(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// A comparable form of a path: canonical when it exists (resolving
/// symlinks and `..`), without the Windows verbatim prefix, with forward
/// slashes, no trailing slash, and lower-cased on Windows.
pub fn comparable(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path)
        .map(|p| strip_verbatim(&p))
        .unwrap_or_else(|_| path.to_path_buf());
    let mut text = resolved.display().to_string().replace('\\', "/");
    while text.len() > 1 && text.ends_with('/') && !text.ends_with(":/") {
        text.pop();
    }
    if cfg!(windows) {
        text = text.to_lowercase();
    }
    text
}

/// Whether `path` is `root` or inside it.
pub fn is_within(path: &Path, root: &Path) -> bool {
    let path = comparable(path);
    let root = comparable(root);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('/') || root.ends_with('/'))
}

/// An indexed folder source: the only places file-creating tools may write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRoot {
    pub source_id: String,
    pub folder: PathBuf,
}

/// The indexed folder sources.
#[async_trait::async_trait]
pub trait SourceRoots: Send + Sync {
    async fn roots(&self) -> Result<Vec<SourceRoot>, ToolError>;

    async fn folders(&self) -> Result<Vec<PathBuf>, ToolError> {
        Ok(self.roots().await?.into_iter().map(|r| r.folder).collect())
    }
}

/// Source folders derived from the index (production).
pub struct IndexedRoots {
    pub rag: std::sync::Arc<tokio::sync::RwLock<RAGEngine>>,
}

#[async_trait::async_trait]
impl SourceRoots for IndexedRoots {
    async fn roots(&self) -> Result<Vec<SourceRoot>, ToolError> {
        let engine = self.rag.read().await;
        let sources = load_sources(&engine).await?;
        Ok(sources
            .into_iter()
            .filter(|s| s.kind == SourceKind::Folder)
            .filter_map(|s| {
                s.folder.map(|folder| SourceRoot {
                    source_id: s.source_id,
                    folder: PathBuf::from(folder),
                })
            })
            .collect())
    }
}

/// The indexed root that contains `path`, if any.
pub fn containing_root<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots.iter().find(|root| is_within(path, root))
}

/// An absolute path argument without `..` components.
pub fn absolute_arg(tool: &str, raw: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(raw.trim());
    if !path.is_absolute() {
        return Err(super::invalid(tool, "the path must be absolute"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ToolError::Forbidden(
            "Paths containing '..' are not allowed".to_string(),
        ));
    }
    Ok(path.to_path_buf())
}

/// An existing folder, canonicalized (symlinks resolved).
pub fn existing_folder(tool: &str, raw: &str) -> Result<PathBuf, ToolError> {
    let path = absolute_arg(tool, raw)?;
    if !path.is_dir() {
        return Err(ToolError::NotFound(format!(
            "{raw} is not an existing folder"
        )));
    }
    std::fs::canonicalize(&path)
        .map(|p| strip_verbatim(&p))
        .map_err(|e| ToolError::Unavailable(format!("Cannot open {raw}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(
            sanitize_name("Q3 report: draft/v2?", "x"),
            "Q3 report_ draft_v2_"
        );
        assert_eq!(sanitize_name("  notes.  ", "x"), "notes");
        assert_eq!(sanitize_name("...", "export"), "export");
        assert_eq!(sanitize_name("", "export"), "export");
        assert_eq!(sanitize_name("CON", "x"), "_CON");
        assert_eq!(sanitize_name("lpt1.txt", "x"), "_lpt1.txt");
        assert_eq!(sanitize_name("a\u{0}b\nc", "x"), "a_b_c");
        assert_eq!(
            sanitize_name(&"y".repeat(400), "x").chars().count(),
            MAX_NAME_CHARS
        );
        assert!(!is_reserved_windows_name("console"));
    }

    #[test]
    fn creation_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let (first, _) = create_unique(dir.path(), "Report", "md").unwrap();
        let (second, _) = create_unique(dir.path(), "Report", "md").unwrap();
        assert_eq!(first.file_name().unwrap(), "Report.md");
        assert_eq!(second.file_name().unwrap(), "Report (2).md");
        assert_eq!(
            next_free_path(dir.path(), "Report", "md")
                .unwrap()
                .file_name()
                .unwrap(),
            "Report (3).md"
        );
        assert_eq!(
            create_exact(&first).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let (folder, _) = create_unique(dir.path(), "Report", "").unwrap();
        assert_eq!(
            folder.file_name().unwrap(),
            "Report",
            "a folder name is free"
        );
    }

    #[test]
    fn containment_ignores_case_separators_and_prefix_tricks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Docs");
        std::fs::create_dir_all(root.join("Sub")).unwrap();
        std::fs::create_dir_all(dir.path().join("Docs2")).unwrap();
        assert!(is_within(&root.join("Sub"), &root));
        assert!(is_within(&root, &root));
        assert!(
            !is_within(&dir.path().join("Docs2"), &root),
            "sibling with a common prefix"
        );
        assert!(is_within(&root.join("Sub").join("..").join("Sub"), &root));
        assert!(!is_within(&root.join("..").join("Docs2"), &root));
        if cfg!(windows) {
            let upper = PathBuf::from(root.display().to_string().to_uppercase());
            assert!(is_within(&upper.join("SUB"), &root));
        }
        let roots = vec![root.clone()];
        assert_eq!(containing_root(&root.join("Sub"), &roots), Some(&root));
        assert!(absolute_arg("t", "relative/x").is_err());
        assert!(matches!(
            absolute_arg("t", &root.join("..").display().to_string()),
            Err(ToolError::Forbidden(_))
        ));
    }
}
