//! The structural view an Architecture review reads, from the code map.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{CodeMap, Pace};
use crate::code_review::structure::{
    cycles, CrateInfo, SourceFile, StructureSource, StructureView, MAX_FILES,
};

/// How long a review waits for a refresh already running before it reads the
/// map as it is.
const REVIEW_WAIT: Duration = Duration::from_secs(30);

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
        let model = self.model()?;
        let public = self.with_store(|s| s.public_symbols())?;
        let map_partial = self.status().map(|s| s.partial).unwrap_or(false);
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
        })
    }
}

impl StructureSource for CodeMap {
    /// Brings the map up to date first (waiting at most [`REVIEW_WAIT`] for a
    /// refresh already running), then reads the view.
    fn build(&self, root: &Path, folder: &Path) -> Result<StructureView, String> {
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
        let label = relative_slash(&root, &folder).unwrap_or_else(|| ".".to_string());
        self.refresh_within(REVIEW_WAIT, &|| Pace::Go, &mut |_, _| {})?;
        self.structure(&label)
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
