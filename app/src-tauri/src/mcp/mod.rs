//! MCP servers and skills for agent sessions.
//!
//! Configuration is `mcp.json` (the Cursor / Claude Desktop format, see
//! `shodh_rag::harness::mcp::config`): one global file in the app data
//! directory and one per workspace (`workspaces/<id>/mcp.json`); a workspace
//! server replaces a global one of the same name.
//!
//! Servers are started on demand and kept running in a pool shared by every
//! session (keyed by the exact command, environment and folder), so a chat
//! never waits for a server another chat already started. A server that
//! fails to start is reported, never fatal: the chat simply goes without its
//! tools. [`McpManager::chat_tools`] decides what one chat can use; the
//! agent session and the composer's tool chip both call it, so the chip
//! shows exactly what the session gets.

pub mod enola;
pub mod skills;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::Serialize;
use serde_json::Value;
use shodh_rag::harness::code_mode::exclude_locally;
use shodh_rag::harness::mcp::config::{self, ConfigError};
use shodh_rag::harness::mcp::{
    server_tools, Approval, CallResult, McpCaller, McpClient, McpConfig, McpError, Mode, Scope,
    ServerConfig, ToolInfo, Transport,
};
use shodh_rag::harness::protocol::ToolLoadMode;
use shodh_rag::harness::skills::{LoadSkillTool, ReadSkillFileTool, Skill};
use shodh_rag::harness::tools::HostTool;

/// Longest a server may take to start and list its tools when a chat needs it.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a server that failed to start is not tried again (except by
/// "Test connection", which always starts it afresh).
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

/// More tools than this in one chat make models pick worse; the composer warns.
pub const MANY_TOOLS: usize = 40;

/// File name of a scope's MCP configuration.
pub const CONFIG_FILE: &str = "mcp.json";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `id` can name a workspace folder on disk.
pub fn is_valid_workspace_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether a URL's host is this computer.
pub fn is_loopback_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| {
            host == "localhost"
                || host == "[::1]"
                || host == "::1"
                || host
                    .parse::<std::net::Ipv4Addr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
}

/// A running server and the tools it listed.
struct Live {
    client: McpClient,
    tools: Vec<ToolInfo>,
}

/// A pool entry: the server's scope and, once started, the server. The
/// lock serialises starting it.
struct Slot {
    scope: String,
    live: tokio::sync::Mutex<Option<Arc<Live>>>,
    /// The last failed start: a server that does not start is not tried
    /// again for [`RETRY_AFTER`], so chats are not held up by it each time.
    failed: Mutex<Option<(std::time::Instant, String)>>,
}

/// What the settings page knows about a server.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    /// `connected`, `error` or `unknown` (not started yet).
    pub state: &'static str,
    pub error: Option<String>,
    pub tools: Vec<ToolInfo>,
}

/// One server as a chat sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatServer {
    pub name: String,
    pub scope: Scope,
    /// Why the chat goes without this server, when it does.
    pub error: Option<String>,
    /// Host tool names the chat gets from it.
    pub tools: Vec<String>,
}

/// What one chat can use, for the composer chip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatToolsView {
    pub servers: Vec<ChatServer>,
    pub skills: Vec<SkillSummary>,
    /// Built-in tools of the mode.
    pub builtin: usize,
    pub total: usize,
    /// More than [`MANY_TOOLS`].
    pub many: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
}

/// The extra tools of one chat (MCP and skills) and what the chip shows.
pub struct ChatTools {
    pub tools: Vec<Arc<dyn HostTool>>,
    pub view: ChatToolsView,
    /// Changes whenever the tool set does (a session is restarted then).
    pub fingerprint: u64,
}

/// MCP servers: configuration files, the pool of running servers and their
/// last known status. Managed as Tauri state.
pub struct McpManager {
    data_dir: PathBuf,
    /// Running servers by [`connection_key`].
    pool: DashMap<u64, Arc<Slot>>,
    /// Status by `<scope key>/<server name>`.
    status: Mutex<HashMap<String, ServerStatus>>,
    /// One configuration change at a time.
    edits: tokio::sync::Mutex<()>,
    me: Weak<McpManager>,
}

/// Key of a running server: its scope, name and exact launch (in memory only).
fn connection_key(scope_key: &str, name: &str, transport: &Transport) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scope_key.hash(&mut hasher);
    name.hash(&mut hasher);
    transport.hash(&mut hasher);
    hasher.finish()
}

fn status_key(scope_key: &str, name: &str) -> String {
    format!("{scope_key}/{name}")
}

/// `global`, or the workspace id.
fn scope_key(scope: Scope, workspace: Option<&str>) -> String {
    match (scope, workspace) {
        (Scope::Workspace, Some(id)) => id.to_string(),
        _ => "global".to_string(),
    }
}

/// Calls a pooled server, restarting it if it stopped.
struct PooledCaller {
    manager: Weak<McpManager>,
    key: u64,
    status: String,
    transport: Transport,
}

#[async_trait]
impl McpCaller for PooledCaller {
    async fn call(&self, tool: &str, arguments: Value) -> Result<CallResult, McpError> {
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| McpError::Closed("the app is closing".into()))?;
        let live = manager
            .live(self.key, &self.status, &self.transport)
            .await
            .map_err(McpError::Closed)?;
        live.client.call_tool(tool, arguments).await
    }
}

impl McpManager {
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            data_dir,
            pool: DashMap::new(),
            status: Mutex::new(HashMap::new()),
            edits: tokio::sync::Mutex::new(()),
            me: me.clone(),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The configuration file of `workspace` (global when `None`).
    pub fn config_path(&self, workspace: Option<&str>) -> Result<PathBuf, String> {
        match workspace {
            None => Ok(self.data_dir.join(CONFIG_FILE)),
            Some(id) if is_valid_workspace_id(id) => {
                Ok(self.data_dir.join("workspaces").join(id).join(CONFIG_FILE))
            }
            Some(_) => Err("invalid workspace id".into()),
        }
    }

    /// A scope's configuration document and what it says (an empty one when
    /// the file does not exist).
    pub fn read(&self, workspace: Option<&str>) -> Result<(Value, McpConfig), String> {
        let path = self.config_path(workspace)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{} could not be read: {e}", path.display())),
        };
        config::parse_text(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Change a scope's configuration with `edit` and save it. Servers of
    /// that scope are restarted on next use.
    pub async fn edit(
        &self,
        workspace: Option<&str>,
        edit: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
    ) -> Result<McpConfig, String> {
        let _one = self.edits.lock().await;
        let path = self.config_path(workspace)?;
        let (mut doc, _) = self.read(workspace)?;
        edit(&mut doc).map_err(|e| e.to_string())?;
        let config = config::parse_doc(&doc).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        write_atomic(&path, &text)
            .map_err(|e| format!("{} could not be saved: {e}", path.display()))?;
        self.restart_scope(&scope_key(
            if workspace.is_some() {
                Scope::Workspace
            } else {
                Scope::Global
            },
            workspace,
        ));
        Ok(config)
    }

    /// Make sure the scope's file exists (for "Edit as file"); returns it.
    pub async fn ensure_file(&self, workspace: Option<&str>) -> Result<PathBuf, String> {
        let _one = self.edits.lock().await;
        let path = self.config_path(workspace)?;
        if !path.exists() {
            let text =
                serde_json::to_string_pretty(&config::empty_doc()).map_err(|e| e.to_string())?;
            write_atomic(&path, &text).map_err(|e| e.to_string())?;
        }
        Ok(path)
    }

    /// Stop the running servers of a scope (they start again when needed)
    /// and forget their status.
    fn restart_scope(&self, scope_key: &str) {
        let prefix = format!("{scope_key}/");
        lock(&self.status).retain(|k, _| !k.starts_with(&prefix));
        let keys: Vec<u64> = self
            .pool
            .iter()
            .filter(|e| e.value().scope == scope_key)
            .map(|e| *e.key())
            .collect();
        for key in keys {
            if let Some((_, slot)) = self.pool.remove(&key) {
                // A server starting right now is dropped (and stopped) by its
                // starter; one that is running stops here.
                if let Ok(mut live) = slot.live.try_lock() {
                    if let Some(live) = live.take() {
                        live.client.shutdown();
                    }
                }
            }
        }
    }

    /// The last known status of a server.
    pub fn status_of(&self, workspace: Option<&str>, scope: Scope, name: &str) -> ServerStatus {
        lock(&self.status)
            .get(&status_key(&scope_key(scope, workspace), name))
            .cloned()
            .unwrap_or(ServerStatus {
                state: "unknown",
                error: None,
                tools: Vec::new(),
            })
    }

    fn set_status(&self, key: &str, status: ServerStatus) {
        lock(&self.status).insert(key.to_string(), status);
    }

    /// The running server under `key`, started if needed.
    async fn live(
        &self,
        key: u64,
        status: &str,
        transport: &Transport,
    ) -> Result<Arc<Live>, String> {
        let scope = status.split('/').next().unwrap_or_default().to_string();
        let slot = self
            .pool
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Slot {
                    scope,
                    live: tokio::sync::Mutex::new(None),
                    failed: Mutex::new(None),
                })
            })
            .clone();
        let mut live_slot = slot.live.lock().await;
        if let Some(live) = live_slot.as_ref().filter(|l| l.client.is_alive()) {
            return Ok(live.clone());
        }
        if let Some((at, error)) = lock(&slot.failed).as_ref() {
            if at.elapsed() < RETRY_AFTER {
                return Err(error.clone());
            }
        }
        let started = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let client = McpClient::connect(transport).await?;
            let tools = client.list_tools().await?;
            Ok::<_, McpError>(Live { client, tools })
        })
        .await
        .unwrap_or_else(|_| Err(McpError::Timeout("start-up".into())));
        match started {
            Ok(live) => {
                let live = Arc::new(live);
                self.set_status(
                    status,
                    ServerStatus {
                        state: "connected",
                        error: None,
                        tools: live.tools.clone(),
                    },
                );
                *live_slot = Some(live.clone());
                *lock(&slot.failed) = None;
                Ok(live)
            }
            Err(e) => {
                let error = e.to_string();
                tracing::warn!(target: "shodh::mcp", server = %status, error = %error, "MCP server unavailable");
                self.set_status(
                    status,
                    ServerStatus {
                        state: "error",
                        error: Some(error.clone()),
                        tools: Vec::new(),
                    },
                );
                *lock(&slot.failed) = Some((std::time::Instant::now(), error.clone()));
                Err(error)
            }
        }
    }

    /// How `server` is started in `folder`: `${workspaceFolder}` filled in,
    /// and for the pinned enola its dashboard and update check off and its
    /// index excluded from git in the folder's repository.
    async fn launch(
        &self,
        server: &ServerConfig,
        folder: Option<&str>,
    ) -> Result<Transport, String> {
        let transport = server.transport.resolved(folder)?;
        if !enola::is_pinned(server, &self.data_dir) {
            return Ok(transport);
        }
        if let Some(folder) = folder {
            let root = PathBuf::from(folder);
            let excluded =
                tokio::task::spawn_blocking(move || exclude_locally(&root, enola::INDEX_EXCLUDE))
                    .await
                    .map_err(|e| e.to_string())?;
            // Without the exclusion enola's index would block Code mode's
            // changes and end up in "Discard changes" commits.
            excluded.map_err(|e| format!("enola's index could not be kept out of git: {e}"))?;
        }
        Ok(enola::harden(transport))
    }

    /// Start (or reuse) `server` of `scope` and list its tools. `folder`
    /// fills in `${workspaceFolder}`; without one, a server that needs it is
    /// started in an empty folder so its tools can still be listed.
    pub async fn test(
        &self,
        workspace: Option<&str>,
        scope: Scope,
        server: &ServerConfig,
        folder: Option<&str>,
    ) -> ServerStatus {
        let probe = self.data_dir.join("mcp").join("probe");
        let folder = match folder {
            Some(folder) => Some(folder.to_string()),
            None if server.transport.needs_workspace_folder() => {
                if let Err(e) = std::fs::create_dir_all(&probe) {
                    return ServerStatus {
                        state: "error",
                        error: Some(e.to_string()),
                        tools: Vec::new(),
                    };
                }
                Some(probe.display().to_string())
            }
            None => None,
        };
        let skey = scope_key(scope, workspace);
        let status = status_key(&skey, &server.name);
        let transport = match self.launch(server, folder.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                return ServerStatus {
                    state: "error",
                    error: Some(e),
                    tools: Vec::new(),
                }
            }
        };
        let key = connection_key(&skey, &server.name, &transport);
        // A test always starts the server afresh.
        if let Some((_, slot)) = self.pool.remove(&key) {
            if let Some(live) = slot.live.lock().await.take() {
                live.client.shutdown();
            }
        }
        let _ = self.live(key, &status, &transport).await;
        self.status_of(workspace, scope, &server.name)
    }

    /// The MCP and skill tools of a chat in `workspace` and `mode`. `folder`
    /// is the workspace's code folder (for `${workspaceFolder}`); `reserved`
    /// are the names the chat's built-in tools already use; `builtin` is how
    /// many there are. `local_only` keeps servers off other computers.
    pub async fn chat_tools(
        &self,
        workspace: Option<&str>,
        mode: Mode,
        folder: Option<&str>,
        reserved: &[&str],
        builtin: usize,
        local_only: bool,
    ) -> ChatTools {
        let global = self.read(None).map(|(_, c)| c).unwrap_or_else(|e| {
            tracing::warn!(target: "shodh::mcp", error = %e, "global MCP config unreadable");
            McpConfig::default()
        });
        let local = workspace.map(|id| {
            self.read(Some(id)).map(|(_, c)| c).unwrap_or_else(|e| {
                tracing::warn!(target: "shodh::mcp", error = %e, "workspace MCP config unreadable");
                McpConfig::default()
            })
        });
        let servers: Vec<(Scope, ServerConfig)> = config::merge(&global, local.as_ref())
            .into_iter()
            .filter(|(_, s)| !s.disabled && s.in_mode(mode))
            .collect();
        let load_mode = match mode {
            Mode::Code => ToolLoadMode::Essential,
            Mode::Research => ToolLoadMode::Discoverable,
        };
        let started = futures::future::join_all(servers.iter().map(|(scope, server)| async move {
            let skey = scope_key(*scope, workspace);
            let status = status_key(&skey, &server.name);
            let transport = match &server.transport {
                Transport::Http { url, .. } if local_only && !is_loopback_url(url) => {
                    return Err(
                        "Local-only mode is on, and this server is on another computer".to_string(),
                    )
                }
                _ => self.launch(server, folder).await?,
            };
            let key = connection_key(&skey, &server.name, &transport);
            let live = self.live(key, &status, &transport).await?;
            Ok((key, status, transport, live))
        }))
        .await;

        let mut taken: HashSet<String> = reserved.iter().map(|s| s.to_string()).collect();
        let mut tools: Vec<Arc<dyn HostTool>> = Vec::new();
        let mut view_servers = Vec::new();
        let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
        for ((scope, server), outcome) in servers.iter().zip(started) {
            match outcome {
                Ok((key, status, transport, live)) => {
                    let caller: Arc<dyn McpCaller> = Arc::new(PooledCaller {
                        manager: self.me.clone(),
                        key,
                        status,
                        transport,
                    });
                    let made = server_tools(
                        server,
                        &live.tools,
                        caller,
                        load_mode,
                        mode,
                        enola::verified(server, &self.data_dir),
                        &mut taken,
                    );
                    let names: Vec<String> = made.iter().map(|t| t.name().to_string()).collect();
                    for tool in &made {
                        (
                            tool.name(),
                            tool.approval() == Approval::Ask,
                            tool.description(),
                        )
                            .hash(&mut fingerprint);
                        key.hash(&mut fingerprint);
                    }
                    tools.extend(made.into_iter().map(|t| Arc::new(t) as Arc<dyn HostTool>));
                    view_servers.push(ChatServer {
                        name: server.name.clone(),
                        scope: *scope,
                        error: None,
                        tools: names,
                    });
                }
                Err(error) => view_servers.push(ChatServer {
                    name: server.name.clone(),
                    scope: *scope,
                    error: Some(error),
                    tools: Vec::new(),
                }),
            }
        }

        let skills = skills::enabled_skills(&self.data_dir, workspace, mode);
        let summaries: Vec<SkillSummary> = skills
            .iter()
            .map(|s| SkillSummary {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect();
        if !skills.is_empty() {
            for skill in &skills {
                (&skill.name, &skill.description, &skill.dir).hash(&mut fingerprint);
            }
            tools.extend(skill_tools(skills));
        }
        let total = builtin + tools.len();
        // Research's built-in tools are loaded on demand; Code mode's and the
        // added ones are in the model's context on every turn.
        let always_loaded = match mode {
            Mode::Code => total,
            Mode::Research => tools.len(),
        };
        ChatTools {
            view: ChatToolsView {
                servers: view_servers,
                skills: summaries,
                builtin,
                total,
                many: always_loaded > MANY_TOOLS,
            },
            tools,
            fingerprint: fingerprint.finish(),
        }
    }
}

/// `load_skill` and `read_skill_file` over `skills`.
fn skill_tools(skills: Vec<Skill>) -> Vec<Arc<dyn HostTool>> {
    vec![
        Arc::new(LoadSkillTool::new(skills.clone())),
        Arc::new(ReadSkillFileTool::new(skills)),
    ]
}

/// Write `text` to `path` through a temporary file (no half-written config).
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workspace_files_live_under_the_workspace_id() {
        let manager = McpManager::new(PathBuf::from("/data"));
        assert_eq!(
            manager.config_path(None).unwrap(),
            PathBuf::from("/data/mcp.json")
        );
        assert_eq!(
            manager.config_path(Some("ws-1")).unwrap(),
            PathBuf::from("/data/workspaces/ws-1/mcp.json")
        );
        assert!(manager.config_path(Some("../x")).is_err());
        assert!(manager.config_path(Some("")).is_err());
    }

    #[test]
    fn loopback_urls_are_recognised() {
        assert!(is_loopback_url("http://127.0.0.1:9000/mcp"));
        assert!(is_loopback_url("http://localhost/mcp"));
        assert!(is_loopback_url("http://[::1]:80/"));
        assert!(!is_loopback_url("https://mcp.example.com/mcp"));
        assert!(!is_loopback_url("http://10.0.0.2/"));
    }

    #[tokio::test]
    async fn edits_are_saved_and_a_workspace_overrides_global_servers() {
        let dir = tempfile::tempdir().unwrap();
        let manager = McpManager::new(dir.path().to_path_buf());
        manager
            .edit(None, |doc| {
                config::add_servers(
                    doc,
                    vec![("shared".into(), json!({"command": "global-cmd"}))],
                )
            })
            .await
            .unwrap();
        manager
            .edit(Some("ws1"), |doc| {
                config::add_servers(
                    doc,
                    vec![(
                        "shared".into(),
                        json!({"command": "ws-cmd", "disabled": true}),
                    )],
                )
            })
            .await
            .unwrap();
        let (_, global) = manager.read(None).unwrap();
        assert_eq!(global.servers[0].transport.summary(), "global-cmd");
        let saved = std::fs::read_to_string(dir.path().join("workspaces/ws1/mcp.json")).unwrap();
        assert!(saved.contains("ws-cmd"));
        // The workspace's disabled copy hides the global one in its chats.
        let tools = manager
            .chat_tools(Some("ws1"), Mode::Research, None, &[], 30, false)
            .await;
        assert!(tools.view.servers.is_empty());
        assert_eq!(tools.view.total, 30);
        assert!(!tools.view.many);
        // Research's many built-in tools load on demand: no warning for them.
        let research = manager
            .chat_tools(None, Mode::Research, None, &[], 56, false)
            .await;
        assert!(!research.view.many);
        let code = manager
            .chat_tools(None, Mode::Code, None, &[], 56, false)
            .await;
        assert!(code.view.many);
        // A bad edit changes nothing.
        let refused = manager
            .edit(None, |doc| {
                config::set_tool(doc, "missing", "t", Some(false), None)
            })
            .await;
        assert!(refused.is_err());
        assert_eq!(manager.read(None).unwrap().1.servers.len(), 1);
    }

    #[tokio::test]
    async fn servers_that_cannot_start_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let manager = McpManager::new(dir.path().to_path_buf());
        manager
            .edit(None, |doc| {
                config::add_servers(
                    doc,
                    vec![
                        (
                            "missing".into(),
                            json!({"command": "shodh-no-such-mcp-server"}),
                        ),
                        (
                            "folder".into(),
                            json!({"command": "x", "cwd": "${workspaceFolder}"}),
                        ),
                        (
                            "remote".into(),
                            json!({"url": "https://mcp.example.invalid/mcp"}),
                        ),
                        (
                            "code-only".into(),
                            json!({"command": "x", "shodh": {"modes": ["code"]}}),
                        ),
                    ],
                )
            })
            .await
            .unwrap();
        let tools = manager
            .chat_tools(None, Mode::Research, None, &[], 5, true)
            .await;
        let errors: Vec<(&str, &str)> = tools
            .view
            .servers
            .iter()
            .map(|s| (s.name.as_str(), s.error.as_deref().unwrap_or("")))
            .collect();
        assert_eq!(
            errors.len(),
            3,
            "the Code-only server is not offered: {errors:?}"
        );
        assert!(errors[0].1.contains("could not be started"), "{errors:?}");
        assert!(errors[1].1.contains("code folder"), "{errors:?}");
        assert!(errors[2].1.contains("Local-only"), "{errors:?}");
        assert!(tools.tools.is_empty());
        assert_eq!(
            manager.status_of(None, Scope::Global, "missing").state,
            "error"
        );
        // A failed server is not started again on the next chat.
        let again = std::time::Instant::now();
        let tools = manager
            .chat_tools(None, Mode::Research, None, &[], 5, true)
            .await;
        assert!(again.elapsed() < Duration::from_secs(1));
        assert_eq!(tools.view.servers[0].error.as_deref(), Some(errors[0].1));
    }
}
