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

/// Symbols named the same that a definition query reads at most (a bound
/// against a pathological project, far above real ones: `fn new` is defined
/// a few hundred times in large Rust workspaces).
const MAX_SAME_NAME: usize = 100_000;
/// Definitions a references query ties uses to at most.
const MAX_DEFS_TIED: usize = 200;
/// Uses of one name looked at at most.
pub const MAX_REFS_SCANNED: usize = 500_000;
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
    /// How many definitions matched (`matches` holds the first ones).
    pub total: usize,
    /// More symbols have the name than a query reads ([`MAX_SAME_NAME`]).
    pub cut: bool,
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
    /// How many definitions matched (uses are tied to the first ones).
    pub definitions_total: usize,
    /// Uses resolved to the definitions (the most relevant first).
    pub resolved: Vec<RefHit>,
    pub resolved_total: usize,
    /// Uses of the same name the map could not resolve to a definition.
    pub by_name: Vec<RefHit>,
    pub by_name_total: usize,
    /// Every use of the name in the map (in the folder asked for).
    pub uses_named: u64,
    /// How many of them were looked at ([`MAX_REFS_SCANNED`]).
    pub uses_scanned: u64,
    /// Something was left out: more uses or definitions than were listed,
    /// or uses not looked at.
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
    /// narrow it, in SQL, before anything is cut. `total` says how many
    /// matched when more than `limit` did.
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
        let rows = self.with_store(|s| {
            s.symbols_named(&name, kind.map(SymbolKind::as_str), path, MAX_SAME_NAME)
        })?;
        let mut matches: Vec<(u8, SymbolHit)> = Vec::new();
        let wanted_modules = if qualifier.is_empty() {
            HashSet::new()
        } else {
            qualified_modules(&model, &qualifier, &name)
        };
        let full_qualifier = qualifier.join("::");
        for row in &rows {
            let found = hit(&model, row);
            let rank = if qualifier.is_empty() {
                Some(if found.exported { 0 } else { 1 })
            } else {
                // `Store::open`: the container is `Store`, and anything written
                // before it must name the container's module.
                let (before, last) = match qualifier.split_last() {
                    Some((last, before)) => (before.join("::"), last.as_str()),
                    None => (String::new(), ""),
                };
                let container_matches = found.container.as_deref() == Some(last)
                    && (before.is_empty()
                        || wanted_modules.contains(&found.module)
                        || qualified_modules(&model, &qualifier[..qualifier.len() - 1], last)
                            .contains(&found.module));
                if container_matches || wanted_modules.contains(&found.module) {
                    Some(0)
                } else if found.module.ends_with(&full_qualifier)
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
        let total = matches.len();
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
            total,
            cut: rows.len() >= MAX_SAME_NAME,
            similar,
        })
    }

    /// Who calls or uses `query` (as [`Self::definitions`] takes it; `path`
    /// narrows the definitions, `within` the uses to a file or folder).
    ///
    /// Every use of the name is looked at (up to [`MAX_REFS_SCANNED`]),
    /// filtered in SQL by `within`, before anything is cut. Uses are ranked:
    /// in the defining file, then in the same module, then in files that
    /// import the definition's module, then elsewhere; the first `limit` of
    /// each list are kept, and the totals say how many there are.
    pub fn references(
        &self,
        query: &str,
        path: Option<&str>,
        within: Option<&str>,
        limit: usize,
    ) -> Result<References, String> {
        let (_, name) = split_query(query);
        if name.is_empty() {
            return Err("Give the name of a symbol.".to_string());
        }
        let model = self.model()?;
        let found_defs = self.definitions(query, None, path, MAX_DEFS_TIED)?;
        let definitions = found_defs.matches.clone();
        let defs: Vec<DefInfo> = definitions
            .iter()
            .map(|d| DefInfo {
                path: d.path.clone(),
                module: d.module.clone(),
                file_module: model
                    .by_path
                    .get(&d.path)
                    .map(|&i| model.files[i].module.clone())
                    .unwrap_or_default(),
                container: d.container.clone(),
                member: SymbolKind::parse(d.kind).is_some_and(SymbolKind::is_member),
            })
            .collect();
        let reexporters = reexporting_modules(&model, &defs, &name);
        let total_named = self.with_store(|s| s.count_refs_named(&name, within))?;
        let mut resolved: [Vec<RefHit>; 4] = Default::default();
        let mut by_name: [Vec<RefHit>; 4] = Default::default();
        let mut resolved_total = 0usize;
        let mut by_name_total = 0usize;
        let scanned = self.with_store(|s| {
            s.each_ref_named(&name, within, MAX_REFS_SCANNED, &mut |row| {
                let how = if defs.is_empty() {
                    None
                } else {
                    tie(&model, &row, &defs, &reexporters)
                };
                let rank = relevance(&model, &row, &defs);
                let found = RefHit {
                    path: row.path,
                    line: row.reference.line,
                    kind: row.reference.kind.as_str(),
                    caller: row.reference.caller,
                    qualifier: row.reference.qualifier,
                    how: how.unwrap_or("by name only"),
                };
                let (bucket, total) = if how.is_some() {
                    (&mut resolved[rank], &mut resolved_total)
                } else {
                    (&mut by_name[rank], &mut by_name_total)
                };
                *total += 1;
                if bucket.len() < limit {
                    bucket.push(found);
                }
                true
            })
        })?;
        let take = |buckets: [Vec<RefHit>; 4]| -> Vec<RefHit> {
            buckets.into_iter().flatten().take(limit).collect()
        };
        let resolved = take(resolved);
        let by_name = take(by_name);
        let scanned_all = (scanned as u64) >= total_named;
        Ok(References {
            query: query.trim().to_string(),
            truncated: !scanned_all
                || resolved.len() < resolved_total
                || by_name.len() < by_name_total
                || found_defs.total > definitions.len(),
            definitions_total: found_defs.total,
            definitions,
            resolved_total,
            by_name_total,
            uses_named: total_named,
            uses_scanned: scanned as u64,
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

/// A definition a use may be tied to.
struct DefInfo {
    path: String,
    /// Its module with inline modules.
    module: String,
    /// The module of its file.
    file_module: String,
    container: Option<String>,
    /// A method or variant: reached only through its type, `Self` or a value.
    member: bool,
}

/// The modules that re-export `name` from a definition's module, for a few
/// steps of re-exports (`pub use code_map::tools;`, `export { x } from './x'`):
/// a use through one of them is a use of the definition.
fn reexporting_modules(model: &Model, defs: &[DefInfo], name: &str) -> HashSet<String> {
    let mut found: HashSet<String> = defs
        .iter()
        .filter(|d| !d.member)
        .flat_map(|d| [d.module.clone(), d.file_module.clone()])
        .collect();
    let defined: HashSet<String> = found.clone();
    for _ in 0..3 {
        let mut more = Vec::new();
        for (i, file) in model.files.iter().enumerate() {
            if found.contains(&file.module) {
                continue;
            }
            let reexports = model.imports[i].iter().any(|entry| {
                let named = entry
                    .import
                    .alias
                    .as_deref()
                    .or(entry.import.name.as_deref());
                entry.import.public
                    && (named == Some(name) || entry.import.name.is_none())
                    && entry.target.as_ref().is_some_and(|t| {
                        found.contains(t)
                            || model.file_module_of(t).is_some_and(|m| found.contains(&m))
                    })
            });
            if reexports {
                more.push(file.module.clone());
            }
        }
        if more.is_empty() {
            break;
        }
        found.extend(more);
    }
    found.retain(|m| !defined.contains(m));
    found
}

/// Where a use sits relative to the definitions: 0 in a defining file, 1 in
/// a defining module, 2 in a file that imports a defining module, 3 elsewhere.
fn relevance(model: &Model, row: &RefRow, defs: &[DefInfo]) -> usize {
    if defs.iter().any(|d| d.path == row.path) {
        return 0;
    }
    let Some(&file) = model.by_path.get(&row.path) else {
        return 3;
    };
    if defs
        .iter()
        .any(|d| d.file_module == model.files[file].module)
    {
        return 1;
    }
    let imports_one = model.imports[file].iter().any(|u| {
        u.target
            .as_ref()
            .is_some_and(|t| defs.iter().any(|d| *t == d.module || *t == d.file_module))
    });
    if imports_one {
        2
    } else {
        3
    }
}

/// The type a use is inside: `Store` for a use in `Store::open`.
fn caller_type(row: &RefRow) -> Option<&str> {
    row.reference
        .caller
        .as_deref()
        .and_then(|c| c.rsplit_once("::"))
        .map(|(ty, _)| ty)
}

/// How a use is tied to one of the definitions, or `None` (by name only).
///
/// A qualified use is tied only through its qualifier: `Self` or `self.` in
/// the definition's own type, the definition's type (in scope, or by a path
/// to its module), a module path, or an imported module or namespace. A
/// `Vec::new()` in the file that defines `Store::new` is therefore never a
/// use of it. An unqualified use is tied to a definition that is not a
/// member when it is in the defining file, imported, or in the same Rust
/// module or Go package.
fn tie(
    model: &Model,
    row: &RefRow,
    defs: &[DefInfo],
    reexporters: &HashSet<String>,
) -> Option<&'static str> {
    let &file = model.by_path.get(&row.path)?;
    let info = &model.files[file];
    let uses = &model.imports[file];
    let reaches = |target: &Option<String>, d: &DefInfo| {
        target.as_ref().is_some_and(|t| {
            *t == d.module
                || *t == d.file_module
                || model.file_module_of(t).is_some_and(|m| m == d.file_module)
                || (!d.member && reexporters.contains(t))
        })
    };
    let rust = info.lang == Lang::Rust;
    let Some(qualifier) = &row.reference.qualifier else {
        let name = row.reference.name.as_str();
        for d in defs.iter().filter(|d| !d.member) {
            if row.path == d.path {
                return Some("in the defining file");
            }
            let imported = uses.iter().any(|u| {
                let named = u.import.alias.as_deref().or(u.import.name.as_deref());
                let by_name = u.import.name.as_deref() == Some(name)
                    || u.import.alias.as_deref() == Some(name);
                let wildcard = u.import.kind == super::ImportKind::Wildcard
                    || (u.import.kind == super::ImportKind::ReExport && named.is_none());
                (by_name || wildcard) && reaches(&u.target, d)
            });
            if imported {
                return Some("imported");
            }
            if (rust || info.lang == Lang::Go) && d.file_module == info.module {
                return Some("in the same module");
            }
        }
        return None;
    };
    let q = qualifier.as_str();
    let path_like = q
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '$');
    let separator = if rust { "::" } else { "." };
    let (before, last) = match q.rsplit_once(separator) {
        Some((before, last)) => (before, last),
        None => ("", q),
    };
    for d in defs {
        let same_type = d.container.is_some()
            && caller_type(row) == d.container.as_deref()
            && row.path == d.path;
        // `self.save()` / `this.save()` / `Self::new()` inside the type itself.
        if matches!(q, "self" | "this" | "Self") {
            if same_type {
                return Some("through Self in the same type");
            }
            continue;
        }
        if row.reference.kind == RefKind::Method && !path_like {
            // A call on a value whose type the map does not know.
            continue;
        }
        if d.container.as_deref() == Some(last) && row.reference.kind != RefKind::Method {
            // `Store::open()`: the type must be the definition's.
            let type_reached = if before.is_empty() {
                row.path == d.path
                    || (rust && info.module == d.file_module)
                    || uses.iter().any(|u| {
                        let named = u.import.alias.as_deref().or(u.import.name.as_deref());
                        named == Some(last) && reaches(&u.target, d)
                    })
            } else if rust && path_like {
                let target =
                    model.resolve_rust_path(file, &row.reference.scope, before, Some(uses));
                reaches(&target, d)
            } else {
                false
            };
            if type_reached {
                return Some("through its type");
            }
            continue;
        }
        if d.member || !path_like {
            continue;
        }
        if rust {
            let target = model.resolve_rust_path(file, &row.reference.scope, q, Some(uses));
            if reaches(&target, d) {
                return Some("through its module path");
            }
        }
        if !q.contains("::") && !q.contains('.') {
            // A module or namespace alias (`import * as api`, `import pkg`, a Go package).
            let alias_hit = uses.iter().any(|u| {
                let named = u
                    .import
                    .alias
                    .as_deref()
                    .or(if u.import.kind == super::ImportKind::Wildcard {
                        None
                    } else {
                        u.import.name.as_deref()
                    })
                    .or_else(|| u.import.spec.rsplit(['/', '.']).next());
                named == Some(q) && reaches(&u.target, d)
            });
            if alias_hit {
                return Some("through an imported module");
            }
        }
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
