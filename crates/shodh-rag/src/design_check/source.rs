//! A UI file prepared for the rules: its comments blanked (offsets kept), its
//! markup tags, its CSS declarations, its class tokens and its element text.
//! Pure and deterministic: the same text always gives the same parts.

use std::sync::LazyLock;

use regex::Regex;

pub use super::parse::CssRule;

/// What kind of UI file a path is, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKind {
    Html,
    Css,
    /// SCSS and Less: CSS with `//` comments.
    Scss,
    /// JSX and TSX.
    Jsx,
    Vue,
    Svelte,
}

impl UiKind {
    /// The kind of `path`, or `None` when it is not a UI file the check reads.
    /// Minified files are generated, not written, so they are left out.
    pub fn of_path(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        if name.contains(".min.") {
            return None;
        }
        let ext = name.rsplit_once('.').map(|(_, e)| e)?;
        Some(match ext {
            "html" | "htm" => Self::Html,
            "css" => Self::Css,
            "scss" | "less" => Self::Scss,
            "jsx" | "tsx" => Self::Jsx,
            "vue" => Self::Vue,
            "svelte" => Self::Svelte,
            _ => return None,
        })
    }

    /// Whether the file holds markup (tags and element text).
    pub fn has_markup(self) -> bool {
        !matches!(self, Self::Css | Self::Scss)
    }
}

/// Test, story and fixture files: UI files that are not the product's
/// interface, left out of folder and automatic checks (a file asked for by
/// name is still checked).
pub fn is_test_or_fixture(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    [".test.", ".spec.", ".stories.", ".story."]
        .iter()
        .any(|part| name.contains(part))
        || lower.split('/').any(|part| {
            matches!(
                part,
                "fixtures" | "__fixtures__" | "__tests__" | "__mocks__" | "test" | "tests"
            )
        })
}

/// A markup tag: `<name ...>` or `</name>`.
#[derive(Debug, Clone)]
pub struct Tag {
    pub start: usize,
    /// Just past its `>`.
    pub end: usize,
    /// Lowercase for HTML elements; as written for components (`Image`).
    pub name: String,
    pub closing: bool,
    pub self_closing: bool,
}

/// One `property: value` declaration (CSS, an inline style, a JSX style object).
#[derive(Debug, Clone)]
pub struct Decl {
    /// Kebab-case and lowercase (`fontSize` reads as `font-size`).
    pub prop: String,
    pub value: String,
    pub start: usize,
    pub value_start: usize,
    pub value_end: usize,
    /// Written as a JSX style object entry (a bare number is pixels there).
    pub jsx: bool,
}

/// A class-like token: `md:hover:text-[10px]` has the base `text-[10px]`.
#[derive(Debug, Clone)]
pub struct Token {
    /// Where the base starts.
    pub start: usize,
    pub end: usize,
    pub base: String,
    /// The variant prefixes (`md:hover:`), empty without any.
    pub variants: String,
}

/// A run of element text between two tags, its `{…}` expressions removed.
#[derive(Debug, Clone)]
pub struct TextRun {
    pub start: usize,
    pub text: String,
    /// Where each piece of `text` that is contiguous in the file starts:
    /// `(byte in text, byte in file)`, in order.
    pub map: Vec<(usize, usize)>,
}

impl TextRun {
    /// The file offset of byte `at` of the run's text.
    pub fn file_offset(&self, at: usize) -> usize {
        let i = self.map.partition_point(|(t, _)| *t <= at);
        match i.checked_sub(1).and_then(|i| self.map.get(i)) {
            Some((t, f)) => f + (at - t),
            None => self.start,
        }
    }
}

/// A prepared file.
pub struct Source<'a> {
    pub path: &'a str,
    pub kind: UiKind,
    pub raw: &'a str,
    /// `raw` with comments replaced by spaces, byte for byte.
    pub text: String,
    line_starts: Vec<usize>,
    pub tags: Vec<Tag>,
    pub decls: Vec<Decl>,
    pub tokens: Vec<Token>,
    pub runs: Vec<TextRun>,
    /// CSS rules (stylesheets, `<style>` blocks), in order.
    pub css_rules: Vec<CssRule>,
}

/// CSS properties the rules read (others are skipped when declarations are collected).
const KNOWN_PROPS: [&str; 29] = [
    "font",
    "font-size",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-bottom-right-radius",
    "border-bottom-left-radius",
    "font-family",
    "font-weight",
    "line-height",
    "letter-spacing",
    "text-align",
    "text-transform",
    "outline",
    "outline-style",
    "outline-width",
    "color",
    "background",
    "background-color",
    "background-image",
    "background-size",
    "background-clip",
    "-webkit-background-clip",
    "box-shadow",
    "text-shadow",
    "border-radius",
    "animation",
    "animation-name",
    "animation-timing-function",
    "transition-timing-function",
];

/// Border properties the side-tab and accent rules read.
fn is_border_prop(prop: &str) -> bool {
    matches!(
        prop,
        "border-left"
            | "border-right"
            | "border-top"
            | "border-bottom"
            | "border-left-width"
            | "border-right-width"
            | "border-inline-start"
            | "border-inline-end"
            | "border-inline-start-width"
            | "border-inline-end-width"
    )
}

static DECL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(-?-?[A-Za-z][A-Za-z-]*)[ \t]*:[ \t]*").expect("declaration pattern")
});

/// `fontSize` → `font-size`; `WebkitBackgroundClip` → `-webkit-background-clip`.
fn kebab(prop: &str) -> String {
    let mut out = String::with_capacity(prop.len() + 4);
    for (i, c) in prop.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 || prop.starts_with("Webkit") {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `raw` with the `comments` spans replaced by spaces, byte for byte (line
/// breaks kept), so every offset still points where it did.
fn blank(raw: &str, comments: &[(usize, usize)]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut at = 0;
    for &(from, to) in comments {
        if from < at || to > raw.len() {
            continue;
        }
        out.push_str(&raw[at..from]);
        for c in raw[from..to].chars() {
            if c == '\n' || c == '\r' {
                out.push(c);
            } else {
                for _ in 0..c.len_utf8() {
                    out.push(' ');
                }
            }
        }
        at = to;
    }
    out.push_str(&raw[at..]);
    out
}

/// How long the unquoted value at the start of `rest` is. A CSS value ends at
/// `;`, `}`, `{` or a line break, and at a quote it did not open itself (the
/// end of a `style="…"` attribute); a quote it opens (`'Inter'`) is part of
/// it. A JSX style value ends at `,`, `}` or a line break.
fn value_len(rest: &str, jsx: bool) -> usize {
    let bytes = rest.as_bytes();
    let mut open: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = open {
            if b == q {
                open = None;
            } else if b == b'\n' {
                return i;
            }
            i += 1;
            continue;
        }
        let stop = if jsx {
            matches!(b, b',' | b'}' | b'\n')
        } else {
            matches!(b, b';' | b'}' | b'{' | b'\n')
        };
        if stop {
            return i;
        }
        if !jsx && matches!(b, b'"' | b'\'') {
            // A quoted name starts a value or follows a space, comma or `(`.
            let starts_word = i == 0 || matches!(bytes[i - 1], b' ' | b',' | b'(');
            let closes_later = starts_word
                && bytes[i + 1..]
                    .iter()
                    .take_while(|c| !matches!(**c, b';' | b'}' | b'\n'))
                    .any(|c| *c == b);
            if !closes_later {
                return i;
            }
            open = Some(b);
        }
        i += 1;
    }
    bytes.len()
}

impl<'a> Source<'a> {
    /// Prepare `raw` (the file at `path`) for the rules: read it with
    /// tree-sitter ([`super::parse`]), blank its comments, and collect its
    /// tags, element text, declarations and class tokens. `Err` when the file
    /// cannot be read within the parse budget.
    pub fn new(path: &'a str, raw: &'a str, kind: UiKind) -> Result<Self, String> {
        let parsed = super::parse::parse(path, kind, raw)?;
        let text = blank(raw, &parsed.comments);
        let mut line_starts = vec![0];
        line_starts.extend(raw.match_indices('\n').map(|(at, _)| at + 1));
        let runs = parsed
            .runs
            .iter()
            .map(|spans| {
                let mut run_text = String::new();
                let mut map = Vec::with_capacity(spans.len());
                for &(from, to) in spans {
                    map.push((run_text.len(), from));
                    run_text.push_str(&text[from..to]);
                }
                TextRun {
                    start: spans[0].0,
                    text: run_text,
                    map,
                }
            })
            .filter(|run| !run.text.trim().is_empty())
            .collect();
        let mut source = Source {
            path,
            kind,
            raw,
            text,
            line_starts,
            tags: parsed.tags,
            decls: Vec::new(),
            tokens: Vec::new(),
            runs,
            css_rules: parsed.css_rules,
        };
        source.decls = source.find_decls();
        source.tokens = source.find_tokens();
        Ok(source)
    }

    fn find_decls(&self) -> Vec<Decl> {
        let text = &self.text;
        let bytes = text.as_bytes();
        let mut decls = Vec::new();
        for caps in DECL_RE.captures_iter(text) {
            let whole = caps.get(0).expect("match");
            let name = caps.get(1).expect("name");
            // The property starts a declaration: after `{`, `;`, a quote, a comma
            // or white space (never inside a word such as `https:`).
            if name.start() > 0
                && !matches!(
                    bytes[name.start() - 1],
                    b'{' | b';' | b'"' | b'\'' | b' ' | b'\t' | b'\n' | b'\r' | b','
                )
            {
                continue;
            }
            let written = name.as_str();
            let jsx = written.chars().any(|c| c.is_ascii_uppercase());
            let prop = if jsx {
                kebab(written)
            } else {
                written.to_ascii_lowercase()
            };
            if !KNOWN_PROPS.contains(&prop.as_str()) && !is_border_prop(&prop) {
                continue;
            }
            let mut value_start = whole.end();
            let value_end;
            match bytes.get(value_start) {
                Some(q @ (b'"' | b'\'' | b'`')) if jsx || matches!(self.kind, UiKind::Jsx) => {
                    let q = *q;
                    let close = text[value_start + 1..]
                        .bytes()
                        .position(|b| b == q || b == b'\n');
                    let Some(close) = close else { continue };
                    value_start += 1;
                    value_end = value_start + close;
                }
                _ => value_end = value_start + value_len(&text[value_start..], jsx),
            }
            let written_value = &text[value_start..value_end];
            let value = written_value.trim().to_string();
            if value.is_empty() || value.len() > 600 {
                continue;
            }
            // The span of the value itself, without the space around it.
            let value_start =
                value_start + (written_value.len() - written_value.trim_start().len());
            let value_end = value_start + value.len();
            decls.push(Decl {
                prop,
                value,
                start: name.start(),
                value_start,
                value_end,
                jsx,
            });
        }
        decls
    }

    fn find_tokens(&self) -> Vec<Token> {
        let text = &self.text;
        let mut tokens = Vec::new();
        let mut start: Option<usize> = None;
        let mut bracket = 0usize;
        let is_stop = |c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '`' | ',' | '{' | '}' | '(' | ')' | '<' | '>' | ';'
                )
        };
        let push = |s: usize, e: usize, tokens: &mut Vec<Token>| {
            let full = &text[s..e];
            if full.is_empty() || full.len() > 120 {
                return;
            }
            // The base follows the last `:` outside brackets.
            let mut depth = 0usize;
            let mut cut = 0;
            for (i, c) in full.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => depth = depth.saturating_sub(1),
                    ':' if depth == 0 => cut = i + 1,
                    _ => {}
                }
            }
            let base = &full[cut..];
            if base.is_empty()
                || !base.starts_with(|c: char| c.is_ascii_alphabetic() || c == '-' || c == '!')
            {
                return;
            }
            tokens.push(Token {
                start: s + cut,
                end: e,
                base: base.trim_start_matches('!').to_string(),
                variants: full[..cut].to_string(),
            });
        };
        for (i, c) in text.char_indices() {
            match c {
                '[' if start.is_some() => bracket += 1,
                ']' if start.is_some() => bracket = bracket.saturating_sub(1),
                _ => {}
            }
            if bracket == 0 && is_stop(c) {
                if let Some(s) = start.take() {
                    push(s, i, &mut tokens);
                }
            } else if start.is_none() && !is_stop(c) {
                start = Some(i);
                bracket = usize::from(c == '[');
            }
        }
        if let Some(s) = start {
            push(s, text.len(), &mut tokens);
        }
        tokens
    }

    /// The 1-based line of byte `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    /// Where 1-based `line` starts.
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts.get(line - 1).copied().unwrap_or(0)
    }

    /// The text of 1-based `line`, without its line break.
    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts.get(line - 1).copied().unwrap_or(0);
        let end = self
            .line_starts
            .get(line)
            .map(|next| next - 1)
            .unwrap_or(self.raw.len());
        self.raw[start..end.max(start)].trim_end_matches('\r')
    }

    /// The tag `offset` is inside, if any.
    pub fn tag_at(&self, offset: usize) -> Option<&Tag> {
        let i = self.tags.partition_point(|t| t.start <= offset);
        let tag = self.tags.get(i.checked_sub(1)?)?;
        (offset < tag.end).then_some(tag)
    }

    /// The CSS block `{…}` around `offset` and its selector, if any.
    pub fn block_at(&self, offset: usize) -> Option<(&str, &str, usize)> {
        let before = &self.text[..offset];
        let open = before.rfind('{')?;
        if before.rfind('}').is_some_and(|close| close > open) {
            return None;
        }
        let end = self.text[offset..]
            .find('}')
            .map(|at| offset + at)
            .unwrap_or(self.text.len());
        let selector_start = before[..open]
            .rfind(['}', ';', '{'])
            .map(|at| at + 1)
            .unwrap_or(0);
        Some((
            self.text[selector_start..open].trim(),
            &self.text[open + 1..end],
            open,
        ))
    }

    /// What a match at `offset` is read together with: its tag, else its CSS
    /// block, else its line. Returns the text and where it starts (an id).
    pub fn scope_at(&self, offset: usize) -> (&str, usize) {
        if let Some(tag) = self.tag_at(offset) {
            return (&self.text[tag.start..tag.end], tag.start);
        }
        if let Some((_, body, open)) = self.block_at(offset) {
            return (body, open);
        }
        let line = self.line_of(offset);
        let start = self.line_starts[line - 1];
        let end = self
            .line_starts
            .get(line)
            .copied()
            .unwrap_or(self.text.len());
        (&self.text[start..end], start)
    }

    /// The selector of the CSS block around `offset` (empty outside one).
    pub fn selector_at(&self, offset: usize) -> &str {
        if self.tag_at(offset).is_some() {
            return "";
        }
        self.block_at(offset).map(|(s, _, _)| s).unwrap_or("")
    }

    /// The tokens inside `start..end`.
    pub fn tokens_in(&self, start: usize, end: usize) -> impl Iterator<Item = &Token> {
        let first = self.tokens.partition_point(|t| t.start < start);
        self.tokens[first..]
            .iter()
            .take_while(move |t| t.end <= end)
    }

    /// The declarations inside `start..end`.
    pub fn decls_in(&self, start: usize, end: usize) -> impl Iterator<Item = &Decl> {
        let first = self.decls.partition_point(|d| d.start < start);
        self.decls[first..]
            .iter()
            .take_while(move |d| d.start < end)
    }

    /// The first element text right after the tag ending at `end`.
    pub fn text_after(&self, end: usize) -> Option<&TextRun> {
        // Before the next tag: the element's own text, not a child's.
        let next_tag = self
            .tags
            .iter()
            .find(|t| t.start >= end)
            .map(|t| t.start)
            .unwrap_or(usize::MAX);
        self.runs
            .iter()
            .find(|r| r.start >= end && r.start < next_tag)
    }
}

/// What [`attr`] returns for a value set at run time (`alt={label}`).
pub const DYNAMIC: &str = "{…}";

/// A tag's attribute: `None` when absent, `Some("")` when present without a
/// value or with an empty one, [`DYNAMIC`] for a `{…}` value, else its
/// literal value.
pub fn attr<'t>(tag: &'t str, name: &str) -> Option<&'t str> {
    let lower = tag.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let at = from + at;
        from = at + name.len();
        let before_ok = at > 0 && matches!(bytes[at - 1], b' ' | b'\t' | b'\n' | b'\r');
        let after = bytes.get(at + name.len()).copied();
        if !before_ok {
            continue;
        }
        match after {
            Some(b'=') => {
                let rest = &tag[at + name.len() + 1..];
                let rest_trim = rest.trim_start();
                return Some(match rest_trim.as_bytes().first() {
                    Some(q @ (b'"' | b'\'')) => {
                        let q = *q as char;
                        let body = &rest_trim[1..];
                        &body[..body.find(q).unwrap_or(body.len())]
                    }
                    // A `{…}` value is set at run time: [`DYNAMIC`].
                    Some(b'{') => DYNAMIC,
                    _ => rest_trim
                        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                        .next()
                        .unwrap_or(""),
                });
            }
            Some(b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>') | None => return Some(""),
            _ => continue,
        }
    }
    None
}

/// [`attr`], also when bound the Vue way (`:alt`, `v-bind:alt`).
pub fn bound<'t>(tag: &'t str, name: &str) -> Option<&'t str> {
    attr(tag, name)
        .or_else(|| attr(tag, &format!(":{name}")).map(|_| DYNAMIC))
        .or_else(|| attr(tag, &format!("v-bind:{name}")).map(|_| DYNAMIC))
}

/// Whether a tag spreads props (`{...props}`) or binds attributes Shodh
/// cannot read, so a missing attribute may be set at run time.
pub fn has_spread(tag: &str) -> bool {
    tag.contains("{...") || tag.contains("v-bind=\"") || tag.contains("{ ...")
}
