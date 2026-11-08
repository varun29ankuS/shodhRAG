//! The code map's tools, for Code sessions only (read):
//! - `code_definition`: where a symbol is defined;
//! - `code_references`: who calls or uses it;
//! - `code_outline`: the symbols of a file or module;
//! - `code_dependencies`: what a module imports, what imports it, and its
//!   cycles.
//!
//! Each answers with `path:line` receipts from Code mode's code map
//! ([`shodh_rag::code_map`]) and shows the top result in the code pane, so the
//! user sees what the agent looked at. Before answering, the map is brought
//! up to date with the folder (only changed files are read again), unless
//! indexing is paused or another refresh holds it; the answer then says so.
//! The map reads Rust, TypeScript/JavaScript, Python and Go; it never holds
//! ignored files, dependency or build folders, `.git`, environment files or
//! keys.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::code_map::{CodeMap, Pace, RefreshOptions, SymbolHit, SymbolKind};
use shodh_rag::harness::events::NavigationTarget;
use shodh_rag::harness::tools::navigate::{emit_shown, SHOWN_TO_USER};
use shodh_rag::harness::tools::{HostTool, ToolContext, ToolError, ToolOutput};
use shodh_rag::harness::RiskTier;

use super::code_intel::grouped;
use super::{invalid, str_arg};
use crate::agent_session_commands::workspace_code_folder;
use crate::code_map_commands::CodeMapState;
use crate::workspace_commands::WorkspaceState;

pub const CODE_DEFINITION: &str = "code_definition";
pub const CODE_REFERENCES: &str = "code_references";
pub const CODE_OUTLINE: &str = "code_outline";
pub const CODE_DEPENDENCIES: &str = "code_dependencies";

/// The tools' names, which no MCP server or skill may take in a Code session.
pub const CODE_MAP_TOOL_NAMES: [&str; 4] = [
    CODE_DEFINITION,
    CODE_REFERENCES,
    CODE_OUTLINE,
    CODE_DEPENDENCIES,
];

/// Definitions listed at most.
const MAX_DEFINITIONS: usize = 10;
/// Uses listed at most.
const MAX_USES: usize = 60;
/// Changed files a tool reads itself before answering at most; more are
/// left to the background refresh.
const MAX_INLINE_READS: usize = 200;
/// The time a tool spends bringing the map up to date at most.
const INLINE_TIME: Duration = Duration::from_secs(5);
/// Lines of a definition selected in the code pane at most.
const MAX_SELECTED: u32 = 60;

/// The code folder a tool works on, and its workspace.
#[async_trait]
pub trait CodeFolderOf: Send + Sync {
    async fn folder(&self, ctx: &ToolContext, tool: &str) -> Result<(String, PathBuf), ToolError>;
}

/// The Code session's workspace's code folder.
pub struct WorkspaceFolder {
    pub workspaces: WorkspaceState,
    pub workspace_id: Option<String>,
}

#[async_trait]
impl CodeFolderOf for WorkspaceFolder {
    async fn folder(&self, ctx: &ToolContext, tool: &str) -> Result<(String, PathBuf), ToolError> {
        let workspace = self
            .workspace_id
            .clone()
            .or_else(|| ctx.scope().workspace.clone())
            .ok_or_else(|| {
                ToolError::Unavailable(format!(
                    "{tool} works on a workspace's code folder, and this chat is in no workspace."
                ))
            })?;
        let folder = workspace_code_folder(&self.workspaces, Some(&workspace))
            .await
            .map_err(ToolError::Unavailable)?;
        Ok((workspace, folder.root().to_path_buf()))
    }
}

struct Shared {
    maps: CodeMapState,
    folder: Arc<dyn CodeFolderOf>,
}

/// A Code session's code map tools ([`CODE_MAP_TOOL_NAMES`]).
pub fn code_map_tools(maps: CodeMapState, folder: Arc<dyn CodeFolderOf>) -> Vec<Arc<dyn HostTool>> {
    let shared = Arc::new(Shared { maps, folder });
    vec![
        Arc::new(DefinitionTool {
            shared: shared.clone(),
        }),
        Arc::new(ReferencesTool {
            shared: shared.clone(),
        }),
        Arc::new(OutlineTool {
            shared: shared.clone(),
        }),
        Arc::new(DependenciesTool { shared }),
    ]
}

/// A path the agent gives, as the map stores paths (`/`, no `./`).
fn path_arg(args: &Value, key: &str, tool: &str) -> Result<Option<String>, ToolError> {
    let Some(path) = str_arg(args, key) else {
        return Ok(None);
    };
    let path = path.replace('\\', "/");
    let path = path
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string();
    if path.len() > 1024
        || path.starts_with('/')
        || path.contains(':') && !path.contains("::")
        || path.split('/').any(|part| part == "..")
    {
        return Err(invalid(
            tool,
            format!("{key} must be relative to the code folder"),
        ));
    }
    Ok(Some(path))
}

fn name_arg<'a>(args: &'a Value, key: &str, tool: &str) -> Result<&'a str, ToolError> {
    let name = str_arg(args, key).ok_or_else(|| invalid(tool, format!("{key} is required")))?;
    if name.chars().count() > 300 {
        return Err(invalid(tool, format!("{key} is too long")));
    }
    Ok(name)
}

impl Shared {
    /// The map of this session's folder, brought up to date first when it can
    /// be; the note says when it could not.
    async fn map(
        &self,
        ctx: &ToolContext,
        tool: &str,
    ) -> Result<(String, Arc<CodeMap>, Option<String>), ToolError> {
        let (workspace, root) = self.folder.folder(ctx, tool).await?;
        let (project, map) = self
            .maps
            .open_for(&root)
            .await
            .map_err(ToolError::Unavailable)?;
        let maps = self.maps.clone();
        let fresh = map.clone();
        let note = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
            if !fresh.is_complete() {
                // Never built here: the background build is pushed ahead (it
                // shows in the status strip, and Pause and Stop reach it), and
                // the answer is from what it has read so far.
                maps.request(&project, false, true);
                if fresh.is_complete() {
                    return Ok(None);
                }
                let status = fresh.status()?;
                return Ok(Some(format!(
                    "The code map is still being built: {} of {} files read, so the answer covers only those. The build runs in the status strip; ask again later for the rest.",
                    status.files,
                    status.listed.map(|n| n.to_string()).unwrap_or_else(|| "the".into())
                )));
            }
            if maps.paused() {
                let status = fresh.status()?;
                return Ok(Some(format!(
                    "Indexing is paused, so the code map was not brought up to date first: it was last checked {}, and files changed since may be missed.",
                    age(status.checked_at.as_deref().or(status.updated_at.as_deref()))
                )));
            }
            // The files changed since the last refresh are read now (this
            // never waits behind another refresh); many at once are left to
            // the background.
            let options = RefreshOptions {
                max_reads: Some(MAX_INLINE_READS),
                deadline: Some(Instant::now() + INLINE_TIME),
            };
            let pace = || if maps.paused() { Pace::Stop } else { Pace::Go };
            let report = fresh.refresh_with(options, &pace, &mut |_, _| {})?;
            if !report.stopped {
                return Ok(None);
            }
            maps.request(&project, false, true);
            let status = fresh.status()?;
            Ok(Some(format!(
                "Many files changed: {} were read now and the rest are being read in the background (in the status strip); the map was last complete {}. Answers about the other changed files may be out of date.",
                report.read,
                age(status.complete_at.as_deref())
            )))
        })
        .await
        .map_err(|e| ToolError::Failed(format!("updating the code map did not finish: {e}")))?
        .map_err(ToolError::Unavailable)?;
        Ok((workspace, map, note))
    }
}

/// "3 min ago" from an RFC 3339 time.
fn age(at: Option<&str>) -> String {
    let Some(at) = at.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) else {
        return "never".to_string();
    };
    let seconds = (chrono::Utc::now() - at.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    match seconds {
        0..=59 => "under a minute ago".to_string(),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// Run a query on a blocking thread.
async fn query<T: Send + 'static>(
    map: Arc<CodeMap>,
    work: impl FnOnce(&CodeMap) -> Result<T, String> + Send + 'static,
) -> Result<T, ToolError> {
    tokio::task::spawn_blocking(move || work(&map))
        .await
        .map_err(|e| ToolError::Failed(format!("reading the code map did not finish: {e}")))?
        .map_err(ToolError::NotFound)
}

/// Show `path` at `line..=end` in the code pane.
fn show(ctx: &ToolContext, workspace: &str, path: &str, line: u32, end: u32, what: &str) {
    let line = line.max(1);
    let end = end.clamp(line, line.saturating_add(MAX_SELECTED));
    emit_shown(
        ctx,
        "ask",
        None,
        Some(NavigationTarget::Code {
            workspace_id: workspace.to_string(),
            conversation_id: ctx.audit_scope().map(|s| s.conversation_id.clone()),
            path: Some(path.to_string()),
            line: Some(line),
            end_line: Some(end),
            view: Some("file".to_string()),
        }),
        Some(what),
    );
}

fn with_note(mut text: String, note: Option<String>) -> String {
    if let Some(note) = note {
        text.push_str("\n\nNote: ");
        text.push_str(&note);
    }
    text
}

fn definition_line(hit: &SymbolHit) -> String {
    let mut line = format!(
        "- {} {} — {}:{}",
        hit.kind,
        hit.qualified(),
        hit.path,
        hit.line
    );
    if !hit.module.is_empty() {
        line.push_str(&format!(" (module {})", hit.module));
    }
    if !hit.signature.is_empty() {
        line.push_str(&format!(": {}", hit.signature));
    }
    line
}

const COVERS: &str = "The code map reads Rust, TypeScript/JavaScript, Python and Go files; for \
                      text in other files, search with grep.";

// ---- code_definition --------------------------------------------------------

struct DefinitionTool {
    shared: Arc<Shared>,
}

#[async_trait]
impl HostTool for DefinitionTool {
    fn name(&self) -> &str {
        CODE_DEFINITION
    }
    fn label(&self) -> &str {
        "Find definition"
    }
    fn label_template(&self) -> &str {
        "Finding where {name} is defined"
    }
    fn description(&self) -> &str {
        "Find where a function, method, type, trait, interface, class, module or constant is \
         defined in this project, from Shodh's code map (much faster and more exact than grep \
         for this). name is the symbol: open, Store::open or Store.open for a member, \
         crate::store::open or my_crate::Store for a path (re-exports are followed). kind \
         narrows it; path keeps it to a file or folder. Returns each definition as path:line \
         with its signature, and shows the first in the code pane. This changes nothing."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 300},
                "kind": {"type": "string", "enum": SymbolKind::names()},
                "path": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = CODE_DEFINITION;
        let name = name_arg(&args, "name", tool)?.to_string();
        let kind = match str_arg(&args, "kind") {
            Some(k) => Some(
                SymbolKind::parse(k).ok_or_else(|| invalid(tool, format!("unknown kind {k}")))?,
            ),
            None => None,
        };
        let path = path_arg(&args, "path", tool)?;
        let (workspace, map, note) = self.shared.map(ctx, tool).await?;
        let asked = name.clone();
        let found = query(map, move |m| {
            m.definitions(&asked, kind, path.as_deref(), MAX_DEFINITIONS)
        })
        .await?;
        if let Some(first) = found.matches.first() {
            let mut text = format!(
                "{} definition{} of {}:\n",
                found.matches.len(),
                if found.matches.len() == 1 { "" } else { "s" },
                name
            );
            for hit in &found.matches {
                text.push_str(&definition_line(hit));
                text.push('\n');
            }
            let what = format!("{} at {}:{}", first.qualified(), first.path, first.line);
            show(
                ctx,
                &workspace,
                &first.path,
                first.line,
                first.end_line,
                &what,
            );
            text.push_str(&format!("The code pane shows {what}. {SHOWN_TO_USER}"));
            return Ok(ToolOutput {
                text_for_model: with_note(text, note),
                summary_for_ui: what,
                detail: Some(json!({ "matches": found.matches })),
            });
        }
        let mut text = format!("The code map has no definition named {name}.");
        if !found.similar.is_empty() {
            text.push_str(" Close names:\n");
            for hit in &found.similar {
                text.push_str(&definition_line(hit));
                text.push('\n');
            }
        } else {
            text.push(' ');
        }
        text.push_str(COVERS);
        Ok(ToolOutput {
            text_for_model: with_note(text, note),
            summary_for_ui: format!("No definition of {name}"),
            detail: Some(json!({ "matches": [], "similar": found.similar })),
        })
    }
}

// ---- code_references --------------------------------------------------------

struct ReferencesTool {
    shared: Arc<Shared>,
}

#[async_trait]
impl HostTool for ReferencesTool {
    fn name(&self) -> &str {
        CODE_REFERENCES
    }
    fn label(&self) -> &str {
        "Find uses"
    }
    fn label_template(&self) -> &str {
        "Finding who uses {name}"
    }
    fn description(&self) -> &str {
        "Find who calls or uses a symbol in this project, from Shodh's code map: each use as \
         path:line with the function it is in and how it was resolved to the definition (in \
         the defining file, imported, through its module path, its type or Self, in the same \
         module). Uses of the same name it could not resolve (such as x.open() on a value, or \
         Vec::new() for Store::new) are listed apart, by name only. Every use of the name is \
         looked at; the nearest are listed first, with how many there are in all. name as for \
         code_definition; path keeps the definition to a file or folder; within keeps the uses \
         to a file or folder. Prefer this to grep for who calls X. Shows the first use in the \
         code pane. This changes nothing."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 300},
                "path": {"type": "string", "minLength": 1, "maxLength": 1024},
                "within": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = CODE_REFERENCES;
        let name = name_arg(&args, "name", tool)?.to_string();
        let path = path_arg(&args, "path", tool)?;
        let within = path_arg(&args, "within", tool)?;
        let (workspace, map, note) = self.shared.map(ctx, tool).await?;
        let asked = name.clone();
        let scope = within.clone();
        let found = query(map, move |m| {
            m.references(&asked, path.as_deref(), scope.as_deref(), MAX_USES)
        })
        .await?;
        let mut text = String::new();
        match found.definitions.as_slice() {
            [] => text.push_str(&format!(
                "The code map has no definition named {name}, so no use is resolved to one.\n"
            )),
            [one] => text.push_str(&format!(
                "{} {} is defined at {}:{}.\n",
                one.kind,
                one.qualified(),
                one.path,
                one.line
            )),
            many => {
                text.push_str(&format!(
                    "{name} has {} definitions{}:\n",
                    found.definitions_total,
                    if found.definitions_total > many.len() {
                        format!(" (uses are resolved to the first {})", many.len())
                    } else {
                        String::new()
                    }
                ));
                for hit in many.iter().take(MAX_DEFINITIONS) {
                    text.push_str(&definition_line(hit));
                    text.push('\n');
                }
            }
        }
        let where_ = within
            .as_deref()
            .map(|w| format!(" in {w}"))
            .unwrap_or_default();
        text.push_str(&format!(
            "The map has {} use{} named {name}{where_}.\n",
            grouped(found.uses_named),
            if found.uses_named == 1 { "" } else { "s" }
        ));
        let use_line = |r: &shodh_rag::code_map::RefHit| {
            let mut line = format!("- {}:{}", r.path, r.line);
            if let Some(caller) = &r.caller {
                line.push_str(&format!(" in {caller}"));
            }
            line.push_str(&format!(" ({}, {})", r.kind, r.how));
            line
        };
        let shown = |listed: usize, total: usize| {
            if listed < total {
                format!(
                    "{} of {} shown, nearest first",
                    grouped(listed as u64),
                    grouped(total as u64)
                )
            } else {
                "all shown".to_string()
            }
        };
        if found.resolved_total == 0 {
            text.push_str("No use is resolved to it.\n");
        } else {
            text.push_str(&format!(
                "{} use{} resolved to it through its file, an import, a path or Self ({}):\n",
                grouped(found.resolved_total as u64),
                if found.resolved_total == 1 { "" } else { "s" },
                shown(found.resolved.len(), found.resolved_total)
            ));
            for r in &found.resolved {
                text.push_str(&use_line(r));
                text.push('\n');
            }
        }
        if found.by_name_total > 0 {
            text.push_str(&format!(
                "{} use{} of the name could not be resolved to a definition, by name only, \
                 so they may be other symbols; check them ({}):\n",
                grouped(found.by_name_total as u64),
                if found.by_name_total == 1 { "" } else { "s" },
                shown(found.by_name.len(), found.by_name_total)
            ));
            for r in &found.by_name {
                text.push_str(&use_line(r));
                text.push('\n');
            }
        }
        if found.uses_scanned < found.uses_named {
            text.push_str(&format!(
                "Only the first {} uses were looked at; grep for the rest.\n",
                grouped(found.uses_scanned)
            ));
        }
        if found.truncated {
            text.push_str(
                "Not every use is listed: narrow it with within (a file or folder) or path (the \
                 definition's file), or grep for the rest.\n",
            );
        }
        let first = found
            .resolved
            .first()
            .map(|r| (r.path.clone(), r.line, r.line))
            .or_else(|| {
                found
                    .definitions
                    .first()
                    .map(|d| (d.path.clone(), d.line, d.end_line))
            });
        let summary = format!(
            "{} use{} of {name}",
            grouped(found.resolved_total as u64),
            if found.resolved_total == 1 { "" } else { "s" }
        );
        if let Some((path, line, end)) = first {
            let what = format!("{path}:{line}");
            show(ctx, &workspace, &path, line, end, &what);
            text.push_str(&format!("The code pane shows {what}. {SHOWN_TO_USER}"));
        } else {
            text.push_str(COVERS);
        }
        Ok(ToolOutput {
            text_for_model: with_note(text, note),
            summary_for_ui: summary,
            detail: Some(json!({
                "definitions": found.definitions,
                "resolved": found.resolved,
                "resolvedTotal": found.resolved_total,
                "byName": found.by_name,
                "byNameTotal": found.by_name_total,
                "usesNamed": found.uses_named,
                "truncated": found.truncated,
            })),
        })
    }
}

// ---- code_outline -----------------------------------------------------------

struct OutlineTool {
    shared: Arc<Shared>,
}

#[async_trait]
impl HostTool for OutlineTool {
    fn name(&self) -> &str {
        CODE_OUTLINE
    }
    fn label(&self) -> &str {
        "Outline"
    }
    fn label_template(&self) -> &str {
        "Listing the symbols of {path}"
    }
    fn description(&self) -> &str {
        "List the symbols of a file, a module (my_crate::store, src/lib) or the files directly \
         in a folder, from Shodh's code map: each function, method, type, trait, class, \
         interface, module and constant with its line and signature, members under their type. \
         Use it to see a file's shape before reading all of it. Shows the file in the code \
         pane. This changes nothing."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = CODE_OUTLINE;
        let target =
            path_arg(&args, "path", tool)?.ok_or_else(|| invalid(tool, "path is required"))?;
        let (workspace, map, note) = self.shared.map(ctx, tool).await?;
        let asked = target.clone();
        let outline = query(map, move |m| m.outline(&asked)).await?;
        let mut text = String::new();
        let mut count = 0usize;
        for file in &outline.files {
            text.push_str(&format!(
                "{} (module {}, {}, {} lines):\n",
                file.path, file.module, file.language, file.lines
            ));
            if let Some(why) = &file.skipped {
                text.push_str(&format!("  Not read: {why}.\n"));
            }
            if file.symbols.is_empty() && file.skipped.is_none() {
                text.push_str("  no symbols\n");
            }
            for symbol in &file.symbols {
                count += 1;
                let indent = if symbol.container.is_some() {
                    "    "
                } else {
                    "  "
                };
                let mut line = format!(
                    "{indent}- {} {} — line {}",
                    symbol.kind, symbol.name, symbol.line
                );
                if symbol.end_line > symbol.line {
                    line.push_str(&format!("–{}", symbol.end_line));
                }
                if !symbol.signature.is_empty() {
                    line.push_str(&format!(": {}", symbol.signature));
                }
                text.push_str(&line);
                text.push('\n');
            }
        }
        if outline.truncated {
            text.push_str("Only the first files are outlined; ask for one file for the rest.\n");
        }
        if let Some(first) = outline.files.first() {
            let what = format!("the outline of {}", first.path);
            show(ctx, &workspace, &first.path, 1, 1, &first.path);
            text.push_str(&format!(
                "The code pane shows {}. {SHOWN_TO_USER}",
                first.path
            ));
            return Ok(ToolOutput {
                text_for_model: with_note(text, note),
                summary_for_ui: format!(
                    "{count} symbol{} in {}",
                    if count == 1 { "" } else { "s" },
                    if outline.files.len() == 1 {
                        first.path.clone()
                    } else {
                        what
                    }
                ),
                detail: Some(json!({ "outline": outline })),
            });
        }
        Ok(ToolOutput {
            text_for_model: with_note(text, note),
            summary_for_ui: format!("No symbols in {target}"),
            detail: Some(json!({ "outline": outline })),
        })
    }
}

// ---- code_dependencies ------------------------------------------------------

struct DependenciesTool {
    shared: Arc<Shared>,
}

#[async_trait]
impl HostTool for DependenciesTool {
    fn name(&self) -> &str {
        CODE_DEPENDENCIES
    }
    fn label(&self) -> &str {
        "Dependencies"
    }
    fn label_template(&self) -> &str {
        "Reading the imports of {path}"
    }
    fn description(&self) -> &str {
        "Show what a module of this project imports (other modules of the project, each with \
         the path:line of its first import, and crates or packages from outside), what imports \
         it, and the import cycles it is in, from Shodh's code map. path is a file, a module \
         (my_crate::store, src/lib) or a folder. Rust modules follow mod declarations \
         (#[path] too), use and paths in code and macros; TypeScript follows relative imports, \
         tsconfig path aliases and re-exports. Shows the module's file in the code pane. This \
         changes nothing."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = CODE_DEPENDENCIES;
        let target =
            path_arg(&args, "path", tool)?.ok_or_else(|| invalid(tool, "path is required"))?;
        let (workspace, map, note) = self.shared.map(ctx, tool).await?;
        let asked = target.clone();
        let all = query(map, move |m| m.dependencies(&asked)).await?;
        let mut text = String::new();
        for deps in &all {
            text.push_str(&format!(
                "Module {} ({}):\n",
                deps.module,
                deps.files.join(", ")
            ));
            if deps.imports.is_empty() {
                text.push_str("Imports no module of the project.\n");
            } else {
                text.push_str("Imports:\n");
                for e in &deps.imports {
                    text.push_str(&format!(
                        "- {} (first at {}:{})\n",
                        e.module, e.path, e.line
                    ));
                }
            }
            if deps.imported_by.is_empty() {
                text.push_str("Imported by no module of the project.\n");
            } else {
                text.push_str("Imported by:\n");
                for e in &deps.imported_by {
                    text.push_str(&format!(
                        "- {} (first at {}:{})\n",
                        e.module, e.path, e.line
                    ));
                }
            }
            if !deps.external.is_empty() {
                text.push_str(&format!(
                    "From outside the project: {}\n",
                    deps.external.join(", ")
                ));
            }
            if deps.cycles.is_empty() {
                text.push_str("In no import cycle.\n");
            } else {
                for group in &deps.cycles {
                    text.push_str(&format!("In an import cycle: {}\n", group.join(" ↔ ")));
                }
            }
            text.push('\n');
        }
        let Some(first) = all.first() else {
            return Err(ToolError::NotFound(format!(
                "The code map has no module {target}."
            )));
        };
        let summary = format!(
            "{}: imports {}, imported by {}{}",
            first.module,
            first.imports.len(),
            first.imported_by.len(),
            if first.cycles.is_empty() {
                ""
            } else {
                ", in a cycle"
            }
        );
        if let Some(file) = first.files.first() {
            let line = first
                .imports
                .iter()
                .filter(|e| &e.path == file)
                .map(|e| e.line)
                .min()
                .unwrap_or(1);
            show(ctx, &workspace, file, line, line, &format!("{file}:{line}"));
            text.push_str(&format!(
                "The code pane shows {file}:{line}. {SHOWN_TO_USER}"
            ));
        }
        Ok(ToolOutput {
            text_for_model: with_note(text, note),
            summary_for_ui: summary,
            detail: Some(json!({ "modules": all })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(PathBuf);

    #[async_trait]
    impl CodeFolderOf for Fixed {
        async fn folder(
            &self,
            _ctx: &ToolContext,
            _tool: &str,
        ) -> Result<(String, PathBuf), ToolError> {
            Ok(("ws-1".to_string(), self.0.clone()))
        }
    }

    fn write(root: &std::path::Path, path: &str, text: &str) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"shop\"\n");
        write(root, "src/lib.rs", "pub mod cart;\npub mod checkout;\n");
        write(
            root,
            "src/cart.rs",
            "use crate::checkout::pay;\npub struct Cart;\nimpl Cart {\n    pub fn total(&self) -> u32 { 0 }\n}\npub fn empty() -> Cart { Cart }\n",
        );
        write(
            root,
            "src/checkout.rs",
            "use crate::cart::{self, Cart};\npub fn pay(c: &Cart) -> u32 {\n    let _ = cart::empty();\n    c.total()\n}\n",
        );
        write(root, ".env", "KEY=secret\n");
        dir
    }

    fn tools_for(root: &std::path::Path, data: &std::path::Path) -> Vec<Arc<dyn HostTool>> {
        code_map_tools(
            CodeMapState::new(data.to_path_buf(), None),
            Arc::new(Fixed(root.to_path_buf())),
        )
    }

    fn ctx() -> (
        ToolContext,
        tokio::sync::mpsc::UnboundedReceiver<shodh_rag::harness::events::AgentEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (ToolContext::new("r", "s", tx), rx)
    }

    /// The code pane targets the tool showed.
    fn shown(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<shodh_rag::harness::events::AgentEvent>,
    ) -> Vec<(String, u32)> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let shodh_rag::harness::events::AgentEvent::Navigated {
                target:
                    Some(NavigationTarget::Code {
                        path: Some(path),
                        line: Some(line),
                        view,
                        ..
                    }),
                ..
            } = event
            {
                assert_eq!(view.as_deref(), Some("file"));
                out.push((path, line));
            }
        }
        out
    }

    #[tokio::test]
    async fn the_tools_are_read_only_and_answer_with_receipts_in_the_code_pane() {
        let project = project();
        let data = tempfile::tempdir().unwrap();
        let tools = tools_for(project.path(), data.path());
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, CODE_MAP_TOOL_NAMES);
        for tool in &tools {
            assert_eq!(tool.tier(), RiskTier::Read);
            assert_eq!(tool.schema()["additionalProperties"], false);
        }
        let (ctx, mut rx) = ctx();

        let out = tools[0]
            .execute(json!({"name": "Cart::total"}), &ctx)
            .await
            .unwrap();
        assert!(
            out.text_for_model.contains("- method Cart::total — src/cart.rs:4 (module shop::cart): pub fn total(&self) -> u32"),
            "{}",
            out.text_for_model
        );
        assert_eq!(out.summary_for_ui, "Cart::total at src/cart.rs:4");
        assert_eq!(shown(&mut rx), vec![("src/cart.rs".to_string(), 4)]);

        let missing = tools[0]
            .execute(json!({"name": "Nowhere"}), &ctx)
            .await
            .unwrap();
        assert!(missing
            .text_for_model
            .starts_with("The code map has no definition named Nowhere."));
        assert!(
            shown(&mut rx).is_empty(),
            "nothing is shown for nothing found"
        );

        let uses = tools[1]
            .execute(json!({"name": "empty"}), &ctx)
            .await
            .unwrap();
        assert!(
            uses.text_for_model
                .contains("function empty is defined at src/cart.rs:6."),
            "{}",
            uses.text_for_model
        );
        assert!(uses
            .text_for_model
            .contains("- src/checkout.rs:3 in pay (call, through its module path)"));
        assert_eq!(uses.summary_for_ui, "1 use of empty");
        assert_eq!(shown(&mut rx), vec![("src/checkout.rs".to_string(), 3)]);

        let outline = tools[2]
            .execute(json!({"path": "src/cart.rs"}), &ctx)
            .await
            .unwrap();
        assert!(outline
            .text_for_model
            .starts_with("src/cart.rs (module shop::cart, rust, 6 lines):"));
        assert!(outline
            .text_for_model
            .contains("    - method total — line 4: pub fn total(&self) -> u32"));
        assert_eq!(outline.summary_for_ui, "3 symbols in src/cart.rs");
        assert_eq!(shown(&mut rx), vec![("src/cart.rs".to_string(), 1)]);

        let deps = tools[3]
            .execute(json!({"path": "shop::checkout"}), &ctx)
            .await
            .unwrap();
        assert!(
            deps.text_for_model
                .contains("- shop::cart (first at src/checkout.rs:1)"),
            "{}",
            deps.text_for_model
        );
        assert!(deps
            .text_for_model
            .contains("In an import cycle: shop::cart ↔ shop::checkout"));
        assert_eq!(
            deps.summary_for_ui,
            "shop::checkout: imports 1, imported by 1, in a cycle"
        );
        assert_eq!(shown(&mut rx), vec![("src/checkout.rs".to_string(), 1)]);
    }

    #[tokio::test]
    async fn the_tools_see_edits_at_once_and_never_secrets_or_paths_outside() {
        let project = project();
        let data = tempfile::tempdir().unwrap();
        let tools = tools_for(project.path(), data.path());
        let (ctx, _rx) = ctx();
        assert!(tools[0]
            .execute(json!({"name": "refund"}), &ctx)
            .await
            .unwrap()
            .text_for_model
            .contains("no definition"));
        write(project.path(), "src/checkout.rs", "pub fn refund() {}\n");
        let out = tools[0]
            .execute(json!({"name": "refund"}), &ctx)
            .await
            .unwrap();
        assert!(
            out.text_for_model.contains("src/checkout.rs:1"),
            "an edit is seen by the next call"
        );
        let secret = tools[0]
            .execute(json!({"name": "KEY"}), &ctx)
            .await
            .unwrap();
        assert!(secret.text_for_model.contains("no definition"));
        assert!(tools[2]
            .execute(json!({"path": "../outside.rs"}), &ctx)
            .await
            .is_err());
        assert!(tools[2]
            .execute(json!({"path": ".env"}), &ctx)
            .await
            .is_err());
        assert!(tools[0]
            .execute(json!({"name": "x", "kind": "nope"}), &ctx)
            .await
            .is_err());
    }
}
