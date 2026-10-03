//! Library file browser commands.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::agent_tools::{list_directory_in, IndexedRoots, Listing, SourceRoot, SourceRoots};
use crate::rag_commands::RagState;

/// Entries one listing returns at most.
const MAX_ENTRIES: usize = 5_000;

/// List `path` (absolute, inside one of `roots`), at most `limit` entries.
fn list_within(
    path: &Path,
    recursive: bool,
    limit: usize,
    roots: &[SourceRoot],
) -> Result<Listing, String> {
    if !path.is_absolute() {
        return Err("The folder path must be absolute".to_string());
    }
    list_directory_in(path, recursive, limit, roots).map_err(|e| e.to_string())
}

/// Files and folders under `path`, which must be inside an indexed source
/// folder (checked after resolving symlinks). Paths are spelled as on disk;
/// folders come first; recursive listings do not follow symlinked folders.
/// `truncated` says more than [`MAX_ENTRIES`] entries existed, so the UI can
/// say the list is incomplete instead of presenting it as everything.
#[tauri::command]
pub async fn list_directory(
    path: String,
    recursive: bool,
    rag: State<'_, RagState>,
) -> Result<Listing, String> {
    let path = PathBuf::from(path.trim());
    let roots = IndexedRoots {
        rag: rag.rag.clone(),
    }
    .roots()
    .await
    .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || list_within(&path, recursive, MAX_ENTRIES, &roots))
        .await
        .map_err(|e| format!("Listing was interrupted: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listings_report_truncation_and_stay_inside_sources() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let folder = PathBuf::from(
            canonical
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_string(),
        );
        for name in ["a.pdf", "b.pdf", "c.txt"] {
            std::fs::write(folder.join(name), b"x").unwrap();
        }
        let roots = vec![SourceRoot {
            source_id: "s1".into(),
            folder: folder.clone(),
        }];
        let all = list_within(&folder, false, 10, &roots).unwrap();
        assert_eq!(all.entries.len(), 3);
        assert!(!all.truncated);
        let cut = list_within(&folder, false, 2, &roots).unwrap();
        assert_eq!(cut.entries.len(), 2);
        assert!(cut.truncated);
        let wire = serde_json::to_value(&cut).unwrap();
        assert_eq!(wire["truncated"], true);
        assert!(wire["entries"].is_array());

        assert!(list_within(Path::new("relative/dir"), false, 10, &roots).is_err());
        let outside = tempfile::tempdir().unwrap();
        assert!(list_within(outside.path(), false, 10, &roots).is_err());
    }
}
