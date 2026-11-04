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
//! - Every registered command is granted to some window, and the permission
//!   files name no other command.
//! - Each capability is keyed by webview label, never by window label: Tauri
//!   grants a window-keyed capability to every webview inside that window,
//!   so a child webview added to `main` (for example an agent-built preview)
//!   would inherit everything `main` has. Each allows local pages only and
//!   grants no file reads and no window management, and the webviews the app
//!   opens are exactly the ones the capabilities name.
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
    const CODE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];

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

    /// `text` with its `//` and `/* */` comments removed (newlines kept, so
    /// line numbers hold). A `'` or `"` string ends at its line's end at the
    /// latest, as in JS, so a quote in a regex literal cannot swallow the
    /// rest of the file; escapes are skipped everywhere, so `\/\/` in a regex
    /// is not a comment.
    pub(super) fn strip_comments(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '\'' | '"' | '`' => {
                    out.push(c);
                    i += 1;
                    while i < chars.len() {
                        let d = chars[i];
                        if d == '\\' {
                            out.push(d);
                            if let Some(&e) = chars.get(i + 1) {
                                out.push(e);
                            }
                            i += 2;
                            continue;
                        }
                        if d == '\n' && c != '`' {
                            break;
                        }
                        out.push(d);
                        i += 1;
                        if d == c {
                            break;
                        }
                    }
                }
                '\\' => {
                    out.push(c);
                    if let Some(&e) = chars.get(i + 1) {
                        out.push(e);
                    }
                    i += 2;
                }
                '/' if chars.get(i + 1) == Some(&'/') => {
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                }
                '/' if chars.get(i + 1) == Some(&'*') => {
                    i += 2;
                    while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                        if chars[i] == '\n' {
                            out.push('\n');
                        }
                        i += 1;
                    }
                    i += 2;
                }
                _ => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        out
    }

    /// `text` (comments already stripped) with every string's contents
    /// blanked to spaces, quotes and newlines kept.
    fn blank_strings(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for c in text.chars() {
            match quote {
                None => {
                    if matches!(c, '\'' | '"' | '`') {
                        quote = Some(c);
                    }
                    out.push(c);
                }
                Some(q) => {
                    if escaped {
                        escaped = false;
                        out.push(if c == '\n' { '\n' } else { ' ' });
                    } else if c == '\\' {
                        escaped = true;
                        out.push(' ');
                    } else if c == q || (c == '\n' && q != '`') {
                        quote = None;
                        out.push(c);
                    } else {
                        out.push(if c == '\n' { '\n' } else { ' ' });
                    }
                }
            }
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
        let text: Vec<char> = blank_strings(&strip_comments(source)).chars().collect();
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
        let mut last_paren: Option<(usize, String)> = None;
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
                    // `) {` or `): Type {` after a parameter list.
                    let after_params = last_paren.as_ref().filter(|(end, _)| {
                        let between: String = text[end + 1..i].iter().collect();
                        let between = between.trim();
                        between.is_empty() || (between.starts_with(':') && !between.contains(';'))
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
                    } else if let Some((_, head)) = after_params {
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
                            last_paren = Some((i, frame.head));
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
            for cap in import.captures_iter(&strip_comments(&read(&file))) {
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
            for cap in literal.captures_iter(&strip_comments(&read(&root.join(file)))) {
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
        let mut problems = Vec::new();

        for (id, allowed) in &permissions {
            if id != MAIN_PERMISSION && id != PRINT_PERMISSION {
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
        let (main_files, mut unresolved) = module_graph(&["main.tsx"], &[PRINT_VIEW]);
        let (print_files, print_unresolved) = module_graph(PRINT_ROOTS, &[]);
        unresolved.extend(print_unresolved);
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

        for command in &commands {
            if !main.contains(command) && !print.contains(command) {
                problems.push(format!(
                    "{command} is registered, but no window is granted it: grant it to the window that calls it, or unregister it"
                ));
            }
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
                if window != MAIN_WINDOW && *window != print_windows {
                    problems.push(format!(
                        "{file}: webview `{window}` is not one the app opens (main, {print_windows})"
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
        for id in app_grants.keys() {
            if id != MAIN_PERMISSION && id != PRINT_PERMISSION {
                problems.push(format!("a capability grants unknown app permission {id}"));
            }
        }

        // The windows the app opens: the config's (the main window) and the
        // print windows of pdf_export.rs, nothing else.
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
                    && path.file_name().is_some_and(|n| n != "acl_manifest.rs")
                    && opens_windows.is_match(&read(&path))
                    && path.file_name().is_some_and(|n| n != "pdf_export.rs")
                {
                    problems.push(format!(
                        "{} opens a window or webview: give it a label and a capability here",
                        path.display()
                    ));
                }
            }
        }
        // Each window the app opens is a WebviewWindow, whose one webview
        // carries the window's label.
        let expected: BTreeSet<String> = [MAIN_WINDOW.to_string(), print_windows.clone()].into();
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
            .build(tauri::generate_context!())
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
            past_the_acl(&result, "write_file not found"),
            "main window: fs write: {result:?}"
        );
        for cmd in [
            "print_job",
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

        // A webview no capability names, e.g. a preview window of its own
        // label: nothing, from the preview origin or the app's.
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

        // On Windows the scheme counts as local only over the protocol the
        // app serves it with (http); the https form is a remote origin.
        if cfg!(windows) {
            let https = preview.replacen("http://", "https://", 1);
            let result = call(&main, "get_app_settings", &https);
            assert!(refused(&result), "{https}: {result:?}");
        }

        // Tripwire, not a guarantee: the Artifacts preview frame lives in the
        // main webview, and Tauri 2.9 counts every registered custom scheme
        // as a local origin (`Webview::is_local_url`, on macOS by scheme, on
        // Windows as http://<scheme>.localhost), so the ACL grants the frame
        // what it grants main; the invoke key is the barrier there
        // (creations/preview.rs). This bypasses the key on purpose. A Tauri
        // that classes the frame as remote refuses it, since no capability
        // allows remote origins: that is an improvement, reported here, not
        // a failure. Anything else is unexpected.
        match call(&main, "get_app_settings", &preview) {
            Ok(answer) if answer == REACHED => {}
            result if refused(&result) => eprintln!(
                "NOTE: the ACL now refuses the preview origin {preview} in the main webview. \
                 The preview is no longer a local origin: update creations/preview.rs and \
                 remove this tripwire."
            ),
            other => panic!("unexpected answer from the preview origin: {other:?}"),
        }
    }
}
