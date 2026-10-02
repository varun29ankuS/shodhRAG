//! Read-only access to indexed source files for the in-app document viewer.
//!
//! Every command resolves the requested path through [`authorize`], which
//! only admits regular files that the RAG index has at least one chunk for.
//! The Tauri asset protocol and fs plugin scopes are deliberately not used:
//! the index is the allow-list, so the viewer can never read a file the user
//! did not add to Shodh.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use tauri::State;

use crate::rag_commands::{is_code_file, RagState};
use shodh_rag::processing::parser::DocumentParser;
use shodh_rag::processing::tabular;

/// Largest file the viewer will read (bytes).
const MAX_SOURCE_BYTES: u64 = 100 * 1024 * 1024;
/// Largest extracted text returned to the webview (characters).
const MAX_TEXT_CHARS: usize = 8_000_000;
/// Rows returned per sheet; larger sheets are truncated with a flag.
const MAX_TABLE_ROWS: usize = 100_000;
/// How long to wait for the index read lock before reporting it busy.
const INDEX_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// Typed error returned to the frontend as `{ kind, message, ... }`.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SourceAccessError {
    /// The source is not a local file (web URL, note, calendar entry).
    #[serde(rename_all = "camelCase")]
    NotAFile { message: String },
    /// The search index is still loading.
    #[serde(rename_all = "camelCase")]
    IndexUnavailable { message: String },
    /// The file is not part of the index, so it may not be read.
    #[serde(rename_all = "camelCase")]
    NotIndexed { message: String },
    /// The file no longer exists at its indexed location.
    #[serde(rename_all = "camelCase")]
    NotFound { message: String },
    /// The file exceeds [`MAX_SOURCE_BYTES`].
    #[serde(rename_all = "camelCase")]
    TooLarge {
        message: String,
        size_bytes: u64,
        limit_bytes: u64,
    },
    /// The file type cannot be shown by the requested viewer.
    #[serde(rename_all = "camelCase")]
    Unsupported { message: String },
    /// Reading or parsing failed.
    #[serde(rename_all = "camelCase")]
    ReadFailed { message: String },
}

/// How the viewer should present a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewerKind {
    Pdf,
    Image,
    Table,
    Text,
    Unsupported,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFileInfo {
    /// Absolute path as stored on disk (original casing).
    pub path: String,
    pub file_name: String,
    pub folder: Option<String>,
    pub extension: String,
    pub size_bytes: u64,
    pub kind: ViewerKind,
    /// MIME type for byte-served kinds (PDF and images).
    pub mime_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceText {
    pub text: String,
    /// True when the text was cut at [`MAX_TEXT_CHARS`].
    pub truncated: bool,
    /// Total characters before truncation.
    pub total_chars: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSheet {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Data rows in the sheet before truncation.
    pub total_rows: usize,
    pub truncated: bool,
}

/// Formats the text viewer renders from the indexer's own parser. Images are
/// excluded (the parser would run OCR) and so are unknown extensions (the
/// parser's plain-text fallback would decode binary data as text).
fn is_text_extension(extension: &str, path: &str) -> bool {
    matches!(
        extension,
        "pdf"
            | "docx"
            | "pptx"
            | "txt"
            | "text"
            | "md"
            | "markdown"
            | "mdx"
            | "rst"
            | "log"
            | "html"
            | "htm"
            | "ini"
            | "cfg"
            | "conf"
            | "env"
            | "css"
            | "scss"
    ) || is_code_file(path)
}

fn image_mime(extension: &str) -> Option<&'static str> {
    match extension {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

fn viewer_kind(extension: &str, path: &str) -> ViewerKind {
    if extension == "pdf" {
        ViewerKind::Pdf
    } else if image_mime(extension).is_some() {
        ViewerKind::Image
    } else if tabular::is_spreadsheet_extension(extension)
        || tabular::is_delimited_extension(extension)
    {
        ViewerKind::Table
    } else if is_text_extension(extension, path) {
        ViewerKind::Text
    } else {
        ViewerKind::Unsupported
    }
}

/// Remove the Windows verbatim prefix (`\\?\C:\…`, `\\?\UNC\server\…`) that
/// `canonicalize` adds, so the path compares equal to how it was indexed.
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{}", rest))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// A file the index vouches for, resolved to its real location.
struct AuthorizedFile {
    path: PathBuf,
    extension: String,
    size_bytes: u64,
}

/// Admit `requested` only if it is an existing regular file and the RAG index
/// holds at least one chunk whose source matches it (as requested or in
/// canonical form, after ingest-time normalization).
async fn authorize(
    state: &State<'_, RagState>,
    requested: &str,
) -> Result<AuthorizedFile, SourceAccessError> {
    let trimmed = requested.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if trimmed.is_empty() || lowered.contains("://") {
        return Err(SourceAccessError::NotAFile {
            message: "This source is not a file on this computer.".to_string(),
        });
    }

    let requested_path = Path::new(trimmed);
    if !requested_path.is_absolute()
        || requested_path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(SourceAccessError::NotIndexed {
            message: "Only files added to Shodh can be shown.".to_string(),
        });
    }

    let canonical = match std::fs::canonicalize(requested_path) {
        Ok(path) => strip_verbatim_prefix(&path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SourceAccessError::NotFound {
                message: "This file is no longer at its indexed location.".to_string(),
            });
        }
        Err(e) => {
            return Err(SourceAccessError::ReadFailed {
                message: format!("Could not resolve the file: {}", e),
            });
        }
    };

    let metadata = std::fs::metadata(&canonical).map_err(|e| SourceAccessError::ReadFailed {
        message: format!("Could not read file details: {}", e),
    })?;
    if !metadata.is_file() {
        return Err(SourceAccessError::NotAFile {
            message: "This source is a folder, not a file.".to_string(),
        });
    }

    if !*state.rag_initialized.read().await {
        return Err(SourceAccessError::IndexUnavailable {
            message: "The search index is still loading. Try again in a moment.".to_string(),
        });
    }

    // Hold the read lock only for the membership queries; all file I/O happens
    // after it is released so indexing (which takes the write lock) never waits
    // on a document being displayed. Acquisition itself is bounded: ingestion
    // can hold the write lock for a long time, and the viewer must then fall
    // back to the retrieved passage instead of spinning indefinitely.
    let indexed = {
        let rag = tokio::time::timeout(INDEX_LOCK_TIMEOUT, state.rag.read())
            .await
            .map_err(|_| SourceAccessError::IndexUnavailable {
                message: "Indexing is in progress, so the document cannot be opened right now. Try again when it finishes.".to_string(),
            })?;
        let mut found = rag.is_indexed_source(requested_path).await.map_err(|e| {
            SourceAccessError::IndexUnavailable {
                message: format!("Could not query the search index: {}", e),
            }
        })?;
        if !found && canonical.as_path() != requested_path {
            found = rag.is_indexed_source(&canonical).await.map_err(|e| {
                SourceAccessError::IndexUnavailable {
                    message: format!("Could not query the search index: {}", e),
                }
            })?;
        }
        found
    };
    if !indexed {
        tracing::warn!(path = %trimmed, "Source viewer refused a file that is not in the index");
        return Err(SourceAccessError::NotIndexed {
            message: "Only files added to Shodh can be shown.".to_string(),
        });
    }

    let extension = canonical
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    Ok(AuthorizedFile {
        path: canonical,
        extension,
        size_bytes: metadata.len(),
    })
}

fn too_large(size_bytes: u64) -> SourceAccessError {
    SourceAccessError::TooLarge {
        message: format!(
            "This file is {:.1} MB; the viewer shows files up to {} MB.",
            size_bytes as f64 / (1024.0 * 1024.0),
            MAX_SOURCE_BYTES / (1024 * 1024)
        ),
        size_bytes,
        limit_bytes: MAX_SOURCE_BYTES,
    }
}

fn ensure_size(file: &AuthorizedFile) -> Result<(), SourceAccessError> {
    if file.size_bytes > MAX_SOURCE_BYTES {
        Err(too_large(file.size_bytes))
    } else {
        Ok(())
    }
}

fn join_failed(e: tokio::task::JoinError) -> SourceAccessError {
    SourceAccessError::ReadFailed {
        message: format!("Reading the file was interrupted: {}", e),
    }
}

/// Describe an indexed file and choose how the viewer should present it.
#[tauri::command]
pub async fn get_source_file_info(
    state: State<'_, RagState>,
    file_path: String,
) -> Result<SourceFileInfo, SourceAccessError> {
    let file = authorize(&state, &file_path).await?;
    let path_str = file.path.display().to_string();
    let kind = viewer_kind(&file.extension, &path_str);
    let mime_type = match kind {
        ViewerKind::Pdf => Some("application/pdf".to_string()),
        ViewerKind::Image => image_mime(&file.extension).map(str::to_string),
        _ => None,
    };
    Ok(SourceFileInfo {
        file_name: file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone()),
        folder: file.path.parent().map(|p| p.display().to_string()),
        extension: file.extension,
        size_bytes: file.size_bytes,
        kind,
        mime_type,
        path: path_str,
    })
}

/// Raw bytes of an indexed PDF or image, sent as a binary IPC payload.
#[tauri::command]
pub async fn read_source_bytes(
    state: State<'_, RagState>,
    file_path: String,
) -> Result<tauri::ipc::Response, SourceAccessError> {
    let file = authorize(&state, &file_path).await?;
    if file.extension != "pdf" && image_mime(&file.extension).is_none() {
        return Err(SourceAccessError::Unsupported {
            message: "Only PDF and image files are served as raw bytes.".to_string(),
        });
    }
    ensure_size(&file)?;

    let path = file.path;
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, SourceAccessError> {
        let handle = std::fs::File::open(&path).map_err(|e| SourceAccessError::ReadFailed {
            message: format!("Could not open the file: {}", e),
        })?;
        let mut bytes = Vec::new();
        // Read one byte past the cap so a file that grew since the size check
        // is still rejected instead of silently truncated.
        handle
            .take(MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| SourceAccessError::ReadFailed {
                message: format!("Could not read the file: {}", e),
            })?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(too_large(bytes.len() as u64));
        }
        Ok(bytes)
    })
    .await
    .map_err(join_failed)??;

    Ok(tauri::ipc::Response::new(bytes))
}

/// Full text of an indexed document, extracted by the same parser the indexer
/// uses, so cited passages can be located in it.
#[tauri::command]
pub async fn read_source_text(
    state: State<'_, RagState>,
    file_path: String,
) -> Result<SourceText, SourceAccessError> {
    let file = authorize(&state, &file_path).await?;
    let path_str = file.path.display().to_string();
    if !is_text_extension(&file.extension, &path_str) {
        return Err(SourceAccessError::Unsupported {
            message: format!("Text view is not available for .{} files.", file.extension),
        });
    }
    ensure_size(&file)?;

    let path = file.path;
    let content = tokio::task::spawn_blocking(move || {
        DocumentParser::new()
            .parse_file(&path)
            .map(|parsed| parsed.content)
            .map_err(|e| SourceAccessError::ReadFailed {
                message: format!("Could not extract text: {:#}", e),
            })
    })
    .await
    .map_err(join_failed)??;

    // Truncate on a character boundary.
    let total_chars = content.chars().count();
    let (text, truncated) = match content.char_indices().nth(MAX_TEXT_CHARS) {
        Some((byte_idx, _)) => (content[..byte_idx].to_string(), true),
        None => (content, false),
    };
    Ok(SourceText {
        text,
        truncated,
        total_chars,
    })
}

/// Sheets of an indexed spreadsheet or CSV/TSV file, normalised exactly as the
/// indexer saw them (same header detection and cell formatting).
#[tauri::command]
pub async fn read_source_table(
    state: State<'_, RagState>,
    file_path: String,
) -> Result<Vec<SourceSheet>, SourceAccessError> {
    let file = authorize(&state, &file_path).await?;
    let extension = file.extension.clone();
    if !tabular::is_spreadsheet_extension(&extension)
        && !tabular::is_delimited_extension(&extension)
    {
        return Err(SourceAccessError::Unsupported {
            message: format!("Table view is not available for .{} files.", extension),
        });
    }
    ensure_size(&file)?;

    let path = file.path;
    let tables = tokio::task::spawn_blocking(move || {
        let result = if tabular::is_spreadsheet_extension(&extension) {
            tabular::read_spreadsheet(&path)
        } else {
            tabular::read_delimited_file(&path, &extension).map(|t| vec![t])
        };
        result.map_err(|e| match e {
            tabular::TabularError::Empty { .. } => SourceAccessError::ReadFailed {
                message: "This file contains no table data.".to_string(),
            },
            other => SourceAccessError::ReadFailed {
                message: format!("Could not read the table: {}", other),
            },
        })
    })
    .await
    .map_err(join_failed)??;

    Ok(tables
        .into_iter()
        .map(|mut table| {
            let total_rows = table.rows.len();
            let truncated = total_rows > MAX_TABLE_ROWS;
            table.rows.truncate(MAX_TABLE_ROWS);
            SourceSheet {
                name: table.name,
                headers: table.headers,
                rows: table.rows,
                total_rows,
                truncated,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_removed() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\C:\Docs\a.pdf")),
            PathBuf::from(r"C:\Docs\a.pdf")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share\a.pdf")),
            PathBuf::from(r"\\server\share\a.pdf")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new("/home/u/a.pdf")),
            PathBuf::from("/home/u/a.pdf")
        );
    }

    #[test]
    fn viewer_kinds_follow_extension() {
        assert_eq!(viewer_kind("pdf", "a.pdf"), ViewerKind::Pdf);
        assert_eq!(viewer_kind("png", "a.png"), ViewerKind::Image);
        assert_eq!(viewer_kind("tiff", "a.tiff"), ViewerKind::Unsupported);
        assert_eq!(viewer_kind("xlsx", "a.xlsx"), ViewerKind::Table);
        assert_eq!(viewer_kind("xlsb", "a.xlsb"), ViewerKind::Table);
        assert_eq!(viewer_kind("tsv", "a.tsv"), ViewerKind::Table);
        assert_eq!(viewer_kind("docx", "a.docx"), ViewerKind::Text);
        assert_eq!(viewer_kind("rs", "a.rs"), ViewerKind::Text);
        assert_eq!(viewer_kind("exe", "a.exe"), ViewerKind::Unsupported);
    }
}
