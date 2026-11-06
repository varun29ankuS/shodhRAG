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

    fn src_file(relative: &str) -> String {
        read(&manifest_dir().join("..").join("src").join(relative))
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

    /// Every view the assistant can open and every kind of item a `navigated`
    /// event can point at has a Read-tier tool that emits it, and the frontend
    /// handles each: it parses the kind, a view applies it, and the lists both
    /// sides validate against (Settings sections, Library parts, workspace tabs,
    /// views) agree. That each listed tool really emits its target is asserted
    /// by the tool's own tests.
    #[tokio::test]
    async fn every_view_and_item_the_assistant_can_open_has_a_tool_and_a_handler() {
        use shodh_rag::harness::events::NavigationTarget;
        use shodh_rag::harness::tools::navigate::VIEWS;
        use shodh_rag::harness::RiskTier;

        use shodh_rag::harness::tools::HostTool;

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
        let mut problems = Vec::new();

        let check_tools = |what: &str, entry: Option<&Value>, problems: &mut Vec<String>| {
            let tools = entry.map(strings).unwrap_or_default();
            if tools.is_empty() {
                problems.push(format!("{what}: no tool opens it"));
                return;
            }
            let mut read = false;
            for tool in &tools {
                match tier_of(tool) {
                    None => problems.push(format!("{what}: {tool} is not a registered tool")),
                    Some(tier) => read |= tier == RiskTier::Read,
                }
            }
            if !read {
                problems.push(format!(
                    "{what}: only writes show it; give the assistant a Read-tier tool that opens it"
                ));
            }
        };

        let views = navigation["views"].as_object().expect("navigation.views");
        for view in VIEWS {
            check_tools(&format!("view {view}"), views.get(view), &mut problems);
        }
        for view in views.keys() {
            if !VIEWS.contains(&view.as_str()) {
                problems.push(format!(
                    "navigation.views lists {view}, which is not a view"
                ));
            }
        }

        let targets = navigation["targets"]
            .as_object()
            .expect("navigation.targets");
        let parser = src_file("features/agent/navigation.ts");
        let frontend: Vec<String> = frontend_files().iter().map(|f| read(f)).collect();
        for kind in NavigationTarget::KINDS {
            check_tools(&format!("target {kind}"), targets.get(kind), &mut problems);
            if !parser.contains(&format!("case '{kind}':")) {
                problems.push(format!(
                    "target {kind}: features/agent/navigation.ts does not parse it"
                ));
            }
            let applied = [
                format!("useNavigationTarget('{kind}'"),
                format!("kind === '{kind}'"),
            ];
            if !frontend
                .iter()
                .any(|text| applied.iter().any(|a| text.contains(a.as_str())))
            {
                problems.push(format!("target {kind}: no view applies it"));
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

        // Both sides validate targets against the same lists.
        let same = |what: &str, rust: &[&str], ts: Vec<String>, problems: &mut Vec<String>| {
            let rust: Vec<String> = rust.iter().map(|s| s.to_string()).collect();
            if rust != ts {
                problems.push(format!("{what}: Rust has {rust:?}, the frontend {ts:?}"));
            }
        };
        let settings_view = src_file("components/shell/SettingsView.tsx");
        let section_ids: Vec<String> = Regex::new(r"(?m)^\s*id: '([a-z]+)',\s*$")
            .unwrap()
            .captures_iter(&settings_view)
            .map(|c| c[1].to_string())
            .collect();
        same(
            "Settings sections (SettingsView)",
            &crate::agent_tools::SETTINGS_SECTIONS[..],
            section_ids,
            &mut problems,
        );
        same(
            "Settings sections (navigation.ts)",
            &crate::agent_tools::SETTINGS_SECTIONS[..],
            ts_array(&parser, "SETTINGS_SECTIONS", "navigation.ts"),
            &mut problems,
        );
        same(
            "Library sections",
            &crate::agent_tools::LIBRARY_SECTIONS[..],
            ts_array(&parser, "LIBRARY_SECTIONS", "navigation.ts"),
            &mut problems,
        );
        same(
            "Workspace tabs",
            &crate::agent_tools::WORKSPACE_TABS[..],
            ts_array(&parser, "WORKSPACE_TABS", "navigation.ts"),
            &mut problems,
        );
        same(
            "Views",
            &VIEWS[..],
            ts_array(&src_file("lib/viewTabs.ts"), "VIEW_TABS", "viewTabs.ts"),
            &mut problems,
        );

        assert!(
            problems.is_empty(),
            "navigation gaps:\n- {}",
            problems.join("\n- ")
        );
    }
}
