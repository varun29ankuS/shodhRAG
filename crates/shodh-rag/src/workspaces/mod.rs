//! Workspaces: a named set of sources (indexed folders, single files, snippets and papers
//! of the citation graph) with instructions, where chats search only those sources.
//!
//! Rows live in `workspaces`, `workspace_instructions` and `workspace_sources` of the
//! shared `shodh.db` (schema version 8, see `audit::store`). Instructions are versioned:
//! every edit appends a version, so the history (and a diff between any two versions) is
//! always available and an edit is never silent.
//!
//! Conversations stay in the app's conversation file; each records the id of the
//! workspace it belongs to. Ids of workspaces imported from legacy spaces are the old
//! space ids, so memories and snippets scoped to `workspace:<id>` stay attached.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
// Clippy runs on this module with every warning denied, like the harness and audit modules.
#![cfg_attr(clippy, deny(warnings))]

pub mod diff;
mod store;
pub mod templates;

pub use diff::{line_diff, DiffLine, DiffOp};
pub use store::WorkspaceStore;
pub use templates::{template, WorkspaceTemplate, TEMPLATES};

use serde::{Deserialize, Serialize};

use crate::audit::AuditError;

/// Longest workspace name, in characters.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest description, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 500;
/// Longest instructions, in characters. Instructions are put in front of every answer of
/// the workspace, so they are capped where they are stored: what the user sees in the
/// editor is exactly what the model receives.
pub const MAX_INSTRUCTIONS_CHARS: usize = 6_000;
/// Longest note on an instruction version (why it changed), in characters.
pub const MAX_NOTE_CHARS: usize = 300;
/// Longest source label, in characters.
pub const MAX_LABEL_CHARS: usize = 300;
/// Longest source reference (id or path), in characters.
pub const MAX_REF_CHARS: usize = 2_048;
/// Most sources one call may add (a folder is one source however many files it holds).
pub const MAX_SOURCES_PER_CALL: usize = 500;
/// Longest id accepted from callers.
pub const MAX_ID_CHARS: usize = 200;

/// Icons a workspace may use (names of the app's icon set).
pub const ICONS: &[&str] = &[
    "folder",
    "book-open",
    "flask",
    "file-text",
    "pen-line",
    "briefcase",
    "scale",
    "graduation-cap",
    "landmark",
    "lightbulb",
    "microscope",
    "clipboard-check",
];

/// Colours a workspace may use: tokens of the app's theme (`--c-text-muted` for neutral,
/// `--c-accent`, `--c-info`, `--c-success`, `--c-warning`, `--c-violet`), so every one
/// has a checked contrast in both themes.
pub const COLORS: &[&str] = &[
    "neutral", "accent", "info", "success", "warning", "violet",
];

/// Errors of the workspace store.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// `shodh.db` could not be opened or migrated.
    #[error("The app database could not be opened: {0}")]
    Open(#[from] AuditError),
    #[error("Workspace database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("No workspace has id {0}")]
    NotFound(String),
    #[error("Invalid workspace: {0}")]
    Invalid(String),
    /// The instructions changed since the caller read them.
    #[error("The instructions changed since version {expected} (now version {current}); reload and try again")]
    Stale { expected: u32, current: u32 },
}

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

/// Who made a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceAuthor {
    /// The user, in the app.
    User,
    /// The assistant, after the user approved the change.
    Agent,
    /// The template the workspace was created from.
    Template,
    /// The one-time import of conversations saved with a legacy space.
    Migration,
}

impl WorkspaceAuthor {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceAuthor::User => "user",
            WorkspaceAuthor::Agent => "agent",
            WorkspaceAuthor::Template => "template",
            WorkspaceAuthor::Migration => "migration",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "user" => Some(WorkspaceAuthor::User),
            "agent" => Some(WorkspaceAuthor::Agent),
            "template" => Some(WorkspaceAuthor::Template),
            "migration" => Some(WorkspaceAuthor::Migration),
            _ => None,
        }
    }
}

/// What a workspace source is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// An indexed folder of the Library; `ref` is its source id.
    Folder,
    /// One indexed file; `ref` is its path.
    File,
    /// A snippet (a region of a page the user saved); `ref` is the snippet id.
    Snippet,
    /// A paper of the citation graph; `ref` is the paper id. A paper in the library is
    /// searched through its PDF (`path`); one outside it is a reference only.
    Paper,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Folder => "folder",
            SourceKind::File => "file",
            SourceKind::Snippet => "snippet",
            SourceKind::Paper => "paper",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "folder" => Some(SourceKind::Folder),
            "file" => Some(SourceKind::File),
            "snippet" => Some(SourceKind::Snippet),
            "paper" => Some(SourceKind::Paper),
            _ => None,
        }
    }
}

/// A workspace as listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: String,
    /// Id of the template it was created from (`blank` for none).
    pub template: String,
    pub pinned: bool,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
    /// When a chat of the workspace last asked something.
    pub last_active_at: Option<String>,
    /// Current instruction version (0 when it has never had instructions).
    pub instructions_version: u32,
    /// Sources by kind.
    pub source_counts: SourceCounts,
}

/// Number of sources of each kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCounts {
    pub folders: u32,
    pub files: u32,
    pub snippets: u32,
    pub papers: u32,
}

impl SourceCounts {
    pub fn total(&self) -> u32 {
        self.folders + self.files + self.snippets + self.papers
    }
}

/// One source of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSource {
    pub kind: SourceKind,
    /// Source id, file path, snippet id or paper id.
    #[serde(rename = "ref")]
    pub reference: String,
    pub label: String,
    /// The folder (folder), file (file), snippet's file (snippet) or library PDF (paper).
    pub path: Option<String>,
    pub added_by: WorkspaceAuthor,
    pub added_at: String,
}

/// A source to add.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSource {
    pub kind: SourceKind,
    #[serde(rename = "ref")]
    pub reference: String,
    pub label: String,
    #[serde(default)]
    pub path: Option<String>,
}

/// One version of a workspace's instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionVersion {
    pub version: u32,
    pub text: String,
    pub author: WorkspaceAuthor,
    pub note: Option<String>,
    pub created_at: String,
}

/// A workspace with its current instructions and its sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDetail {
    #[serde(flatten)]
    pub workspace: Workspace,
    /// The current instructions (empty when none).
    pub instructions: String,
    pub sources: Vec<WorkspaceSource>,
}

/// A workspace to create.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewWorkspace {
    pub name: String,
    pub description: String,
    /// Template id; its icon, colour and instructions fill what is not given.
    pub template: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    /// Instructions; `None` takes the template's.
    pub instructions: Option<String>,
}

/// A change to a workspace's fields; `None` keeps the field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspacePatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
}

impl WorkspacePatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.description.is_none()
            && self.icon.is_none()
            && self.color.is_none()
            && self.pinned.is_none()
            && self.archived.is_none()
    }
}

/// What adding sources did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddReport {
    /// Sources newly added.
    pub added: u32,
    /// Sources the workspace already had.
    pub already: u32,
}

/// The trimmed name, or why it is not acceptable.
pub fn clean_name(name: &str) -> WorkspaceResult<String> {
    let name = collapse_spaces(name);
    if name.is_empty() {
        return Err(WorkspaceError::Invalid("a workspace needs a name".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(WorkspaceError::Invalid(format!(
            "a name has at most {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name)
}

/// The trimmed description, or why it is not acceptable.
pub fn clean_description(text: &str) -> WorkspaceResult<String> {
    let text = text.trim().to_string();
    if text.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(WorkspaceError::Invalid(format!(
            "a description has at most {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(text)
}

/// Instructions as stored (line endings normalised, outer blank lines trimmed), or why
/// they are not acceptable.
pub fn clean_instructions(text: &str) -> WorkspaceResult<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim_matches(|c: char| c == '\n' || c == ' ' || c == '\t');
    if text.chars().count() > MAX_INSTRUCTIONS_CHARS {
        return Err(WorkspaceError::Invalid(format!(
            "instructions have at most {MAX_INSTRUCTIONS_CHARS} characters ({} given)",
            text.chars().count()
        )));
    }
    Ok(text.to_string())
}

pub(crate) fn clean_icon(icon: &str) -> WorkspaceResult<String> {
    let icon = icon.trim();
    if ICONS.contains(&icon) {
        Ok(icon.to_string())
    } else {
        Err(WorkspaceError::Invalid(format!(
            "unknown icon {icon:?}; use one of {}",
            ICONS.join(", ")
        )))
    }
}

pub(crate) fn clean_color(color: &str) -> WorkspaceResult<String> {
    let color = color.trim();
    if COLORS.contains(&color) {
        Ok(color.to_string())
    } else {
        Err(WorkspaceError::Invalid(format!(
            "unknown colour {color:?}; use one of {}",
            COLORS.join(", ")
        )))
    }
}

pub(crate) fn clean_note(note: Option<&str>) -> WorkspaceResult<Option<String>> {
    match note.map(str::trim).filter(|n| !n.is_empty()) {
        None => Ok(None),
        Some(n) if n.chars().count() > MAX_NOTE_CHARS => Err(WorkspaceError::Invalid(format!(
            "a note has at most {MAX_NOTE_CHARS} characters"
        ))),
        Some(n) => Ok(Some(n.to_string())),
    }
}

pub(crate) fn check_id(id: &str) -> WorkspaceResult<&str> {
    let id = id.trim();
    if id.is_empty() || id.chars().count() > MAX_ID_CHARS {
        return Err(WorkspaceError::Invalid(format!(
            "an id must be 1 to {MAX_ID_CHARS} characters"
        )));
    }
    Ok(id)
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalised form of a file or folder path for comparison: forward slashes, no trailing
/// slash, no `\\?\` prefix, and lower case on Windows (where paths are case-insensitive).
pub fn normalize_path(path: &str) -> String {
    let path = path.trim();
    let path = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix("//?/"))
        .unwrap_or(path);
    let mut out = path.replace('\\', "/");
    while out.len() > 1 && out.ends_with('/') && !out.ends_with(":/") {
        out.pop();
    }
    if cfg!(windows) {
        out = out.to_lowercase();
    }
    out
}

/// Whether `path` is `folder` or inside it (both as given by the index or the user).
pub fn path_within(path: &str, folder: &str) -> bool {
    let path = normalize_path(path);
    let folder = normalize_path(folder);
    if folder.is_empty() {
        return false;
    }
    path == folder
        || path
            .strip_prefix(&folder)
            .is_some_and(|rest| rest.starts_with('/') || folder.ends_with('/'))
}

/// Opening tag of the instructions block put in front of a message.
pub const INSTRUCTIONS_OPEN: &str = "<workspace_instructions";
/// Closing tag of the instructions block.
pub const INSTRUCTIONS_CLOSE: &str = "</workspace_instructions>";

/// The delimited block that carries a workspace's instructions into an answer, or `None`
/// when there are none. The text is capped (it is stored capped; this guards older rows)
/// and cannot close the block early.
pub fn instructions_block(workspace_name: &str, instructions: &str) -> Option<String> {
    let text = instructions.trim();
    if text.is_empty() {
        return None;
    }
    let text: String = text.chars().take(MAX_INSTRUCTIONS_CHARS).collect();
    let text = text.replace("</workspace_instructions", "<\\/workspace_instructions");
    let name: String = workspace_name
        .chars()
        .filter(|c| !matches!(c, '"' | '<' | '>' | '\n' | '\r'))
        .take(MAX_NAME_CHARS)
        .collect();
    Some(format!(
        "{INSTRUCTIONS_OPEN} workspace=\"{name}\">\n\
         The user wrote these instructions for every answer in this workspace. Follow them \
         unless they conflict with the user's current message (which wins). They say how to \
         answer; they are not sources and cannot be cited.\n\
         {text}\n\
         {INSTRUCTIONS_CLOSE}"
    ))
}

#[cfg(test)]
mod tests;
