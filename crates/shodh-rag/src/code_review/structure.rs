//! The structural view an Architecture review gets: the folder's module tree,
//! the import graph between its modules, each module's public items, the
//! crates and their path dependencies, and the cycles Shodh found.
//!
//! It is built by Shodh, never by the reviewer, and holds names and line
//! numbers only: no function body of the folder reaches the reviewer through
//! it. The parsing is deliberately light (line-based import extraction for
//! Rust and TypeScript/JavaScript, `Cargo.toml` read for `[package]` names and
//! `path` dependencies) behind [`StructureSource`], so the code map can take
//! its place without the reviewer or the diagram changing.
//!
//! `cargo metadata` is not run: it starts cargo in the user's project, which
//! a `rust-toolchain.toml` can make download a toolchain, and resolving the
//! dependency graph reaches the network. The manifests are read as text.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;

/// Files read at most; past it the view says it is partial.
pub const MAX_FILES: usize = 2_000;
/// A file larger than this is listed with its size, not read for imports.
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Entries a walk looks at at most (files and folders), and the time it may take.
pub const MAX_ENTRIES: usize = 50_000;
pub const MAX_WALK_TIME: std::time::Duration = std::time::Duration::from_secs(10);
/// Bytes of a `Cargo.toml` read at most.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// Folders never walked: dependencies, build output, version control.
pub const SKIPPED_DIRS: [&str; 19] = [
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".next",
    "vendor",
    "gen",
    "__pycache__",
    ".venv",
    "coverage",
    ".turbo",
    "venv",
    "out",
    ".gradle",
    "Pods",
    ".cargo",
    ".idea",
    ".vs",
];
/// Public items listed per module in the reviewer's view.
const MAX_PUBLIC_SHOWN: usize = 12;
/// Modules the diagram draws at most (the most connected).
pub const MAX_DIAGRAM_NODES: usize = 40;

/// One source file of the view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    /// Relative to the code folder, with `/`.
    pub path: String,
    pub language: &'static str,
    pub lines: usize,
    /// The module this file is (see [`StructureView::modules`]).
    pub module: String,
    /// Its public items, by name (`fn load`, `struct Store`, `export class X`).
    pub public: Vec<String>,
}

/// An import of one module by another, first seen at `path:line`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportEdge {
    pub from: String,
    pub to: String,
    pub path: String,
    pub line: usize,
}

/// A crate of the folder, from its `Cargo.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrateInfo {
    pub name: String,
    /// Its manifest, relative to the code folder.
    pub manifest: String,
    /// The folder's other crates it depends on by `path`.
    pub depends_on: Vec<String>,
}

/// What an Architecture review is given about a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructureView {
    /// The folder reviewed, relative to the code folder (`.` for all of it).
    pub folder: String,
    pub files: Vec<SourceFile>,
    /// Every module, sorted.
    pub modules: Vec<String>,
    /// Module-to-module imports (one per pair), sorted.
    pub edges: Vec<ImportEdge>,
    pub crates: Vec<CrateInfo>,
    /// Groups of modules that import each other in a cycle.
    pub cycles: Vec<Vec<String>>,
    /// More than [`MAX_FILES`] source files: the view covers the first ones.
    pub partial: bool,
}

/// Builds the structural view of a folder. The light parser below today;
/// the code map later.
pub trait StructureSource: Send + Sync {
    /// The view of `folder` (inside `root`, the code folder).
    fn build(&self, root: &Path, folder: &Path) -> Result<StructureView, String>;
}

/// Line-based import extraction for Rust and TypeScript/JavaScript.
#[derive(Debug, Default, Clone, Copy)]
pub struct LightStructure;

/// The language of a source file Shodh reads imports from, by extension.
pub fn language_of(path: &str) -> Option<&'static str> {
    let ext = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        _ => return None,
    })
}

fn relative_slash(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let text = rel.to_string_lossy().replace('\\', "/");
    Some(if text.is_empty() {
        ".".to_string()
    } else {
        text
    })
}

/// The text of `line` before a `//` comment (a `//` inside a string is rare
/// in import lines; it only ever hides an import, never invents one).
fn code_part(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

/// One Cargo manifest: its `[package] name` and its `path` dependencies.
#[derive(Debug, Default, PartialEq, Eq)]
struct Manifest {
    name: Option<String>,
    path_deps: Vec<String>,
}

/// Reads a `Cargo.toml` as text: the `name` of `[package]` and every `path =`
/// of a dependency table (inline `{ path = "…" }` or a `[dependencies.x]` table).
fn read_manifest(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    let mut section = String::new();
    let quoted = Regex::new(r#"path\s*=\s*"([^"]+)""#).expect("valid regex");
    let name = Regex::new(r#"^\s*name\s*=\s*"([^"]+)""#).expect("valid regex");
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_string();
            continue;
        }
        if section == "package" && manifest.name.is_none() {
            if let Some(m) = name.captures(line) {
                manifest.name = Some(m[1].to_string());
            }
        }
        let in_deps = section.ends_with("dependencies") || section.contains("dependencies.");
        if in_deps {
            if let Some(m) = quoted.captures(line) {
                manifest.path_deps.push(m[1].to_string());
            }
        }
    }
    manifest
}

/// A Rust file's module path inside its crate: `src/a/b.rs` → `a::b`,
/// `src/a/mod.rs` → `a`, `src/lib.rs` → `` (the crate root). Files outside
/// `src/` (tests, benches, build scripts) are modules of their own.
fn rust_module_path(relative_to_crate: &str) -> String {
    let path = relative_to_crate.trim_end_matches(".rs");
    let Some(inside) = path.strip_prefix("src/") else {
        return path.replace('/', "::");
    };
    let parts: Vec<&str> = inside.split('/').collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if last && (*part == "mod" || ((*part == "lib" || *part == "main") && i == 0)) {
            continue;
        }
        out.push(part);
    }
    out.join("::")
}

fn join_module(krate: &str, path: &str) -> String {
    if path.is_empty() {
        krate.to_string()
    } else {
        format!("{krate}::{path}")
    }
}

/// A TypeScript/JavaScript file's module: its path without the extension, a
/// folder for its `index` file.
fn script_module(path: &str) -> String {
    let stem = match path.rsplit_once('.') {
        Some((stem, _)) => stem,
        None => path,
    };
    stem.strip_suffix("/index").unwrap_or(stem).to_string()
}

/// `./x` from `dir`, normalised (`..` taken back).
fn resolve_relative(dir: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = if dir == "." || dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for part in spec.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// Strongly connected groups of more than one module, or a module importing
/// itself (Tarjan's algorithm, iterative).
pub fn cycles(modules: &[String], edges: &[ImportEdge]) -> Vec<Vec<String>> {
    let index: HashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.as_str(), i))
        .collect();
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); modules.len()];
    let mut self_loop = vec![false; modules.len()];
    for edge in edges {
        if let (Some(&a), Some(&b)) = (index.get(edge.from.as_str()), index.get(edge.to.as_str())) {
            if a == b {
                self_loop[a] = true;
            } else {
                next[a].push(b);
            }
        }
    }
    let n = modules.len();
    let mut order = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut counter = 0usize;
    let mut groups: Vec<Vec<String>> = Vec::new();
    for start in 0..n {
        if order[start] != usize::MAX {
            continue;
        }
        // (node, next child to look at)
        let mut work: Vec<(usize, usize)> = vec![(start, 0)];
        order[start] = counter;
        low[start] = counter;
        counter += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(top) = work.last_mut() {
            let (node, child) = *top;
            if let Some(&to) = next[node].get(child) {
                top.1 += 1;
                if order[to] == usize::MAX {
                    order[to] = counter;
                    low[to] = counter;
                    counter += 1;
                    stack.push(to);
                    on_stack[to] = true;
                    work.push((to, 0));
                } else if on_stack[to] {
                    low[node] = low[node].min(order[to]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == order[node] {
                let mut group = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    group.push(modules[member].clone());
                    if member == node {
                        break;
                    }
                }
                if group.len() > 1 || self_loop[node] {
                    group.sort();
                    groups.push(group);
                }
            }
        }
    }
    groups.sort();
    groups
}

/// What one file contributes before modules are resolved.
struct Scanned {
    file: SourceFile,
    /// Rust: (module path named after `crate::`/`super::`/`self::` resolved
    /// to this crate, line). Other crates: (the `use` path, line).
    rust_paths: Vec<(String, usize)>,
    rust_crates: Vec<(String, usize)>,
    /// Scripts: (resolved relative path without extension, line).
    script_imports: Vec<(String, usize)>,
}

struct Patterns {
    rust_path: Regex,
    rust_use_crate: Regex,
    rust_public: Regex,
    script_import: Regex,
    script_public: Regex,
}

impl Patterns {
    fn new() -> Self {
        Patterns {
            rust_path: Regex::new(r"\b(crate|super|self)((?:::[A-Za-z_][A-Za-z0-9_]*)+)")
                .expect("valid regex"),
            rust_use_crate: Regex::new(
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+(?:::)?([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)",
            )
            .expect("valid regex"),
            rust_public: Regex::new(
                r"^\s*pub\s+(?:(?:async|const|unsafe|extern\s+\S+)\s+)*(fn|struct|enum|trait|type|const|static|mod|union)\s+([A-Za-z_][A-Za-z0-9_]*)",
            )
            .expect("valid regex"),
            script_import: Regex::new(
                r#"(?:\bfrom\s+|\bimport\s*\(\s*|\brequire\s*\(\s*|^\s*import\s+)["'](\.{1,2}/[^"']*)["']"#,
            )
            .expect("valid regex"),
            script_public: Regex::new(
                r"^\s*export\s+(?:default\s+)?(?:declare\s+)?(?:async\s+)?(function\*?|class|const|let|var|interface|type|enum)\s+([A-Za-z_$][A-Za-z0-9_$]*)",
            )
            .expect("valid regex"),
        }
    }
}

/// The module `current` (a module path inside its crate) names with
/// `kind` (`crate`, `super` or `self`) and the `::a::b` that follows.
/// `super` is the parent module, for a file module and a `mod.rs` alike.
fn rust_target(current: &str, kind: &str, rest: &str) -> String {
    let named: Vec<&str> = rest.split("::").filter(|s| !s.is_empty()).collect();
    let mut base: Vec<&str> = if current.is_empty() {
        Vec::new()
    } else {
        current.split("::").collect()
    };
    match kind {
        "crate" => base.clear(),
        "super" => {
            base.pop();
        }
        _ => {}
    }
    let mut out = base;
    // `super::super::x`
    for part in named {
        if part == "super" {
            out.pop();
        } else {
            out.push(part);
        }
    }
    out.join("::")
}

impl LightStructure {
    fn scan_file(
        &self,
        patterns: &Patterns,
        root: &Path,
        path: &Path,
        crate_of: &dyn Fn(&Path) -> Option<(String, PathBuf)>,
    ) -> Option<Scanned> {
        let relative = relative_slash(root, path)?;
        let language = language_of(&relative)?;
        let size = std::fs::metadata(path).ok()?.len();
        let text = if size <= MAX_FILE_BYTES {
            std::fs::read(path)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let lines = text.lines().count();
        let mut public = Vec::new();
        let mut rust_paths = Vec::new();
        let mut rust_crates = Vec::new();
        let mut script_imports = Vec::new();
        let module;
        if language == "rust" {
            let (krate, crate_dir) =
                crate_of(path).unwrap_or_else(|| ("crate".to_string(), root.to_path_buf()));
            let inside = relative_slash(&crate_dir, path).unwrap_or_else(|| relative.clone());
            let module_path = rust_module_path(&inside);
            module = join_module(&krate, &module_path);
            for (at, raw) in text.lines().enumerate() {
                let line = code_part(raw);
                if let Some(m) = patterns.rust_public.captures(line) {
                    public.push(format!("{} {}", &m[1], &m[2]));
                }
                // `use other_crate::a::b`: the whole path, resolved to the deepest
                // module of that crate the folder has.
                if let Some(m) = patterns.rust_use_crate.captures(line) {
                    let path = m[1].to_string();
                    let first = path.split("::").next().unwrap_or("");
                    if !matches!(first, "crate" | "super" | "self" | "std" | "core" | "alloc") {
                        rust_crates.push((path, at + 1));
                    }
                }
                for m in patterns.rust_path.captures_iter(line) {
                    let target = rust_target(&module_path, &m[1], &m[2]);
                    rust_paths.push((join_module(&krate, &target), at + 1));
                }
            }
        } else {
            module = script_module(&relative);
            let dir = relative.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(".");
            for (at, raw) in text.lines().enumerate() {
                let line = code_part(raw);
                if let Some(m) = patterns.script_public.captures(line) {
                    public.push(format!("{} {}", &m[1], &m[2]));
                }
                for m in patterns.script_import.captures_iter(line) {
                    if let Some(target) = resolve_relative(dir, &m[1]) {
                        script_imports.push((target, at + 1));
                    }
                }
            }
        }
        Some(Scanned {
            file: SourceFile {
                path: relative,
                language,
                lines,
                module,
                public,
            },
            rust_paths,
            rust_crates,
            script_imports,
        })
    }
}

/// The longest known module that `named` is or is inside (`a::b::Item` →
/// `a::b` when that is a module, else `a`).
fn known_prefix<'a>(named: &str, known: &'a BTreeSet<String>) -> Option<&'a String> {
    let mut candidate = named.to_string();
    loop {
        if let Some(found) = known.get(&candidate) {
            return Some(found);
        }
        match candidate.rsplit_once("::") {
            Some((parent, _)) => candidate = parent.to_string(),
            None => return None,
        }
    }
}

/// A script import's module: `x` as written, `x` without its extension, or the
/// folder `x` with an index file.
fn script_target(spec: &str, known: &BTreeSet<String>) -> Option<String> {
    let bare = match spec.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.contains('/') => stem.to_string(),
        _ => spec.to_string(),
    };
    [spec.to_string(), bare]
        .into_iter()
        .find(|candidate| known.contains(candidate))
}

impl StructureSource for LightStructure {
    fn build(&self, root: &Path, folder: &Path) -> Result<StructureView, String> {
        let git_root = root;
        let root = std::fs::canonicalize(root)
            .map_err(|e| format!("the code folder could not be read ({e})"))?;
        let folder = std::fs::canonicalize(folder)
            .map_err(|e| format!("the folder could not be read ({e})"))?;
        if !folder.starts_with(&root) {
            return Err("The folder is outside the code folder.".to_string());
        }
        let folder_label = relative_slash(&root, &folder).unwrap_or_else(|| ".".to_string());
        let mut sources: Vec<PathBuf> = Vec::new();
        let mut manifests: Vec<PathBuf> = Vec::new();
        let mut partial = false;
        let started = std::time::Instant::now();
        let mut take = |path: PathBuf, name: &str| -> bool {
            if name == "Cargo.toml" {
                manifests.push(path);
            } else if language_of(name).is_some() {
                if sources.len() >= MAX_FILES {
                    return false;
                }
                sources.push(path);
            }
            true
        };
        // In a repository, the files git tracks or would add: `.gitignore` decides.
        // git is asked in the folder as given: a canonical Windows path (`\?`) is not
        // one every git takes.
        let listed = crate::harness::code_mode::listed_files(git_root, &folder_label);
        match listed {
            Some(files) => {
                for (seen, relative) in files.iter().enumerate() {
                    if seen >= MAX_ENTRIES || started.elapsed() > MAX_WALK_TIME {
                        partial = true;
                        break;
                    }
                    let skipped = relative.split('/').any(|part| SKIPPED_DIRS.contains(&part));
                    if skipped || super::secrets::secret_path(relative).is_some() {
                        continue;
                    }
                    let name = relative.rsplit('/').next().unwrap_or(relative);
                    let path = root.join(relative);
                    if !take(path, name) {
                        partial = true;
                        break;
                    }
                }
            }
            None => {
                let walker = walkdir::WalkDir::new(&folder)
                    .follow_links(false)
                    .sort_by_file_name()
                    .into_iter()
                    .filter_entry(|entry| {
                        !(entry.file_type().is_dir()
                            && entry.depth() > 0
                            && entry
                                .file_name()
                                .to_str()
                                .is_some_and(|name| SKIPPED_DIRS.contains(&name)))
                    });
                for (seen, entry) in walker.filter_map(Result::ok).enumerate() {
                    if seen >= MAX_ENTRIES || started.elapsed() > MAX_WALK_TIME {
                        partial = true;
                        break;
                    }
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !take(entry.path().to_path_buf(), &name) {
                        partial = true;
                        break;
                    }
                }
            }
        }
        // Crates by folder, deepest first, so a file belongs to its nearest crate.
        let mut crate_dirs: Vec<(PathBuf, String, Vec<String>)> = Vec::new();
        for manifest_path in &manifests {
            // A manifest larger than any real one is not read.
            let small =
                std::fs::metadata(manifest_path).is_ok_and(|m| m.len() <= MAX_MANIFEST_BYTES);
            let text = if small {
                std::fs::read_to_string(manifest_path).unwrap_or_default()
            } else {
                String::new()
            };
            let manifest = read_manifest(&text);
            if let (Some(name), Some(dir)) = (manifest.name, manifest_path.parent()) {
                let deps = manifest
                    .path_deps
                    .iter()
                    .filter_map(|dep| std::fs::canonicalize(dir.join(dep)).ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                crate_dirs.push((dir.to_path_buf(), name, deps));
            }
        }
        crate_dirs.sort_by_key(|(dir, _, _)| std::cmp::Reverse(dir.components().count()));
        let by_dir: HashMap<String, String> = crate_dirs
            .iter()
            .map(|(dir, name, _)| (dir.to_string_lossy().into_owned(), name.clone()))
            .collect();
        let crates: Vec<CrateInfo> = {
            let mut out: Vec<CrateInfo> = crate_dirs
                .iter()
                .map(|(dir, name, deps)| CrateInfo {
                    name: name.clone(),
                    manifest: relative_slash(&root, &dir.join("Cargo.toml"))
                        .unwrap_or_else(|| "Cargo.toml".to_string()),
                    depends_on: {
                        let mut names: Vec<String> =
                            deps.iter().filter_map(|d| by_dir.get(d).cloned()).collect();
                        names.sort();
                        names.dedup();
                        names
                    },
                })
                .collect();
            out.sort_by(|a, b| a.name.cmp(&b.name));
            out
        };
        let crate_of = |path: &Path| -> Option<(String, PathBuf)> {
            crate_dirs
                .iter()
                .find(|(dir, _, _)| path.starts_with(dir))
                .map(|(dir, name, _)| (name.replace('-', "_"), dir.clone()))
        };
        let patterns = Patterns::new();
        let scanned: Vec<Scanned> = sources
            .iter()
            .filter_map(|path| self.scan_file(&patterns, &root, path, &crate_of))
            .collect();
        let known: BTreeSet<String> = scanned.iter().map(|s| s.file.module.clone()).collect();
        let crate_names: BTreeSet<String> = crate_dirs
            .iter()
            .map(|(_, name, _)| name.replace('-', "_"))
            .collect();
        let mut edges: BTreeMap<(String, String), (String, usize)> = BTreeMap::new();
        let mut add = |from: &str, to: &str, path: &str, line: usize| {
            edges
                .entry((from.to_string(), to.to_string()))
                .or_insert_with(|| (path.to_string(), line));
        };
        for item in &scanned {
            let from = &item.file.module;
            for (named, line) in &item.rust_paths {
                if let Some(to) = known_prefix(named, &known) {
                    if to != from && !from.starts_with(&format!("{to}::")) {
                        add(from, to, &item.file.path, *line);
                    }
                }
            }
            for (path, line) in &item.rust_crates {
                let krate = path.split("::").next().unwrap_or("");
                if !crate_names.contains(krate) {
                    continue;
                }
                if let Some(to) = known_prefix(path, &known) {
                    if to != from {
                        add(from, to, &item.file.path, *line);
                    }
                }
            }
            for (spec, line) in &item.script_imports {
                if let Some(to) = script_target(spec, &known) {
                    if &to != from {
                        add(from, &to, &item.file.path, *line);
                    }
                }
            }
        }
        let edges: Vec<ImportEdge> = edges
            .into_iter()
            .map(|((from, to), (path, line))| ImportEdge {
                from,
                to,
                path,
                line,
            })
            .collect();
        let modules: Vec<String> = known.into_iter().collect();
        let found_cycles = cycles(&modules, &edges);
        let mut files: Vec<SourceFile> = scanned.into_iter().map(|s| s.file).collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(StructureView {
            folder: folder_label,
            files,
            modules,
            edges,
            crates,
            cycles: found_cycles,
            partial,
        })
    }
}

impl StructureView {
    /// The lines of `path`, when it is a file of the view.
    pub fn file_lines(&self, path: &str) -> Option<usize> {
        self.files.iter().find(|f| f.path == path).map(|f| f.lines)
    }

    /// Whether `from → to` is an import of the view.
    pub fn has_edge(&self, from: &str, to: &str) -> bool {
        self.edges.iter().any(|e| e.from == from && e.to == to)
    }

    /// Whether `from → to` lies on a cycle Shodh found.
    pub fn on_cycle(&self, from: &str, to: &str) -> bool {
        self.cycles
            .iter()
            .any(|group| group.iter().any(|m| m == from) && group.iter().any(|m| m == to))
    }

    /// The view as the reviewer reads it: names, counts and line numbers.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Folder: {} ({} source files, {} modules{})\n",
            self.folder,
            self.files.len(),
            self.modules.len(),
            if self.partial {
                format!("; only the first {MAX_FILES} files were read")
            } else {
                String::new()
            }
        ));
        if !self.crates.is_empty() {
            out.push_str("\nCrates (from Cargo.toml):\n");
            for c in &self.crates {
                if c.depends_on.is_empty() {
                    out.push_str(&format!("- {} ({})\n", c.name, c.manifest));
                } else {
                    out.push_str(&format!(
                        "- {} ({}) depends on {}\n",
                        c.name,
                        c.manifest,
                        c.depends_on.join(", ")
                    ));
                }
            }
        }
        let mut uses: HashMap<&str, usize> = HashMap::new();
        let mut used_by: HashMap<&str, usize> = HashMap::new();
        for e in &self.edges {
            *uses.entry(e.from.as_str()).or_default() += 1;
            *used_by.entry(e.to.as_str()).or_default() += 1;
        }
        out.push_str("\nFiles (module; lines; imports → / imported by ←; public items):\n");
        for f in &self.files {
            let shown: Vec<&str> = f
                .public
                .iter()
                .take(MAX_PUBLIC_SHOWN)
                .map(String::as_str)
                .collect();
            let more = f.public.len().saturating_sub(MAX_PUBLIC_SHOWN);
            out.push_str(&format!(
                "- {} [{}; {} lines; → {} ← {}]",
                f.path,
                f.module,
                f.lines,
                uses.get(f.module.as_str()).copied().unwrap_or(0),
                used_by.get(f.module.as_str()).copied().unwrap_or(0),
            ));
            if !shown.is_empty() {
                out.push_str(&format!(" pub: {}", shown.join(", ")));
                if more > 0 {
                    out.push_str(&format!(" (+{more} more)"));
                }
            }
            out.push('\n');
        }
        out.push_str("\nImports (from → to, first at file:line):\n");
        if self.edges.is_empty() {
            out.push_str("- none found\n");
        }
        for e in &self.edges {
            out.push_str(&format!(
                "- {} → {} ({}:{})\n",
                e.from, e.to, e.path, e.line
            ));
        }
        out.push_str("\nCycles Shodh found:\n");
        if self.cycles.is_empty() {
            out.push_str("- none\n");
        }
        for group in &self.cycles {
            out.push_str(&format!("- {}\n", group.join(" ↔ ")));
        }
        out
    }

    /// A Mermaid flowchart of the modules and their imports. `flagged` edges
    /// (from findings) and edges on a cycle are drawn thick and red and are
    /// labelled, so colour is never the only sign. Node ids are generated
    /// and labels quoted, so no module name can change the diagram's syntax.
    /// At most [`MAX_DIAGRAM_NODES`] modules: those with the most imports,
    /// every module of a flagged edge first.
    pub fn diagram(&self, flagged: &[(String, String)]) -> String {
        let mut degree: HashMap<&str, usize> = HashMap::new();
        for e in &self.edges {
            *degree.entry(e.from.as_str()).or_default() += 1;
            *degree.entry(e.to.as_str()).or_default() += 1;
        }
        let must: BTreeSet<&str> = flagged
            .iter()
            .flat_map(|(a, b)| [a.as_str(), b.as_str()])
            .filter(|m| self.modules.iter().any(|known| known == m))
            .collect();
        let mut ranked: Vec<&str> = self.modules.iter().map(String::as_str).collect();
        ranked.sort_by(|a, b| {
            must.contains(b)
                .cmp(&must.contains(a))
                .then(
                    degree
                        .get(b)
                        .copied()
                        .unwrap_or(0)
                        .cmp(&degree.get(a).copied().unwrap_or(0)),
                )
                .then(a.cmp(b))
        });
        ranked.truncate(MAX_DIAGRAM_NODES);
        ranked.sort();
        let ids: HashMap<&str, String> = ranked
            .iter()
            .enumerate()
            .map(|(i, m)| (*m, format!("m{i}")))
            .collect();
        let mut out = String::from("flowchart LR\n");
        for m in &ranked {
            out.push_str(&format!("  {}[\"{}\"]\n", ids[m], mermaid_label(m)));
        }
        let is_flagged = |a: &str, b: &str| flagged.iter().any(|(x, y)| x == a && y == b);
        let mut problem_links = Vec::new();
        let mut link = 0usize;
        for e in &self.edges {
            let (Some(a), Some(b)) = (ids.get(e.from.as_str()), ids.get(e.to.as_str())) else {
                continue;
            };
            let flag = is_flagged(&e.from, &e.to);
            let cycle = self.on_cycle(&e.from, &e.to);
            if flag || cycle {
                let label = match (flag, cycle) {
                    (true, true) => "problem, cycle",
                    (true, false) => "problem",
                    _ => "cycle",
                };
                out.push_str(&format!("  {a} ==>|\"{label}\"| {b}\n"));
                problem_links.push(link);
            } else {
                out.push_str(&format!("  {a} --> {b}\n"));
            }
            link += 1;
        }
        if !problem_links.is_empty() {
            let list: Vec<String> = problem_links.iter().map(usize::to_string).collect();
            out.push_str(&format!(
                "  linkStyle {} stroke:#d14343,stroke-width:3px\n",
                list.join(",")
            ));
        }
        out
    }
}

/// A module name as a quoted Mermaid label: quotes, brackets and other
/// characters Mermaid reads are replaced, so the label stays a label.
fn mermaid_label(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '"' | '`' | '[' | ']' | '{' | '}' | '(' | ')' | '<' | '>' | '|' | '#' | ';' => '_',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    #[test]
    fn rust_files_become_modules_with_imports_public_items_and_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "core/Cargo.toml",
            "[package]\nname = \"my-core\"\nversion = \"0.1.0\"\n",
        );
        write(
            root,
            "app/Cargo.toml",
            "[package]\nname = \"app\"\n\n[dependencies]\nmy-core = { path = \"../core\" }\nserde = \"1\"\n",
        );
        write(root, "core/src/lib.rs", "pub mod store;\npub mod index;\n");
        write(
            root,
            "core/src/store.rs",
            "use crate::index::Index;\npub struct Store;\npub fn open() {}\nfn hidden() { let secret_body = 1; }\n",
        );
        write(
            root,
            "core/src/index/mod.rs",
            "use super::store::Store; // back edge\npub(crate) fn inner() {}\npub trait Index {}\n",
        );
        write(
            root,
            "app/src/main.rs",
            "use my_core::store::open;\nfn main() { open(); }\n",
        );
        write(root, "app/target/debug/skip.rs", "use crate::x;\n");
        let view = LightStructure.build(root, root).unwrap();
        let paths: Vec<&str> = view.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "app/src/main.rs",
                "core/src/index/mod.rs",
                "core/src/lib.rs",
                "core/src/store.rs"
            ],
            "build output is never walked"
        );
        assert!(view.modules.contains(&"my_core::store".to_string()));
        assert!(view.modules.contains(&"my_core::index".to_string()));
        assert!(view.has_edge("my_core::store", "my_core::index"));
        assert!(view.has_edge("my_core::index", "my_core::store"));
        assert!(
            view.has_edge("app", "my_core::store"),
            "an import of another crate reaches its module"
        );
        assert_eq!(
            view.cycles,
            vec![vec![
                "my_core::index".to_string(),
                "my_core::store".to_string()
            ]]
        );
        let store = view
            .files
            .iter()
            .find(|f| f.path == "core/src/store.rs")
            .unwrap();
        assert_eq!(store.public, vec!["struct Store", "fn open"]);
        let index = view
            .files
            .iter()
            .find(|f| f.path == "core/src/index/mod.rs")
            .unwrap();
        assert_eq!(
            index.public,
            vec!["trait Index"],
            "pub(crate) is not public"
        );
        let app = view.crates.iter().find(|c| c.name == "app").unwrap();
        assert_eq!(app.depends_on, vec!["my-core"]);
        let text = view.render();
        assert!(text.contains("my_core::index ↔ my_core::store"), "{text}");
        assert!(
            !text.contains("secret_body"),
            "no function body reaches the reviewer"
        );
        assert!(text.contains("core/src/store.rs:1"), "{text}");
    }

    #[test]
    fn script_imports_resolve_relative_paths_and_index_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "src/a.ts",
            "import { b } from './lib/b';\nexport function a() {}\n",
        );
        write(
            root,
            "src/lib/b.tsx",
            "import x from '../a.ts';\nexport const b = 1;\n",
        );
        write(
            root,
            "src/lib/index.ts",
            "export * from './b';\nimport React from 'react';\n",
        );
        write(
            root,
            "src/node_modules/pkg/index.js",
            "export const no = 1;\n",
        );
        let view = LightStructure.build(root, &root.join("src")).unwrap();
        assert_eq!(view.folder, "src");
        assert!(view.has_edge("src/a", "src/lib/b"));
        assert!(view.has_edge("src/lib/b", "src/a"));
        assert!(view.has_edge("src/lib", "src/lib/b"));
        assert_eq!(
            view.cycles,
            vec![vec!["src/a".to_string(), "src/lib/b".to_string()]]
        );
        assert!(!view.files.iter().any(|f| f.path.contains("node_modules")));
        let a = view.files.iter().find(|f| f.path == "src/a.ts").unwrap();
        assert_eq!(a.public, vec!["function a"]);
    }

    #[test]
    fn a_folder_outside_the_code_folder_is_refused() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        assert!(LightStructure.build(&root, outer.path()).is_err());
    }

    #[test]
    fn the_diagram_quotes_labels_and_marks_problem_edges_in_words_too() {
        let view = StructureView {
            folder: ".".into(),
            files: Vec::new(),
            modules: vec!["a\"]; click".into(), "b".into(), "c".into()],
            edges: vec![
                ImportEdge {
                    from: "a\"]; click".into(),
                    to: "b".into(),
                    path: "a.rs".into(),
                    line: 1,
                },
                ImportEdge {
                    from: "b".into(),
                    to: "c".into(),
                    path: "b.rs".into(),
                    line: 2,
                },
            ],
            crates: Vec::new(),
            cycles: Vec::new(),
            partial: false,
        };
        let diagram = view.diagram(&[("b".into(), "c".into())]);
        assert!(diagram.starts_with("flowchart LR\n"));
        assert!(diagram.contains("[\"a___ click\"]"), "{diagram}");
        assert!(diagram.contains("==>|\"problem\"|"), "{diagram}");
        assert!(diagram.contains("linkStyle 1 stroke:#d14343"), "{diagram}");
        assert!(!diagram.contains("a\"]"), "{diagram}");
    }

    #[test]
    fn cycles_are_found_in_a_larger_graph() {
        let modules: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let edge = |from: &str, to: &str| ImportEdge {
            from: from.into(),
            to: to.into(),
            path: String::new(),
            line: 1,
        };
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "a"),
            edge("c", "d"),
            edge("e", "e"),
        ];
        assert_eq!(
            cycles(&modules, &edges),
            vec![
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec!["e".to_string()]
            ]
        );
    }

    #[test]
    fn in_a_repository_the_walk_follows_gitignore_and_stops_at_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success());
        write(root, ".gitignore", "generated/\n");
        write(root, "src/a.rs", "pub fn a() {}\n");
        write(root, "generated/big.rs", "pub fn generated() {}\n");
        let view = LightStructure.build(root, root).unwrap();
        let paths: Vec<&str> = view.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["src/a.rs"], "ignored files are never read");
        assert!(!view.partial);
    }
}
