//! Codex transcripts: `~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<uuid>.jsonl`.
//!
//! Two formats, both read (checked against `codex-rs/protocol` and `codex-rs/history`):
//! - **v1, flat** (early 2025): a first line `{id, timestamp, instructions, git}`, then
//!   one response item per line (`{"type": "message" | "function_call" |
//!   "function_call_output" | "reasoning", ...}`) and `{"record_type": "state"}` lines.
//!   Lines carry no time of their own.
//! - **v2, envelope**: every line is `{timestamp, type, payload}` with `type`
//!   `session_meta` (`id`, `cwd`, `timestamp`, `cli_version`, `git{branch,
//!   commit_hash, repository_url}`), `turn_context` (`cwd`, `model`), `response_item`
//!   (the items above, plus `custom_tool_call`, `custom_tool_call_output` and
//!   `local_shell_call`), `event_msg` (UI events that repeat the items: not read) and
//!   `compacted`.
//!
//! Response items: `message` (`role`, `content` of `input_text` / `output_text` /
//! `text` items), `function_call` (`name`, `arguments`: JSON inside a string, `call_id`),
//! `function_call_output` (`call_id`, `output`: a string, in v1 often JSON holding
//! `{output, metadata}`, or content items), `reasoning` (never read). User messages that
//! are the CLI's own context (`<environment_context>`, `<user_instructions>`) are not
//! turns; the project folder is read from them.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;

use super::claude::{flatten_value, keeps_back, note_cwd, summarize_input, SECRET_FILE_KEPT_BACK};
use super::lines::{for_each_line, Line};
use super::redact::Redactor;
use super::{
    keep_text, parse_time, Format, LineError, ParseBudget, Role, SessionInfo, Tool, Transcript,
    Turn,
};

static CWD_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<cwd>\s*([^<]+?)\s*</cwd>").unwrap_or_else(|e| panic!("{e}")));
static SESSION_UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})")
        .unwrap_or_else(|e| panic!("{e}"))
});

/// The session id in a rollout file's name.
pub fn session_from_path(path: &Path) -> Option<String> {
    let name = path.file_stem()?.to_str()?;
    SESSION_UUID
        .captures_iter(name)
        .last()
        .map(|c| c[1].to_string())
}

struct Call {
    name: String,
    kept_back: bool,
}

struct Reader<'a> {
    redactor: &'a Redactor,
    budget: ParseBudget,
    format: Option<Format>,
    info: SessionInfo,
    turns: Vec<Turn>,
    calls: HashMap<String, Call>,
    skipped: usize,
    redacted: usize,
    /// Byte offset of the line being read.
    offset: u64,
}

fn string<'v>(value: &'v Value, key: &str) -> Option<&'v str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// Reads a Codex transcript in either format.
pub fn read(path: &Path, redactor: &Redactor, budget: ParseBudget) -> std::io::Result<Transcript> {
    let mut reader = Reader {
        redactor,
        budget,
        format: None,
        info: SessionInfo::default(),
        turns: Vec::new(),
        calls: HashMap::new(),
        skipped: 0,
        redacted: 0,
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
        format,
        mut info,
        mut turns,
        skipped,
        redacted,
        ..
    } = reader;
    if info.session_id.is_none() {
        info.session_id = session_from_path(path);
    }
    // v1 lines carry no time: every turn is placed at the session's start, and the
    // session ends when the file was last written.
    if format == Some(Format::CodexFlat) {
        let modified = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .map(DateTime::<Utc>::from);
        if let Some(modified) = modified {
            info.ended = Some(info.ended.map_or(modified, |e| e.max(modified)));
        }
        for turn in &mut turns {
            if turn.at.is_none() {
                turn.at = info.started;
            }
        }
    }
    Ok(Transcript {
        tool: Tool::Codex,
        format: format.unwrap_or(Format::CodexEnvelope),
        info,
        turns,
        errors,
        skipped,
        redacted,
    })
}

impl Reader<'_> {
    fn clean(&mut self, text: &str) -> String {
        let (out, count) = self.redactor.redact_counted(text);
        self.redacted += count;
        out
    }

    fn seen(&mut self, at: Option<DateTime<Utc>>) {
        if let Some(at) = at {
            self.info.started = Some(self.info.started.map_or(at, |s| s.min(at)));
            self.info.ended = Some(self.info.ended.map_or(at, |e| e.max(at)));
        }
    }

    /// Reads one line; `false` once the turns asked for are read.
    fn line(&mut self, line: Line) -> bool {
        self.offset = line.offset;
        self.read_line(line);
        self.turns.len() < self.budget.max_turns
    }

    fn read_line(&mut self, line: Line) {
        let value = line.value;
        // v2: an envelope.
        if let (Some(kind), Some(payload)) = (string(&value, "type"), value.get("payload")) {
            if self.format.is_none() {
                self.format = Some(Format::CodexEnvelope);
            }
            let at = string(&value, "timestamp").and_then(parse_time);
            self.seen(at);
            match kind {
                "session_meta" => self.session_meta(payload),
                "turn_context" => {
                    if let Some(model) = string(payload, "model") {
                        self.info.model = Some(self.clean(model));
                    }
                    if let Some(cwd) = string(payload, "cwd") {
                        note_cwd(&mut self.info, cwd);
                        if self.info.cwd.is_none() {
                            self.info.cwd = Some(self.clean(cwd));
                        }
                    }
                    self.skipped += 1;
                }
                "response_item" => self.item(payload, at, line.number),
                _ => self.skipped += 1,
            }
            return;
        }
        // v1: the first line names the session.
        if value.get("record_type").is_some() {
            self.skipped += 1;
            return;
        }
        if value.get("instructions").is_some()
            || (value.get("id").is_some() && value.get("type").is_none())
        {
            self.format = Some(Format::CodexFlat);
            self.session_meta(&value);
            return;
        }
        if self.format.is_none() {
            self.format = Some(Format::CodexFlat);
        }
        if value.get("type").is_some() {
            self.item(&value, None, line.number);
        } else {
            self.skipped += 1;
        }
    }

    fn session_meta(&mut self, meta: &Value) {
        if let Some(id) = string(meta, "id").or_else(|| string(meta, "session_id")) {
            self.info.session_id = Some(self.clean(id));
        }
        if let Some(cwd) = string(meta, "cwd") {
            note_cwd(&mut self.info, cwd);
            self.info.cwd = Some(self.clean(cwd));
        }
        if let Some(version) = string(meta, "cli_version") {
            self.info.version = Some(self.clean(version));
        }
        let at = string(meta, "timestamp").and_then(parse_time);
        self.seen(at);
        if let Some(git) = meta.get("git") {
            if let Some(branch) = string(git, "branch") {
                self.info.branch = Some(self.clean(branch));
            }
        }
        if let Some(nickname) = string(meta, "agent_nickname") {
            self.info.thread = Some(self.clean(nickname));
        }
    }

    fn push(
        &mut self,
        role: Role,
        at: Option<DateTime<Utc>>,
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
        let (text, left_out) = keep_text(text);
        self.turns.push(Turn {
            index: self.budget.first_turn + self.turns.len() as u32,
            role,
            at,
            tool_name,
            text,
            line,
            offset: self.offset,
            left_out,
            kept_back,
        });
    }

    fn item(&mut self, item: &Value, at: Option<DateTime<Utc>>, line: u64) {
        match string(item, "type") {
            Some("message") => self.message(item, at, line),
            Some("function_call") => {
                let name = string(item, "name").unwrap_or("tool").to_string();
                // `arguments` is JSON inside a string.
                let arguments = match item.get("arguments") {
                    Some(Value::String(s)) => {
                        serde_json::from_str(s).unwrap_or(Value::String(s.clone()))
                    }
                    Some(other) => other.clone(),
                    None => Value::Null,
                };
                self.call(name, &arguments, string(item, "call_id"), at, line);
            }
            Some("custom_tool_call") => {
                let name = string(item, "name").unwrap_or("tool").to_string();
                let input = item.get("input").cloned().unwrap_or(Value::Null);
                self.call(name, &input, string(item, "call_id"), at, line);
            }
            Some("local_shell_call") => {
                let command = item
                    .get("action")
                    .and_then(|a| a.get("command"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let input = serde_json::json!({ "command": command });
                self.call(
                    "shell".to_string(),
                    &input,
                    string(item, "call_id"),
                    at,
                    line,
                );
            }
            Some("function_call_output") | Some("custom_tool_call_output") => {
                let call = string(item, "call_id").and_then(|id| self.calls.remove(id));
                let name = call.as_ref().map(|c| c.name.clone());
                if call.as_ref().is_some_and(|c| c.kept_back) {
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
                let text = output_text(item.get("output"));
                let text = self.clean(&text);
                self.push(Role::ToolOutput, at, name, text, line, None);
            }
            // Reasoning is never read; web searches and the rest carry nothing to index.
            _ => self.skipped += 1,
        }
    }

    fn call(
        &mut self,
        name: String,
        input: &Value,
        call_id: Option<&str>,
        at: Option<DateTime<Utc>>,
        line: u64,
    ) {
        let kept_back = keeps_back(&name, input);
        if let Some(id) = call_id {
            self.calls.insert(
                id.to_string(),
                Call {
                    name: name.clone(),
                    kept_back,
                },
            );
        }
        if kept_back {
            let name = self.clean(&name);
            self.push(
                Role::ToolCall,
                at,
                Some(name),
                String::new(),
                line,
                Some(SECRET_FILE_KEPT_BACK),
            );
            return;
        }
        let summary = super::clip(
            &self.clean(&summarize_input(input)),
            super::claude::MAX_CALL_SUMMARY_CHARS,
        );
        let name = self.clean(&name);
        self.push(Role::ToolCall, at, Some(name), summary, line, None);
    }

    fn message(&mut self, item: &Value, at: Option<DateTime<Utc>>, line: u64) {
        let role = match string(item, "role") {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            // `developer` and `system` messages are the CLI's instructions.
            _ => {
                self.skipped += 1;
                return;
            }
        };
        let text = match item.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|part| match string(part, "type") {
                    Some("input_text") | Some("output_text") | Some("text") => {
                        part.get("text").and_then(Value::as_str).map(str::to_string)
                    }
                    Some("input_image") => Some("[image]".to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let trimmed = text.trim_start();
        if role == Role::User
            && (trimmed.starts_with("<environment_context>")
                || trimmed.starts_with("<user_instructions>")
                || trimmed.starts_with("# AGENTS.md instructions"))
        {
            if let Some(cwd) = CWD_TAG.captures(trimmed).map(|c| c[1].to_string()) {
                note_cwd(&mut self.info, &cwd);
                if self.info.cwd.is_none() {
                    self.info.cwd = Some(self.clean(&cwd));
                }
            }
            self.skipped += 1;
            return;
        }
        let text = self.clean(&text);
        self.push(role, at, None, text, line, None);
    }
}

/// A tool output's text: a plain string, a string holding v1's `{output, metadata}`
/// JSON, or content items.
fn output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(s)) => {
            let trimmed = s.trim_start();
            if trimmed.starts_with('{') {
                if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
                    if let Some(inner) = map.get("output").and_then(Value::as_str) {
                        let code = map
                            .get("metadata")
                            .and_then(|m| m.get("exit_code"))
                            .and_then(Value::as_i64);
                        return match code {
                            Some(code) if code != 0 => format!("Exit code {code}\n{inner}"),
                            _ => inner.to_string(),
                        };
                    }
                }
            }
            s.clone()
        }
        Some(Value::Object(map)) => match map.get("content") {
            Some(content) => output_text(Some(content)),
            None => {
                let mut lines = Vec::new();
                flatten_value(&Value::Object(map.clone()), "", &mut lines);
                lines.join("\n")
            }
        },
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                _ => match string(item, "type") {
                    Some("input_text") | Some("output_text") | Some("text") => {
                        item.get("text").and_then(Value::as_str).map(str::to_string)
                    }
                    Some("input_image") => Some("[image]".to_string()),
                    _ => None,
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
