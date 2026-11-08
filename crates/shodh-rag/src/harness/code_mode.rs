//! Code mode: one conversation's omp session runs with omp's own coding
//! tools, confined to the workspace's code folder.
//!
//! What omp 18.4.10 offers and how it is used (verified against the pinned
//! binary and its bundled docs: `omp://approval-mode.md`, `omp://rpc.md`,
//! `omp://hooks.md`):
//! - `--tools` selects the built-in tools; the session's tool inventory is
//!   checked after start ([`CODE_TOOLS`], exactly).
//! - omp has no setting that keeps file tools inside a folder: `read ..\x`
//!   and absolute paths work. Confinement is a `tool_call` hook
//!   ([`GUARD_HOOK`], loaded with `--hook`) that blocks any path resolving
//!   outside the folder, every URL and internal resource, and every tool
//!   that is not a Code mode tool. The session refuses to start unless the
//!   hook announced itself ([`GUARD_COMMAND`]).
//! - The hook asks for approval of every edit and write through omp's
//!   dialog channel (an `input` request titled [`APPROVAL_TITLE`]); the
//!   session maps it to `ApprovalRequested` and answers with
//!   [`ALLOW_ANSWER`] or the reason it was refused.
//! - omp's own shell and every other omp tool that runs programs are off
//!   ([`OMP_EXECUTION_TOOLS`]). Commands run through Shodh's `run_command`
//!   host tool ([`super::run_command`]): its own process with a clean
//!   environment and limits, decided by the same approval path as edits.
//!   Commands cannot be confined by path; each one needs approval (or the
//!   allowlist) and destructive ones carry a warning ([`destructive_reason`]).
//! - Undo: before the first change of a session in a git repository the
//!   session switches to a new branch `shodh/<id>` ([`plan_change`],
//!   [`create_branch`]); "discard changes" commits the work on that branch
//!   and returns to the original one ([`discard_changes`]). Files git
//!   ignores are not in that commit, so changes to them always ask
//!   ([`change_target`]).
//! - Files that can run programs by themselves or through a build or test
//!   command (`.vscode/`, `.envrc`, `package.json`, `build.rs`,
//!   `conftest.py`, ...: [`execution_risk`]) always ask, with a warning;
//!   the guard refuses edits and writes in `.git/`.
//! - A command a test or build runs can run any file of the folder, so the
//!   files that changed without the user seeing the change (edits applied
//!   without asking, cut previews, what commands and MCP tools wrote) are
//!   listed per conversation, and allowlisted commands ask until the user
//!   approves one whose prompt showed every listed file ([`ReviewState`],
//!   [`tree_snapshot`]).
//! - Shodh's own git commands run without hooks, the file system monitor or
//!   content filters ([`git`], [`run_git`]), so files in the folder cannot
//!   make them run programs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::error::HarnessError;
use super::events::RiskTier;
use super::modes::{self, AgentMode, CommandGate, Gate};
use super::omp::StepMeta;
use super::profile::DIAGRAM_GUIDE;
use super::sidecar::Thinking;
use super::truncate_chars;

/// omp's own tools a Code session has, exactly (omp names). `find` in omp
/// 18.4.10 is `glob`; `lsp` needs language servers in the isolated home and
/// is off. Commands run through Shodh's `run_command` host tool, never omp's
/// shell.
pub const CODE_TOOLS: [&str; 6] = ["read", "grep", "glob", "ast_grep", "edit", "write"];

/// omp tools that change files; the guard asks for each call.
pub const CHANGE_TOOLS: [&str; 2] = ["edit", "write"];

/// omp's tools a Code session has in a mode that only reads
/// ([`super::modes::AgentMode::Plan`]): [`CODE_TOOLS`] without
/// [`CHANGE_TOOLS`], and no `run_command`.
pub const READ_TOOLS: [&str; 4] = ["read", "grep", "glob", "ast_grep"];

/// omp's tools of a Code session that only reads (`reads_only`) or not.
pub fn omp_tools(reads_only: bool) -> &'static [&'static str] {
    if reads_only {
        &READ_TOOLS
    } else {
        &CODE_TOOLS
    }
}

/// omp's tools that run programs or other agents (omp 18.4.10's
/// `BUILTIN_TOOL_NAMES` and its extra tools). A Code session must offer none
/// of them, whatever its host tools are called: its inventory check fails
/// closed on any of these names ([`inventory_matches`]), and the guard
/// blocks them.
pub const OMP_EXECUTION_TOOLS: [&str; 10] = [
    "bash",
    "eval",
    "run_code",
    "debug",
    "ida",
    "task",
    "computer",
    "browser",
    "vibe_spawn",
    "vibe_send",
];

/// How many tools a Code session has besides the host tools of its chat:
/// omp's [`CODE_TOOLS`] and Shodh's `run_command`.
pub const CODE_BUILTIN_TOOLS: usize = CODE_TOOLS.len() + 1;

/// Names a Code session's other host tools (MCP servers, skills) cannot
/// take: omp's Code tools, Shodh's `run_command`, and omp's tools that run
/// programs ([`OMP_EXECUTION_TOOLS`]), so none can stand in for them.
pub fn reserved_tool_names() -> Vec<&'static str> {
    CODE_TOOLS
        .iter()
        .copied()
        .chain(std::iter::once(super::run_command::RUN_COMMAND))
        .chain(OMP_EXECUTION_TOOLS)
        .collect()
}

/// The guard hook (an omp extension module).
pub const GUARD_HOOK: &str = include_str!("code_guard.js");

/// The command the guard registers; its presence proves the guard loaded.
pub const GUARD_COMMAND: &str = "shodh-guard";

/// Title of the guard's approval request.
pub const APPROVAL_TITLE: &str = "shodh-approval";

/// The answer that lets a guarded call run; any other answer is the reason
/// it was refused.
pub const ALLOW_ANSWER: &str = "allow";

/// Environment variable carrying the code folder to the guard.
pub const CODE_ROOT_ENV: &str = "SHODH_CODE_ROOT";

/// Environment variable telling the guard the session only reads (`1`): it
/// blocks every edit and write without asking.
pub const READS_ONLY_ENV: &str = "SHODH_READS_ONLY";

/// Pins omp's edit tool to single-file replace edits (`path`, `old_string`,
/// `new_string`), whose target the guard can check. The default hashline
/// patch format can name several files and move them.
pub const EDIT_VARIANT_ENV: (&str, &str) = ("PI_EDIT_VARIANT", "replace");

/// Prefix of the branches Code mode creates.
pub const BRANCH_PREFIX: &str = "shodh/";

pub const MAX_PREVIEW_CHARS: usize = 2_000;
const MAX_LABEL_CHARS: usize = 80;

/// Discovery sources whose project files (settings, hooks, MCP servers,
/// context files) are ignored in Code sessions.
pub(crate) const DISABLED_DISCOVERY: [&str; 10] = [
    "native",
    "agents",
    "agents-md",
    "claude",
    "codex",
    "cursor",
    "gemini",
    "github",
    "opencode",
    "windsurf",
];

/// The workspace folder a Code session works in: an existing directory,
/// fully resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeFolder {
    root: PathBuf,
}

impl CodeFolder {
    /// Resolve `path` (symlinks and junctions included). Fails unless it is
    /// an existing directory.
    pub fn open(path: &Path) -> Result<Self, HarnessError> {
        let resolved = std::fs::canonicalize(path).map_err(|e| {
            HarnessError::CodeFolder(format!("{} cannot be opened ({e})", path.display()))
        })?;
        if !resolved.is_dir() {
            return Err(HarnessError::CodeFolder(format!(
                "{} is not a folder",
                path.display()
            )));
        }
        Ok(Self {
            root: strip_verbatim(resolved),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder as the UI and the audit log show it.
    pub fn display(&self) -> String {
        self.root.display().to_string()
    }
}

/// `\\?\C:\x` → `C:\x` (Windows `canonicalize` output that omp, git and
/// shells do not all accept). UNC verbatim paths are kept.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// Code mode's own settings (see [`code_overlay_config`]).
fn code_settings() -> Value {
    // The guard is the approval gate; omp's own prompts would ask twice.
    let approvals: serde_json::Map<String, Value> = CODE_TOOLS
        .iter()
        .map(|tool| (tool.to_string(), json!("allow")))
        .collect();
    json!({
        "tools": { "approvalMode": "always-ask", "approval": approvals },
        // No fallback models: a folder's chains (merged entry by entry into
        // any the overlays set) could route the session to another model.
        "retry": { "fallbackChains": null },
        "edit": { "mode": "replace" },
        "grep": { "enabled": true },
        "glob": { "enabled": true },
        "astGrep": { "enabled": true },
        // omp's shell is off: commands run through Shodh's `run_command`.
        // No background jobs or services either.
        "async": { "enabled": false },
        "launch": { "enabled": false },
        "bash": { "enabled": false, "patterns": [], "autoBackground": { "enabled": false } },
        "bashInterceptor": { "enabled": false },
        "lsp": { "enabled": false },
        "workspace": { "additionalDirectories": [] },
        "enabledProviders": [],
        "disabledProviders": DISABLED_DISCOVERY,
    })
}

/// The second `--config` overlay of a Code session (written as JSON, which
/// YAML accepts).
///
/// Overlays outrank the code folder's own `.omp/settings.json` and
/// `.omp/config.yml`, but only for the keys they set: omp 18.4.10 reads a
/// folder's settings before it applies `disabledProviders`, and the folder
/// supplies every other key (shell path, extensions, MCP, approval of other
/// tiers...). So every setting omp has is pinned here. `settings` is omp's
/// `config list --json` (each key with its `value`, if set, and `type`);
/// each key takes Code mode's value, else the base overlay's
/// ([`super::sidecar::overlay_config`]), else omp's own. Unset keys are
/// pinned to null, which keeps a folder's value out. Records (maps, which omp
/// merges entry by entry across layers) are pinned to null as well, which omp
/// reads as empty: model roles too, which omp also reads straight from the
/// folder's `.omp/config.yml` (into the same layer), and fallback chains. The
/// one record kept is the tool approvals Code mode sets: without them omp
/// would ask about every edit, write and command itself, and its own prompts
/// are declined. A folder can add approval entries for other names; omp
/// consults those only for tools a Code session does not have (its tool list
/// is exact) and for `xd://` device writes, which the guard blocks like every
/// internal URL.
pub fn code_overlay_config(settings: &serde_json::Map<String, Value>) -> Value {
    let mut pinned: std::collections::BTreeMap<String, Value> = settings
        .iter()
        .map(|(key, info)| {
            let value = match info.get("type").and_then(Value::as_str) {
                Some("record") => Value::Null,
                _ => info.get("value").cloned().unwrap_or(Value::Null),
            };
            (key.clone(), value)
        })
        .collect();
    for layer in [super::sidecar::overlay_config(), code_settings()] {
        flatten_settings(&layer, "", settings, &mut pinned);
    }
    let mut overlay = serde_json::Map::new();
    for (key, value) in pinned {
        insert_path(&mut overlay, &key, value);
    }
    Value::Object(overlay)
}

/// Adds the leaves of `value` to `out` as dotted keys. A key omp knows is a
/// leaf whatever its value (a record's entries are its value).
fn flatten_settings(
    value: &Value,
    prefix: &str,
    known: &serde_json::Map<String, Value>,
    out: &mut std::collections::BTreeMap<String, Value>,
) {
    match value {
        Value::Object(map) if prefix.is_empty() || !known.contains_key(prefix) => {
            for (name, child) in map {
                let key = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                flatten_settings(child, &key, known, out);
            }
        }
        _ => {
            out.insert(prefix.to_string(), value.clone());
        }
    }
}

/// Sets `value` at the dotted `key` in nested maps.
fn insert_path(map: &mut serde_json::Map<String, Value>, key: &str, value: Value) {
    match key.split_once('.') {
        None => {
            map.insert(key.to_string(), value);
        }
        Some((head, rest)) => {
            let child = map
                .entry(head.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if !child.is_object() {
                *child = Value::Object(serde_json::Map::new());
            }
            if let Value::Object(child) = child {
                insert_path(child, rest, value);
            }
        }
    }
}

/// Launch flags of a Code session. Unlike Research sessions, omp's dialog
/// channel stays on (no `--no-ui`): approvals travel over it. Reasoning
/// follows the same setting as Research sessions ([`Thinking`]). A session
/// that only reads (`reads_only`) gets only omp's reading tools.
pub fn code_launch_args(
    model_arg: &str,
    base_overlay: &Path,
    code_overlay: &Path,
    guard_hook: &Path,
    system_prompt: &str,
    thinking: Thinking,
    reads_only: bool,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "--mode",
        "rpc",
        "--no-extensions",
        "--no-skills",
        "--no-rules",
        "--no-lsp",
        "--no-pty",
        "--no-session",
        "--thinking",
        thinking.flag(),
        "--approval-mode=always-ask",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(format!("--tools={}", omp_tools(reads_only).join(",")));
    args.push(format!("--hook={}", guard_hook.display()));
    args.push(format!("--model={model_arg}"));
    args.push(format!("--config={}", base_overlay.display()));
    args.push(format!("--config={}", code_overlay.display()));
    args.push(format!("--system-prompt={system_prompt}"));
    args
}

/// Diagrams of code: files the reader can open, and architecture (and what a
/// change did to it) from enola's facts when it is connected.
const CODE_DIAGRAMS: &str = "\n\
In Code mode give nodes that are files or folders a \"path\" relative to the code folder, so the \
reader can open them (Shodh checks that each exists).\n\
For the architecture of the code, when the enola tools are available: read the modules and their \
dependencies with them (explore, query_facts, traverse), then draw an architecture diagram whose \
nodes are modules or folders with their path, grouped and named by you, and whose edges are \
dependencies enola reported. For what a change did to the architecture: when the user wants that, \
pin a baseline before the first change (generate_snapshot, then set_baseline); after the change \
run generate_snapshot and diff_snapshot, and draw one architecture diagram marking what it reports \
with \"change\". If no baseline was pinned, say that the comparison is not available instead of \
guessing. Use only what enola reported.";

/// The system prompt of a Code session. `instructions` are the
/// conversation's own (they never override the rules).
pub fn code_system_prompt(
    folder: &CodeFolder,
    instructions: Option<&str>,
    mode: AgentMode,
) -> String {
    let mut prompt = if mode.creates() {
        format!(
            "You are Shodh's builder, making what the user asks for in its own new creation \
             folder {folder}.\n\
             \n\
             Tools: read (files), grep (text search), glob (find files), ast_grep (syntax-aware \
             search), edit (replace old_string with new_string in one file), write (create or \
             replace a whole file), run_command (run one bash command in the creation folder, in \
             its own process with a time limit; anything it starts in the background is stopped \
             when it ends) and finish_creation (record what you built and the choices you made, \
             once, at the end).\n\
             \n\
             Rules:\n\
             - Work only inside the creation folder. Paths outside it, URLs and other resources \
             are blocked.\n\
             - Build the whole thing, not a sketch: every file it needs, no placeholders, no \
             TODOs. Anything a browser shows starts at index.html at the top of the folder and \
             loads its scripts, styles and images by relative paths, so Shodh's preview can show \
             it; it must work offline unless the user asked for something online.\n\
             - Prefer write for new files and edit for changes to them.\n\
             - If the user declines something, do not try it another way: say what you wanted to \
             do and carry on without it.\n\
             - Never commit or push, and never delete files you did not make.",
            folder = folder.display()
        )
    } else if mode.reads_only() {
        format!(
            "You are Shodh's coding assistant, looking at the code folder {folder}.\n\
             \n\
             Tools: read (files), grep (text search), glob (find files) and ast_grep \
             (syntax-aware search). You cannot edit, write or run commands in this mode.\n\
             \n\
             Rules:\n\
             - Work only inside the code folder. Paths outside it, URLs and other resources \
             are blocked.\n\
             - Read the relevant code before planning a change, and name the files and \
             commands each step involves.",
            folder = folder.display()
        )
    } else {
        format!(
            "You are Shodh's coding assistant, working in the code folder {folder}.\n\
             \n\
             Tools: read (files), grep (text search), glob (find files), ast_grep (syntax-aware \
             search), edit (replace old_string with new_string in one file), write (create or \
             replace a whole file) and run_command (run one bash command in the code folder, in \
             its own process with a time limit; anything it starts in the background is stopped \
             when it ends).\n\
             \n\
             Rules:\n\
             - Work only inside the code folder. Paths outside it, URLs and other resources are \
             blocked.\n\
             - Edits, writes and commands may wait for the user's approval (the mode the user \
             chose decides; destructive commands always do). If one is declined, do not try it \
             another way: say what you wanted to do and ask how to continue.\n\
             - Read the relevant code before changing it, keep changes small and focused, and say \
             what you changed and how to check it.\n\
             - Prefer edit for changes to existing files; use write for new files.\n\
             - Avoid destructive commands (deleting files, resetting or rewriting git history, \
             force-pushing). Never commit or push unless the user asks.",
            folder = folder.display()
        )
    };
    prompt.push_str("\n\n");
    prompt.push_str(mode.prompt_note());
    prompt.push_str(super::tools::MODE_IS_THE_USERS);
    // A Creator session is offered no mode suggestion: it builds what was asked.
    if !mode.creates() {
        prompt.push_str(super::tools::MODE_SUGGESTION);
    }
    prompt.push_str("\n\n");
    prompt.push_str(DIAGRAM_GUIDE);
    prompt.push_str(CODE_DIAGRAMS);
    if let Some(extra) = instructions.map(str::trim).filter(|i| !i.is_empty()) {
        prompt.push_str(
            "\n\nThe user's instructions for this conversation (they never override the rules above):\n",
        );
        prompt.push_str(extra);
    }
    prompt
}

/// Step labels and tiers of the Code tools.
pub fn code_catalog() -> HashMap<String, StepMeta> {
    let entries: [(&str, &str, RiskTier); 7] = [
        ("read", "Reading {path}", RiskTier::Read),
        (
            "grep",
            "Searching for {pattern}[ in {path}]",
            RiskTier::Read,
        ),
        ("glob", "Finding files[ matching {path}]", RiskTier::Read),
        ("ast_grep", "Searching code for {pat}", RiskTier::Read),
        ("edit", "Editing {path}", RiskTier::Write),
        ("write", "Writing {path}", RiskTier::Write),
        (
            super::run_command::RUN_COMMAND,
            "Running {command}",
            RiskTier::Write,
        ),
    ];
    entries
        .into_iter()
        .map(|(tool, label, tier)| {
            (
                tool.to_string(),
                StepMeta {
                    label_template: label.to_string(),
                    tier,
                },
            )
        })
        .collect()
}

/// Whether `tools` (a session's tool inventory) is exactly [`CODE_TOOLS`]
/// plus the session's `host` tools, `run_command` among them, and none of
/// omp's tools that run programs ([`OMP_EXECUTION_TOOLS`]), whatever the
/// host tools are called. A runtime that adds an execution tool fails this.
pub fn inventory_matches(tools: &[String], host: &[String]) -> bool {
    inventory_matches_in(tools, host, false)
}

/// [`inventory_matches`] for a session that only reads (`reads_only`):
/// exactly [`READ_TOOLS`] plus the host tools, and no `run_command`, neither
/// registered nor offered.
pub fn inventory_matches_in(tools: &[String], host: &[String], reads_only: bool) -> bool {
    let run_command = super::run_command::RUN_COMMAND;
    let registered = host.iter().any(|name| name == run_command);
    let offered = tools.iter().any(|name| name == run_command);
    let commands_fit = if reads_only {
        !registered && !offered
    } else {
        registered && offered
    };
    if !commands_fit
        || tools
            .iter()
            .chain(host)
            .any(|name| OMP_EXECUTION_TOOLS.contains(&name.as_str()))
    {
        return false;
    }
    let mut have: Vec<&str> = tools.iter().map(String::as_str).collect();
    have.sort_unstable();
    have.dedup();
    let mut want: Vec<&str> = omp_tools(reads_only)
        .iter()
        .copied()
        .chain(host.iter().map(String::as_str))
        .collect();
    want.sort_unstable();
    want.dedup();
    have.len() == tools.len() && have == want
}

/// Environment variable naming a Code session's host tools to the guard
/// (comma-separated).
pub const HOST_TOOLS_ENV: &str = "SHODH_HOST_TOOLS";

/// The overlay that lets omp run a Code session's `host` tools without
/// its own prompt: Shodh asks for them (per tool setting) and audits them.
/// Its content depends only on the names, so sessions share files safely.
pub fn host_tools_overlay_config(host: &[String]) -> Value {
    let approvals: serde_json::Map<String, Value> = host
        .iter()
        .map(|tool| (tool.clone(), json!("allow")))
        .collect();
    json!({ "tools": { "approval": approvals } })
}

/// A guarded call waiting for the user's decision: an edit or write omp's
/// guard asked about, or a `run_command` call.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardRequest {
    /// The dialog id omp expects the answer for; `None` for a `run_command`
    /// call, which the session answers with the tool's result.
    pub ui_id: Option<String>,
    pub tool_call_id: String,
    pub tool: String,
    pub input: Value,
    /// The file an edit or write lands in, resolved by the guard exactly as
    /// it checked it (`None` for commands, or when the guard sent none).
    pub target: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuardPayload {
    tool_call_id: String,
    tool: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    target: Option<String>,
}

/// The guard's approval request, if `title`/`payload` are one. Only change
/// tools are ever asked about.
pub fn parse_guard_request(
    ui_id: &str,
    title: Option<&str>,
    payload: Option<&str>,
) -> Option<GuardRequest> {
    if title != Some(APPROVAL_TITLE) {
        return None;
    }
    let parsed: GuardPayload = serde_json::from_str(payload?).ok()?;
    if parsed.tool_call_id.trim().is_empty() || !CHANGE_TOOLS.contains(&parsed.tool.as_str()) {
        return None;
    }
    Some(GuardRequest {
        ui_id: Some(ui_id.to_string()),
        tool_call_id: parsed.tool_call_id,
        tool: parsed.tool,
        input: parsed.input,
        target: parsed
            .target
            .filter(|t| !t.trim().is_empty())
            .map(PathBuf::from),
    })
}

// ── Approvals ──────────────────────────────────────────────────────────────
//
// How much runs without asking is the conversation's mode
// ([`super::modes::AgentMode`]), chosen by the user per conversation. A
// workspace adds its own list of commands Auto mode runs besides the ones its
// rules recognise ([`CodeSettings::allowlist`]). Files saved by earlier
// versions may still name a workspace approval level (`approval`); it is
// ignored, so such a workspace asks until the user picks a mode.

/// The commands a new workspace trusts.
pub const DEFAULT_ALLOWLIST: [&str; 3] = ["cargo test", "npm test", "pytest"];

/// Most allowlist entries kept.
pub const MAX_ALLOWLIST: usize = 32;

/// Longest allowlist entry.
pub const MAX_ALLOWLIST_CHARS: usize = 120;

/// Time limit of a command, in seconds: default and range.
pub const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 600;
pub const COMMAND_TIMEOUT_RANGE: std::ops::RangeInclusive<u64> = 10..=3_600;

/// Memory all of a command's processes may use together, in MB: default and
/// range.
pub const DEFAULT_MEMORY_LIMIT_MB: u64 = 4_096;
pub const MEMORY_LIMIT_RANGE: std::ops::RangeInclusive<u64> = 256..=65_536;

/// Share of the whole machine's CPU a command may use, in percent: default
/// and range.
pub const DEFAULT_CPU_PERCENT: u32 = 80;
pub const CPU_PERCENT_RANGE: std::ops::RangeInclusive<u32> = 10..=100;

/// A workspace's Code settings (`workspaces/<id>/code.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeSettings {
    /// Commands that run without asking in Auto mode besides the ones its
    /// rules recognise ([`super::modes::rules`]), each exactly as listed
    /// (words separated by any whitespace).
    pub allowlist: Vec<String>,
    /// Commands an earlier version kept on this workspace's list while the
    /// workspace asked for every command: kept for the user, never run without
    /// asking, until they move them back to [`CodeSettings::allowlist`].
    pub parked_allowlist: Vec<String>,
    /// The approval level an earlier version saved for this workspace
    /// (`askEveryTime`, `autoApplyEdits`, `trusted`), kept until the user has
    /// seen that each conversation's mode replaces it (a one-time notice).
    pub former_level: Option<String>,
    /// The approval level as an earlier version saved it: read, never
    /// written ([`CodeSettings::migrated`]).
    #[serde(rename = "approval", skip_serializing)]
    pub legacy_approval: Option<String>,
    /// The user trusts this workspace to run its own builds and tests without
    /// asking in Auto mode, as its build files were then ([`BuildTrust`]). Set
    /// only by the user's "Trust" in the workspace's Code settings, never by a
    /// save of the other settings.
    pub build_trust: Option<BuildTrust>,
    /// Longest a command may run, in seconds ([`COMMAND_TIMEOUT_RANGE`]); the
    /// model may ask for less, never more.
    pub command_timeout_secs: u64,
    /// Memory a command's processes may use together, in MB
    /// ([`MEMORY_LIMIT_RANGE`]).
    pub memory_limit_mb: u64,
    /// Share of the machine's CPU a command may use, in percent
    /// ([`CPU_PERCENT_RANGE`]).
    pub cpu_percent: u32,
}

impl Default for CodeSettings {
    fn default() -> Self {
        Self {
            allowlist: DEFAULT_ALLOWLIST.iter().map(|s| s.to_string()).collect(),
            parked_allowlist: Vec::new(),
            former_level: None,
            legacy_approval: None,
            build_trust: None,
            command_timeout_secs: DEFAULT_COMMAND_TIMEOUT_SECS,
            memory_limit_mb: DEFAULT_MEMORY_LIMIT_MB,
            cpu_percent: DEFAULT_CPU_PERCENT,
        }
    }
}

/// Characters that let a command do more than its first words say: command
/// separators, redirection, substitution, variables and line breaks.
const SHELL_SPECIAL: [char; 14] = [
    ';', '|', '&', '>', '<', '`', '$', '(', ')', '{', '}', '%', '\n', '\r',
];

fn has_shell_special(text: &str) -> bool {
    text.contains(SHELL_SPECIAL)
}

impl CodeSettings {
    /// Why the command limits cannot be saved, or `None` when each is in its
    /// range.
    pub fn limits_problem(&self) -> Option<String> {
        if !COMMAND_TIMEOUT_RANGE.contains(&self.command_timeout_secs) {
            return Some(format!(
                "The time limit must be between {} seconds and {} minutes.",
                COMMAND_TIMEOUT_RANGE.start(),
                COMMAND_TIMEOUT_RANGE.end() / 60
            ));
        }
        if !MEMORY_LIMIT_RANGE.contains(&self.memory_limit_mb) {
            return Some(format!(
                "The memory limit must be between {} MB and {} GB.",
                MEMORY_LIMIT_RANGE.start(),
                MEMORY_LIMIT_RANGE.end() / 1_024
            ));
        }
        if !CPU_PERCENT_RANGE.contains(&self.cpu_percent) {
            return Some(format!(
                "The CPU cap must be between {}% and {}%.",
                CPU_PERCENT_RANGE.start(),
                CPU_PERCENT_RANGE.end()
            ));
        }
        None
    }

    /// The settings with the allowlist cleaned (entries trimmed, with single
    /// spaces, without shell special characters or duplicates, capped) and
    /// the command limits brought into their ranges.
    /// The settings of a file an earlier version saved, as this version reads
    /// them: the workspace approval level is replaced by each conversation's
    /// mode. A list the workspace kept while it asked for every command was
    /// never used, so it is parked, not turned on; the level is kept for the
    /// one-time notice. A file this version saved is unchanged.
    pub fn migrated(mut self) -> Self {
        let Some(level) = self.legacy_approval.take() else {
            return self;
        };
        if level != "trusted" {
            let parked = std::mem::take(&mut self.allowlist);
            for entry in parked {
                if !self.parked_allowlist.contains(&entry) {
                    self.parked_allowlist.push(entry);
                }
            }
        }
        if self.former_level.is_none() {
            self.former_level = Some(level);
        }
        self
    }

    /// Whether the user's trust of this workspace's builds and tests holds for
    /// the project as it is now (`now`): `Ok` when it does, `Err` with why not.
    pub fn build_trust_for(&self, now: &ProjectFingerprint) -> Result<(), String> {
        match &self.build_trust {
            None => {
                Err("you have not trusted this workspace to run its builds and tests".to_string())
            }
            Some(trust) => match trust.fingerprint.first_change(now) {
                None => Ok(()),
                Some(file) => Err(format!(
                    "{file} changed since you trusted this workspace to run its builds and tests"
                )),
            },
        }
    }

    pub fn cleaned(mut self) -> Self {
        self.command_timeout_secs = self
            .command_timeout_secs
            .clamp(*COMMAND_TIMEOUT_RANGE.start(), *COMMAND_TIMEOUT_RANGE.end());
        self.memory_limit_mb = self
            .memory_limit_mb
            .clamp(*MEMORY_LIMIT_RANGE.start(), *MEMORY_LIMIT_RANGE.end());
        self.cpu_percent = self
            .cpu_percent
            .clamp(*CPU_PERCENT_RANGE.start(), *CPU_PERCENT_RANGE.end());
        let mut seen = std::collections::HashSet::new();
        self.allowlist = self
            .allowlist
            .iter()
            .map(|entry| entry.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|entry| {
                !entry.is_empty()
                    && entry.chars().count() <= MAX_ALLOWLIST_CHARS
                    && !has_shell_special(entry)
                    && !entry.split(' ').next().is_some_and(|w| w.contains('='))
            })
            .filter(|entry| seen.insert(entry.clone()))
            .take(MAX_ALLOWLIST)
            .collect();
        self
    }

    /// Whether `command` is one of the allowlisted commands: exactly an
    /// entry's words, with no shell special characters or leading variable
    /// assignment. Extra words are never allowed: flags such as
    /// `--config build.rustc-wrapper=<program>` or `--script-shell=<program>`
    /// make a test command run any program.
    pub fn allows(&self, command: &str) -> bool {
        // bash splits words on spaces and tabs only: any other blank (a
        // no-break space, an em space) would make bash read other words than
        // these.
        if has_shell_special(command)
            || command
                .chars()
                .any(|c| c.is_whitespace() && c != ' ' && c != '\t')
        {
            return false;
        }
        let words: Vec<&str> = command
            .split([' ', '\t'])
            .filter(|word| !word.is_empty())
            .collect();
        if words.is_empty() || words[0].contains('=') {
            return false;
        }
        self.allowlist.iter().any(|entry| {
            let listed: Vec<&str> = entry.split_whitespace().collect();
            !listed.is_empty() && !has_shell_special(entry) && words == listed
        })
    }

    /// The settings file of a workspace's data folder.
    pub fn path(workspace_dir: &Path) -> PathBuf {
        workspace_dir.join("code.json")
    }

    /// A workspace's settings; the defaults when it has none.
    pub fn load(workspace_dir: &Path) -> Result<Self, String> {
        let path = Self::path(workspace_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Self>(&text)
                .map(Self::migrated)
                .map(Self::cleaned)
                .map_err(|e| format!("{} is not readable: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{} is not readable: {e}", path.display())),
        }
    }

    /// Save the (cleaned) settings; returns what was saved. Command limits
    /// out of their ranges are refused, not changed.
    pub fn save(self, workspace_dir: &Path) -> Result<Self, String> {
        if let Some(problem) = self.limits_problem() {
            return Err(problem);
        }
        let settings = self.cleaned();
        let path = Self::path(workspace_dir);
        std::fs::create_dir_all(workspace_dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })?;
        Ok(settings)
    }
}

/// What Shodh knows about the file an edit or write lands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeTarget {
    /// A file inside the code folder that "Discard changes" can restore.
    Plain,
    /// A file whose change can run programs by itself ([`execution_risk`]);
    /// `untracked` when git also ignores it.
    RunsPrograms { risk: &'static str, untracked: bool },
    /// A file of the repository's `.git` folder: never changed.
    GitFolder,
    /// A `.gitignore`: it decides what "Discard changes" can restore.
    IgnoreRules,
    /// A file git ignores: "Discard changes" does not restore it.
    Untracked,
    /// The file is unknown or outside the code folder (always asks).
    Unknown,
}

impl ChangeTarget {
    /// Whether git ignores the file, so "Discard changes" cannot restore it.
    pub fn untracked(self) -> bool {
        matches!(
            self,
            ChangeTarget::Untracked
                | ChangeTarget::RunsPrograms {
                    untracked: true,
                    ..
                }
        )
    }
}

const GIT_FOLDER_RISK: &str = "Changes git's own files, which can make git run programs.";

/// The undo note of a change to a `.gitignore`.
pub const IGNORE_RULES_UNDO: &str = "This changes which files git ignores, so \"Discard changes\" \
     may not restore every file this conversation changed.";

/// Why changing the file at `relative` (a path inside the code folder) can
/// run programs without a command being approved, or `None`. Names are
/// compared without case, the way Windows and macOS find files.
pub fn execution_risk(relative: &Path) -> Option<&'static str> {
    let parts: Vec<String> = relative
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    let name = parts.last().map(String::as_str).unwrap_or("");
    let parent = parts
        .len()
        .checked_sub(2)
        .and_then(|i| parts.get(i))
        .map(String::as_str);
    if parts.iter().any(|p| p == ".git") {
        return Some(GIT_FOLDER_RISK);
    }
    if parts.iter().any(|p| p == ".vscode") {
        return Some("Editor settings and tasks can run commands when the folder is opened.");
    }
    if parts.iter().any(|p| p == ".husky" || p == ".githooks") {
        return Some("git hooks run programs on commits, checkouts and pushes.");
    }
    if parts.len() >= 3 && parts[0] == ".github" && parts[1] == "workflows" {
        return Some("CI workflows run commands when the change is pushed.");
    }
    if parts.iter().any(|p| p == ".yarn") {
        return Some("yarn runs the release and plugins kept here with every yarn command.");
    }
    if parts.iter().any(|p| p == ".mvn") {
        return Some("Maven settings and extensions here run with every Maven command.");
    }
    if parent == Some(".cargo") && (name == "config" || name == "config.toml") {
        return Some("Cargo settings can run programs with every cargo command.");
    }
    RUNS_PROGRAMS
        .iter()
        .find(|(patterns, _)| patterns.iter().any(|pattern| pattern.matches(name)))
        .map(|(_, risk)| *risk)
}

/// How [`RUNS_PROGRAMS`] names a file (lowercase, compared with the
/// lowercased file name).
enum FileName {
    /// Exactly this name.
    Is(&'static str),
    /// A name with this ending (dot included).
    EndsWith(&'static str),
    /// A name with this beginning, such as `jest.config.` for every
    /// `jest.config.*`.
    StartsWith(&'static str),
}

impl FileName {
    fn matches(&self, name: &str) -> bool {
        match self {
            FileName::Is(exact) => name == *exact,
            FileName::EndsWith(end) => name.ends_with(end),
            FileName::StartsWith(start) => name.starts_with(start),
        }
    }
}

/// Files, wherever they are in the code folder, whose change can run
/// programs by itself or when a build or test command runs (the allowlist's
/// `cargo test`, `npm test` and `pytest` included), with the reason the
/// prompt shows. Folder rules (`.git`, `.vscode`, hooks folders, CI
/// workflows, `.cargo/config`) are in [`execution_risk`]; every file rule is
/// here.
const RUNS_PROGRAMS: &[(&[FileName], &str)] = {
    use FileName::{EndsWith, Is, StartsWith};
    &[
        (
            &[Is(".envrc")],
            "direnv runs .envrc in the shell before each command.",
        ),
        (
            &[Is("mcp-arch.yaml"), Is("mcp-arch.yml")],
            "enola runs the programs mcp-arch.yaml names when it builds a snapshot.",
        ),
        (
            &[Is("package.json")],
            "Its scripts run with npm commands, tests included.",
        ),
        (
            &[Is(".npmrc")],
            "npm settings can choose the program that runs scripts.",
        ),
        (
            &[Is(".yarnrc"), Is(".yarnrc.yml"), Is(".pnpmfile.cjs")],
            "Yarn and pnpm settings can choose code that runs with every install and script.",
        ),
        (
            &[
                StartsWith("jest.config."),
                StartsWith("vitest.config."),
                StartsWith("vitest.workspace."),
                StartsWith(".mocharc."),
            ],
            "The test runner loads the code this file names when the tests run.",
        ),
        (
            &[Is(".pre-commit-config.yaml"), Is(".pre-commit-config.yml")],
            "pre-commit runs the hooks this file names on every commit.",
        ),
        (
            &[
                Is("lefthook.yml"),
                Is("lefthook.yaml"),
                Is(".lefthook.yml"),
                Is(".lefthook.yaml"),
                Is("lefthook-local.yml"),
                Is("lefthook-local.yaml"),
            ],
            "lefthook runs the commands this file names on git events.",
        ),
        (
            &[Is("build.rs")],
            "Cargo compiles and runs build.rs with every build, tests included.",
        ),
        (
            &[Is("cargo.toml")],
            "Cargo.toml chooses the build scripts, macros and dependencies that run with \
             every cargo command, tests included.",
        ),
        (
            &[Is("rust-toolchain"), Is("rust-toolchain.toml")],
            "rustup reads this file to choose the compiler programs cargo runs.",
        ),
        (
            &[Is("conftest.py")],
            "pytest runs conftest.py when the tests run.",
        ),
        (
            &[
                Is("pytest.ini"),
                Is(".pytest.ini"),
                Is("pyproject.toml"),
                Is("setup.cfg"),
                Is("tox.ini"),
            ],
            "pytest and Python build tools read these settings, which can load plugins \
             and run code.",
        ),
        (
            &[Is("setup.py"), Is("noxfile.py")],
            "Python runs this file when the project is built, installed or tested.",
        ),
        (
            &[
                Is("sitecustomize.py"),
                Is("usercustomize.py"),
                EndsWith(".pth"),
            ],
            "Python runs this file at start-up, before any script or test.",
        ),
        (
            &[Is("pyvenv.cfg")],
            "It chooses the Python program a virtual environment runs.",
        ),
        (
            &[Is("makefile"), Is("gnumakefile"), EndsWith(".mk")],
            "make runs the commands this file names.",
        ),
        (
            &[Is("cmakelists.txt"), EndsWith(".cmake")],
            "CMake runs the commands this file names when the project is configured or built.",
        ),
        (
            &[Is("justfile"), Is(".justfile")],
            "just runs the recipes this file names.",
        ),
        (
            &[
                StartsWith("vite.config."),
                StartsWith("babel.config."),
                Is(".babelrc"),
                StartsWith(".babelrc."),
                StartsWith("playwright.config."),
                StartsWith("karma.conf."),
                StartsWith("webpack.config."),
                Is(".nycrc"),
                StartsWith(".nycrc."),
            ],
            "JavaScript build and test tools load the code this file names when they run.",
        ),
        (
            &[EndsWith(".code-workspace")],
            "Editor workspace tasks can run commands when the workspace is opened.",
        ),
        (
            &[
                Is("build.gradle"),
                Is("build.gradle.kts"),
                Is("settings.gradle"),
                Is("settings.gradle.kts"),
                Is("gradle.properties"),
                Is("gradle-wrapper.properties"),
                Is("gradlew"),
                Is("gradlew.bat"),
            ],
            "Gradle runs this build logic, or downloads the Gradle it names, with every build and test.",
        ),
        (
            &[Is("pom.xml"), Is("mvnw"), Is("mvnw.cmd")],
            "Maven runs the plugins this file names with every build and test.",
        ),
        (
            &[
                EndsWith(".csproj"),
                EndsWith(".fsproj"),
                EndsWith(".vbproj"),
                Is("directory.build.props"),
                Is("directory.build.targets"),
                Is("global.json"),
            ],
            "MSBuild runs the targets these files name with every dotnet build and test.",
        ),
        (
            &[Is("go.mod"), Is("go.work")],
            "go.mod can name the Go toolchain that runs every go command.",
        ),
        (
            &[Is("pnpm-workspace.yaml"), Is(".pnp.cjs"), Is(".pnp.loader.mjs")],
            "pnpm and yarn read these to decide what runs before a script, and yarn loads them into every script.",
        ),
        (
            &[
                Is("mise.toml"),
                Is(".mise.toml"),
                Is("mise.local.toml"),
                Is(".tool-versions"),
                Is(".python-version"),
                Is(".node-version"),
                Is(".nvmrc"),
            ],
            "These choose which program versions run the project's commands, and can install them.",
        ),
        (
            &[StartsWith("eslint.config."), StartsWith(".eslintrc.")],
            "ESLint runs its configuration as code when the linter runs.",
        ),
    ]
};

/// `target` relative to `root`, when it is inside it. Compared without
/// case on Windows, where the guard and Rust may spell a folder differently.
fn inside(root: &Path, target: &Path) -> Option<PathBuf> {
    let mut rest = target.components();
    for part in root.components() {
        let next = rest.next()?;
        let same = if cfg!(windows) {
            part.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&next.as_os_str().to_string_lossy())
        } else {
            part == next
        };
        if !same {
            return None;
        }
    }
    let relative: PathBuf = rest.collect();
    (!relative.as_os_str().is_empty()
        && relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))))
    .then_some(relative)
}

/// The file `request` changes, relative to `folder`, when the guard named
/// one inside it.
pub fn target_in_folder(folder: &Path, request: &GuardRequest) -> Option<PathBuf> {
    request
        .target
        .as_deref()
        .and_then(|target| inside(folder, target))
}

const HOOKS_PATH_RISK: &str =
    "This is the repository's hooks folder (core.hooksPath): git runs its files as programs.";

/// Classify the file `request` changes in `folder` (blocking: asks git
/// whether it ignores the file, and where the repository keeps its hooks,
/// when `in_repo`).
pub fn change_target(folder: &Path, request: &GuardRequest, in_repo: bool) -> ChangeTarget {
    let (Some(relative), Some(target)) =
        (target_in_folder(folder, request), request.target.as_deref())
    else {
        return ChangeTarget::Unknown;
    };
    let mut risk = execution_risk(&relative);
    if risk == Some(GIT_FOLDER_RISK) {
        return ChangeTarget::GitFolder;
    }
    if in_repo && risk.is_none() {
        // A hooks folder the repository configured, or one git cannot name.
        let hooked = configured_hooks_dirs(folder).map_or(true, |dirs| {
            dirs.iter()
                .any(|dir| target == dir.as_path() || inside(dir, target).is_some())
        });
        if hooked {
            risk = Some(HOOKS_PATH_RISK);
        }
    }
    if in_repo && risk.is_none() {
        // A file git reads as configuration (`include.path`), or any file
        // when git cannot list them.
        let included = configured_includes(folder).map_or(true, |files| {
            files.iter().any(|file| same_path(file, target))
        });
        if included {
            risk = Some(INCLUDED_CONFIG_RISK);
        }
    }
    let untracked = in_repo && !matches!(git_takes_changes(folder, &relative), Ok(true));
    let ignore_rules = relative
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(".gitignore"));
    match risk {
        Some(risk) => ChangeTarget::RunsPrograms { risk, untracked },
        None if untracked => ChangeTarget::Untracked,
        None if ignore_rules => ChangeTarget::IgnoreRules,
        None => ChangeTarget::Plain,
    }
}

const INCLUDED_CONFIG_RISK: &str =
    "git reads this file as configuration (include.path), which can make git run programs.";

/// Whether `a` and `b` name the same path (without case on Windows).
fn same_path(a: &Path, b: &Path) -> bool {
    let (a, b): (Vec<_>, Vec<_>) = (a.components().collect(), b.components().collect());
    a.len() == b.len()
        && a.iter().zip(&b).all(|(x, y)| {
            if cfg!(windows) {
                x.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&y.as_os_str().to_string_lossy())
            } else {
                x == y
            }
        })
}

/// The files git's configuration includes (`include.path` and
/// `includeIf.<condition>.path` in any config scope, read without Shodh's
/// overrides), each in every place it may name: as given when absolute,
/// else relative to the git folder (a repository config's base), the work
/// tree and `folder`; `~/` against the home folder.
fn configured_includes(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let listed = config_values(folder, &["--get-regexp", r"^include(if)?\."])?;
    let top = PathBuf::from(run_git(folder, &["rev-parse", "--show-toplevel"])?);
    let git_dir = PathBuf::from(run_git(folder, &["rev-parse", "--absolute-git-dir"])?);
    let home = dirs::home_dir();
    let mut files = Vec::new();
    for line in listed.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        if !key.ends_with(".path") || value.is_empty() {
            continue;
        }
        let bases: Vec<PathBuf> = if let Some(rest) = value.strip_prefix("~/") {
            home.iter().map(|h| h.join(rest)).collect()
        } else if Path::new(value).is_absolute() {
            vec![PathBuf::from(value)]
        } else {
            [&git_dir, &top, &folder.to_path_buf()]
                .iter()
                .map(|base| base.join(value))
                .collect()
        };
        for file in bases {
            // A file that does not exist yet: its folder resolved, its name kept.
            let resolved = std::fs::canonicalize(&file).ok().or_else(|| {
                let name = file.file_name()?;
                std::fs::canonicalize(file.parent()?)
                    .ok()
                    .map(|dir| dir.join(name))
            });
            files.push(resolved.map(strip_verbatim).unwrap_or(file));
        }
    }
    Ok(files)
}

/// The hooks folders the repository holding `folder` configures
/// (`core.hooksPath` in any of its config files, read without Shodh's own
/// override), resolved against the work tree.
fn configured_hooks_dirs(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let top = PathBuf::from(run_git(folder, &["rev-parse", "--show-toplevel"])?);
    let values = config_values(folder, &["--get-all", "core.hooksPath"])?;
    Ok(values
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let dir = Path::new(value);
            let dir = if dir.is_absolute() {
                dir.to_path_buf()
            } else {
                top.join(dir)
            };
            std::fs::canonicalize(&dir)
                .map(strip_verbatim)
                .unwrap_or(dir)
        })
        .collect())
}

/// Whether "Discard changes" (`git add --all`) takes in a change to
/// `relative`: true unless git ignores the file. `Err` when git cannot tell.
fn git_takes_changes(folder: &Path, relative: &Path) -> Result<bool, String> {
    let output = git(folder)?
        .arg("check-ignore")
        .arg("--quiet")
        .arg("--")
        .arg(relative)
        .output()
        .map_err(|e| format!("git could not be started: {e}"))?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// The undo note of a change to a file git ignores.
pub const UNTRACKED_UNDO: &str =
    "This file is not tracked by git; this change cannot be undone by Discard.";

/// What the user has not reviewed in a conversation's code folder. A
/// command can run any file of the folder (a test, a module a test imports,
/// a file a script reads), so an allowlisted command runs without asking
/// only while nothing here is listed ([`auto_approval`]). Files are cleared
/// only when the user approves an allowlisted command whose prompt showed
/// every one of them ([`ShownReview`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReviewState {
    /// Files changed without the user seeing the change (relative to the
    /// folder, `/`-separated): edits applied without asking, edits whose
    /// prompt cut the content, and files commands and MCP tools changed.
    pub files: Vec<String>,
    /// Why Shodh cannot tell what changed (a command ran outside git, a
    /// snapshot failed, a call stopped before it could be compared); while
    /// set, allowlisted commands ask.
    pub unknown: Option<String>,
    /// Commands the user approved that start a process that keeps running.
    /// Shodh cannot see when such a process stops, so while any is listed,
    /// every edit and write of the conversation asks.
    pub background: Vec<String>,
    /// Commands and MCP tool calls allowed to run whose changes have not
    /// been compared yet (call id → label). Written before the call may
    /// start and removed with its result, so a call cut off by a crash or a
    /// closed session still counts: while any is listed, commands ask.
    pub uncompared: std::collections::BTreeMap<String, String>,
}

impl ReviewState {
    /// Add what `other` lists to this state.
    pub fn merge(&mut self, other: ReviewState) {
        for file in other.files {
            if !self.files.contains(&file) {
                self.files.push(file);
            }
        }
        if self.unknown.is_none() {
            self.unknown = other.unknown;
        }
        for process in other.background {
            if !self.background.contains(&process) {
                self.background.push(process);
            }
        }
        self.uncompared.extend(other.uncompared);
    }

    /// The state when the conversation's record cannot be read: counted as
    /// changed, so commands ask.
    pub fn unreadable(reason: &str) -> Self {
        ReviewState {
            unknown: Some(format!(
                "Shodh could not read which files changed without your review ({reason})"
            )),
            ..ReviewState::default()
        }
    }

    /// Whether the user has seen every change, so an allowlisted command
    /// may run by itself.
    pub fn is_clear(&self) -> bool {
        self.files.is_empty() && self.unknown.is_none() && self.uncompared.is_empty()
    }

    /// List `changed` (folder-relative paths) as not reviewed.
    pub fn add_files<I: IntoIterator<Item = String>>(&mut self, changed: I) {
        for file in changed {
            if !self.files.contains(&file) {
                self.files.push(file);
            }
        }
    }

    /// Record that Shodh cannot tell what changed, and why.
    pub fn mark_unknown(&mut self, reason: String) {
        self.unknown = Some(reason);
    }

    /// The user approved an allowlisted command whose prompt showed `shown`:
    /// exactly those files, and the unknown note if it was the one shown,
    /// count as reviewed. Anything listed after the prompt opened stays.
    pub fn clear_shown(&mut self, shown: &ShownReview) {
        self.files.retain(|file| !shown.files.contains(file));
        if shown.unknown.is_some() && self.unknown == shown.unknown {
            self.unknown = None;
        }
        self.uncompared
            .retain(|id, _| !shown.uncompared.contains(id));
    }

    /// The call `id` (`label`) may change files: until its result is
    /// recorded ([`ReviewState::compared`]), commands ask.
    pub fn start_call(&mut self, id: &str, label: &str) {
        self.uncompared.insert(id.to_string(), label.to_string());
    }

    /// The call `id` was compared: list what it changed, or why that cannot
    /// be told.
    pub fn compared(&mut self, id: &str, changed: Result<Vec<String>, String>) {
        self.uncompared.remove(id);
        match changed {
            Ok(files) => self.add_files(files),
            Err(reason) => self.mark_unknown(reason),
        }
    }
}

/// What a command prompt showed of the [`ReviewState`]: every listed file,
/// and the unknown note. Approving that prompt clears exactly this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShownReview {
    pub files: Vec<String>,
    pub unknown: Option<String>,
    /// Ids of the uncompared calls the prompt named.
    pub uncompared: Vec<String>,
}

/// Preview key of the files a command prompt lists as changed without
/// review: every one of them, as an array.
pub const UNREVIEWED_KEY: &str = "changed_without_review";

/// Preview key of what approving does with those files.
pub const REVIEW_NOTE_KEY: &str = "review_note";

/// Preview key of the token the prompt that showed the files sends back with
/// its approval; only then is [`ShownReview`] cleared. An approval without
/// it (the Inbox, a keyboard shortcut) runs the command and clears nothing.
pub const REVIEW_TOKEN_KEY: &str = "review_token";

/// Preview key of why Auto mode asks about a command it does not run by
/// itself, in plain words.
pub const AUTO_MODE_KEY: &str = "auto_mode";

/// What is going on in the session that bears on a guarded call.
#[derive(Debug, Clone, Copy)]
pub struct GateContext<'a> {
    /// The conversation's [`ReviewState`].
    pub review: &'a ReviewState,
    /// A command or MCP tool call allowed earlier that is still running (its
    /// label): it may run, or be changed by, what this call does.
    pub running: Option<&'a str>,
    /// An MCP tool of the session that can change files without asking (its
    /// name): omp runs it in the same step as a command, at the same time.
    pub unasked_writer: Option<&'a str>,
    /// Why what is around a command Auto mode recognises makes it ask
    /// ([`command_surroundings`]): a file it shows leads outside the project,
    /// or git's settings name a program.
    pub command_problem: Option<&'a str>,
    /// Whether a command that runs the project's own code may run without
    /// asking: commands are sandboxed on this system, or the user trusts this
    /// workspace's builds and tests as its build files are now, or approved one
    /// in this conversation since they last changed ([`project_fingerprint`]).
    pub project_code_cleared: bool,
    /// Commands run in an operating-system sandbox on this system
    /// ([`super::run_command::commands_sandboxed`]): Creator mode's commands
    /// run without asking only then. Passed in, so both answers are tested on
    /// every system.
    pub sandboxed: bool,
    /// Creator mode where commands are not sandboxed: the command is one
    /// Shodh's rules know only inspects (ls, cat, head, tail, wc, grep, rg,
    /// pwd; never git), and every file it shows resolves inside the creation
    /// folder, links followed ([`command_surroundings`]).
    pub inspects: bool,
}

/// Preview key of what a command that runs the project's own code does, on
/// the prompt Auto mode shows before the first one of a conversation.
pub const PROJECT_CODE_KEY: &str = "runs_project_code";

/// What that prompt says, where commands are not sandboxed.
pub const PROJECT_CODE_TEXT: &str = "This runs the project's own code (build scripts, package \
     scripts, conftest.py, its tests) with your account and your network: commands are not \
     sandboxed on this system. Approving it here lets this conversation's builds and tests run \
     without asking until the files that choose what they run change. To let every \
     conversation run them, trust the workspace in its Code settings.";

// ── Build trust ────────────────────────────────────────────────────────────

/// What the files that choose and run a project's builds and tests were when
/// a fingerprint was taken: each file's path (relative to the code folder,
/// `/`-separated) with a hash of its content, and for the tool folders that
/// load code (`node_modules`, virtual environments) a hash of their listing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFingerprint(pub std::collections::BTreeMap<String, String>);

impl ProjectFingerprint {
    /// The first path that differs between `self` and `now`, or `None` when
    /// they are the same.
    pub fn first_change(&self, now: &ProjectFingerprint) -> Option<String> {
        let keys: std::collections::BTreeSet<&String> = self.0.keys().chain(now.0.keys()).collect();
        keys.into_iter()
            .find(|key| self.0.get(*key) != now.0.get(*key))
            .cloned()
    }
}

/// The user's trust that a workspace may run its own builds and tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildTrust {
    /// The project when the user trusted it: any change revokes the trust.
    pub fingerprint: ProjectFingerprint,
    /// When (RFC 3339).
    pub trusted_at: String,
}

/// Files that choose or run the programs of builds and tests, checked at the
/// root of the code folder even when git ignores them (a project `.npmrc` or
/// `.envrc` often is).
const ROOT_BUILD_FILES: [&str; 26] = [
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pnp.cjs",
    ".envrc",
    ".tool-versions",
    "mise.toml",
    ".mise.toml",
    ".python-version",
    ".node-version",
    ".nvmrc",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config",
    ".cargo/config.toml",
    "build.rs",
    "Cargo.toml",
    "package.json",
    "pnpm-workspace.yaml",
    "conftest.py",
    "pytest.ini",
    "pyproject.toml",
    "setup.cfg",
    "tox.ini",
    "go.mod",
    "go.work",
];

/// Folders git usually ignores whose contents builds and tests load: their
/// listing is in the fingerprint, so a package or script dropped into one
/// revokes trust.
const TOOL_FOLDERS: [&str; 10] = [
    "node_modules",
    "node_modules/.bin",
    ".venv/bin",
    ".venv/Scripts",
    "venv/bin",
    "venv/Scripts",
    ".venv/Lib/site-packages",
    "venv/Lib/site-packages",
    ".yarn/releases",
    ".yarn/plugins",
];

/// A hash of `dir`'s entries (name, size, modification time), or `missing`.
fn listing_hash(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let Ok(entries) = std::fs::read_dir(dir) else {
        return "missing".to_string();
    };
    let mut lines: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let meta = entry.metadata().ok();
            let len = meta.as_ref().map_or(0, |m| m.len());
            let modified = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{}\t{len}\t{modified}", entry.file_name().to_string_lossy())
        })
        .collect();
    lines.sort_unstable();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("listing {:x}", hasher.finalize())
}

/// The virtual environments' `site-packages` folders on macOS and Linux
/// (`lib/pythonX.Y/site-packages`).
fn unix_site_packages(folder: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for venv in [".venv", "venv"] {
        let Ok(versions) = std::fs::read_dir(folder.join(venv).join("lib")) else {
            continue;
        };
        for version in versions.filter_map(Result::ok) {
            let name = version.file_name().to_string_lossy().into_owned();
            if name.starts_with("python") {
                found.push(format!("{venv}/lib/{name}/site-packages"));
            }
        }
    }
    found.sort_unstable();
    found
}

/// What the files that choose and run `folder`'s builds and tests are now
/// (blocking): every file git tracks or would track whose change can run
/// programs ([`execution_risk`]), the [`ROOT_BUILD_FILES`] even when ignored,
/// and the listings of the [`TOOL_FOLDERS`] and virtual environments. Fails
/// outside a git repository: trust needs a repository Shodh can read.
pub fn project_fingerprint(folder: &Path) -> Result<ProjectFingerprint, String> {
    match run_git(folder, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(out) if out == "true" => {}
        _ => return Err("the code folder is not a git repository".to_string()),
    }
    let raw = run_git_raw(
        folder,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ".",
        ],
    )?;
    let mut files = std::collections::BTreeMap::new();
    for entry in raw.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        let relative = String::from_utf8_lossy(entry).to_string();
        if execution_risk(Path::new(&relative)).is_some() {
            files.insert(relative.clone(), content_hash(&folder.join(&relative)));
        }
        if files.len() > MAX_SNAPSHOT_FILES {
            return Err("the project has too many build files to fingerprint".to_string());
        }
    }
    for name in ROOT_BUILD_FILES {
        files
            .entry(name.to_string())
            .or_insert_with(|| content_hash(&folder.join(name)));
    }
    for dir in TOOL_FOLDERS
        .iter()
        .map(|d| d.to_string())
        .chain(unix_site_packages(folder))
    {
        files.insert(format!("{dir}/"), listing_hash(&folder.join(&dir)));
    }
    Ok(ProjectFingerprint(files))
}

/// Command arguments that do nothing when given as these (some model layers
/// send every optional argument, empty).
fn unset(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => true,
        Value::String(text) => text.trim().is_empty(),
        _ => false,
    }
}

/// Whether a `run_command` call is only a command: no time limit of its own
/// and no other argument.
pub fn plain_command_input(input: &Value) -> bool {
    let Some(map) = input.as_object() else {
        return false;
    };
    map.get("command").and_then(Value::as_str).is_some()
        && map
            .iter()
            .all(|(key, value)| key == "command" || unset(value))
}

/// Programs that start something that keeps running after the command
/// returns, as a process of the command (Shodh stops it when the command
/// ends).
const DETACHING_PROGRAMS: [&str; 10] = [
    "nohup",
    "setsid",
    "disown",
    "start-process",
    "saps",
    "start-job",
    "sajb",
    "screen",
    "tmux",
    "daemonize",
];

/// Programs that have the system start something for the command: a
/// scheduled task, a service, a launch agent or job, an app opened through
/// the desktop. What they start is not a process of the command, so Shodh
/// cannot stop it when the command ends.
const SYSTEM_STARTERS: [&str; 24] = [
    // macOS
    "launchctl",
    "open",
    "osascript",
    "at",
    "batch",
    "crontab",
    // Windows
    "schtasks",
    "wmic",
    "invoke-cimmethod",
    "icim",
    "invoke-wmimethod",
    "iwmi",
    "register-scheduledtask",
    "new-scheduledtask",
    "start-scheduledtask",
    "set-scheduledtask",
    "sc",
    "new-service",
    "start-service",
    "set-service",
    "explorer",
    // Linux
    "systemd-run",
    "systemctl",
    "loginctl",
];

/// Programs that run the words after them as another command.
const COMMAND_PREFIXES: [&str; 9] = [
    "sudo", "env", "nohup", "command", "exec", "time", "xargs", "nice", "builtin",
];

/// Shells whose arguments are a command line of their own (`powershell
/// -Command Register-ScheduledTask ...`).
const INNER_SHELLS: [&str; 6] = ["powershell", "pwsh", "cmd", "bash", "sh", "zsh"];

/// Whether `command` has the system start a program for it
/// ([`SYSTEM_STARTERS`]) as the program of one of its parts, after a prefix
/// (`sudo`, `env`, ...), or inside a shell it runs.
pub fn starts_system_program(command: &str) -> bool {
    segments(command).iter().any(|words| {
        let program = words.iter().map(|word| program_name(word)).find(|name| {
            !COMMAND_PREFIXES.contains(name) && !name.contains('=') && !name.starts_with('-')
        });
        let Some(program) = program else {
            return false;
        };
        SYSTEM_STARTERS.contains(&program)
            || (INNER_SHELLS.contains(&program)
                && words
                    .iter()
                    .any(|word| SYSTEM_STARTERS.contains(&program_name(word))))
    })
}

/// Whether `command` starts a process that keeps running after it returns:
/// a lone `&` outside quotes, `start`, a program that detaches one, or one
/// the system starts for it ([`starts_system_program`]). A heuristic for the
/// prompt's note; it never lets a command run unasked.
pub fn starts_background(command: &str) -> bool {
    if starts_system_program(command) {
        return true;
    }
    let chars: Vec<char> = command.chars().collect();
    let mut quote: Option<char> = None;
    for (at, &c) in chars.iter().enumerate() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '&' => {
                let before = at.checked_sub(1).map(|i| chars[i]);
                let after = chars.get(at + 1).copied();
                if !matches!(before, Some('&' | '>' | '<' | '|'))
                    && !matches!(after, Some('&' | '>'))
                {
                    return true;
                }
            }
            None => {}
        }
    }
    segments(command).iter().any(|words| {
        let first = words.first().map(|w| program_name(w)).unwrap_or("");
        first == "start"
            || (first == "cmd"
                && words
                    .windows(2)
                    .any(|pair| matches!(pair[0].as_str(), "/c" | "/k") && pair[1] == "start"))
            || words
                .iter()
                .any(|word| DETACHING_PROGRAMS.contains(&program_name(word)))
    })
}

/// A command Auto mode may run without asking, as far as its words go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCommand {
    /// What it is, in plain words ("a test command").
    pub what: &'static str,
    /// It runs git ([`modes::rules::Rule::git`]).
    pub git: bool,
    /// What it reads and shows, relative to the code folder
    /// ([`modes::rules::Rule::reads`]).
    pub reads: Vec<String>,
    /// It runs the project's own code ([`modes::rules::Rule::runs_project_code`]).
    /// A command on the workspace's list always counts as one.
    pub runs_project_code: bool,
}

/// Whether Auto mode would run this command without asking if nothing were
/// listed as unreviewed: one that Shodh's rules recognise
/// ([`modes::rules::check`]) or exactly a workspace list entry with no other
/// argument, not destructive, starting nothing meant to keep running, with
/// no argument but the command. `Err` says why it asks, in plain words.
pub fn auto_command(
    settings: &CodeSettings,
    request: &GuardRequest,
    plan: &ChangePlan,
) -> Result<AutoCommand, String> {
    let command = str_arg(&request.input, "command");
    if request.tool != super::run_command::RUN_COMMAND {
        return Err("it is not a command".to_string());
    }
    if let ChangePlan::Refuse(reason) = plan {
        return Err(reason.clone());
    }
    if let Some(reason) = destructive_reason(command) {
        return Err(format!(
            "it is destructive ({})",
            reason.trim_end_matches('.')
        ));
    }
    if starts_background(command) {
        return Err("it starts something that keeps running".to_string());
    }
    if !plain_command_input(&request.input) {
        return Err("it asks for its own time limit".to_string());
    }
    if settings.allows(command) {
        // A listed entry is a whole command of plain words: whether it is git
        // decides the git settings check.
        let git = command.split([' ', '\t']).find(|w| !w.is_empty()) == Some("git");
        return Ok(AutoCommand {
            what: "a command on the workspace's list",
            git,
            reads: Vec::new(),
            runs_project_code: true,
        });
    }
    modes::rules::check(command).map(|rule| AutoCommand {
        what: rule.what,
        git: rule.git,
        reads: rule.reads,
        runs_project_code: rule.runs_project_code,
    })
}

/// Whether Auto mode would run this command by itself if nothing were listed
/// as unreviewed ([`auto_command`]). Only approving such a command marks
/// files reviewed: it is the action the list protects.
pub fn command_eligible(
    settings: &CodeSettings,
    request: &GuardRequest,
    plan: &ChangePlan,
) -> bool {
    auto_command(settings, request, plan).is_ok_and(|command| command.runs_project_code)
}

/// Why a guarded call may run without asking in `mode`, or `None` when the
/// user decides. Destructive commands always ask. Edits run by themselves
/// only in the modes that accept edits ([`modes::code_edits`]), only while a
/// branch can undo them, only in files that cannot run programs (`target`,
/// [`change_target`]) and only while no command of the conversation is
/// running (or, from before commands ran in their own process, may still
/// be). A command runs by itself only in Auto mode ([`modes::code_commands`])
/// when it is one Auto mode recognises ([`auto_command`]), while the user has
/// seen every change ([`ReviewState::is_clear`]), nothing else is running, no
/// MCP tool can change files without asking and nothing around the command
/// makes it ask ([`GateContext::command_problem`]). Creator mode decides by
/// its own rules ([`creator_approval`]).
pub fn auto_approval(
    mode: AgentMode,
    settings: &CodeSettings,
    request: &GuardRequest,
    plan: &ChangePlan,
    target: ChangeTarget,
    gate: &GateContext<'_>,
) -> Option<&'static str> {
    if mode.creates() {
        return creator_approval(request, target, gate);
    }
    let undoable = matches!(plan, ChangePlan::Continue(_) | ChangePlan::CreateBranch);
    match request.tool.as_str() {
        "edit" | "write" => (undoable
            && modes::code_edits(mode) == Gate::Run
            && target == ChangeTarget::Plain
            && gate.review.background.is_empty()
            && gate.running.is_none())
        .then_some("edits run without asking in this mode"),
        tool if tool == super::run_command::RUN_COMMAND => {
            let command = auto_command(settings, request, plan).ok()?;
            // A command that runs the project's code needs the project cleared
            // (sandboxed, trusted, or approved in this conversation) and every
            // change seen; one that only inspects needs neither.
            let project_code_ok =
                !command.runs_project_code || (gate.project_code_cleared && gate.review.is_clear());
            (modes::code_commands(mode) == CommandGate::Rules
                && project_code_ok
                && gate.running.is_none()
                && gate.unasked_writer.is_none()
                && gate.command_problem.is_none())
            .then_some(if command.runs_project_code {
                "Auto mode runs this build or test without asking"
            } else {
                "Auto mode runs this command without asking: it only reads"
            })
        }
        _ => None,
    }
}

/// Why a guarded call of a Creator session may run without asking, or `None`
/// when the user decides.
///
/// - An edit or write runs by itself when it lands inside the creation folder
///   (`target` is named and not the folder's `.git`; the guard has already
///   blocked every path outside the folder, links followed), while no command
///   of the conversation runs and none it had the system start may still.
///   Files that choose what builds run (package.json, build.rs, ...) are made
///   without asking too: the folder is new and the agent writes all of it, and
///   where commands are not sandboxed every command asks anyway.
/// - A command runs by itself only where commands are sandboxed
///   ([`GateContext::sandboxed`]: the sandbox can change only the creation
///   folder and the conversation's scratch folder, and has no network), as one
///   plain call that is not destructive, starts nothing that keeps running or
///   that the system starts for it (`open`, `launchctl`, `osascript` leave the
///   sandbox), installs no package ([`modes::installs::mentions_install`]: an
///   install needs the network), with nothing else running and no MCP tool
///   that changes files without asking. Auto mode's trust and review checks do
///   not apply: the sandbox bounds what a command can reach, and the folder
///   holds only what this conversation made.
pub fn creator_approval(
    request: &GuardRequest,
    target: ChangeTarget,
    gate: &GateContext<'_>,
) -> Option<&'static str> {
    match request.tool.as_str() {
        "edit" | "write" => (modes::code_edits(AgentMode::Creator) == Gate::Run
            && !matches!(target, ChangeTarget::Unknown | ChangeTarget::GitFolder)
            && gate.review.background.is_empty()
            && gate.running.is_none())
        .then_some("Creator mode makes files in its creation folder without asking"),
        tool if tool == super::run_command::RUN_COMMAND && !gate.sandboxed => {
            // Not sandboxed: only a command that inspects files of the folder.
            (gate.inspects
                && plain_command_input(&request.input)
                && gate.running.is_none()
                && gate.unasked_writer.is_none())
            .then_some(
                "Creator mode runs this without asking: it only reads files in its creation \
                 folder",
            )
        }
        tool if tool == super::run_command::RUN_COMMAND => {
            let command = str_arg(&request.input, "command");
            (modes::code_commands(AgentMode::Creator) == CommandGate::Sandboxed
                && gate.sandboxed
                && !command.trim().is_empty()
                && destructive_reason(command).is_none()
                && !starts_background(command)
                && !modes::installs::mentions_install(command)
                && plain_command_input(&request.input)
                && gate.running.is_none()
                && gate.unasked_writer.is_none())
            .then_some(
                "Creator mode runs this command without asking: it is sandboxed to the creation \
                 folder, with no network",
            )
        }
        _ => None,
    }
}

/// Why Creator mode asks about a command, in plain words, for its prompt
/// (`None` when it would run by itself, [`creator_approval`]).
pub fn creator_command_reason(request: &GuardRequest, gate: &GateContext<'_>) -> Option<String> {
    let command = str_arg(&request.input, "command");
    if !gate.sandboxed {
        if gate.inspects {
            if let Some(running) = gate.running {
                return Some(format!("{running} is still running"));
            }
            if let Some(writer) = gate.unasked_writer {
                return Some(format!("the tool {writer} can change files without asking"));
            }
            if !plain_command_input(&request.input) {
                return Some("it asks for its own time limit".to_string());
            }
            return None;
        }
        return Some(
            "commands are not sandboxed on this system, so each one waits for your approval. \
             It can run files the assistant wrote in this folder (made without asking), with \
             your account and your network"
                .to_string(),
        );
    }
    if modes::installs::mentions_install(command) {
        return Some(
            "it installs or downloads packages, which needs the network: the sandbox cannot \
             limit that to a package registry"
                .to_string(),
        );
    }
    if let Some(reason) = destructive_reason(command) {
        return Some(format!(
            "it is destructive ({})",
            reason.trim_end_matches('.')
        ));
    }
    if starts_background(command) {
        return Some("it starts something that keeps running".to_string());
    }
    if !plain_command_input(&request.input) {
        return Some("it asks for its own time limit".to_string());
    }
    if let Some(running) = gate.running {
        return Some(format!("{running} is still running"));
    }
    if let Some(writer) = gate.unasked_writer {
        return Some(format!("the tool {writer} can change files without asking"));
    }
    None
}

/// What a Creator prompt says about undoing a change: there is no branch, so
/// the creation folder's saved versions and "Delete it" take its place.
pub const CREATION_UNDO: &str = "This stays in the creation folder. Each finished answer is saved \
     as a version in Artifacts, and \"Delete it\" removes the whole folder.";

/// Preview key of why Creator mode asks about a command (the prompt shows it
/// as "Creator mode: Asks because ...").
pub const CREATOR_MODE_KEY: &str = "creator_mode";

/// Preview key of what an approved package install gets in Creator mode.
pub const NETWORK_KEY: &str = "network";

/// What the prompt of a recognised install says where commands are
/// sandboxed, when its packages run their own install scripts.
pub const INSTALL_NETWORK_TEXT: &str = "Approving lets this one install reach the whole \
     internet. It still runs in the sandbox, so it can change only the creation folder, but the \
     packages it downloads run their own install scripts, with that same internet access and no \
     further prompt. Their scripts run because the command does not skip them (npm, pnpm and \
     yarn: --ignore-scripts; pip: --only-binary :all:).";

/// The same prompt when the command skips the packages' install scripts.
pub const INSTALL_NETWORK_NO_SCRIPTS_TEXT: &str = "Approving lets this one install reach the \
     whole internet to download packages. It still runs in the sandbox, so it can change only \
     the creation folder, and it skips the packages' own install scripts.";

/// What an approved `command`'s prompt says about the network it gets
/// ([`approved_network`]), by whether its packages' scripts run.
pub fn install_network_text(command: &str) -> &'static str {
    match modes::installs::install(command) {
        Ok(install) if !install.scripts => INSTALL_NETWORK_NO_SCRIPTS_TEXT,
        _ => INSTALL_NETWORK_TEXT,
    }
}

/// Whether an approved command of a `mode` session runs with the network in
/// its sandbox: only a package install Creator mode recognises
/// ([`modes::installs::install`]), where commands are sandboxed (elsewhere
/// commands are not contained, and the network is not Shodh's to give).
pub fn approved_network(mode: AgentMode, sandboxed: bool, command: &str) -> bool {
    mode.creates() && sandboxed && modes::installs::install(command).is_ok()
}

/// Why Auto mode asks about a command whose words it recognises
/// ([`auto_command`]), from what is around it, or `None`. Every file a
/// read-only command shows must resolve inside `folder`, links included (a
/// link to a key elsewhere would send it to the model unasked); a git
/// command runs by itself only while the git settings it reads name no
/// program ([`git_config_problem`]). `home` is the commands' home folder.
pub fn command_surroundings(folder: &Path, home: &Path, command: &AutoCommand) -> Option<String> {
    for read in &command.reads {
        if !resolves_inside(folder, read) {
            return Some(format!("{read} leads outside the project"));
        }
    }
    if command.git {
        return git_config_problem(folder, home);
    }
    None
}

/// Whether `relative` (a path the command names, relative to `folder`)
/// resolves inside `folder`, links followed. A path that does not exist
/// resolves inside when its existing parent does.
fn resolves_inside(folder: &Path, relative: &str) -> bool {
    let Ok(root) = std::fs::canonicalize(folder) else {
        return false;
    };
    let mut candidate = folder.join(relative);
    loop {
        match std::fs::canonicalize(&candidate) {
            Ok(resolved) => return resolved.starts_with(&root),
            Err(_) => match candidate.parent() {
                Some(parent) if parent != candidate.as_path() => candidate = parent.to_path_buf(),
                _ => return false,
            },
        }
    }
}

/// git settings that name no program, by section (and, for a section with
/// subsections, its key): every other setting of the repository or of the
/// commands' home makes git commands ask, because git may run a program it
/// names (`core.fsmonitor`, `core.pager`, `diff.<driver>.textconv`,
/// `alias.*`, `include.path`, ...).
const SAFE_GIT_KEYS: &[&str] = &[
    "core.repositoryformatversion",
    "core.filemode",
    "core.bare",
    "core.logallrefupdates",
    "core.symlinks",
    "core.ignorecase",
    "core.precomposeunicode",
    "core.autocrlf",
    "core.eol",
    "core.safecrlf",
    "core.longpaths",
    "core.quotepath",
    "core.checkstat",
    "core.trustctime",
    "core.untrackedcache",
    "core.abbrev",
    "user.name",
    "user.email",
    "commit.gpgsign",
    "tag.gpgsign",
    "init.defaultbranch",
    "extensions.worktreeconfig",
    "extensions.objectformat",
    "pull.rebase",
    "pull.ff",
    "push.default",
    "push.autosetupremote",
    "fetch.prune",
    "gc.auto",
    "index.version",
    "status.showuntrackedfiles",
];

/// Subsection keys that name no program (`remote.<name>.url`).
const SAFE_GIT_SUBSECTION_KEYS: &[(&str, &str)] = &[
    ("remote", "url"),
    ("remote", "fetch"),
    ("remote", "pushurl"),
    ("remote", "tagopt"),
    ("remote", "prune"),
    ("branch", "remote"),
    ("branch", "merge"),
    ("branch", "rebase"),
    ("branch", "pushremote"),
    ("submodule", "url"),
    ("submodule", "active"),
    ("submodule", "branch"),
];

/// Whether the git setting `key` (as `git config --name-only` lists it)
/// names no program.
pub fn safe_git_key(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    if SAFE_GIT_KEYS.contains(&key.as_str()) || key.starts_with("color.") {
        return true;
    }
    match (key.split_once('.'), key.rsplit_once('.')) {
        (Some((section, _)), Some((_, name))) => SAFE_GIT_SUBSECTION_KEYS
            .iter()
            .any(|(s, n)| *s == section && *n == name && key.matches('.').count() >= 2),
        _ => false,
    }
}

/// Why git commands ask in Auto mode: a setting of the repository (its
/// config and the work tree's) or of the commands' `home` (`.gitconfig` and
/// `.config/git/config`, which an earlier command can write) that may name a
/// program, or settings Shodh cannot read. `None` when every one is safe
/// ([`safe_git_key`]). The system's and the user's own settings are theirs.
pub fn git_config_problem(folder: &Path, home: &Path) -> Option<String> {
    let mut listed =
        match config_values(folder, &["--local", "--includes", "--name-only", "--list"]) {
            Ok(listed) => listed,
            Err(e) => {
                return Some(format!(
                    "Shodh could not read the repository's git settings ({e})"
                ))
            }
        };
    match config_values(
        folder,
        &["--local", "--bool", "--get", "extensions.worktreeConfig"],
    ) {
        Ok(value) if value.trim() == "true" => {
            match config_values(
                folder,
                &["--worktree", "--includes", "--name-only", "--list"],
            ) {
                Ok(more) => listed.push_str(&more),
                Err(e) => {
                    return Some(format!(
                        "Shodh could not read the work tree's git settings ({e})"
                    ))
                }
            }
        }
        Ok(_) => {}
        Err(e) => {
            return Some(format!(
                "Shodh could not read the repository's git settings ({e})"
            ))
        }
    }
    for file in [
        home.join(".gitconfig"),
        home.join(".config").join("git").join("config"),
    ] {
        if !file.exists() {
            continue;
        }
        let path = file.display().to_string();
        match config_values(
            folder,
            &[
                "--file",
                path.as_str(),
                "--includes",
                "--name-only",
                "--list",
            ],
        ) {
            Ok(more) => listed.push_str(&more),
            Err(e) => {
                return Some(format!(
                    "Shodh could not read the git settings in this conversation's home ({e})"
                ))
            }
        }
    }
    listed
        .lines()
        .map(str::trim)
        .find(|key| !key.is_empty() && !safe_git_key(key))
        .map(|key| format!("git's settings name {key}, which can run a program"))
}

/// Add what the session knows to a prompt. A command prompt gets the files
/// changed without review (all of them), what approving does with them, and
/// what is running alongside; an edit or write prompt, a process that may
/// run it. `eligible` is [`command_eligible`] for this call; `token` goes on
/// a command prompt whose approval can clear files ([`REVIEW_TOKEN_KEY`]).
/// Returns what an approval carrying that token clears.
pub fn note_review(
    approval: &mut CodeApproval,
    request: &GuardRequest,
    gate: &GateContext<'_>,
    eligible: bool,
    token: &str,
) -> Option<ShownReview> {
    let map = approval.preview.as_object_mut()?;
    if request.tool != super::run_command::RUN_COMMAND {
        if let Some(running) = gate.running {
            map.insert(
                "running_now".to_string(),
                json!(format!(
                    "{running} is still running and may run this change, so edits ask until \
                     it ends."
                )),
            );
        }
        if let Some(process) = gate.review.background.last() {
            map.insert(
                "running_process".to_string(),
                json!(format!(
                    "{process} was started earlier in this conversation and may still be \
                     running; it may run this change. Shodh cannot tell when it stops, so \
                     every edit asks."
                )),
            );
        }
        return None;
    }
    if let Some(running) = gate.running {
        map.insert(
            "running_now".to_string(),
            json!(format!(
                "{running} is still running and may run files this command changes."
            )),
        );
    }
    let command = str_arg(&request.input, "command");
    if starts_system_program(command) {
        map.insert("keeps_running".to_string(), json!(SYSTEM_START_NOTE));
    } else if starts_background(command) {
        map.insert("keeps_running".to_string(), json!(BACKGROUND_NOTE));
    }
    let review = gate.review;
    if review.is_clear() {
        return None;
    }
    let mut note = String::new();
    if let Some(reason) = &review.unknown {
        note.push_str(&format!(
            "Shodh cannot tell which files changed: {reason}. "
        ));
    }
    if !review.uncompared.is_empty() {
        let calls: Vec<&str> = review.uncompared.values().map(String::as_str).collect();
        note.push_str(&format!(
            "Shodh has not compared what {} changed (still running, or cut off). ",
            calls.join(", ")
        ));
    }
    if !review.files.is_empty() {
        map.insert(UNREVIEWED_KEY.to_string(), json!(review.files));
        note.push_str("The files listed changed without your review. ");
    }
    if eligible {
        note.push_str("Approving runs this command, which may run them, and marks them reviewed.");
        map.insert(REVIEW_TOKEN_KEY.to_string(), json!(token));
    } else {
        note.push_str(
            "Approving this command does not mark them reviewed; only approving a command \
             Auto mode recognises (a test, build or linter, or one on the workspace's list) \
             does.",
        );
    }
    map.insert(REVIEW_NOTE_KEY.to_string(), json!(note));
    eligible.then(|| ShownReview {
        files: review.files.clone(),
        unknown: review.unknown.clone(),
        uncompared: review.uncompared.keys().cloned().collect(),
    })
}

/// What an approval prompt shows for a guarded call.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeApproval {
    pub label: String,
    pub tier: RiskTier,
    pub preview: Value,
    /// An edit's or write's content was cut in the preview: approving it is
    /// no review of the file ([`PREVIEW_CUT_NOTE`]).
    pub preview_cut: bool,
}

/// What an edit or write prompt whose content was cut says.
pub const PREVIEW_CUT_NOTE: &str = "The preview is cut, so approving does not count as reviewing this file: it stays listed as changed without your review until you approve an allowlisted command that lists it.";

/// What a command prompt says about a command that tries to start a process
/// meant to keep running.
pub const BACKGROUND_NOTE: &str = "This tries to start a process that keeps running after the \
     command. Shodh stops every process the command started when the command ends, so it will \
     not keep running.";

/// What a command prompt says about a command that has the system start a
/// program for it ([`starts_system_program`]).
pub const SYSTEM_START_NOTE: &str = "This has the system start a program for it (a scheduled \
     task, a service, a launch agent or an app). Shodh cannot stop that program when the \
     command ends: it may keep running and change files, so every edit in this conversation \
     will ask from now on.";

fn clipped(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        text.to_string()
    } else {
        format!(
            "{}… ({} more characters)",
            truncate_chars(text, max),
            count - max
        )
    }
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", truncate_chars(&flat, max))
    }
}

fn str_arg<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The prompt for `request`. `undo` says what happens to the change if the
/// user wants it back (a branch is created first, or nothing can undo it);
/// `target` is the file an edit or write changes ([`change_target`]): one
/// that can run programs makes the prompt destructive, with the reason.
pub fn approval_for(
    request: &GuardRequest,
    undo: Option<&str>,
    target: ChangeTarget,
) -> CodeApproval {
    let input = &request.input;
    let file_tier = match target {
        ChangeTarget::RunsPrograms { .. } => RiskTier::Destructive,
        _ => RiskTier::Write,
    };
    let preview_cut = match request.tool.as_str() {
        "write" => str_arg(input, "content").chars().count() > MAX_PREVIEW_CHARS,
        "edit" => ["old_string", "new_string"]
            .iter()
            .any(|key| str_arg(input, key).chars().count() > MAX_PREVIEW_CHARS / 2),
        _ => false,
    };
    let (label, tier, mut preview) = match request.tool.as_str() {
        "write" => {
            let path = str_arg(input, "path");
            (
                format!("Write {}", one_line(path, MAX_LABEL_CHARS)),
                file_tier,
                json!({
                    "file": path,
                    "content": clipped(str_arg(input, "content"), MAX_PREVIEW_CHARS),
                }),
            )
        }
        "edit" => {
            let path = str_arg(input, "path");
            (
                format!("Edit {}", one_line(path, MAX_LABEL_CHARS)),
                file_tier,
                json!({
                    "file": path,
                    "replace": clipped(str_arg(input, "old_string"), MAX_PREVIEW_CHARS / 2),
                    "with": clipped(str_arg(input, "new_string"), MAX_PREVIEW_CHARS / 2),
                    "every_match": input.get("replace_all").and_then(Value::as_bool).unwrap_or(false),
                }),
            )
        }
        _ => {
            let command = str_arg(input, "command");
            let warning = destructive_reason(command);
            (
                format!("Run: {}", one_line(command, MAX_LABEL_CHARS)),
                if warning.is_some() {
                    RiskTier::Destructive
                } else {
                    RiskTier::Write
                },
                json!({
                    "warning": warning,
                    "command": clipped(command, MAX_PREVIEW_CHARS),
                    "runs_in": "the code folder",
                }),
            )
        }
    };
    if let Some(map) = preview.as_object_mut() {
        if let (ChangeTarget::RunsPrograms { risk, .. }, "edit" | "write") =
            (target, request.tool.as_str())
        {
            map.insert("warning".to_string(), json!(risk));
        }
        if let Some(undo) = undo {
            map.insert("undo".to_string(), json!(undo));
        }
        if preview_cut {
            map.insert("preview_cut".to_string(), json!(PREVIEW_CUT_NOTE));
        }
    }
    CodeApproval {
        label,
        tier,
        preview_cut,
        preview,
    }
}

/// Key of the diff an edit or write prompt shows: a list of
/// [`crate::code_review::diff::FileChange`] (one file).
pub const CODE_DIFF_KEY: &str = "codeDiff";

/// Lines an edit or write prompt's diff shows at most; a larger change keeps
/// the prompt's text rows only.
pub const MAX_APPROVAL_DIFF_LINES: usize = 800;

/// Bytes of a file read to show the diff of an edit or write to it.
pub const MAX_DIFF_FILE_BYTES: u64 = 1024 * 1024;

/// The text of the file an edit or write lands in, for its diff: `Ok(None)`
/// when it does not exist yet, `Err` when it cannot be shown (no target,
/// unreadable, larger than [`MAX_DIFF_FILE_BYTES`], not UTF-8). Blocking.
#[allow(clippy::result_unit_err)]
pub fn read_for_diff(target: Option<&Path>) -> Result<Option<String>, ()> {
    let target = target.ok_or(())?;
    match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.is_file() && meta.len() <= MAX_DIFF_FILE_BYTES => {}
        Ok(_) => return Err(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    }
    let bytes = std::fs::read(target).map_err(|_| ())?;
    String::from_utf8(bytes).map(Some).map_err(|_| ())
}

/// The diff an edit or write would make to `current` (the file's text now;
/// `None` when it does not exist yet), shown under `path`. `None` when it
/// cannot be shown exactly: an edit whose text is not in the file, or found
/// more than once without `replace_all`, or a change past
/// [`MAX_APPROVAL_DIFF_LINES`].
pub fn change_diff(
    request: &GuardRequest,
    path: &str,
    current: Option<&str>,
) -> Option<crate::code_review::diff::FileChange> {
    let input = &request.input;
    let after = match request.tool.as_str() {
        "write" => str_arg(input, "content").to_string(),
        "edit" => {
            let current = current?;
            let old = str_arg(input, "old_string");
            if old.is_empty() {
                return None;
            }
            let new = str_arg(input, "new_string");
            let every = input
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match current.matches(old).count() {
                0 => return None,
                1 => current.replacen(old, new, 1),
                _ if every => current.replace(old, new),
                _ => return None,
            }
        }
        _ => return None,
    };
    let change = crate::code_review::diff::FileChange::between(path, current, Some(&after));
    (!change.too_large && change.line_count() <= MAX_APPROVAL_DIFF_LINES).then_some(change)
}

/// Files git does not track that [`working_changes`] reads at most; the rest
/// are listed by name.
pub const MAX_UNTRACKED_READ: usize = 200;

/// The conversation's code changes in `folder`: the working tree against
/// `base` (a branch or commit; `HEAD` when `None`), and every file git does
/// not track yet as added, at most `max_lines` lines shown in all
/// ([`crate::code_review::diff::capped`]). Git runs hardened ([`git`]) with
/// the repository's content filters off ([`FILTERED_COMMANDS`]) and external
/// diff programs and text conversion off, so no program of the repository
/// runs. Files that may hold secrets are listed by name, never read.
/// `path` limits it to one file or folder. Blocking.
pub fn working_changes(
    folder: &Path,
    base: Option<&str>,
    path: Option<&str>,
    max_lines: usize,
) -> Result<Vec<crate::code_review::diff::FileChange>, String> {
    use crate::code_review::diff::{capped, parse_unified, FileChange, LIMIT_NOTE};
    use crate::code_review::secrets::{secret_path, SECRET_NOTE};
    let base = base.unwrap_or("HEAD");
    if base.starts_with('-') || base.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("“{base}” is not a branch or commit name"));
    }
    let mut diff_args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--unified=3",
        base,
        "--",
    ];
    if let Some(path) = path {
        diff_args.push(path);
    }
    // Untrimmed: a last context line of spaces is a line.
    let raw_diff = run_git_raw(folder, &diff_args)?;
    let mut changes = parse_unified(&String::from_utf8_lossy(&raw_diff));
    let mut used: usize = changes.iter().map(FileChange::line_count).sum();
    let mut untracked_args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
    if let Some(path) = path {
        untracked_args.push(path);
    }
    let raw = run_git_raw(folder, &untracked_args)?;
    for (read, name) in raw
        .split(|b| *b == 0)
        .filter(|n| !n.is_empty())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .enumerate()
    {
        if secret_path(&name).is_some() {
            changes.push(FileChange::named(&name, "added", SECRET_NOTE));
            continue;
        }
        if read >= MAX_UNTRACKED_READ || used >= max_lines {
            changes.push(FileChange::named(&name, "added", LIMIT_NOTE));
            continue;
        }
        let full = folder.join(&name);
        let meta = std::fs::symlink_metadata(&full).ok();
        let readable = meta
            .as_ref()
            .is_some_and(|m| m.is_file() && m.len() <= MAX_DIFF_FILE_BYTES);
        let text = readable
            .then(|| std::fs::read(&full).ok())
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        let change = match text {
            Some(text) => FileChange::between(&name, None, Some(&text)),
            None => FileChange {
                path: name,
                status: "added",
                binary: readable,
                too_large: !readable,
                hunks: Vec::new(),
                note: None,
            },
        };
        used += change.line_count();
        changes.push(change);
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(capped(changes, max_lines))
}

/// Whether git ignores `relative` (a path inside `folder`): `false` outside a
/// repository. Blocking; runs no program of the repository (`check-ignore`
/// reads only the ignore rules).
pub fn git_ignores(folder: &Path, relative: &str) -> bool {
    let Ok(mut command) = git(folder) else {
        return false;
    };
    command
        .args(["check-ignore", "--quiet", "--", relative])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// The files of `within` (inside `folder`) that git tracks or would add,
/// relative to `folder` with `/`: every file `.gitignore` does not exclude.
/// `None` outside a repository. Blocking; runs no program of the repository.
///
/// `safe.directory` is set for this folder alone, on this one listing: a
/// repository owned by another account (an external drive, a shared folder)
/// is listed as git lists it, so `.gitignore` still decides, instead of git
/// refusing and the caller reading the folder without it. A listing runs no
/// program of the repository (hooks and fsmonitor are off), so trusting the
/// folder for it gives the folder's owner nothing to run.
pub fn listed_files(folder: &Path, within: &str) -> Option<Vec<String>> {
    let safe = safe_directory_settings(folder);
    let mut args: Vec<&str> = Vec::new();
    for setting in &safe {
        args.push("-c");
        args.push(setting);
    }
    args.extend([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        within,
    ]);
    let raw = run_git_raw(folder, &args).ok()?;
    Some(
        raw.split(|b| *b == 0)
            .filter(|n| !n.is_empty())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect(),
    )
}

/// `safe.directory=<folder>` for `folder` as given and as git writes its
/// canonical path (forward slashes, no `\\?\`), never `*`.
fn safe_directory_settings(folder: &Path) -> Vec<String> {
    let mut out = vec![format!(
        "safe.directory={}",
        folder.to_string_lossy().replace('\\', "/")
    )];
    if let Ok(canonical) = std::fs::canonicalize(folder) {
        let text = canonical.to_string_lossy().replace('\\', "/");
        let text = text.strip_prefix("//?/").unwrap_or(&text).to_string();
        let setting = format!("safe.directory={text}");
        if !out.contains(&setting) {
            out.push(setting);
        }
    }
    out
}

/// Split a shell command into lowercased words per command segment. Quotes
/// are dropped, so commands inside `bash -c "…"` are seen too; programs are
/// reduced to their file name without `.exe`.
fn segments(command: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut word = String::new();
    let flush_word = |word: &mut String, current: &mut Vec<String>| {
        if !word.is_empty() {
            current.push(std::mem::take(word).to_lowercase());
        }
    };
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' | '`' | '(' | ')' | '{' | '}' => flush_word(&mut word, &mut current),
            ';' | '|' | '&' | '\n' | '\r' => {
                flush_word(&mut word, &mut current);
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                if matches!(chars.peek(), Some('|') | Some('&')) {
                    chars.next();
                }
            }
            c if c.is_whitespace() => flush_word(&mut word, &mut current),
            c => word.push(c),
        }
    }
    flush_word(&mut word, &mut current);
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn program_name(word: &str) -> &str {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    base.strip_suffix(".exe").unwrap_or(base)
}

fn has_flag(args: &[String], short: char, long: &str) -> bool {
    args.iter().any(|a| {
        a == long
            || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains(short))
            || (a.starts_with('/') && a.len() == 2 && a[1..].starts_with(short))
    })
}

/// Why `command` needs extra care, or `None`. Deliberately broad: a false
/// warning costs a second look, a missed one can cost data.
pub fn destructive_reason(command: &str) -> Option<&'static str> {
    let lowered = command.to_lowercase();
    if lowered.contains("drop table") || lowered.contains("drop database") {
        return Some("Deletes database data.");
    }
    let segments = segments(command);
    for (index, words) in segments.iter().enumerate() {
        for (at, word) in words.iter().enumerate() {
            let args = &words[at + 1..];
            let reason = match program_name(word) {
                "rm" | "remove-item" | "ri" | "rmdir" | "rd" | "del" | "erase" | "unlink"
                | "shred" => Some(
                    if has_flag(args, 'r', "--recursive")
                        || has_flag(args, 's', "-recurse")
                        || args.iter().any(|a| a == "-recurse")
                    {
                        "Deletes files and folders recursively."
                    } else {
                        "Deletes files."
                    },
                ),
                "git" => git_reason(args),
                "format" | "diskpart" | "mkfs" | "dd" | "fdisk" | "clear-disk" => {
                    Some("Can erase a disk.")
                }
                w if w.starts_with("mkfs.") => Some("Can erase a disk."),
                "shutdown" | "reboot" | "halt" | "poweroff" | "stop-computer"
                | "restart-computer" => Some("Shuts down or restarts the computer."),
                "chmod" | "chown" | "icacls" | "takeown"
                    if has_flag(args, 'r', "--recursive") || has_flag(args, 't', "/t") =>
                {
                    Some("Changes permissions recursively.")
                }
                "kill" | "taskkill" | "pkill" | "killall" | "stop-process" => {
                    Some("Stops running programs.")
                }
                "cargo" => account_reason(args, &CARGO_ACCOUNT_COMMANDS, REGISTRY_ACCOUNT),
                "npm" | "pnpm" | "yarn" => {
                    account_reason(args, &NPM_ACCOUNT_COMMANDS, REGISTRY_ACCOUNT)
                }
                "twine" | "poetry" | "uv" | "gem" | "flit" | "hatch" => {
                    account_reason(args, &PUBLISH_COMMANDS, REGISTRY_ACCOUNT)
                }
                "docker" | "podman" => {
                    account_reason(args, &CONTAINER_ACCOUNT_COMMANDS, REGISTRY_ACCOUNT)
                }
                "gh" => account_reason(args, &GH_ACCOUNT_COMMANDS, GITHUB_ACCOUNT),
                _ => None,
            };
            if reason.is_some() {
                return reason;
            }
        }
        // `curl … | sh`: a downloaded script runs.
        let piped_into_shell = index > 0
            && words.first().is_some_and(|w| {
                matches!(
                    program_name(w),
                    "sh" | "bash"
                        | "zsh"
                        | "pwsh"
                        | "powershell"
                        | "iex"
                        | "invoke-expression"
                        | "python"
                        | "python3"
                        | "node"
                )
            })
            && segments[index - 1].iter().any(|w| {
                matches!(
                    program_name(w),
                    "curl" | "wget" | "iwr" | "invoke-webrequest" | "irm" | "invoke-restmethod"
                )
            });
        if piped_into_shell {
            return Some("Runs a script downloaded from the internet.");
        }
    }
    None
}

/// What a prompt says about a command that acts with the user's account on
/// a package or container registry.
const REGISTRY_ACCOUNT: &str = "Uses your account on a package registry: publishes, changes or \
     removes a package, or signs in.";

/// What a prompt says about a `gh` command that acts with the user's GitHub
/// account.
const GITHUB_ACCOUNT: &str = "Uses your GitHub account.";

const CARGO_ACCOUNT_COMMANDS: [&str; 5] = ["publish", "yank", "owner", "login", "logout"];
const NPM_ACCOUNT_COMMANDS: [&str; 12] = [
    "publish",
    "unpublish",
    "deprecate",
    "undeprecate",
    "token",
    "owner",
    "login",
    "logout",
    "adduser",
    "dist-tag",
    "access",
    "team",
];
const PUBLISH_COMMANDS: [&str; 5] = ["upload", "publish", "push", "register", "login"];
const CONTAINER_ACCOUNT_COMMANDS: [&str; 3] = ["push", "login", "logout"];
const GH_ACCOUNT_COMMANDS: [&str; 16] = [
    "auth", "api", "release", "repo", "secret", "variable", "pr", "issue", "workflow", "run",
    "gist", "ssh-key", "gpg-key", "label", "project", "cache",
];

/// `reason` when the subcommand of `args` (the first word that is not an
/// option or a `+toolchain`) is one of `commands`.
fn account_reason(
    args: &[String],
    commands: &[&str],
    reason: &'static str,
) -> Option<&'static str> {
    args.iter()
        .find(|arg| !arg.starts_with('-') && !arg.starts_with('+'))
        .filter(|sub| commands.contains(&sub.as_str()))
        .map(|_| reason)
}

fn git_reason(args: &[String]) -> Option<&'static str> {
    // Skip global options (`-C dir`, `-c k=v`) to the subcommand.
    let mut rest = args;
    while let Some(first) = rest.first() {
        if first == "-c" || first == "-C" || first == "--git-dir" || first == "--work-tree" {
            rest = rest.get(2..).unwrap_or(&[]);
        } else if first.starts_with('-') {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    let (sub, args) = rest.split_first()?;
    match sub.as_str() {
        "reset"
            if args
                .iter()
                .any(|a| a == "--hard" || a == "--merge" || a == "--keep") =>
        {
            Some("Discards uncommitted changes.")
        }
        "clean" if has_flag(args, 'f', "--force") => Some("Deletes untracked files."),
        "push"
            if has_flag(args, 'f', "--force")
                || args.iter().any(|a| {
                    a.starts_with("--force")
                        || a.starts_with('+')
                        || a == "--delete"
                        || a == "--mirror"
                }) =>
        {
            Some("Rewrites or deletes history on the remote.")
        }
        "push" => Some("Sends commits to a remote repository."),
        "checkout"
            if args
                .iter()
                .any(|a| a == "--" || a == "." || a == "-f" || a == "--force") =>
        {
            Some("Discards uncommitted changes.")
        }
        "restore" => Some("Discards uncommitted changes."),
        "stash" if args.first().is_some_and(|a| a == "drop" || a == "clear") => {
            Some("Deletes stashed changes.")
        }
        "branch" if has_flag(args, 'd', "--delete") => Some("Deletes a branch."),
        "rebase" | "filter-branch" | "filter-repo" => Some("Rewrites commit history."),
        "commit" if args.iter().any(|a| a == "--amend") => Some("Rewrites the last commit."),
        "prune" => Some("Permanently removes unreachable history."),
        "reflog" if args.first().is_some_and(|a| a == "expire" || a == "delete") => {
            Some("Permanently removes unreachable history.")
        }
        "gc" if args.iter().any(|a| a.starts_with("--prune")) => {
            Some("Permanently removes unreachable history.")
        }
        _ => None,
    }
}

// ── Undo: the Code branch ──────────────────────────────────────────────────

/// The code folder's git state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitState {
    /// Not inside a git work tree (or git is not installed).
    NotRepo,
    Repo {
        /// The checked-out branch; `None` when HEAD is detached.
        branch: Option<String>,
        /// The commit HEAD points at; `None` before the first commit.
        head: Option<String>,
        /// Uncommitted changes, untracked files included.
        dirty: bool,
    },
    /// A git repository whose state git could not report (its message).
    Unreadable(String),
}

/// The branch a conversation's Code work happens on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeBranch {
    pub folder: String,
    pub branch: String,
    /// What was checked out before: a branch name, or a commit when HEAD
    /// was detached.
    pub base: String,
    #[serde(default)]
    pub base_is_commit: bool,
    pub created_at: String,
    /// Files edits and writes changed without asking, relative to the folder
    /// (`/`-separated): "Discard changes" commits them even if a later
    /// `.gitignore` change hid them from git.
    #[serde(default)]
    pub auto_applied: Vec<String>,
}

/// What must happen before a guarded change may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangePlan {
    /// The conversation's branch is checked out already.
    Continue(CodeBranch),
    /// Switch to a new branch first (after the user approves).
    CreateBranch,
    /// No git: the change cannot be undone; the prompt says so.
    NotRepo,
    /// The change may not run; the reason goes to the model and the user.
    Refuse(String),
}

/// Decide what the next change needs. `recorded` is the conversation's
/// branch, if it has one in this folder.
pub fn plan_change(state: &GitState, recorded: Option<&CodeBranch>, folder: &str) -> ChangePlan {
    match state {
        GitState::NotRepo => ChangePlan::NotRepo,
        GitState::Unreadable(error) => ChangePlan::Refuse(format!(
            "Shodh could not read the code folder's git state, so it changes nothing. \
             git said: {error}"
        )),
        GitState::Repo {
            branch,
            head,
            dirty,
        } => {
            if let (Some(recorded), Some(current)) = (recorded, branch) {
                if recorded.folder == folder && &recorded.branch == current {
                    return ChangePlan::Continue(recorded.clone());
                }
            }
            if head.is_none() {
                return ChangePlan::Refuse(
                    "The code folder's git repository has no commits yet. Commit once so \
                     Shodh's changes can be undone, then ask again."
                        .to_string(),
                );
            }
            if *dirty {
                return ChangePlan::Refuse(
                    "The code folder has uncommitted changes. Commit or stash them first, so \
                     Shodh's changes stay separate and can be undone, then ask again."
                        .to_string(),
                );
            }
            ChangePlan::CreateBranch
        }
    }
}

/// The empty folder Shodh's git commands take their hooks from, so no hook
/// of the repository (or of a `core.hooksPath` in its config) ever runs.
/// Created on first use; `Err` when it cannot be, or has files in it.
pub(crate) fn empty_hooks_dir() -> Result<PathBuf, String> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    let dir = DIR
        .get_or_init(|| dirs::data_local_dir().map(|base| base.join("shodh").join("git-no-hooks")))
        .clone()
        .ok_or_else(|| "The app data folder for git is not known.".to_string())?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("{} could not be created: {e}", dir.display()))?;
    let mut entries =
        std::fs::read_dir(&dir).map_err(|e| format!("{} could not be read: {e}", dir.display()))?;
    if entries.next().is_some() {
        return Err(format!(
            "{} must be empty (Shodh runs git without hooks); remove what is in it.",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Configuration every git command of Shodh's runs with: no hooks, no file
/// system monitor (a `core.fsmonitor` command in the repository's config
/// runs on every `git status`) and no untracked cache.
fn hardening(hooks: &Path) -> [String; 3] {
    [
        format!("core.hooksPath={}", hooks.display()),
        "core.fsmonitor=false".to_string(),
        "core.untrackedCache=false".to_string(),
    ]
}

/// git in `folder` with [`hardening`]; [`run_git`] also switches content
/// filters off.
pub(crate) fn git(folder: &Path) -> Result<Command, String> {
    let hooks = empty_hooks_dir()?;
    let mut command = Command::new("git");
    command.arg("-C").arg(folder);
    for setting in hardening(&hooks) {
        command.arg("-c").arg(setting);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

/// git commands that may pass file contents through the repository's clean
/// or smudge filters (programs its config names).
const FILTERED_COMMANDS: [&str; 5] = ["status", "add", "commit", "checkout", "diff"];

/// The git subcommand of `args`: the first word after global `-c key=value`
/// and `-C dir` options.
fn git_subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut rest = args;
    while let [flag, _, tail @ ..] = rest {
        if *flag != "-c" && *flag != "-C" {
            break;
        }
        rest = tail;
    }
    rest.first().copied()
}

/// git config, read without Shodh's `-c` settings (reading config runs no
/// program). Used only for `git config` reads.
fn config_git(folder: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(folder)
        .arg("config")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `git config <args>`: its output, empty when nothing matched (exit 1),
/// `Err` with git's message otherwise.
fn config_values(folder: &Path, args: &[&str]) -> Result<String, String> {
    let output = config_git(folder)
        .args(args)
        .output()
        .map_err(|e| format!("git could not be started: {e}"))?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Some(1) => Ok(String::new()),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// The filter names in `git config --name-only --get-regexp '^filter\.'`
/// output (`filter.<name>.<key>` per line).
fn repository_filter_names(output: &str) -> Vec<String> {
    let mut names: Vec<String> = output
        .lines()
        .filter_map(|key| key.trim().strip_prefix("filter."))
        .filter_map(|rest| rest.rsplit_once('.').map(|(name, _)| name.to_string()))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// `-c` settings that switch off every content filter the repository's own
/// config names (`.git/config` with its includes, and the work tree's config
/// when `extensions.worktreeConfig` is on): an empty command is no filter,
/// and a filter marked `required` would then fail the command. The user's
/// and the system's filters (Git LFS) are theirs and keep working.
fn filters_off(folder: &Path) -> Result<Vec<String>, String> {
    const LIST: [&str; 4] = ["--includes", "--name-only", "--get-regexp", r"^filter\."];
    let mut listed = config_values(folder, &[&["--local"][..], &LIST[..]].concat())?;
    let worktree = config_values(
        folder,
        &["--local", "--bool", "--get", "extensions.worktreeConfig"],
    )?;
    if worktree.trim() == "true" {
        listed.push_str(&config_values(
            folder,
            &[&["--worktree"][..], &LIST[..]].concat(),
        )?);
    }
    let names = repository_filter_names(&listed);
    Ok(names
        .iter()
        .flat_map(|name| {
            ["clean=", "smudge=", "process=", "required=false"]
                .map(|setting| format!("filter.{name}.{setting}"))
        })
        .collect())
}

/// Run git; `Ok(stdout)` on success, `Err(stderr)` otherwise.
fn run_git(folder: &Path, args: &[&str]) -> Result<String, String> {
    run_git_raw(folder, args).map(|out| String::from_utf8_lossy(&out).trim().to_string())
}

/// The folder's git state (blocking).
pub fn git_state(folder: &Path) -> GitState {
    match run_git(folder, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(out) if out == "true" => {}
        _ => return GitState::NotRepo,
    }
    let branch = run_git(folder, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let head = run_git(folder, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok();
    let status = run_git(
        folder,
        &["status", "--porcelain", "--untracked-files=normal"],
    );
    repo_state(branch, head, status)
}

/// The state of a repository from its branch, HEAD and `git status`
/// output. A status git could not give is reported as such, never guessed.
fn repo_state(
    branch: Option<String>,
    head: Option<String>,
    status: Result<String, String>,
) -> GitState {
    match status {
        Ok(out) => GitState::Repo {
            branch,
            head,
            dirty: !out.is_empty(),
        },
        Err(e) => GitState::Unreadable(if e.trim().is_empty() {
            "git status failed without saying why".to_string()
        } else {
            e
        }),
    }
}

/// Keep `pattern` out of git for the repository holding `folder`, through
/// its local exclude file (`info/exclude`, never committed or shared), so
/// files a tool keeps there (enola's `.enola/` index) neither make the folder
/// look changed nor end up in "Discard changes" commits (blocking). Returns
/// whether the file was changed; a folder outside git is left alone.
pub fn exclude_locally(folder: &Path, pattern: &str) -> Result<bool, String> {
    if pattern.trim().is_empty() || pattern.contains(['\n', '\r']) {
        return Err("invalid exclude pattern".to_string());
    }
    if git_state(folder) == GitState::NotRepo {
        return Ok(false);
    }
    // `.git` is a file in linked worktrees: ask git where the file lives.
    let listed = run_git(folder, &["rev-parse", "--git-path", "info/exclude"])?;
    let path = folder.join(listed.trim());
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{} could not be read: {e}", path.display())),
    };
    if current.lines().any(|line| line.trim() == pattern) {
        return Ok(false);
    }
    let newline = if current.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(newline);
    }
    text.push_str(pattern);
    text.push_str(newline);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("{} could not be created: {e}", dir.display()))?;
    }
    std::fs::write(&path, text)
        .map_err(|e| format!("{} could not be written: {e}", path.display()))?;
    Ok(true)
}

fn branch_exists(folder: &Path, name: &str) -> bool {
    run_git(
        folder,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
}

/// Create and check out a new `shodh/<id>` branch from what is checked out
/// now (blocking). The state must have been planned as
/// [`ChangePlan::CreateBranch`].
pub fn create_branch(folder: &CodeFolder, now_rfc3339: &str) -> Result<CodeBranch, String> {
    let root = folder.root();
    let (base, base_is_commit) = match git_state(root) {
        GitState::Repo {
            branch: Some(branch),
            head: Some(_),
            dirty: false,
        } => (branch, false),
        GitState::Repo {
            branch: None,
            head: Some(head),
            dirty: false,
        } => (head, true),
        _ => return Err("The code folder changed; ask again.".to_string()),
    };
    let name = (0..8)
        .map(|_| {
            let id = uuid::Uuid::new_v4().simple().to_string();
            format!("{BRANCH_PREFIX}{}", &id[..8])
        })
        .find(|name| !branch_exists(root, name))
        .ok_or_else(|| "No free branch name was found.".to_string())?;
    run_git(root, &["checkout", "-b", &name])
        .map_err(|e| format!("Switching to a new branch failed: {e}"))?;
    Ok(CodeBranch {
        folder: folder.display(),
        branch: name,
        base,
        base_is_commit,
        created_at: now_rfc3339.to_string(),
        auto_applied: Vec::new(),
    })
}

/// Leave the Code branch: commit whatever Shodh changed on it (kept there for
/// inspection) and check out the original branch again (blocking).
pub fn discard_changes(record: &CodeBranch) -> Result<(), String> {
    let root = Path::new(&record.folder);
    match git_state(root) {
        GitState::Repo {
            branch: Some(current),
            dirty,
            ..
        } if current == record.branch => {
            // Files changed without asking go into the commit even when git
            // now ignores them, so checking out the base removes or restores
            // them like every other change.
            let auto: Vec<&str> = record
                .auto_applied
                .iter()
                .map(String::as_str)
                .filter(|p| root.join(p).symlink_metadata().is_ok())
                .collect();
            let dirty = if auto.is_empty() {
                dirty
            } else {
                let mut add = vec!["add", "--force", "--"];
                add.extend(&auto);
                run_git(root, &add)?;
                !run_git(root, &["status", "--porcelain", "--untracked-files=normal"])?.is_empty()
            };
            if dirty {
                run_git(root, &["add", "--all"])?;
                // A snapshot of the agent's work on its own branch. No hook
                // runs (`--no-verify` alone skips only pre-commit and
                // commit-msg; [`git`] points hooks at an empty folder), and
                // it is not signed.
                run_git(
                    root,
                    &[
                        "-c",
                        "user.name=Shodh",
                        "-c",
                        "user.email=shodh@localhost",
                        "-c",
                        "commit.gpgsign=false",
                        "commit",
                        "--no-verify",
                        "--quiet",
                        "-m",
                        "Shodh: changes discarded from a Code conversation",
                    ],
                )?;
            }
            if record.base_is_commit {
                run_git(root, &["checkout", "--quiet", "--detach", &record.base])?;
            } else {
                run_git(root, &["checkout", "--quiet", &record.base])?;
            }
            Ok(())
        }
        GitState::Repo {
            branch: Some(current),
            ..
        } => Err(format!(
            "The code folder is on {current}, not {}; nothing was changed.",
            record.branch
        )),
        GitState::Repo { branch: None, .. } => Err(format!(
            "The code folder is not on {}; nothing was changed.",
            record.branch
        )),
        GitState::NotRepo => Err("The code folder is no longer a git repository.".to_string()),
        GitState::Unreadable(error) => Err(format!(
            "Shodh could not read the code folder's git state, so nothing was changed. \
             git said: {error}"
        )),
    }
}

/// Code branches by conversation, in one JSON file.
#[derive(Debug, Clone)]
pub struct CodeBranchStore {
    path: PathBuf,
}

static STORE_LOCK: Mutex<()> = Mutex::new(());

impl CodeBranchStore {
    pub const FILE: &'static str = "code_branches.json";

    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(Self::FILE),
        }
    }

    fn read(&self) -> Result<HashMap<String, CodeBranch>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} is not readable: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(format!("{} is not readable: {e}", self.path.display())),
        }
    }

    fn write(&self, records: &HashMap<String, CodeBranch>) -> Result<(), String> {
        let text = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn get(&self, conversation_id: &str) -> Result<Option<CodeBranch>, String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read()?.remove(conversation_id))
    }

    /// Every conversation's branch, the newest first (the Artifacts tab's
    /// "Code changes").
    pub fn list(&self) -> Result<Vec<(String, CodeBranch)>, String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut records: Vec<(String, CodeBranch)> = self.read()?.into_iter().collect();
        records.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
        Ok(records)
    }

    pub fn put(&self, conversation_id: &str, record: CodeBranch) -> Result<(), String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut records = self.read()?;
        records.insert(conversation_id.to_string(), record);
        self.write(&records)
    }

    pub fn remove(&self, conversation_id: &str) -> Result<Option<CodeBranch>, String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut records = self.read()?;
        let removed = records.remove(conversation_id);
        if removed.is_some() {
            self.write(&records)?;
        }
        Ok(removed)
    }
}

/// [`ReviewState`]s by conversation, in one JSON file beside the branch
/// records. Kept apart from them: a folder outside git has no branch, and
/// "Discard changes" removing a branch is no review.
#[derive(Debug, Clone)]
pub struct ReviewStore {
    path: PathBuf,
}

static REVIEW_LOCK: Mutex<()> = Mutex::new(());

impl ReviewStore {
    pub const FILE: &'static str = "code_reviews.json";

    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(Self::FILE),
        }
    }

    /// The store in the folder of `branches`.
    pub fn beside(branches: &CodeBranchStore) -> Self {
        Self {
            path: branches.path.with_file_name(Self::FILE),
        }
    }

    fn read(&self) -> Result<HashMap<String, ReviewState>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} is not readable: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(format!("{} is not readable: {e}", self.path.display())),
        }
    }

    fn write(&self, records: &HashMap<String, ReviewState>) -> Result<(), String> {
        let text = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }

    /// The key of a code folder's record: what is unreviewed belongs to the
    /// folder, which every conversation working in it shares.
    pub fn folder_key(folder: &CodeFolder) -> String {
        format!("folder:{}", folder.display())
    }

    /// Fold the record an earlier version kept under the conversation
    /// `from` into `into` (the folder's), so nothing it listed is forgotten.
    pub fn adopt(&self, from: &str, into: &str) -> Result<(), String> {
        let _guard = REVIEW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut records = self.read()?;
        let Some(old) = records.remove(from) else {
            return Ok(());
        };
        records.entry(into.to_string()).or_default().merge(old);
        self.write(&records)
    }

    /// The conversation's state; clear when it has none.
    pub fn get(&self, conversation_id: &str) -> Result<ReviewState, String> {
        let _guard = REVIEW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read()?.remove(conversation_id).unwrap_or_default())
    }

    /// Change the conversation's state with `change` and save it; returns
    /// the saved state.
    pub fn update<F: FnOnce(&mut ReviewState)>(
        &self,
        conversation_id: &str,
        change: F,
    ) -> Result<ReviewState, String> {
        let _guard = REVIEW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut records = self.read()?;
        let mut state = records.remove(conversation_id).unwrap_or_default();
        change(&mut state);
        if state != ReviewState::default() {
            records.insert(conversation_id.to_string(), state.clone());
        }
        self.write(&records)?;
        Ok(state)
    }
}

// ── What commands and tools change ─────────────────────────────────────────

/// Most changed files a snapshot compares; past it, what changed is unknown.
const MAX_SNAPSHOT_FILES: usize = 20_000;

/// What git shows of the code folder: the commit checked out and every
/// changed or untracked file under the folder (files git ignores are not
/// shown), each with its status and a hash of its content. Two snapshots
/// around a command name what it changed ([`changed_between`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSnapshot {
    head: Option<String>,
    /// The folder's path in the repository (`git rev-parse --show-prefix`).
    prefix: String,
    /// Folder-relative path (`/`-separated) → status and content hash.
    files: std::collections::BTreeMap<String, String>,
}

/// Run git; `Ok(stdout)` untouched on success, `Err(stderr)` otherwise.
fn run_git_raw(folder: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = git(folder)?;
    if git_subcommand(args).is_some_and(|a| FILTERED_COMMANDS.contains(&a)) {
        for setting in filters_off(folder)? {
            command.arg("-c").arg(setting);
        }
    }
    let output = command
        .args(args)
        .output()
        .map_err(|e| format!("git could not be started: {e}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            stderr
        })
    }
}

/// A hash of what is at `path` now: a file's bytes, a link's target, or
/// that it is a folder or missing.
fn content_hash(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    match std::fs::symlink_metadata(path) {
        Err(_) => "missing".to_string(),
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(path) {
            Ok(target) => format!("link {}", target.display()),
            Err(e) => format!("unreadable link {e}"),
        },
        Ok(meta) if meta.is_dir() => "folder".to_string(),
        // A large file (a dataset, a build output) is told apart by its size
        // and modification time, so a snapshot stays fast.
        Ok(meta) if meta.len() > MAX_HASHED_BYTES => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos());
            format!("{} bytes, modified {modified}", meta.len())
        }
        Ok(_) => {
            let hashed = std::fs::File::open(path).and_then(|mut file| {
                let mut hasher = Sha256::new();
                std::io::copy(&mut file, &mut hasher)?;
                Ok(hex::encode(hasher.finalize()))
            });
            hashed.unwrap_or_else(|e| format!("unreadable {e}"))
        }
    }
}

/// Largest file whose content a snapshot hashes ([`content_hash`]).
const MAX_HASHED_BYTES: u64 = 16 * 1024 * 1024;

/// The NUL-separated paths in `raw`, relative to the folder whose
/// repository path is `prefix` (paths outside it are left out).
fn folder_paths(raw: &[u8], prefix: &str) -> Vec<String> {
    raw.split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let path = String::from_utf8_lossy(entry);
            path.strip_prefix(prefix)
                .map(|rest| rest.trim_end_matches('/').to_string())
        })
        .filter(|path| !path.is_empty())
        .collect()
}

/// What git shows of `folder` now (blocking). `Err` outside git, or when
/// git cannot tell: then what a command changes is unknown.
pub fn tree_snapshot(folder: &Path) -> Result<TreeSnapshot, String> {
    match run_git(folder, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(out) if out == "true" => {}
        _ => {
            return Err(
                "the code folder is not a git repository, so Shodh cannot see what a command \
                 or tool changed"
                    .to_string(),
            )
        }
    }
    let prefix = run_git(folder, &["rev-parse", "--show-prefix"])?;
    let top = PathBuf::from(run_git(folder, &["rev-parse", "--show-toplevel"])?);
    let head = run_git(folder, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok();
    let raw = run_git_raw(
        folder,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--",
            ".",
        ],
    )?;
    let mut files = std::collections::BTreeMap::new();
    for entry in raw.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        if entry.len() < 4 || entry[2] != b' ' {
            return Err("git status gave a line Shodh cannot read".to_string());
        }
        let status = String::from_utf8_lossy(&entry[..2]).to_string();
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        let Some(relative) = path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let relative = relative.trim_end_matches('/');
        if relative.is_empty() {
            continue;
        }
        if files.len() >= MAX_SNAPSHOT_FILES {
            return Err(format!(
                "more than {MAX_SNAPSHOT_FILES} files are changed or untracked, too many to compare"
            ));
        }
        files.insert(
            relative.to_string(),
            format!("{status} {}", content_hash(&top.join(&path))),
        );
    }
    Ok(TreeSnapshot {
        head,
        prefix,
        files,
    })
}

/// The files of `folder` that changed between two snapshots (blocking):
/// every file whose status or content differs, and when the checked-out
/// commit moved, every file that differs between the two commits.
pub fn changed_between(
    folder: &Path,
    before: &TreeSnapshot,
    after: &TreeSnapshot,
) -> Result<Vec<String>, String> {
    let mut changed: Vec<String> = before
        .files
        .keys()
        .chain(after.files.keys())
        .filter(|path| before.files.get(*path) != after.files.get(*path))
        .cloned()
        .collect();
    if before.head != after.head {
        let (Some(from), Some(to)) = (&before.head, &after.head) else {
            return Err(
                "the repository's commits changed in a way Shodh cannot compare".to_string(),
            );
        };
        let raw = run_git_raw(
            folder,
            &[
                "diff",
                "--no-ext-diff",
                "--name-only",
                "-z",
                "--no-renames",
                from.as_str(),
                to.as_str(),
                "--",
                ".",
            ],
        )?;
        changed.extend(folder_paths(&raw, &after.prefix));
    }
    changed.sort_unstable();
    changed.dedup();
    Ok(changed)
}

#[cfg(test)]
pub(crate) mod tests;
