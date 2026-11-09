//! Turn chunking: what of each turn is indexed, and in which part of its session.
//!
//! - A prompt or a reply is indexed whole, in windows when it is long.
//! - A tool call is indexed as its name and a summary of its arguments.
//! - A tool output is indexed as its head and tail; the rest is a pointer only (the
//!   receipt opens the turn, and the transcript holds all of it). Outputs kept back (a
//!   file that may hold secrets) are not indexed at all.
//!
//! A session's turns are stored in parts of [`PART_TURNS`] turns, each its own source in
//! the index, so a session that grows re-embeds only its last part and the new ones.

use sha2::{Digest, Sha256};

use super::{clip, Role, Transcript, Turn};
use crate::rag_engine::GivenChunk;

/// Turns per part of a session.
pub const PART_TURNS: usize = 40;
/// Characters of a prompt or reply window.
pub const WINDOW_CHARS: usize = 1400;
/// Characters a window shares with the one before it.
pub const WINDOW_OVERLAP: usize = 150;
/// A tool output up to this length is indexed whole.
pub const OUTPUT_WHOLE_CHARS: usize = 1600;
/// Characters of a longer output's head and tail that are indexed.
pub const OUTPUT_HEAD_CHARS: usize = 900;
pub const OUTPUT_TAIL_CHARS: usize = 500;
/// Turns too short to say anything (`ok`, `y`) are not indexed on their own.
const MIN_TURN_CHARS: usize = 2;

/// One chunk of a part, and the turn it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnChunk {
    pub turn: u32,
    pub chunk: GivenChunk,
}

/// The chunks of one part of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// 0-based.
    pub number: u32,
    /// The turns it holds (`first..=last`).
    pub first_turn: u32,
    pub last_turn: u32,
    pub chunks: Vec<TurnChunk>,
    /// Changes whenever anything that is indexed of it changes.
    pub hash: String,
}

/// What the chunks of a session say about it (embedded and keyword-indexed with every
/// chunk, so a session is found by its project, title and branch too).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionLabel {
    pub tool: String,
    pub project: String,
    pub title: String,
    pub branch: Option<String>,
    pub date: Option<String>,
}

impl SessionLabel {
    fn context(&self, turn: &Turn) -> String {
        let mut parts = vec![format!("{} session", self.tool)];
        if !self.project.is_empty() {
            parts.push(format!("project {}", self.project));
        }
        if let Some(branch) = &self.branch {
            parts.push(format!("branch {branch}"));
        }
        if let Some(date) = &self.date {
            parts.push(date.clone());
        }
        let mut line = parts.join(" · ");
        if !self.title.is_empty() {
            line.push_str(&format!(". Session: {}", self.title));
        }
        line.push_str(&format!(". {}", turn_heading(turn)));
        line.push('\n');
        line
    }
}

/// A turn's place in plain words: "Turn 12, tool output of Bash".
pub fn turn_heading(turn: &Turn) -> String {
    let number = turn.index + 1;
    match (&turn.role, &turn.tool_name) {
        (Role::ToolCall, Some(name)) => format!("Turn {number}, tool call {name}"),
        (Role::ToolOutput, Some(name)) => format!("Turn {number}, tool output of {name}"),
        (role, _) => format!("Turn {number}, {}", role.label().to_lowercase()),
    }
}

/// The text of a turn as it is indexed, in pieces: windows of a long prompt or reply, a
/// tool output's head and tail. Empty for an output kept back.
pub fn indexed_pieces(turn: &Turn) -> Vec<String> {
    if turn.kept_back.is_some() || turn.text.trim().chars().count() < MIN_TURN_CHARS {
        return Vec::new();
    }
    let text = turn.text.trim();
    match turn.role {
        Role::User => windows(text, "You: "),
        Role::Assistant => windows(text, "Assistant: "),
        Role::ToolCall => {
            let name = turn.tool_name.as_deref().unwrap_or("tool");
            vec![clip(&format!("Ran {name}: {text}"), WINDOW_CHARS)]
        }
        Role::ToolOutput => {
            let name = turn.tool_name.as_deref().unwrap_or("a tool");
            vec![format!("Output of {name}: {}", head_and_tail(text))]
        }
    }
}

/// A tool output as indexed: whole when short, else its head, a note of what is left
/// out, and its tail.
pub fn head_and_tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= OUTPUT_WHOLE_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(OUTPUT_HEAD_CHARS).collect();
    let tail: String = text.chars().skip(count - OUTPUT_TAIL_CHARS).collect();
    let left_out = count - OUTPUT_HEAD_CHARS - OUTPUT_TAIL_CHARS;
    format!("{head}\n[… {left_out} characters not indexed …]\n{tail}")
}

fn windows(text: &str, prefix: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= WINDOW_CHARS {
        return vec![format!("{prefix}{text}")];
    }
    let step = WINDOW_CHARS - WINDOW_OVERLAP;
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + WINDOW_CHARS).min(chars.len());
        let piece: String = chars[start..end].iter().collect();
        out.push(format!("{prefix}{piece}"));
        if end == chars.len() {
            break;
        }
        start += step;
    }
    out
}

/// The parts of a transcript, every chunk with its context.
pub fn parts(transcript: &Transcript, label: &SessionLabel, fingerprint: &str) -> Vec<Part> {
    transcript
        .turns
        .chunks(PART_TURNS)
        .enumerate()
        .map(|(number, turns)| {
            let mut chunks = Vec::new();
            for turn in turns {
                for piece in indexed_pieces(turn) {
                    chunks.push(TurnChunk {
                        turn: turn.index,
                        chunk: GivenChunk {
                            text: piece,
                            context: label.context(turn),
                            heading: turn_heading(turn),
                        },
                    });
                }
            }
            let mut hash = Sha256::new();
            hash.update(fingerprint.as_bytes());
            hash.update(super::index::INDEX_VERSION.to_le_bytes());
            for c in &chunks {
                hash.update(c.turn.to_le_bytes());
                for text in [&c.chunk.text, &c.chunk.context, &c.chunk.heading] {
                    hash.update((text.len() as u64).to_le_bytes());
                    hash.update(text.as_bytes());
                }
            }
            Part {
                number: number as u32,
                first_turn: turns.first().map_or(0, |t| t.index),
                last_turn: turns.last().map_or(0, |t| t.index),
                chunks,
                hash: hash
                    .finalize()
                    .iter()
                    .take(16)
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            }
        })
        .collect()
}
