//! The structural view an Architecture review gets: the folder's module tree,
//! the import graph between its modules, each module's public items, the
//! crates and their path dependencies, and the cycles Shodh found.
//!
//! It is built by Shodh, never by the reviewer, and holds names and line
//! numbers only: no function body of the folder reaches the reviewer through
//! it. It comes from Code mode's code map ([`crate::code_map`]), which reads
//! the folder with tree-sitter (Rust `mod` trees with `#[path]`, `use` and
//! paths inside macros, TypeScript path aliases and re-exports, Python and Go
//! imports), behind [`StructureSource`].
//!
//! `cargo metadata` is not run: it starts cargo in the user's project, which
//! a `rust-toolchain.toml` can make download a toolchain, and resolving the
//! dependency graph reaches the network. The manifests are read as text.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::Serialize;

/// Files the view lists at most; past it the view says it is partial.
pub const MAX_FILES: usize = 2_000;
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
    /// Why the view may miss files, imports or cycles (the code map was not
    /// complete, or could not be brought up to date in time); the reviewer
    /// is told, so it does not report an absence as a finding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<String>,
}

/// Builds the structural view of a folder (the code map: [`crate::code_map::CodeMap`]).
pub trait StructureSource: Send + Sync {
    /// The view of `folder` (inside `root`, the code folder).
    fn build(&self, root: &Path, folder: &Path) -> Result<StructureView, String>;
}

/// The language of a source file, by extension (also the code pane's highlighter name).
pub fn language_of(path: &str) -> Option<&'static str> {
    crate::code_map::Lang::of_path(path).map(crate::code_map::Lang::name)
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
            if self.partial && self.files.len() >= MAX_FILES {
                format!("; only the first {MAX_FILES} files were read")
            } else if self.partial {
                "; the project has more files than the code map holds".to_string()
            } else {
                String::new()
            }
        ));
        if let Some(why) = &self.incomplete {
            out.push_str(&format!(
                "INCOMPLETE VIEW: {why}. Files, imports and cycles may be missing: do not report \
                 that something is absent (no cycle, no import, unused) as a finding.\n"
            ));
        }
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
            incomplete: None,
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
}
