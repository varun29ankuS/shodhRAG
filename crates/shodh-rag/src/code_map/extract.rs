//! What one source file says, read from its tree-sitter syntax tree: the
//! symbols it defines, the names it calls or uses, what it imports, and (in
//! Rust) the modules it declares.
//!
//! Everything here is a pure function of the file's language and bytes, so
//! the map stores it keyed by the file's content hash and never has to look
//! at another file to keep it current. Resolving names across files is done
//! when the map is asked ([`super::model`]).
//!
//! The syntax tree is walked with an explicit stack, never by recursion, so a
//! deeply nested file cannot overflow the thread's stack. Rust macro bodies
//! are token trees that tree-sitter does not parse further; their tokens are
//! scanned for calls and `crate::`/`super::`/`self::` paths.

use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tree_sitter::{Node, ParseOptions, ParseState, Parser, Point};

use super::lang::Lang;

/// References kept per file at most (the rest of a generated file is noise).
pub const MAX_REFS_PER_FILE: usize = 20_000;
/// Symbols kept per file at most.
pub const MAX_SYMBOLS_PER_FILE: usize = 10_000;
/// Imports kept per file at most.
pub const MAX_IMPORTS_PER_FILE: usize = 5_000;
/// Characters of a call's receiver kept (`self.store.items` in `self.store.items.push()`).
const MAX_QUALIFIER_CHARS: usize = 120;
/// Characters of a definition's first line kept as its signature.
const MAX_SIGNATURE_CHARS: usize = 200;
/// Tokens of one macro body scanned at most.
const MAX_TOKENS_SCANNED: usize = 50_000;
/// Syntax nodes of one file walked at most.
const MAX_NODES: usize = 2_000_000;

/// What a symbol is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Union,
    Class,
    Enum,
    Variant,
    Trait,
    Interface,
    Type,
    Module,
    Constant,
    Static,
    Variable,
    Macro,
}

const SYMBOL_KINDS: [SymbolKind; 15] = [
    SymbolKind::Function,
    SymbolKind::Method,
    SymbolKind::Struct,
    SymbolKind::Union,
    SymbolKind::Class,
    SymbolKind::Enum,
    SymbolKind::Variant,
    SymbolKind::Trait,
    SymbolKind::Interface,
    SymbolKind::Type,
    SymbolKind::Module,
    SymbolKind::Constant,
    SymbolKind::Static,
    SymbolKind::Variable,
    SymbolKind::Macro,
];

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Struct => "struct",
            SymbolKind::Union => "union",
            SymbolKind::Class => "class",
            SymbolKind::Enum => "enum",
            SymbolKind::Variant => "variant",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::Type => "type",
            SymbolKind::Module => "module",
            SymbolKind::Constant => "constant",
            SymbolKind::Static => "static",
            SymbolKind::Variable => "variable",
            SymbolKind::Macro => "macro",
        }
    }

    pub fn parse(name: &str) -> Option<SymbolKind> {
        SYMBOL_KINDS.into_iter().find(|k| k.as_str() == name)
    }

    /// Every kind's name, for a tool schema.
    pub fn names() -> Vec<&'static str> {
        SYMBOL_KINDS.iter().map(|k| k.as_str()).collect()
    }

    /// Belongs to a type (a method, an enum variant), not to a module.
    pub fn is_member(self) -> bool {
        matches!(self, SymbolKind::Method | SymbolKind::Variant)
    }
}

/// How a name is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefKind {
    /// `f()`, `a::f()`, `new F()`.
    Call,
    /// `x.f()`: a call on a value whose type the map does not know.
    Method,
    /// `m!()`.
    Macro,
    /// A type in a signature, a field or a generic argument.
    Type,
    /// Any other use: `a::CONST`, `<Component />`, an enum variant.
    Use,
}

const REF_KINDS: [RefKind; 5] = [
    RefKind::Call,
    RefKind::Method,
    RefKind::Macro,
    RefKind::Type,
    RefKind::Use,
];

impl RefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RefKind::Call => "call",
            RefKind::Method => "method",
            RefKind::Macro => "macro",
            RefKind::Type => "type",
            RefKind::Use => "use",
        }
    }

    pub fn parse(name: &str) -> Option<RefKind> {
        REF_KINDS.into_iter().find(|k| k.as_str() == name)
    }
}

/// How something is imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ImportKind {
    /// `use a::b::C`, `import { C } from './b'`, `from b import C`, `import "b"`.
    Use,
    /// `use a::*`, `import * as ns from`, `from b import *`.
    Wildcard,
    /// `export { C } from './b'`, `export * from './b'`.
    ReExport,
    /// `require('./b')`.
    Require,
    /// `import('./b')`.
    Dynamic,
    /// `extern crate b`.
    ExternCrate,
    /// A Rust path written in code (`crate::a::f()`, a type `super::B`, a path
    /// inside a macro): a dependency on its module without a `use`.
    Path,
}

const IMPORT_KINDS: [ImportKind; 7] = [
    ImportKind::Use,
    ImportKind::Wildcard,
    ImportKind::ReExport,
    ImportKind::Require,
    ImportKind::Dynamic,
    ImportKind::ExternCrate,
    ImportKind::Path,
];

impl ImportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportKind::Use => "use",
            ImportKind::Wildcard => "wildcard",
            ImportKind::ReExport => "reexport",
            ImportKind::Require => "require",
            ImportKind::Dynamic => "dynamic",
            ImportKind::ExternCrate => "extern_crate",
            ImportKind::Path => "path",
        }
    }

    pub fn parse(name: &str) -> Option<ImportKind> {
        IMPORT_KINDS.into_iter().find(|k| k.as_str() == name)
    }
}

/// A definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// The keyword that defines it in its language (`fn`, `struct`,
    /// `function`, `const`, `def`, `func`); empty when there is none.
    pub keyword: &'static str,
    /// 1-based.
    pub line: u32,
    pub end_line: u32,
    /// The type or class a method or variant belongs to.
    pub container: Option<String>,
    /// Rust: the inline modules it is inside (`tests`, `a::b`); empty at the
    /// file's own level.
    pub scope: String,
    /// Visible outside its module: `pub` (not `pub(crate)`), `export`, a
    /// Python name without a leading `_`, a Go name with a capital.
    pub exported: bool,
    /// Its first line(s) up to the body, whitespace collapsed.
    pub signature: String,
}

/// A use of a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The name used (the last part of a path).
    pub name: String,
    /// What it was reached through: a path (`crate::store`, `Store`), an
    /// import alias, or for a method call the value's text (`self.items`).
    pub qualifier: Option<String>,
    pub line: u32,
    pub kind: RefKind,
    /// The definition the use is inside (`Store::open`, `render`).
    pub caller: Option<String>,
    pub scope: String,
}

/// One import (a Rust `use` tree is one per leaf).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Rust: the full path (`crate::a::B`, `super::c::*`). Scripts: the module
    /// specifier as written (`./lib/b`, `@/x`, `react`). Python: the module
    /// (`pkg.mod`, `..sibling`). Go: the import path.
    pub spec: String,
    /// The name imported (`B`), `default` for a default import; none for a
    /// whole module.
    pub name: Option<String>,
    pub alias: Option<String>,
    pub line: u32,
    pub kind: ImportKind,
    /// Re-exported: `pub use`, `export … from`.
    pub public: bool,
    pub scope: String,
}

/// A Rust `mod` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDecl {
    pub name: String,
    pub line: u32,
    /// `#[path = "…"]` on it.
    pub path_attr: Option<String>,
    /// The inline modules it is declared inside.
    pub scope: String,
    /// `mod x { … }` rather than `mod x;`.
    pub inline: bool,
}

/// Everything the map keeps of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub symbols: Vec<Symbol>,
    pub refs: Vec<Reference>,
    pub imports: Vec<Import>,
    pub mods: Vec<ModDecl>,
    /// Go: the package clause.
    pub package: Option<String>,
    /// The parser recovered from syntax errors somewhere in the file.
    pub had_errors: bool,
}

/// Read `source` as `lang`, with the default time budget and no way to stop.
pub fn extract(lang: Lang, source: &[u8]) -> Result<FileFacts, String> {
    extract_within(lang, source, super::PARSE_BUDGET, &|| false)
}

/// Read `source` as `lang`. The parse ends early (an error) when it takes
/// longer than `budget` or when `should_stop` says so; tree-sitter asks
/// between its steps, so a slow file never holds a refresh for long.
pub fn extract_within(
    lang: Lang,
    source: &[u8],
    budget: Duration,
    should_stop: &dyn Fn() -> bool,
) -> Result<FileFacts, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&lang.grammar())
        .map_err(|e| format!("the {} grammar did not load: {e}", lang.name()))?;
    let started = Instant::now();
    let mut timed_out = false;
    // Returning true halts the parse (tree-sitter's `progress_callback`).
    let mut halt = |_: &ParseState| -> bool {
        if started.elapsed() > budget {
            timed_out = true;
            return true;
        }
        should_stop()
    };
    let len = source.len();
    let mut input = |at: usize, _: Point| {
        if at < len {
            &source[at..]
        } else {
            &source[len..]
        }
    };
    let tree = parser.parse_with_options(
        &mut input,
        None,
        Some(ParseOptions::new().progress_callback(&mut halt)),
    );
    let Some(tree) = tree else {
        return Err(if timed_out {
            format!(
                "it took longer than {} s to read, so it is not read",
                budget.as_secs()
            )
        } else {
            "the refresh stopped while reading it".to_string()
        });
    };
    let root = tree.root_node();
    let mut walker = Walker::new(lang, source);
    walker.facts.had_errors = root.has_error();
    walker.walk(root);
    Ok(walker.finish())
}

/// What a node's children inherit.
#[derive(Debug, Clone, Default)]
struct Ctx {
    /// The type, trait, class or interface whose body this is.
    container: Option<Rc<str>>,
    /// Functions here are methods.
    in_type_body: bool,
    /// A Rust trait's body: its methods are as public as the trait.
    trait_body: bool,
    /// The trait or interface is exported.
    container_exported: bool,
    /// The definition uses here are inside.
    caller: Option<Rc<str>>,
    /// Rust inline modules.
    scope: Rc<str>,
}

type RefKey = (String, Option<String>, u32, RefKind);
type ImportKey = (String, Option<String>, Option<String>, ImportKind, String);

struct Walker<'s> {
    lang: Lang,
    src: &'s [u8],
    facts: FileFacts,
    /// Nodes already read by their parent (a declaration's name, a call's
    /// callee): not read again, nor their children.
    handled: HashSet<usize>,
    seen_refs: HashSet<RefKey>,
    seen_imports: HashSet<ImportKey>,
}

fn line_of(node: Node<'_>) -> u32 {
    u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX)
}

fn end_line_of(node: Node<'_>) -> u32 {
    u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX)
}

/// `text` on one line, whitespace collapsed, at most `max` characters.
fn compact(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max + 4));
    let mut space = false;
    let mut count = 0usize;
    for c in text.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            count += 1;
            space = false;
        }
        if count >= max {
            out.push('…');
            break;
        }
        out.push(c);
        count += 1;
    }
    out
}

/// `text` without any whitespace (a path written over lines).
fn no_space(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn join_path(prefix: &str, part: &str) -> String {
    let part = part.trim_start_matches("::");
    match (prefix.is_empty(), part.is_empty()) {
        (true, _) => part.to_string(),
        (false, true) => prefix.to_string(),
        (false, false) => format!("{prefix}::{part}"),
    }
}

fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// A string literal's contents (`"x.rs"`, `'./b'`, `` `fmt` ``, `r#"x"#`).
fn unquote(text: &str) -> String {
    let t = text.trim();
    let t = match t.strip_prefix('r') {
        Some(raw) if raw.starts_with('"') || raw.starts_with('#') => raw.trim_matches('#'),
        _ => t,
    };
    t.trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string()
}

/// A Rust name written as a constant (`MAX_FILES`) or a Python one.
fn is_constant_name(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

impl<'s> Walker<'s> {
    fn new(lang: Lang, src: &'s [u8]) -> Self {
        Walker {
            lang,
            src,
            facts: FileFacts::default(),
            handled: HashSet::new(),
            seen_refs: HashSet::new(),
            seen_imports: HashSet::new(),
        }
    }

    fn text(&self, node: Node<'_>) -> &'s str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn field_text(&self, node: Node<'_>, field: &str) -> Option<&'s str> {
        node.child_by_field_name(field).map(|n| self.text(n))
    }

    fn walk(&mut self, root: Node<'_>) {
        let mut stack: Vec<(Node<'_>, Ctx)> = vec![(root, Ctx::default())];
        let mut visited = 0usize;
        while let Some((node, ctx)) = stack.pop() {
            visited += 1;
            if visited > MAX_NODES {
                break;
            }
            if self.handled.contains(&node.id()) {
                continue;
            }
            let next = match self.lang {
                Lang::Rust => self.rust(node, &ctx),
                Lang::TypeScript | Lang::Tsx | Lang::JavaScript | Lang::Jsx => {
                    self.script(node, &ctx)
                }
                Lang::Python => self.python(node, &ctx),
                Lang::Go => self.go(node, &ctx),
            };
            let Some(next) = next else {
                continue;
            };
            for i in (0..node.named_child_count()).rev() {
                if let Some(child) = node.named_child(i) {
                    stack.push((child, next.clone()));
                }
            }
        }
    }

    fn finish(mut self) -> FileFacts {
        self.facts
            .symbols
            .sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
        self.facts
            .refs
            .sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
        self.facts.imports.sort_by_key(|i| i.line);
        self.facts
    }

    /// The text of `node` up to its `body` field (or all of it), as one line.
    fn signature(&self, node: Node<'_>, body: Option<&str>) -> String {
        let end = body
            .and_then(|field| node.child_by_field_name(field))
            .map(|b| b.start_byte())
            .unwrap_or_else(|| node.end_byte())
            .min(self.src.len());
        let start = node.start_byte().min(end);
        let raw = String::from_utf8_lossy(&self.src[start..end]);
        let trimmed =
            raw.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '{' | ':' | '='));
        compact(trimmed, MAX_SIGNATURE_CHARS)
    }

    /// Record a definition named by `name_node`; returns its qualified name
    /// (`Store::open`) for the uses inside it.
    #[allow(clippy::too_many_arguments)]
    fn symbol(
        &mut self,
        name_node: Node<'_>,
        def: Node<'_>,
        kind: SymbolKind,
        keyword: &'static str,
        ctx: &Ctx,
        exported: bool,
        body: Option<&str>,
        container: Option<&str>,
    ) -> Option<Rc<str>> {
        self.handled.insert(name_node.id());
        let name = self.text(name_node).trim();
        if name.is_empty() || name.len() > 200 || self.facts.symbols.len() >= MAX_SYMBOLS_PER_FILE {
            return None;
        }
        let container = container.map(str::to_string).or_else(|| {
            kind.is_member()
                .then(|| ctx.container.as_deref().map(str::to_string))
                .flatten()
        });
        let qualified: Rc<str> = match &container {
            Some(c) => format!("{c}::{name}").into(),
            None => name.into(),
        };
        self.facts.symbols.push(Symbol {
            name: name.to_string(),
            kind,
            keyword,
            line: line_of(def),
            end_line: end_line_of(def),
            container,
            scope: ctx.scope.to_string(),
            exported,
            signature: self.signature(def, body),
        });
        Some(qualified)
    }

    fn reference(
        &mut self,
        name: &str,
        qualifier: Option<String>,
        line: u32,
        kind: RefKind,
        ctx: &Ctx,
    ) {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 200
            || self.facts.refs.len() >= MAX_REFS_PER_FILE
            || !name
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        {
            return;
        }
        let qualifier = qualifier
            .map(|q| compact(&q, MAX_QUALIFIER_CHARS))
            .filter(|q| !q.is_empty());
        if !self
            .seen_refs
            .insert((name.to_string(), qualifier.clone(), line, kind))
        {
            return;
        }
        self.facts.refs.push(Reference {
            name: name.to_string(),
            qualifier,
            line,
            kind,
            caller: ctx.caller.as_deref().map(str::to_string),
            scope: ctx.scope.to_string(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn import(
        &mut self,
        spec: String,
        name: Option<String>,
        alias: Option<String>,
        line: u32,
        kind: ImportKind,
        public: bool,
        scope: &str,
    ) {
        if spec.is_empty() || spec.len() > 1_000 || self.facts.imports.len() >= MAX_IMPORTS_PER_FILE
        {
            return;
        }
        // A path written many times in code counts once, at its first line.
        let key = if kind == ImportKind::Path {
            (spec.clone(), None, None, kind, scope.to_string())
        } else {
            (
                spec.clone(),
                name.clone(),
                alias.clone(),
                kind,
                line.to_string(),
            )
        };
        if !self.seen_imports.insert(key) {
            return;
        }
        self.facts.imports.push(Import {
            spec,
            name,
            alias,
            line,
            kind,
            public,
            scope: scope.to_string(),
        });
    }

    // ---- Rust -----------------------------------------------------------

    fn rust_visibility(&self, node: Node<'_>) -> Option<&'s str> {
        (0..node.named_child_count())
            .filter_map(|i| node.named_child(i))
            .find(|c| c.kind() == "visibility_modifier")
            .map(|c| self.text(c))
    }

    /// `pub` exactly: `pub(crate)` and `pub(super)` are not public.
    fn rust_pub(&self, node: Node<'_>) -> bool {
        self.rust_visibility(node)
            .is_some_and(|v| v.trim() == "pub")
    }

    /// The attributes written right above `node`, as (name, value).
    fn rust_attributes(&self, node: Node<'_>) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        let mut at = node.prev_named_sibling();
        while let Some(sibling) = at {
            match sibling.kind() {
                "attribute_item" => {
                    if let Some(attribute) = sibling.named_child(0) {
                        let name = attribute
                            .named_child(0)
                            .map(|n| no_space(self.text(n)))
                            .unwrap_or_default();
                        let value = attribute
                            .child_by_field_name("value")
                            .map(|v| unquote(self.text(v)));
                        out.push((name, value));
                    }
                }
                "line_comment" | "block_comment" => {}
                _ => break,
            }
            at = sibling.prev_named_sibling();
        }
        out
    }

    /// The name of the type an `impl` is for: `Store` in `impl<T> Trait for Store<T>`.
    fn rust_type_name(&self, node: Node<'_>) -> String {
        match node.kind() {
            "type_identifier" | "primitive_type" => self.text(node).to_string(),
            "generic_type" | "reference_type" | "pointer_type" => node
                .child_by_field_name("type")
                .map(|t| self.rust_type_name(t))
                .unwrap_or_else(|| compact(self.text(node), 60)),
            "scoped_type_identifier" => self
                .field_text(node, "name")
                .map(str::to_string)
                .unwrap_or_else(|| compact(self.text(node), 60)),
            _ => compact(self.text(node), 60),
        }
    }

    /// A Rust path written in code: a dependency on its module when it starts
    /// at the crate, the current module or a module name.
    fn rust_path(&mut self, path: &str, line: u32, scope: &str) {
        let path = no_space(path);
        let path = path.trim_start_matches("::");
        let mut segments = path.split("::");
        let Some(first) = segments.next() else {
            return;
        };
        let more = segments.next().is_some();
        let local = matches!(first, "crate" | "self" | "super" | "$crate");
        let module_like = more
            && first
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && !matches!(first, "std" | "core" | "alloc");
        if (local && more) || module_like {
            let name = last_segment(path).to_string();
            self.import(
                path.to_string(),
                Some(name),
                None,
                line,
                ImportKind::Path,
                false,
                scope,
            );
        }
    }

    fn rust_use(&mut self, node: Node<'_>, prefix: &str, public: bool, scope: &str) {
        let line = line_of(node);
        match node.kind() {
            "use_as_clause" => {
                let path = self
                    .field_text(node, "path")
                    .map(no_space)
                    .unwrap_or_default();
                let alias = self.field_text(node, "alias").map(str::to_string);
                let full = join_path(prefix, &path);
                let name = last_segment(&full).to_string();
                self.import(
                    full,
                    Some(name),
                    alias,
                    line,
                    ImportKind::Use,
                    public,
                    scope,
                );
            }
            "use_wildcard" => {
                let path = node
                    .named_child(0)
                    .map(|n| no_space(self.text(n)))
                    .unwrap_or_default();
                let full = join_path(prefix, &path);
                if !full.is_empty() {
                    self.import(
                        format!("{full}::*"),
                        None,
                        None,
                        line,
                        ImportKind::Wildcard,
                        public,
                        scope,
                    );
                }
            }
            "use_list" => {
                for i in 0..node.named_child_count() {
                    if let Some(child) = node.named_child(i) {
                        self.rust_use(child, prefix, public, scope);
                    }
                }
            }
            "scoped_use_list" => {
                let path = self
                    .field_text(node, "path")
                    .map(no_space)
                    .unwrap_or_default();
                let inner = join_path(prefix, &path);
                if let Some(list) = node.child_by_field_name("list") {
                    self.rust_use(list, &inner, public, scope);
                }
            }
            "self" if !prefix.is_empty() => {
                let name = last_segment(prefix).to_string();
                self.import(
                    prefix.to_string(),
                    Some(name),
                    None,
                    line,
                    ImportKind::Use,
                    public,
                    scope,
                );
            }
            "line_comment" | "block_comment" => {}
            _ => {
                let full = join_path(prefix, &no_space(self.text(node)));
                if !full.is_empty() {
                    let name = last_segment(&full).to_string();
                    self.import(full, Some(name), None, line, ImportKind::Use, public, scope);
                }
            }
        }
    }

    fn rust_call(&mut self, func: Node<'_>, ctx: &Ctx) {
        let line = line_of(func);
        match func.kind() {
            "identifier" => {
                self.handled.insert(func.id());
                let name = self.text(func);
                self.reference(name, None, line, RefKind::Call, ctx);
            }
            "scoped_identifier" => {
                self.handled.insert(func.id());
                let name = self.field_text(func, "name").unwrap_or("");
                let path = self.field_text(func, "path").map(no_space);
                self.reference(name, path, line, RefKind::Call, ctx);
                let full = self.text(func);
                self.rust_path(full, line, &ctx.scope);
            }
            "field_expression" => {
                if let Some(field) = func.child_by_field_name("field") {
                    self.handled.insert(field.id());
                    let value = self.field_text(func, "value").map(str::to_string);
                    let name = self.text(field);
                    self.reference(name, value, line, RefKind::Method, ctx);
                }
            }
            "generic_function" => {
                if let Some(inner) = func.child_by_field_name("function") {
                    self.handled.insert(func.id());
                    self.rust_call(inner, ctx);
                    // The type arguments are still read.
                    if let Some(args) = func.child_by_field_name("type_arguments") {
                        self.rust_types_in(args, ctx);
                    }
                }
            }
            _ => {}
        }
    }

    /// Type names inside `node` (generic arguments of a handled callee).
    fn rust_types_in(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if n.kind() == "type_identifier" {
                let name = self.text(n);
                self.reference(name, None, line_of(n), RefKind::Type, ctx);
                continue;
            }
            for i in 0..n.named_child_count() {
                if let Some(c) = n.named_child(i) {
                    stack.push(c);
                }
            }
        }
    }

    /// Calls, macro calls and module paths in the token trees under `node`
    /// (a macro call or a `macro_rules!` definition), which tree-sitter keeps
    /// as tokens.
    fn rust_tokens(&mut self, node: Node<'_>, ctx: &Ctx) {
        let mut leaves: Vec<Node<'_>> = Vec::new();
        let mut stack: Vec<Node<'_>> = Vec::new();
        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                if child.kind() == "token_tree" || child.kind() == "macro_rule" {
                    stack.push(child);
                }
            }
        }
        while let Some(n) = stack.pop() {
            if leaves.len() >= MAX_TOKENS_SCANNED {
                break;
            }
            if n.child_count() == 0 {
                leaves.push(n);
                continue;
            }
            for i in (0..n.child_count()).rev() {
                if let Some(child) = n.child(i) {
                    stack.push(child);
                }
            }
        }
        let path_start = |kind: &str| {
            matches!(
                kind,
                "identifier" | "crate" | "self" | "super" | "metavariable"
            )
        };
        let mut i = 0;
        while i < leaves.len() {
            let kind = leaves[i].kind();
            if !path_start(kind) {
                i += 1;
                continue;
            }
            let start = i;
            let mut segments = vec![self.text(leaves[i]).to_string()];
            let mut j = i;
            while j + 2 < leaves.len()
                && leaves[j + 1].kind() == "::"
                && path_start(leaves[j + 2].kind())
            {
                segments.push(self.text(leaves[j + 2]).to_string());
                j += 2;
            }
            let line = line_of(leaves[start]);
            let after_dot = start > 0 && leaves[start - 1].kind() == ".";
            let next = leaves.get(j + 1).map(|n| n.kind());
            let name = segments.last().cloned().unwrap_or_default();
            let qualifier = (segments.len() > 1).then(|| segments[..segments.len() - 1].join("::"));
            match next {
                Some("(") if !name.starts_with('$') => {
                    let kind = if after_dot && qualifier.is_none() {
                        RefKind::Method
                    } else {
                        RefKind::Call
                    };
                    self.reference(&name, qualifier, line, kind, ctx);
                }
                Some("!") if !after_dot => {
                    self.reference(&name, qualifier, line, RefKind::Macro, ctx);
                }
                _ => {}
            }
            if segments.len() > 1 {
                self.rust_path(&segments.join("::"), line, &ctx.scope);
            }
            i = j + 1;
        }
    }

    fn rust(&mut self, node: Node<'_>, ctx: &Ctx) -> Option<Ctx> {
        match node.kind() {
            "function_item" | "function_signature_item" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let kind = if ctx.in_type_body {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                let exported = self.rust_pub(node) || (ctx.trait_body && ctx.container_exported);
                let qualified =
                    self.symbol(name, node, kind, "fn", ctx, exported, Some("body"), None);
                Some(Ctx {
                    container: None,
                    in_type_body: false,
                    trait_body: false,
                    container_exported: false,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            "struct_item" | "union_item" | "enum_item" | "trait_item" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let (kind, keyword) = match node.kind() {
                    "struct_item" => (SymbolKind::Struct, "struct"),
                    "union_item" => (SymbolKind::Union, "union"),
                    "enum_item" => (SymbolKind::Enum, "enum"),
                    _ => (SymbolKind::Trait, "trait"),
                };
                let exported = self.rust_pub(node);
                let name = self.text(name_node).trim().to_string();
                let qualified = self.symbol(
                    name_node,
                    node,
                    kind,
                    keyword,
                    ctx,
                    exported,
                    Some("body"),
                    None,
                );
                if kind == SymbolKind::Enum {
                    if let Some(body) = node.child_by_field_name("body") {
                        for i in 0..body.named_child_count() {
                            let Some(variant) = body.named_child(i) else {
                                continue;
                            };
                            if variant.kind() != "enum_variant" {
                                continue;
                            }
                            if let Some(variant_name) = variant.child_by_field_name("name") {
                                self.symbol(
                                    variant_name,
                                    variant,
                                    SymbolKind::Variant,
                                    "",
                                    ctx,
                                    exported,
                                    Some("body"),
                                    Some(&name),
                                );
                            }
                        }
                    }
                }
                Some(Ctx {
                    container: Some(name.as_str().into()),
                    in_type_body: kind == SymbolKind::Trait,
                    trait_body: kind == SymbolKind::Trait,
                    container_exported: exported,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            "impl_item" => {
                let ty = node
                    .child_by_field_name("type")
                    .map(|t| self.rust_type_name(t))
                    .unwrap_or_default();
                Some(Ctx {
                    container: (!ty.is_empty()).then(|| ty.as_str().into()),
                    in_type_body: true,
                    trait_body: false,
                    container_exported: false,
                    caller: ctx.caller.clone(),
                    scope: ctx.scope.clone(),
                })
            }
            "mod_item" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return None;
                };
                let name = self.text(name_node).trim().to_string();
                let inline = node.child_by_field_name("body").is_some();
                let path_attr = self
                    .rust_attributes(node)
                    .into_iter()
                    .find(|(attr, value)| attr == "path" && value.is_some())
                    .and_then(|(_, value)| value);
                let exported = self.rust_pub(node);
                self.symbol(
                    name_node,
                    node,
                    SymbolKind::Module,
                    "mod",
                    ctx,
                    exported,
                    Some("body"),
                    None,
                );
                self.facts.mods.push(ModDecl {
                    name: name.clone(),
                    line: line_of(node),
                    path_attr,
                    scope: ctx.scope.to_string(),
                    inline,
                });
                inline.then(|| Ctx {
                    container: None,
                    in_type_body: false,
                    trait_body: false,
                    container_exported: false,
                    caller: None,
                    scope: join_path(&ctx.scope, &name).into(),
                })
            }
            "const_item" | "static_item" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let (kind, keyword) = if node.kind() == "const_item" {
                        (SymbolKind::Constant, "const")
                    } else {
                        (SymbolKind::Static, "static")
                    };
                    // An associated constant belongs to its type.
                    let container = ctx.in_type_body.then(|| ctx.container.clone()).flatten();
                    let exported =
                        self.rust_pub(node) || (ctx.trait_body && ctx.container_exported);
                    self.symbol(
                        name,
                        node,
                        kind,
                        keyword,
                        ctx,
                        exported,
                        Some("value"),
                        container.as_deref(),
                    );
                }
                Some(ctx.clone())
            }
            "type_item" | "associated_type" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let container = ctx.in_type_body.then(|| ctx.container.clone()).flatten();
                    let exported =
                        self.rust_pub(node) || (ctx.trait_body && ctx.container_exported);
                    self.symbol(
                        name,
                        node,
                        SymbolKind::Type,
                        "type",
                        ctx,
                        exported,
                        Some("type"),
                        container.as_deref(),
                    );
                }
                Some(ctx.clone())
            }
            "macro_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let exported = self
                        .rust_attributes(node)
                        .iter()
                        .any(|(attr, _)| attr == "macro_export");
                    let qualified = self.symbol(
                        name,
                        node,
                        SymbolKind::Macro,
                        "macro_rules!",
                        ctx,
                        exported,
                        None,
                        None,
                    );
                    let inner = Ctx {
                        caller: qualified.or_else(|| ctx.caller.clone()),
                        ..ctx.clone()
                    };
                    self.rust_tokens(node, &inner);
                }
                None
            }
            "use_declaration" => {
                let public = self.rust_visibility(node).is_some();
                if let Some(argument) = node.child_by_field_name("argument") {
                    self.rust_use(argument, "", public, &ctx.scope);
                }
                None
            }
            "extern_crate_declaration" => {
                if let Some(name) = self.field_text(node, "name") {
                    let alias = self.field_text(node, "alias").map(str::to_string);
                    self.import(
                        name.to_string(),
                        Some(name.to_string()),
                        alias,
                        line_of(node),
                        ImportKind::ExternCrate,
                        self.rust_visibility(node).is_some(),
                        &ctx.scope,
                    );
                }
                None
            }
            "attribute_item"
            | "inner_attribute_item"
            | "line_comment"
            | "block_comment"
            | "visibility_modifier" => None,
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    self.rust_call(func, ctx);
                }
                Some(ctx.clone())
            }
            "macro_invocation" => {
                let line = line_of(node);
                if let Some(mac) = node.child_by_field_name("macro") {
                    match mac.kind() {
                        "scoped_identifier" => {
                            let name = self.field_text(mac, "name").unwrap_or("");
                            let path = self.field_text(mac, "path").map(no_space);
                            self.reference(name, path, line, RefKind::Macro, ctx);
                            let full = self.text(mac);
                            self.rust_path(full, line, &ctx.scope);
                        }
                        _ => {
                            let name = self.text(mac);
                            self.reference(name, None, line, RefKind::Macro, ctx);
                        }
                    }
                }
                self.rust_tokens(node, ctx);
                None
            }
            "scoped_identifier" => {
                let line = line_of(node);
                let name = self.field_text(node, "name").unwrap_or("");
                let path = self.field_text(node, "path").map(no_space);
                self.reference(name, path, line, RefKind::Use, ctx);
                let full = self.text(node);
                self.rust_path(full, line, &ctx.scope);
                None
            }
            "scoped_type_identifier" => {
                let line = line_of(node);
                let name = self.field_text(node, "name").unwrap_or("");
                let path = self.field_text(node, "path").map(no_space);
                self.reference(name, path, line, RefKind::Type, ctx);
                let full = self.text(node);
                self.rust_path(full, line, &ctx.scope);
                None
            }
            "type_identifier" => {
                let name = self.text(node);
                self.reference(name, None, line_of(node), RefKind::Type, ctx);
                None
            }
            "struct_expression" => {
                if let Some(name) = node.child_by_field_name("name") {
                    if name.kind() == "type_identifier" {
                        self.handled.insert(name.id());
                        let text = self.text(name);
                        self.reference(text, None, line_of(name), RefKind::Use, ctx);
                    }
                }
                Some(ctx.clone())
            }
            _ => Some(ctx.clone()),
        }
    }

    // ---- TypeScript / JavaScript ----------------------------------------

    /// Directly inside an `export` statement.
    fn script_exported(node: Node<'_>) -> bool {
        node.parent()
            .is_some_and(|p| p.kind() == "export_statement")
    }

    /// At the file's (or a namespace's) top level, directly or through `export`.
    fn script_top_level(node: Node<'_>) -> bool {
        let mut parent = node.parent();
        if parent.is_some_and(|p| p.kind() == "export_statement") {
            parent = parent.and_then(|p| p.parent());
        }
        match parent {
            None => true,
            Some(p) => match p.kind() {
                "program" => true,
                "statement_block" => p
                    .parent()
                    .is_some_and(|g| matches!(g.kind(), "internal_module" | "module")),
                _ => false,
            },
        }
    }

    fn is_function_value(node: Option<Node<'_>>) -> bool {
        node.is_some_and(|v| {
            matches!(
                v.kind(),
                "arrow_function" | "function_expression" | "function" | "generator_function"
            )
        })
    }

    /// The module a string node names (`'./b'` → `./b`).
    fn string_value(&self, node: Node<'_>) -> String {
        unquote(self.text(node))
    }

    fn script_import(&mut self, node: Node<'_>, ctx: &Ctx) {
        let Some(source) = node.child_by_field_name("source") else {
            return;
        };
        let spec = self.string_value(source);
        let line = line_of(node);
        let mut any = false;
        for i in 0..node.named_child_count() {
            let Some(child) = node.named_child(i) else {
                continue;
            };
            match child.kind() {
                "import_clause" => {
                    for j in 0..child.named_child_count() {
                        let Some(part) = child.named_child(j) else {
                            continue;
                        };
                        match part.kind() {
                            "identifier" => {
                                any = true;
                                let alias = self.text(part).to_string();
                                self.import(
                                    spec.clone(),
                                    Some("default".into()),
                                    Some(alias),
                                    line_of(part),
                                    ImportKind::Use,
                                    false,
                                    &ctx.scope,
                                );
                            }
                            "namespace_import" => {
                                any = true;
                                let alias = part.named_child(0).map(|n| self.text(n).to_string());
                                self.import(
                                    spec.clone(),
                                    None,
                                    alias,
                                    line_of(part),
                                    ImportKind::Wildcard,
                                    false,
                                    &ctx.scope,
                                );
                            }
                            "named_imports" => {
                                for k in 0..part.named_child_count() {
                                    let Some(specifier) = part.named_child(k) else {
                                        continue;
                                    };
                                    if specifier.kind() != "import_specifier" {
                                        continue;
                                    }
                                    any = true;
                                    let name = self.field_text(specifier, "name").map(unquote);
                                    let alias =
                                        self.field_text(specifier, "alias").map(str::to_string);
                                    self.import(
                                        spec.clone(),
                                        name,
                                        alias,
                                        line_of(specifier),
                                        ImportKind::Use,
                                        false,
                                        &ctx.scope,
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "import_require_clause" => {
                    any = true;
                    let alias = child.named_child(0).map(|n| self.text(n).to_string());
                    self.import(
                        spec.clone(),
                        None,
                        alias,
                        line,
                        ImportKind::Require,
                        false,
                        &ctx.scope,
                    );
                }
                _ => {}
            }
        }
        if !any {
            self.import(spec, None, None, line, ImportKind::Use, false, &ctx.scope);
        }
    }

    fn script_export_from(&mut self, node: Node<'_>, source: Node<'_>, ctx: &Ctx) {
        let spec = self.string_value(source);
        let line = line_of(node);
        let mut any = false;
        for i in 0..node.named_child_count() {
            let Some(child) = node.named_child(i) else {
                continue;
            };
            match child.kind() {
                "export_clause" => {
                    for j in 0..child.named_child_count() {
                        let Some(specifier) = child.named_child(j) else {
                            continue;
                        };
                        if specifier.kind() != "export_specifier" {
                            continue;
                        }
                        any = true;
                        let name = self.field_text(specifier, "name").map(unquote);
                        let alias = self.field_text(specifier, "alias").map(unquote);
                        self.import(
                            spec.clone(),
                            name,
                            alias,
                            line_of(specifier),
                            ImportKind::ReExport,
                            true,
                            &ctx.scope,
                        );
                    }
                }
                "namespace_export" => {
                    any = true;
                    let alias = child.named_child(0).map(|n| unquote(self.text(n)));
                    self.import(
                        spec.clone(),
                        None,
                        alias,
                        line,
                        ImportKind::ReExport,
                        true,
                        &ctx.scope,
                    );
                }
                _ => {}
            }
        }
        if !any {
            // `export * from './b'`
            self.import(
                spec,
                None,
                None,
                line,
                ImportKind::ReExport,
                true,
                &ctx.scope,
            );
        }
    }

    fn script_class_like(
        &mut self,
        node: Node<'_>,
        ctx: &Ctx,
        kind: SymbolKind,
        keyword: &'static str,
    ) -> Option<Ctx> {
        let Some(name_node) = node.child_by_field_name("name") else {
            return Some(ctx.clone());
        };
        let name = self.text(name_node).trim().to_string();
        let exported = Self::script_exported(node);
        let qualified = self.symbol(
            name_node,
            node,
            kind,
            keyword,
            ctx,
            exported,
            Some("body"),
            None,
        );
        Some(Ctx {
            container: Some(name.as_str().into()),
            in_type_body: true,
            trait_body: false,
            container_exported: exported,
            caller: qualified.or_else(|| ctx.caller.clone()),
            scope: ctx.scope.clone(),
        })
    }

    fn script(&mut self, node: Node<'_>, ctx: &Ctx) -> Option<Ctx> {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" | "function_signature" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let exported = Self::script_exported(node);
                let qualified = self.symbol(
                    name,
                    node,
                    SymbolKind::Function,
                    "function",
                    ctx,
                    exported,
                    Some("body"),
                    None,
                );
                Some(Ctx {
                    container: None,
                    in_type_body: false,
                    trait_body: false,
                    container_exported: false,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            // `export default function App() {}` may be read as an expression.
            "function_expression" | "function" if Self::script_exported(node) => {
                let Some(name) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let qualified = self.symbol(
                    name,
                    node,
                    SymbolKind::Function,
                    "function",
                    ctx,
                    true,
                    Some("body"),
                    None,
                );
                Some(Ctx {
                    container: None,
                    in_type_body: false,
                    trait_body: false,
                    container_exported: false,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            "class_declaration" | "abstract_class_declaration" => {
                self.script_class_like(node, ctx, SymbolKind::Class, "class")
            }
            "interface_declaration" => {
                self.script_class_like(node, ctx, SymbolKind::Interface, "interface")
            }
            "type_alias_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let exported = Self::script_exported(node);
                    self.symbol(
                        name,
                        node,
                        SymbolKind::Type,
                        "type",
                        ctx,
                        exported,
                        Some("value"),
                        None,
                    );
                }
                Some(ctx.clone())
            }
            "enum_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let exported = Self::script_exported(node);
                    self.symbol(
                        name,
                        node,
                        SymbolKind::Enum,
                        "enum",
                        ctx,
                        exported,
                        Some("body"),
                        None,
                    );
                }
                Some(ctx.clone())
            }
            "internal_module" | "module" => {
                if let Some(name) = node.child_by_field_name("name") {
                    if name.kind() != "string" {
                        let exported = Self::script_exported(node);
                        self.symbol(
                            name,
                            node,
                            SymbolKind::Module,
                            "namespace",
                            ctx,
                            exported,
                            Some("body"),
                            None,
                        );
                    }
                }
                Some(ctx.clone())
            }
            "method_definition" | "method_signature" | "abstract_method_signature" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let qualified = if ctx.in_type_body {
                    self.symbol(
                        name,
                        node,
                        SymbolKind::Method,
                        "",
                        ctx,
                        ctx.container_exported,
                        Some("body"),
                        None,
                    )
                } else {
                    None
                };
                Some(Ctx {
                    container: None,
                    in_type_body: false,
                    trait_body: false,
                    container_exported: false,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            "public_field_definition" | "field_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .or_else(|| node.child_by_field_name("property"));
                let value = node.child_by_field_name("value");
                if let (Some(name), true) =
                    (name, ctx.in_type_body && Self::is_function_value(value))
                {
                    let qualified = self.symbol(
                        name,
                        node,
                        SymbolKind::Method,
                        "",
                        ctx,
                        ctx.container_exported,
                        Some("value"),
                        None,
                    );
                    return Some(Ctx {
                        container: None,
                        in_type_body: false,
                        trait_body: false,
                        container_exported: false,
                        caller: qualified.or_else(|| ctx.caller.clone()),
                        scope: ctx.scope.clone(),
                    });
                }
                Some(ctx.clone())
            }
            "variable_declarator" => {
                let Some(declaration) = node.parent() else {
                    return Some(ctx.clone());
                };
                let keyword = match declaration.kind() {
                    "lexical_declaration" => match self.field_text(declaration, "kind") {
                        Some("let") => "let",
                        _ => "const",
                    },
                    "variable_declaration" => "var",
                    _ => return Some(ctx.clone()),
                };
                if !Self::script_top_level(declaration) {
                    return Some(ctx.clone());
                }
                let Some(name) = node
                    .child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                else {
                    return Some(ctx.clone());
                };
                let value = node.child_by_field_name("value");
                let kind = if Self::is_function_value(value) {
                    SymbolKind::Function
                } else if keyword == "const" {
                    SymbolKind::Constant
                } else {
                    SymbolKind::Variable
                };
                let exported = Self::script_exported(declaration);
                let qualified = self.symbol(
                    name,
                    node,
                    kind,
                    keyword,
                    ctx,
                    exported,
                    Some("value"),
                    None,
                );
                Some(Ctx {
                    caller: if kind == SymbolKind::Function {
                        qualified.or_else(|| ctx.caller.clone())
                    } else {
                        ctx.caller.clone()
                    },
                    ..ctx.clone()
                })
            }
            "export_statement" => {
                if let Some(source) = node.child_by_field_name("source") {
                    self.script_export_from(node, source, ctx);
                    return None;
                }
                Some(ctx.clone())
            }
            "import_statement" => {
                self.script_import(node, ctx);
                None
            }
            "call_expression" => {
                let line = line_of(node);
                if let Some(func) = node.child_by_field_name("function") {
                    match func.kind() {
                        "identifier" => {
                            self.handled.insert(func.id());
                            let name = self.text(func);
                            let first_string = node
                                .child_by_field_name("arguments")
                                .and_then(|a| a.named_child(0))
                                .filter(|a| a.kind() == "string");
                            if name == "require" {
                                if let Some(arg) = first_string {
                                    let spec = self.string_value(arg);
                                    self.import(
                                        spec,
                                        None,
                                        None,
                                        line,
                                        ImportKind::Require,
                                        false,
                                        &ctx.scope,
                                    );
                                }
                            } else {
                                self.reference(name, None, line, RefKind::Call, ctx);
                            }
                        }
                        "import" => {
                            let first_string = node
                                .child_by_field_name("arguments")
                                .and_then(|a| a.named_child(0))
                                .filter(|a| a.kind() == "string");
                            if let Some(arg) = first_string {
                                let spec = self.string_value(arg);
                                self.import(
                                    spec,
                                    None,
                                    None,
                                    line,
                                    ImportKind::Dynamic,
                                    false,
                                    &ctx.scope,
                                );
                            }
                        }
                        "member_expression" => {
                            if let Some(property) = func.child_by_field_name("property") {
                                self.handled.insert(property.id());
                                let object = self.field_text(func, "object").map(str::to_string);
                                let name = self.text(property);
                                self.reference(name, object, line, RefKind::Method, ctx);
                            }
                        }
                        _ => {}
                    }
                }
                Some(ctx.clone())
            }
            "new_expression" => {
                if let Some(constructor) = node.child_by_field_name("constructor") {
                    let line = line_of(node);
                    match constructor.kind() {
                        "identifier" => {
                            self.handled.insert(constructor.id());
                            let name = self.text(constructor);
                            self.reference(name, None, line, RefKind::Call, ctx);
                        }
                        "member_expression" => {
                            if let Some(property) = constructor.child_by_field_name("property") {
                                self.handled.insert(property.id());
                                let object =
                                    self.field_text(constructor, "object").map(str::to_string);
                                let name = self.text(property);
                                self.reference(name, object, line, RefKind::Call, ctx);
                            }
                        }
                        _ => {}
                    }
                }
                Some(ctx.clone())
            }
            "jsx_opening_element" | "jsx_self_closing_element" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let line = line_of(node);
                    match name.kind() {
                        "identifier" => {
                            let text = self.text(name);
                            if text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                                self.reference(text, None, line, RefKind::Use, ctx);
                            }
                        }
                        "member_expression" => {
                            if let Some(property) = name.child_by_field_name("property") {
                                let object = self.field_text(name, "object").map(str::to_string);
                                let text = self.text(property);
                                self.reference(text, object, line, RefKind::Use, ctx);
                            }
                        }
                        _ => {}
                    }
                }
                Some(ctx.clone())
            }
            "type_identifier" => {
                let name = self.text(node);
                self.reference(name, None, line_of(node), RefKind::Type, ctx);
                None
            }
            "comment" | "string" | "regex" => None,
            _ => Some(ctx.clone()),
        }
    }

    // ---- Python -----------------------------------------------------------

    /// At the module's top level (directly or under a decorator).
    fn python_top_level(node: Node<'_>) -> bool {
        let mut parent = node.parent();
        if parent.is_some_and(|p| p.kind() == "decorated_definition") {
            parent = parent.and_then(|p| p.parent());
        }
        parent.is_none_or(|p| p.kind() == "module")
    }

    fn python(&mut self, node: Node<'_>, ctx: &Ctx) -> Option<Ctx> {
        match node.kind() {
            "function_definition" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let kind = if ctx.in_type_body {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                let text = self.text(name);
                let exported = !text.starts_with('_')
                    && (Self::python_top_level(node)
                        || (ctx.in_type_body && ctx.container_exported));
                let qualified =
                    self.symbol(name, node, kind, "def", ctx, exported, Some("body"), None);
                Some(Ctx {
                    container: None,
                    in_type_body: false,
                    trait_body: false,
                    container_exported: false,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            "class_definition" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let name = self.text(name_node).trim().to_string();
                let exported = !name.starts_with('_') && Self::python_top_level(node);
                let qualified = self.symbol(
                    name_node,
                    node,
                    SymbolKind::Class,
                    "class",
                    ctx,
                    exported,
                    Some("body"),
                    None,
                );
                Some(Ctx {
                    container: Some(name.as_str().into()),
                    in_type_body: true,
                    trait_body: false,
                    container_exported: exported,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            "expression_statement" => {
                if node.parent().is_some_and(|p| p.kind() == "module") {
                    if let Some(assignment) =
                        node.named_child(0).filter(|n| n.kind() == "assignment")
                    {
                        if let Some(left) = assignment
                            .child_by_field_name("left")
                            .filter(|l| l.kind() == "identifier")
                        {
                            let name = self.text(left);
                            if is_constant_name(name) {
                                self.symbol(
                                    left,
                                    assignment,
                                    SymbolKind::Constant,
                                    "",
                                    ctx,
                                    !name.starts_with('_'),
                                    Some("right"),
                                    None,
                                );
                            }
                        }
                    }
                }
                Some(ctx.clone())
            }
            "import_statement" => {
                for i in 0..node.named_child_count() {
                    let Some(child) = node.named_child(i) else {
                        continue;
                    };
                    let line = line_of(child);
                    match child.kind() {
                        "dotted_name" => {
                            let spec = no_space(self.text(child));
                            self.import(spec, None, None, line, ImportKind::Use, false, &ctx.scope);
                        }
                        "aliased_import" => {
                            let spec = self
                                .field_text(child, "name")
                                .map(no_space)
                                .unwrap_or_default();
                            let alias = self.field_text(child, "alias").map(str::to_string);
                            self.import(
                                spec,
                                None,
                                alias,
                                line,
                                ImportKind::Use,
                                false,
                                &ctx.scope,
                            );
                        }
                        _ => {}
                    }
                }
                None
            }
            "import_from_statement" => {
                let Some(module) = node.child_by_field_name("module_name") else {
                    return None;
                };
                let spec = no_space(self.text(module));
                let mut any = false;
                for i in 0..node.named_child_count() {
                    let Some(child) = node.named_child(i) else {
                        continue;
                    };
                    if child.id() == module.id() {
                        continue;
                    }
                    let line = line_of(child);
                    match child.kind() {
                        "dotted_name" => {
                            any = true;
                            let name = no_space(self.text(child));
                            self.import(
                                spec.clone(),
                                Some(name),
                                None,
                                line,
                                ImportKind::Use,
                                false,
                                &ctx.scope,
                            );
                        }
                        "aliased_import" => {
                            any = true;
                            let name = self.field_text(child, "name").map(no_space);
                            let alias = self.field_text(child, "alias").map(str::to_string);
                            self.import(
                                spec.clone(),
                                name,
                                alias,
                                line,
                                ImportKind::Use,
                                false,
                                &ctx.scope,
                            );
                        }
                        "wildcard_import" => {
                            any = true;
                            self.import(
                                spec.clone(),
                                None,
                                None,
                                line,
                                ImportKind::Wildcard,
                                false,
                                &ctx.scope,
                            );
                        }
                        _ => {}
                    }
                }
                if !any {
                    self.import(
                        spec,
                        None,
                        None,
                        line_of(node),
                        ImportKind::Use,
                        false,
                        &ctx.scope,
                    );
                }
                None
            }
            "call" => {
                let line = line_of(node);
                if let Some(func) = node.child_by_field_name("function") {
                    match func.kind() {
                        "identifier" => {
                            self.handled.insert(func.id());
                            let name = self.text(func);
                            self.reference(name, None, line, RefKind::Call, ctx);
                        }
                        "attribute" => {
                            if let Some(attribute) = func.child_by_field_name("attribute") {
                                self.handled.insert(attribute.id());
                                let object = self.field_text(func, "object").map(str::to_string);
                                let name = self.text(attribute);
                                self.reference(name, object, line, RefKind::Method, ctx);
                            }
                        }
                        _ => {}
                    }
                }
                Some(ctx.clone())
            }
            "comment" | "string" => None,
            _ => Some(ctx.clone()),
        }
    }

    // ---- Go ---------------------------------------------------------------

    fn go_exported(name: &str) -> bool {
        name.chars().next().is_some_and(|c| c.is_uppercase())
    }

    /// The receiver's type of a method: `Store` in `func (s *Store[T]) Open()`.
    fn go_receiver(&self, node: Node<'_>) -> Option<String> {
        let receiver = node.child_by_field_name("receiver")?;
        let parameter = (0..receiver.named_child_count())
            .filter_map(|i| receiver.named_child(i))
            .find(|c| c.kind() == "parameter_declaration")?;
        let ty = parameter.child_by_field_name("type")?;
        let text = no_space(self.text(ty));
        let text = text.trim_start_matches('*');
        let text = text.split('[').next().unwrap_or(text);
        (!text.is_empty()).then(|| text.to_string())
    }

    fn go_top_level(spec: Node<'_>) -> bool {
        let mut parent = spec.parent();
        while let Some(p) = parent {
            match p.kind() {
                "const_declaration" | "var_declaration" | "var_spec_list" => parent = p.parent(),
                "source_file" => return true,
                _ => return false,
            }
        }
        false
    }

    fn go(&mut self, node: Node<'_>, ctx: &Ctx) -> Option<Ctx> {
        match node.kind() {
            "package_clause" => {
                if let Some(name) = node.named_child(0) {
                    self.facts.package = Some(self.text(name).to_string());
                }
                None
            }
            "function_declaration" | "method_declaration" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let text = self.text(name).to_string();
                let exported = Self::go_exported(&text);
                let qualified = if node.kind() == "method_declaration" {
                    let receiver = self.go_receiver(node);
                    self.symbol(
                        name,
                        node,
                        SymbolKind::Method,
                        "func",
                        ctx,
                        exported,
                        Some("body"),
                        receiver.as_deref(),
                    )
                } else {
                    self.symbol(
                        name,
                        node,
                        SymbolKind::Function,
                        "func",
                        ctx,
                        exported,
                        Some("body"),
                        None,
                    )
                };
                Some(Ctx {
                    container: None,
                    in_type_body: false,
                    trait_body: false,
                    container_exported: false,
                    caller: qualified.or_else(|| ctx.caller.clone()),
                    scope: ctx.scope.clone(),
                })
            }
            "type_spec" | "type_alias" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return Some(ctx.clone());
                };
                let name = self.text(name_node).trim().to_string();
                let kind = match node.child_by_field_name("type").map(|t| t.kind()) {
                    Some("struct_type") => SymbolKind::Struct,
                    Some("interface_type") => SymbolKind::Interface,
                    _ => SymbolKind::Type,
                };
                let exported = Self::go_exported(&name);
                self.symbol(
                    name_node,
                    node,
                    kind,
                    "type",
                    ctx,
                    exported,
                    Some("type"),
                    None,
                );
                Some(Ctx {
                    container: Some(name.as_str().into()),
                    in_type_body: kind == SymbolKind::Interface,
                    trait_body: false,
                    container_exported: exported,
                    caller: ctx.caller.clone(),
                    scope: ctx.scope.clone(),
                })
            }
            "method_elem" => {
                if ctx.in_type_body {
                    if let Some(name) = node.child_by_field_name("name") {
                        let exported = Self::go_exported(self.text(name));
                        self.symbol(
                            name,
                            node,
                            SymbolKind::Method,
                            "func",
                            ctx,
                            exported,
                            None,
                            None,
                        );
                    }
                }
                Some(ctx.clone())
            }
            "const_spec" | "var_spec" => {
                if Self::go_top_level(node) {
                    let (kind, keyword) = if node.kind() == "const_spec" {
                        (SymbolKind::Constant, "const")
                    } else {
                        (SymbolKind::Variable, "var")
                    };
                    for i in 0..node.child_count() {
                        let Some(child) = node.child(i) else {
                            continue;
                        };
                        let is_name = u32::try_from(i)
                            .ok()
                            .and_then(|i| node.field_name_for_child(i))
                            == Some("name");
                        if is_name && child.kind() == "identifier" {
                            let exported = Self::go_exported(self.text(child));
                            self.symbol(
                                child,
                                node,
                                kind,
                                keyword,
                                ctx,
                                exported,
                                Some("value"),
                                None,
                            );
                        }
                    }
                }
                Some(ctx.clone())
            }
            "import_spec" => {
                if let Some(path) = node.child_by_field_name("path") {
                    let spec = unquote(self.text(path));
                    let alias = self.field_text(node, "name").map(str::to_string);
                    self.import(
                        spec,
                        None,
                        alias,
                        line_of(node),
                        ImportKind::Use,
                        false,
                        &ctx.scope,
                    );
                }
                None
            }
            "call_expression" => {
                let line = line_of(node);
                if let Some(func) = node.child_by_field_name("function") {
                    match func.kind() {
                        "identifier" => {
                            self.handled.insert(func.id());
                            let name = self.text(func);
                            self.reference(name, None, line, RefKind::Call, ctx);
                        }
                        "selector_expression" => {
                            if let Some(field) = func.child_by_field_name("field") {
                                self.handled.insert(field.id());
                                let operand = self.field_text(func, "operand").map(str::to_string);
                                let name = self.text(field);
                                self.reference(name, operand, line, RefKind::Method, ctx);
                            }
                        }
                        _ => {}
                    }
                }
                Some(ctx.clone())
            }
            "qualified_type" => {
                let name = self.field_text(node, "name").unwrap_or("");
                let package = self.field_text(node, "package").map(str::to_string);
                self.reference(name, package, line_of(node), RefKind::Type, ctx);
                None
            }
            "type_identifier" => {
                let name = self.text(node);
                self.reference(name, None, line_of(node), RefKind::Type, ctx);
                None
            }
            "comment" | "interpreted_string_literal" | "raw_string_literal" => None,
            _ => Some(ctx.clone()),
        }
    }
}
