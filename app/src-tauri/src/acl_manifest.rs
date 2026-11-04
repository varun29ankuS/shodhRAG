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
//!   commands only in files it declares.
//! - **Print windows.** `print-window` allows only commands named in the
//!   print view's modules, and every command named in the modules only the
//!   print view loads.
//! - Every registered command is granted to some window, and the permission
//!   files name no other command.
//! - Each capability names its windows (never webviews), allows local pages
//!   only, grants no file reads and no window management, and the windows
//!   the app opens are exactly the ones the capabilities name.
//! - No module calls `invoke` at its top level: `main.tsx` imports the whole
//!   app shell, so every module is evaluated in print windows too.
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
            for cap in import.captures_iter(&read(&file)) {
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
            for cap in literal.captures_iter(&read(&root.join(file))) {
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
                if !matches!(
                    key.as_str(),
                    "$schema" | "identifier" | "description" | "local" | "windows" | "permissions"
                ) {
                    problems.push(format!(
                        "{file}: `{key}` is not allowed (no webviews, remote URLs or platform splits: a grant follows a window label on the app's own pages)"
                    ));
                }
            }
            if capability.get("local") != Some(&Value::Bool(true)) {
                problems.push(format!("{file}: must say \"local\": true"));
            }
            let windows: Vec<String> = capability["windows"]
                .as_array()
                .map(|w| {
                    w.iter()
                        .map(|w| w.as_str().expect("window label").to_string())
                        .collect()
                })
                .unwrap_or_default();
            if windows.is_empty() {
                problems.push(format!("{file}: names no window"));
            }
            for window in &windows {
                if window != MAIN_WINDOW && *window != print_windows {
                    problems.push(format!(
                        "{file}: window `{window}` is not one the app opens (main, {print_windows})"
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
                    "{id} must be granted by exactly one capability, for window `{window}` only; found {other:?}"
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
        let expected: BTreeSet<String> = [MAIN_WINDOW.to_string(), print_windows.clone()].into();
        if windows_named != expected {
            problems.push(format!(
                "capabilities name {windows_named:?}, the app opens {expected:?}"
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
        // Print windows evaluate every module main.tsx imports, so a
        // top-level call would run there with the print grants.
        let top_level = Regex::new(
            r"^(?:export\s+)?(?:(?:const|let|var)\s+[A-Za-z_$][\w$]*\s*=\s*)?(?:void\s+|await\s+)?invoke\s*(?:<[^()]*>)?\s*\(",
        )
        .unwrap();
        let root = frontend_root();
        let (files, _) = module_graph(&["main.tsx"], &[]);
        let mut problems = Vec::new();
        for file in files {
            for (n, line) in read(&root.join(&file)).lines().enumerate() {
                if top_level.is_match(line) {
                    problems.push(format!("{file}:{}: {line}", n + 1));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "invoke at module top level (call it from a component or a function):\n- {}",
            problems.join("\n- ")
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

    #[test]
    fn the_compiled_acl_refuses_every_other_window_and_origin() {
        // The real context: this app's capabilities and permissions as the
        // build compiled them. The handler answers any command, so a request
        // that reaches it got past the ACL.
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

        // The main window, on the app's pages: its commands, not the print view's.
        assert_eq!(call(&main, "get_app_settings", local), reached);
        assert_eq!(call(&main, "creation_delete", local), reached);
        assert!(
            call(&main, "print_job", local).is_err(),
            "print_job is the print view's"
        );
        assert!(
            call(&main, "plugin:fs|read_file", local).is_err(),
            "the webview reads no files through the fs plugin"
        );

        // Any remote page, even in the main window: nothing.
        for url in ["https://example.com/", "http://localhost.evil.example/"] {
            assert!(call(&main, "get_app_settings", url).is_err(), "{url}");
            assert!(call(&main, "agent_approve", url).is_err(), "{url}");
            assert!(call(&main, "plugin:event|listen", url).is_err(), "{url}");
        }

        // A print window: its job and figure reads only.
        assert_eq!(call(&print, "print_job", local), reached);
        assert_eq!(call(&print, "read_source_bytes", local), reached);
        for cmd in [
            "get_app_settings",
            "creation_delete",
            "agent_approve",
            "update_app_preferences",
            "plugin:fs|write_file",
            "plugin:dialog|save",
        ] {
            assert!(call(&print, cmd, local).is_err(), "print window: {cmd}");
        }

        // A window no capability names, e.g. a preview of its own label:
        // nothing, from the preview origin or the app's.
        for url in [preview.as_str(), local] {
            for cmd in [
                "creation_delete",
                "agent_approve",
                "get_app_settings",
                "plugin:event|listen",
            ] {
                assert!(call(&other, cmd, url).is_err(), "{cmd} from {url}");
            }
        }

        // The Artifacts preview frame lives in the main window, and Tauri 2.9
        // counts every registered custom scheme as a local origin, so the ACL
        // grants it what it grants the main window; the invoke key is the
        // barrier there (creations/preview.rs). If this starts failing, Tauri
        // tells the frame apart now: deny it here and update preview.rs.
        assert_eq!(
            call(&main, "get_app_settings", &preview),
            reached,
            "the ACL no longer treats the preview origin as the app's own"
        );
    }
}
