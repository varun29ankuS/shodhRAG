//! Which files of a project the code map reads.
//!
//! In a git repository: the files git tracks or would add (`git ls-files
//! --cached --others --exclude-standard`), so `.gitignore` decides. Outside
//! one: a walk of the folder. Either way it never reads dependency and build
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

/// The files of the project at `root` (as given, for git) the map reads,
/// sorted by path.
pub fn list(root: &Path) -> Listing {
    let started = Instant::now();
    let mut listing = Listing::default();
    let mut sources = 0usize;
    let mut take = |listing: &mut Listing, relative: &str| -> bool {
        let Some(listed) = classify(relative) else {
            return true;
        };
        if matches!(listed, Listed::Source(..)) {
            if sources >= MAX_MAP_FILES {
                return false;
            }
            sources += 1;
        }
        listing.files.push(listed);
        true
    };
    match crate::harness::code_mode::listed_files(root, ".") {
        Some(files) => {
            for (seen, relative) in files.iter().enumerate() {
                if seen >= MAX_ENTRIES || started.elapsed() > MAX_WALK_TIME {
                    listing.partial = true;
                    break;
                }
                // `--cached` lists files deleted from the folder but not
                // from git's index, and git lists links as files.
                let is_file = std::fs::symlink_metadata(root.join(relative))
                    .is_ok_and(|m| m.file_type().is_file());
                if !is_file {
                    continue;
                }
                if !take(&mut listing, relative) {
                    listing.partial = true;
                    break;
                }
            }
        }
        None => {
            let walker = walkdir::WalkDir::new(root)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|entry| {
                    !(entry.file_type().is_dir()
                        && entry.depth() > 0
                        && entry.file_name().to_str().is_some_and(|name| {
                            SKIPPED_DIRS.contains(&name) || secret_path(name).is_some()
                        }))
                });
            for (seen, entry) in walker.filter_map(Result::ok).enumerate() {
                if seen >= MAX_ENTRIES || started.elapsed() > MAX_WALK_TIME {
                    listing.partial = true;
                    break;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let Ok(relative) = entry.path().strip_prefix(root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                if !take(&mut listing, &relative) {
                    listing.partial = true;
                    break;
                }
            }
        }
    }
    listing.files.sort_by(|a, b| a.path().cmp(b.path()));
    listing.files.dedup_by(|a, b| a.path() == b.path());
    listing
}
