//! Recall: the user's own AI coding sessions (Claude Code and Codex transcripts),
//! indexed in place on this computer and searched with Shodh's hybrid search.
//!
//! Layers, each rebuilt from the one below:
//! - **L0, the raw source:** the transcripts exactly as the tools wrote them. They are
//!   read where they are and never copied; nothing here writes to them.
//! - **L1, time and provenance:** every turn keeps its tool, project folder, session id,
//!   turn number and time: the receipt that opens the exact turn ([`query::open`]).
//! - **L2, entities:** exact ids found by pattern, with no model: PR and issue numbers,
//!   commit SHAs, file paths, branches, model ids, error codes, URLs ([`entities`]).
//!
//! Search is recall-oriented: it returns pointers and receipts, and the caller opens the
//! source before citing it. Every string taken from a transcript passes through the
//! [`redact::Redactor`] first, so a secret in a transcript is never indexed, shown or
//! sent to a model.
//!
//! The index is its own library (its own folder, search model record, vectors and
//! keyword index), never the user's document library: Ask, Slack and Telegram answers
//! cannot reach it.

pub mod catalog;
pub mod chunks;
pub mod claude;
pub mod codex;
pub mod discover;
pub mod entities;
pub mod index;
mod lines;
pub mod query;
pub mod redact;
pub mod settings;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The coding tools whose sessions Recall reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    ClaudeCode,
    Codex,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::ClaudeCode, Tool::Codex];

    /// The id used in settings, sources and the tools' arguments.
    pub fn id(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "claude_code",
            Tool::Codex => "codex",
        }
    }

    /// The tool's name as the user knows it.
    pub fn label(self) -> &'static str {
        match self {
            Tool::ClaudeCode => "Claude Code",
            Tool::Codex => "Codex",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// Who a turn is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// What the user typed.
    User,
    /// What the assistant wrote.
    Assistant,
    /// A tool the assistant called, with a summary of its arguments.
    ToolCall,
    /// What a tool returned.
    ToolOutput,
}

impl Role {
    pub fn id(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::ToolCall => "tool_call",
            Role::ToolOutput => "tool_output",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [
            Role::User,
            Role::Assistant,
            Role::ToolCall,
            Role::ToolOutput,
        ]
        .into_iter()
        .find(|r| r.id() == id)
    }

    /// In plain words, for the viewer and the tools.
    pub fn label(self) -> &'static str {
        match self {
            Role::User => "You",
            Role::Assistant => "Assistant",
            Role::ToolCall => "Tool call",
            Role::ToolOutput => "Tool output",
        }
    }
}

/// One turn of a session: a prompt, a reply, a tool call or a tool's output. Its text is
/// already redacted.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// 0-based, in the order the turns appear in the file.
    pub index: u32,
    pub role: Role,
    pub at: Option<DateTime<Utc>>,
    /// The tool called, or whose output this is.
    pub tool_name: Option<String>,
    /// Redacted text. A tool output holds all of it here; only its head and tail are
    /// indexed ([`chunks`]).
    pub text: String,
    /// 1-based line of the file the turn was read from.
    pub line: u64,
    /// Byte offset of that line: a receipt is opened by reading from there.
    pub offset: u64,
    /// Characters of the turn not kept (the middle of a very long output or paste).
    pub left_out: usize,
    /// Why the turn's text is not kept (the output of reading a file that may hold
    /// secrets): its text is then empty.
    pub kept_back: Option<&'static str>,
}

/// A line that could not be read. Its raw text is never kept: only where it is, why, and
/// a short redacted preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LineError {
    pub line: u64,
    /// Byte offset of the line in the file.
    pub offset: u64,
    pub message: String,
    /// The start of the line with its JSON escapes undone, redacted.
    pub preview: String,
}

/// What a transcript says about its session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionInfo {
    /// The tool's own session id.
    pub session_id: Option<String>,
    /// A subagent's id, when the file is a subagent's thread of a session.
    pub thread: Option<String>,
    /// The project folder the session ran in (redacted, as shown).
    pub cwd: Option<String>,
    /// The key chains ([`discover::project_chain`]) of every folder the session worked
    /// in, the first first: excluding any of them excludes the session.
    pub cwd_chains: Vec<Vec<String>>,
    pub branch: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    /// The tool's version that wrote the file.
    pub version: Option<String>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    /// Pull requests the tool linked to the session (`#n` and the URL).
    pub pull_requests: Vec<String>,
}

/// The file format a transcript was read as (versioned: the fixtures are per version).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    /// Claude Code's JSONL (`~/.claude/projects/<project>/<session>.jsonl`).
    #[serde(rename = "claude-code/jsonl")]
    ClaudeCodeJsonl,
    /// Codex's first rollout format: a first line `{id, timestamp, instructions, git}`,
    /// response items on their own lines, `record_type` state lines.
    #[serde(rename = "codex/v1-flat")]
    CodexFlat,
    /// Codex's rollout format with an envelope per line: `{timestamp, type, payload}`.
    #[serde(rename = "codex/v2-envelope")]
    CodexEnvelope,
}

impl Format {
    pub fn id(self) -> &'static str {
        match self {
            Format::ClaudeCodeJsonl => "claude-code/jsonl",
            Format::CodexFlat => "codex/v1-flat",
            Format::CodexEnvelope => "codex/v2-envelope",
        }
    }
}

/// A transcript read and redacted.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub tool: Tool,
    pub format: Format,
    pub info: SessionInfo,
    pub turns: Vec<Turn>,
    pub errors: Vec<LineError>,
    /// Lines of kinds Recall does not index (requests, attachments, snapshots, state).
    pub skipped: usize,
    /// Secrets replaced while reading.
    pub redacted: usize,
}

/// Limits on reading one transcript, so one damaged or enormous file cannot hold the
/// indexer (as the code map bounds its parse).
#[derive(Debug, Clone, Copy)]
pub struct ParseBudget {
    /// Lines longer than this are recorded as errors and not read.
    pub max_line_bytes: usize,
    /// Turns read at most; later ones are left out (counted as skipped).
    pub max_turns: usize,
    /// Errors recorded at most (the rest are counted).
    pub max_errors: usize,
    /// Where reading starts: a byte offset at a line start, the lines before it, and the
    /// index of the first turn there (all 0: the start of the file).
    pub start_offset: u64,
    pub start_line: u64,
    pub first_turn: u32,
}

impl Default for ParseBudget {
    fn default() -> Self {
        Self {
            max_line_bytes: 16 * 1024 * 1024,
            max_turns: 20_000,
            max_errors: 200,
            start_offset: 0,
            start_line: 0,
            first_turn: 0,
        }
    }
}

/// Characters of a line kept in an error's preview.
pub const ERROR_PREVIEW_CHARS: usize = 160;

/// Reads the transcript at `path` as `tool`'s format, redacting everything it keeps.
pub fn read_transcript(
    tool: Tool,
    path: &std::path::Path,
    redactor: &redact::Redactor,
    budget: ParseBudget,
) -> std::io::Result<Transcript> {
    match tool {
        Tool::ClaudeCode => claude::read(path, redactor, budget),
        Tool::Codex => codex::read(path, redactor, budget),
    }
}

/// `text` cut to at most `max` characters, at a character boundary, with an ellipsis
/// when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Parses an RFC 3339 time.
pub fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// `text` with the user's home folder (`home`) written as `~`, ignoring case and `\`
/// versus `/`: what the model reads of a session never holds the user's name and
/// folders, which an answer could quote.
pub fn hide_home(text: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.len() < 3 {
        return text.to_string();
    }
    let mut out = text.to_string();
    // As written, JSON-escaped (`C:\\Users\\Ada`), as MSYS writes it (`/c/Users/Ada`),
    // and as Claude Code names a project's folder (`C--Users-Ada`).
    let normal = home.replace('\\', "/");
    let mut forms = vec![home.to_string(), home.replace('\\', "\\\\")];
    let bytes = normal.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        forms.push(format!("/{}{}", &normal[..1], &normal[2..]));
    }
    forms.push(normal.replace([':', '/'], "-"));
    for form in forms {
        out = replace_folded(&out, &form);
    }
    out
}

/// `text` with every `needle` (ignoring ASCII case and `\` versus `/`) written as `~`.
fn replace_folded(text: &str, needle: &str) -> String {
    let fold = |s: &str| -> Vec<u8> {
        s.bytes()
            .map(|b| {
                if b == b'\\' {
                    b'/'
                } else {
                    b.to_ascii_lowercase()
                }
            })
            .collect()
    };
    let hay = fold(text);
    let needle = fold(needle);
    if needle.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let (mut i, mut last) = (0, 0);
    while i + needle.len() <= hay.len() {
        let end = i + needle.len();
        if hay[i..end] == needle[..] && text.is_char_boundary(i) && text.is_char_boundary(end) {
            out.push_str(&text[last..i]);
            out.push('~');
            i = end;
            last = end;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// The user's home folder, as [`hide_home`] takes it.
pub fn home_folder() -> Option<String> {
    dirs::home_dir().map(|h| h.display().to_string())
}

/// Characters of a turn's text kept at most: a longer output or paste keeps its head
/// and tail (what is indexed and opened of it), so a huge session is never held whole.
pub const KEEP_TURN_CHARS: usize = 16_000;

/// `text` cut to [`KEEP_TURN_CHARS`] (its head and tail, with a note between), and the
/// characters left out. Cut only after redaction, so no cut splits a secret.
pub(crate) fn keep_text(text: String) -> (String, usize) {
    let count = text.chars().count();
    if count <= KEEP_TURN_CHARS {
        return (text, 0);
    }
    let head_chars = KEEP_TURN_CHARS * 2 / 3;
    let tail_chars = KEEP_TURN_CHARS - head_chars;
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text.chars().skip(count - tail_chars).collect();
    let left_out = count - head_chars - tail_chars;
    (
        format!("{head}\n[… {left_out} characters not kept …]\n{tail}"),
        left_out,
    )
}
