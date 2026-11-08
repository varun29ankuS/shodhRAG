//! The project as a whole, computed from the map's per-file rows when it is
//! asked: which module each file is, what each import reaches, the
//! module-to-module import graph and the crates.
//!
//! - **Rust**: the module tree is built from `mod` declarations, starting at
//!   each crate's roots (`src/lib.rs`, `src/main.rs`, `[lib]`/`[[bin]]`
//!   paths, `src/bin/*`, `tests/*`, `examples/*`, `benches/*`, `build.rs`),
//!   so `#[path = "…"]` modules land where the compiler puts them. A file no
//!   declaration reaches (behind a `cfg` the map cannot see, or declared by a
//!   macro) is named after its path. `use` paths and paths written in code
//!   (also inside macros) resolve against that tree, `crate::`, `self::`,
//!   `super::`, the folder's other crates and local modules.
//! - **TypeScript/JavaScript**: relative specifiers, `index` files, and
//!   `paths`/`baseUrl` aliases of the nearest `tsconfig.json`/`jsconfig.json`.
//! - **Python**: dotted modules from the package folders (`__init__.py`),
//!   absolute and relative imports.
//! - **Go**: packages (folders), named by `go.mod`'s module path.
//!
//! Nothing here reads a file: it is all from the map's rows, so a query sees
//! one consistent state of the map.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use regex::Regex;

use super::extract::{Import, ImportKind};
use super::lang::{ConfigKind, Lang};
use super::store::Store;
use super::tsconfig::{self, join_normal, parent_dir, Aliases};
use crate::code_review::structure::{cycles, ImportEdge};

/// A source file of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub id: i64,
    pub path: String,
    pub lang: Lang,
    pub lines: u32,
    /// The module this file is.
    pub module: String,
    /// Rust: the root module of the crate target it is part of (`my_core`
    /// for the library, `app::main` for a binary beside a library,
    /// `app::bin::x`, `app::tests::it`), or `crate` outside any crate.
    pub crate_root: Option<String>,
    /// Rust: the folder of its package (`""` at the project root).
    pub crate_dir: Option<String>,
    pub skipped: Option<String>,
}

/// An import with what it reaches inside the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImport {
    pub import: Import,
    /// The module of the project it reaches (for Rust the deepest one,
    /// which may be an inline module); `None` outside the project.
    pub target: Option<String>,
}

/// A crate of the project, from its `Cargo.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateRec {
    pub name: String,
    /// Its folder, relative to the project (`""` at the root).
    pub dir: String,
    pub manifest: String,
    /// The project's other crates it depends on (by `path`, or by name
    /// with `workspace = true`).
    pub depends_on: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Model {
    /// Source files, by path.
    pub files: Vec<SourceInfo>,
    pub by_path: HashMap<String, usize>,
    /// Files by module.
    pub modules: BTreeMap<String, Vec<usize>>,
    /// Rust inline modules (`a::tests`), which have no file of their own.
    pub inline_modules: HashSet<String>,
    pub crates: Vec<CrateRec>,
    /// Module-to-module imports, one per pair at its first file:line, sorted.
    pub edges: Vec<ImportEdge>,
    /// Each file's imports, resolved (same order as `files`).
    pub imports: Vec<Vec<ResolvedImport>>,
    /// Workspace crate names (with `_`) to their library's root module.
    crate_roots: HashMap<String, String>,
    /// The crate names (with `_`) code of each package may use: its own
    /// library and the workspace crates it depends on, by package folder.
    crate_allowed: HashMap<String, HashSet<String>>,
    python_modules: HashSet<String>,
    script_modules: HashSet<String>,
    /// Python dotted names to their modules (more than one: ambiguous).
    python_dotted: HashMap<String, Vec<String>>,
    go_modules: HashSet<String>,
    /// Python files' dotted names, by path.
    python_dotted_of: HashMap<String, String>,
    /// Each crate's folder with its `[lib]` and `[[bin]]` paths.
    manifest_roots: Vec<(String, Option<String>, Vec<String>)>,
    /// Folders that hold a tsconfig/jsconfig, with its aliases.
    configs_by_dir: HashMap<String, Aliases>,
}

/// One Cargo manifest, read as text.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub name: Option<String>,
    pub path_deps: Vec<String>,
    /// `[lib] path`.
    pub lib_path: Option<String>,
    /// `[lib] name`: what other crates call the library (`app_lib`).
    pub lib_name: Option<String>,
    /// `[[bin]] path`s.
    pub bin_paths: Vec<String>,
    /// Every dependency's name, whatever its source (`path`, a version,
    /// `workspace = true`).
    pub dep_names: Vec<String>,
}

/// Reads a `Cargo.toml` as text: the `name` of `[package]`, every `path =` of
/// a dependency table (inline `{ path = "…" }` or a `[dependencies.x]` table),
/// and the `path` of `[lib]` and each `[[bin]]`. `cargo metadata` is never run:
/// it starts cargo in the user's project, which can download a toolchain and
/// reach the network.
pub fn read_manifest(text: &str) -> Manifest {
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
        let path = quoted.captures(line).map(|m| m[1].to_string());
        if section.ends_with("dependencies") || section.contains("dependencies.") {
            if let Some(path) = path {
                manifest.path_deps.push(path);
            }
            // `x = …` in a dependency table, or the `x` of `[dependencies.x]`.
            let named = if section.ends_with("dependencies") {
                line.split_once('=').map(|(key, _)| {
                    // `serde.workspace = true` is the dependency `serde`.
                    let key = key.trim().trim_matches('"');
                    key.split('.')
                        .next()
                        .unwrap_or(key)
                        .trim_matches('"')
                        .to_string()
                })
            } else {
                section
                    .rsplit_once("dependencies.")
                    .map(|(_, key)| key.trim().trim_matches('"').to_string())
            };
            if let Some(dep) = named.filter(|d| !d.is_empty()) {
                if !manifest.dep_names.contains(&dep) {
                    manifest.dep_names.push(dep);
                }
            }
        } else if section == "lib" {
            if let Some(path) = path {
                manifest.lib_path = Some(path);
            }
            if let Some(m) = name.captures(line) {
                manifest.lib_name = Some(m[1].to_string());
            }
        } else if section == "bin" {
            if let Some(path) = path {
                manifest.bin_paths.push(path);
            }
        }
    }
    manifest
}

/// A Rust file's module path inside its crate, by its path: `src/a/b.rs` →
/// `a::b`, `src/a/mod.rs` → `a`, `src/lib.rs` → `` (the crate root). Files
/// outside `src/` (tests, benches, build scripts) are modules of their own.
pub fn rust_module_path(relative_to_crate: &str) -> String {
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

pub fn join_module(base: &str, path: &str) -> String {
    match (base.is_empty(), path.is_empty()) {
        (true, _) => path.to_string(),
        (_, true) => base.to_string(),
        _ => format!("{base}::{path}"),
    }
}

fn parent_module(module: &str) -> Option<String> {
    module
        .rsplit_once("::")
        .map(|(parent, _)| parent.to_string())
}

/// A TypeScript/JavaScript file's module: its path without the extension, a
/// folder for its `index` file.
pub fn script_module(path: &str) -> String {
    let stem = strip_script_extension(path);
    stem.strip_suffix("/index")
        .map(str::to_string)
        .unwrap_or_else(|| {
            if stem == "index" {
                ".".to_string()
            } else {
                stem
            }
        })
}

/// `x.d.ts` → `x`, `x.tsx` → `x`; other paths unchanged.
fn strip_script_extension(path: &str) -> String {
    for ext in [".d.ts", ".d.mts", ".d.cts"] {
        if let Some(stem) = path.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    if let Some((stem, ext)) = path.rsplit_once('.') {
        if !ext.contains('/')
            && matches!(
                ext.to_ascii_lowercase().as_str(),
                "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"
            )
        {
            return stem.to_string();
        }
    }
    path.to_string()
}

/// A Python file's module: its path without `.py`, a folder for `__init__.py`.
fn python_module(path: &str) -> String {
    let stem = path
        .strip_suffix(".pyi")
        .or_else(|| path.strip_suffix(".py"))
        .unwrap_or(path);
    stem.strip_suffix("/__init__")
        .map(str::to_string)
        .unwrap_or_else(|| {
            if stem == "__init__" {
                ".".to_string()
            } else {
                stem.to_string()
            }
        })
}

fn under(path: &str, dir: &str) -> bool {
    dir.is_empty() || path == dir || path.starts_with(&format!("{dir}/"))
}

/// `path` relative to `dir` (which it is under).
fn inside<'a>(path: &'a str, dir: &str) -> &'a str {
    if dir.is_empty() {
        path
    } else {
        path.strip_prefix(dir)
            .map(|rest| rest.trim_start_matches('/'))
            .unwrap_or(path)
    }
}

impl Model {
    /// The model of everything `store` holds.
    pub fn build(store: &Store) -> Result<Model, String> {
        let rows = store.files()?;
        let configs = store.configs()?;
        let imports_by_id = store.imports()?;
        let mods_by_id = store.mods()?;
        let mut model = Model::default();
        for row in &rows {
            let Some(lang) = Lang::from_name(&row.kind) else {
                continue;
            };
            model.by_path.insert(row.path.clone(), model.files.len());
            model.files.push(SourceInfo {
                id: row.id,
                path: row.path.clone(),
                lang,
                lines: row.lines,
                module: String::new(),
                crate_root: None,
                crate_dir: None,
                skipped: row.skipped.clone(),
            });
        }
        model.read_crates(&configs);
        model.name_rust_modules(&mods_by_id);
        model.name_other_modules(&configs);
        for (i, file) in model.files.iter().enumerate() {
            model
                .modules
                .entry(file.module.clone())
                .or_default()
                .push(i);
        }
        model.read_aliases(&configs);
        // Imports resolve in two passes for Rust: a path's first segment may be
        // a name another `use` of the file brought in (`use crate::store;` then
        // `store::open()`), so `Path` entries resolve after the `use`s.
        let mut resolved: Vec<Vec<ResolvedImport>> = Vec::with_capacity(model.files.len());
        for (i, file) in model.files.iter().enumerate() {
            let list = imports_by_id.get(&file.id).cloned().unwrap_or_default();
            let first: Vec<ResolvedImport> = list
                .into_iter()
                .map(|import| {
                    let target = if import.kind == ImportKind::Path {
                        None
                    } else {
                        model.resolve_import(i, &import, None)
                    };
                    ResolvedImport { import, target }
                })
                .collect();
            resolved.push(first);
        }
        for i in 0..resolved.len() {
            let uses = resolved[i].clone();
            for entry in resolved[i].iter_mut() {
                if entry.import.kind == ImportKind::Path {
                    entry.target = model.resolve_import(i, &entry.import, Some(&uses));
                }
            }
        }
        model.imports = resolved;
        model.edges = model.compute_edges();
        Ok(model)
    }

    fn read_crates(&mut self, configs: &HashMap<String, String>) {
        let mut manifests: Vec<(&String, Manifest)> = configs
            .iter()
            .filter(|(path, _)| ConfigKind::of_path(path) == Some(ConfigKind::Cargo))
            .map(|(path, text)| (path, read_manifest(text)))
            .filter(|(_, m)| m.name.is_some())
            .collect();
        manifests.sort_by(|a, b| a.0.cmp(b.0));
        let by_dir: HashMap<String, String> = manifests
            .iter()
            .map(|(path, m)| {
                (
                    parent_dir(path).to_string(),
                    m.name.clone().unwrap_or_default(),
                )
            })
            .collect();
        let workspace_names: HashSet<String> = manifests
            .iter()
            .filter_map(|(_, m)| m.name.clone())
            .collect();
        for (path, manifest) in &manifests {
            let dir = parent_dir(path).to_string();
            let name = manifest.name.clone().unwrap_or_default();
            let mut depends_on: Vec<String> = manifest
                .path_deps
                .iter()
                .filter_map(|dep| join_normal(&dir, dep))
                .filter_map(|dep_dir| by_dir.get(&dep_dir).cloned())
                .chain(
                    manifest
                        .dep_names
                        .iter()
                        .filter(|dep| workspace_names.contains(*dep) && **dep != name)
                        .cloned(),
                )
                .collect();
            depends_on.sort();
            depends_on.dedup();
            let root = name.replace('-', "_");
            // What this package's code may name as a crate: its own library
            // (by package and `[lib]` name) and the crates it depends on.
            let mut allowed: HashSet<String> =
                depends_on.iter().map(|d| d.replace('-', "_")).collect();
            allowed.insert(root.clone());
            if let Some(lib) = &manifest.lib_name {
                let lib = lib.replace('-', "_");
                self.crate_roots.insert(lib.clone(), root.clone());
                allowed.insert(lib);
            }
            self.crate_allowed.insert(dir.clone(), allowed);
            self.crate_roots.insert(root.clone(), root);
            self.crates.push(CrateRec {
                name,
                dir,
                manifest: (*path).clone(),
                depends_on,
            });
        }
        self.crates.sort_by(|a, b| a.name.cmp(&b.name));
        // Lib and bin paths, kept for the roots below.
        self.manifest_roots = manifests
            .iter()
            .map(|(path, m)| {
                let dir = parent_dir(path).to_string();
                (dir, m.lib_path.clone(), m.bin_paths.clone())
            })
            .collect();
    }

    /// The crate `path` belongs to: the deepest one whose folder holds it.
    fn crate_of(&self, path: &str) -> Option<&CrateRec> {
        self.crates
            .iter()
            .filter(|c| under(path, &c.dir))
            .max_by_key(|c| c.dir.len())
    }

    fn name_rust_modules(&mut self, mods_by_id: &HashMap<i64, Vec<super::extract::ModDecl>>) {
        let crates = self.crates.clone();
        let mut assigned: Vec<Option<String>> = vec![None; self.files.len()];
        let mut crate_root_of: Vec<Option<String>> = vec![None; self.files.len()];
        let mut target_root: Vec<Option<String>> = vec![None; self.files.len()];
        for (i, file) in self.files.iter().enumerate() {
            if file.lang != Lang::Rust {
                continue;
            }
            crate_root_of[i] = Some(
                self.crate_of(&file.path)
                    .map(|c| c.name.replace('-', "_"))
                    .unwrap_or_else(|| "crate".to_string()),
            );
        }
        for krate in &crates {
            let root = krate.name.replace('-', "_");
            let (lib_path, bin_paths) = self
                .manifest_roots
                .iter()
                .find(|(dir, _, _)| *dir == krate.dir)
                .map(|(_, lib, bins)| (lib.clone(), bins.clone()))
                .unwrap_or_default();
            let in_crate = |path: &str, model: &Model| -> bool {
                model.crate_of(path).is_some_and(|c| c.dir == krate.dir)
            };
            // (file, its module, the root module of the target it belongs to)
            let mut queue: Vec<(usize, String, String)> = Vec::new();
            let lib = lib_path
                .as_deref()
                .and_then(|p| join_normal(&krate.dir, p))
                .unwrap_or_else(|| join_normal(&krate.dir, "src/lib.rs").unwrap_or_default());
            let has_lib = self.by_path.contains_key(&lib);
            let explicit_bins: Vec<String> = bin_paths
                .iter()
                .filter_map(|p| join_normal(&krate.dir, p))
                .collect();
            for (i, file) in self.files.iter().enumerate() {
                if file.lang != Lang::Rust || !in_crate(&file.path, self) {
                    continue;
                }
                let rel = inside(&file.path, &krate.dir);
                let is_root = file.path == lib
                    || rel == "src/main.rs"
                    || explicit_bins.contains(&file.path)
                    || rel == "build.rs"
                    || (rel.starts_with("src/bin/")
                        && (rel.matches('/').count() == 2
                            || (rel.matches('/').count() == 3 && rel.ends_with("/main.rs"))))
                    || ["tests/", "examples/", "benches/"]
                        .iter()
                        .any(|d| rel.starts_with(d) && rel.matches('/').count() == 1);
                if !is_root {
                    continue;
                }
                // A binary beside a library is a crate of its own: `crate::` in
                // it is its own tree, not the library's.
                let module = if file.path == lib {
                    root.clone()
                } else if rel == "src/main.rs" && has_lib {
                    join_module(&root, "main")
                } else {
                    join_module(&root, &rust_module_path(rel))
                };
                if assigned[i].is_none() {
                    assigned[i] = Some(module.clone());
                    target_root[i] = Some(module.clone());
                    queue.push((i, module.clone(), module));
                }
            }
            while let Some((i, module, target)) = queue.pop() {
                let path = self.files[i].path.clone();
                let Some(decls) = mods_by_id.get(&self.files[i].id) else {
                    continue;
                };
                let dir = parent_dir(&path).to_string();
                let name = path.rsplit('/').next().unwrap_or(&path);
                // A crate root or a `mod.rs` holds its submodules in its own
                // folder; `a.rs` holds them in `a/`.
                let mod_rs = name == "mod.rs"
                    || assigned[i].as_deref() == Some(target.as_str())
                    || queue_root(&path, &krate.dir);
                let own_dir = if mod_rs {
                    dir.clone()
                } else {
                    join_normal(&dir, name.trim_end_matches(".rs")).unwrap_or_default()
                };
                for decl in decls {
                    let scope_dir = decl.scope.replace("::", "/");
                    let child = join_module(&join_module(&module, &decl.scope), &decl.name);
                    if decl.inline {
                        self.inline_modules.insert(child);
                        continue;
                    }
                    let candidates: Vec<String> = match &decl.path_attr {
                        Some(attr) => {
                            let base = if decl.scope.is_empty() {
                                dir.clone()
                            } else {
                                join_normal(&own_dir, &scope_dir).unwrap_or_default()
                            };
                            join_normal(&base, attr).into_iter().collect()
                        }
                        None => {
                            let base = join_normal(&own_dir, &scope_dir).unwrap_or_default();
                            [
                                join_normal(&base, &format!("{}.rs", decl.name)),
                                join_normal(&base, &format!("{}/mod.rs", decl.name)),
                            ]
                            .into_iter()
                            .flatten()
                            .collect()
                        }
                    };
                    for candidate in candidates {
                        let Some(&found) = self.by_path.get(&candidate) else {
                            continue;
                        };
                        if self.files[found].lang != Lang::Rust || assigned[found].is_some() {
                            continue;
                        }
                        assigned[found] = Some(child.clone());
                        target_root[found] = Some(target.clone());
                        queue.push((found, child.clone(), target.clone()));
                        break;
                    }
                }
            }
        }
        for (i, file) in self.files.iter_mut().enumerate() {
            if file.lang != Lang::Rust {
                continue;
            }
            let root = crate_root_of[i]
                .clone()
                .unwrap_or_else(|| "crate".to_string());
            file.module = match assigned[i].take() {
                Some(module) => module,
                None => {
                    let dir = self
                        .crates
                        .iter()
                        .filter(|c| under(&file.path, &c.dir))
                        .max_by_key(|c| c.dir.len())
                        .map(|c| c.dir.clone())
                        .unwrap_or_default();
                    join_module(&root, &rust_module_path(inside(&file.path, &dir)))
                }
            };
            file.crate_dir = self
                .crates
                .iter()
                .filter(|c| under(&file.path, &c.dir))
                .max_by_key(|c| c.dir.len())
                .map(|c| c.dir.clone());
            file.crate_root = Some(target_root[i].take().unwrap_or(root));
        }
    }

    fn name_other_modules(&mut self, configs: &HashMap<String, String>) {
        let go_mods: Vec<(String, String)> = configs
            .iter()
            .filter(|(path, _)| ConfigKind::of_path(path) == Some(ConfigKind::GoMod))
            .filter_map(|(path, text)| {
                text.lines()
                    .map(str::trim)
                    .find_map(|l| l.strip_prefix("module "))
                    .map(|m| (parent_dir(path).to_string(), unquote_go(m.trim())))
            })
            .collect();
        let inits: HashSet<String> = self
            .files
            .iter()
            .filter(|f| f.lang == Lang::Python && f.path.rsplit('/').next() == Some("__init__.py"))
            .map(|f| parent_dir(&f.path).to_string())
            .collect();
        for i in 0..self.files.len() {
            let path = self.files[i].path.clone();
            let module = match self.files[i].lang {
                Lang::Rust => continue,
                lang if lang.is_script() => {
                    let m = script_module(&path);
                    self.script_modules.insert(m.clone());
                    m
                }
                Lang::Python => {
                    let m = python_module(&path);
                    let dotted = python_dotted(&path, &inits);
                    self.python_dotted_of.insert(path.clone(), dotted.clone());
                    self.python_modules.insert(m.clone());
                    self.python_dotted
                        .entry(dotted)
                        .or_default()
                        .push(m.clone());
                    m
                }
                _ => {
                    let dir = parent_dir(&path);
                    let m = go_mods
                        .iter()
                        .filter(|(d, _)| under(dir, d))
                        .max_by_key(|(d, _)| d.len())
                        .map(|(d, module)| {
                            let rest = inside(dir, d);
                            if rest.is_empty() {
                                module.clone()
                            } else {
                                format!("{module}/{rest}")
                            }
                        })
                        .unwrap_or_else(|| {
                            if dir.is_empty() {
                                ".".to_string()
                            } else {
                                dir.to_string()
                            }
                        });
                    self.go_modules.insert(m.clone());
                    m
                }
            };
            self.files[i].module = module;
        }
    }

    fn read_aliases(&mut self, configs: &HashMap<String, String>) {
        let mut dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in configs.keys() {
            if ConfigKind::of_path(path) == Some(ConfigKind::TsConfig) {
                dirs.entry(parent_dir(path).to_string())
                    .or_default()
                    .push(path.clone());
            }
        }
        for (dir, mut paths) in dirs {
            paths.sort();
            // tsconfig.json first; a tsconfig.json without aliases that only
            // points at others (Vite's tsconfig.app.json) takes theirs.
            paths.sort_by_key(|p| {
                let name = p.rsplit('/').next().unwrap_or(p);
                !(name == "tsconfig.json" || name == "jsconfig.json")
            });
            let mut merged = Aliases::default();
            for path in &paths {
                let found = tsconfig::aliases(path, configs);
                if merged.base_url.is_none() {
                    merged.base_url = found.base_url;
                }
                for entry in found.paths {
                    if !merged.paths.iter().any(|(p, _)| *p == entry.0) {
                        merged.paths.push(entry);
                    }
                }
            }
            self.configs_by_dir.insert(dir, merged);
        }
    }

    /// The aliases of the nearest config at or above `dir`.
    fn aliases_for(&self, dir: &str) -> Option<&Aliases> {
        let mut at = dir.to_string();
        loop {
            if let Some(found) = self.configs_by_dir.get(&at) {
                return Some(found);
            }
            if at.is_empty() {
                return None;
            }
            at = parent_dir(&at).to_string();
        }
    }

    fn known_rust(&self, module: &str) -> bool {
        self.modules.contains_key(module) || self.inline_modules.contains(module)
    }

    /// The deepest module of the project a Rust path reaches from `file` (in
    /// its inline module `scope`). `uses` are the file's resolved imports,
    /// for a path that starts with a name one of them brought in.
    pub fn resolve_rust_path(
        &self,
        file: usize,
        scope: &str,
        path: &str,
        uses: Option<&[ResolvedImport]>,
    ) -> Option<String> {
        let path = path.trim_start_matches("::");
        let segments: Vec<&str> = path
            .split("::")
            .filter(|s| !s.is_empty() && *s != "*")
            .collect();
        let first = *segments.first()?;
        let info = self.files.get(file)?;
        let current = join_module(&info.module, scope);
        let crate_root = info
            .crate_root
            .clone()
            .unwrap_or_else(|| "crate".to_string());
        let (mut base, rest): (String, &[&str]) = match first {
            "crate" | "$crate" => (crate_root, &segments[1..]),
            "self" => (current, &segments[1..]),
            "super" => {
                let mut base = current;
                let mut at = 0;
                while segments.get(at) == Some(&"super") {
                    base = parent_module(&base)?;
                    at += 1;
                }
                (base, &segments[at..])
            }
            "std" | "core" | "alloc" => return None,
            // As rustc looks a name up: a module of the current scope, then a
            // name a `use` brought in, then a crate this package may use.
            name => {
                let child = join_module(&current, name);
                let imported = uses.and_then(|uses| {
                    uses.iter().find_map(|u| {
                        let named = u.import.alias.as_deref().or(u.import.name.as_deref());
                        (named == Some(name) && u.import.kind != ImportKind::Path)
                            .then(|| u.target.clone())
                            .flatten()
                    })
                });
                if self.known_rust(&child) {
                    (current, &segments[..])
                } else if let Some(target) = imported {
                    (target, &segments[1..])
                } else if let Some(root) = self
                    .crate_roots
                    .get(name)
                    .filter(|root| self.may_use_crate(info, root))
                {
                    (root.clone(), &segments[1..])
                } else {
                    return None;
                }
            }
        };
        if !self.known_rust(&base)
            && !self
                .modules
                .keys()
                .any(|m| m.starts_with(&format!("{base}::")))
        {
            return None;
        }
        for segment in rest {
            let next = join_module(&base, segment);
            if self.known_rust(&next) {
                base = next;
            } else {
                break;
            }
        }
        Some(base)
    }

    /// Whether code of `info`'s package may name the crate whose library
    /// root is `root`: its own library, or a crate it depends on.
    fn may_use_crate(&self, info: &SourceInfo, root: &str) -> bool {
        match info
            .crate_dir
            .as_ref()
            .and_then(|dir| self.crate_allowed.get(dir))
        {
            Some(allowed) => allowed.contains(root),
            // Outside any package: every crate of the project.
            None => true,
        }
    }

    /// A script specifier's module from `file`.
    fn resolve_script(&self, file: usize, spec: &str) -> Option<String> {
        let path = self.files.get(file)?.path.clone();
        let dir = parent_dir(&path).to_string();
        let relative =
            spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../");
        let candidates: Vec<String> = if relative {
            join_normal(&dir, spec).into_iter().collect()
        } else {
            self.aliases_for(&dir)
                .map(|a| a.candidates(spec))
                .unwrap_or_default()
        };
        candidates.into_iter().find_map(|candidate| {
            let bare = strip_script_extension(&candidate);
            [candidate, bare]
                .into_iter()
                .map(|c| if c.is_empty() { ".".to_string() } else { c })
                .find(|c| self.script_modules.contains(c))
        })
    }

    /// A Python import's module from `file`.
    fn resolve_python(&self, file: usize, import: &Import) -> Option<String> {
        let path = &self.files.get(file)?.path;
        let spec = import.spec.as_str();
        let dots = spec.chars().take_while(|c| *c == '.').count();
        let rest = &spec[dots..];
        let mut bases: Vec<String> = Vec::new();
        if dots > 0 {
            let own = self.python_dotted_of.get(path).cloned().unwrap_or_default();
            let mut package: Vec<&str> = own.split('.').filter(|s| !s.is_empty()).collect();
            if !path.ends_with("__init__.py") {
                package.pop();
            }
            for _ in 1..dots {
                package.pop()?;
            }
            let mut base = package.join(".");
            if !rest.is_empty() {
                base = if base.is_empty() {
                    rest.to_string()
                } else {
                    format!("{base}.{rest}")
                };
            }
            bases.push(base);
        } else {
            // A script's own folder comes first, as Python puts it first on
            // its path.
            let dir = parent_dir(path);
            let local = |dotted: &str| -> Option<String> {
                let module = join_normal(dir, &dotted.replace('.', "/"))?;
                self.python_modules.contains(&module).then_some(module)
            };
            if let Some(name) = &import.name {
                if let Some(m) = local(&format!("{rest}.{name}")) {
                    return Some(m);
                }
            }
            if let Some(m) = local(rest) {
                return Some(m);
            }
            bases.push(rest.to_string());
        }
        // A dotted name two files have is ambiguous: tied to neither.
        let unique = |dotted: &str| -> Option<Option<String>> {
            self.python_dotted
                .get(dotted)
                .map(|found| (found.len() == 1).then(|| found[0].clone()))
        };
        for base in bases {
            if let Some(name) = &import.name {
                let sub = if base.is_empty() {
                    name.clone()
                } else {
                    format!("{base}.{name}")
                };
                if let Some(found) = unique(&sub) {
                    return found;
                }
            }
            let mut candidate = base;
            loop {
                if let Some(found) = unique(&candidate) {
                    return found;
                }
                match candidate.rsplit_once('.') {
                    Some((parent, _)) => candidate = parent.to_string(),
                    None => break,
                }
            }
        }
        None
    }

    fn resolve_import(
        &self,
        file: usize,
        import: &Import,
        uses: Option<&[ResolvedImport]>,
    ) -> Option<String> {
        let lang = self.files.get(file)?.lang;
        match lang {
            Lang::Rust => match import.kind {
                ImportKind::ExternCrate => self.crate_roots.get(&import.spec).cloned(),
                _ => self.resolve_rust_path(file, &import.scope, &import.spec, uses),
            },
            Lang::Python => self.resolve_python(file, import),
            Lang::Go => self
                .go_modules
                .contains(&import.spec)
                .then(|| import.spec.clone()),
            _ => self.resolve_script(file, &import.spec),
        }
    }

    /// The deepest module that has a file and that `module` is or is inside.
    pub fn file_module_of(&self, module: &str) -> Option<String> {
        let mut candidate = module.to_string();
        loop {
            if self.modules.contains_key(&candidate) {
                return Some(candidate);
            }
            candidate = parent_module(&candidate)?;
        }
    }

    fn compute_edges(&self) -> Vec<ImportEdge> {
        let mut edges: BTreeMap<(String, String), (String, usize)> = BTreeMap::new();
        for (i, file) in self.files.iter().enumerate() {
            let from = &file.module;
            for entry in &self.imports[i] {
                let Some(target) = &entry.target else {
                    continue;
                };
                let to = if file.lang == Lang::Rust {
                    match self.file_module_of(target) {
                        Some(to) => to,
                        None => continue,
                    }
                } else {
                    target.clone()
                };
                if &to == from {
                    continue;
                }
                // A Rust module using its parent (`super::x` in a child) is
                // part of its own tree, not a dependency between modules.
                if file.lang == Lang::Rust && from.starts_with(&format!("{to}::")) {
                    continue;
                }
                edges
                    .entry((from.clone(), to))
                    .or_insert_with(|| (file.path.clone(), entry.import.line as usize));
            }
        }
        edges
            .into_iter()
            .map(|((from, to), (path, line))| ImportEdge {
                from,
                to,
                path,
                line,
            })
            .collect()
    }

    /// Groups of modules that import each other.
    pub fn cycles(&self) -> Vec<Vec<String>> {
        let modules: Vec<String> = self.modules.keys().cloned().collect();
        cycles(&modules, &self.edges)
    }

    /// The modules `name` may mean: a module as written, or the module of a
    /// file path.
    pub fn modules_named(&self, name: &str) -> Vec<String> {
        let name = name.trim().trim_start_matches("./").replace('\\', "/");
        let mut out: BTreeSet<String> = BTreeSet::new();
        if self.modules.contains_key(&name) {
            out.insert(name.clone());
        }
        if let Some(&i) = self.by_path.get(&name) {
            out.insert(self.files[i].module.clone());
        }
        if out.is_empty() {
            // A folder: the modules of the files directly in it.
            for file in &self.files {
                if parent_dir(&file.path) == name {
                    out.insert(file.module.clone());
                }
            }
        }
        out.into_iter().collect()
    }
}

/// `"x"` → `x` (a go.mod module path may be quoted).
fn unquote_go(text: &str) -> String {
    text.trim_matches(|c| c == '"' || c == '`').to_string()
}

/// A Python file's dotted name from its package folders: `src/pkg/mod.py`
/// with `src/pkg/__init__.py` → `pkg.mod`.
fn python_dotted(path: &str, inits: &HashSet<String>) -> String {
    let stem = path
        .strip_suffix(".pyi")
        .or_else(|| path.strip_suffix(".py"))
        .unwrap_or(path);
    let mut parts: Vec<&str> = stem.split('/').collect();
    if parts.last() == Some(&"__init__") {
        parts.pop();
    }
    if parts.is_empty() {
        return String::new();
    }
    let mut first = parts.len() - 1;
    while first > 0 && inits.contains(&parts[..first].join("/")) {
        first -= 1;
    }
    parts[first..].join(".")
}

/// Whether `path` is a crate root of the crate in `dir` that holds its
/// submodules in its own folder (`src/main.rs`, `src/bin/x.rs`, `tests/x.rs`).
fn queue_root(path: &str, dir: &str) -> bool {
    let rel = inside(path, dir);
    rel == "src/main.rs"
        || rel == "src/lib.rs"
        || rel == "build.rs"
        || (rel.starts_with("src/bin/") && rel.matches('/').count() == 2)
        || rel.ends_with("/main.rs") && rel.starts_with("src/bin/")
        || ["tests/", "examples/", "benches/"]
            .iter()
            .any(|d| rel.starts_with(d) && rel.matches('/').count() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_paths_follow_rusts_file_layout() {
        assert_eq!(rust_module_path("src/lib.rs"), "");
        assert_eq!(rust_module_path("src/main.rs"), "");
        assert_eq!(rust_module_path("src/a/mod.rs"), "a");
        assert_eq!(rust_module_path("src/a/b.rs"), "a::b");
        assert_eq!(rust_module_path("tests/it.rs"), "tests::it");
        assert_eq!(script_module("src/lib/index.ts"), "src/lib");
        assert_eq!(script_module("src/a.d.ts"), "src/a");
        assert_eq!(python_module("pkg/__init__.py"), "pkg");
        let inits: HashSet<String> = ["src/pkg".to_string()].into_iter().collect();
        assert_eq!(python_dotted("src/pkg/mod.py", &inits), "pkg.mod");
        assert_eq!(python_dotted("src/pkg/__init__.py", &inits), "pkg");
        assert_eq!(python_dotted("tool.py", &inits), "tool");
    }

    #[test]
    fn manifests_give_names_path_dependencies_and_roots() {
        let m = read_manifest(
            "[package]\nname = \"app\"\n\n[lib]\npath = \"lib/root.rs\"\n\n[[bin]]\nname = \"x\"\npath = \"tools/x.rs\"\n\n[dependencies]\ncore = { path = \"../core\" } # local\nserde = \"1\"\n\n[dependencies.other]\npath = \"../other\"\n",
        );
        assert_eq!(m.name.as_deref(), Some("app"));
        assert_eq!(m.path_deps, vec!["../core", "../other"]);
        assert_eq!(m.lib_path.as_deref(), Some("lib/root.rs"));
        assert_eq!(m.bin_paths, vec!["tools/x.rs"]);
    }
}
