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
    parse_time, Format, LineError, ParseBudget, Role, SessionInfo, Tool, Transcript, Turn,
};

/// What an output kept back for a file that may hold secrets says.
pub const SECRET_FILE_KEPT_BACK: &str =
    "Not indexed: the tool read a file that may hold secrets (keys, tokens, passwords).";

/// A tool call, remembered until its output arrives.
struct Call {
    name: String,
    /// The output reads a file that may hold secrets.
    secret_file: bool,
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
    };
    let mut errors: Vec<LineError> = Vec::new();
    let mut error_count = 0usize;
    for_each_line(
        path,
        redactor,
        budget,
        &mut errors,
        &mut error_count,
        |line| {
            reader.line(line);
            true
        },
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
}

fn string<'v>(value: &'v Value, key: &str) -> Option<&'v str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

impl Reader<'_> {
    fn clean(&mut self, text: &str) -> String {
        let (out, count) = self.redactor.redact_counted(text);
        self.redacted += count;
        out
    }

    fn line(&mut self, line: Line) {
        let value = line.value;
        let Some(kind) = string(&value, "type").map(str::to_string) else {
            self.skipped += 1;
            return;
        };
        self.session_fields(&value);
        match kind.as_str() {
            "user" => self.user(&value, line.number),
            "assistant" => self.assistant(&value, line.number),
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
        line: u64,
        kept_back: Option<&'static str>,
    ) {
        if self.turns.len() >= self.budget.max_turns {
            self.skipped += 1;
            return;
        }
        if text.trim().is_empty() && kept_back.is_none() {
            return;
        }
        self.turns.push(Turn {
            index: self.turns.len() as u32,
            role,
            at,
            tool_name,
            text,
            line,
            kept_back,
        });
    }

    fn user(&mut self, value: &Value, line: u64) {
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
                self.push(Role::User, at, None, text, line, None);
            }
            Value::Array(blocks) => {
                for block in blocks {
                    match string(block, "type") {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                let text = self.clean(text);
                                self.push(Role::User, at, None, text, line, None);
                            }
                        }
                        Some("tool_result") => self.tool_result(block, at, line),
                        _ => {}
                    }
                }
            }
            _ => self.skipped += 1,
        }
    }

    fn tool_result(&mut self, block: &Value, at: Option<chrono::DateTime<chrono::Utc>>, line: u64) {
        let call = string(block, "tool_use_id").and_then(|id| self.calls.remove(id));
        let name = call.as_ref().map(|c| c.name.clone());
        if call.as_ref().is_some_and(|c| c.secret_file) {
            self.push(
                Role::ToolOutput,
                at,
                name,
                String::new(),
                line,
                Some(SECRET_FILE_KEPT_BACK),
            );
            return;
        }
        let mut text = content_text(block.get("content"));
        if block.get("is_error").and_then(Value::as_bool) == Some(true) {
            text = format!("Error: {text}");
        }
        let text = self.clean(&text);
        self.push(Role::ToolOutput, at, name, text, line, None);
    }

    fn assistant(&mut self, value: &Value, line: u64) {
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
                self.push(Role::Assistant, at, None, text, line, None);
                continue;
            }
            match string(block, "type") {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        let text = self.clean(text);
                        self.push(Role::Assistant, at, None, text, line, None);
                    }
                }
                Some("tool_use") | Some("server_tool_use") => {
                    let name = string(block, "name").unwrap_or("tool").to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let secret_file = reads_secret_file(&name, &input);
                    if let Some(id) = string(block, "id") {
                        self.calls.insert(
                            id.to_string(),
                            Call {
                                name: name.clone(),
                                secret_file,
                            },
                        );
                    }
                    let summary = super::clip(
                        &self.clean(&summarize_input(&input)),
                        MAX_CALL_SUMMARY_CHARS,
                    );
                    let name = self.clean(&name);
                    self.push(Role::ToolCall, at, Some(name), summary, line, None);
                }
                _ => {}
            }
        }
    }
}

/// The text of a tool result's `content`: a string, or `text` blocks (images are named).
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
                    _ => None,
                },
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
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
/// file, a pattern), then the rest as JSON. Not yet redacted, and not cut: it is cut only
/// after redaction, so a cut never splits a secret into a part no rule recognises.
pub(crate) fn summarize_input(input: &Value) -> String {
    let Value::Object(map) = input else {
        return match input {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    };
    let mut lines = Vec::new();
    for key in SUMMARY_KEYS {
        if let Some(value) = map.get(key) {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Array(items) => items
                    .iter()
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                    .collect::<Vec<_>>()
                    .join(" "),
                other => other.to_string(),
            };
            if !text.trim().is_empty() {
                lines.push(format!("{key}: {text}"));
            }
        }
    }
    let rest: serde_json::Map<String, Value> = map
        .iter()
        .filter(|(k, _)| !SUMMARY_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !rest.is_empty() {
        lines.push(Value::Object(rest).to_string());
    }
    lines.join("\n")
}

/// Whether a tool call reads a file that may hold secrets (`.env`, keys, credentials:
/// [`crate::code_review::secrets::secret_path`]), so its output is kept back.
pub(crate) fn reads_secret_file(name: &str, input: &Value) -> bool {
    let reading = matches!(
        name,
        "Read" | "NotebookRead" | "read_file" | "view" | "cat" | "Glob" | "Grep"
    );
    if reading
        && ["file_path", "path", "notebook_path"]
            .iter()
            .filter_map(|key| input.get(*key).and_then(Value::as_str))
            .any(|path| crate::code_review::secrets::secret_path(path).is_some())
    {
        return true;
    }
    // A shell command that names such a file (`cat .env`, `type id_rsa`).
    ["command", "cmd"]
        .iter()
        .filter_map(|key| input.get(*key))
        .any(|command| command_names_secret_file(command))
}

/// Whether a command (a string, or an argument list) names a file that may hold secrets.
pub(crate) fn command_names_secret_file(command: &Value) -> bool {
    let words: Vec<String> = match command {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => return false,
    };
    words.iter().any(|text| {
        text.split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '`' | ';' | '|' | '&' | '<' | '>' | '(' | ')'
                )
        })
        .filter(|w| !w.is_empty() && !w.starts_with('-'))
        .any(|w| crate::code_review::secrets::secret_path(w).is_some())
    })
}
