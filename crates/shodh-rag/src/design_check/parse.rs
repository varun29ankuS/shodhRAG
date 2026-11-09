//! Exact reading of a UI file with tree-sitter, the parser the code map
//! uses (`code_map::lang`): which bytes are comments, where each markup tag
//! is, which runs of bytes are element text, and the CSS rules and their
//! selectors.
//!
//! A string, an attribute value, a template literal or a regular expression
//! is a node of its own, so a `/*`, `//` or `<!--` inside one is never a
//! comment. JSX text is the grammar's `jsx_text`, never the code around it.
//! HTML, Vue and Svelte files are read with the HTML grammar; their
//! `<script>` bodies with the JavaScript or TypeScript grammar and their
//! `<style>` bodies with the CSS grammar. SCSS and Less are read with the
//! CSS grammar, which knows their `//` comments.
//!
//! Each file has the code map's budget ([`crate::code_map::PARSE_BUDGET`]) for
//! all its parses together; a file that takes longer is not checked.

use std::time::Instant;

use tree_sitter::{Language, Node, ParseOptions, ParseState, Parser, Point, Tree};

use super::source::{Tag, UiKind};

/// The grammars the design check reads with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grammar {
    JavaScript,
    TypeScript,
    Tsx,
    Html,
    Css,
}

impl Grammar {
    fn language(self) -> Language {
        match self {
            // The JavaScript grammar reads JSX.
            Grammar::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Grammar::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Grammar::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Grammar::Html => tree_sitter_html::LANGUAGE.into(),
            Grammar::Css => tree_sitter_css::LANGUAGE.into(),
        }
    }

    /// The grammar of a file.
    pub fn of(path: &str, kind: UiKind) -> Grammar {
        match kind {
            UiKind::Css | UiKind::Scss => Grammar::Css,
            UiKind::Html | UiKind::Vue | UiKind::Svelte => Grammar::Html,
            UiKind::Jsx if path.to_ascii_lowercase().ends_with(".tsx") => Grammar::Tsx,
            UiKind::Jsx => Grammar::JavaScript,
        }
    }
}

/// A CSS rule: its selectors (each on its own, white space collapsed) and
/// its block (`{` to `}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssRule {
    pub selectors: Vec<String>,
    pub start: usize,
    pub end: usize,
}

/// What a file holds, by byte offsets.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// Comments, in order.
    pub comments: Vec<(usize, usize)>,
    /// Markup tags, in order.
    pub tags: Vec<Tag>,
    /// Runs of element text: each the spans of the file it is made of, in
    /// order (expressions and comments between them left out).
    pub runs: Vec<Vec<(usize, usize)>>,
    pub css_rules: Vec<CssRule>,
}

/// Parse `source` with `grammar`, halting at `deadline`.
fn tree(grammar: Grammar, source: &[u8], deadline: Instant) -> Result<Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&grammar.language())
        .map_err(|e| format!("a grammar did not load: {e}"))?;
    let mut timed_out = false;
    let mut halt = |_: &ParseState| -> bool {
        if Instant::now() > deadline {
            timed_out = true;
            return true;
        }
        false
    };
    let len = source.len();
    let mut input = |at: usize, _: Point| {
        if at < len {
            &source[at..]
        } else {
            &source[len..]
        }
    };
    parser
        .parse_with_options(
            &mut input,
            None,
            Some(ParseOptions::new().progress_callback(&mut halt)),
        )
        .ok_or_else(|| {
            if timed_out {
                format!(
                    "it took longer than {} s to read, so it is not checked",
                    crate::code_map::PARSE_BUDGET.as_secs()
                )
            } else {
                "it could not be read".to_string()
            }
        })
}

/// Every node of `root`, depth first, in source order.
fn nodes(root: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut cursor = root.walk();
    loop {
        out.push(cursor.node());
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return out;
            }
        }
    }
}

fn text_of<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.start_byte()..node.end_byte()]).unwrap_or("")
}

/// Read `source` (the whole file) as `path` of `kind`.
pub fn parse(path: &str, kind: UiKind, source: &str) -> Result<Parsed, String> {
    let deadline = Instant::now() + crate::code_map::PARSE_BUDGET;
    let mut parsed = Parsed::default();
    read(
        Grammar::of(path, kind),
        kind,
        source.as_bytes(),
        0,
        deadline,
        &mut parsed,
    )?;
    parsed.comments.sort_unstable();
    parsed.tags.sort_by_key(|t| t.start);
    parsed
        .runs
        .sort_by_key(|r| r.first().map(|s| s.0).unwrap_or(0));
    parsed.css_rules.sort_by_key(|r| r.start);
    Ok(parsed)
}

/// Read `bytes` (which start at `offset` in the file) with `grammar`.
fn read(
    grammar: Grammar,
    kind: UiKind,
    bytes: &[u8],
    offset: usize,
    deadline: Instant,
    parsed: &mut Parsed,
) -> Result<(), String> {
    let tree = tree(grammar, bytes, deadline)?;
    let root = tree.root_node();
    let at = |n: Node<'_>| (offset + n.start_byte(), offset + n.end_byte());
    // Template expressions in Vue and Svelte text (`{{ x }}`, `{x}`).
    let braces = matches!(kind, UiKind::Vue | UiKind::Svelte);
    for node in nodes(root) {
        match node.kind() {
            "comment" | "html_comment" | "js_comment" => parsed.comments.push(at(node)),
            "jsx_opening_element" | "jsx_self_closing_element" | "jsx_closing_element" => {
                let Some(name) = node.child_by_field_name("name") else {
                    continue;
                };
                let (start, end) = at(node);
                let written = text_of(name, bytes);
                parsed.tags.push(Tag {
                    start,
                    end,
                    name: tag_name(written),
                    closing: node.kind() == "jsx_closing_element",
                    self_closing: node.kind() == "jsx_self_closing_element",
                });
            }
            "start_tag" | "self_closing_tag" | "end_tag" => {
                let Some(name) = first_child_of(node, "tag_name") else {
                    continue;
                };
                let (start, end) = at(node);
                parsed.tags.push(Tag {
                    start,
                    end,
                    name: text_of(name, bytes).to_ascii_lowercase(),
                    closing: node.kind() == "end_tag",
                    self_closing: node.kind() == "self_closing_tag",
                });
            }
            "jsx_element" => collect_runs(
                node,
                bytes,
                offset,
                &["jsx_text", "html_character_reference"],
                false,
                parsed,
            ),
            "element" | "document" => {
                collect_runs(node, bytes, offset, &["text", "entity"], braces, parsed)
            }
            "script_element" | "style_element" => {
                let Some(body) = first_child_of(node, "raw_text") else {
                    continue;
                };
                let inner = if node.kind() == "style_element" {
                    Grammar::Css
                } else if script_is_typescript(node, bytes) {
                    Grammar::TypeScript
                } else {
                    Grammar::JavaScript
                };
                let (start, end) = (body.start_byte(), body.end_byte());
                read(
                    inner,
                    UiKind::Css,
                    &bytes[start..end],
                    offset + start,
                    deadline,
                    parsed,
                )?;
            }
            "rule_set" => {
                let (Some(selectors), Some(block)) = (
                    first_child_of(node, "selectors"),
                    first_child_of(node, "block"),
                ) else {
                    continue;
                };
                let (start, end) = at(block);
                parsed.css_rules.push(CssRule {
                    selectors: split_selectors(text_of(selectors, bytes)),
                    start,
                    end,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// A JSX tag's name as the rules match it: HTML elements lowercase,
/// components as written (`Image`, `Foo.Bar`).
fn tag_name(written: &str) -> String {
    if written
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase())
    {
        written.to_ascii_lowercase()
    } else {
        written.to_string()
    }
}

fn first_child_of<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|c| c.kind() == kind);
    found
}

/// Whether a `<script>` is TypeScript (`lang="ts"`, `lang="typescript"`).
fn script_is_typescript(script: Node<'_>, bytes: &[u8]) -> bool {
    let Some(start) = first_child_of(script, "start_tag") else {
        return false;
    };
    let text = text_of(start, bytes).to_ascii_lowercase();
    text.contains("lang=\"ts\"")
        || text.contains("lang='ts'")
        || text.contains("lang=\"typescript\"")
        || text.contains("lang=ts")
}

/// A selector list, each selector with its white space collapsed.
pub fn split_selectors(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect()
}

/// The text runs of an element: each maximal group of its text children
/// (`text_kinds`) with the expressions and comments between them, as the
/// spans from the first text to the last with those left out.
fn collect_runs(
    element: Node<'_>,
    bytes: &[u8],
    offset: usize,
    text_kinds: &[&str],
    braces: bool,
    parsed: &mut Parsed,
) {
    let mut cursor = element.walk();
    let children: Vec<Node<'_>> = element.children(&mut cursor).collect();
    let mut group: Vec<Node<'_>> = Vec::new();
    for child in children {
        let kind = child.kind();
        let inline = text_kinds.contains(&kind)
            || matches!(kind, "jsx_expression" | "comment" | "html_comment");
        if inline {
            group.push(child);
        } else {
            flush_run(&group, bytes, offset, text_kinds, braces, parsed);
            group.clear();
        }
    }
    flush_run(&group, bytes, offset, text_kinds, braces, parsed);
}

/// One group of an element's text children as a run (when it has text).
fn flush_run(
    group: &[Node<'_>],
    bytes: &[u8],
    offset: usize,
    text_kinds: &[&str],
    braces: bool,
    parsed: &mut Parsed,
) {
    {
        let texts: Vec<&Node<'_>> = group
            .iter()
            .filter(|n| text_kinds.contains(&n.kind()))
            .collect();
        if let (Some(first), Some(last)) = (texts.first(), texts.last()) {
            let (from, to) = (first.start_byte(), last.end_byte());
            let mut spans = Vec::new();
            let mut at = from;
            for gap in group
                .iter()
                .filter(|n| !text_kinds.contains(&n.kind()))
                .filter(|n| n.start_byte() >= from && n.end_byte() <= to)
            {
                if gap.start_byte() > at {
                    spans.push((at, gap.start_byte()));
                }
                at = gap.end_byte();
            }
            if to > at {
                spans.push((at, to));
            }
            if braces {
                spans = without_braces(&spans, bytes);
            }
            let spans: Vec<(usize, usize)> = spans
                .into_iter()
                .filter(|(a, b)| b > a)
                .map(|(a, b)| (offset + a, offset + b))
                .collect();
            if !spans.is_empty() {
                parsed.runs.push(spans);
            }
        }
    }
}

/// `spans` with every `{…}` (a Vue or Svelte expression) cut out.
fn without_braces(spans: &[(usize, usize)], bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    for &(from, to) in spans {
        let mut start = (depth == 0).then_some(from);
        for (i, b) in bytes.iter().enumerate().take(to).skip(from) {
            match b {
                b'{' => {
                    if depth == 0 {
                        if let Some(s) = start.take() {
                            out.push((s, i));
                        }
                    }
                    depth += 1;
                }
                b'}' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        start = Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push((s, to));
        }
    }
    out
}

/// Plain literal colours of a Tailwind config (`tailwind.config.js`/`.ts`):
/// every `colors: { … }` object, nested keys joined with `-` (`brand: {
/// 500: '#123' }` is `brand-500`; `DEFAULT` is the key itself). Values that
/// are not string literals (functions, `var()` in a template) are left out.
pub fn tailwind_config_colors(source: &str, typescript: bool) -> Vec<(String, String)> {
    let deadline = Instant::now() + crate::code_map::PARSE_BUDGET;
    let grammar = if typescript {
        Grammar::TypeScript
    } else {
        Grammar::JavaScript
    };
    let bytes = source.as_bytes();
    let Ok(tree) = tree(grammar, bytes, deadline) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in nodes(tree.root_node()) {
        if node.kind() != "pair" {
            continue;
        }
        let (Some(key), Some(value)) = (
            node.child_by_field_name("key"),
            node.child_by_field_name("value"),
        ) else {
            continue;
        };
        if key_text(key, bytes).as_deref() == Some("colors") && value.kind() == "object" {
            flatten_colors(value, bytes, "", &mut out);
        }
    }
    out
}

fn key_text(key: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = text_of(key, bytes);
    match key.kind() {
        "property_identifier" | "number" => Some(text.to_string()),
        "string" => Some(text.trim_matches(|c| c == '"' || c == '\'').to_string()),
        _ => None,
    }
}

fn flatten_colors(object: Node<'_>, bytes: &[u8], prefix: &str, out: &mut Vec<(String, String)>) {
    let mut cursor = object.walk();
    let pairs: Vec<Node<'_>> = object.named_children(&mut cursor).collect();
    for pair in pairs {
        if pair.kind() != "pair" {
            continue;
        }
        let (Some(key), Some(value)) = (
            pair.child_by_field_name("key"),
            pair.child_by_field_name("value"),
        ) else {
            continue;
        };
        let Some(key) = key_text(key, bytes) else {
            continue;
        };
        let name = match (prefix.is_empty(), key.as_str()) {
            (true, _) => key.clone(),
            (false, "DEFAULT") => prefix.to_string(),
            (false, _) => format!("{prefix}-{key}"),
        };
        match value.kind() {
            "string" => {
                let literal = text_of(value, bytes).trim_matches(|c| c == '"' || c == '\'');
                out.push((name, literal.to_string()));
            }
            "object" => flatten_colors(value, bytes, &name, out),
            _ => {}
        }
    }
}
