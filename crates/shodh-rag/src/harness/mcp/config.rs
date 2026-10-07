//! `mcp.json`: MCP servers in the format Cursor and Claude Desktop use,
//! `{"mcpServers": {name: {command, args, env, cwd} | {url, headers}}}`
//! (VS Code's top-level `servers` is read too).
//!
//! Shodh's own settings for a server live in the same entry, so pasting a
//! config from another app needs nothing extra and "Edit as file" shows
//! everything:
//! - `disabled: true` turns the server off (the key other clients use);
//! - `shodh.modes`: `["research"]`, `["code"]` or both (the default);
//! - `shodh.approval`: the default for the server's tools, `ask` or `auto`;
//! - `shodh.tools.<tool>`: `{ "enabled": false }` and/or `{ "approval": ... }`.
//!
//! A config is kept as JSON and edited in place, so keys Shodh does not know
//! survive every change. Environment values and headers are secrets: they are
//! redacted from `Debug` and never logged.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Replaced by the workspace's code folder in `cwd`, `args` and `env`.
pub const WORKSPACE_FOLDER_VAR: &str = "${workspaceFolder}";

/// Most servers one config may hold.
pub const MAX_SERVERS: usize = 64;

/// Longest server name.
pub const MAX_NAME_CHARS: usize = 48;

/// Where a server is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Workspace,
}

/// The answer modes a server's tools are offered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Research,
    Code,
}

/// Whether a tool call waits for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Approval {
    Ask,
    Auto,
}

/// How Shodh reaches a server. Values of `env` and `headers` are secrets.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
            } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", &env.keys().collect::<Vec<_>>())
                .field("cwd", cwd)
                .finish(),
            Transport::Http { url, headers } => f
                .debug_struct("Http")
                .field("url", url)
                .field("headers", &headers.keys().collect::<Vec<_>>())
                .finish(),
        }
    }
}

impl Transport {
    /// Whether the server's process works in the workspace's code folder (or
    /// is given it), so it cannot run without one.
    pub fn needs_workspace_folder(&self) -> bool {
        match self {
            Transport::Stdio { args, env, cwd, .. } => cwd
                .iter()
                .chain(args.iter())
                .chain(env.values())
                .any(|v| v.contains(WORKSPACE_FOLDER_VAR)),
            Transport::Http { .. } => false,
        }
    }

    /// The transport with [`WORKSPACE_FOLDER_VAR`] replaced by `folder`.
    pub fn resolved(&self, folder: Option<&str>) -> Result<Transport, String> {
        let Transport::Stdio {
            command,
            args,
            env,
            cwd,
        } = self
        else {
            return Ok(self.clone());
        };
        if self.needs_workspace_folder() && folder.is_none() {
            return Err("it works in the workspace's code folder, and this chat has none".into());
        }
        let fill = |value: &str| match folder {
            Some(folder) => value.replace(WORKSPACE_FOLDER_VAR, folder),
            None => value.to_string(),
        };
        Ok(Transport::Stdio {
            command: fill(command),
            args: args.iter().map(|a| fill(a)).collect(),
            env: env.iter().map(|(k, v)| (k.clone(), fill(v))).collect(),
            cwd: cwd.as_deref().map(fill),
        })
    }

    /// What the settings page shows: the command line or URL, never secrets.
    pub fn summary(&self) -> String {
        match self {
            Transport::Stdio { command, args, .. } => std::iter::once(command.as_str())
                .chain(args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            Transport::Http { url, .. } => url.clone(),
        }
    }
}

/// One tool's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSetting {
    /// `None` means enabled.
    pub enabled: Option<bool>,
    /// `None` means the server's default, else the tool's own annotations.
    pub approval: Option<Approval>,
}

/// One configured server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub name: String,
    pub transport: Transport,
    pub disabled: bool,
    /// Empty means every mode.
    pub modes: Vec<Mode>,
    pub approval: Option<Approval>,
    pub tools: BTreeMap<String, ToolSetting>,
}

impl ServerConfig {
    /// Whether the server's tools are offered in `mode`.
    pub fn in_mode(&self, mode: Mode) -> bool {
        self.modes.is_empty() || self.modes.contains(&mode)
    }

    /// Whether the server's tool `tool` is offered.
    pub fn tool_enabled(&self, tool: &str) -> bool {
        self.tools.get(tool).and_then(|s| s.enabled).unwrap_or(true)
    }

    /// Whether calls of `tool` wait for the user: the tool's setting, else
    /// the server's, else `auto` only for tools the server marks read-only.
    pub fn approval_for(&self, tool: &str, read_only_hint: bool) -> Approval {
        self.tools
            .get(tool)
            .and_then(|s| s.approval)
            .or(self.approval)
            .unwrap_or(if read_only_hint {
                Approval::Auto
            } else {
                Approval::Ask
            })
    }
}

/// A server entry that could not be read (the rest of the config still is).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerProblem {
    pub name: String,
    pub error: String,
}

/// Why a whole config could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("this is not valid JSON ({0})")]
    Json(String),
    #[error("expected an object with \"mcpServers\"")]
    Shape,
    #[error("{0}")]
    Invalid(String),
}

/// A parsed config: its servers and the entries that were not understood.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfig {
    pub servers: Vec<ServerConfig>,
    pub problems: Vec<ServerProblem>,
}

/// Whether `name` is a usable server name: 1 to [`MAX_NAME_CHARS`] of
/// letters, digits, `-`, `_` and `.`.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn string_map(value: Option<&Value>, what: &str) -> Result<BTreeMap<String, String>, String> {
    match value {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k.clone(), s.clone())),
                Value::Number(n) => Ok((k.clone(), n.to_string())),
                Value::Bool(b) => Ok((k.clone(), b.to_string())),
                _ => Err(format!("{what} \"{k}\" must be text")),
            })
            .collect(),
        Some(_) => Err(format!("{what} must be an object")),
    }
}

fn parse_approval(value: &Value) -> Result<Option<Approval>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) if s == "ask" => Ok(Some(Approval::Ask)),
        Value::String(s) if s == "auto" => Ok(Some(Approval::Auto)),
        _ => Err("approval must be \"ask\" or \"auto\"".into()),
    }
}

/// Read one server entry.
pub fn parse_server(name: &str, entry: &Value) -> Result<ServerConfig, String> {
    if !is_valid_name(name) {
        return Err(format!(
            "a server name must be 1 to {MAX_NAME_CHARS} letters, digits, '-', '_' or '.'"
        ));
    }
    let Value::Object(entry) = entry else {
        return Err("a server must be an object".into());
    };
    let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
    let transport = match (entry.get("command"), entry.get("url")) {
        (Some(Value::String(command)), None) if !command.trim().is_empty() => {
            if !matches!(kind, "" | "stdio") {
                return Err(format!("type \"{kind}\" does not go with a command"));
            }
            let args = match entry.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|a| match a {
                        Value::String(s) => Ok(s.clone()),
                        Value::Number(n) => Ok(n.to_string()),
                        _ => Err("args must be text".to_string()),
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err("args must be a list".into()),
            };
            let cwd = match entry.get("cwd") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
                Some(_) => return Err("cwd must be a folder path".into()),
            };
            Transport::Stdio {
                command: command.trim().to_string(),
                args,
                env: string_map(entry.get("env"), "env")?,
                cwd,
            }
        }
        (None, Some(Value::String(url))) => {
            if kind == "sse" {
                return Err("the older SSE transport is not supported; use the server's streamable HTTP URL".into());
            }
            let parsed = url::Url::parse(url.trim()).map_err(|e| format!("url: {e}"))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err("url must start with http:// or https://".into());
            }
            Transport::Http {
                url: url.trim().to_string(),
                headers: string_map(entry.get("headers"), "header")?,
            }
        }
        (Some(_), Some(_)) => return Err("a server has a command or a url, not both".into()),
        _ => return Err("a server needs a \"command\" or a \"url\"".into()),
    };
    let disabled = match entry.get("disabled") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("disabled must be true or false".into()),
    };
    let shodh = entry.get("shodh").cloned().unwrap_or(Value::Null);
    let modes = match shodh.get("modes") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|m| match m.as_str() {
                Some("research") => Ok(Mode::Research),
                Some("code") => Ok(Mode::Code),
                _ => Err("modes may be \"research\" and \"code\"".to_string()),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("modes must be a list".into()),
    };
    let approval = parse_approval(shodh.get("approval").unwrap_or(&Value::Null))?;
    let mut tools = BTreeMap::new();
    if let Some(Value::Object(map)) = shodh.get("tools") {
        for (tool, setting) in map {
            let enabled = match setting.get("enabled") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => return Err(format!("tools.{tool}.enabled must be true or false")),
            };
            let approval = parse_approval(setting.get("approval").unwrap_or(&Value::Null))
                .map_err(|e| format!("tools.{tool}: {e}"))?;
            tools.insert(tool.clone(), ToolSetting { enabled, approval });
        }
    }
    Ok(ServerConfig {
        name: name.to_string(),
        transport,
        disabled,
        modes,
        approval,
        tools,
    })
}

/// The server map of a config document (`mcpServers`, or VS Code's `servers`).
fn servers_of(doc: &Value) -> Result<Option<&Map<String, Value>>, ConfigError> {
    match doc {
        Value::Object(map) => match map.get("mcpServers").or_else(|| map.get("servers")) {
            None => Ok(None),
            Some(Value::Object(servers)) => Ok(Some(servers)),
            Some(_) => Err(ConfigError::Shape),
        },
        _ => Err(ConfigError::Shape),
    }
}

/// Read a config document. Bad entries are reported, not fatal.
pub fn parse_doc(doc: &Value) -> Result<McpConfig, ConfigError> {
    let mut config = McpConfig::default();
    let Some(servers) = servers_of(doc)? else {
        return Ok(config);
    };
    for (name, entry) in servers.iter().take(MAX_SERVERS) {
        match parse_server(name, entry) {
            Ok(server) => config.servers.push(server),
            Err(error) => config.problems.push(ServerProblem {
                name: name.clone(),
                error,
            }),
        }
    }
    if servers.len() > MAX_SERVERS {
        config.problems.push(ServerProblem {
            name: String::new(),
            error: format!("only the first {MAX_SERVERS} servers are used"),
        });
    }
    Ok(config)
}

/// Parse config text (empty text is an empty config).
pub fn parse_text(text: &str) -> Result<(Value, McpConfig), ConfigError> {
    if text.trim().is_empty() {
        let doc = empty_doc();
        return Ok((doc, McpConfig::default()));
    }
    let doc: Value = serde_json::from_str(text).map_err(|e| ConfigError::Json(e.to_string()))?;
    let config = parse_doc(&doc)?;
    Ok((doc, config))
}

/// `{"mcpServers": {}}`.
pub fn empty_doc() -> Value {
    json!({ "mcpServers": {} })
}

/// The servers of `global` and `workspace`, with their scope: a workspace
/// server replaces a global one of the same name.
pub fn merge(global: &McpConfig, workspace: Option<&McpConfig>) -> Vec<(Scope, ServerConfig)> {
    let mut merged: Vec<(Scope, ServerConfig)> = Vec::new();
    for server in &global.servers {
        let overridden = workspace.is_some_and(|w| w.servers.iter().any(|s| s.name == server.name));
        if !overridden {
            merged.push((Scope::Global, server.clone()));
        }
    }
    if let Some(workspace) = workspace {
        merged.extend(
            workspace
                .servers
                .iter()
                .map(|s| (Scope::Workspace, s.clone())),
        );
    }
    merged
}

/// The servers pasted by a user: a whole config, or `{name: entry, ...}`.
/// Every entry must be readable.
pub fn parse_pasted(text: &str) -> Result<Vec<(String, Value)>, ConfigError> {
    let doc: Value =
        serde_json::from_str(text.trim()).map_err(|e| ConfigError::Json(e.to_string()))?;
    let entries: Map<String, Value> = match servers_of(&doc)? {
        Some(servers) => servers.clone(),
        None => match doc {
            Value::Object(map) if !map.is_empty() => map,
            _ => return Err(ConfigError::Shape),
        },
    };
    if entries.is_empty() {
        return Err(ConfigError::Invalid("the config lists no servers".into()));
    }
    entries
        .into_iter()
        .map(|(name, entry)| {
            parse_server(&name, &entry)
                .map(|_| (name.clone(), entry))
                .map_err(|e| ConfigError::Invalid(format!("{name}: {e}")))
        })
        .collect()
}

fn servers_mut(doc: &mut Value) -> Result<&mut Map<String, Value>, ConfigError> {
    if !doc.is_object() {
        *doc = empty_doc();
    }
    let root = doc.as_object_mut().ok_or(ConfigError::Shape)?;
    let key = if root.contains_key("servers") && !root.contains_key("mcpServers") {
        "servers"
    } else {
        "mcpServers"
    };
    let servers = root.entry(key).or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers.as_object_mut().ok_or(ConfigError::Shape)
}

/// `map[key]` as an object, made one if it is missing or something else.
fn object_at<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, ConfigError> {
    let value = map.entry(key).or_insert_with(|| json!({}));
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().ok_or(ConfigError::Shape)
}

/// Add (or replace) servers in `doc`. Fails before changing anything if the
/// config would hold more than [`MAX_SERVERS`].
pub fn add_servers(doc: &mut Value, entries: Vec<(String, Value)>) -> Result<(), ConfigError> {
    let servers = servers_mut(doc)?;
    let new = entries
        .iter()
        .filter(|(name, _)| !servers.contains_key(name))
        .count();
    if servers.len() + new > MAX_SERVERS {
        return Err(ConfigError::Invalid(format!(
            "a config holds at most {MAX_SERVERS} servers"
        )));
    }
    for (name, entry) in entries {
        servers.insert(name, entry);
    }
    Ok(())
}

/// Remove a server; returns whether it was there.
pub fn remove_server(doc: &mut Value, name: &str) -> Result<bool, ConfigError> {
    Ok(servers_mut(doc)?.remove(name).is_some())
}

fn entry_mut<'a>(
    doc: &'a mut Value,
    name: &str,
) -> Result<&'a mut Map<String, Value>, ConfigError> {
    match servers_mut(doc)?.get_mut(name) {
        Some(Value::Object(entry)) => Ok(entry),
        _ => Err(ConfigError::Invalid(format!("no server named {name}"))),
    }
}

/// Turn a server on or off.
pub fn set_server_enabled(doc: &mut Value, name: &str, enabled: bool) -> Result<(), ConfigError> {
    let entry = entry_mut(doc, name)?;
    if enabled {
        entry.remove("disabled");
    } else {
        entry.insert("disabled".into(), Value::Bool(true));
    }
    Ok(())
}

/// Set the modes a server is offered in (empty: every mode).
pub fn set_server_modes(doc: &mut Value, name: &str, modes: &[Mode]) -> Result<(), ConfigError> {
    let shodh = object_at(entry_mut(doc, name)?, "shodh")?;
    if modes.is_empty() || (modes.contains(&Mode::Research) && modes.contains(&Mode::Code)) {
        shodh.remove("modes");
    } else {
        shodh.insert("modes".into(), json!(modes));
    }
    Ok(())
}

/// Change one tool's settings; `None` leaves that setting alone.
pub fn set_tool(
    doc: &mut Value,
    server: &str,
    tool: &str,
    enabled: Option<bool>,
    approval: Option<Approval>,
) -> Result<(), ConfigError> {
    let tools = object_at(object_at(entry_mut(doc, server)?, "shodh")?, "tools")?;
    let setting = object_at(tools, tool)?;
    match enabled {
        Some(true) => {
            setting.remove("enabled");
        }
        Some(false) => {
            setting.insert("enabled".into(), Value::Bool(false));
        }
        None => {}
    }
    if let Some(approval) = approval {
        setting.insert("approval".into(), json!(approval));
    }
    if setting.is_empty() {
        tools.remove(tool);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURSOR: &str = r#"{
        "mcpServers": {
            "github": {
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_secret" }
            },
            "remote": {
                "url": "https://mcp.example.com/mcp",
                "headers": { "Authorization": "Bearer tok_secret" }
            },
            "broken": { "args": [] }
        }
    }"#;

    #[test]
    fn cursor_and_claude_desktop_configs_are_read() {
        let (_, config) = parse_text(CURSOR).unwrap();
        assert_eq!(config.servers.len(), 2);
        let github = &config.servers[0];
        assert_eq!(github.name, "github");
        assert_eq!(
            github.transport,
            Transport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                env: [(
                    "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                    "ghp_secret".to_string()
                )]
                .into(),
                cwd: None,
            }
        );
        assert!(!github.disabled);
        assert!(github.in_mode(Mode::Research) && github.in_mode(Mode::Code));
        let remote = &config.servers[1];
        assert!(
            matches!(&remote.transport, Transport::Http { url, .. } if url == "https://mcp.example.com/mcp")
        );
        // A bad entry is reported, the rest still load.
        assert_eq!(config.problems.len(), 1);
        assert_eq!(config.problems[0].name, "broken");
        // VS Code's `servers` key.
        let (_, vscode) = parse_text(r#"{"servers": {"x": {"command": "x"}}}"#).unwrap();
        assert_eq!(vscode.servers[0].name, "x");
        // Empty text is an empty config; other shapes are errors.
        assert_eq!(parse_text("  ").unwrap().1, McpConfig::default());
        assert!(matches!(parse_text("[1]"), Err(ConfigError::Shape)));
        assert!(matches!(parse_text("{"), Err(ConfigError::Json(_))));
    }

    #[test]
    fn secrets_never_appear_in_debug_output() {
        let (_, config) = parse_text(CURSOR).unwrap();
        let printed = format!("{config:?}");
        assert!(!printed.contains("ghp_secret"));
        assert!(!printed.contains("tok_secret"));
        assert!(printed.contains("GITHUB_PERSONAL_ACCESS_TOKEN"));
        assert_eq!(
            config.servers[0].transport.summary(),
            "npx -y @modelcontextprotocol/server-github"
        );
    }

    #[test]
    fn entries_are_validated() {
        for (entry, error) in [
            (json!({"command": "x", "url": "https://a"}), "not both"),
            (json!({"url": "ftp://a"}), "http"),
            (json!({"url": "https://a", "type": "sse"}), "SSE"),
            (json!({"command": "x", "args": "-y"}), "list"),
            (json!({"command": "x", "env": {"A": [1]}}), "text"),
            (
                json!({"command": "x", "shodh": {"modes": ["chat"]}}),
                "modes",
            ),
            (
                json!({"command": "x", "shodh": {"approval": "never"}}),
                "ask",
            ),
        ] {
            let message = parse_server("s", &entry).unwrap_err();
            assert!(message.contains(error), "{entry}: {message}");
        }
        assert!(parse_server("bad name", &json!({"command": "x"})).is_err());
        assert!(parse_server(&"n".repeat(49), &json!({"command": "x"})).is_err());
    }

    #[test]
    fn workspace_servers_replace_global_ones_of_the_same_name() {
        let (_, global) = parse_text(
            r#"{"mcpServers": {"a": {"command": "global-a"}, "b": {"command": "global-b"}}}"#,
        )
        .unwrap();
        let (_, workspace) = parse_text(
            r#"{"mcpServers": {"b": {"command": "ws-b", "disabled": true}, "c": {"url": "http://127.0.0.1:9/mcp"}}}"#,
        )
        .unwrap();
        let merged = merge(&global, Some(&workspace));
        let names: Vec<(Scope, &str)> = merged.iter().map(|(s, c)| (*s, c.name.as_str())).collect();
        assert_eq!(
            names,
            [
                (Scope::Global, "a"),
                (Scope::Workspace, "b"),
                (Scope::Workspace, "c")
            ]
        );
        assert!(merged[1].1.disabled);
        assert_eq!(merge(&global, None).len(), 2);
    }

    #[test]
    fn approval_follows_tool_then_server_then_annotations() {
        let server = parse_server(
            "s",
            &json!({"command": "x", "shodh": {"tools": {
                "write_file": {"approval": "auto"},
                "hidden": {"enabled": false}
            }}}),
        )
        .unwrap();
        assert_eq!(server.approval_for("read_file", true), Approval::Auto);
        assert_eq!(server.approval_for("delete", false), Approval::Ask);
        assert_eq!(server.approval_for("write_file", false), Approval::Auto);
        assert!(!server.tool_enabled("hidden"));
        assert!(server.tool_enabled("read_file"));
        let all_auto =
            parse_server("s", &json!({"command": "x", "shodh": {"approval": "auto"}})).unwrap();
        assert_eq!(all_auto.approval_for("anything", false), Approval::Auto);
    }

    #[test]
    fn workspace_folder_is_filled_in_or_required() {
        let server = parse_server(
            "enola",
            &json!({"command": "C:/tools/enola.exe", "cwd": WORKSPACE_FOLDER_VAR}),
        )
        .unwrap();
        assert!(server.transport.needs_workspace_folder());
        assert!(server.transport.resolved(None).is_err());
        match server.transport.resolved(Some("C:/code/app")).unwrap() {
            Transport::Stdio { cwd, .. } => assert_eq!(cwd.as_deref(), Some("C:/code/app")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn edits_keep_keys_shodh_does_not_know() {
        let mut doc: Value = serde_json::from_str(
            r#"{"mcpServers": {"a": {"command": "x", "autoApprove": ["t"]}}, "other": 1}"#,
        )
        .unwrap();
        set_server_enabled(&mut doc, "a", false).unwrap();
        set_server_modes(&mut doc, "a", &[Mode::Code]).unwrap();
        set_tool(&mut doc, "a", "t", Some(false), Some(Approval::Auto)).unwrap();
        assert_eq!(doc["other"], 1);
        assert_eq!(doc["mcpServers"]["a"]["autoApprove"], json!(["t"]));
        let server = parse_doc(&doc).unwrap().servers.remove(0);
        assert!(server.disabled);
        assert_eq!(server.modes, [Mode::Code]);
        assert!(!server.tool_enabled("t"));
        assert_eq!(server.approval_for("t", false), Approval::Auto);
        set_server_enabled(&mut doc, "a", true).unwrap();
        set_server_modes(&mut doc, "a", &[Mode::Research, Mode::Code]).unwrap();
        set_tool(&mut doc, "a", "t", Some(true), None).unwrap();
        let server = parse_doc(&doc).unwrap().servers.remove(0);
        assert!(!server.disabled && server.modes.is_empty() && server.tool_enabled("t"));
        assert!(set_tool(&mut doc, "missing", "t", Some(true), None).is_err());
        assert!(remove_server(&mut doc, "a").unwrap());
        assert!(!remove_server(&mut doc, "a").unwrap());
    }

    #[test]
    fn pasted_configs_may_be_whole_or_bare_entries() {
        let whole = parse_pasted(CURSOR);
        assert!(matches!(whole, Err(ConfigError::Invalid(m)) if m.starts_with("broken")));
        let bare =
            parse_pasted(r#"{"fs": {"command": "npx", "args": ["-y", "server-fs"]}}"#).unwrap();
        assert_eq!(bare[0].0, "fs");
        let mut doc = empty_doc();
        add_servers(&mut doc, bare).unwrap();
        assert_eq!(parse_doc(&doc).unwrap().servers[0].name, "fs");
        assert!(parse_pasted("{}").is_err());
        let many: Vec<(String, Value)> = (0..MAX_SERVERS)
            .map(|i| (format!("s{i}"), json!({"command": "x"})))
            .collect();
        assert!(add_servers(&mut doc, many).is_err(), "65 servers");
    }
}
