//! Generated visuals: the diagrams, charts, sketches, plots, simulations, display equations
//! and tables that answers contain, kept as objects the user can find, refine and reuse.
//!
//! Rows live in `generated_visuals` of the shared `shodh.db` (schema version 3, see
//! `audit::store`). A visual is a chain of versions: version 1 is captured from an answer,
//! later versions are refinements (`parent_id` is the version that was refined). Versions
//! are never edited; title, pin, note and deletion apply to the whole chain.
//!
//! Captures are deduplicated per origin (conversation, message, side thread, side answer)
//! by a SHA-256 of the kind and the normalised source ([`content_hash`]), so recording the
//! same answer twice (or backfilling it later) adds nothing.

pub mod expr;
pub mod spec;
mod store;

pub use spec::validate_spec;
pub use store::VisualStore;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::audit::AuditError;

/// Longest stored source of a visual, in characters (the SVG render cap; plot and
/// simulation specs are capped lower by their renderers).
pub const MAX_SOURCE_CHARS: usize = 100_000;
/// Longest stored slider state (`params`) as JSON, in bytes.
pub const MAX_PARAMS_BYTES: usize = 4_096;
/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 120;
/// Longest note, in characters.
pub const MAX_NOTE_CHARS: usize = 2_000;
/// Longest refinement instruction kept with a version, in characters.
pub const MAX_INSTRUCTION_CHARS: usize = 1_000;
/// Most visuals one capture call may record.
pub const MAX_CAPTURE_BLOCKS: usize = 64;
/// Default and largest page of a listing.
pub const DEFAULT_LIST_LIMIT: u32 = 60;
pub const MAX_LIST_LIMIT: u32 = 500;
/// Most conversations one listing may be limited to.
pub const MAX_LIST_CONVERSATIONS: usize = 5_000;

/// Errors of the visuals store.
#[derive(Debug, thiserror::Error)]
pub enum VisualError {
    /// `shodh.db` could not be opened or migrated.
    #[error("The app database could not be opened: {0}")]
    Open(#[from] AuditError),
    #[error("Visuals database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Visual data could not be encoded: {0}")]
    Json(#[from] serde_json::Error),
    #[error("No visual has id {0}")]
    NotFound(String),
    #[error("The visual {0} was deleted")]
    Deleted(String),
    #[error("Invalid visual: {0}")]
    Invalid(String),
    #[error("A stored visual is unreadable: {0}")]
    Corrupt(String),
}

pub type VisualResult<T> = Result<T, VisualError>;

/// What kind of visual a row holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualKind {
    /// A Mermaid diagram (source as drawn, header included).
    Mermaid,
    /// A ```chart JSON spec.
    Chart,
    /// A ```svg sketch.
    Svg,
    /// A ```plot JSON spec.
    Plot,
    /// A ```simulation JSON spec.
    Simulation,
    /// A display equation (TeX without delimiters).
    Equation,
    /// A Markdown (GFM) table.
    Table,
}

impl VisualKind {
    pub const ALL: [VisualKind; 7] = [
        VisualKind::Mermaid,
        VisualKind::Chart,
        VisualKind::Svg,
        VisualKind::Plot,
        VisualKind::Simulation,
        VisualKind::Equation,
        VisualKind::Table,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VisualKind::Mermaid => "mermaid",
            VisualKind::Chart => "chart",
            VisualKind::Svg => "svg",
            VisualKind::Plot => "plot",
            VisualKind::Simulation => "simulation",
            VisualKind::Equation => "equation",
            VisualKind::Table => "table",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == text)
    }

    /// A human name, for titles and messages.
    pub fn noun(self) -> &'static str {
        match self {
            VisualKind::Mermaid => "Diagram",
            VisualKind::Chart => "Chart",
            VisualKind::Svg => "Sketch",
            VisualKind::Plot => "Plot",
            VisualKind::Simulation => "Simulation",
            VisualKind::Equation => "Equation",
            VisualKind::Table => "Table",
        }
    }

    /// Kinds whose source is a JSON object.
    fn is_json(self) -> bool {
        matches!(
            self,
            VisualKind::Chart | VisualKind::Plot | VisualKind::Simulation
        )
    }
}

/// Who created a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualAuthor {
    /// Recorded from an answer.
    Capture,
    /// Refined by the user in the gallery.
    User,
    /// Revised by the agent (`revise_visual`).
    Agent,
}

impl VisualAuthor {
    pub fn as_str(self) -> &'static str {
        match self {
            VisualAuthor::Capture => "capture",
            VisualAuthor::User => "user",
            VisualAuthor::Agent => "agent",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "capture" => Some(VisualAuthor::Capture),
            "user" => Some(VisualAuthor::User),
            "agent" => Some(VisualAuthor::Agent),
            _ => None,
        }
    }
}

/// Where captured visuals came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualOrigin {
    pub conversation_id: String,
    /// The answer (or, for a side answer, the answer its thread hangs on). `None` for a side
    /// thread kept on the conversation (about a task or a document).
    #[serde(default)]
    pub message_id: Option<String>,
    /// The side thread of a side answer.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// The side answer (thread turn).
    #[serde(default)]
    pub turn_id: Option<String>,
}

/// One visual block found in an answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewVisual {
    pub kind: VisualKind,
    pub title: String,
    pub source: String,
    /// Slider positions and similar view state (a JSON object). `{}` when absent.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// A refinement of an existing version.
#[derive(Debug, Clone, PartialEq)]
pub struct NewVersion {
    pub source: String,
    pub params: Option<serde_json::Value>,
    /// What was asked for ("label the forces").
    pub instruction: Option<String>,
    pub author: VisualAuthor,
}

/// One stored version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualRecord {
    pub id: String,
    pub root_id: String,
    pub parent_id: Option<String>,
    pub version: u32,
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub kind: VisualKind,
    pub title: String,
    pub source: String,
    pub params: serde_json::Value,
    pub content_hash: String,
    pub pinned: bool,
    pub note: String,
    pub instruction: Option<String>,
    pub created_by: VisualAuthor,
    pub created_at: String,
    pub updated_at: String,
}

/// A visual as a gallery card: its latest version and how many versions it has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualSummary {
    #[serde(flatten)]
    pub latest: VisualRecord,
    pub version_count: u32,
    /// When version 1 was captured.
    pub first_created_at: String,
}

/// A version in the version switcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualVersionInfo {
    pub id: String,
    pub version: u32,
    pub created_by: VisualAuthor,
    pub instruction: Option<String>,
    pub created_at: String,
}

/// One version with every version of its chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualDetail {
    pub visual: VisualRecord,
    pub versions: Vec<VisualVersionInfo>,
}

/// Filters of a listing. Every filter is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualQuery {
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Only visuals of these conversations (a workspace's chats). An empty list matches
    /// nothing; `None` applies no such limit.
    #[serde(default)]
    pub conversation_ids: Option<Vec<String>>,
    #[serde(default)]
    pub kind: Option<VisualKind>,
    /// Words to find in titles, notes and sources (every word must match; prefixes count).
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub pinned_only: bool,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

/// One page of a listing: pinned first, then most recently changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualPage {
    pub items: Vec<VisualSummary>,
    /// Visuals matching the filters, over all pages.
    pub total: u64,
}

/// Result of recording an answer's visuals, by index into the request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureReport {
    /// Ids of new visuals.
    pub created: Vec<String>,
    /// Ids of visuals this origin already had (deleted ones included, which stay deleted).
    pub existing: Vec<String>,
    /// Blocks that were not recorded, with why.
    pub skipped: Vec<SkippedBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedBlock {
    pub index: usize,
    pub reason: String,
}

/// Source with line endings unified, trailing spaces of each line and blank lines at both
/// ends removed. Two blocks with the same normalised source are the same visual.
pub fn normalize_source(source: &str) -> String {
    let unified = source.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = unified.lines().map(str::trim_end).collect();
    let start = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(start, |i| i + 1);
    lines[start..end].join("\n")
}

/// Hex SHA-256 of `kind`, a newline and the normalised source.
pub fn content_hash(kind: VisualKind, source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(normalize_source(source).as_bytes());
    hex::encode(hasher.finalize())
}

/// The source to store, after checking it is a plausible visual of `kind`. Over-long
/// sources are rejected, never truncated (a truncated spec does not draw).
pub fn validate_source(kind: VisualKind, source: &str) -> VisualResult<String> {
    let normalized = normalize_source(source);
    if normalized.is_empty() {
        return Err(VisualError::Invalid(format!(
            "the {} is empty",
            kind.noun().to_lowercase()
        )));
    }
    let chars = normalized.chars().count();
    if chars > MAX_SOURCE_CHARS {
        return Err(VisualError::Invalid(format!(
            "the {} has {chars} characters; at most {MAX_SOURCE_CHARS} are kept",
            kind.noun().to_lowercase()
        )));
    }
    if kind.is_json() {
        let value: serde_json::Value = serde_json::from_str(&normalized).map_err(|e| {
            VisualError::Invalid(format!("a {} spec must be JSON: {e}", kind.as_str()))
        })?;
        if !value.is_object() {
            return Err(VisualError::Invalid(format!(
                "a {} spec must be a JSON object",
                kind.as_str()
            )));
        }
    }
    match kind {
        VisualKind::Svg if !normalized.to_ascii_lowercase().contains("<svg") => Err(
            VisualError::Invalid("an svg sketch must contain an <svg> element".to_string()),
        ),
        VisualKind::Table if !is_markdown_table(&normalized) => Err(VisualError::Invalid(
            "a table must be a Markdown table: a header row, a |---| separator and rows"
                .to_string(),
        )),
        _ => Ok(normalized),
    }
}

/// A header row followed by a GFM delimiter row.
fn is_markdown_table(text: &str) -> bool {
    let mut lines = text.lines().map(str::trim);
    let (Some(header), Some(delimiter)) = (lines.next(), lines.next()) else {
        return false;
    };
    header.contains('|')
        && delimiter.contains('-')
        && delimiter
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// Slider state as stored: a JSON object no larger than [`MAX_PARAMS_BYTES`].
pub fn validate_params(params: Option<&serde_json::Value>) -> VisualResult<String> {
    let value = match params {
        None | Some(serde_json::Value::Null) => return Ok("{}".to_string()),
        Some(v) => v,
    };
    if !value.is_object() {
        return Err(VisualError::Invalid(
            "params must be a JSON object".to_string(),
        ));
    }
    let text = serde_json::to_string(value)?;
    if text.len() > MAX_PARAMS_BYTES {
        return Err(VisualError::Invalid(format!(
            "params are {} bytes; at most {MAX_PARAMS_BYTES} are kept",
            text.len()
        )));
    }
    Ok(text)
}

/// A title as stored: whitespace collapsed, capped at [`MAX_TITLE_CHARS`], the kind's noun
/// when empty.
pub fn clean_title(title: &str, kind: VisualKind) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return kind.noun().to_string();
    }
    cap_chars(&flat, MAX_TITLE_CHARS)
}

fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A note as stored: trimmed; rejected when longer than [`MAX_NOTE_CHARS`].
pub fn clean_note(note: &str) -> VisualResult<String> {
    let trimmed = note.trim();
    let chars = trimmed.chars().count();
    if chars > MAX_NOTE_CHARS {
        return Err(VisualError::Invalid(format!(
            "the note has {chars} characters; at most {MAX_NOTE_CHARS} are kept"
        )));
    }
    Ok(trimmed.to_string())
}

/// An FTS5 query for `text`: every word quoted (so operators and quotes in the input are
/// matched literally) and matched as a prefix; words are ANDed. `None` when `text` has no
/// word.
pub fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == '"')
        .map(str::trim)
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .take(16)
        .map(|t| format!("\"{t}\"*"))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_ignores_line_endings_and_edge_whitespace() {
        assert_eq!(
            normalize_source("\r\n\ngraph TD  \r\n  A-->B\t\r\n\n"),
            "graph TD\n  A-->B"
        );
        assert_eq!(
            content_hash(VisualKind::Mermaid, "graph TD\n  A-->B"),
            content_hash(VisualKind::Mermaid, "\ngraph TD \r\n  A-->B\n\n")
        );
        assert_ne!(
            content_hash(VisualKind::Mermaid, "graph TD"),
            content_hash(VisualKind::Svg, "graph TD")
        );
    }

    #[test]
    fn content_hash_is_sha256_of_kind_and_source() {
        // Fixed vector: also asserted by the frontend (tests/visualGallery.test.ts).
        assert_eq!(
            content_hash(VisualKind::Equation, "\nE = mc^2 \r\n"),
            "1a32250b01d47db2a264b337686ee04f5d98c857e235eff0441bee49fe124dfb"
        );
    }

    #[test]
    fn sources_are_validated_per_kind() {
        assert!(validate_source(VisualKind::Chart, "{\"type\":\"bar\"}").is_ok());
        assert!(matches!(
            validate_source(VisualKind::Chart, "[1,2]"),
            Err(VisualError::Invalid(_))
        ));
        assert!(matches!(
            validate_source(VisualKind::Plot, "not json"),
            Err(VisualError::Invalid(_))
        ));
        assert!(validate_source(VisualKind::Svg, "<svg viewBox='0 0 1 1'></svg>").is_ok());
        assert!(validate_source(VisualKind::Svg, "<g/>").is_err());
        assert!(validate_source(VisualKind::Table, "| a | b |\n|---|:-:|\n| 1 | 2 |").is_ok());
        assert!(validate_source(VisualKind::Table, "a, b\n1, 2").is_err());
        assert!(validate_source(VisualKind::Equation, "   \n ").is_err());
        let huge = "x".repeat(MAX_SOURCE_CHARS + 1);
        assert!(validate_source(VisualKind::Equation, &huge).is_err());
    }

    #[test]
    fn params_titles_notes_and_queries_are_bounded() {
        assert_eq!(validate_params(None).unwrap(), "{}");
        assert!(validate_params(Some(&serde_json::json!([1]))).is_err());
        let big = serde_json::json!({ "x": "y".repeat(MAX_PARAMS_BYTES) });
        assert!(validate_params(Some(&big)).is_err());
        assert_eq!(clean_title("  ", VisualKind::Plot), "Plot");
        assert_eq!(
            clean_title(" Forces \n on  a block ", VisualKind::Svg),
            "Forces on a block"
        );
        assert_eq!(
            clean_title(&"a".repeat(500), VisualKind::Svg)
                .chars()
                .count(),
            MAX_TITLE_CHARS
        );
        assert!(clean_note(&"n".repeat(MAX_NOTE_CHARS + 1)).is_err());
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("AND \"x"), Some("\"AND\"* \"x\"*".to_string()));
        assert_eq!(fts_query("--"), None);
    }
}
