//! The code map on fixture projects: extraction per language with the tricky
//! cases (macros, `#[path]` modules, TypeScript path aliases, re-exports),
//! incremental refreshes on change and delete, the files it never reads, the
//! queries' receipts, and the structural view an Architecture review reads.

use std::path::Path;

use super::extract::{extract, ImportKind, RefKind, SymbolKind};
use super::*;
use crate::code_review::structure::StructureSource;

fn write(root: &Path, path: &str, text: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, text).unwrap();
}

fn go() -> Pace {
    Pace::Go
}

fn built(root: &Path, data: &Path) -> CodeMap {
    let map = CodeMap::open(data, root).unwrap();
    map.refresh(&go, &mut |_, _| {}).unwrap();
    map
}

fn git(root: &Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap()
        .success());
}

// ---- extraction -------------------------------------------------------------

#[test]
fn rust_definitions_calls_uses_and_macros_are_read() {
    let src = br#"
use crate::store::{self, Store as S, index::*};
pub use super::other::Thing;
#[path = "gen/x.rs"]
mod generated;
mod inline { pub fn deep() {} }

pub struct Store { items: Vec<Item> }
pub(crate) enum Mode { Fast, Slow }
pub trait Index { fn find(&self) -> usize; }
impl Store {
    pub fn open() -> Self { helper(); Self::new(); crate::util::log("x"); Store { items: vec![] } }
    fn new() -> Self { let s = store::make(); s.items.push(1); s }
}
macro_rules! make_it { ($x:expr) => { $crate::store::build($x) }; }
pub const MAX: usize = 3;
fn helper() { println!("{}", compute(1)); make_it!(2); }
"#;
    let facts = extract(Lang::Rust, src).unwrap();
    let names: Vec<(&str, SymbolKind, bool)> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.exported))
        .collect();
    for expected in [
        ("generated", SymbolKind::Module, false),
        ("inline", SymbolKind::Module, false),
        ("deep", SymbolKind::Function, true),
        ("Store", SymbolKind::Struct, true),
        ("Mode", SymbolKind::Enum, false),
        ("Fast", SymbolKind::Variant, false),
        ("Index", SymbolKind::Trait, true),
        ("find", SymbolKind::Method, true),
        ("open", SymbolKind::Method, true),
        ("new", SymbolKind::Method, false),
        ("make_it", SymbolKind::Macro, false),
        ("MAX", SymbolKind::Constant, true),
        ("helper", SymbolKind::Function, false),
    ] {
        assert!(names.contains(&expected), "{expected:?} in {names:?}");
    }
    let open = facts.symbols.iter().find(|s| s.name == "open").unwrap();
    assert_eq!(open.container.as_deref(), Some("Store"));
    assert_eq!(open.signature, "pub fn open() -> Self");
    let deep = facts.symbols.iter().find(|s| s.name == "deep").unwrap();
    assert_eq!(deep.scope, "inline");

    let generated = facts.mods.iter().find(|m| m.name == "generated").unwrap();
    assert_eq!(generated.path_attr.as_deref(), Some("gen/x.rs"));
    assert!(!generated.inline);
    assert!(facts.mods.iter().any(|m| m.name == "inline" && m.inline));

    let specs: Vec<(&str, Option<&str>, Option<&str>, ImportKind, bool)> = facts
        .imports
        .iter()
        .filter(|i| i.kind != ImportKind::Path)
        .map(|i| {
            (
                i.spec.as_str(),
                i.name.as_deref(),
                i.alias.as_deref(),
                i.kind,
                i.public,
            )
        })
        .collect();
    assert!(
        specs.contains(&("crate::store", Some("store"), None, ImportKind::Use, false)),
        "{specs:?}"
    );
    assert!(specs.contains(&(
        "crate::store::Store",
        Some("Store"),
        Some("S"),
        ImportKind::Use,
        false
    )));
    assert!(specs.contains(&(
        "crate::store::index::*",
        None,
        None,
        ImportKind::Wildcard,
        false
    )));
    assert!(specs.contains(&(
        "super::other::Thing",
        Some("Thing"),
        None,
        ImportKind::Use,
        true
    )));

    let refs: Vec<(&str, Option<&str>, RefKind, Option<&str>)> = facts
        .refs
        .iter()
        .map(|r| {
            (
                r.name.as_str(),
                r.qualifier.as_deref(),
                r.kind,
                r.caller.as_deref(),
            )
        })
        .collect();
    assert!(
        refs.contains(&("helper", None, RefKind::Call, Some("Store::open"))),
        "{refs:?}"
    );
    assert!(refs.contains(&("new", Some("Self"), RefKind::Call, Some("Store::open"))));
    assert!(refs.contains(&(
        "log",
        Some("crate::util"),
        RefKind::Call,
        Some("Store::open")
    )));
    assert!(refs.contains(&("make", Some("store"), RefKind::Call, Some("Store::new"))));
    assert!(refs.contains(&("push", Some("s.items"), RefKind::Method, Some("Store::new"))));
    assert!(refs.contains(&("Item", None, RefKind::Type, Some("Store"))));
    // Inside macro calls and macro_rules! bodies, which tree-sitter keeps as tokens.
    assert!(
        refs.contains(&("compute", None, RefKind::Call, Some("helper"))),
        "{refs:?}"
    );
    assert!(refs.contains(&("make_it", None, RefKind::Macro, Some("helper"))));
    assert!(refs.contains(&("println", None, RefKind::Macro, Some("helper"))));
    assert!(refs.contains(&(
        "build",
        Some("$crate::store"),
        RefKind::Call,
        Some("make_it")
    )));
    let paths: Vec<&str> = facts
        .imports
        .iter()
        .filter(|i| i.kind == ImportKind::Path)
        .map(|i| i.spec.as_str())
        .collect();
    assert!(paths.contains(&"crate::util::log"), "{paths:?}");
    assert!(paths.contains(&"$crate::store::build"), "{paths:?}");
    assert!(paths.contains(&"store::make"), "{paths:?}");
}

#[test]
fn typescript_definitions_imports_and_reexports_are_read() {
    let src = br#"
import React, { useState as useS } from 'react';
import * as api from '@/lib/api';
import './side-effect.css';
export { helper, other as renamed } from './helpers';
export * from './all';
export interface Props { name: string; render(): void }
export type Id = string;
export enum Color { Red }
export const Button = ({ name }: Props) => { const [s] = useS(0); return <Icon size={1} />; };
export default function App() { api.get(name); new Store(); return null; }
class Store { open() { this.save(); } private save = () => {}; }
let counter = 0;
const lazy = () => import('./lazy');
const legacy = require('./legacy');
"#;
    let facts = extract(Lang::Tsx, src).unwrap();
    let names: Vec<(&str, SymbolKind, bool, &str)> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.exported, s.keyword))
        .collect();
    for expected in [
        ("Props", SymbolKind::Interface, true, "interface"),
        ("render", SymbolKind::Method, true, ""),
        ("Id", SymbolKind::Type, true, "type"),
        ("Color", SymbolKind::Enum, true, "enum"),
        ("Button", SymbolKind::Function, true, "const"),
        ("App", SymbolKind::Function, true, "function"),
        ("Store", SymbolKind::Class, false, "class"),
        ("open", SymbolKind::Method, false, ""),
        ("save", SymbolKind::Method, false, ""),
        ("counter", SymbolKind::Variable, false, "let"),
    ] {
        assert!(names.contains(&expected), "{expected:?} in {names:?}");
    }
    let imports: Vec<(&str, Option<&str>, Option<&str>, ImportKind, bool)> = facts
        .imports
        .iter()
        .map(|i| {
            (
                i.spec.as_str(),
                i.name.as_deref(),
                i.alias.as_deref(),
                i.kind,
                i.public,
            )
        })
        .collect();
    for expected in [
        (
            "react",
            Some("default"),
            Some("React"),
            ImportKind::Use,
            false,
        ),
        (
            "react",
            Some("useState"),
            Some("useS"),
            ImportKind::Use,
            false,
        ),
        ("@/lib/api", None, Some("api"), ImportKind::Wildcard, false),
        ("./side-effect.css", None, None, ImportKind::Use, false),
        (
            "./helpers",
            Some("helper"),
            None,
            ImportKind::ReExport,
            true,
        ),
        (
            "./helpers",
            Some("other"),
            Some("renamed"),
            ImportKind::ReExport,
            true,
        ),
        ("./all", None, None, ImportKind::ReExport, true),
        ("./lazy", None, None, ImportKind::Dynamic, false),
        ("./legacy", None, None, ImportKind::Require, false),
    ] {
        assert!(imports.contains(&expected), "{expected:?} in {imports:?}");
    }
    let refs: Vec<(&str, Option<&str>, RefKind, Option<&str>)> = facts
        .refs
        .iter()
        .map(|r| {
            (
                r.name.as_str(),
                r.qualifier.as_deref(),
                r.kind,
                r.caller.as_deref(),
            )
        })
        .collect();
    assert!(
        refs.contains(&("useS", None, RefKind::Call, Some("Button"))),
        "{refs:?}"
    );
    assert!(refs.contains(&("Icon", None, RefKind::Use, Some("Button"))));
    assert!(refs.contains(&("get", Some("api"), RefKind::Method, Some("App"))));
    assert!(refs.contains(&("Store", None, RefKind::Call, Some("App"))));
    assert!(refs.contains(&("save", Some("this"), RefKind::Method, Some("Store::open"))));
    assert!(refs.contains(&("Props", None, RefKind::Type, Some("Button"))));
}

#[test]
fn python_and_go_definitions_and_imports_are_read() {
    let py = br#"
import os, json as j
from . import sibling
from ..pkg.mod import thing as t, other
from x import *
MAX_SIZE = 10
class Store(Base):
    def open(self):
        helper()
        self.save()
def _private(): pass
def helper(): os.path.join("a")
"#;
    let facts = extract(Lang::Python, py).unwrap();
    let names: Vec<(&str, SymbolKind, bool)> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.exported))
        .collect();
    for expected in [
        ("MAX_SIZE", SymbolKind::Constant, true),
        ("Store", SymbolKind::Class, true),
        ("open", SymbolKind::Method, true),
        ("_private", SymbolKind::Function, false),
        ("helper", SymbolKind::Function, true),
    ] {
        assert!(names.contains(&expected), "{expected:?} in {names:?}");
    }
    let imports: Vec<(&str, Option<&str>, Option<&str>, ImportKind)> = facts
        .imports
        .iter()
        .map(|i| {
            (
                i.spec.as_str(),
                i.name.as_deref(),
                i.alias.as_deref(),
                i.kind,
            )
        })
        .collect();
    for expected in [
        ("os", None, None, ImportKind::Use),
        ("json", None, Some("j"), ImportKind::Use),
        (".", Some("sibling"), None, ImportKind::Use),
        ("..pkg.mod", Some("thing"), Some("t"), ImportKind::Use),
        ("..pkg.mod", Some("other"), None, ImportKind::Use),
        ("x", None, None, ImportKind::Wildcard),
    ] {
        assert!(imports.contains(&expected), "{expected:?} in {imports:?}");
    }
    assert!(facts.refs.iter().any(|r| r.name == "helper"
        && r.kind == RefKind::Call
        && r.caller.as_deref() == Some("Store::open")));

    let go_src = br#"
package store

import (
    "fmt"
    st "example.com/proj/internal/state"
)

const Limit = 3
type Store struct { s st.State }
type Reader interface { Read() int }
func (s *Store) Open() { fmt.Println(helper()) }
func helper() int { return 1 }
"#;
    let facts = extract(Lang::Go, go_src).unwrap();
    assert_eq!(facts.package.as_deref(), Some("store"));
    let names: Vec<(&str, SymbolKind, bool, Option<&str>)> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.exported, s.container.as_deref()))
        .collect();
    for expected in [
        ("Limit", SymbolKind::Constant, true, None),
        ("Store", SymbolKind::Struct, true, None),
        ("Reader", SymbolKind::Interface, true, None),
        ("Read", SymbolKind::Method, true, Some("Reader")),
        ("Open", SymbolKind::Method, true, Some("Store")),
        ("helper", SymbolKind::Function, false, None),
    ] {
        assert!(names.contains(&expected), "{expected:?} in {names:?}");
    }
    assert!(facts
        .imports
        .iter()
        .any(|i| i.spec == "example.com/proj/internal/state" && i.alias.as_deref() == Some("st")));
    assert!(facts
        .refs
        .iter()
        .any(|r| r.name == "Println" && r.qualifier.as_deref() == Some("fmt")));
}

#[test]
fn a_file_with_syntax_errors_still_gives_what_it_can() {
    let facts = extract(Lang::Rust, b"pub fn ok() {}\nfn broken( {\n").unwrap();
    assert!(facts.had_errors);
    assert!(facts.symbols.iter().any(|s| s.name == "ok"));
}

// ---- the project model ------------------------------------------------------

#[test]
fn rust_modules_follow_mod_trees_with_path_attributes_and_macro_paths() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"demo-app\"\n");
    write(
        root,
        "src/lib.rs",
        "pub mod store;\n#[path = \"generated/schema_v2.rs\"]\npub mod schema;\nmod util;\npub use store::Store;\n",
    );
    write(root, "src/store.rs", "pub mod index;\npub struct Store;\nimpl Store { pub fn open() -> Self { crate::schema::check(); Store } }\n");
    write(
        root,
        "src/store/index.rs",
        "use super::Store;\npub fn rebuild() { log_it!(crate::util::note()); }\n",
    );
    write(root, "src/generated/schema_v2.rs", "pub fn check() {}\n");
    write(
        root,
        "src/util.rs",
        "pub fn note() {}\n#[macro_export]\nmacro_rules! log_it { ($e:expr) => { $e } }\n",
    );
    let map = built(root, data.path());
    let model = map.model().unwrap();
    let module = |path: &str| model.files[model.by_path[path]].module.clone();
    assert_eq!(module("src/lib.rs"), "demo_app");
    assert_eq!(module("src/store.rs"), "demo_app::store");
    assert_eq!(module("src/store/index.rs"), "demo_app::store::index");
    assert_eq!(
        module("src/generated/schema_v2.rs"),
        "demo_app::schema",
        "a #[path] module is where the compiler puts it"
    );
    let has = |from: &str, to: &str| model.edges.iter().any(|e| e.from == from && e.to == to);
    assert!(
        has("demo_app::store", "demo_app::schema"),
        "{:?}",
        model.edges
    );
    assert!(
        has("demo_app::store::index", "demo_app::util"),
        "a path inside a macro call is a dependency"
    );
    // `pub use store::Store;` makes demo_app::Store the one in store.rs.
    let found = map.definitions("demo_app::Store", None, None, 5).unwrap();
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].path, "src/store.rs");
    assert_eq!(found.matches[0].line, 2);
}

#[test]
fn typescript_aliases_index_files_and_reexports_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "web/tsconfig.json",
        "{\n  // comments and trailing commas, as tsconfig allows\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    \"paths\": { \"@/*\": [\"./src/*\"], },\n  },\n}\n",
    );
    write(root, "web/src/App.tsx", "import { Button } from '@/ui';\nimport { fetchAll } from '@/lib/api.ts';\nexport function App() { fetchAll(); return <Button />; }\n");
    write(
        root,
        "web/src/ui/index.ts",
        "export { Button } from './Button';\n",
    );
    write(
        root,
        "web/src/ui/Button.tsx",
        "export const Button = () => null;\n",
    );
    write(
        root,
        "web/src/lib/api.ts",
        "import { App } from '../App';\nexport async function fetchAll() { return App; }\n",
    );
    let map = built(root, data.path());
    let model = map.model().unwrap();
    let has = |from: &str, to: &str| model.edges.iter().any(|e| e.from == from && e.to == to);
    assert!(has("web/src/App", "web/src/ui"), "{:?}", model.edges);
    assert!(has("web/src/App", "web/src/lib/api"));
    assert!(
        has("web/src/ui", "web/src/ui/Button"),
        "a re-export is an import"
    );
    assert!(model.cycles().contains(&vec![
        "web/src/App".to_string(),
        "web/src/lib/api".to_string()
    ]));
    let refs = map.references("fetchAll", None, None, 50).unwrap();
    assert_eq!(refs.definitions[0].path, "web/src/lib/api.ts");
    assert!(refs
        .resolved
        .iter()
        .any(|r| r.path == "web/src/App.tsx" && r.line == 3 && r.caller.as_deref() == Some("App")));
}

#[test]
fn python_and_go_modules_resolve_inside_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/pkg/__init__.py", "");
    write(
        root,
        "src/pkg/core.py",
        "from .util import tidy\nimport pkg.util\ndef run():\n    tidy()\n",
    );
    write(root, "src/pkg/util.py", "def tidy():\n    pass\n");
    write(root, "go.mod", "module example.com/proj\n\ngo 1.22\n");
    write(
        root,
        "cmd/main.go",
        "package main\nimport \"example.com/proj/internal/state\"\nfunc main() { state.Load() }\n",
    );
    write(
        root,
        "internal/state/state.go",
        "package state\nfunc Load() {}\n",
    );
    let map = built(root, data.path());
    let model = map.model().unwrap();
    let has = |from: &str, to: &str| model.edges.iter().any(|e| e.from == from && e.to == to);
    assert!(has("src/pkg/core", "src/pkg/util"), "{:?}", model.edges);
    assert!(has(
        "example.com/proj/cmd",
        "example.com/proj/internal/state"
    ));
    let refs = map.references("Load", None, None, 50).unwrap();
    assert!(refs
        .resolved
        .iter()
        .any(|r| r.path == "cmd/main.go" && r.how == "through an imported module"));
    let refs = map.references("tidy", None, None, 50).unwrap();
    assert!(refs
        .resolved
        .iter()
        .any(|r| r.path == "src/pkg/core.py" && r.line == 4));
}

// ---- refreshing ---------------------------------------------------------------

#[test]
fn a_refresh_reads_only_what_changed_and_drops_what_was_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/a.rs", "pub fn alpha() {}\n");
    write(root, "src/b.rs", "pub fn beta() { alpha(); }\n");
    let map = CodeMap::open(data.path(), root).unwrap();
    let first = map.refresh(&go, &mut |_, _| {}).unwrap();
    assert_eq!(first.read, 2);
    assert!(map.is_complete());
    // Files written just now are hashed again (their time may not have
    // moved), but unchanged contents are not read again.
    let again = map.refresh(&go, &mut |_, _| {}).unwrap();
    assert_eq!(again.read, 0);
    assert_eq!(again.unchanged, 2);

    write(root, "src/a.rs", "pub fn alpha_renamed() {}\n");
    let changed = map.refresh(&go, &mut |_, _| {}).unwrap();
    assert_eq!(changed.read, 1, "only the changed file is read");
    assert!(map
        .definitions("alpha", None, None, 5)
        .unwrap()
        .matches
        .is_empty());
    assert_eq!(
        map.definitions("alpha_renamed", None, None, 5)
            .unwrap()
            .matches
            .len(),
        1
    );

    std::fs::remove_file(root.join("src/b.rs")).unwrap();
    let deleted = map.refresh(&go, &mut |_, _| {}).unwrap();
    assert_eq!(deleted.removed, 1);
    assert!(map
        .definitions("beta", None, None, 5)
        .unwrap()
        .matches
        .is_empty());
    assert!(map
        .references("alpha", None, None, 5)
        .unwrap()
        .by_name
        .is_empty());
    assert_eq!(map.status().unwrap().files, 1);

    // A map reopened later keeps what it read.
    drop(map);
    let reopened = CodeMap::open(data.path(), root).unwrap();
    assert_eq!(reopened.status().unwrap().files, 1);
    reopened.clear().unwrap();
    assert_eq!(reopened.status().unwrap().files, 0);
}

#[test]
fn a_refresh_stops_and_holds_when_its_caller_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    for i in 0..5 {
        write(dir.path(), &format!("f{i}.py"), "def f(): pass\n");
    }
    let map = CodeMap::open(data.path(), dir.path()).unwrap();
    let stop = || Pace::Stop;
    let stopped = map.refresh(&stop, &mut |_, _| {}).unwrap();
    assert!(stopped.stopped);
    assert_eq!(stopped.read, 0);
    assert!(
        !map.is_complete(),
        "a stopped refresh is not a complete map"
    );
    let asked = std::sync::atomic::AtomicUsize::new(0);
    let hold_once = || {
        if asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Pace::Hold
        } else {
            Pace::Go
        }
    };
    let mut seen = Vec::new();
    let done = map
        .refresh(&hold_once, &mut |d, t| seen.push((d, t)))
        .unwrap();
    assert_eq!(done.read, 5);
    assert_eq!(seen.last(), Some(&(5, 5)));
}

#[test]
fn ignored_dependency_build_and_sensitive_files_are_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "--quiet"]);
    write(root, ".gitignore", "generated/\n");
    write(root, "src/a.rs", "pub fn a() {}\n");
    write(root, "generated/big.rs", "pub fn generated() {}\n");
    write(root, "node_modules/pkg/index.js", "export const no = 1;\n");
    write(root, "target/debug/build.rs", "pub fn built() {}\n");
    write(root, ".env.ts", "export const KEY = 'secret';\n");
    write(root, "config/secrets.js", "export const k = 1;\n");
    write(root, ".ssh/helper.py", "def k(): pass\n");
    write(root, "huge.rs", &"pub fn x() {}\n".repeat(60_000));
    let map = built(root, data.path());
    let model = map.model().unwrap();
    let paths: Vec<&str> = model.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["huge.rs", "src/a.rs"], "{paths:?}");
    let huge = &model.files[model.by_path["huge.rs"]];
    assert!(huge
        .skipped
        .as_deref()
        .is_some_and(|s| s.contains("not read")));
    assert!(map
        .definitions("x", None, None, 5)
        .unwrap()
        .matches
        .is_empty());
    assert!(walk::classify(".git/config").is_none());
    assert!(walk::classify(".env").is_none());
    assert!(walk::classify("config/.env.production.js").is_none());
    assert!(walk::classify("certs/id_rsa").is_none());
    assert!(walk::classify("src/main.rs").is_some());
}

// ---- receipts ---------------------------------------------------------------

#[test]
fn the_queries_answer_with_file_and_line_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"shop\"\n");
    write(root, "src/lib.rs", "pub mod cart;\npub mod checkout;\n");
    write(root, "src/cart.rs", "pub struct Cart;\nimpl Cart {\n    pub fn total(&self) -> u32 { 0 }\n}\npub fn empty() -> Cart { Cart }\n");
    write(root, "src/checkout.rs", "use crate::cart::{self, Cart};\npub fn pay(c: &Cart) -> u32 {\n    let _ = cart::empty();\n    c.total()\n}\n");
    let map = built(root, data.path());

    let found = map.definitions("Cart::total", None, None, 5).unwrap();
    assert_eq!(found.matches.len(), 1);
    let total = &found.matches[0];
    assert_eq!(
        (total.path.as_str(), total.line, total.kind),
        ("src/cart.rs", 3, "method")
    );
    assert_eq!(total.module, "shop::cart");
    let missing = map.definitions("Crat", None, None, 5).unwrap();
    assert!(missing.matches.is_empty());
    let similar = map.definitions("tota", None, None, 5).unwrap();
    assert!(similar.similar.iter().any(|s| s.name == "total"));

    let refs = map.references("empty", None, None, 50).unwrap();
    let receipt: Vec<(&str, u32, &str, Option<&str>)> = refs
        .resolved
        .iter()
        .map(|r| (r.path.as_str(), r.line, r.how, r.caller.as_deref()))
        .collect();
    assert!(
        receipt.contains(&("src/checkout.rs", 3, "through its module path", Some("pay"))),
        "{receipt:?}"
    );
    let uses = map.references("Cart", None, None, 50).unwrap();
    assert!(uses
        .resolved
        .iter()
        .any(|r| r.path == "src/checkout.rs" && r.line == 2 && r.how == "imported"));
    let methods = map.references("total", None, None, 50).unwrap();
    assert!(
        methods
            .by_name
            .iter()
            .any(|r| r.path == "src/checkout.rs" && r.line == 4),
        "a method call on a value is a use by name only"
    );

    let outline = map.outline("src/cart.rs").unwrap();
    let symbols: Vec<(&str, u32)> = outline.files[0]
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.line))
        .collect();
    assert_eq!(symbols, vec![("Cart", 1), ("total", 3), ("empty", 5)]);
    assert_eq!(
        map.outline("shop::checkout").unwrap().files[0].path,
        "src/checkout.rs"
    );
    assert!(map.outline("nope.rs").is_err());

    let deps = map.dependencies("src/checkout.rs").unwrap();
    assert_eq!(deps[0].module, "shop::checkout");
    assert_eq!(
        deps[0].imports,
        vec![DepEdge {
            module: "shop::cart".into(),
            path: "src/checkout.rs".into(),
            line: 1
        }]
    );
    let back = map.dependencies("shop::cart").unwrap();
    assert_eq!(back[0].imported_by[0].module, "shop::checkout");
    assert!(back[0].cycles.is_empty());
    let jump = map.symbols_matching("ca", 10).unwrap();
    assert_eq!(jump[0].name, "Cart");
}

// ---- the Architecture review's view ---------------------------------------------

#[test]
fn rust_files_become_modules_with_imports_public_items_and_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
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
    let view = MapStructure::in_dir(data.path()).build(root, root).unwrap();
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
    let data = tempfile::tempdir().unwrap();
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
    let view = MapStructure::in_dir(data.path())
        .build(root, &root.join("src"))
        .unwrap();
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
    let b = view
        .files
        .iter()
        .find(|f| f.path == "src/lib/b.tsx")
        .unwrap();
    assert_eq!(
        (b.language, b.public.clone()),
        ("tsx", vec!["const b".to_string()])
    );
}

#[test]
fn a_folder_outside_the_code_folder_is_refused() {
    let outer = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = outer.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    assert!(MapStructure::in_dir(data.path())
        .build(&root, outer.path())
        .is_err());
}

#[test]
fn in_a_repository_the_walk_follows_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "--quiet"]);
    write(root, ".gitignore", "generated/\n");
    write(root, "src/a.rs", "pub fn a() {}\n");
    write(root, "generated/big.rs", "pub fn generated() {}\n");
    let view = MapStructure::in_dir(data.path()).build(root, root).unwrap();
    let paths: Vec<&str> = view.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["src/a.rs"], "ignored files are never read");
    assert!(!view.partial);
}

#[test]
fn the_open_map_reads_the_view_after_bringing_itself_up_to_date() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"demo\"\n");
    write(root, "src/lib.rs", "pub mod sum;\n");
    write(root, "src/sum.rs", "pub fn sum() {}\n");
    let map = CodeMap::open(data.path(), root).unwrap();
    let view = map.build(root, root).unwrap();
    assert_eq!(
        view.modules,
        vec!["demo".to_string(), "demo::sum".to_string()]
    );
    write(
        root,
        "src/other.rs",
        "use crate::sum::sum;\npub fn other() { sum(); }\n",
    );
    write(root, "src/lib.rs", "pub mod sum;\npub mod other;\n");
    let view = map.build(root, root).unwrap();
    assert!(
        view.has_edge("demo::other", "demo::sum"),
        "{:?}",
        view.edges
    );
    assert!(map.build(dir.path(), &dir.path().join("..")).is_err());
}

// ---- the review's fix round ----------------------------------------------------

#[test]
fn a_qualified_call_is_tied_only_through_its_qualifier() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"shop\"\n");
    write(root, "src/lib.rs", "pub mod store;\npub mod other;\n");
    write(
        root,
        "src/store.rs",
        "pub struct Store;\nimpl Store {\n    pub fn new() -> Self { let _v: Vec<u8> = Vec::new(); Self::new_inner() }\n    fn new_inner() -> Self { Store }\n    pub fn open() -> Self { Self::new() }\n}\npub fn build() -> Store { Store::new() }\n",
    );
    write(
        root,
        "src/other.rs",
        "use crate::store::Store;\npub struct Other;\nimpl Other { pub fn new() -> Self { Other } }\npub fn make() { let _s = Store::new(); let _o = Other::new(); let _m = std::collections::HashMap::<u8, u8>::new(); }\n",
    );
    let map = built(root, data.path());
    let refs = map.references("Store::new", None, None, 50).unwrap();
    let resolved: Vec<(&str, u32, Option<&str>)> = refs
        .resolved
        .iter()
        .map(|r| (r.path.as_str(), r.line, r.qualifier.as_deref()))
        .collect();
    assert!(
        resolved.contains(&("src/store.rs", 5, Some("Self"))),
        "{resolved:?}"
    );
    assert!(
        resolved.contains(&("src/store.rs", 7, Some("Store"))),
        "{resolved:?}"
    );
    assert!(
        resolved.contains(&("src/other.rs", 4, Some("Store"))),
        "{resolved:?}"
    );
    for wrong in ["Vec", "Other", "std::collections::HashMap"] {
        assert!(
            !refs
                .resolved
                .iter()
                .any(|r| r.qualifier.as_deref() == Some(wrong)),
            "{wrong}::new() is never a proven use of Store::new: {resolved:?}"
        );
    }
    assert!(refs
        .by_name
        .iter()
        .any(|r| r.qualifier.as_deref() == Some("Vec")));
    assert_eq!(
        refs.resolved_total + refs.by_name_total,
        refs.uses_named as usize
    );
}

#[test]
fn every_use_is_counted_and_the_nearest_are_listed_first() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"many\"\n");
    write(
        root,
        "src/lib.rs",
        "pub mod a;\npub mod far;\npub fn ping() {}\n",
    );
    let calls: String = (0..300)
        .map(|i| format!("fn c{i}() {{ crate::ping(); }}\n"))
        .collect();
    write(root, "src/a.rs", &calls);
    write(
        root,
        "src/far.rs",
        "pub fn ping() {}\npub fn local() { ping(); }\n",
    );
    let map = built(root, data.path());
    let refs = map.references("crate::ping", None, None, 10).unwrap();
    assert_eq!(refs.uses_named, 301);
    assert_eq!(refs.uses_scanned, 301);
    assert_eq!(refs.resolved.len(), 10);
    assert_eq!(
        refs.resolved_total, 300,
        "every call through the path is counted"
    );
    assert!(refs.truncated);
    let narrowed = map
        .references("crate::ping", None, Some("src/far.rs"), 10)
        .unwrap();
    assert_eq!(narrowed.uses_named, 1);
    assert_eq!(narrowed.resolved_total, 0);
    assert_eq!(
        narrowed.by_name_total, 1,
        "far.rs calls its own ping, not this one"
    );
    // Definitions are filtered before they are cut.
    let found = map
        .definitions("ping", None, Some("src/far.rs"), 1)
        .unwrap();
    assert_eq!(
        (found.matches[0].path.as_str(), found.total),
        ("src/far.rs", 1)
    );
    assert_eq!(map.definitions("ping", None, None, 1).unwrap().total, 2);
}

#[test]
fn a_refresh_that_waits_holds_nothing_and_another_reads_the_edit_meanwhile() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    write(&root, "src/a.rs", "pub fn first() {}\n");
    let map = Arc::new(built(&root, data.path()));
    // A background refresh held behind other indexing, as long as `hold` is set.
    let hold = Arc::new(AtomicBool::new(true));
    let waiting = {
        let (map, hold) = (map.clone(), hold.clone());
        std::thread::spawn(move || {
            let pace = || {
                if hold.load(Ordering::SeqCst) {
                    Pace::Hold
                } else {
                    Pace::Go
                }
            };
            map.refresh(&pace, &mut |_, _| {}).unwrap()
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(map.is_refreshing());
    write(&root, "src/a.rs", "\n\n\npub fn first() {}\n");
    let started = std::time::Instant::now();
    let report = map.refresh(&go, &mut |_, _| {}).unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "never kept waiting behind the held refresh"
    );
    assert_eq!(report.read, 1);
    let found = map.definitions("first", None, None, 5).unwrap();
    assert_eq!(
        found.matches[0].line, 4,
        "the edited file's lines, not the old ones"
    );
    hold.store(false, Ordering::SeqCst);
    waiting.join().unwrap();
}

#[test]
fn a_view_of_a_map_not_yet_complete_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"demo\"\n");
    write(root, "src/lib.rs", "pub mod a;\n");
    write(root, "src/a.rs", "pub fn a() {}\n");
    let map = CodeMap::open(data.path(), root).unwrap();
    let stop = || Pace::Stop;
    map.refresh(&stop, &mut |_, _| {}).unwrap();
    let view = map.structure(".").unwrap();
    let why = view
        .incomplete
        .clone()
        .expect("an incomplete map's view says so");
    assert!(why.contains("still being built"), "{why}");
    assert!(view.render().contains("INCOMPLETE VIEW"));
    // A refresh cut off by its deadline says so too.
    let view = map
        .review_view(root, root, std::time::Duration::ZERO)
        .unwrap();
    assert!(view.incomplete.is_some());
    // Complete: nothing to say.
    let view = map
        .review_view(root, root, std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(view.incomplete, None);
    assert!(!view.render().contains("INCOMPLETE VIEW"));
}

#[test]
fn a_cut_listing_keeps_what_it_did_not_reach_and_always_lists_configs() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.py", "def a(): pass\n");
    write(root, "b.py", "def b(): pass\n");
    write(root, "c.py", "def c(): pass\n");
    write(root, "z/tsconfig.json", "{}\n");
    built(root, data.path());
    let limited = CodeMap::open(data.path(), root)
        .unwrap()
        .with_limits(walk::Limits {
            max_files: 2,
            ..walk::Limits::default()
        });
    let report = limited.refresh(&go, &mut |_, _| {}).unwrap();
    assert!(report.partial);
    assert_eq!(
        report.removed, 0,
        "a file past the cut is not taken for deleted"
    );
    assert_eq!(
        limited
            .definitions("c", None, None, 5)
            .unwrap()
            .matches
            .len(),
        1
    );
    let listing = walk::list_with(
        root,
        &walk::Limits {
            max_files: 2,
            ..walk::Limits::default()
        },
    )
    .unwrap();
    assert!(
        listing.files.iter().any(|f| f.path() == "z/tsconfig.json"),
        "configs past the cut are listed"
    );
    assert_eq!(listing.cut.as_deref(), Some("c.py"));
}

#[test]
fn a_rename_is_taken_back_before_anything_else_even_when_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/store.rs", "pub struct Store;\n");
    let map = built(root, data.path());
    std::fs::rename(root.join("src/store.rs"), root.join("src/storage.rs")).unwrap();
    let stop = || Pace::Stop;
    let report = map.refresh(&stop, &mut |_, _| {}).unwrap();
    assert!(report.stopped);
    assert_eq!(report.removed, 1);
    assert!(
        map.definitions("Store", None, None, 5)
            .unwrap()
            .matches
            .is_empty(),
        "the old file is gone at once; the new one is read next time"
    );
}

#[test]
fn an_unchanged_refresh_keeps_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    write(dir.path(), "a.py", "def a(): pass\n");
    let map = built(dir.path(), data.path());
    map.model().unwrap();
    assert!(map.model_is_current());
    map.refresh(&go, &mut |_, _| {}).unwrap();
    assert!(map.model_is_current(), "touched times change no answer");
    write(dir.path(), "a.py", "def b(): pass\n");
    map.refresh(&go, &mut |_, _| {}).unwrap();
    assert!(!map.model_is_current(), "a changed file does");
}

#[test]
fn a_slow_or_minified_file_is_listed_not_read() {
    let big: String = (0..20_000)
        .map(|i| format!("fn f{i}() {{ g({i}); }}\n"))
        .collect();
    let timed_out = extract::extract_within(
        Lang::Rust,
        big.as_bytes(),
        std::time::Duration::ZERO,
        &|| false,
    );
    assert!(timed_out.unwrap_err().contains("longer than"));
    let stopped = extract::extract_within(
        Lang::Rust,
        big.as_bytes(),
        std::time::Duration::from_secs(60),
        &|| true,
    );
    assert!(stopped.unwrap_err().contains("stopped"));
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let minified = format!("{}\n", "var a=1;".repeat(4_000));
    write(dir.path(), "dist2/app.min.js", &minified);
    let map = built(dir.path(), data.path());
    let model = map.model().unwrap();
    let file = &model.files[model.by_path["dist2/app.min.js"]];
    assert!(
        file.skipped
            .as_deref()
            .is_some_and(|s| s.contains("minified")),
        "{:?}",
        file.skipped
    );
}

#[test]
fn python_prefers_the_scripts_own_folder_and_ties_nothing_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a/utils.py", "def helper(): pass\n");
    write(root, "b/utils.py", "def helper(): pass\n");
    write(root, "b/main.py", "import utils\nutils.helper()\n");
    write(root, "c/main.py", "import utils\nutils.helper()\n");
    let map = built(root, data.path());
    let model = map.model().unwrap();
    let has = |from: &str, to: &str| model.edges.iter().any(|e| e.from == from && e.to == to);
    assert!(has("b/main", "b/utils"), "{:?}", model.edges);
    assert!(!has("b/main", "a/utils"));
    assert!(
        !model.edges.iter().any(|e| e.from == "c/main"),
        "two utils: neither is assumed"
    );
}

#[test]
fn local_modules_win_over_crate_names_and_a_binary_is_its_own_crate() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "config/Cargo.toml", "[package]\nname = \"config\"\n");
    write(root, "config/src/lib.rs", "pub fn load() {}\n");
    write(
        root,
        "app/Cargo.toml",
        "[package]\nname = \"app\"\n\n[dependencies]\nserde = \"1\"\n",
    );
    write(root, "app/src/lib.rs", "pub mod config;\npub mod x;\n");
    write(root, "app/src/config.rs", "pub fn load() {}\n");
    write(
        root,
        "app/src/x.rs",
        "pub fn run() { config::load(); crate::config::load(); }\n",
    );
    write(
        root,
        "app/src/main.rs",
        "mod cli;\nfn main() { crate::cli::go(); app::x::run(); }\n",
    );
    write(root, "app/src/cli.rs", "pub fn go() {}\n");
    write(
        root,
        "tool/Cargo.toml",
        "[package]\nname = \"tool\"\n\n[dependencies]\nconfig = { workspace = true }\n",
    );
    write(root, "tool/src/main.rs", "fn main() { config::load(); }\n");
    let map = built(root, data.path());
    let model = map.model().unwrap();
    let module = |path: &str| model.files[model.by_path[path]].module.clone();
    assert_eq!(
        module("app/src/main.rs"),
        "app::main",
        "a binary beside a library is its own crate"
    );
    assert_eq!(module("app/src/cli.rs"), "app::main::cli");
    let has = |from: &str, to: &str| model.edges.iter().any(|e| e.from == from && e.to == to);
    assert!(
        has("app::main", "app::main::cli") || !has("app::main", "app::cli"),
        "{:?}",
        model.edges
    );
    assert!(
        has("app::main", "app::x"),
        "the binary reaches the library by name"
    );
    assert!(
        !has("app::x", "config"),
        "app does not depend on the config crate: {:?}",
        model.edges
    );
    assert!(
        has("tool", "config"),
        "a workspace dependency is a dependency"
    );
    let refs = map
        .references("config::load", Some("config/src/lib.rs"), None, 20)
        .unwrap();
    assert!(
        !refs.resolved.iter().any(|r| r.path == "app/src/x.rs"),
        "{refs:?}"
    );
}

#[test]
fn outside_git_the_walk_still_honours_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, ".gitignore", "scratch/\nlocal_settings.py\n");
    write(root, "app.py", "def run(): pass\n");
    write(root, "local_settings.py", "SECRET_NAME = 1\n");
    write(root, "scratch/try.py", "def t(): pass\n");
    let map = built(root, data.path());
    let model = map.model().unwrap();
    let paths: Vec<&str> = model.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["app.py"]);
}

#[test]
fn a_damaged_map_file_is_moved_aside_and_built_again() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    write(dir.path(), "a.py", "def a(): pass\n");
    let file = CodeMap::map_path(data.path(), dir.path()).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        b"this is not a database at all, not even close to one",
    )
    .unwrap();
    let map = built(dir.path(), data.path());
    assert_eq!(
        map.definitions("a", None, None, 5).unwrap().matches.len(),
        1
    );
    let aside = std::fs::read_dir(file.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().contains("corrupt"));
    assert!(aside, "the damaged file is kept aside");
}
