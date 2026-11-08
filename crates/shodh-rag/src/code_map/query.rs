//! What the code map answers: where a symbol is defined, who calls or uses
//! it, a file's or module's symbols, and a module's imports both ways with
//! its cycles. Every answer carries `path:line` receipts.
//!
//! References are resolved as far as the map can without types: a use in the
//! defining file, one reached through an import of the definition's module
//! (by name, alias, wildcard or a module alias), a path that resolves to it
//! (`crate::store::open`, `Store::open`), or one in the same Rust module or Go
//! package. A method call on a value (`x.open()`) cannot be tied to a type
//! without type checking, so it is reported as a use by name only.

use std::collections::{BTreeSet, HashSet};

use serde::Serialize;

use super::extract::{RefKind, SymbolKind};
use super::lang::Lang;
use super::model::{join_module, Model};
use super::store::{RefRow, SymbolRow};
use super::CodeMap;

/// Symbols named the same that a definition query reads at most.
const MAX_SAME_NAME: usize = 400;
/// References to one name read at most.
const MAX_REFS_READ: usize = 5_000;
/// Files an outline covers at most.
const MAX_OUTLINE_FILES: usize = 20;

/// A definition, with where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolHit {
    pub name: String,
    pub kind: &'static str,
    pub path: String,
    pub line: u32,
    pub end_line: u32,
    pub container: Option<String>,
    /// The module it is in (with Rust inline modules).
    pub module: String,
    pub signature: String,
    pub exported: bool,
    pub language: &'static str,
}

impl SymbolHit {
    /// `Store::open` or `open`.
    pub fn qualified(&self) -> String {
        match &self.container {
            Some(c) => format!("{c}::{}", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Definitions {
    pub query: String,
    pub matches: Vec<SymbolHit>,
    /// When nothing matched: names close to it.
    pub similar: Vec<SymbolHit>,
}

/// A use of a symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefHit {
    pub path: String,
    pub line: u32,
    pub kind: &'static str,
    /// The definition the use is inside.
    pub caller: Option<String>,
    pub qualifier: Option<String>,
    /// How it was tied to the definition, in words.
    pub how: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct References {
    pub query: String,
    pub definitions: Vec<SymbolHit>,
    /// Uses tied to the definitions.
    pub resolved: Vec<RefHit>,
    /// Uses of the same name the map could not tie to a definition.
    pub by_name: Vec<RefHit>,
    /// More uses than were listed.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineFile {
    pub path: String,
    pub module: String,
    pub language: &'static str,
    pub lines: u32,
    pub symbols: Vec<SymbolHit>,
    /// Why the file's symbols are not known.
    pub skipped: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outline {
    pub target: String,
    pub files: Vec<OutlineFile>,
    /// More files than were outlined.
    pub truncated: bool,
}

/// One import between modules, at its first file:line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepEdge {
    pub module: String,
    pub path: String,
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependencies {
    pub module: String,
    pub files: Vec<String>,
    /// Modules of the project it imports.
    pub imports: Vec<DepEdge>,
    /// Modules of the project that import it.
    pub imported_by: Vec<DepEdge>,
    /// What it imports from outside the project (crates, packages).
    pub external: Vec<String>,
    /// Groups of modules it is in a cycle with (itself included).
    pub cycles: Vec<Vec<String>>,
}

fn hit(model: &Model, row: &SymbolRow) -> SymbolHit {
    let (module, language) = match model.by_path.get(&row.path) {
        Some(&i) => (
            join_module(&model.files[i].module, &row.symbol.scope),
            model.files[i].lang.name(),
        ),
        None => (String::new(), ""),
    };
    SymbolHit {
        name: row.symbol.name.clone(),
        kind: row.symbol.kind.as_str(),
        path: row.path.clone(),
        line: row.symbol.line,
        end_line: row.symbol.end_line,
        container: row.symbol.container.clone(),
        module,
        signature: row.symbol.signature.clone(),
        exported: row.symbol.exported,
        language,
    }
}

/// `crate::store::Store::open` → (["crate", "store", "Store"], "open");
/// `api.get` → (["api"], "get"); `src/a.rs:open` is not a query form.
fn split_query(query: &str) -> (Vec<String>, String) {
    let q = query.trim().trim_end_matches("()").trim_end_matches('!');
    let parts: Vec<String> = if q.contains("::") {
        q.split("::").map(str::to_string).collect()
    } else if q.contains('.') && !q.contains('/') {
        q.split('.').map(str::to_string).collect()
    } else {
        vec![q.to_string()]
    };
    let mut parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
    let name = parts.pop().unwrap_or_default();
    (parts, name)
}

fn under(path: &str, dir: &str) -> bool {
    let dir = dir.trim().trim_start_matches("./").trim_end_matches('/');
    dir.is_empty() || dir == "." || path == dir || path.starts_with(&format!("{dir}/"))
}

impl CodeMap {
    /// Where `query` is defined: a name (`open`), a member (`Store::open`,
    /// `Store.open`) or a module path (`crate::store::open`, `my_core::Store`,
    /// following `pub use` re-exports). `kind` and `path` (a file or folder)
    /// narrow it.
    pub fn definitions(
        &self,
        query: &str,
        kind: Option<SymbolKind>,
        path: Option<&str>,
        limit: usize,
    ) -> Result<Definitions, String> {
        let (qualifier, name) = split_query(query);
        if name.is_empty() {
            return Err("Give the name of a symbol.".to_string());
        }
        let model = self.model()?;
        let rows = self.with_store(|s| s.symbols_named(&name, MAX_SAME_NAME))?;
        let rows: Vec<SymbolRow> = rows
            .into_iter()
            .filter(|r| kind.is_none_or(|k| r.symbol.kind == k))
            .filter(|r| path.is_none_or(|p| under(&r.path, p)))
            .collect();
        let mut matches: Vec<(u8, SymbolHit)> = Vec::new();
        let wanted_modules = if qualifier.is_empty() {
            HashSet::new()
        } else {
            qualified_modules(&model, &qualifier, &name)
        };
        for row in &rows {
            let found = hit(&model, row);
            let rank = if qualifier.is_empty() {
                Some(if found.exported { 0 } else { 1 })
            } else {
                let last = qualifier.last().map(String::as_str);
                if found.container.as_deref() == last || wanted_modules.contains(&found.module) {
                    Some(0)
                } else if found.module.ends_with(&qualifier.join("::"))
                    || found.module.ends_with(&qualifier.join("/"))
                {
                    Some(1)
                } else {
                    None
                }
            };
            if let Some(rank) = rank {
                matches.push((rank, found));
            }
        }
        matches.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| is_test_path(&a.1.path).cmp(&is_test_path(&b.1.path)))
                .then_with(|| a.1.path.cmp(&b.1.path))
                .then_with(|| a.1.line.cmp(&b.1.line))
        });
        let matches: Vec<SymbolHit> = matches.into_iter().map(|(_, h)| h).take(limit).collect();
        let similar = if matches.is_empty() {
            let prefix: String = name.chars().take(64).collect();
            self.with_store(|s| s.symbols_starting(&prefix, 12))?
                .iter()
                .filter(|r| path.is_none_or(|p| under(&r.path, p)))
                .map(|r| hit(&model, r))
                .take(10)
                .collect()
        } else {
            Vec::new()
        };
        Ok(Definitions {
            query: query.trim().to_string(),
            matches,
            similar,
        })
    }

    /// Who calls or uses `query` (as [`Self::definitions`] takes it).
    pub fn references(
        &self,
        query: &str,
        path: Option<&str>,
        limit: usize,
    ) -> Result<References, String> {
        let (_, name) = split_query(query);
        if name.is_empty() {
            return Err("Give the name of a symbol.".to_string());
        }
        let model = self.model()?;
        let definitions = self.definitions(query, None, path, 20)?.matches;
        let rows = self.with_store(|s| s.refs_named(&name, MAX_REFS_READ))?;
        let truncated_read = rows.len() >= MAX_REFS_READ;
        let def_files: HashSet<&str> = definitions.iter().map(|d| d.path.as_str()).collect();
        let mut def_modules: HashSet<String> =
            definitions.iter().map(|d| d.module.clone()).collect();
        // A module that re-exports the name from a definition's module stands
        // for it (`pub use code_map::tools;`, `export { x } from './x'`), for
        // a few steps of re-exports.
        for _ in 0..3 {
            let mut more = Vec::new();
            for (i, file) in model.files.iter().enumerate() {
                if def_modules.contains(&file.module) {
                    continue;
                }
                let reexports = model.imports[i].iter().any(|entry| {
                    let named = entry
                        .import
                        .alias
                        .as_deref()
                        .or(entry.import.name.as_deref());
                    entry.import.public
                        && (named == Some(name.as_str()) || entry.import.name.is_none())
                        && entry.target.as_ref().is_some_and(|t| {
                            def_modules.contains(t)
                                || model
                                    .file_module_of(t)
                                    .is_some_and(|m| def_modules.contains(&m))
                        })
                });
                if reexports {
                    more.push(file.module.clone());
                }
            }
            if more.is_empty() {
                break;
            }
            def_modules.extend(more);
        }
        let def_file_modules: HashSet<String> = definitions
            .iter()
            .filter_map(|d| {
                model
                    .by_path
                    .get(&d.path)
                    .map(|&i| model.files[i].module.clone())
            })
            .collect();
        let containers: HashSet<&str> = definitions
            .iter()
            .filter_map(|d| d.container.as_deref())
            .collect();
        let def_lines: HashSet<(&str, u32)> = definitions
            .iter()
            .map(|d| (d.path.as_str(), d.line))
            .collect();
        let mut resolved = Vec::new();
        let mut by_name = Vec::new();
        for row in &rows {
            if def_lines.contains(&(row.path.as_str(), row.reference.line))
                && row.reference.kind == RefKind::Type
            {
                continue;
            }
            let how = if definitions.is_empty() {
                None
            } else {
                tie(
                    &model,
                    row,
                    &def_files,
                    &def_modules,
                    &def_file_modules,
                    &containers,
                )
            };
            let found = RefHit {
                path: row.path.clone(),
                line: row.reference.line,
                kind: row.reference.kind.as_str(),
                caller: row.reference.caller.clone(),
                qualifier: row.reference.qualifier.clone(),
                how: how.unwrap_or("by name only"),
            };
            if how.is_some() {
                resolved.push(found);
            } else {
                by_name.push(found);
            }
        }
        let total = resolved.len() + by_name.len();
        resolved.truncate(limit);
        by_name.truncate(limit.saturating_sub(resolved.len()));
        Ok(References {
            query: query.trim().to_string(),
            definitions,
            truncated: truncated_read || total > resolved.len() + by_name.len(),
            resolved,
            by_name,
        })
    }

    /// The symbols of a file (`src/store.rs`), a module (`my_core::store`,
    /// `src/lib`) or the files directly in a folder.
    pub fn outline(&self, target: &str) -> Result<Outline, String> {
        let model = self.model()?;
        let target = target.trim().trim_start_matches("./").replace('\\', "/");
        let mut files: Vec<usize> = Vec::new();
        if let Some(&i) = model.by_path.get(&target) {
            files.push(i);
        } else {
            for module in model.modules_named(&target) {
                files.extend(model.modules.get(&module).into_iter().flatten().copied());
            }
        }
        files.sort();
        files.dedup();
        if files.is_empty() {
            return Err(format!(
                "The code map has no file or module {target}. Give a path relative to the code \
                 folder, or a module."
            ));
        }
        let truncated = files.len() > MAX_OUTLINE_FILES;
        let mut out = Vec::new();
        for i in files.into_iter().take(MAX_OUTLINE_FILES) {
            let file = &model.files[i];
            let rows = self.with_store(|s| s.symbols_of_file(&file.path))?;
            out.push(OutlineFile {
                path: file.path.clone(),
                module: file.module.clone(),
                language: file.lang.name(),
                lines: file.lines,
                symbols: rows.iter().map(|r| hit(&model, r)).collect(),
                skipped: file.skipped.clone(),
            });
        }
        Ok(Outline {
            target,
            files: out,
            truncated,
        })
    }

    /// What the module of `target` (a file, a module or a folder) imports of
    /// the project and from outside it, what imports it, and its cycles.
    pub fn dependencies(&self, target: &str) -> Result<Vec<Dependencies>, String> {
        let model = self.model()?;
        let modules = model.modules_named(target);
        if modules.is_empty() {
            return Err(format!(
                "The code map has no file or module {}. Give a path relative to the code folder, \
                 or a module.",
                target.trim()
            ));
        }
        let cycles = model.cycles();
        let mut out = Vec::new();
        for module in modules.into_iter().take(MAX_OUTLINE_FILES) {
            let file_ids: Vec<usize> = model.modules.get(&module).cloned().unwrap_or_default();
            let edge = |module: &str, path: &str, line: usize| DepEdge {
                module: module.to_string(),
                path: path.to_string(),
                line: u32::try_from(line).unwrap_or(u32::MAX),
            };
            let imports = model
                .edges
                .iter()
                .filter(|e| e.from == module)
                .map(|e| edge(&e.to, &e.path, e.line))
                .collect();
            let imported_by = model
                .edges
                .iter()
                .filter(|e| e.to == module)
                .map(|e| edge(&e.from, &e.path, e.line))
                .collect();
            let mut external: BTreeSet<String> = BTreeSet::new();
            for &i in &file_ids {
                let lang = model.files[i].lang;
                for entry in &model.imports[i] {
                    if entry.target.is_some() || entry.import.kind == super::ImportKind::Path {
                        continue;
                    }
                    if let Some(name) = external_name(lang, &entry.import.spec) {
                        external.insert(name);
                    }
                }
            }
            out.push(Dependencies {
                files: file_ids
                    .iter()
                    .map(|&i| model.files[i].path.clone())
                    .collect(),
                imports,
                imported_by,
                external: external.into_iter().take(60).collect(),
                cycles: cycles
                    .iter()
                    .filter(|group| group.contains(&module))
                    .cloned()
                    .collect(),
                module,
            });
        }
        Ok(out)
    }

    /// Symbols whose name starts with `prefix` (any case), for jumping to one.
    pub fn symbols_matching(&self, prefix: &str, limit: usize) -> Result<Vec<SymbolHit>, String> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.model()?;
        let rows = self.with_store(|s| s.symbols_starting(prefix, limit))?;
        Ok(rows.iter().map(|r| hit(&model, r)).collect())
    }
}

/// Tests are listed after the code they test.
fn is_test_path(path: &str) -> bool {
    path.split('/')
        .any(|p| p == "tests" || p == "test" || p == "__tests__")
        || path.ends_with("tests.rs")
        || path.contains(".test.")
        || path.contains(".spec.")
}

/// The modules a qualified query's qualifier (`["crate", "store"]`,
/// `["my_core"]`) may name, and the modules its last part re-exports `name`
/// from (`pub use`, `export … from`).
fn qualified_modules(model: &Model, qualifier: &[String], name: &str) -> HashSet<String> {
    let mut wanted: HashSet<String> = HashSet::new();
    let joined = qualifier.join("::");
    let stripped = joined
        .strip_prefix("crate::")
        .or_else(|| joined.strip_prefix("self::"))
        .unwrap_or(&joined)
        .to_string();
    for module in model.modules.keys().chain(model.inline_modules.iter()) {
        if *module == joined
            || *module == stripped
            || module.ends_with(&format!("::{stripped}"))
            || (joined == "crate" && !module.contains("::"))
            || module.ends_with(&format!("/{}", qualifier.join("/")))
        {
            wanted.insert(module.clone());
        }
    }
    // One step of re-exports: `pub use crate::store::Store;` in `my_core`
    // makes `my_core::Store` the one in `my_core::store`.
    let mut more = Vec::new();
    for module in &wanted {
        for &i in model.modules.get(module).into_iter().flatten() {
            for entry in &model.imports[i] {
                if !entry.import.public {
                    continue;
                }
                let named = entry
                    .import
                    .alias
                    .as_deref()
                    .or(entry.import.name.as_deref());
                let wildcard = entry.import.name.is_none();
                if named == Some(name) || wildcard {
                    if let Some(target) = &entry.target {
                        more.push(target.clone());
                    }
                }
            }
        }
    }
    wanted.extend(more);
    wanted
}

/// How a use is tied to one of the definitions, or `None`.
fn tie(
    model: &Model,
    row: &RefRow,
    def_files: &HashSet<&str>,
    def_modules: &HashSet<String>,
    def_file_modules: &HashSet<String>,
    containers: &HashSet<&str>,
) -> Option<&'static str> {
    if def_files.contains(row.path.as_str()) && row.reference.kind != RefKind::Method {
        return Some("in the defining file");
    }
    let &file = model.by_path.get(&row.path)?;
    let info = &model.files[file];
    let uses = &model.imports[file];
    let reaches = |target: &Option<String>| {
        target.as_ref().is_some_and(|t| {
            def_modules.contains(t)
                || def_file_modules.contains(t)
                || model
                    .file_module_of(t)
                    .is_some_and(|m| def_file_modules.contains(&m))
        })
    };
    if let Some(qualifier) = &row.reference.qualifier {
        let last = qualifier.rsplit("::").next().unwrap_or(qualifier);
        if containers.contains(last) && row.reference.kind != RefKind::Method {
            return Some("through its type");
        }
        let path_like = qualifier
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '$');
        if info.lang == Lang::Rust && path_like {
            let target = model.resolve_rust_path(file, &row.reference.scope, qualifier, Some(uses));
            if reaches(&target) {
                return Some("through its module path");
            }
        }
        if path_like && !qualifier.contains("::") {
            // A module or namespace alias (`import * as api`, `import pkg`, a Go package).
            let alias_hit = uses.iter().any(|u| {
                let named = u
                    .import
                    .alias
                    .as_deref()
                    .or(u.import.name.as_deref())
                    .or_else(|| u.import.spec.rsplit(['/', '.']).next());
                named == Some(qualifier.as_str()) && reaches(&u.target)
            });
            if alias_hit {
                return Some("through an imported module");
            }
        }
        return None;
    }
    let name = row.reference.name.as_str();
    let imported = uses.iter().any(|u| {
        let named = u.import.alias.as_deref().or(u.import.name.as_deref());
        let by_name =
            u.import.name.as_deref() == Some(name) || u.import.alias.as_deref() == Some(name);
        let wildcard = u.import.kind == super::ImportKind::Wildcard
            || (u.import.kind == super::ImportKind::ReExport && named.is_none());
        (by_name || wildcard) && reaches(&u.target)
    });
    if imported {
        return Some("imported");
    }
    if (info.lang == Lang::Rust || info.lang == Lang::Go) && def_file_modules.contains(&info.module)
    {
        return Some("in the same module");
    }
    None
}

/// The crate or package an import outside the project names.
fn external_name(lang: Lang, spec: &str) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let name = match lang {
        Lang::Rust => {
            let first = spec.trim_start_matches("::").split("::").next()?;
            if matches!(first, "crate" | "self" | "super") {
                return None;
            }
            first.to_string()
        }
        Lang::Python => {
            if spec.starts_with('.') {
                return None;
            }
            spec.split('.').next()?.to_string()
        }
        Lang::Go => spec.to_string(),
        _ => {
            if spec.starts_with('.') {
                return None;
            }
            let mut parts = spec.split('/');
            let first = parts.next()?;
            if first.starts_with('@') {
                match parts.next() {
                    Some(second) => format!("{first}/{second}"),
                    None => first.to_string(),
                }
            } else {
                first.to_string()
            }
        }
    };
    Some(name)
}
