//! A UI file prepared for the rules: its comments blanked (offsets kept), its
//! markup tags, its CSS declarations, its class tokens and its element text.
//! Pure and deterministic: the same text always gives the same parts.

use std::sync::LazyLock;

use regex::Regex;

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

    fn line_comments(self) -> bool {
        matches!(self, Self::Scss | Self::Jsx | Self::Vue | Self::Svelte)
    }

    /// Files with JavaScript: template literals and regular expressions.
    fn scripts(self) -> bool {
        matches!(self, Self::Jsx | Self::Vue | Self::Svelte)
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
static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(/?)([A-Za-z][A-Za-z0-9.:_-]*)").expect("tag pattern"));

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

/// The last character before `i` on the same line that is not a space or a
/// tab (`None` at the start of a line).
fn prev_on_line(bytes: &[u8], i: usize) -> Option<u8> {
    bytes[..i]
        .iter()
        .rev()
        .take_while(|b| **b != b'\n')
        .find(|b| !matches!(**b, b' ' | b'\t' | b'\r'))
        .copied()
}

/// Whether a quote at `i` opens a string: an attribute value inside a tag
/// (after `=`), or a string in code or CSS (after an operator, a bracket, a
/// comma or a colon). A quote after a letter (`Don't`) is prose. JavaScript's
/// backtick always opens a template literal.
fn opens_string(bytes: &[u8], i: usize, in_tag: bool, scripts: bool) -> bool {
    let quote = bytes[i];
    if quote == b'`' {
        return scripts;
    }
    match prev_on_line(bytes, i) {
        None => !in_tag,
        Some(b'=') => true,
        Some(b) => matches!(
            b,
            b'(' | b',' | b'[' | b'{' | b':' | b'?' | b'+' | b'!' | b'&' | b'|' | b';'
        ),
    }
}

/// Just past the string opened at `i`: its closing quote (backslash escapes
/// skipped), or the end of the line for an unclosed `'` or `"`. A template
/// literal may span lines.
fn string_end(bytes: &[u8], i: usize) -> usize {
    let quote = bytes[i];
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'\n' if quote != b'`' => return j,
            b if b == quote => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

/// Just past a regular expression literal at `i` (`/…/flags` on one line),
/// when the `/` there starts one: after an operator, a bracket or a comma,
/// never `//`, `/*` or the `/` of a closing tag.
fn regex_end(bytes: &[u8], i: usize) -> Option<usize> {
    if matches!(bytes.get(i + 1), Some(b'/' | b'*') | None) {
        return None;
    }
    if !matches!(
        prev_on_line(bytes, i),
        Some(b'(' | b',' | b'=' | b':' | b'[' | b'!' | b'&' | b'|' | b'?' | b';')
    ) {
        return None;
    }
    let mut class = false;
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\n' => return None,
            b'\\' => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => return Some(j + 1),
            _ => {}
        }
        j += 1;
    }
    None
}

/// `raw` with `/* */`, `<!-- -->` and (for `line` kinds) `//` comments
/// blanked. Strings, attribute values, template literals and regular
/// expressions are copied as they are: `accept="image/*"`, `'./*.svg'` or
/// `fetch('/api/*')` never start a comment. A `//` is a comment at the start of
/// a line or after code punctuation, never in element text (`<code> // x`)
/// or a URL.
fn blank_comments(raw: &str, kind: UiKind) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    let markup = kind.has_markup();
    let scripts = kind.scripts();
    let mut in_tag = false;
    let blank = |out: &mut String, s: &str| {
        for c in s.chars() {
            if c == '\n' {
                out.push('\n');
            } else {
                for _ in 0..c.len_utf8() {
                    out.push(' ');
                }
            }
        }
    };
    while i < bytes.len() {
        let rest = &raw[i..];
        let b = bytes[i];
        if matches!(b, b'"' | b'\'' | b'`') && opens_string(bytes, i, in_tag, scripts) {
            let end = string_end(bytes, i);
            out.push_str(&raw[i..end]);
            i = end;
            continue;
        }
        if b == b'/' && scripts {
            if let Some(end) = regex_end(bytes, i) {
                out.push_str(&raw[i..end]);
                i = end;
                continue;
            }
        }
        if markup {
            match b {
                b'<' if bytes
                    .get(i + 1)
                    .is_some_and(|n| n.is_ascii_alphabetic() || *n == b'/') =>
                {
                    in_tag = true
                }
                b'>' => in_tag = false,
                _ => {}
            }
        }
        let (open, close) = if rest.starts_with("/*") {
            ("/*", "*/")
        } else if markup && !in_tag && rest.starts_with("<!--") {
            ("<!--", "-->")
        } else if kind.line_comments()
            && rest.starts_with("//")
            && matches!(
                prev_on_line(bytes, i),
                None | Some(b';' | b'{' | b'}' | b')' | b',' | b']')
            )
        {
            ("//", "\n")
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
            continue;
        };
        let body_end = rest[open.len()..]
            .find(close)
            .map(|at| open.len() + at + if close == "\n" { 0 } else { close.len() })
            .unwrap_or(rest.len());
        blank(&mut out, &rest[..body_end]);
        i += body_end;
    }
    out
}

/// Just past the `>` that ends the tag opened at `start`, skipping quoted
/// values and `{…}` expressions. `None` for text that is not a tag.
fn tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut depth = 0usize;
    let limit = (start + 16 * 1024).min(bytes.len());
    let mut i = start + 1;
    while i < limit {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'"' | b'\'' | b'`' => quote = Some(b),
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b'>' if depth == 0 => return Some(i + 1),
                b'<' if depth == 0 && i > start + 1 => return None,
                _ => {}
            },
        }
        i += 1;
    }
    None
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

/// `text` with every balanced `{…}` removed (an unclosed one to the end).
/// Also returns where each kept piece starts: `(byte in the result, byte in
/// `text`)`.
fn without_expressions(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::new();
    let mut depth = 0usize;
    let mut kept_last = false;
    for (i, c) in text.char_indices() {
        let keep = match c {
            '{' => {
                depth += 1;
                false
            }
            '}' if depth > 0 => {
                depth -= 1;
                false
            }
            _ => depth == 0,
        };
        if keep {
            if !kept_last {
                map.push((out.len(), i));
            }
            out.push(c);
        }
        kept_last = keep;
    }
    (out, map)
}

/// Whether a run between JSX tags is code rather than element text.
fn looks_like_code(text: &str) -> bool {
    let t = text.trim();
    t.starts_with(')')
        || t.starts_with('}')
        || t.ends_with('(')
        || t.contains("=>")
        || t.contains("return ")
        || t.contains("function ")
        || t.contains("const ")
        || (t.contains(';') && t.contains('\n'))
}

impl<'a> Source<'a> {
    pub fn new(path: &'a str, raw: &'a str, kind: UiKind) -> Self {
        let text = blank_comments(raw, kind);
        let mut line_starts = vec![0];
        line_starts.extend(raw.match_indices('\n').map(|(at, _)| at + 1));
        let mut source = Source {
            path,
            kind,
            raw,
            text,
            line_starts,
            tags: Vec::new(),
            decls: Vec::new(),
            tokens: Vec::new(),
            runs: Vec::new(),
        };
        if kind.has_markup() {
            source.tags = source.find_tags();
            source.runs = source.find_runs();
        }
        source.decls = source.find_decls();
        source.tokens = source.find_tokens();
        source
    }

    fn find_tags(&self) -> Vec<Tag> {
        let mut tags: Vec<Tag> = Vec::new();
        let mut after = 0;
        for caps in TAG_RE.captures_iter(&self.text) {
            let whole = caps.get(0).expect("match");
            if whole.start() < after {
                continue;
            }
            let Some(end) = tag_end(&self.text, whole.start()) else {
                continue;
            };
            let written = &caps[2];
            let is_html = written
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase());
            let name = if is_html {
                written.to_ascii_lowercase()
            } else {
                written.to_string()
            };
            let self_closing = self.text[..end].trim_end_matches('>').ends_with('/');
            tags.push(Tag {
                start: whole.start(),
                end,
                name,
                closing: &caps[1] == "/",
                self_closing,
            });
            after = end;
        }
        tags
    }

    /// Element text between tags, skipping `<script>` and `<style>` bodies.
    fn find_runs(&self) -> Vec<TextRun> {
        let mut runs = Vec::new();
        let mut skip_until: Option<&str> = None;
        for pair in self.tags.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if let Some(name) = skip_until {
                if a.closing && a.name == name {
                    skip_until = None;
                } else {
                    continue;
                }
            }
            if !a.closing && !a.self_closing && (a.name == "script" || a.name == "style") {
                skip_until = Some(if a.name == "script" {
                    "script"
                } else {
                    "style"
                });
                continue;
            }
            if b.start <= a.end {
                continue;
            }
            let between = &self.text[a.end..b.start];
            let (text, map) = if matches!(self.kind, UiKind::Html) {
                (between.to_string(), vec![(0, a.end)])
            } else {
                if looks_like_code(between) {
                    continue;
                }
                let (text, pieces) = without_expressions(between);
                let map = pieces.into_iter().map(|(t, f)| (t, a.end + f)).collect();
                (text, map)
            };
            if text.trim().is_empty() {
                continue;
            }
            runs.push(TextRun {
                start: a.end,
                text,
                map,
            });
        }
        runs
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
        self.runs.iter().find(|r| r.start == end)
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
