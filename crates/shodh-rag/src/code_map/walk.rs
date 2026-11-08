//! Which files of a project the code map reads.
//!
//! In a git repository: the files git tracks or would add (`git ls-files
//! --cached --others --exclude-standard`), so `.gitignore` decides. Outside
//! one: a walk of the folder that honours `.gitignore`. Either way it never reads dependency and build
//! folders ([`SKIPPED_DIRS`]), the `.git` folder, environment files, keys or
//! credential files ([`secret_path`]), or a link (which could lead out of
//! the project), and it stops at its limits.

use std::path::Path;
use std::time::{Duration, Instant};

use super::lang::{ConfigKind, Lang};
use crate::code_review::secrets::secret_path;
use crate::code_review::structure::SKIPPED_DIRS;

/// Source files the map holds at most; past it the map says it is partial.
pub const MAX_MAP_FILES: usize = 20_000;
/// Entries a listing looks at at most, and the time it may take.
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_WALK_TIME: Duration = Duration::from_secs(30);

/// The limits of one listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Source files taken at most.
    pub max_files: usize,
    /// Entries looked at at most.
    pub max_entries: usize,
    pub max_time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_files: MAX_MAP_FILES,
            max_entries: MAX_ENTRIES,
            max_time: MAX_WALK_TIME,
        }
    }
}

/// A file the map reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    Source(String, Lang),
    Config(String, ConfigKind),
}

impl Listed {
    pub fn path(&self) -> &str {
        match self {
            Listed::Source(path, _) | Listed::Config(path, _) => path,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub files: Vec<Listed>,
    /// A limit was reached: files past it are not in the map.
    pub partial: bool,
    /// When the listing stopped early, the first path it did not look at
    /// (or did not take): files of the map from there on are kept as they
    /// were, never taken for deleted. `Some("")` keeps every file.
    pub cut: Option<String>,
    /// The listing stopped at its time or entry limit (not the file limit):
    /// the map is not complete.
    pub timed_out: bool,
}

/// Whether the map may read `relative` (`/`-separated, inside the project),
/// and as what.
pub fn classify(relative: &str) -> Option<Listed> {
    if relative.is_empty()
        || relative
            .split('/')
            .any(|part| SKIPPED_DIRS.contains(&part) || part == "..")
        || secret_path(relative).is_some()
    {
        return None;
    }
    if let Some(kind) = ConfigKind::of_path(relative) {
        return Some(Listed::Config(relative.to_string(), kind));
    }
    Lang::of_path(relative).map(|lang| Listed::Source(relative.to_string(), lang))
}

/// Lower the cut to `path` (the earliest path not taken).
fn cut_at(cut: &mut Option<String>, path: &str) {
    if cut.as_deref().is_none_or(|c| path < c) {
        *cut = Some(path.to_string());
    }
}

/// The files of the project at `root` (as given, for git) the map reads,
/// sorted by path. Config files are always listed, also past the source file
/// limit, so crate names and path aliases stay right for the files the map
/// holds.
///
/// In a repository git decides (with `safe.directory` set for this folder
/// only, so a folder owned by another account is still read as git reads
/// it). Elsewhere, or when git cannot list the files, the folder is walked
/// honouring `.gitignore`, `.ignore` and `.git/info/exclude`.
pub fn list(root: &Path) -> Result<Listing, String> {
    list_with(root, &Limits::default())
}

/// [`list`] within `limits`.
pub fn list_with(root: &Path, limits: &Limits) -> Result<Listing, String> {
    let started = Instant::now();
    let mut listing = Listing::default();
    let mut sources = 0usize;
    let mut take = |listing: &mut Listing, relative: &str| {
        let Some(listed) = classify(relative) else {
            return;
        };
        if matches!(listed, Listed::Source(..)) {
            if sources >= limits.max_files {
                listing.partial = true;
                cut_at(&mut listing.cut, relative);
                return;
            }
            sources += 1;
        }
        listing.files.push(listed);
    };
    match crate::harness::code_mode::listed_files(root, ".") {
        Some(files) => {
            for (seen, relative) in files.iter().enumerate() {
                if seen >= limits.max_entries || started.elapsed() > limits.max_time {
                    listing.partial = true;
                    listing.timed_out = true;
                    for rest in &files[seen..] {
                        cut_at(&mut listing.cut, rest);
                    }
                    break;
                }
                // `--cached` lists files deleted from the folder but not
                // from git's index, and git lists links as files.
                let is_file = std::fs::symlink_metadata(root.join(relative))
                    .is_ok_and(|m| m.file_type().is_file());
                if is_file {
                    take(&mut listing, relative);
                }
            }
        }
        None => {
            let walker = ignore::WalkBuilder::new(root)
                .hidden(false)
                .follow_links(false)
                .git_ignore(true)
                .git_exclude(true)
                .git_global(false)
                .require_git(false)
                .parents(false)
                .sort_by_file_name(|a, b| a.cmp(b))
                .filter_entry(|entry| {
                    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                    !(is_dir
                        && entry.depth() > 0
                        && entry.file_name().to_str().is_some_and(|name| {
                            SKIPPED_DIRS.contains(&name) || secret_path(name).is_some()
                        }))
                })
                .build();
            for (seen, entry) in walker.filter_map(Result::ok).enumerate() {
                if seen >= limits.max_entries || started.elapsed() > limits.max_time {
                    listing.partial = true;
                    listing.timed_out = true;
                    // A walk is not in path order: nothing is known to be gone.
                    listing.cut = Some(String::new());
                    break;
                }
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let Ok(relative) = entry.path().strip_prefix(root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                take(&mut listing, &relative);
            }
        }
    }
    listing.files.sort_by(|a, b| a.path().cmp(b.path()));
    listing.files.dedup_by(|a, b| a.path() == b.path());
    Ok(listing)
}
