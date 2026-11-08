//! The structural view an Architecture review reads, from the code map.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{CodeMap, Pace, RefreshOptions};
use crate::code_review::structure::{
    cycles, CrateInfo, SourceFile, StructureSource, StructureView, MAX_FILES,
};

/// How long a review may spend bringing the map up to date before it reads
/// the map as it is (and says the view may be incomplete).
pub const REVIEW_WAIT: Duration = Duration::from_secs(30);

/// `path` relative to `root`, with `/` (`.` for the root itself).
fn relative_slash(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let text = rel.to_string_lossy().replace('\\', "/");
    Some(if text.is_empty() {
        ".".to_string()
    } else {
        text
    })
}

fn under(path: &str, folder: &str) -> bool {
    folder == "." || folder.is_empty() || path == folder || path.starts_with(&format!("{folder}/"))
}

impl CodeMap {
    /// The view of `folder` (relative to the project, `.` for all of it) as
    /// the map holds it now: its files with their modules and public items,
    /// the imports between its modules, its crates and its cycles. Names,
    /// counts and line numbers only: no function body is in it.
    pub fn structure(&self, folder: &str) -> Result<StructureView, String> {
        self.structure_with(folder, None)
    }

    /// [`Self::structure`], saying why the view may be incomplete: `why`
    /// when given, else when the map was never complete, how much of it was
    /// read.
    pub fn structure_with(
        &self,
        folder: &str,
        why: Option<String>,
    ) -> Result<StructureView, String> {
        let status = self.status()?;
        let incomplete = why.or_else(|| {
            status.complete_at.is_none().then(|| {
                format!(
                    "the code map is still being built: {} of {} files read",
                    status.files,
                    status
                        .listed
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "the".to_string())
                )
            })
        });
        let model = self.model()?;
        let public = self.with_store(|s| s.public_symbols())?;
        let map_partial = status.partial;
        let mut chosen: Vec<usize> = (0..model.files.len())
            .filter(|&i| under(&model.files[i].path, folder))
            .collect();
        chosen.sort_by(|a, b| model.files[*a].path.cmp(&model.files[*b].path));
        let partial = map_partial || chosen.len() > MAX_FILES;
        chosen.truncate(MAX_FILES);
        let files: Vec<SourceFile> = chosen
            .iter()
            .map(|&i| {
                let file = &model.files[i];
                let items = public
                    .get(&file.id)
                    .map(|symbols| {
                        symbols
                            .iter()
                            .filter(|s| !s.kind.is_member() && s.kind != super::SymbolKind::Macro)
                            .map(|s| {
                                if s.keyword.is_empty() {
                                    s.name.clone()
                                } else {
                                    format!("{} {}", s.keyword, s.name)
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                SourceFile {
                    path: file.path.clone(),
                    language: file.lang.name(),
                    lines: file.lines as usize,
                    module: file.module.clone(),
                    public: items,
                }
            })
            .collect();
        let mut modules: Vec<String> = files.iter().map(|f| f.module.clone()).collect();
        modules.sort();
        modules.dedup();
        let known: std::collections::HashSet<&str> = modules.iter().map(String::as_str).collect();
        let edges: Vec<_> = model
            .edges
            .iter()
            .filter(|e| known.contains(e.from.as_str()) && known.contains(e.to.as_str()))
            .cloned()
            .collect();
        let crates: Vec<CrateInfo> = model
            .crates
            .iter()
            .filter(|c| under(&c.manifest, folder))
            .map(|c| CrateInfo {
                name: c.name.clone(),
                manifest: c.manifest.clone(),
                depends_on: c.depends_on.clone(),
            })
            .collect();
        let found_cycles = cycles(&modules, &edges);
        Ok(StructureView {
            folder: if folder.is_empty() {
                ".".to_string()
            } else {
                folder.to_string()
            },
            files,
            modules,
            edges,
            crates,
            cycles: found_cycles,
            partial,
            incomplete,
        })
    }

    /// The folder `folder` of the project at `root`, relative to it, when
    /// it is inside it and this is that project's map.
    pub fn folder_label(&self, root: &Path, folder: &Path) -> Result<String, String> {
        let root = std::fs::canonicalize(root)
            .map_err(|e| format!("the code folder could not be read ({e})"))?;
        let folder = std::fs::canonicalize(folder)
            .map_err(|e| format!("the folder could not be read ({e})"))?;
        if !folder.starts_with(&root) {
            return Err("The folder is outside the code folder.".to_string());
        }
        if root != self.canonical_root() {
            return Err("This code map is of another folder.".to_string());
        }
        Ok(relative_slash(&root, &folder).unwrap_or_else(|| ".".to_string()))
    }

    /// Bring the map up to date (for at most `wait`), then read the view of
    /// `folder`. When the time ran out the view says so.
    pub fn review_view(
        &self,
        root: &Path,
        folder: &Path,
        wait: Duration,
    ) -> Result<StructureView, String> {
        let label = self.folder_label(root, folder)?;
        let options = RefreshOptions {
            max_reads: None,
            deadline: Some(std::time::Instant::now() + wait),
        };
        let report = self.refresh_with(options, &|| Pace::Go, &mut |_, _| {})?;
        let why = report.timed_out.then(|| {
            format!(
                "the code map could not be brought up to date within {} s",
                wait.as_secs()
            )
        });
        self.structure_with(&label, why)
    }
}

impl StructureSource for CodeMap {
    /// Brings the map up to date first (for at most [`REVIEW_WAIT`]), then
    /// reads the view; a view of a map that is not complete says so.
    fn build(&self, root: &Path, folder: &Path) -> Result<StructureView, String> {
        self.review_view(root, folder, REVIEW_WAIT)
    }
}

/// The structural view from the code map kept in `data_dir`, opened for the
/// folder reviewed (for callers that do not hold the map open).
#[derive(Debug, Clone)]
pub struct MapStructure {
    pub data_dir: PathBuf,
}

impl MapStructure {
    pub fn in_dir(data_dir: &Path) -> Self {
        MapStructure {
            data_dir: data_dir.to_path_buf(),
        }
    }
}

impl StructureSource for MapStructure {
    fn build(&self, root: &Path, folder: &Path) -> Result<StructureView, String> {
        let canonical_root = std::fs::canonicalize(root)
            .map_err(|e| format!("the code folder could not be read ({e})"))?;
        let canonical_folder = std::fs::canonicalize(folder)
            .map_err(|e| format!("the folder could not be read ({e})"))?;
        if !canonical_folder.starts_with(&canonical_root) {
            return Err("The folder is outside the code folder.".to_string());
        }
        CodeMap::open(&self.data_dir, root)?.build(root, folder)
    }
}
