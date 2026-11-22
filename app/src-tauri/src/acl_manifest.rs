//! App ACL gate: every app command is granted to the windows that call it,
//! and to no other window, webview or origin.
//!
//! The app defines its own ACL manifest (`permissions/`), so Tauri checks
//! app commands, not only plugin commands, against the capabilities
//! (`capabilities/`): a window may call a command only when a capability
//! naming that window grants it, on the app's own (local) pages. A command
//! the calling window is not granted fails at runtime, which no build
//! catches, so the static checks below are the gate:
//!
//! - **Main window.** `main-window` allows exactly the commands named in the
//!   frontend modules that `main.tsx` loads for the app shell. A command is
//!   named when a string literal equals it, as in `agent_coverage.rs`, so
//!   wrappers around `invoke` count too; that file's test allows non-literal
//!   commands only in files it declares. Comments are stripped first, so a
//!   command mentioned in a comment is not a use.
//! - **Print windows.** `print-window` allows only commands named in the
//!   print view's modules, and every command named in the modules only the
//!   print view loads.
//! - **The floating chat** (webview `dock`, over the embedded preview).
//!   `dock-window` allows exactly the commands named in the modules only the
//!   dock view loads: its relay commands, which no other permission may
//!   allow, and commands the main window may call itself. Its capability
//!   grants that and event listening, nothing else.
//! - Every registered command is granted to some window, and the permission
//!   files name no other command.
//! - Each capability is keyed by webview label, never by window label: Tauri
//!   grants a window-keyed capability to every webview inside that window,
//!   so a child webview added to `main` (the embedded preview, the dock)
//!   would inherit everything `main` has. Each allows local pages only and
//!   grants no file reads and no window management, and the webviews the app
//!   opens are exactly the ones the capabilities name, plus the Artifacts
//!   previews (`preview-<id>`, a window of their own or a child webview of
//!   `main`), which no capability may name: their agent-built pages hold the
//!   invoke key, so the ACL refuses them every command, plugin command and
//!   event call. A preview is built only by the `locked!` macro, which
//!   carries every lock; only `preview_embed.rs` adds one to `main`, and
//!   `dock_layer.rs` builds the dock alone, denying its every permission.
//! - No module calls `invoke` or `listen` while it is evaluated: `main.tsx`
//!   imports the whole app shell, so every module is evaluated in print
//!   windows too. Code inside functions, methods and arrow functions counts
//!   as deferred unless the function is passed to a call made at load (a
//!   `.then` callback, an IIFE-like `new Promise(..)`), except for the React
//!   wrappers that only store it (`memo`, `forwardRef`, `lazy`). A top-level
//!   call to some other function that invokes inside is not seen.
//!
//! The last test loads the compiled ACL into a mock app and sends requests
//! from each window and origin.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::path::{Path, PathBuf};

    use regex::Regex;
    use serde_json::Value;
    use tauri::test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY};
    use tauri::{WebviewUrl, WebviewWindow, WebviewWindowBuilder};

    use crate::agent_coverage::tests::registered_commands;

    const MAIN_PERMISSION: &str = "main-window";
    const PRINT_PERMISSION: &str = "print-window";
    const MAIN_WINDOW: &str = "main";
    /// The print view's module; `main.tsx` renders it on the print route only.
    const PRINT_VIEW: &str = "features/print/PrintView.tsx";
    /// What `main.tsx` renders on the print route.
    const PRINT_ROOTS: &[&str] = &[
        PRINT_VIEW,
        "contexts/ThemeContext.tsx",
        "components/ErrorBoundary.tsx",
    ];
    const DOCK_PERMISSION: &str = "dock-window";
    /// The floating chat's webview over the embedded preview (dock_layer.rs).
    const DOCK_WEBVIEW: &str = crate::dock_layer::DOCK_WEBVIEW;
    /// The floating chat's module; `main.tsx` renders it on the dock route only.
    const DOCK_VIEW: &str = "features/dock/DockView.tsx";
    /// What `main.tsx` renders on the dock route.
    const DOCK_ROOTS: &[&str] = &[
        DOCK_VIEW,
        "contexts/ThemeContext.tsx",
        "components/ErrorBoundary.tsx",
    ];
    /// The dock's relay commands: granted to the dock alone, never to a
    /// window that could act on what they carry.
    const DOCK_ONLY_COMMANDS: &[&str] = &["dock_layer_action", "dock_layer_size"];
    const CODE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];
    /// Registered commands no webview is granted, with the reason. The ACL
    /// refuses them to every window.
    const UNGRANTED: &[(&str, &str)] = &[(
        "search_documents",
        "no frontend module calls it (only a comment in features/agent/reducer.ts names it); the agent searches through its own search_documents tool, which runs in Rust",
    )];

    fn manifest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn frontend_root() -> PathBuf {
        std::fs::canonicalize(manifest_dir().join("..").join("src"))
            .expect("the frontend sources exist")
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn json(path: &Path) -> Value {
        serde_json::from_str(&read(path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn print_windows() -> String {
        format!("{}*", crate::pdf_export::PRINT_WINDOW_PREFIX)
    }

    fn files_in(dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|entry| entry.expect("directory entry").path())
            .collect();
        out.sort();
        out
    }

    /// The commands each app permission allows, by identifier.
    fn app_permissions() -> BTreeMap<String, BTreeSet<String>> {
        let mut out = BTreeMap::new();
        for path in files_in(&manifest_dir().join("permissions")) {
            assert!(
                path.is_file() && path.extension().is_some_and(|e| e == "json"),
                "{}: the app's permissions are JSON files directly in permissions/, so this test reads every one",
                path.display()
            );
            let file = json(&path);
            for key in file
                .as_object()
                .expect("a permission file is an object")
                .keys()
            {
                assert_eq!(
                    key,
                    "permission",
                    "{}: only plain permissions (no default, no sets), so a grant is always visible by name",
                    path.display()
                );
            }
            for permission in file["permission"].as_array().expect("permission array") {
                let id = permission["identifier"].as_str().expect("identifier");
                assert!(
                    permission.get("scope").is_none(),
                    "{id}: app permissions carry no scope"
                );
                let commands = &permission["commands"];
                assert!(
                    commands
                        .get("deny")
                        .is_none_or(|d| d.as_array().is_some_and(|d| d.is_empty())),
                    "{id}: a command nobody is granted is already denied; deny lists hide nothing"
                );
                let allow: BTreeSet<String> = commands["allow"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{id}: commands.allow"))
                    .iter()
                    .map(|c| c.as_str().expect("command name").to_string())
                    .collect();
                assert!(
                    out.insert(id.to_string(), allow).is_none(),
                    "{id} is defined twice"
                );
            }
        }
        out
    }

    /// `source` up to its unit tests (`#[cfg(test)] mod tests`), which may
    /// build windows of their own in the mock runtime.
    fn without_tests(source: &str) -> &str {
        source
            .find("#[cfg(test)]\nmod tests")
            .map_or(source, |end| &source[..end])
    }

    /// The text of the item starting at `head` (a macro or a function), up
    /// to the brace that closes its body; empty when there is none.
    fn item_body<'a>(code: &'a str, head: &str) -> &'a str {
        let Some(start) = code.find(head) else {
            return "";
        };
        let Some(open) = code[start..].find('{').map(|i| start + i) else {
            return "";
        };
        let mut depth = 0usize;
        for (i, c) in code[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &code[start..open + i + 1];
                    }
                }
                _ => {}
            }
        }
        ""
    }

    fn capabilities() -> Vec<(String, Value)> {
        files_in(&manifest_dir().join("capabilities"))
            .into_iter()
            .map(|path| {
                assert!(
                    path.extension().is_some_and(|e| e == "json"),
                    "{}: capabilities are JSON files, so this test reads every one",
                    path.display()
                );
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (name, json(&path))
            })
            .collect()
    }

    fn permission_id(entry: &Value) -> &str {
        match entry {
            Value::String(id) => id,
            Value::Object(o) => o
                .get("identifier")
                .and_then(Value::as_str)
                .expect("permission identifier"),
            other => panic!("unexpected permission entry {other}"),
        }
    }

    fn relative(path: &Path, root: &Path) -> String {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn with_suffix(base: &Path, suffix: &str) -> PathBuf {
        let mut s = base.as_os_str().to_owned();
        s.push(suffix);
        PathBuf::from(s)
    }

    /// The module a relative or `@/` import names: `None` for packages and
    /// assets, `Err` when no file answers it.
    fn resolve(from: &Path, root: &Path, spec: &str) -> Option<Result<PathBuf, String>> {
        let spec = spec.split(['?', '#']).next().unwrap_or(spec);
        let base = if let Some(rest) = spec.strip_prefix("@/") {
            root.join(rest)
        } else if spec.starts_with("./") || spec.starts_with("../") {
            from.parent().expect("a file has a folder").join(spec)
        } else {
            return None;
        };
        let mut candidates = vec![base.clone()];
        for ext in CODE_EXTENSIONS {
            candidates.push(with_suffix(&base, &format!(".{ext}")));
        }
        for ext in CODE_EXTENSIONS {
            candidates.push(base.join(format!("index.{ext}")));
        }
        let found = candidates.into_iter().find(|c| c.is_file());
        match found {
            Some(file) => {
                let is_code = file
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| CODE_EXTENSIONS.contains(&e));
                is_code.then(|| {
                    std::fs::canonicalize(&file).map_err(|e| format!("{}: {e}", file.display()))
                })
            }
            None => Some(Err(format!(
                "{}: import '{spec}' names no file",
                relative(from, root)
            ))),
        }
    }

    /// Index just past the `'…'` or `"…"` string at `start`. Such a string
    /// ends at its line's end at the latest, as in JS.
    fn quote_end(chars: &[char], start: usize) -> usize {
        let quote = chars[start];
        let mut i = start + 1;
        while i < chars.len() {
            match chars[i] {
                '\\' => i += 2,
                '\n' => return i,
                c if c == quote => return i + 1,
                _ => i += 1,
            }
        }
        chars.len()
    }

    /// Index just past the template literal at `start`, `${…}` included.
    fn template_end(chars: &[char], start: usize) -> usize {
        let mut i = start + 1;
        while i < chars.len() {
            match chars[i] {
                '\\' => i += 2,
                '`' => return i + 1,
                '$' if chars.get(i + 1) == Some(&'{') => i = expression_end(chars, i + 2),
                _ => i += 1,
            }
        }
        chars.len()
    }

    /// Index just past the `}` closing a `${` expression whose code starts
    /// at `start`.
    fn expression_end(chars: &[char], start: usize) -> usize {
        let mut depth = 0usize;
        let mut prev: Option<char> = None;
        let mut i = start;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '\\' => i += 2,
                '`' => i = template_end(chars, i),
                '\'' | '"' => i = quote_end(chars, i),
                '/' if regex_can_start(prev, "", chars.get(i + 1).copied()) => {
                    i = regex_end(chars, i)
                }
                '{' => {
                    depth += 1;
                    i += 1;
                }
                '}' if depth == 0 => return i + 1,
                '}' => {
                    depth -= 1;
                    i += 1;
                }
                _ => i += 1,
            }
            if !c.is_whitespace() {
                prev = Some(c);
            }
        }
        chars.len()
    }

    /// Whether a `/` after `prev` (the last code character) and `word` (the
    /// last word) starts a regex literal rather than dividing. `</` closes a
    /// JSX tag and `/>` ends one, so neither is a regex.
    fn regex_can_start(prev: Option<char>, word: &str, next: Option<char>) -> bool {
        if matches!(next, Some('>' | '/' | '*')) {
            return false;
        }
        match prev {
            None => true,
            Some('<') => false,
            Some(p) if is_word(p) => matches!(
                word,
                "return"
                    | "typeof"
                    | "case"
                    | "do"
                    | "else"
                    | "in"
                    | "of"
                    | "void"
                    | "yield"
                    | "await"
            ),
            Some(p) => !matches!(p, ')' | ']' | '\'' | '"' | '`'),
        }
    }

    /// Index just past the regex literal (flags included) at `start`; a `/`
    /// with no closing `/` on its line is only a slash.
    fn regex_end(chars: &[char], start: usize) -> usize {
        let mut i = start + 1;
        let mut in_class = false;
        while i < chars.len() {
            match chars[i] {
                '\\' => i += 2,
                '\n' => return start + 1,
                '[' => {
                    in_class = true;
                    i += 1;
                }
                ']' => {
                    in_class = false;
                    i += 1;
                }
                '/' if !in_class => {
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    return i;
                }
                _ => i += 1,
            }
        }
        start + 1
    }

    /// `text` with its comments removed (newlines kept, so line numbers
    /// hold). With `blank`, the contents of strings, template literals and
    /// regex literals become spaces too. Template expressions (`${…}`) and
    /// regex literals are followed, so a quote, backtick or `//` inside one
    /// neither opens a string nor starts a comment.
    pub(super) fn scrub(text: &str, blank: bool) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut prev: Option<char> = None;
        let mut word = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            let end = match c {
                '/' if next == Some('/') => {
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                    continue;
                }
                '/' if next == Some('*') => {
                    i += 2;
                    while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                        if chars[i] == '\n' {
                            out.push('\n');
                        }
                        i += 1;
                    }
                    i += 2;
                    continue;
                }
                '\'' | '"' => quote_end(&chars, i),
                '`' => template_end(&chars, i),
                '/' if regex_can_start(prev, &word, next) => regex_end(&chars, i),
                _ => {
                    out.push(c);
                    if is_word(c) {
                        if i == 0 || !is_word(chars[i - 1]) {
                            word.clear();
                        }
                        word.push(c);
                    }
                    if !c.is_whitespace() {
                        prev = Some(c);
                    }
                    i += 1;
                    continue;
                }
            };
            let end = end.min(chars.len()).max(i + 1);
            for (k, &ch) in chars.iter().enumerate().take(end).skip(i) {
                if !blank || k == i || ch == '\n' {
                    out.push(ch);
                } else {
                    out.push(' ');
                }
            }
            // A literal is an operand: a `/` after it divides.
            prev = Some(c);
            word.clear();
            i = end;
        }
        out
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Runs {
        /// Runs while the module is evaluated.
        AtLoad,
        /// Runs only when something calls it later.
        Later,
    }

    struct Frame {
        close: char,
        runs: Runs,
        /// The arguments of a call (not a grouping or a signature's params).
        call_args: bool,
        /// The word before this `(`, e.g. `if` or a function's name.
        head: String,
        /// A concise arrow body, which ends at `,` `;` or its enclosing closer.
        concise: bool,
    }

    /// Calls that keep a function for later instead of calling it.
    const DEFERRING_CALLS: &[&str] = &["memo", "forwardRef", "lazy"];
    /// `kw (...) {` opens a block of the enclosing code, not a function.
    const BLOCK_HEADS: &[&str] = &["if", "for", "while", "switch", "catch", "with"];

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }

    /// `(line, callee)` of every call to one of `callees` that runs while the
    /// module is evaluated.
    pub(super) fn calls_at_load(source: &str, callees: &[String]) -> Vec<(usize, String)> {
        let text: Vec<char> = scrub(source, true).chars().collect();
        let class = Regex::new(r"\bclass\b").unwrap();
        let prev_sig = |i: usize| -> Option<(usize, char)> {
            (0..i)
                .rev()
                .map(|j| (j, text[j]))
                .find(|(_, c)| !c.is_whitespace())
        };
        let word_ending_at = |end: usize| -> String {
            let mut start = end;
            while start > 0 && is_word(text[start - 1]) {
                start -= 1;
            }
            text[start..end].iter().collect()
        };
        let arrow_runs = |frame: &Frame| {
            if frame.call_args && frame.runs == Runs::AtLoad {
                Runs::AtLoad
            } else {
                Runs::Later
            }
        };
        let mut stack = vec![Frame {
            close: '\0',
            runs: Runs::AtLoad,
            call_args: false,
            head: String::new(),
            concise: false,
        }];
        // The last `)` closed: where it was and the word before its `(`.
        let mut last_paren: Option<(usize, String, usize)> = None;
        let mut line = 1;
        let mut found = Vec::new();
        let mut i = 0;
        while i < text.len() {
            let c = text[i];
            let top = stack.last().expect("the module frame stays");
            match c {
                '\n' => {
                    line += 1;
                    // A statement starting at column 0 ends any concise
                    // arrow body left open at module level.
                    if text.get(i + 1).is_some_and(|n| is_word(*n)) {
                        while stack.len() > 1 && stack.last().is_some_and(|f| f.concise) {
                            stack.pop();
                        }
                    }
                }
                // An escaped character (in a regex literal) is never a bracket.
                '\\' => {
                    i += 2;
                    continue;
                }
                ',' | ';' if top.concise => {
                    stack.pop();
                }
                '(' => {
                    let before = prev_sig(i);
                    let head = before
                        .filter(|(_, p)| is_word(*p))
                        .map(|(j, _)| word_ending_at(j + 1))
                        .unwrap_or_default();
                    let call = before.is_some_and(|(_, p)| is_word(p) || p == ')' || p == ']')
                        && !BLOCK_HEADS.contains(&head.as_str())
                        && head != "function";
                    let runs = if call && DEFERRING_CALLS.contains(&head.as_str()) {
                        Runs::Later
                    } else {
                        top.runs
                    };
                    stack.push(Frame {
                        close: ')',
                        runs,
                        call_args: call,
                        head,
                        concise: false,
                    });
                }
                '[' => {
                    let runs = top.runs;
                    stack.push(Frame {
                        close: ']',
                        runs,
                        call_args: false,
                        head: String::new(),
                        concise: false,
                    });
                }
                '{' => {
                    let before = prev_sig(i);
                    let after_arrow =
                        before.is_some_and(|(j, p)| p == '>' && j > 0 && text[j - 1] == '=');
                    // `) {` or `): Type {` after a parameter list closed at
                    // this depth (a return type may hold `{ a: T; b: U }`).
                    let depth = stack.len();
                    let after_params = last_paren.as_ref().filter(|(end, _, closed_at)| {
                        let between: String = text[end + 1..i].iter().collect();
                        let between = between.trim();
                        *closed_at == depth && (between.is_empty() || between.starts_with(':'))
                    });
                    let word_before = before
                        .filter(|(_, p)| is_word(*p))
                        .map(|(j, _)| word_ending_at(j + 1))
                        .unwrap_or_default();
                    let statement_start = (0..i)
                        .rev()
                        .find(|&j| matches!(text[j], ';' | '{' | '}'))
                        .map_or(0, |j| j + 1);
                    let statement: String = text[statement_start..i].iter().collect();
                    let runs = if after_arrow {
                        arrow_runs(top)
                    } else if let Some((_, head, _)) = after_params {
                        if BLOCK_HEADS.contains(&head.as_str()) {
                            top.runs
                        } else {
                            arrow_runs(top)
                        }
                    } else if matches!(word_before.as_str(), "else" | "try" | "finally" | "do") {
                        top.runs
                    } else if class.is_match(&statement) {
                        Runs::Later
                    } else {
                        top.runs
                    };
                    stack.push(Frame {
                        close: '}',
                        runs,
                        call_args: false,
                        head: String::new(),
                        concise: false,
                    });
                }
                ')' | ']' | '}' => {
                    while stack.len() > 1 && stack.last().is_some_and(|f| f.concise) {
                        stack.pop();
                    }
                    if stack.len() > 1 && stack.last().is_some_and(|f| f.close == c) {
                        let frame = stack.pop().expect("checked");
                        if c == ')' {
                            last_paren = Some((i, frame.head, stack.len()));
                        }
                    }
                }
                '=' if text.get(i + 1) == Some(&'>') => {
                    let next = (i + 2..text.len())
                        .map(|j| text[j])
                        .find(|c| !c.is_whitespace());
                    if next != Some('{') {
                        let runs = arrow_runs(top);
                        stack.push(Frame {
                            close: '\0',
                            runs,
                            call_args: false,
                            head: String::new(),
                            concise: true,
                        });
                    }
                    i += 2;
                    continue;
                }
                _ if is_word(c) && (i == 0 || !(is_word(text[i - 1]) || text[i - 1] == '.')) => {
                    let mut end = i;
                    while end < text.len() && is_word(text[end]) {
                        end += 1;
                    }
                    let word: String = text[i..end].iter().collect();
                    let next = (end..text.len())
                        .map(|j| text[j])
                        .find(|c| !c.is_whitespace());
                    if top.runs == Runs::AtLoad
                        && matches!(next, Some('(' | '<'))
                        && callees.iter().any(|callee| *callee == word)
                    {
                        found.push((line, word));
                    }
                    i = end;
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
        found
    }

    /// Every module reachable from `roots` by static or dynamic import, as
    /// paths relative to `app/src`, never entering `excluded`. Type-only
    /// imports count too, which can only add modules.
    fn module_graph(roots: &[&str], excluded: &[&str]) -> (BTreeSet<String>, Vec<String>) {
        let root = frontend_root();
        let import = Regex::new(r#"(?:\bfrom|\bimport)\s*\(?\s*["']([^"'\n]+)["']"#).unwrap();
        let mut seen = BTreeSet::new();
        let mut problems = Vec::new();
        let mut queue: VecDeque<PathBuf> = roots
            .iter()
            .map(|r| std::fs::canonicalize(root.join(r)).unwrap_or_else(|e| panic!("{r}: {e}")))
            .collect();
        while let Some(file) = queue.pop_front() {
            let name = relative(&file, &root);
            if excluded.contains(&name.as_str()) || !seen.insert(name) {
                continue;
            }
            for cap in import.captures_iter(&scrub(&read(&file), false)) {
                match resolve(&file, &root, &cap[1]) {
                    Some(Ok(next)) => queue.push_back(next),
                    Some(Err(problem)) => problems.push(problem),
                    None => {}
                }
            }
        }
        (seen, problems)
    }

    /// Command → the modules (of `files`) that name it.
    fn named_commands(
        files: &BTreeSet<String>,
        commands: &BTreeSet<String>,
    ) -> BTreeMap<String, BTreeSet<String>> {
        let root = frontend_root();
        let literal = Regex::new(r#"["'`]([A-Za-z_][A-Za-z0-9_]*)["'`]"#).unwrap();
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for file in files {
            for cap in literal.captures_iter(&scrub(&read(&root.join(file)), false)) {
                if commands.contains(&cap[1]) {
                    out.entry(cap[1].to_string())
                        .or_default()
                        .insert(file.clone());
                }
            }
        }
        out
    }

    #[test]
    fn every_app_command_is_granted_to_exactly_the_windows_that_call_it() {
        let commands = registered_commands();
        assert!(
            commands.len() > 50,
            "parsed only {} commands",
            commands.len()
        );
        let permissions = app_permissions();
        let main = permissions
            .get(MAIN_PERMISSION)
            .expect("the main-window permission");
        let print = permissions
            .get(PRINT_PERMISSION)
            .expect("the print-window permission");
        let dock = permissions
            .get(DOCK_PERMISSION)
            .expect("the dock-window permission");
        let mut problems = Vec::new();

        for (id, allowed) in &permissions {
            if ![MAIN_PERMISSION, PRINT_PERMISSION, DOCK_PERMISSION].contains(&id.as_str()) {
                problems.push(format!(
                    "permissions/ defines {id}, which no window check here covers"
                ));
            }
            for command in allowed.difference(&commands) {
                problems.push(format!(
                    "{id} allows {command}, which is not a registered command"
                ));
            }
        }

        let main_tsx = read(&frontend_root().join("main.tsx"));
        assert!(
            main_tsx.contains(&format!(
                "path === '/{}'",
                crate::pdf_export::PRINT_VIEW_PATH
            )) && main_tsx.contains("<PrintView />"),
            "main.tsx renders the print view on the print route; PRINT_ROOTS must follow it"
        );
        assert!(
            main_tsx.contains("path === '/dock-view'") && main_tsx.contains("<DockView />"),
            "main.tsx renders the floating chat on the dock route (dock_layer.rs loads dock-view); DOCK_ROOTS must follow it"
        );
        let (main_files, mut unresolved) = module_graph(&["main.tsx"], &[PRINT_VIEW, DOCK_VIEW]);
        let (print_files, print_unresolved) = module_graph(PRINT_ROOTS, &[]);
        let (dock_files, dock_unresolved) = module_graph(DOCK_ROOTS, &[]);
        unresolved.extend(print_unresolved);
        unresolved.extend(dock_unresolved);
        problems.extend(unresolved);
        assert!(
            main_files.len() > 100,
            "the main window's module graph has only {} modules",
            main_files.len()
        );
        let print_only: BTreeSet<&String> = print_files.difference(&main_files).collect();
        assert!(
            print_only.iter().any(|f| f.as_str() == PRINT_VIEW),
            "the print view is loaded by print windows only"
        );
        let dock_only: BTreeSet<String> = dock_files.difference(&main_files).cloned().collect();
        assert!(
            dock_only.contains(DOCK_VIEW),
            "the floating chat's view is loaded by the dock webview only"
        );

        let main_named = named_commands(&main_files, &commands);
        for (command, files) in &main_named {
            if !main.contains(command) {
                problems.push(format!(
                    "{command} is named in {} (main window), but {MAIN_PERMISSION} does not allow it",
                    files.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for command in main {
            if !main_named.contains_key(command) {
                problems.push(format!(
                    "{MAIN_PERMISSION} allows {command}, which no module of the main window names"
                ));
            }
        }

        let print_named = named_commands(&print_files, &commands);
        for (command, files) in &print_named {
            let only_print: Vec<&String> =
                files.iter().filter(|f| print_only.contains(f)).collect();
            if !only_print.is_empty() && !print.contains(command) {
                problems.push(format!(
                    "{command} is named in {} (print view only), but {PRINT_PERMISSION} does not allow it",
                    only_print.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for command in print {
            if !print_named.contains_key(command) {
                problems.push(format!(
                    "{PRINT_PERMISSION} allows {command}, which no module of the print view names"
                ));
            }
        }

        // The dock: exactly the commands its own modules name, which are its
        // relay commands and what main may also call; the relay commands
        // belong to the dock alone.
        let dock_named = named_commands(&dock_only, &commands);
        for (command, files) in &dock_named {
            if !dock.contains(command) {
                problems.push(format!(
                    "{command} is named in {} (dock only), but {DOCK_PERMISSION} does not allow it",
                    files.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for command in dock {
            if !dock_named.contains_key(command) {
                problems.push(format!(
                    "{DOCK_PERMISSION} allows {command}, which no module only the dock loads names"
                ));
            }
            let dock_only_command = DOCK_ONLY_COMMANDS.contains(&command.as_str());
            if !dock_only_command && !main.contains(command) {
                problems.push(format!(
                    "{DOCK_PERMISSION} allows {command}: the dock may call only its relay commands and what the main window may call itself"
                ));
            }
        }
        for command in DOCK_ONLY_COMMANDS {
            if !dock.contains(*command) {
                problems.push(format!("{DOCK_PERMISSION} must allow {command}"));
            }
            for (id, allowed) in &permissions {
                if id != DOCK_PERMISSION && allowed.contains(*command) {
                    problems.push(format!(
                        "{id} allows {command}, which only the dock may call"
                    ));
                }
            }
        }
        for command in &commands {
            let ungranted = UNGRANTED.iter().any(|(c, _)| c == command);
            let granted =
                main.contains(command) || print.contains(command) || dock.contains(command);
            if !granted && !ungranted {
                problems.push(format!(
                    "{command} is registered, but no window is granted it: grant it to the window that calls it, or list it in UNGRANTED with the reason"
                ));
            }
            if granted && ungranted {
                problems.push(format!(
                    "{command} is granted now: remove it from UNGRANTED"
                ));
            }
        }
        for (command, why) in UNGRANTED {
            if !commands.contains(*command) {
                problems.push(format!(
                    "UNGRANTED lists {command}, which is not a registered command"
                ));
            }
            assert!(why.len() >= 20, "UNGRANTED.{command} needs a reason");
        }

        assert!(
            problems.is_empty(),
            "app ACL gaps:\n- {}",
            problems.join("\n- ")
        );
    }

    #[test]
    fn capabilities_name_their_windows_and_allow_local_pages_only() {
        let print_windows = print_windows();
        let fs_allowed = [
            "fs:allow-write-file",
            "fs:allow-write-text-file",
            "fs:deny-default",
        ];
        let mut problems = Vec::new();
        let mut app_grants: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
        let mut windows_named = BTreeSet::new();

        for (file, capability) in capabilities() {
            let object = capability.as_object().expect("a capability is an object");
            for key in object.keys() {
                match key.as_str() {
                    "$schema" | "identifier" | "description" | "local" | "webviews"
                    | "permissions" => {}
                    // Tauri grants a window-keyed capability to every webview
                    // in that window (`resolve_access` matches window OR
                    // webview label), so a child webview added to `main`
                    // would get all of main's grants.
                    "windows" => problems.push(format!(
                        "{file}: key the capability by `webviews`, not `windows`: a window's grants reach every webview it hosts"
                    )),
                    _ => problems.push(format!(
                        "{file}: `{key}` is not allowed (no remote URLs or platform splits: a grant follows a webview label on the app's own pages)"
                    )),
                }
            }
            if capability.get("local") != Some(&Value::Bool(true)) {
                problems.push(format!("{file}: must say \"local\": true"));
            }
            let windows: Vec<String> = capability["webviews"]
                .as_array()
                .map(|w| {
                    w.iter()
                        .map(|w| w.as_str().expect("webview label").to_string())
                        .collect()
                })
                .unwrap_or_default();
            if windows.is_empty() {
                problems.push(format!("{file}: names no webview"));
            }
            for window in &windows {
                if window != MAIN_WINDOW && *window != print_windows && window != DOCK_WEBVIEW {
                    problems.push(format!(
                        "{file}: webview `{window}` is not one the app opens (main, {print_windows}, {DOCK_WEBVIEW})"
                    ));
                }
                windows_named.insert(window.clone());
            }
            for entry in capability["permissions"].as_array().expect("permissions") {
                let id = permission_id(entry);
                if !id.contains(':') {
                    app_grants
                        .entry(id.to_string())
                        .or_default()
                        .push(windows.clone());
                } else if id.starts_with("fs:") {
                    if entry.is_object() || !fs_allowed.contains(&id) {
                        problems.push(format!(
                            "{file}: {id} is not allowed: the webview reads no files through the fs plugin and writes only where a save dialog put the path"
                        ));
                    }
                } else if id == "core:default"
                    || ((id.starts_with("core:window:") || id.starts_with("core:webview:"))
                        && id != "core:webview:allow-internal-toggle-devtools")
                {
                    problems.push(format!(
                        "{file}: {id} is not allowed: no frontend code manages windows or webviews"
                    ));
                }
            }
        }

        let expect_grant = |id: &str, window: &str, problems: &mut Vec<String>| {
            match app_grants.get(id).map(Vec::as_slice) {
                Some([windows]) if windows.len() == 1 && windows[0] == window => {}
                other => problems.push(format!(
                    "{id} must be granted by exactly one capability, for webview `{window}` only; found {other:?}"
                )),
            }
        };
        expect_grant(MAIN_PERMISSION, MAIN_WINDOW, &mut problems);
        expect_grant(PRINT_PERMISSION, &print_windows, &mut problems);
        expect_grant(DOCK_PERMISSION, DOCK_WEBVIEW, &mut problems);
        for id in app_grants.keys() {
            if ![MAIN_PERMISSION, PRINT_PERMISSION, DOCK_PERMISSION].contains(&id.as_str()) {
                problems.push(format!("a capability grants unknown app permission {id}"));
            }
        }

        // The dock: one capability, for the dock alone, with its relay
        // permission and event listening only (no plugin, no device).
        let dock_grant: BTreeSet<String> = [
            DOCK_PERMISSION,
            "core:event:allow-listen",
            "core:event:allow-unlisten",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect();
        for (file, capability) in capabilities() {
            let labels: Vec<&str> = capability["webviews"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !labels.contains(&DOCK_WEBVIEW) {
                continue;
            }
            if labels != [DOCK_WEBVIEW] {
                problems.push(format!(
                    "{file}: the dock's capability names the dock alone, not {labels:?}"
                ));
            }
            let granted: BTreeSet<String> = capability["permissions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| permission_id(entry).to_string())
                .collect();
            if granted != dock_grant {
                problems.push(format!(
                    "{file}: the dock is granted {granted:?}; it may have exactly {dock_grant:?}"
                ));
            }
        }

        // The Artifacts preview windows: no capability may name their label,
        // exactly or by a pattern, so the ACL refuses them everything.
        let preview_label = crate::creations::preview_window::new_label();
        for (file, capability) in capabilities() {
            for label in capability["webviews"].as_array().into_iter().flatten() {
                let label = label.as_str().unwrap_or_default();
                let matches = match label.strip_suffix('*') {
                    Some(prefix) => preview_label.starts_with(prefix),
                    None => label == preview_label,
                };
                if matches || label.contains(crate::creations::preview::WEBVIEW_PREFIX) {
                    problems.push(format!(
                        "{file}: webview `{label}` reaches the preview window {preview_label}; agent-built pages must hold no grants"
                    ));
                }
            }
        }

        // The windows and webviews the app opens: the config's (the main
        // window), the print windows of pdf_export.rs, the Artifacts
        // previews, which no capability names, and the floating chat over
        // the embedded preview (`dock`). A preview, in its own window
        // (creations/preview_window.rs) or embedded in the main window
        // (creations/preview_embed.rs), is built only by the `locked!` macro,
        // which carries every lock.
        let source = |name: &str| read(&manifest_dir().join("src").join(name));
        let preview_source = source("creations/preview_window.rs");
        let embed_source = source("creations/preview_embed.rs");
        let dock_source = source("dock_layer.rs");
        let preview_code = without_tests(&preview_source);
        let embed_code = without_tests(&embed_source);
        let dock_code = without_tests(&dock_source);
        let locked_macro = item_body(preview_code, "macro_rules! locked");
        if locked_macro.is_empty() {
            problems.push("creations/preview_window.rs has no locked! macro".to_string());
        }
        for lock in [
            ".incognito(true)",
            ".devtools(cfg!(debug_assertions))",
            ".initialization_script_for_all_frames(preview::GUARD_SCRIPT)",
            ".on_navigation(",
            ".on_new_window(",
            ".on_download(",
            ".data_directory(opening.data_dir.clone())",
            ".additional_browser_args(&browser_args(",
            "crate::profile::process_browser_args()",
        ] {
            if !locked_macro.contains(lock) {
                problems.push(format!(
                    "the locked! macro (creations/preview_window.rs) no longer locks a preview with {lock}"
                ));
            }
        }
        for lock in [
            "COREWEBVIEW2_PERMISSION_STATE_DENY",
            "add_ProcessFailed",
            "WindowEvent::Destroyed => on_preview_destroyed(",
            "lock_platform(app, opening.label, Place::Window,",
        ] {
            if !preview_code.contains(lock) {
                problems.push(format!(
                    "creations/preview_window.rs no longer locks its preview with {lock}"
                ));
            }
        }
        // A normal window: the user's own close keys close it.
        if preview_source.contains("prevent_close(") {
            problems.push(
                "creations/preview_window.rs swallows a close request: Alt+F4 and Cmd+W must close the preview"
                    .to_string(),
            );
        }
        // Every preview builder starts inside locked!, and only the embedded
        // preview adds a child, with the locked builder.
        let any_builder = Regex::new(r"\b(?:WebviewWindowBuilder|WebviewBuilder)::new\(").unwrap();
        let locked_builder =
            Regex::new(r"locked!\(\s*(?:WebviewWindowBuilder|WebviewBuilder)::new\(").unwrap();
        let window_builder = Regex::new(r"\bWindowBuilder::new\(").unwrap();
        let builders = any_builder.find_iter(preview_code).count();
        if builders == 0 || builders != locked_builder.find_iter(preview_code).count() {
            problems.push(
                "creations/preview_window.rs builds a webview outside locked!: every preview gets every lock"
                    .to_string(),
            );
        }
        if window_builder.is_match(preview_code) || preview_code.contains(".add_child(") {
            problems.push(
                "creations/preview_window.rs builds a bare window or adds a child webview: only preview_embed.rs embeds, with locked_webview"
                    .to_string(),
            );
        }
        let children = embed_code.matches(".add_child(").count();
        let locked_children = Regex::new(r"\.add_child\(\s*preview_window::locked_webview\(")
            .unwrap()
            .find_iter(embed_code)
            .count();
        if children == 0 || children != locked_children {
            problems.push(
                "creations/preview_embed.rs embeds a webview that is not preview_window::locked_webview"
                    .to_string(),
            );
        }
        if any_builder.is_match(embed_code) || window_builder.is_match(embed_code) {
            problems.push(
                "creations/preview_embed.rs builds a webview or window of its own: embed only the locked preview"
                    .to_string(),
            );
        }
        if !embed_code.contains("preview_window::lock_platform(") {
            problems.push(
                "creations/preview_embed.rs no longer sets WebView2's permission and crash handlers (lock_platform)"
                    .to_string(),
            );
        }
        // The dock: exactly one webview, labelled dock, every permission
        // request denied.
        let dock_builders: Vec<&str> = any_builder
            .find_iter(dock_code)
            .map(|m| m.as_str())
            .collect();
        let dock_labelled = Regex::new(r"WebviewBuilder::new\(\s*DOCK_WEBVIEW\s*,")
            .unwrap()
            .find_iter(dock_code)
            .count();
        if dock_builders.len() != 1 || dock_labelled != 1 || window_builder.is_match(dock_code) {
            problems.push(format!(
                "dock_layer.rs must build exactly one webview, WebviewBuilder::new(DOCK_WEBVIEW, ..); it builds {dock_builders:?}"
            ));
        }
        if !dock_code.contains(".on_permission_request(deny_permissions)")
            || !item_body(dock_code, "fn deny_permissions").contains("PermissionResponse::Deny")
        {
            problems.push(
                "dock_layer.rs no longer denies the dock's every permission request (deny_permissions)"
                    .to_string(),
            );
        }
        let config = json(&manifest_dir().join("tauri.conf.json"));
        let config_windows: Vec<&str> = config["app"]["windows"]
            .as_array()
            .expect("app.windows")
            .iter()
            .map(|w| w["label"].as_str().expect("every window has a label"))
            .collect();
        if config_windows != [MAIN_WINDOW] {
            problems.push(format!(
                "tauri.conf.json opens {config_windows:?}; give each window its own capability here"
            ));
        }
        let opens_windows = Regex::new(
            r"\b(?:WebviewWindowBuilder|WebviewBuilder|WindowBuilder)::new\(|\.add_child\(",
        )
        .unwrap();
        let mut stack = vec![manifest_dir().join("src")];
        while let Some(dir) = stack.pop() {
            for path in files_in(&dir) {
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        ![
                            "acl_manifest.rs",
                            "pdf_export.rs",
                            "preview_window.rs",
                            "preview_embed.rs",
                            "dock_layer.rs",
                            "lib.rs",
                        ]
                        .contains(&n)
                    })
                    && opens_windows.is_match(&read(&path))
                {
                    problems.push(format!(
                        "{} opens a window or webview: give it a label and a capability here",
                        path.display()
                    ));
                }
            }
        }
        // Each window the app opens is a WebviewWindow, whose one webview
        // carries the window's label, and the dock is a child webview of the
        // main window, with its own label and capability.
        let expected: BTreeSet<String> = [
            MAIN_WINDOW.to_string(),
            print_windows.clone(),
            DOCK_WEBVIEW.to_string(),
        ]
        .into();
        if windows_named != expected {
            problems.push(format!(
                "capabilities name the webviews {windows_named:?}, the app opens {expected:?}"
            ));
        }

        assert!(
            problems.is_empty(),
            "capability problems:\n- {}",
            problems.join("\n- ")
        );
    }

    #[test]
    fn no_module_calls_invoke_at_its_top_level() {
        // Print windows evaluate every module main.tsx imports, so a call
        // made at load would run there with the print grants.
        let alias = Regex::new(r"\binvoke\s+as\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap();
        let root = frontend_root();
        let (files, _) = module_graph(&["main.tsx"], &[]);
        let mut problems = Vec::new();
        for file in files {
            let text = read(&root.join(&file));
            let mut callees = vec!["invoke".to_string(), "listen".to_string()];
            callees.extend(alias.captures_iter(&text).map(|c| c[1].to_string()));
            for (line, callee) in calls_at_load(&text, &callees) {
                problems.push(format!("{file}:{line}: {callee}"));
            }
        }
        assert!(
            problems.is_empty(),
            "calls made while the module loads (call them from a component or a function):\n- {}",
            problems.join("\n- ")
        );
    }

    #[test]
    fn the_load_time_scan_tells_load_from_later() {
        let callees = vec!["invoke".to_string(), "listen".to_string()];
        let lines = |src: &str| -> Vec<usize> {
            calls_at_load(src, &callees)
                .into_iter()
                .map(|(l, _)| l)
                .collect()
        };
        // Deferred: functions, methods, arrows kept for later, React wrappers,
        // class members, comments and strings.
        let later = [
            "export async function load(): Promise<X> {\n  return invoke('a');\n}",
            "const f = async () => invoke<T>('a');\nconst g = () => {\n  listen('e', h);\n};",
            "export const api = {\n  async get() { return invoke('a'); },\n  put: (x) => invoke('b', { x }),\n};",
            "export default memo(function Card() {\n  useEffect(() => { invoke('a'); }, []);\n  return null;\n});",
            "const C = forwardRef((p, r) => {\n  useEffect(() => { void invoke('a'); });\n});",
            "class A extends B<P> {\n  x = () => invoke('a');\n  m() { invoke('b'); }\n}",
            "// invoke('a');\n/* invoke('b'); */\nconst s = \"invoke('c')\";\nconst re = /\\/\\//;",
            "if (ok) {\n  const h = () => invoke('a');\n}",
            // A return type holding `;`, and a template expression whose
            // regex holds a backtick and a quote.
            "export async function f(p: string): Promise<{ a: string; b: number }> {\n  return invoke('a', { p });\n}",
            "function g(x) {\n  const d = x ? `a: \\`${x.replace(/`/g, \"'\")}\\`` : '';\n  return invoke('a', { d });\n}",
            // JSX: closing tags, self-closing tags and an apostrophe in text.
            "export function C() {\n  useEffect(() => { listen('e', h); }, []);\n  return <div className=\"a\">it's <b>{x}</b><br /></div>;\n}",
        ];
        for src in later {
            assert!(lines(src).is_empty(), "{src}: {:?}", lines(src));
        }
        // At load: top-level calls, inside top-level blocks, callbacks run
        // by a call made at load, and an initializer.
        assert_eq!(lines("invoke('a');"), vec![1]);
        assert_eq!(
            lines("const x = 1;\nexport const v = await invoke<T>('a');"),
            vec![2]
        );
        assert_eq!(
            lines("if (import.meta.env.DEV) {\n  invoke('a');\n}"),
            vec![2]
        );
        assert_eq!(lines("start().then(() => invoke('a'));"), vec![1]);
        assert_eq!(
            lines("new Promise((r) => {\n  listen('e', r);\n});"),
            vec![2]
        );
        assert_eq!(lines("const f = () => 1;\nlisten('e', f);"), vec![2]);
        // Brackets inside literals do not shift the frames: code after them
        // is still seen at its true level.
        assert_eq!(
            lines("function g(x) {\n  return `${x.replace(/`/g, \"(\")}` + ')';\n}\ninvoke('a');"),
            vec![4]
        );
        assert_eq!(
            lines("const re = /[\"'{(]/g;\nconst s = '}';\ninvoke('a');"),
            vec![3]
        );
        let aliased = vec!["call".to_string()];
        assert_eq!(
            calls_at_load("call('a');", &aliased),
            vec![(1, "call".to_string())]
        );
    }

    const REACHED: &str = "the command handler ran";

    fn local_page() -> &'static str {
        // The app's own pages (Tauri's protocol URL on each platform).
        if cfg!(windows) {
            "http://tauri.localhost/"
        } else {
            "tauri://localhost/"
        }
    }

    fn preview_page() -> String {
        format!(
            "{}/{}/index.html",
            crate::creations::preview::origin(),
            "0".repeat(32)
        )
    }

    fn window(app: &tauri::App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
        WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
            .build()
            .unwrap_or_else(|e| panic!("window {label}: {e}"))
    }

    /// Sends `cmd` from `url` in `window`, past the invoke key.
    fn call(window: &WebviewWindow<MockRuntime>, cmd: &str, url: &str) -> Result<String, String> {
        get_ipc_response(
            window,
            tauri::webview::InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: url.parse().expect("request URL"),
                body: tauri::ipc::InvokeBody::default(),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| {
            body.deserialize::<String>()
                .expect("the handler answers text")
        })
        .map_err(|e| e.to_string())
    }

    /// Refused by the ACL itself. Tauri's ACL rejections (debug builds,
    /// `RuntimeAuthority::resolve_access_message`) say "not allowed", or
    /// "explicitly denied" for a deny rule.
    fn refused(result: &Result<String, String>) -> bool {
        matches!(result, Err(e) if e.contains("not allowed") || e.contains("explicitly denied"))
    }

    /// Got past the ACL to the plugin, which then failed for the reason
    /// given: the mock app registers no app plugins, and the core plugins
    /// get no arguments here.
    fn past_the_acl(result: &Result<String, String>, reason: &str) -> bool {
        !refused(result) && matches!(result, Err(e) if e.contains(reason))
    }

    #[test]
    fn the_compiled_acl_refuses_every_other_window_and_origin() {
        // The real context: this app's capabilities and permissions as the
        // build compiled them. The handler answers any app command, so an app
        // command that reaches it got past the ACL.
        let app = mock_builder()
            .register_uri_scheme_protocol(crate::creations::preview::SCHEME, |_ctx, _request| {
                tauri::http::Response::new(Vec::<u8>::new())
            })
            .invoke_handler(|invoke| {
                invoke.resolver.resolve(REACHED);
                true
            })
            .build(crate::compiled_context())
            .expect("the app builds with its compiled ACL");
        let main = window(&app, MAIN_WINDOW);
        let print = window(
            &app,
            &format!("{}1", crate::pdf_export::PRINT_WINDOW_PREFIX),
        );
        let other = window(&app, "creation-preview");
        let local = local_page();
        let preview = preview_page();
        let reached = Ok(REACHED.to_string());
        const NO_ARGUMENT: &str = "missing required key";

        // The main window, on the app's pages: its commands and plugin calls
        // get past the ACL; the print view's command and fs reads do not.
        assert_eq!(call(&main, "get_app_settings", local), reached);
        assert_eq!(call(&main, "creation_delete", local), reached);
        let result = call(&main, "plugin:event|listen", local);
        assert!(
            past_the_acl(&result, NO_ARGUMENT),
            "main window: listen: {result:?}"
        );
        let result = call(&main, "plugin:fs|write_file", local);
        assert!(
            past_the_acl(&result, "plugin fs not found"),
            "main window: fs write: {result:?}"
        );
        for cmd in [
            "print_job",
            "search_documents",
            "plugin:fs|read_file",
            "plugin:fs|read_text_file",
        ] {
            let result = call(&main, cmd, local);
            assert!(refused(&result), "main window: {cmd}: {result:?}");
        }

        // Any remote page, even in the main window: nothing.
        for url in ["https://example.com/", "http://localhost.evil.example/"] {
            for cmd in ["get_app_settings", "agent_approve", "plugin:event|listen"] {
                let result = call(&main, cmd, url);
                assert!(refused(&result), "{cmd} from {url}: {result:?}");
            }
        }

        // A print window: its job and figure reads only.
        assert_eq!(call(&print, "print_job", local), reached);
        assert_eq!(call(&print, "read_source_bytes", local), reached);
        let result = call(&print, "plugin:event|listen", local);
        assert!(
            past_the_acl(&result, NO_ARGUMENT),
            "print window: listen: {result:?}"
        );
        for cmd in [
            "get_app_settings",
            "creation_delete",
            "agent_approve",
            "update_app_preferences",
            "plugin:fs|write_file",
            "plugin:dialog|save",
            "plugin:opener|open_url",
        ] {
            let result = call(&print, cmd, local);
            assert!(refused(&result), "print window: {cmd}: {result:?}");
        }

        // The floating chat over the embedded preview: its relay commands
        // and listening only. It is refused main's commands (a main-only
        // read, main's publish, a plugin command main has) and every write.
        let dock = window(&app, DOCK_WEBVIEW);
        assert_eq!(call(&dock, "dock_layer_action", local), reached);
        assert_eq!(call(&dock, "dock_layer_size", local), reached);
        let result = call(&dock, "plugin:event|listen", local);
        assert!(
            past_the_acl(&result, NO_ARGUMENT),
            "dock: listen: {result:?}"
        );
        for cmd in [
            "creation_preview_status",
            "dock_layer_publish",
            "plugin:path|resolve_directory",
            "get_app_settings",
            "agent_approve",
            "creation_delete",
            "print_job",
            "plugin:fs|write_file",
            "plugin:dialog|save",
            "plugin:opener|open_url",
            "plugin:clipboard-manager|read_text",
            "plugin:notification|request_permission",
        ] {
            let result = call(&dock, cmd, local);
            assert!(refused(&result), "dock: {cmd}: {result:?}");
        }
        for url in ["https://example.com/", "http://localhost.evil.example/"] {
            let result = call(&dock, "dock_layer_action", url);
            assert!(
                refused(&result),
                "dock: dock_layer_action from {url}: {result:?}"
            );
        }
        // No other window may send what only the dock sends.
        for (name, other) in [("main", &main), ("print", &print)] {
            for cmd in DOCK_ONLY_COMMANDS {
                let result = call(other, cmd, local);
                assert!(refused(&result), "{name}: {cmd}: {result:?}");
            }
        }

        // A webview no capability names: nothing, from the preview origin or
        // the app's.
        for url in [preview.as_str(), local] {
            for cmd in [
                "creation_delete",
                "agent_approve",
                "get_app_settings",
                "plugin:event|listen",
            ] {
                let result = call(&other, cmd, url);
                assert!(refused(&result), "{cmd} from {url}: {result:?}");
            }
        }

        // The Artifacts preview window (creations/preview_window.rs): its page
        // is a main frame and holds the invoke key, so the ACL is the barrier.
        // It is refused every registered command, every plugin command the
        // app's windows use and more, and every event call, from its own
        // origin (both Windows forms), the app's pages and the web.
        let shown = window(&app, &crate::creations::preview_window::new_label());
        let mut origins = vec![preview.clone(), local.to_string()];
        if cfg!(windows) {
            origins.push(preview.replacen("http://", "https://", 1));
        }
        origins.push("https://example.com/".to_string());
        let mut commands: Vec<String> = registered_commands().into_iter().collect();
        commands.extend(
            PREVIEW_REFUSED_PLUGIN_COMMANDS
                .iter()
                .map(|c| c.to_string()),
        );
        let mut reached_preview = Vec::new();
        for url in &origins {
            for cmd in &commands {
                let result = call(&shown, cmd, url);
                if !refused(&result) {
                    reached_preview.push(format!("{cmd} from {url}: {result:?}"));
                }
            }
        }
        assert!(
            reached_preview.is_empty(),
            "the preview window got past the ACL:\n- {}",
            reached_preview.join("\n- ")
        );
    }

    /// Plugin commands sent from the preview window: every one the app's
    /// windows are granted, the event calls (a page may neither hear nor send
    /// app events), and the window, webview and file calls no window has.
    const PREVIEW_REFUSED_PLUGIN_COMMANDS: &[&str] = &[
        "plugin:event|listen",
        "plugin:event|unlisten",
        "plugin:event|emit",
        "plugin:event|emit_to",
        "plugin:path|resolve_directory",
        "plugin:app|version",
        "plugin:webview|internal_toggle_devtools",
        "plugin:webview|create_webview_window",
        "plugin:webview|create_webview",
        "plugin:webview|set_webview_position",
        "plugin:window|show",
        "plugin:window|close",
        "plugin:window|set_position",
        "plugin:window|set_focus",
        "plugin:dialog|ask",
        "plugin:dialog|open",
        "plugin:dialog|save",
        "plugin:dialog|message",
        "plugin:dialog|confirm",
        "plugin:opener|open_url",
        "plugin:opener|open_path",
        "plugin:opener|reveal_item_in_dir",
        "plugin:fs|write_file",
        "plugin:fs|write_text_file",
        "plugin:fs|read_file",
        "plugin:fs|read_text_file",
        "plugin:fs|read_dir",
        "plugin:clipboard-manager|write_image",
        "plugin:clipboard-manager|write_text",
        "plugin:clipboard-manager|read_text",
        "plugin:notification|is_permission_granted",
        "plugin:notification|request_permission",
        "plugin:notification|notify",
        "plugin:updater|check",
        "plugin:autostart|enable",
    ];

    /// No module of the app shell names the preview scheme: agent-built pages
    /// load only in their own preview window (whose address Rust picks), never
    /// in a frame or page of the main webview. With the scheme refusing every
    /// other webview (creations/preview.rs) and `frame-src` naming no preview
    /// (the same file's tests), this catches the preview moving back into
    /// `main`, where every custom scheme counts as a local origin and so
    /// would resolve to main's grants.
    #[test]
    fn the_app_shell_never_loads_a_preview_itself() {
        let root = frontend_root();
        let (files, unresolved) = module_graph(&["main.tsx"], &[]);
        assert!(unresolved.is_empty(), "{unresolved:?}");
        let mut problems = Vec::new();
        for file in &files {
            let text = read(&root.join(file));
            if text.contains(crate::creations::preview::SCHEME) {
                problems.push(format!(
                    "{file} names {}",
                    crate::creations::preview::SCHEME
                ));
            }
        }
        let page = read(&root.join("features/artifacts/ArtifactPage.tsx"));
        if page.contains("<iframe") {
            problems.push("ArtifactPage.tsx renders a frame".to_string());
        }
        assert!(
            problems.is_empty(),
            "the app shell would load agent code itself:\n- {}",
            problems.join("\n- ")
        );
    }

    /// Every Rust source of the app, but this file.
    fn app_sources() -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![manifest_dir().join("src")];
        while let Some(dir) = stack.pop() {
            for path in files_in(&dir) {
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_some_and(|n| n != "acl_manifest.rs")
                {
                    let text = read(&path);
                    out.push((path, text));
                }
            }
        }
        out
    }

    /// Tauri answers `plugin:__TAURI_CHANNEL__|fetch` without asking the ACL
    /// (`ipc::channel::FETCH_CHANNEL_DATA_COMMAND`), and channel data is
    /// app-wide by id. The preview window's page holds the invoke key, so
    /// once any command used an `ipc::Channel` the page could poll its ids.
    /// Before adopting channels, give them a check of their own.
    #[test]
    fn no_command_uses_an_ipc_channel_the_acl_does_not_check() {
        let channel =
            Regex::new(r"\bipc::Channel\b|\bChannel\s*<|ipc::\{[^}]*\bChannel\b").unwrap();
        let found: Vec<String> = app_sources()
            .into_iter()
            .filter(|(_, text)| channel.is_match(text))
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            found.is_empty(),
            "these use tauri::ipc::Channel, whose data fetch skips the ACL (an Artifacts preview could read it): {found:?}"
        );
    }

    /// On macOS the webview grants every camera and microphone request
    /// without asking (wry's UI delegate), and the preview window cannot turn
    /// that off; only the absence of the usage keys keeps macOS from giving
    /// a page the device. Adding a key (meeting capture) needs a per-webview
    /// denial for previews first.
    #[test]
    fn no_media_usage_key_ships_while_previews_could_get_devices() {
        let keys = ["NSMicrophoneUsageDescription", "NSCameraUsageDescription"];
        let mut found = Vec::new();
        let mut stack = vec![manifest_dir()];
        while let Some(dir) = stack.pop() {
            for path in files_in(&dir) {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                if path.is_dir() {
                    if !["target", "gen", "node_modules", "icons"].contains(&name.as_str()) {
                        stack.push(path);
                    }
                } else if name.ends_with(".plist")
                    || name.ends_with(".entitlements")
                    || name == "tauri.conf.json"
                    || (name.starts_with("tauri.") && name.ends_with(".conf.json"))
                {
                    let text = read(&path);
                    for key in keys {
                        if text.contains(key) {
                            found.push(format!("{} names {key}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "a media usage key would let every Artifacts preview page use the device without a prompt on macOS: deny media capture in the preview webview first.\n- {}",
            found.join("\n- ")
        );
    }
}
