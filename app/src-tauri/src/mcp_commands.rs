//! Settings → Tools & connections, and the composer's tool chip.
//!
//! These commands manage what the agent may use (servers, per-tool approval,
//! skills); the agent itself is never given them (see `agent-coverage.json`):
//! an agent that could connect servers or relax approvals could widen its own
//! reach.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use shodh_rag::audit::{AuditEventType, AuditQuery};
use shodh_rag::harness::code_mode::CODE_TOOLS;
use shodh_rag::harness::events::RiskTier;
use shodh_rag::harness::mcp::config::{self, ServerProblem};
use shodh_rag::harness::mcp::{Approval, Mode, Scope, ServerConfig, Transport};
use shodh_rag::harness::skills::{resource_files, SkillProblem};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::agent_session_commands::{base_registry, chat_extra_tools, workspace_folder};
use crate::audit_commands::AuditState;
use crate::mcp::skills::{self, SkillInstaller, SkillSource, StagedInstall};
use crate::mcp::{enola, ChatToolsView, McpManager, ServerStatus};

/// The MCP manager, as Tauri state.
pub struct McpState(pub Arc<McpManager>);

type CommandResult<T> = Result<T, String>;

fn workspace(id: Option<String>) -> CommandResult<Option<String>> {
    match id.map(|i| i.trim().to_string()).filter(|i| !i.is_empty()) {
        Some(id) if crate::mcp::is_valid_workspace_id(&id) => Ok(Some(id)),
        Some(_) => Err("invalid workspace id".into()),
        None => Ok(None),
    }
}

/// One tool of a server, for the settings page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolView {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub read_only: bool,
    pub enabled: bool,
    pub approval: Approval,
}

/// One server of a scope, for the settings page. Never carries env values
/// or headers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub name: String,
    /// `stdio` or `http`.
    pub kind: &'static str,
    /// The command line or URL.
    pub target: String,
    pub enabled: bool,
    /// Empty: every mode.
    pub modes: Vec<Mode>,
    /// `connected`, `error` or `unknown`.
    pub status: &'static str,
    pub error: Option<String>,
    pub tools: Vec<ToolView>,
    /// Runs in the workspace's code folder.
    pub needs_folder: bool,
}

fn server_view(server: &ServerConfig, status: ServerStatus) -> ServerView {
    ServerView {
        name: server.name.clone(),
        kind: match server.transport {
            Transport::Stdio { .. } => "stdio",
            Transport::Http { .. } => "http",
        },
        target: server.transport.summary(),
        enabled: !server.disabled,
        modes: server.modes.clone(),
        status: status.state,
        error: status.error,
        needs_folder: server.transport.needs_workspace_folder(),
        tools: status
            .tools
            .iter()
            .map(|t| ToolView {
                name: t.name.clone(),
                title: t
                    .title
                    .clone()
                    .or_else(|| t.annotations.as_ref().and_then(|a| a.title.clone())),
                description: t.description.clone(),
                read_only: t.read_only(),
                enabled: server.tool_enabled(&t.name),
                approval: server.approval_for(&t.name, t.read_only()),
            })
            .collect(),
    }
}

/// A built-in tool, for the settings page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinTool {
    pub name: String,
    pub label: String,
    pub description: String,
    /// `read`, `write` or `destructive`.
    pub tier: &'static str,
    /// When the agent last called it (RFC 3339), from the audit log.
    pub last_used: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinGroup {
    pub group: &'static str,
    pub tools: Vec<BuiltinTool>,
}

/// The settings-page group of a built-in tool.
pub fn tool_group(name: &str) -> &'static str {
    match name {
        "search_documents" | "open_document" | "list_sources" | "show_document" | "show_source"
        | "add_folder" | "reindex_source" | "remove_source" | "list_directory"
        | "create_folder" | "download_file" | "export_document" => "Library and files",
        "web_search" | "fetch_url" | "search_papers" => "Web",
        "query_results" | "paper_graph" | "get_paper" | "find_papers" | "show_figure"
        | "get_equation" | "list_snippets" | "create_snippet" => "Papers and research",
        "create_task" | "create_event" | "list_tasks" | "list_events" | "update_task"
        | "complete_task" | "update_event" | "delete_task" | "delete_event" | "show_calendar" => {
            "Tasks and calendar"
        }
        "remember" | "recall" | "update_memory" | "forget" => "Memory",
        "list_workspaces"
        | "open_workspace"
        | "create_workspace"
        | "update_workspace"
        | "add_to_workspace"
        | "remove_from_workspace" => "Workspaces",
        "search_conversations"
        | "open_conversation"
        | "organize_conversation"
        | "list_visuals"
        | "open_visual"
        | "revise_visual"
        | "organize_visual" => "Conversations and visuals",
        "update_plan" | "open_view" | "show_audit" | "audit_query" | "get_settings"
        | "update_setting" => "App",
        _ => "Other",
    }
}

fn tier_name(tier: RiskTier) -> &'static str {
    match tier {
        RiskTier::Read => "read",
        RiskTier::Write => "write",
        RiskTier::Destructive => "destructive",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub name: String,
    pub description: String,
    /// On in this scope.
    pub enabled: bool,
    pub files: usize,
    /// Empty: every mode.
    pub modes: Vec<Mode>,
    /// License and repository, for skills installed from a recommended set.
    pub license: Option<String>,
    pub repo: Option<String>,
}

/// A recommended skill set, for the settings page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendedView {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub repo: &'static str,
    pub license: &'static str,
    /// The skills it installs (folder names).
    pub skills: Vec<&'static str>,
    pub modes: Vec<Mode>,
    /// Every one of its skills is installed.
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnolaView {
    pub version: &'static str,
    /// enola publishes a build for this computer.
    pub supported: bool,
    pub installed: bool,
    /// The global config has an `enola` server.
    pub registered: bool,
}

/// Everything the Tools & connections page shows for one scope.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsOverview {
    pub scope: Scope,
    pub config_path: String,
    pub servers: Vec<ServerView>,
    /// Entries of the file that could not be read.
    pub problems: Vec<ServerProblem>,
    /// Why the whole file could not be read, if so.
    pub config_error: Option<String>,
    pub builtin: Vec<BuiltinGroup>,
    pub skills: Vec<SkillView>,
    pub skill_problems: Vec<SkillProblem>,
    pub recommended: Vec<RecommendedView>,
    pub enola: EnolaView,
}

/// When each tool was last called, from the newest `tool_call` events.
fn last_used(audit: &AuditState) -> BTreeMap<String, String> {
    let Some(log) = audit.log() else {
        return BTreeMap::new();
    };
    let query = AuditQuery {
        types: vec![AuditEventType::ToolCall],
        limit: Some(5_000),
        ..AuditQuery::default()
    };
    let mut seen = BTreeMap::new();
    match log.query(&query) {
        Ok(rows) => {
            for row in rows {
                if let Some(tool) = row.payload.get("tool").and_then(|t| t.as_str()) {
                    seen.entry(tool.to_string()).or_insert(row.ts);
                }
            }
        }
        Err(e) => tracing::warn!(target: "shodh::mcp", error = %e, "tool usage unreadable"),
    }
    seen
}

#[tauri::command]
pub async fn tools_overview(
    app: AppHandle,
    workspace_id: Option<String>,
    mcp: State<'_, McpState>,
    audit: State<'_, AuditState>,
) -> CommandResult<ToolsOverview> {
    let workspace = workspace(workspace_id)?;
    let manager = mcp.0.clone();
    let scope = if workspace.is_some() {
        Scope::Workspace
    } else {
        Scope::Global
    };
    let config_path = manager.config_path(workspace.as_deref())?;
    let (servers, problems, config_error) = match manager.read(workspace.as_deref()) {
        Ok((_, config)) => (
            config
                .servers
                .iter()
                .map(|s| server_view(s, manager.status_of(workspace.as_deref(), scope, &s.name)))
                .collect(),
            config.problems,
            None,
        ),
        Err(e) => (Vec::new(), Vec::new(), Some(e)),
    };

    let used = last_used(&audit);
    let registry = base_registry(&app).await.map_err(|e| e.message)?;
    let mut groups: BTreeMap<&'static str, Vec<BuiltinTool>> = BTreeMap::new();
    for name in registry.names() {
        let Some((label, description, tier)) = registry.describe(name) else {
            continue;
        };
        groups
            .entry(tool_group(name))
            .or_default()
            .push(BuiltinTool {
                name: name.to_string(),
                label,
                description,
                tier: tier_name(tier),
                last_used: used.get(name).cloned(),
            });
    }
    let mut builtin: Vec<BuiltinGroup> = groups
        .into_iter()
        .map(|(group, tools)| BuiltinGroup { group, tools })
        .collect();
    builtin.push(BuiltinGroup {
        group: "Code mode",
        tools: CODE_TOOLS
            .iter()
            .map(|name| BuiltinTool {
                name: name.to_string(),
                label: name.to_string(),
                description: match *name {
                    "read" => "Read a file of the code folder.",
                    "grep" => "Search the code folder's text.",
                    "glob" => "Find files of the code folder by name.",
                    "ast_grep" => "Search code by its syntax.",
                    "edit" => "Change part of a file (asks first).",
                    "write" => "Create or replace a file (asks first).",
                    _ => "Run one shell command in the code folder (asks first).",
                }
                .to_string(),
                tier: if matches!(*name, "edit" | "write" | "bash") {
                    "write"
                } else {
                    "read"
                },
                last_used: used.get(*name).cloned(),
            })
            .collect(),
    });

    let data_dir = manager.data_dir().to_path_buf();
    let settings = skills::load_settings(&data_dir);
    let (installed, skill_problems) = skills::installed(&data_dir);
    let skills = installed
        .iter()
        .map(|s| SkillView {
            name: s.name.clone(),
            description: s.description.clone(),
            enabled: settings.enabled(&s.name, workspace.as_deref()),
            files: resource_files(&s.dir).len(),
            modes: settings.modes.get(&s.name).cloned().unwrap_or_default(),
            license: settings.origins.get(&s.name).map(|o| o.license.clone()),
            repo: settings.origins.get(&s.name).map(|o| o.repo.clone()),
        })
        .collect();
    let names: Vec<&str> = installed.iter().map(|s| s.name.as_str()).collect();
    let recommended = skills::RECOMMENDED
        .iter()
        .map(|set| {
            let skills: Vec<&'static str> = set
                .folders
                .iter()
                .map(|f| f.rsplit('/').next().unwrap_or(f))
                .collect();
            RecommendedView {
                id: set.id,
                title: set.title,
                description: set.description,
                repo: set.repo,
                license: set.license,
                installed: names
                    .iter()
                    .filter(|n| {
                        settings
                            .origins
                            .get(**n)
                            .is_some_and(|o| o.repo == set.repo)
                    })
                    .count()
                    >= set.folders.len(),
                skills,
                modes: set.modes.to_vec(),
            }
        })
        .collect();

    let registered = manager
        .read(None)
        .map(|(_, c)| c.servers.iter().any(|s| s.name == enola::SERVER_NAME))
        .unwrap_or(false);
    Ok(ToolsOverview {
        scope,
        config_path: config_path.display().to_string(),
        servers,
        problems,
        config_error,
        builtin,
        skills,
        skill_problems,
        recommended,
        enola: EnolaView {
            version: enola::VERSION,
            supported: enola::platform().is_some(),
            installed: enola::binary_path(&data_dir).is_file(),
            registered,
        },
    })
}

/// Start a server (afresh) and list its tools.
#[tauri::command]
pub async fn mcp_test_server(
    app: AppHandle,
    workspace_id: Option<String>,
    name: String,
    mcp: State<'_, McpState>,
) -> CommandResult<ServerView> {
    let workspace = workspace(workspace_id)?;
    let manager = mcp.0.clone();
    let (_, config) = manager.read(workspace.as_deref())?;
    let server = config
        .servers
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("No server named {name}"))?;
    let scope = if workspace.is_some() {
        Scope::Workspace
    } else {
        Scope::Global
    };
    let folder = workspace_folder(&app, workspace.as_deref()).await;
    let status = manager
        .test(workspace.as_deref(), scope, server, folder.as_deref())
        .await;
    Ok(server_view(server, status))
}

/// Add servers from pasted `mcp.json` text (or the form, which sends the
/// same). Returns the names added.
#[tauri::command]
pub async fn mcp_add_servers(
    workspace_id: Option<String>,
    config_text: String,
    mcp: State<'_, McpState>,
) -> CommandResult<Vec<String>> {
    let workspace = workspace(workspace_id)?;
    let entries = config::parse_pasted(&config_text).map_err(|e| e.to_string())?;
    let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
    mcp.0
        .edit(workspace.as_deref(), |doc| {
            config::add_servers(doc, entries)
        })
        .await?;
    tracing::info!(target: "shodh::mcp", servers = ?names, "MCP servers added");
    Ok(names)
}

#[tauri::command]
pub async fn mcp_remove_server(
    workspace_id: Option<String>,
    name: String,
    mcp: State<'_, McpState>,
) -> CommandResult<()> {
    let workspace = workspace(workspace_id)?;
    mcp.0
        .edit(workspace.as_deref(), |doc| {
            config::remove_server(doc, &name).map(|_| ())
        })
        .await?;
    Ok(())
}

/// Turn a server on or off and/or set its modes.
#[tauri::command]
pub async fn mcp_set_server(
    workspace_id: Option<String>,
    name: String,
    enabled: Option<bool>,
    modes: Option<Vec<Mode>>,
    mcp: State<'_, McpState>,
) -> CommandResult<()> {
    let workspace = workspace(workspace_id)?;
    mcp.0
        .edit(workspace.as_deref(), |doc| {
            if let Some(enabled) = enabled {
                config::set_server_enabled(doc, &name, enabled)?;
            }
            if let Some(modes) = &modes {
                config::set_server_modes(doc, &name, modes)?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

/// Turn one tool on or off and/or set whether it asks first.
#[tauri::command]
pub async fn mcp_set_tool(
    workspace_id: Option<String>,
    server: String,
    tool: String,
    enabled: Option<bool>,
    approval: Option<Approval>,
    mcp: State<'_, McpState>,
) -> CommandResult<()> {
    let workspace = workspace(workspace_id)?;
    mcp.0
        .edit(workspace.as_deref(), |doc| {
            config::set_tool(doc, &server, &tool, enabled, approval)
        })
        .await?;
    Ok(())
}

/// "Edit as file": open the scope's `mcp.json` with the system's default
/// app (created empty if needed). Returns its path.
#[tauri::command]
pub async fn mcp_open_config(
    app: AppHandle,
    workspace_id: Option<String>,
    mcp: State<'_, McpState>,
) -> CommandResult<String> {
    let workspace = workspace(workspace_id)?;
    let path = mcp.0.ensure_file(workspace.as_deref()).await?;
    let shown = path.display().to_string();
    app.opener()
        .open_path(&shown, None::<&str>)
        .map_err(|e| format!("{shown} could not be opened: {e}"))?;
    Ok(shown)
}

/// Download, verify and register enola (see `mcp::enola`).
#[tauri::command]
pub async fn enola_install(mcp: State<'_, McpState>) -> CommandResult<String> {
    let manager = mcp.0.clone();
    let binary = enola::install(manager.data_dir()).await?;
    manager
        .edit(None, |doc| enola::register(doc, &binary))
        .await?;
    Ok(binary.display().to_string())
}

#[tauri::command]
pub async fn skills_prepare_install(
    source: String,
    mcp: State<'_, McpState>,
    installer: State<'_, SkillInstaller>,
) -> CommandResult<StagedInstall> {
    let source = SkillSource::parse(&source)?;
    installer.prepare(mcp.0.data_dir(), source).await
}

/// Stage a recommended skill set (its pinned commit, its listed skills only).
#[tauri::command]
pub async fn skills_prepare_recommended(
    id: String,
    mcp: State<'_, McpState>,
    installer: State<'_, SkillInstaller>,
) -> CommandResult<StagedInstall> {
    let set = skills::recommended(&id).ok_or_else(|| format!("No recommended skills {id}"))?;
    installer
        .prepare(mcp.0.data_dir(), SkillSource::Recommended(set))
        .await
}

/// Set the modes a skill is offered in (empty: every mode).
#[tauri::command]
pub async fn skills_set_modes(
    name: String,
    modes: Vec<Mode>,
    mcp: State<'_, McpState>,
) -> CommandResult<()> {
    let data_dir = mcp.0.data_dir().to_path_buf();
    let mut settings = skills::load_settings(&data_dir);
    settings.set_modes(&name, &modes);
    skills::save_settings(&data_dir, &settings)
}

#[tauri::command]
pub async fn skills_confirm_install(
    token: String,
    mcp: State<'_, McpState>,
    installer: State<'_, SkillInstaller>,
) -> CommandResult<Vec<String>> {
    let names = installer.confirm(mcp.0.data_dir(), &token).await?;
    tracing::info!(target: "shodh::mcp", skills = ?names, "skills installed");
    Ok(names)
}

#[tauri::command]
pub async fn skills_cancel_install(
    token: String,
    installer: State<'_, SkillInstaller>,
) -> CommandResult<()> {
    installer.cancel(&token);
    Ok(())
}

/// Turn a skill on or off in a workspace (or everywhere).
#[tauri::command]
pub async fn skills_set_enabled(
    workspace_id: Option<String>,
    name: String,
    enabled: bool,
    mcp: State<'_, McpState>,
) -> CommandResult<()> {
    let workspace = workspace(workspace_id)?;
    let data_dir = mcp.0.data_dir().to_path_buf();
    let mut settings = skills::load_settings(&data_dir);
    settings.set(&name, workspace.as_deref(), enabled);
    skills::save_settings(&data_dir, &settings)
}

#[tauri::command]
pub async fn skills_remove(name: String, mcp: State<'_, McpState>) -> CommandResult<bool> {
    skills::remove(mcp.0.data_dir(), &name)
}

/// What a chat in `workspace_id` and `mode` can use (the composer chip):
/// the same computation the agent session makes.
#[tauri::command]
pub async fn tools_for_chat(
    app: AppHandle,
    workspace_id: Option<String>,
    mode: Mode,
) -> CommandResult<ChatToolsView> {
    let workspace = workspace(workspace_id)?;
    let chat = chat_extra_tools(&app, workspace.as_deref(), mode)
        .await
        .map_err(|e| e.message)?;
    Ok(chat.view)
}

/// Forget a deleted workspace's MCP config and skill choices.
pub fn forget_workspace(app: &AppHandle, workspace_id: &str) {
    let Some(state) = app.try_state::<McpState>() else {
        return;
    };
    let data_dir = state.0.data_dir().to_path_buf();
    if crate::mcp::is_valid_workspace_id(workspace_id) {
        let dir = data_dir.join("workspaces").join(workspace_id);
        if dir.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                tracing::warn!(target: "shodh::mcp", error = %e, "workspace tool settings not removed");
            }
        }
    }
    let mut settings = skills::load_settings(&data_dir);
    if settings.workspaces.remove(workspace_id).is_some() {
        if let Err(e) = skills::save_settings(&data_dir, &settings) {
            tracing::warn!(target: "shodh::mcp", error = %e, "skill settings not saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_tool_has_a_group() {
        use shodh_rag::harness::profile::{app_tools, CORE_TOOLS};
        for name in CORE_TOOLS.iter().chain(app_tools::ALL.iter()) {
            assert_ne!(tool_group(name), "Other", "{name} has no settings group");
        }
    }

    #[test]
    fn server_views_carry_no_secrets() {
        let server = config::parse_server(
            "gh",
            &serde_json::json!({"command": "npx", "args": ["-y", "gh"], "env": {"TOKEN": "ghp_secret"}}),
        )
        .unwrap();
        let view = server_view(&server, ServerStatus::default());
        let text = serde_json::to_string(&view).unwrap();
        assert!(!text.contains("ghp_secret"));
        assert_eq!(view.target, "npx -y gh");
        assert_eq!(view.kind, "stdio");
    }
}
