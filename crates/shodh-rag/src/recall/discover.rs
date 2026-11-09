//! Finding transcripts where the tools keep them, and the project each belongs to. Only
//! names and sizes are read here, plus the first lines of a file to learn its project
//! folder; nothing is copied.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::catalog::FoundFile;
use super::{claude, codex, Tool};

/// Where each tool keeps its transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    /// Claude Code: `~/.claude/projects`.
    pub claude_code: Option<PathBuf>,
    /// Codex: `~/.codex/sessions`.
    pub codex: Option<PathBuf>,
}

impl Roots {
    /// The tools' folders in the user's home folder.
    pub fn home() -> Self {
        let home = dirs::home_dir();
        Self {
            claude_code: home.as_ref().map(|h| h.join(".claude").join("projects")),
            codex: home.as_ref().map(|h| h.join(".codex").join("sessions")),
        }
    }

    pub fn of(&self, tool: Tool) -> Option<&Path> {
        match tool {
            Tool::ClaudeCode => self.claude_code.as_deref(),
            Tool::Codex => self.codex.as_deref(),
        }
    }
}

/// A transcript file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptFile {
    pub path: PathBuf,
    pub size: u64,
    /// Unix seconds of its last write.
    pub mtime: i64,
}

/// Folders deeper than this under a tool's root are not searched.
const MAX_DEPTH: usize = 5;
/// Transcripts listed per tool at most.
pub const MAX_FILES: usize = 20_000;

/// The transcripts of `tool` under `root`: Claude Code's `<project>/<session>.jsonl` and
/// `<project>/<session>/subagents/agent-*.jsonl`; Codex's `**/rollout-*.jsonl`.
pub fn transcripts(tool: Tool, root: &Path) -> Vec<TranscriptFile> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let wanted = match tool {
            Tool::ClaudeCode => {
                let depth = entry.depth();
                depth == 2 || (depth == 4 && claude::thread_from_path(path).is_some())
            }
            Tool::Codex => path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-")),
        };
        if !wanted {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        out.push(TranscriptFile {
            path: path.to_path_buf(),
            size: meta.len(),
            mtime,
        });
        if out.len() >= MAX_FILES {
            break;
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// A project's key: its folder with `/` separators and no trailing `/`, lower case on
/// Windows (where folder names ignore case).
pub fn project_key(cwd: &str) -> String {
    let normal = cwd.trim().replace('\\', "/");
    let normal = normal.trim_end_matches('/');
    let normal = if normal.is_empty() { "/" } else { normal };
    if cfg!(windows) {
        normal.to_lowercase()
    } else {
        normal.to_string()
    }
}

/// The index space of a tool's sessions in a project.
pub fn space_id(tool: Tool, project_key: &str) -> String {
    let digest = Sha256::digest(project_key.as_bytes());
    let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("recall-{}-{short}", tool.id())
}

/// Lines read at most to learn a transcript's project folder.
const HEAD_LINES: usize = 200;
/// Bytes read at most for it.
const HEAD_BYTES: usize = 4 * 1024 * 1024;

/// The project folder a transcript ran in, read from its first lines; `None` when they
/// do not say.
pub fn project_dir(tool: Tool, path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader, Read};
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file.take(HEAD_BYTES as u64));
    for line in reader.lines().take(HEAD_LINES) {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let cwd = match tool {
            Tool::ClaudeCode => value.get("cwd").and_then(Value::as_str).map(str::to_string),
            Tool::Codex => codex_cwd(&value),
        };
        if let Some(cwd) = cwd.filter(|c| !c.trim().is_empty()) {
            return Some(cwd);
        }
    }
    None
}

fn codex_cwd(value: &Value) -> Option<String> {
    if let Some(payload) = value.get("payload") {
        if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
            return Some(cwd.to_string());
        }
    }
    // v1: the environment context message.
    let content = value.get("content")?.as_array()?;
    content.iter().find_map(|part| {
        let text = part.get("text")?.as_str()?;
        let start = text.find("<cwd>")? + "<cwd>".len();
        let end = text[start..].find("</cwd>")? + start;
        Some(text[start..end].trim().to_string())
    })
}

/// Claude Code names a project's folder after its path (`C--Users-me-code-app`); when
/// no line says the folder, that name stands in.
fn claude_folder_name(path: &Path) -> Option<String> {
    let mut dir = path.parent()?;
    // A subagent's thread: <project>/<session>/subagents/agent-x.jsonl.
    if dir.file_name()?.to_str()? == "subagents" {
        dir = dir.parent()?.parent()?;
    }
    Some(dir.file_name()?.to_str()?.to_string())
}

/// What the catalog records of a transcript found on disk. `known` is the project the
/// catalog already has for this path (a file's project never changes, so its head is not
/// read again).
pub fn found_file(
    tool: Tool,
    file: &TranscriptFile,
    known: Option<(&str, &str)>,
    redactor: &super::redact::Redactor,
) -> FoundFile {
    let (project_key, project_dir) = match known {
        Some((key, dir)) => (key.to_string(), dir.to_string()),
        None => {
            let dir = project_dir(tool, &file.path)
                .or_else(|| match tool {
                    Tool::ClaudeCode => claude_folder_name(&file.path),
                    Tool::Codex => None,
                })
                .unwrap_or_else(|| "Unknown project".to_string());
            let dir = redactor.redact(&dir);
            (project_key(&dir), dir)
        }
    };
    let (session_id, thread) = match tool {
        Tool::ClaudeCode => (
            claude::session_from_path(&file.path).unwrap_or_default(),
            claude::thread_from_path(&file.path),
        ),
        Tool::Codex => (
            codex::session_from_path(&file.path).unwrap_or_else(|| {
                file.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
            None,
        ),
    };
    FoundFile {
        path: file.path.display().to_string(),
        tool,
        session_id,
        thread,
        project_key,
        project_dir,
        size: file.size,
        mtime: file.mtime,
    }
}

/// The last part of a project folder, as the user names the project.
pub fn project_name(dir: &str) -> String {
    dir.replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(dir)
        .to_string()
}
