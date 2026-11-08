//! TypeScript path aliases: `compilerOptions.baseUrl` and `paths` of a
//! `tsconfig.json` (or `jsconfig.json`), following `extends` to other configs
//! of the project.
//!
//! tsconfig files are JSON with comments and trailing commas (JSONC), which a
//! JSON parser refuses; [`strip_jsonc`] turns them into JSON first.

use std::collections::HashMap;

use serde_json::Value;

/// `extends` chains followed at most.
const MAX_EXTENDS: usize = 8;

/// The aliases one tsconfig gives the files under its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aliases {
    /// The folder bare specifiers resolve from (`baseUrl`), relative to the
    /// project root with `/`; `None` without a `baseUrl`.
    pub base_url: Option<String>,
    /// `paths` patterns, each with its targets already relative to the
    /// project root (`@/*` → `app/src/*`), longest prefix first.
    pub paths: Vec<(String, Vec<String>)>,
}

/// JSON with comments (`//`, `/* */`) and trailing commas removed. Strings
/// are kept exactly, so a `//` inside one is not taken for a comment.
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                // A comma before a closing bracket (after blanks and comments
                // already removed or to come) is dropped.
                let mut j = i + 1;
                loop {
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'/') {
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                        continue;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'*') {
                        j += 2;
                        while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                            j += 1;
                        }
                        j += 2;
                        continue;
                    }
                    break;
                }
                if !matches!(chars.get(j), Some('}') | Some(']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The folder of a project-relative path (`""` for the root).
pub fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

/// `dir` joined with `relative`, `.` and `..` taken back; `None` when it
/// would leave the project.
pub fn join_normal(dir: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// One config read: its own options, relative to its own folder.
#[derive(Debug, Clone, Default)]
struct Raw {
    extends: Vec<String>,
    base_url: Option<String>,
    paths: Vec<(String, Vec<String>)>,
}

fn read_raw(text: &str) -> Raw {
    let Ok(value) = serde_json::from_str::<Value>(&strip_jsonc(text)) else {
        return Raw::default();
    };
    let extends = match value.get("extends") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    let options = value.get("compilerOptions");
    let base_url = options
        .and_then(|o| o.get("baseUrl"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let paths = options
        .and_then(|o| o.get("paths"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(pattern, targets)| {
                    let targets = targets
                        .as_array()
                        .map(|list| {
                            list.iter()
                                .filter_map(|t| t.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    (pattern.clone(), targets)
                })
                .collect()
        })
        .unwrap_or_default();
    Raw {
        extends,
        base_url,
        paths,
    }
}

/// The config an `extends` of the config at `from` (project-relative) names,
/// when it is a file of the project (`./base`, `../tsconfig.base.json`).
/// Package configs (`@tsconfig/node18`) are not in the project.
fn extended_path(from: &str, spec: &str) -> Option<String> {
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return None;
    }
    let mut path = join_normal(parent_dir(from), spec)?;
    if !path.ends_with(".json") {
        path.push_str(".json");
    }
    Some(path)
}

/// The aliases of the config at `path`, given every config of the project by
/// path. `baseUrl` and `paths` come from the nearest config of the `extends`
/// chain that sets them, relative to that config's folder (`paths` without a
/// `baseUrl` are relative to their own config).
pub fn aliases(path: &str, configs: &HashMap<String, String>) -> Aliases {
    let mut base_url: Option<String> = None;
    let mut paths: Option<Vec<(String, Vec<String>)>> = None;
    let mut chain = vec![path.to_string()];
    let mut seen = Vec::new();
    while let Some(current) = chain.pop() {
        if seen.contains(&current) || seen.len() >= MAX_EXTENDS {
            continue;
        }
        seen.push(current.clone());
        let Some(text) = configs.get(&current) else {
            continue;
        };
        let raw = read_raw(text);
        let dir = parent_dir(&current);
        let own_base = raw.base_url.as_deref().and_then(|b| join_normal(dir, b));
        if base_url.is_none() {
            base_url = own_base.clone();
        }
        if paths.is_none() && !raw.paths.is_empty() {
            let from = own_base
                .clone()
                .or_else(|| base_url.clone())
                .unwrap_or_else(|| dir.to_string());
            paths = Some(
                raw.paths
                    .iter()
                    .map(|(pattern, targets)| {
                        let targets = targets
                            .iter()
                            .filter_map(|t| join_normal(&from, t))
                            .collect();
                        (pattern.clone(), targets)
                    })
                    .collect(),
            );
        }
        for spec in raw.extends.iter().rev() {
            if let Some(next) = extended_path(&current, spec) {
                chain.push(next);
            }
        }
    }
    let mut paths = paths.unwrap_or_default();
    // The most specific pattern first: `@/components/*` before `@/*`.
    paths.sort_by(|a, b| {
        let prefix = |p: &str| p.split('*').next().unwrap_or("").len();
        prefix(&b.0).cmp(&prefix(&a.0)).then_with(|| a.0.cmp(&b.0))
    });
    Aliases { base_url, paths }
}

impl Aliases {
    /// The project-relative paths a bare specifier may stand for, in the
    /// order TypeScript tries them.
    pub fn candidates(&self, spec: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (pattern, targets) in &self.paths {
            let matched = match pattern.split_once('*') {
                Some((prefix, suffix)) => spec
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .map(str::to_string),
                None => (spec == pattern).then(String::new),
            };
            if let Some(star) = matched {
                for target in targets {
                    out.push(target.replace('*', &star));
                }
            }
        }
        if let Some(base) = &self.base_url {
            if let Some(path) = join_normal(base, spec) {
                out.push(path);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas_are_removed_but_strings_kept() {
        let text =
            "{\n  // a comment\n  \"a\": \"http://x/*y*/\", /* block */\n  \"b\": [1, 2,],\n}\n";
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value["a"], "http://x/*y*/");
        assert_eq!(value["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn paths_follow_extends_and_resolve_from_their_own_config() {
        let mut configs = HashMap::new();
        configs.insert(
            "tsconfig.base.json".to_string(),
            "{ \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@lib/*\": [\"libs/*\"], } } }".to_string(),
        );
        configs.insert(
            "app/tsconfig.json".to_string(),
            "{\n /* app */ \"extends\": \"../tsconfig.base\",\n \"compilerOptions\": {}\n}"
                .to_string(),
        );
        let aliases = aliases("app/tsconfig.json", &configs);
        assert_eq!(aliases.base_url.as_deref(), Some(""));
        assert_eq!(aliases.candidates("@lib/x"), vec!["libs/x", "@lib/x"]);
    }

    #[test]
    fn the_most_specific_pattern_is_tried_first() {
        let mut configs = HashMap::new();
        configs.insert(
            "app/tsconfig.json".to_string(),
            "{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"@/*\":[\"./src/*\"],\"@/ui/*\":[\"./src/components/ui/*\"]}}}".to_string(),
        );
        let aliases = aliases("app/tsconfig.json", &configs);
        assert_eq!(
            aliases.candidates("@/ui/button"),
            vec![
                "app/src/components/ui/button",
                "app/src/ui/button",
                "app/@/ui/button"
            ]
        );
        assert!(
            join_normal("a", "../../x").is_none(),
            "never above the project"
        );
    }
}
