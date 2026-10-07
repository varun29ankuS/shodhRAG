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
//! - The hook asks for approval of every edit, write and shell command
//!   through omp's dialog channel (an `input` request titled
//!   [`APPROVAL_TITLE`]); the session maps it to `ApprovalRequested` and
//!   answers with [`ALLOW_ANSWER`] or the reason it was refused.
//! - Shell commands cannot be confined by path; each one needs approval and
//!   destructive ones carry a warning ([`destructive_reason`]).
//! - Undo: before the first change of a session in a git repository the
//!   session switches to a new branch `shodh/<id>` ([`plan_change`],
//!   [`create_branch`]); "discard changes" commits the work on that branch
//!   and returns to the original one ([`discard_changes`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::error::HarnessError;
use super::events::RiskTier;
use super::omp::StepMeta;
use super::truncate_chars;

/// The tools a Code session has, exactly (omp names). `find` in omp 18.4.10
/// is `glob`; `lsp` needs language servers in the isolated home and is off.
pub const CODE_TOOLS: [&str; 7] = ["read", "grep", "glob", "ast_grep", "edit", "write", "bash"];

/// Tools that change files or run commands; the guard asks for each call.
pub const CHANGE_TOOLS: [&str; 3] = ["edit", "write", "bash"];

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

/// Pins omp's edit tool to single-file replace edits (`path`, `old_string`,
/// `new_string`), whose target the guard can check. The default hashline
/// patch format can name several files and move them.
pub const EDIT_VARIANT_ENV: (&str, &str) = ("PI_EDIT_VARIANT", "replace");

/// Prefix of the branches Code mode creates.
pub const BRANCH_PREFIX: &str = "shodh/";

const MAX_PREVIEW_CHARS: usize = 2_000;
const MAX_LABEL_CHARS: usize = 80;

/// Discovery sources whose project files (settings, hooks, MCP servers,
/// context files) are ignored in Code sessions.
const DISABLED_DISCOVERY: [&str; 10] = [
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

/// The second `--config` overlay of a Code session (written as JSON, which
/// YAML accepts). Overlays outrank the code folder's own `.omp` settings.
pub fn code_overlay_config() -> Value {
    // The guard is the approval gate; omp's own prompts would ask twice.
    let approvals: serde_json::Map<String, Value> = CODE_TOOLS
        .iter()
        .map(|tool| (tool.to_string(), json!("allow")))
        .collect();
    json!({
        "tools": { "approvalMode": "always-ask", "approval": approvals },
        "edit": { "mode": "replace" },
        "grep": { "enabled": true },
        "glob": { "enabled": true },
        "astGrep": { "enabled": true },
        "bash": { "enabled": true, "patterns": [] },
        "bashInterceptor": { "enabled": false },
        "lsp": { "enabled": false },
        "workspace": { "additionalDirectories": [] },
        "enabledProviders": [],
        "disabledProviders": DISABLED_DISCOVERY,
    })
}

/// Launch flags of a Code session. Unlike Research sessions, omp's dialog
/// channel stays on (no `--no-ui`): approvals travel over it.
pub fn code_launch_args(
    model_arg: &str,
    base_overlay: &Path,
    code_overlay: &Path,
    guard_hook: &Path,
    system_prompt: &str,
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
        "off",
        "--approval-mode=always-ask",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(format!("--tools={}", CODE_TOOLS.join(",")));
    args.push(format!("--hook={}", guard_hook.display()));
    args.push(format!("--model={model_arg}"));
    args.push(format!("--config={}", base_overlay.display()));
    args.push(format!("--config={}", code_overlay.display()));
    args.push(format!("--system-prompt={system_prompt}"));
    args
}

/// The system prompt of a Code session. `instructions` are the
/// conversation's own (they never override the rules).
pub fn code_system_prompt(folder: &CodeFolder, instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "You are Shodh's coding assistant, working in the code folder {folder}.\n\
         \n\
         Tools: read (files), grep (text search), glob (find files), ast_grep (syntax-aware \
         search), edit (replace old_string with new_string in one file), write (create or \
         replace a whole file) and bash (run one shell command in the code folder).\n\
         \n\
         Rules:\n\
         - Work only inside the code folder. Paths outside it, URLs and other resources are \
         blocked.\n\
         - Every edit, write and command waits for the user's approval. If one is declined, do \
         not try it another way: say what you wanted to do and ask how to continue.\n\
         - Read the relevant code before changing it, keep changes small and focused, and say \
         what you changed and how to check it.\n\
         - Prefer edit for changes to existing files; use write for new files.\n\
         - Avoid destructive commands (deleting files, resetting or rewriting git history, \
         force-pushing). Never commit or push unless the user asks.",
        folder = folder.display()
    );
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
        ("bash", "Running {command}", RiskTier::Write),
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

/// Whether `tools` (a session's tool inventory) is exactly [`CODE_TOOLS`].
pub fn inventory_matches(tools: &[String]) -> bool {
    let mut have: Vec<&str> = tools.iter().map(String::as_str).collect();
    have.sort_unstable();
    have.dedup();
    let mut want: Vec<&str> = CODE_TOOLS.to_vec();
    want.sort_unstable();
    have.len() == tools.len() && have == want
}

/// A guarded call waiting for the user's decision.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardRequest {
    /// The dialog id omp expects the answer for.
    pub ui_id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub input: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuardPayload {
    tool_call_id: String,
    tool: String,
    #[serde(default)]
    input: Value,
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
        ui_id: ui_id.to_string(),
        tool_call_id: parsed.tool_call_id,
        tool: parsed.tool,
        input: parsed.input,
    })
}

/// What an approval prompt shows for a guarded call.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeApproval {
    pub label: String,
    pub tier: RiskTier,
    pub preview: Value,
}

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
/// user wants it back (a branch is created first, or nothing can undo it).
pub fn approval_for(request: &GuardRequest, undo: Option<&str>) -> CodeApproval {
    let input = &request.input;
    let (label, tier, mut preview) = match request.tool.as_str() {
        "write" => {
            let path = str_arg(input, "path");
            (
                format!("Write {}", one_line(path, MAX_LABEL_CHARS)),
                RiskTier::Write,
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
                RiskTier::Write,
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
            let folder = input
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .unwrap_or("the code folder");
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
                    "runs_in": folder,
                }),
            )
        }
    };
    if let (Some(undo), Some(map)) = (undo, preview.as_object_mut()) {
        map.insert("undo".to_string(), json!(undo));
    }
    CodeApproval {
        label,
        tier,
        preview,
    }
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

fn git(folder: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(folder)
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

/// Run git; `Ok(stdout)` on success, `Err(stderr)` otherwise.
fn run_git(folder: &Path, args: &[&str]) -> Result<String, String> {
    let output = git(folder)
        .args(args)
        .output()
        .map_err(|e| format!("git could not be started: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            stderr
        })
    }
}

/// The folder's git state (blocking).
pub fn git_state(folder: &Path) -> GitState {
    match run_git(folder, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(out) if out == "true" => {}
        _ => return GitState::NotRepo,
    }
    let branch = run_git(folder, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let head = run_git(folder, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok();
    let dirty = run_git(
        folder,
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .map(|out| !out.is_empty())
    // Unknown status counts as dirty: never risk mixing changes.
    .unwrap_or(true);
    GitState::Repo {
        branch,
        head,
        dirty,
    }
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
            if dirty {
                run_git(root, &["add", "--all"])?;
                // A snapshot of the agent's work on its own branch: the user's
                // commit hooks and signing do not apply to it.
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

#[cfg(test)]
mod tests;
