//! Agent coverage gate: the assistant must be able to do what the UI can,
//! or the gap must be a recorded decision.
//!
//! `agent-coverage.json` maps every Tauri command the frontend uses to the
//! agent tool(s) that give the assistant the same ability, or excludes it
//! with a reason. The test below fails when the frontend starts using a
//! command that has neither, when the manifest names a tool that is not
//! registered (and allowed), or when it lists a command that does not exist.
//!
//! Finding the commands the frontend uses: every string literal in
//! `app/src` (ts, tsx, js, jsx) that equals a registered command name counts
//! as a use, so wrappers around `invoke` are covered too. `invoke` calls
//! whose command is not a string literal are only allowed in files the
//! manifest declares (with a reason), so a command name cannot be assembled
//! at runtime to slip past the check.

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use regex::Regex;
    use serde_json::Value;

    use crate::agent_tools::{build_registry, testing};
    use shodh_rag::harness::profile::AgentProfile;

    fn manifest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Command names in `generate_handler![...]` of lib.rs.
    pub(crate) fn registered_commands() -> BTreeSet<String> {
        let lib = read(&manifest_dir().join("src").join("lib.rs"));
        let start = lib
            .find("generate_handler![")
            .expect("generate_handler! in lib.rs");
        let body = &lib[start..];
        let end = body.find("])").expect("end of generate_handler!");
        let path =
            Regex::new(r"(?m)^\s*[A-Za-z_][A-Za-z0-9_]*::([A-Za-z_][A-Za-z0-9_]*)\s*,").unwrap();
        path.captures_iter(&body[..end])
            .map(|c| c[1].to_string())
            .collect()
    }

    fn frontend_files() -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "node_modules") {
                        continue;
                    }
                    walk(&path, out);
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"))
                {
                    out.push(path);
                }
            }
        }
        let root = manifest_dir().join("..").join("src");
        let mut out = Vec::new();
        walk(&root, &mut out);
        assert!(
            !out.is_empty(),
            "no frontend sources under {}",
            root.display()
        );
        out
    }

    fn relative(path: &Path) -> String {
        let root = manifest_dir().join("..").join("src");
        path.strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    struct Scan {
        /// Command → files that mention it.
        used: BTreeMap<String, BTreeSet<String>>,
        /// `invoke` calls whose command is not a literal, as `file:line`.
        dynamic: Vec<(String, String)>,
        /// Literal `invoke('x')` commands that are not registered.
        unknown: BTreeSet<String>,
    }

    fn scan(commands: &BTreeSet<String>) -> Scan {
        let literal = Regex::new(r#"["'`]([A-Za-z_][A-Za-z0-9_]*)["'`]"#).unwrap();
        // `invoke(`, `invoke<T>(` and aliases created with `invoke as x`.
        let alias = Regex::new(r"\binvoke\s+as\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap();
        let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut dynamic = Vec::new();
        let mut unknown = BTreeSet::new();
        for file in frontend_files() {
            let text = read(&file);
            let name = relative(&file);
            for cap in literal.captures_iter(&text) {
                if commands.contains(&cap[1]) {
                    used.entry(cap[1].to_string())
                        .or_default()
                        .insert(name.clone());
                }
            }
            let mut callees = vec!["invoke".to_string()];
            callees.extend(alias.captures_iter(&text).map(|c| c[1].to_string()));
            for callee in callees {
                let call = Regex::new(&format!(
                    r"(?:^|[^A-Za-z0-9_$.]){}\s*(?:<[^()]*?>)?\s*\(\s*(\S)",
                    regex::escape(&callee)
                ))
                .unwrap();
                let first_literal = Regex::new(&format!(
                    r#"(?:^|[^A-Za-z0-9_$.]){}\s*(?:<[^()]*?>)?\s*\(\s*["'`]([A-Za-z0-9_]+)["'`]"#,
                    regex::escape(&callee)
                ))
                .unwrap();
                for cap in first_literal.captures_iter(&text) {
                    if !commands.contains(&cap[1]) {
                        unknown.insert(format!("{} in {name}", &cap[1]));
                    }
                }
                for m in call.captures_iter(&text) {
                    let first = &m[1];
                    if first == "'" || first == "\"" || first == "`" {
                        continue;
                    }
                    let offset = m.get(1).map(|g| g.start()).unwrap_or(0);
                    let line = text[..offset].lines().count();
                    dynamic.push((name.clone(), format!("{name}:{line}")));
                }
            }
        }
        Scan {
            used,
            dynamic,
            unknown,
        }
    }

    fn manifest() -> Value {
        let path = manifest_dir().join("agent-coverage.json");
        serde_json::from_str(&read(&path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn mapped_tools(entry: &Value) -> Vec<String> {
        match (entry.get("tool"), entry.get("tools")) {
            (Some(Value::String(t)), _) => vec![t.clone()],
            (_, Some(Value::Array(ts))) => ts
                .iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    }

    #[tokio::test]
    async fn every_ui_command_is_covered_by_an_agent_tool_or_excluded_with_a_reason() {
        let commands = registered_commands();
        assert!(
            commands.len() > 50,
            "parsed only {} commands",
            commands.len()
        );
        let manifest = manifest();
        let entries = manifest["commands"].as_object().expect("commands object");
        let declared_dynamic: BTreeSet<&str> = manifest["dynamicInvokeFiles"]
            .as_object()
            .expect("dynamicInvokeFiles object")
            .iter()
            .inspect(|(file, why)| {
                assert!(
                    why.as_str().is_some_and(|w| w.trim().len() >= 20),
                    "dynamicInvokeFiles.{file} needs a reason"
                );
            })
            .map(|(file, _)| file.as_str())
            .collect();

        let t = testing::host().await;
        let registry = build_registry(t.host.clone()).expect("registry builds");
        let registered: BTreeSet<&str> = registry.names().into_iter().collect();
        let profile = AgentProfile::assistant();

        let scan = scan(&commands);
        let mut problems = Vec::new();

        for (file, at) in &scan.dynamic {
            if !declared_dynamic.contains(file.as_str()) {
                problems.push(format!(
                    "{at}: invoke is called with a non-literal command; use a string literal or declare the file in dynamicInvokeFiles with a reason"
                ));
            }
        }
        for unknown in &scan.unknown {
            problems.push(format!("invoke of an unregistered command: {unknown}"));
        }
        for (command, files) in &scan.used {
            if !entries.contains_key(command) {
                problems.push(format!(
                    "{command} (used in {}) has no entry in agent-coverage.json: map it to the agent tool that does the same, or exclude it with a reason",
                    files.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for (command, entry) in entries {
            if !commands.contains(command) {
                problems.push(format!(
                    "agent-coverage.json lists {command}, which is not a registered command"
                ));
            }
            let tools = mapped_tools(entry);
            let excluded = entry.get("excluded").and_then(Value::as_str);
            match (tools.is_empty(), excluded) {
                (true, Some(reason)) if reason.trim().len() >= 20 => {}
                (true, Some(_)) => problems.push(format!(
                    "{command}: the exclusion reason is too short to explain anything"
                )),
                (true, None) => problems.push(format!(
                    "{command}: map it to a tool or exclude it with a reason"
                )),
                (false, Some(_)) => {
                    problems.push(format!("{command}: either map it or exclude it, not both"))
                }
                (false, None) => {
                    for tool in tools {
                        if !registered.contains(tool.as_str()) {
                            problems.push(format!(
                                "{command} maps to {tool}, which is not a registered agent tool"
                            ));
                        } else if !profile.allows(&tool) {
                            problems.push(format!("{command} maps to {tool}, which the assistant profile does not allow"));
                        }
                    }
                }
            }
        }
        assert!(
            problems.is_empty(),
            "agent coverage gaps:\n- {}",
            problems.join("\n- ")
        );
    }

    #[test]
    fn the_scanner_catches_wrappers_aliases_and_dynamic_names() {
        let commands: BTreeSet<String> = ["load_tasks", "secret_cmd"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let literal = Regex::new(r#"["'`]([A-Za-z_][A-Za-z0-9_]*)["'`]"#).unwrap();
        let wrapper = "const call = (c) => invoke(c); call('secret_cmd');";
        let found: Vec<String> = literal
            .captures_iter(wrapper)
            .map(|c| c[1].to_string())
            .filter(|c| commands.contains(c))
            .collect();
        assert_eq!(
            found,
            vec!["secret_cmd"],
            "a wrapper's literal counts as a use"
        );
        let dynamic =
            Regex::new(r"(?:^|[^A-Za-z0-9_$.])invoke\s*(?:<[^()]*?>)?\s*\(\s*(\S)").unwrap();
        let firsts: Vec<String> = dynamic
            .captures_iter("invoke<Foo>(name); invoke('load_tasks'); x.invoke(y);")
            .map(|c| c[1].to_string())
            .collect();
        assert_eq!(
            firsts,
            vec!["n", "'"],
            "member calls like x.invoke are not Tauri invoke"
        );
    }

    /// The strings of a `const NAME ... = [ ... ]` array in a TypeScript file.
    fn ts_array(text: &str, name: &str, file: &str) -> Vec<String> {
        let re = Regex::new(&format!(
            r"const {}\b[^=]*=\s*\[([^\]]*)\]",
            regex::escape(name)
        ))
        .unwrap();
        let body = re
            .captures(text)
            .unwrap_or_else(|| panic!("{name} in {file}"))
            .get(1)
            .unwrap()
            .as_str()
            .to_string();
        Regex::new(r"'([^']*)'")
            .unwrap()
            .captures_iter(&body)
            .map(|c| c[1].to_string())
            .collect()
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every frontend source, by its path under `app/src` (`/`-separated).
    fn frontend_sources() -> BTreeMap<String, String> {
        frontend_files()
            .iter()
            .map(|f| (relative(f), read(f)))
            .collect()
    }

    /// Every Rust source of the app (`app/src-tauri/src`), joined: where the per-tool
    /// navigation tests live.
    fn app_rust_sources() -> String {
        fn walk(dir: &Path, out: &mut String) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push_str(&read(&path));
                    out.push('\n');
                }
            }
        }
        let mut out = String::new();
        walk(&manifest_dir().join("src"), &mut out);
        out
    }

    /// The body of `const <name> = useCallback(` up to its dependency list.
    fn callback_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
        let start = text.find(&format!("const {name} = useCallback("))?;
        let rest = &text[start..];
        let end = rest.find("\n  }, [")?;
        Some(&rest[..end])
    }

    /// The frontend file (under `app/src`) whose view applies targets of `kind`; `None`
    /// for `conversation`, which the chat session opens itself. A new kind fails the
    /// test until it is given an owner here.
    fn kind_owner(kind: &str) -> Option<&'static str> {
        match kind {
            "conversation" => None,
            "document" => Some("features/ask/AskView.tsx"),
            "calendar" => Some("features/tasks/TasksStore.tsx"),
            "audit" => Some("features/audit/AuditView.tsx"),
            "source" => Some("App-SplitView.tsx"),
            "visual" => Some("features/visuals/VisualNavigator.tsx"),
            "workspace" => Some("features/workspaces/WorkspacesView.tsx"),
            "artifact" => Some("features/artifacts/ArtifactsView.tsx"),
            "code" | "review" | "design_check" => Some("features/code/CodeReviewContext.tsx"),
            "setting" => Some("components/shell/SettingsView.tsx"),
            "library" => Some("features/library/LibraryView.tsx"),
            "status" => Some("components/shell/SystemStatusStrip.tsx"),
            _ => Some("(no owner: add the kind to kind_owner)"),
        }
    }

    /// What the frontend lacks for the assistant's navigation to reach the screen:
    /// a parser and a view for every target kind (the view's own
    /// `useNavigationTarget('<kind>'`, never an unrelated `kind === ...`), a highlight
    /// for every setting the assistant may change, a handler for every Library
    /// section, the same lists on both sides, and every move going through the
    /// navigation gate. Pure over `sources`, so the checks are themselves tested by
    /// mutating the sources.
    fn frontend_navigation_problems(sources: &BTreeMap<String, String>) -> Vec<String> {
        use shodh_rag::harness::events::NavigationTarget;
        use shodh_rag::harness::tools::navigate::VIEWS;

        let file = |name: &str| sources.get(name).map(String::as_str).unwrap_or("");
        let mut problems = Vec::new();
        let parser = file("features/agent/navigation.ts");
        let session = file("features/ask/ChatSessionContext.tsx");
        for kind in NavigationTarget::KINDS {
            if !parser.contains(&format!("case '{kind}':")) {
                problems.push(format!(
                    "target {kind}: features/agent/navigation.ts does not parse it"
                ));
            }
            let applied = match kind_owner(kind) {
                // A conversation is opened by the session itself, never by a view.
                None => session.contains("nav.target?.kind === 'conversation'"),
                // The view that owns the kind handles it; another file's handler (the
                // snippet shelf's for `library`) or an unrelated `kind === ...` does not count.
                Some(owner) => file(owner).contains(&format!("useNavigationTarget('{kind}'")),
            };
            if !applied {
                problems.push(format!("target {kind}: no view applies it"));
            }
        }

        // Every setting the assistant may change and Settings shows is highlighted there.
        for key in crate::app_settings::AGENT_WRITABLE {
            if key.section().is_none() {
                continue;
            }
            let marker = format!("agentItemKey('setting', '{}')", key.as_str());
            if !sources.values().any(|text| text.contains(&marker)) {
                problems.push(format!(
                    "setting {}: no control is marked {marker}, so update_setting highlights nothing",
                    key.as_str()
                ));
            }
        }

        // Every Library section has a handler in the Library.
        let library = file("features/library/LibraryView.tsx");
        for section in crate::agent_tools::LIBRARY_SECTIONS {
            if !library.contains(&format!("target.section === '{section}'")) {
                problems.push(format!(
                    "Library section {section}: LibraryView does not open it"
                ));
            }
        }

        // Both sides validate targets against the same lists.
        let mut same = |what: &str, rust: &[&str], ts: Vec<String>| {
            let rust: Vec<String> = rust.iter().map(|s| s.to_string()).collect();
            if rust != ts {
                problems.push(format!("{what}: Rust has {rust:?}, the frontend {ts:?}"));
            }
        };
        let section_ids: Vec<String> = Regex::new(r"(?m)^\s*id: '([a-z]+)',\s*$")
            .unwrap()
            .captures_iter(file("components/shell/SettingsView.tsx"))
            .map(|c| c[1].to_string())
            .collect();
        same(
            "Settings sections (SettingsView)",
            &crate::agent_tools::SETTINGS_SECTIONS[..],
            section_ids,
        );
        same(
            "Settings sections (navigation.ts)",
            &crate::agent_tools::SETTINGS_SECTIONS[..],
            ts_array(parser, "SETTINGS_SECTIONS", "navigation.ts"),
        );
        same(
            "Library sections",
            &crate::agent_tools::LIBRARY_SECTIONS[..],
            ts_array(parser, "LIBRARY_SECTIONS", "navigation.ts"),
        );
        same(
            "Workspace tabs",
            &crate::agent_tools::WORKSPACE_TABS[..],
            ts_array(parser, "WORKSPACE_TABS", "navigation.ts"),
        );
        same(
            "Views",
            &VIEWS[..],
            ts_array(file("lib/viewTabs.ts"), "VIEW_TABS", "viewTabs.ts"),
        );

        // Every move reaches the screen through the gate: the view switch and the
        // target hand-off happen only inside followNavigation, which only the gate calls.
        match callback_body(session, "followNavigation") {
            None => problems.push("ChatSessionContext has no followNavigation".to_string()),
            Some(body) => {
                for marker in ["CustomEvent('switchTab'", "publishTarget("] {
                    let total = session.matches(marker).count();
                    let inside = body.matches(marker).count();
                    if inside == 0 || total != inside {
                        problems.push(format!(
                            "ChatSessionContext: {marker} must appear only inside followNavigation ({inside} of {total})"
                        ));
                    }
                }
                let follows = session.matches("followNavigation(").count();
                let gated: usize = ["deliver", "goToHeldNavigation"]
                    .iter()
                    .filter_map(|name| callback_body(session, name))
                    .map(|b| b.matches("followNavigation(").count())
                    .sum();
                if follows != gated {
                    problems.push(format!(
                        "ChatSessionContext: followNavigation is called {follows} times, {gated} of them through the gate"
                    ));
                }
            }
        }
        // The System status panel opens from its own target, through the same gate.
        if sources
            .values()
            .any(|text| text.contains("'show-system-status'"))
        {
            problems.push(
                "the System status panel still opens from its own event, outside the gate"
                    .to_string(),
            );
        }
        problems
    }

    /// Every view the assistant can open and every kind of item a `navigated`
    /// event can point at has a Read-tier tool that emits it (besides `open_view`,
    /// which opens every view), every Library section has one, every tool on the map
    /// has a test that reads its event with `testing::expect_navigation` (which fails
    /// for a tool not on the map), and the frontend handles each.
    #[tokio::test]
    async fn every_view_and_item_the_assistant_can_open_has_a_tool_and_a_handler() {
        use shodh_rag::harness::events::NavigationTarget;
        use shodh_rag::harness::tools::navigate::VIEWS;
        use shodh_rag::harness::tools::HostTool;
        use shodh_rag::harness::RiskTier;

        let t = testing::host().await;
        let registry = build_registry(t.host.clone()).expect("registry builds");
        // A Code session adds its own tools (review_code) to these.
        let code_tools = crate::agent_tools::code_review_tools(
            t.host.effects.clone(),
            t.host.workspaces.clone(),
            t.host.data_dir.clone(),
            None,
        );
        let tier_of = |name: &str| -> Option<RiskTier> {
            registry.tool(name).map(|tool| tool.tier()).or_else(|| {
                code_tools
                    .iter()
                    .find(|tool| tool.name() == name)
                    .map(|tool| tool.tier())
            })
        };
        let navigation = &manifest()["navigation"];
        let tested: BTreeSet<String> = Regex::new(
            r#"(?:expect_navigation|next_calendar_target|next_setting_target)\(\s*&mut \w+,\s*"([a-z_]+)""#,
        )
        .unwrap()
        .captures_iter(&app_rust_sources())
        .map(|c| c[1].to_string())
        .collect();
        let mut problems = Vec::new();

        let mut check = |what: &str, entry: Option<&Value>, ignore_open_view: bool| {
            let tools = entry.map(strings).unwrap_or_default();
            let mut read = false;
            for tool in &tools {
                match tier_of(tool) {
                    None => problems.push(format!("{what}: {tool} is not a registered tool")),
                    Some(tier) => {
                        read |= tier == RiskTier::Read && !(ignore_open_view && tool == "open_view")
                    }
                }
                if !tested.contains(tool) {
                    problems.push(format!(
                        "{what}: {tool} is on the map but no test reads its event with expect_navigation(&mut rx, \"{tool}\")"
                    ));
                }
            }
            if !read {
                problems.push(format!(
                    "{what}: no Read-tier tool opens it{}",
                    if ignore_open_view {
                        " (besides open_view)"
                    } else {
                        ""
                    }
                ));
            }
        };

        let views = navigation["views"].as_object().expect("navigation.views");
        for view in VIEWS {
            check(&format!("view {view}"), views.get(view), true);
        }
        let targets = navigation["targets"]
            .as_object()
            .expect("navigation.targets");
        for kind in NavigationTarget::KINDS {
            check(&format!("target {kind}"), targets.get(kind), false);
        }
        let sections = navigation["librarySections"]
            .as_object()
            .expect("navigation.librarySections");
        for section in crate::agent_tools::LIBRARY_SECTIONS {
            check(
                &format!("Library section {section}"),
                sections.get(section),
                false,
            );
        }
        for view in views.keys() {
            if !VIEWS.contains(&view.as_str()) {
                problems.push(format!(
                    "navigation.views lists {view}, which is not a view"
                ));
            }
        }
        for kind in targets.keys() {
            if !NavigationTarget::KINDS.contains(&kind.as_str()) {
                problems.push(format!(
                    "navigation.targets lists {kind}, which is not a target kind"
                ));
            }
        }

        // Opening and showing never changes data: every open_ and show_ tool is Read tier.
        for name in registry.names() {
            if name.starts_with("open_") || name.starts_with("show_") {
                if let Some((_, _, tier)) = registry.describe(name) {
                    if tier != RiskTier::Read {
                        problems.push(format!("{name} only shows, but is {tier:?} tier"));
                    }
                }
            }
        }

        problems.extend(frontend_navigation_problems(&frontend_sources()));
        assert!(
            problems.is_empty(),
            "navigation gaps:\n- {}",
            problems.join("\n- ")
        );
    }

    /// The frontend checks fail when what they check is gone: a view's handler, a
    /// setting's highlight, a Library section's handler, or the gate.
    #[test]
    fn the_frontend_navigation_checks_catch_a_missing_handler() {
        let real = frontend_sources();
        let now = frontend_navigation_problems(&real);
        assert!(now.is_empty(), "{now:?}");
        let mutated = |file: &str, from: &str, to: &str| {
            let mut sources = real.clone();
            let text = sources.get_mut(file).unwrap_or_else(|| panic!("{file}"));
            assert!(text.contains(from), "{file} has no {from}");
            *text = text.replace(from, to);
            frontend_navigation_problems(&sources)
        };
        let names = |problems: Vec<String>, needle: &str| {
            assert!(
                problems.iter().any(|p| p.contains(needle)),
                "no problem names {needle}: {problems:?}"
            );
        };
        // The Library's handler deleted: an unrelated `kind === 'library'` elsewhere
        // does not count.
        names(
            mutated(
                "features/library/LibraryView.tsx",
                "useNavigationTarget('library'",
                "useNavigationTarget_removed('library'",
            ),
            "target library",
        );
        names(
            mutated(
                "components/SearchSettings.tsx",
                "agentItemKey('setting', 'search_max_results')",
                "agentItemKey('setting', 'other')",
            ),
            "setting search_max_results",
        );
        names(
            mutated(
                "features/library/LibraryView.tsx",
                "target.section === 'graph'",
                "target.section === 'graph_removed'",
            ),
            "Library section graph",
        );
        // A view switch outside the gate.
        names(
            mutated(
                "features/ask/ChatSessionContext.tsx",
                "const onEnvelope = useCallback(",
                "window.dispatchEvent(new CustomEvent('switchTab', { detail: 'ask' }));\n  const onEnvelope = useCallback(",
            ),
            "CustomEvent('switchTab'",
        );
        names(
            mutated(
                "features/ask/ChatSessionContext.tsx",
                "nav.target?.kind === 'conversation'",
                "nav.target?.kind === 'chat'",
            ),
            "target conversation",
        );
    }

    /// The tools of `navigate.rs` and GitHub's status, as the app registers them, send
    /// the events the map lists.
    #[tokio::test]
    async fn the_crates_navigation_tools_send_what_the_map_lists() {
        use shodh_rag::harness::profile::AgentProfile;
        use shodh_rag::harness::tools::{ApprovalGate, ToolCall};
        let t = testing::host().await;
        let registry = build_registry(t.host.clone()).expect("registry builds");
        let gate = ApprovalGate::default();
        let (ctx, mut rx) = testing::ctx();
        let call = |tool: &str, args: Value| ToolCall {
            tool: tool.to_string(),
            args,
            call_index: 1,
        };
        let profile = AgentProfile::assistant();
        let doc = if cfg!(windows) {
            "C:/docs/acme.pdf"
        } else {
            "/docs/acme.pdf"
        };
        for (tool, args) in [
            ("open_view", serde_json::json!({ "view": "library" })),
            (
                "show_document",
                serde_json::json!({ "path": doc, "page": 4 }),
            ),
            ("show_audit", serde_json::json!({ "types": ["tool_call"] })),
        ] {
            let out = registry
                .dispatch(call(tool, args), &profile, &gate, &ctx)
                .await;
            assert!(out.ok, "{tool}: {}", out.text_for_model);
        }
        assert_eq!(
            testing::expect_navigation(&mut rx, "open_view").view,
            "library"
        );
        let doc_shown = testing::expect_navigation(&mut rx, "show_document");
        assert_eq!(doc_shown.label.as_deref(), Some("acme.pdf, page 4"));
        assert_eq!(
            testing::expect_navigation(&mut rx, "show_audit").view,
            "activity"
        );
        // GitHub's status opens the GitHub page even before a sign-in.
        let _ = registry
            .dispatch(
                call("github_status", serde_json::json!({})),
                &profile,
                &gate,
                &ctx,
            )
            .await;
        let github = testing::expect_navigation(&mut rx, "github_status");
        assert_eq!(
            (github.view.as_str(), github.focus.as_deref()),
            ("connectors", Some("github"))
        );
    }
}
