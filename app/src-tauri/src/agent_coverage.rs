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
mod tests {
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
    fn registered_commands() -> BTreeSet<String> {
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
}
