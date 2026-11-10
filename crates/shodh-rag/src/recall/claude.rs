//! Claude Code transcripts: `~/.claude/projects/<project>/<session>.jsonl`, and a
//! subagent's thread at `<project>/<session>/subagents/agent-<id>.jsonl`.
//!
//! One JSON object per line, discriminated by `type`. What Recall reads:
//! - `user`: `message.content` is a string (a prompt) or an array of blocks: `text`
//!   (a prompt) and `tool_result` (`tool_use_id`, `content` as a string or an array of
//!   `text` / `image` blocks, `is_error`). Lines with `isMeta` (caveats the CLI adds) and
//!   `isCompactSummary` (a summary that repeats the session) are passed over.
//! - `assistant`: `message.content` blocks: `text`, `tool_use` and `server_tool_use`
//!   (`id`, `name`, `input`); `thinking` and `redacted_thinking` are never read.
//!   `message.model` names the model.
//! - `ai-title` (`aiTitle`), `custom-title` (`customTitle`), `summary` (`summary`, older
//!   versions), `agent-name` (`agentName`), `pr-link` (`prNumber`, `prUrl`).
//! - every line: `sessionId`, `cwd`, `gitBranch`, `version`, `timestamp`, `agentId`.
//!
//! Everything else (`api-request*`, which repeat whole requests, `attachment`,
//! `file-history-*`, `system`, `queue-operation`, mode and permission lines, unknown
//! kinds) is counted as skipped. `toolUseResult` repeats the tool output and is not read.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::lines::{for_each_line, Line};
use super::redact::Redactor;
use super::{
    keep_text, parse_time, Format, LineError, ParseBudget, Role, SessionInfo, Tool, Transcript,
    Turn,
};

/// What a call and its output kept back say.
pub const SECRET_FILE_KEPT_BACK: &str =
    "Not indexed: the tool read or wrote a file that may hold secrets (keys, tokens, passwords), or ran a command that prints them.";

/// A tool call, remembered until its output arrives.
struct Call {
    name: String,
    /// The call and its output are kept back.
    kept_back: bool,
}

/// Reads a Claude Code transcript.
pub fn read(path: &Path, redactor: &Redactor, budget: ParseBudget) -> std::io::Result<Transcript> {
    let mut reader = Reader {
        redactor,
        budget,
        info: SessionInfo::default(),
        turns: Vec::new(),
        calls: HashMap::new(),
        skipped: 0,
        redacted: 0,
        custom_title: None,
        ai_title: None,
        summary_title: None,
        line: 0,
        offset: 0,
    };
    let mut errors: Vec<LineError> = Vec::new();
    let mut error_count = 0usize;
    for_each_line(
        path,
        redactor,
        budget,
        &mut errors,
        &mut error_count,
        |line| reader.line(line),
    )?;
    let Reader {
        mut info,
        turns,
        skipped,
        redacted,
        custom_title,
        ai_title,
        summary_title,
        ..
    } = reader;
    info.title = custom_title.or(ai_title).or(summary_title);
    if info.session_id.is_none() {
        info.session_id = session_from_path(path);
    }
    if info.thread.is_none() {
        info.thread = thread_from_path(path);
    }
    Ok(Transcript {
        tool: Tool::ClaudeCode,
        format: Format::ClaudeCodeJsonl,
        info,
        turns,
        errors,
        skipped,
        redacted,
    })
}

/// The session a file belongs to, from its path: `<session>.jsonl`, or
/// `<session>/subagents/agent-<id>.jsonl` for a subagent's thread.
pub fn session_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem.starts_with("agent-") {
        let subagents = path.parent()?;
        if subagents.file_name()?.to_str()? == "subagents" {
            return Some(subagents.parent()?.file_name()?.to_str()?.to_string());
        }
    }
    Some(stem.to_string())
}

/// A subagent's id, from a `subagents/agent-<id>.jsonl` path.
pub fn thread_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let parent = path.parent()?.file_name()?.to_str()?;
    (parent == "subagents").then(|| stem.strip_prefix("agent-").unwrap_or(stem).to_string())
}

struct Reader<'a> {
    redactor: &'a Redactor,
    budget: ParseBudget,
    info: SessionInfo,
    turns: Vec<Turn>,
    calls: HashMap<String, Call>,
    skipped: usize,
    redacted: usize,
    custom_title: Option<String>,
    ai_title: Option<String>,
    summary_title: Option<String>,
    /// The line being read.
    line: u64,
    offset: u64,
}

fn string<'v>(value: &'v Value, key: &str) -> Option<&'v str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// Records the folder a session worked in (its key chain, made from the folder as
/// written, never shown).
pub(crate) fn note_cwd(info: &mut SessionInfo, cwd: &str) {
    let chain = super::discover::project_chain(cwd);
    if !info.cwd_chains.contains(&chain) {
        info.cwd_chains.push(chain);
    }
}

impl Reader<'_> {
    fn clean(&mut self, text: &str) -> String {
        let (out, count) = self.redactor.redact_counted(text);
        self.redacted += count;
        out
    }

    /// Reads one line; `false` once the turns asked for are read.
    fn line(&mut self, line: Line) -> bool {
        self.line = line.number;
        self.offset = line.offset;
        let value = line.value;
        let Some(kind) = string(&value, "type").map(str::to_string) else {
            self.skipped += 1;
            return true;
        };
        self.session_fields(&value);
        match kind.as_str() {
            "user" => self.user(&value),
            "assistant" => self.assistant(&value),
            "ai-title" => self.ai_title = string(&value, "aiTitle").map(|t| self.clean(t)),
            "custom-title" => {
                self.custom_title = string(&value, "customTitle").map(|t| self.clean(t))
            }
            "summary" => {
                if self.summary_title.is_none() {
                    self.summary_title = string(&value, "summary").map(|t| self.clean(t));
                }
            }
            "agent-name" => {
                if self.ai_title.is_none() {
                    self.ai_title = string(&value, "agentName").map(|t| self.clean(t));
                }
            }
            "pr-link" => {
                let number = value.get("prNumber").and_then(Value::as_u64);
                let url = string(&value, "prUrl").map(|u| self.clean(u));
                if let Some(n) = number {
                    self.info.pull_requests.push(format!("#{n}"));
                }
                if let Some(url) = url {
                    self.info.pull_requests.push(url);
                }
            }
            _ => self.skipped += 1,
        }
        self.turns.len() < self.budget.max_turns
    }

    /// The fields every line may carry.
    fn session_fields(&mut self, value: &Value) {
        if self.info.session_id.is_none() {
            if let Some(id) = string(value, "sessionId") {
                self.info.session_id = Some(self.clean(id));
            }
        }
        if self.info.thread.is_none()
            && value.get("isSidechain").and_then(Value::as_bool) == Some(true)
        {
            if let Some(agent) = string(value, "agentId") {
                self.info.thread = Some(self.clean(agent));
            }
        }
        if let Some(cwd) = string(value, "cwd") {
            note_cwd(&mut self.info, cwd);
            if self.info.cwd.is_none() {
                self.info.cwd = Some(self.clean(cwd));
            }
        }
        if let Some(branch) = string(value, "gitBranch") {
            if branch != "HEAD" {
                self.info.branch = Some(self.clean(branch));
            }
        }
        if let Some(version) = string(value, "version") {
            self.info.version = Some(self.clean(version));
        }
        if let Some(at) = string(value, "timestamp").and_then(parse_time) {
            self.info.started = Some(self.info.started.map_or(at, |s| s.min(at)));
            self.info.ended = Some(self.info.ended.map_or(at, |e| e.max(at)));
        }
    }

    fn push(
        &mut self,
        role: Role,
        at: Option<chrono::DateTime<chrono::Utc>>,
        tool_name: Option<String>,
        text: String,
        kept_back: Option<&'static str>,
    ) {
        if self.turns.len() >= self.budget.max_turns {
            self.skipped += 1;
            return;
        }
        if text.trim().is_empty() && kept_back.is_none() {
            return;
        }
        let (text, left_out) = keep_text(text);
        self.turns.push(Turn {
            index: self.budget.first_turn + self.turns.len() as u32,
            role,
            at,
            tool_name,
            text,
            line: self.line,
            offset: self.offset,
            left_out,
            kept_back,
        });
    }

    fn user(&mut self, value: &Value) {
        if value.get("isMeta").and_then(Value::as_bool) == Some(true)
            || value.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
        {
            self.skipped += 1;
            return;
        }
        let at = string(value, "timestamp").and_then(parse_time);
        let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
            self.skipped += 1;
            return;
        };
        match content {
            Value::String(text) => {
                let text = self.clean(text);
                self.push(Role::User, at, None, text, None);
            }
            Value::Array(blocks) => {
                for block in blocks {
                    match string(block, "type") {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                let text = self.clean(text);
                                self.push(Role::User, at, None, text, None);
                            }
                        }
                        Some("tool_result") => self.tool_result(block, at),
                        _ => {}
                    }
                }
            }
            _ => self.skipped += 1,
        }
    }

    fn tool_result(&mut self, block: &Value, at: Option<chrono::DateTime<chrono::Utc>>) {
        let call = string(block, "tool_use_id").and_then(|id| self.calls.remove(id));
        let name = call.as_ref().map(|c| c.name.clone());
        if call.as_ref().is_some_and(|c| c.kept_back) {
            self.push(
                Role::ToolOutput,
                at,
                name,
                String::new(),
                Some(SECRET_FILE_KEPT_BACK),
            );
            return;
        }
        let mut text = content_text(block.get("content"));
        if block.get("is_error").and_then(Value::as_bool) == Some(true) {
            text = format!("Error: {text}");
        }
        let text = self.clean(&text);
        self.push(Role::ToolOutput, at, name, text, None);
    }

    fn assistant(&mut self, value: &Value) {
        let at = string(value, "timestamp").and_then(parse_time);
        let Some(message) = value.get("message") else {
            self.skipped += 1;
            return;
        };
        if let Some(model) = string(message, "model") {
            if !model.starts_with('<') {
                self.info.model = Some(self.clean(model));
            }
        }
        let Some(blocks) = message.get("content") else {
            return;
        };
        let blocks: Vec<&Value> = match blocks {
            Value::Array(items) => items.iter().collect(),
            Value::String(_) => vec![blocks],
            _ => Vec::new(),
        };
        for block in blocks {
            if let Value::String(text) = block {
                let text = self.clean(text);
                self.push(Role::Assistant, at, None, text, None);
                continue;
            }
            match string(block, "type") {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        let text = self.clean(text);
                        self.push(Role::Assistant, at, None, text, None);
                    }
                }
                Some("tool_use") | Some("server_tool_use") => {
                    let name = string(block, "name").unwrap_or("tool").to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let kept_back = keeps_back(&name, &input);
                    if let Some(id) = string(block, "id") {
                        self.calls.insert(
                            id.to_string(),
                            Call {
                                name: name.clone(),
                                kept_back,
                            },
                        );
                    }
                    let shown_name = self.clean(&name);
                    if kept_back {
                        self.push(
                            Role::ToolCall,
                            at,
                            Some(shown_name),
                            String::new(),
                            Some(SECRET_FILE_KEPT_BACK),
                        );
                        continue;
                    }
                    let summary = super::clip(
                        &self.clean(&summarize_input(&input)),
                        MAX_CALL_SUMMARY_CHARS,
                    );
                    self.push(Role::ToolCall, at, Some(shown_name), summary, None);
                }
                _ => {}
            }
        }
    }
}

/// Every string of a JSON value as readable lines (`key: value`, nested keys joined with
/// dots), never as JSON text: escapes would hide line starts and word boundaries from
/// the redactor (`\nAKIA…`).
pub(crate) fn flatten_value(value: &Value, key: &str, out: &mut Vec<String>) {
    let child = |k: &str| {
        if key.is_empty() {
            k.to_string()
        } else {
            format!("{key}.{k}")
        }
    };
    match value {
        Value::Null => {}
        Value::String(s) => out.push(if key.is_empty() {
            s.clone()
        } else {
            format!("{key}: {s}")
        }),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                flatten_value(item, &child(&i.to_string()), out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                flatten_value(v, &child(k), out);
            }
        }
        other => out.push(if key.is_empty() {
            other.to_string()
        } else {
            format!("{key}: {other}")
        }),
    }
}

/// The text of a tool result's `content`: a string, or `text` blocks (images are named),
/// or any other shape as readable lines.
pub(crate) fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                _ => match string(item, "type") {
                    Some("text") => item.get("text").and_then(Value::as_str).map(str::to_string),
                    Some("image") => Some("[image]".to_string()),
                    _ => {
                        let mut lines = Vec::new();
                        flatten_value(item, "", &mut lines);
                        Some(lines.join("\n"))
                    }
                },
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => String::new(),
        Some(other) => {
            let mut lines = Vec::new();
            flatten_value(other, "", &mut lines);
            lines.join("\n")
        }
    }
}

/// Argument keys shown first in a tool call's summary, in this order.
const SUMMARY_KEYS: [&str; 12] = [
    "description",
    "command",
    "cmd",
    "file_path",
    "path",
    "notebook_path",
    "pattern",
    "url",
    "query",
    "prompt",
    "old_string",
    "new_string",
];

/// Characters of a tool call's summary at most.
pub const MAX_CALL_SUMMARY_CHARS: usize = 1200;

/// A tool call's arguments in a few readable lines: the known keys first (a command, a
/// file, a pattern), then every other string as `key: value`. Not yet redacted, and not
/// cut: it is cut only after redaction, so a cut never splits a secret into a part no
/// rule recognises.
pub(crate) fn summarize_input(input: &Value) -> String {
    let Value::Object(map) = input else {
        let mut lines = Vec::new();
        flatten_value(input, "", &mut lines);
        return lines.join("\n");
    };
    let mut lines = Vec::new();
    for key in SUMMARY_KEYS {
        if let Some(value) = map.get(key) {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Array(items) if items.iter().all(Value::is_string) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                other => {
                    let mut parts = Vec::new();
                    flatten_value(other, "", &mut parts);
                    parts.join("\n")
                }
            };
            if !text.trim().is_empty() {
                lines.push(format!("{key}: {text}"));
            }
        }
    }
    for (key, value) in map {
        if !SUMMARY_KEYS.contains(&key.as_str()) {
            flatten_value(value, key, &mut lines);
        }
    }
    lines.join("\n")
}

/// Whether a tool call, and its output, are kept back: it reads or writes a file that
/// may hold secrets (`.env`, keys, credentials: [`crate::code_review::secrets::secret_path`]),
/// or runs a command that names one or prints secrets (`printenv`, `gh auth token`).
pub(crate) fn keeps_back(_name: &str, input: &Value) -> bool {
    let mut strings = Vec::new();
    collect_paths(input, "", &mut strings);
    if strings
        .iter()
        .any(|path| crate::code_review::secrets::secret_path(path).is_some())
    {
        return true;
    }
    // A patch's files (`*** Update File: .env`).
    if let Value::String(patch) = input {
        if patch_names_secret_file(patch) {
            return true;
        }
    }
    if let Some(Value::String(patch)) = input.get("input").or_else(|| input.get("patch")) {
        if patch_names_secret_file(patch) {
            return true;
        }
    }
    ["command", "cmd", "script"]
        .iter()
        .filter_map(|key| input.get(*key))
        .any(|command| command_names_secret_file(command) || command_prints_secrets(command))
}

/// The values of every key that names a path or a file, at any depth.
fn collect_paths(value: &Value, key: &str, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            let k = key.to_ascii_lowercase();
            if k.contains("path") || k.contains("file") || k == "target" || k == "destination" {
                out.push(s.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_paths(item, key, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                collect_paths(v, k, out);
            }
        }
        _ => {}
    }
}

/// Whether a patch adds, updates, deletes or moves a file that may hold secrets.
pub(crate) fn patch_names_secret_file(patch: &str) -> bool {
    patch.lines().any(|line| {
        let line = line.trim();
        [
            "*** Add File:",
            "*** Update File:",
            "*** Delete File:",
            "*** Move to:",
            "+++ ",
            "--- ",
        ]
        .iter()
        .find_map(|p| line.strip_prefix(p))
        .map(|path| {
            path.trim()
                .trim_start_matches("a/")
                .trim_start_matches("b/")
        })
        .is_some_and(|path| crate::code_review::secrets::secret_path(path).is_some())
    })
}

fn command_texts(command: &Value) -> Vec<String> {
    match command {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether a command (a string, or an argument list) names a file that may hold secrets.
pub(crate) fn command_names_secret_file(command: &Value) -> bool {
    command_texts(command).iter().any(|text| {
        text.split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '`' | ';' | '|' | '&' | '<' | '>' | '(' | ')' | '='
                )
        })
        .filter(|w| !w.is_empty() && !w.starts_with('-'))
        .any(|w| crate::code_review::secrets::secret_path(w).is_some())
    })
}

/// Commands whose output is secrets, by their first words; a flag a command needs to
/// print them follows after a `+`.
const SECRET_COMMANDS: [&str; 26] = [
    "printenv",
    "gh auth token",
    "gh auth status +--show-token",
    "gh auth status +-t",
    "kubectl config view +--raw",
    "kubectl get secret",
    "kubectl get secrets",
    "docker inspect",
    "heroku config",
    "vercel env pull",
    "vercel env ls",
    "netlify env:list",
    "aws configure get",
    "aws configure export-credentials",
    "aws sts get-session-token",
    "aws ecr get-login-password",
    "gcloud auth print-access-token",
    "gcloud auth print-identity-token",
    "az account get-access-token",
    "op read",
    "op item get",
    "security find-generic-password",
    "security find-internet-password",
    "vault read",
    "vault kv get",
    "doppler secrets",
];

/// Whether a command prints secrets: `env` or `set` alone, `export -p`, or one of
/// [`SECRET_COMMANDS`], anywhere in a pipeline or a list of commands.
pub(crate) fn command_prints_secrets(command: &Value) -> bool {
    command_texts(command).iter().any(|text| {
        text.split(['|', ';', '\n', '&'])
            .any(|segment| segment_prints_secrets(segment))
    })
}

fn segment_prints_secrets(segment: &str) -> bool {
    let mut words: Vec<&str> = segment
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == ')'))
        .filter(|w| !w.is_empty())
        .collect();
    // `sudo`, `exec` and `NAME=value` prefixes run what follows.
    while let Some(first) = words.first() {
        if matches!(
            *first,
            "sudo" | "exec" | "command" | "bash" | "-lc" | "sh" | "-c"
        ) || (first.contains('=') && !first.starts_with('-'))
        {
            words.remove(0);
        } else {
            break;
        }
    }
    let Some(first) = words.first() else {
        return false;
    };
    match *first {
        // `printenv NAME` prints that variable, a secret as often as not.
        "printenv" => return true,
        "env" => return words[1..].iter().all(|w| w.starts_with('-')),
        "set" => return words.len() == 1,
        "export" => return words.len() == 1 || words[1..] == ["-p"],
        _ => {}
    }
    SECRET_COMMANDS.iter().any(|entry| {
        let (prefix, flag) = match entry.split_once(" +") {
            Some((p, f)) => (p, Some(f)),
            None => (*entry, None),
        };
        let wanted: Vec<&str> = prefix.split(' ').collect();
        words.len() >= wanted.len()
            && words[..wanted.len()] == wanted[..]
            && flag.is_none_or(|f| words.contains(&f))
    })
}
