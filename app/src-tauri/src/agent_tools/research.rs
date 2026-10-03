//! Building a research folder: `create_folder` and `download_file` (write),
//! and `list_directory` (read), all confined to indexed source folders.
//!
//! Confinement: every path must canonicalize (symlinks resolved) to a
//! location inside a registered source folder. New folder names are checked
//! segment by segment (no `..`, separators, reserved characters, reserved
//! Windows device names, or trailing dots/spaces) and the result is checked
//! again after creation, so a symlinked segment cannot lead outside.
//!
//! Downloads reuse the web tools' SSRF-safe client, are refused in
//! Local-only mode or with web access off, accept only document and image
//! types whose first bytes match the claimed type, stream to a `.part` file
//! under a 100 MB cap, and land under a name that never replaces an existing
//! file (hard link + remove, which fails instead of overwriting). The saved
//! file is audited with its source URL, SHA-256 and time, then indexed so it
//! is searchable at once.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use shodh_rag::audit::payload::{source_change, ChangeOrigin};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    ApprovalPreview, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::web::{SafeClient, WebError};
use shodh_rag::harness::RiskTier;

use super::files::{
    absolute_arg, is_reserved_windows_name, is_within, sanitize_name, SourceRoot, MAX_NAME_CHARS,
};
use super::sources::strip_verbatim;
use super::web::web_block_reason;
use super::{invalid, limit_arg, str_arg, AgentHost, FileIndexJob};

/// Largest download.
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;
/// Entries `list_directory` returns at most.
pub const MAX_LISTED_ENTRIES: usize = 2_000;
const DEFAULT_LISTED_ENTRIES: usize = 200;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(CreateFolderTool { host: host.clone() }))?;
    registry.register(Arc::new(DownloadFileTool { host: host.clone() }))?;
    registry.register(Arc::new(ListDirectoryTool { host: host.clone() }))?;
    Ok(())
}

// ── Confinement ────────────────────────────────────────────────────────────

/// Why `segment` cannot be a folder or file name, if it cannot.
pub fn invalid_segment(segment: &str) -> Option<&'static str> {
    if segment.is_empty() {
        return Some("empty name");
    }
    if segment == "." || segment == ".." {
        return Some("'.' and '..' are not names");
    }
    if segment.chars().count() > MAX_NAME_CHARS {
        return Some("name too long");
    }
    if segment.chars().any(|c| {
        matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
    }) {
        return Some("contains a character that file names cannot hold");
    }
    if segment.ends_with('.') || segment.ends_with(' ') || segment.starts_with(' ') {
        return Some("names cannot start with a space or end with a dot or space");
    }
    if is_reserved_windows_name(segment) {
        return Some("reserved Windows device name");
    }
    None
}

/// Split a relative folder path (`/` or `\` separated) into checked
/// segments.
pub fn relative_segments(tool: &str, raw: &str) -> Result<Vec<String>, ToolError> {
    let segments: Vec<&str> = raw.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(invalid(tool, "the folder name is empty"));
    }
    if segments.len() > 8 {
        return Err(invalid(tool, "at most 8 nested folders at once"));
    }
    segments
        .into_iter()
        .map(|s| match invalid_segment(s) {
            Some(why) => Err(invalid(tool, format!("{s:?}: {why}"))),
            None => Ok(s.to_string()),
        })
        .collect()
}

/// The source root containing `path` (compared canonically).
fn root_for<'a>(path: &Path, roots: &'a [SourceRoot]) -> Option<&'a SourceRoot> {
    roots.iter().find(|r| is_within(path, &r.folder))
}

fn outside_sources(path: &Path) -> ToolError {
    ToolError::Forbidden(format!(
        "{} is not inside an indexed source folder; only source folders (see list_sources) can \
         be changed.",
        path.display()
    ))
}

/// The deepest existing ancestor of `path` (canonical) and the remaining
/// segments below it.
fn split_existing(path: &Path) -> Result<(PathBuf, Vec<String>), ToolError> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| ToolError::NotFound(format!("{} does not exist", path.display())))?;
        missing.push(name);
        existing = existing
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ToolError::NotFound(format!("{} does not exist", path.display())))?;
    }
    missing.reverse();
    let canonical = std::fs::canonicalize(&existing)
        .map(|p| strip_verbatim(&p))
        .map_err(|e| ToolError::Unavailable(format!("Cannot open {}: {e}", existing.display())))?;
    if !canonical.is_dir() {
        return Err(ToolError::Forbidden(format!(
            "{} is a file, not a folder",
            canonical.display()
        )));
    }
    Ok((canonical, missing))
}

/// A folder inside a source root, possibly not created yet.
struct FolderPlan {
    root: SourceRoot,
    /// Canonical existing ancestor.
    existing: PathBuf,
    /// Folders still to create below `existing`.
    missing: Vec<String>,
}

impl FolderPlan {
    fn target(&self) -> PathBuf {
        self.missing
            .iter()
            .fold(self.existing.clone(), |p, s| p.join(s))
    }
}

fn plan_folder(tool: &str, path: &Path, roots: &[SourceRoot]) -> Result<FolderPlan, ToolError> {
    let (existing, missing) = split_existing(path)?;
    for segment in &missing {
        if let Some(why) = invalid_segment(segment) {
            return Err(invalid(tool, format!("{segment:?}: {why}")));
        }
    }
    let root = root_for(&existing, roots)
        .cloned()
        .ok_or_else(|| outside_sources(path))?;
    Ok(FolderPlan {
        root,
        existing,
        missing,
    })
}

/// Create the plan's missing folders one level at a time, re-checking
/// confinement after each (a concurrently planted symlink cannot lead out).
fn create_planned(plan: &FolderPlan) -> Result<PathBuf, ToolError> {
    let mut current = plan.existing.clone();
    for segment in &plan.missing {
        let next = current.join(segment);
        match std::fs::create_dir(&next) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && next.is_dir() => {}
            Err(e) => {
                return Err(ToolError::Unavailable(format!(
                    "Could not create {}: {e}",
                    next.display()
                )))
            }
        }
        let canonical = std::fs::canonicalize(&next)
            .map(|p| strip_verbatim(&p))
            .map_err(|e| ToolError::Unavailable(format!("Cannot open {}: {e}", next.display())))?;
        if !is_within(&canonical, &plan.root.folder) {
            return Err(outside_sources(&canonical));
        }
        current = canonical;
    }
    Ok(current)
}

// ── create_folder ──────────────────────────────────────────────────────────

pub struct CreateFolderTool {
    host: Arc<AgentHost>,
}

fn create_folder_target(tool: &str, args: &Value) -> Result<PathBuf, ToolError> {
    let parent = str_arg(args, "parent").ok_or_else(|| invalid(tool, "`parent` is required"))?;
    let name = str_arg(args, "name").ok_or_else(|| invalid(tool, "`name` is required"))?;
    let parent = absolute_arg(tool, parent)?;
    let segments = relative_segments(tool, name)?;
    Ok(segments.iter().fold(parent, |p, s| p.join(s)))
}

#[async_trait]
impl HostTool for CreateFolderTool {
    fn name(&self) -> &'static str {
        app_tools::CREATE_FOLDER
    }
    fn label(&self) -> &'static str {
        "Create folder"
    }
    fn label_template(&self) -> &'static str {
        "Creating folder {name}"
    }
    fn description(&self) -> &'static str {
        "Create a folder (or nested folders, e.g. \"Research/Transformers\") inside an indexed \
         source folder. parent must be that source folder (from list_sources) or a folder inside \
         it. Folders elsewhere cannot be created."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "parent": {"type": "string", "minLength": 3, "maxLength": 1024},
                "name": {"type": "string", "minLength": 1, "maxLength": 400}
            },
            "required": ["parent", "name"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::CREATE_FOLDER;
        let target = create_folder_target(tool, args)?;
        let roots = self.host.roots.roots().await?;
        let plan = plan_folder(tool, &target, &roots)?;
        if plan.missing.is_empty() {
            return Err(ToolError::Failed(format!(
                "{} already exists.",
                plan.existing.display()
            )));
        }
        Ok(ApprovalPreview {
            label: Some(format!("Create folder {}", plan.target().display())),
            details: json!({
                "path": plan.target().display().to_string(),
                "source": plan.root.folder.display().to_string(),
                "newFolders": plan.missing,
            }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CREATE_FOLDER;
        let target = create_folder_target(tool, &args)?;
        let roots = self.host.roots.roots().await?;
        let plan = plan_folder(tool, &target, &roots)?;
        let existed = plan.missing.is_empty();
        let created = create_planned(&plan)?;
        let shown = created.display().to_string();
        if !existed {
            ctx.audit(
                AuditEventType::SourceChange,
                source_change(
                    "create_folder",
                    ChangeOrigin::Agent,
                    Some(&plan.root.source_id),
                    Some(&shown),
                    json!({"ok": true, "created": plan.missing}),
                ),
            );
            self.host.effects.library_changed(&plan.root.source_id);
        }
        Ok(ToolOutput {
            text_for_model: if existed {
                format!("{shown} already exists.")
            } else {
                format!("Created {shown}.")
            },
            summary_for_ui: if existed {
                format!("{shown} already exists")
            } else {
                format!("Created {shown}")
            },
            detail: Some(
                json!({ "path": shown, "sourceId": plan.root.source_id, "existed": existed }),
            ),
        })
    }
}

// ── download_file ──────────────────────────────────────────────────────────

/// File types a download may have, by extension, with their media types.
const ALLOWED_TYPES: [(&str, &[&str]); 13] = [
    ("pdf", &["application/pdf"]),
    (
        "docx",
        &["application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
    ),
    (
        "xlsx",
        &["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
    ),
    ("csv", &["text/csv", "application/csv"]),
    ("txt", &["text/plain"]),
    ("md", &["text/markdown", "text/x-markdown"]),
    ("html", &["text/html", "application/xhtml+xml"]),
    ("png", &["image/png"]),
    ("jpg", &["image/jpeg"]),
    ("jpeg", &["image/jpeg"]),
    ("gif", &["image/gif"]),
    ("webp", &["image/webp"]),
    ("tiff", &["image/tiff"]),
];

fn extension_for_mime(mime: &str) -> Option<&'static str> {
    ALLOWED_TYPES
        .iter()
        .find(|(_, mimes)| mimes.contains(&mime))
        .map(|(ext, _)| *ext)
}

fn is_allowed_extension(ext: &str) -> bool {
    ALLOWED_TYPES.iter().any(|(e, _)| *e == ext)
}

fn is_text_extension(ext: &str) -> bool {
    matches!(ext, "csv" | "txt" | "md" | "html")
}

/// Whether the first bytes are those of a file of type `ext`.
pub fn magic_matches(ext: &str, head: &[u8]) -> bool {
    match ext {
        "pdf" => head.starts_with(b"%PDF-"),
        "docx" | "xlsx" => head.starts_with(b"PK\x03\x04"),
        "png" => head.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => head.starts_with(&[0xFF, 0xD8, 0xFF]),
        "gif" => head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"),
        "webp" => head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP",
        "tiff" => head.starts_with(b"II*\0") || head.starts_with(b"MM\0*"),
        // Text: no NUL bytes (binary data claiming to be text) and decodable.
        t if is_text_extension(t) => {
            !head.contains(&0)
                && (std::str::from_utf8(head).is_ok()
                    || std::str::from_utf8(&head[..head.len().saturating_sub(3)]).is_ok())
        }
        _ => false,
    }
}

/// The value after `key=` in a header parameter, if `part` is that key.
fn param<'a>(part: &'a str, key: &str) -> Option<&'a str> {
    let (name, value) = part.split_once('=')?;
    name.trim()
        .eq_ignore_ascii_case(key)
        .then(|| value.trim().trim_matches('"'))
}

/// The file name from a `Content-Disposition` header (`filename*` first).
pub fn content_disposition_name(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').map(str::trim) {
        if let Some(value) = param(part, "filename*") {
            // RFC 5987: charset'language'percent-encoded-value
            let encoded = value.splitn(3, '\'').nth(2).unwrap_or(value);
            if let Some(decoded) = percent_decode(encoded).filter(|n| !n.trim().is_empty()) {
                return Some(decoded);
            }
        } else if let Some(value) = param(part, "filename") {
            plain = Some(value.to_string());
        }
    }
    plain.filter(|n| !n.trim().is_empty())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decode `%XX` escapes; `None` when the result is not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// The last path segment of a URL, percent-decoded.
fn url_file_name(url: &url::Url) -> Option<String> {
    let segment = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    percent_decode(segment)
}

/// Split a name into stem and lower-cased extension.
fn split_name(name: &str) -> (String, Option<String>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 5 => {
            (stem.to_string(), Some(ext.to_ascii_lowercase()))
        }
        _ => (name.to_string(), None),
    }
}

/// Decide the saved type: the server's media type when it is allowed, else
/// the name's extension when that is allowed.
fn decide_extension(mime: &str, name_ext: Option<&str>) -> Option<&'static str> {
    extension_for_mime(mime).or_else(|| {
        let ext = name_ext?;
        ALLOWED_TYPES
            .iter()
            .map(|(e, _)| *e)
            .find(|e| *e == ext)
            .filter(|_| {
                matches!(
                    mime,
                    "" | "application/octet-stream" | "binary/octet-stream"
                )
            })
    })
}

/// Hard-link `part` to the first free `stem.ext`, `stem (2).ext`, … and
/// remove `part`. Linking fails if the name exists, so nothing is replaced.
fn publish_part(part: &Path, dir: &Path, stem: &str, ext: &str) -> std::io::Result<PathBuf> {
    for n in 1..=999u32 {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let target = dir.join(name);
        match std::fs::hard_link(part, &target) {
            Ok(()) => {
                std::fs::remove_file(part)?;
                return Ok(target);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free file name",
    ))
}

/// Deletes the partial file unless disarmed.
struct PartGuard(Option<PathBuf>);

impl Drop for PartGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), error = %e, "removing a partial download failed");
                }
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    path: String,
    url: String,
    final_url: String,
    content_type: String,
    bytes: u64,
    sha256: String,
}

pub struct DownloadFileTool {
    host: Arc<AgentHost>,
}

impl DownloadFileTool {
    fn client(&self) -> &SafeClient {
        &self.host.web
    }

    fn check_web(&self) -> Result<(), ToolError> {
        match web_block_reason(&self.host.data_dir) {
            Some(reason) => Err(ToolError::Forbidden(reason)),
            None => Ok(()),
        }
    }

    async fn plan(&self, args: &Value) -> Result<(url::Url, FolderPlan), ToolError> {
        let tool = app_tools::DOWNLOAD_FILE;
        let raw_url = str_arg(args, "url").ok_or_else(|| invalid(tool, "`url` is required"))?;
        let url = shodh_rag::harness::web::client::check_url(raw_url).map_err(|e| match e {
            WebError::InvalidUrl(_) => invalid(tool, e.to_string()),
            other => ToolError::Forbidden(other.to_string()),
        })?;
        let folder =
            str_arg(args, "folder").ok_or_else(|| invalid(tool, "`folder` is required"))?;
        let folder = absolute_arg(tool, folder)?;
        let roots = self.host.roots.roots().await?;
        let plan = plan_folder(tool, &folder, &roots)?;
        Ok((url, plan))
    }
}

#[async_trait]
impl HostTool for DownloadFileTool {
    fn name(&self) -> &'static str {
        app_tools::DOWNLOAD_FILE
    }
    fn label(&self) -> &'static str {
        "Download file"
    }
    fn label_template(&self) -> &'static str {
        "Downloading[ {file_name}] from {url}"
    }
    fn description(&self) -> &'static str {
        "Download a document or image from the web (PDF, DOCX, XLSX, CSV, TXT, Markdown, HTML, \
         PNG, JPEG, GIF, WebP, TIFF; up to 100 MB) into a folder inside an indexed source folder, \
         e.g. <source>/Research/<topic> (created if missing), then index it so it is searchable. \
         Only download open-access files the user may keep. Never overwrites. Unavailable in \
         Local-only mode or when web access is off."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "minLength": 8, "maxLength": 2048},
                "folder": {"type": "string", "minLength": 3, "maxLength": 1024},
                "file_name": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "required": ["url", "folder"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        self.check_web()?;
        let (url, plan) = self.plan(args).await?;
        let target = plan.target();
        let name = str_arg(args, "file_name")
            .map(|n| sanitize_name(n, "download"))
            .or_else(|| url_file_name(&url).map(|n| sanitize_name(&n, "download")));
        Ok(ApprovalPreview {
            label: Some(format!("Download {} into {}", url, target.display())),
            details: json!({
                "url": url.as_str(),
                "folder": target.display().to_string(),
                "file": name,
                "newFolders": plan.missing,
                "size": "checked while downloading (at most 100 MB)",
            }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.check_web()?;
        let (url, plan) = self.plan(&args).await?;
        let fetched = self
            .client()
            .get(url.as_str(), vec![])
            .await
            .map_err(|e| match e {
                WebError::Blocked { .. }
                | WebError::Credentials
                | WebError::UnsupportedScheme(_) => ToolError::Forbidden(e.to_string()),
                other => ToolError::Unavailable(other.to_string()),
            })?;
        let final_url = fetched.final_url.clone();
        if !(200..300).contains(&fetched.status) {
            return Err(ToolError::Unavailable(format!(
                "{final_url} answered with status {}.",
                fetched.status
            )));
        }
        if let Some(length) = fetched
            .header("content-length")
            .and_then(|l| l.trim().parse::<u64>().ok())
        {
            if length > MAX_DOWNLOAD_BYTES {
                return Err(ToolError::Unavailable(format!(
                    "The file is {} MB; downloads are limited to {} MB.",
                    length / (1024 * 1024),
                    MAX_DOWNLOAD_BYTES / (1024 * 1024)
                )));
            }
        }
        let mime = fetched
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let suggested = str_arg(&args, "file_name")
            .map(str::to_string)
            .or_else(|| {
                fetched
                    .header("content-disposition")
                    .and_then(content_disposition_name)
            })
            .or_else(|| url_file_name(&final_url))
            .unwrap_or_else(|| "download".to_string());
        let (stem, name_ext) = split_name(&sanitize_name(&suggested, "download"));
        let ext = decide_extension(&mime, name_ext.as_deref()).ok_or_else(|| {
            ToolError::Forbidden(format!(
                "{final_url} is {}, which is not a downloadable document or image type.",
                if mime.is_empty() {
                    "of unknown type"
                } else {
                    &mime
                }
            ))
        })?;
        if !is_allowed_extension(ext) {
            return Err(ToolError::Forbidden(format!(
                "{ext} files are not downloaded."
            )));
        }

        let folder = create_planned(&plan)?;
        let part_path = folder.join(format!(".{stem}.{}.part", uuid::Uuid::new_v4().simple()));
        let mut part = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
            .map_err(|e| {
                ToolError::Unavailable(format!("Could not write in {}: {e}", folder.display()))
            })?;
        let mut guard = PartGuard(Some(part_path.clone()));
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut head: Vec<u8> = Vec::with_capacity(16 * 1024);
        let mut body = fetched.body;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| ToolError::Unavailable(e.to_string()))?;
            written += chunk.len() as u64;
            if written > MAX_DOWNLOAD_BYTES {
                return Err(ToolError::Unavailable(format!(
                    "The file is larger than {} MB; the download was stopped.",
                    MAX_DOWNLOAD_BYTES / (1024 * 1024)
                )));
            }
            if head.len() < 16 * 1024 {
                let take = (16 * 1024 - head.len()).min(chunk.len());
                head.extend_from_slice(&chunk[..take]);
            }
            hasher.update(&chunk);
            part.write_all(&chunk)
                .map_err(|e| ToolError::Unavailable(format!("Writing the download failed: {e}")))?;
        }
        part.sync_all()
            .map_err(|e| ToolError::Unavailable(format!("Writing the download failed: {e}")))?;
        drop(part);
        if written == 0 {
            return Err(ToolError::Unavailable(
                "The server sent an empty file.".to_string(),
            ));
        }
        if !magic_matches(ext, &head) {
            return Err(ToolError::Forbidden(format!(
                "The content does not look like a .{ext} file (its first bytes do not match); it was \
                 not saved."
            )));
        }
        let saved_path = publish_part(&part_path, &folder, &stem, ext)
            .map_err(|e| ToolError::Unavailable(format!("Saving the download failed: {e}")))?;
        guard.0 = None;
        let sha256 = hex_digest(hasher.finalize().as_slice());
        let shown = saved_path.display().to_string();
        let saved = Saved {
            path: shown.clone(),
            url: url.to_string(),
            final_url: final_url.to_string(),
            content_type: mime.clone(),
            bytes: written,
            sha256: sha256.clone(),
        };
        ctx.audit(
            AuditEventType::SourceChange,
            source_change(
                "download",
                ChangeOrigin::Agent,
                Some(&plan.root.source_id),
                Some(&shown),
                json!({
                    "ok": true,
                    "url": saved.url,
                    "final_url": saved.final_url,
                    "sha256": sha256,
                    "bytes": written,
                    "content_type": mime,
                    "at": chrono::Utc::now().to_rfc3339(),
                }),
            ),
        );
        self.host.effects.index_file(
            ctx,
            FileIndexJob {
                path: shown.clone(),
                source_id: plan.root.source_id.clone(),
            },
        );
        let name = saved_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| shown.clone());
        Ok(ToolOutput {
            text_for_model: format!(
                "Saved {shown} ({} KB, sha256 {}). It is being indexed and will be searchable shortly.",
                written.div_ceil(1024),
                &sha256[..12]
            ),
            summary_for_ui: format!("Saved {name} ({} KB)", written.div_ceil(1024)),
            detail: Some(serde_json::to_value(&saved).unwrap_or(Value::Null)),
        })
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── list_directory ─────────────────────────────────────────────────────────

/// One directory entry, with the path as spelled on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: Option<u64>,
    pub modified_ms: Option<u64>,
    pub extension: Option<String>,
    /// The file type can be indexed.
    pub supported: bool,
}

/// What a listing found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub path: String,
    pub entries: Vec<DirEntry>,
    /// More entries existed than were returned.
    pub truncated: bool,
}

fn entry_for(path: &Path, meta: &std::fs::Metadata) -> DirEntry {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = (!meta.is_dir())
        .then(|| {
            path.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
        })
        .flatten();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| u64::try_from(d.as_millis()).ok());
    DirEntry {
        supported: extension
            .as_deref()
            .is_some_and(shodh_rag::indexing::is_supported_file_type),
        name,
        path: path.display().to_string(),
        is_dir: meta.is_dir(),
        size_bytes: (!meta.is_dir()).then(|| meta.len()),
        modified_ms,
        extension,
    }
}

/// List `path` (which must be inside a source root), folders first, by
/// name. Recursive listings do not follow symlinked folders.
pub fn list_directory_in(
    path: &Path,
    recursive: bool,
    limit: usize,
    roots: &[SourceRoot],
) -> Result<Listing, ToolError> {
    let canonical = std::fs::canonicalize(path)
        .map(|p| strip_verbatim(&p))
        .map_err(|e| ToolError::NotFound(format!("{} cannot be opened: {e}", path.display())))?;
    if !canonical.is_dir() {
        return Err(ToolError::NotFound(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    if root_for(&canonical, roots).is_none() {
        return Err(outside_sources(path));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut pending = vec![canonical.clone()];
    'walk: while let Some(dir) = pending.pop() {
        let mut level: Vec<(PathBuf, std::fs::Metadata)> = std::fs::read_dir(&dir)
            .map_err(|e| ToolError::Unavailable(format!("{} cannot be read: {e}", dir.display())))?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let p = e.path();
                std::fs::symlink_metadata(&p).ok().map(|m| (p, m))
            })
            .filter(|(p, _)| {
                !p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(".part"))
                    .unwrap_or(false)
            })
            .collect();
        level.sort_by(|(a, am), (b, bm)| {
            bm.is_dir()
                .cmp(&am.is_dir())
                .then_with(|| a.file_name().cmp(&b.file_name()))
        });
        for (p, meta) in level {
            if entries.len() >= limit {
                truncated = true;
                break 'walk;
            }
            let is_link = meta.file_type().is_symlink();
            let meta = if is_link {
                match std::fs::metadata(&p) {
                    Ok(m) => m,
                    Err(_) => continue,
                }
            } else {
                meta
            };
            if recursive && meta.is_dir() && !is_link {
                pending.push(p.clone());
            }
            entries.push(entry_for(&p, &meta));
        }
    }
    Ok(Listing {
        path: canonical.display().to_string(),
        entries,
        truncated,
    })
}

pub struct ListDirectoryTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ListDirectoryTool {
    fn name(&self) -> &'static str {
        app_tools::LIST_DIRECTORY
    }
    fn label(&self) -> &'static str {
        "List folder"
    }
    fn label_template(&self) -> &'static str {
        "Listing {path}"
    }
    fn description(&self) -> &'static str {
        "List the files and folders in a folder inside an indexed source folder (names, sizes, \
         dates, and whether each file type can be indexed). recursive lists subfolders too."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 3, "maxLength": 1024},
                "recursive": {"type": "boolean"},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LISTED_ENTRIES}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::LIST_DIRECTORY;
        let path = str_arg(&args, "path").ok_or_else(|| invalid(tool, "`path` is required"))?;
        let path = absolute_arg(tool, path)?;
        let recursive = args
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let limit = limit_arg(&args, DEFAULT_LISTED_ENTRIES, MAX_LISTED_ENTRIES);
        let roots = self.host.roots.roots().await?;
        let listing =
            tokio::task::spawn_blocking(move || list_directory_in(&path, recursive, limit, &roots))
                .await
                .map_err(|e| ToolError::Failed(format!("Listing was interrupted: {e}")))??;
        let folders = listing.entries.iter().filter(|e| e.is_dir).count();
        let files = listing.entries.len() - folders;
        let lines: Vec<String> = listing
            .entries
            .iter()
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.path)
                } else {
                    format!(
                        "{} ({} KB{})",
                        e.path,
                        e.size_bytes.unwrap_or(0).div_ceil(1024),
                        if e.supported { "" } else { ", not indexable" }
                    )
                }
            })
            .collect();
        let more = if listing.truncated {
            "\n[more entries exist; raise limit or list a subfolder]"
        } else {
            ""
        };
        Ok(ToolOutput {
            text_for_model: format!(
                "{} folders and {} files in {}:\n{}{more}",
                folders,
                files,
                listing.path,
                lines.join("\n")
            ),
            summary_for_ui: format!("{folders} folders, {files} files"),
            detail: Some(serde_json::to_value(&listing).unwrap_or(Value::Null)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use shodh_rag::harness::web::client::{HttpRequest, HttpResponse, HttpTransport, Resolver};
    use std::collections::HashMap;
    use std::net::IpAddr;

    /// In-process DNS + HTTP: hosts resolve to fixed addresses, URLs answer
    /// with canned responses.
    #[derive(Default)]
    struct FakeNet {
        hosts: HashMap<String, Vec<IpAddr>>,
        routes: HashMap<String, (u16, Vec<(String, String)>, Vec<u8>)>,
    }

    impl FakeNet {
        fn host(mut self, host: &str, ip: &str) -> Self {
            self.hosts.insert(host.into(), vec![ip.parse().unwrap()]);
            self
        }
        fn route(
            mut self,
            url: &str,
            status: u16,
            headers: &[(&str, &str)],
            body: Vec<u8>,
        ) -> Self {
            let headers = headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self.routes.insert(url.into(), (status, headers, body));
            self
        }
        fn client(self) -> SafeClient {
            let net = Arc::new(self);
            SafeClient::new(net.clone(), net)
        }
    }

    #[async_trait]
    impl Resolver for FakeNet {
        async fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            self.hosts
                .get(host)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unknown host"))
        }
    }

    #[async_trait]
    impl HttpTransport for FakeNet {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, WebError> {
            let (status, headers, body) = self
                .routes
                .get(request.url.as_str())
                .cloned()
                .ok_or(WebError::Status { status: 404 })?;
            let chunks: Vec<Result<Bytes, WebError>> = body
                .chunks(64 * 1024)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect();
            Ok(HttpResponse {
                status,
                headers,
                body: Box::pin(futures::stream::iter(chunks)),
            })
        }
    }

    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\ntrailer\n%%EOF\n";

    fn net() -> FakeNet {
        FakeNet::default()
            .host("arxiv.org", "151.101.3.42")
            .route(
                "https://arxiv.org/pdf/1706.03762",
                200,
                &[
                    ("content-type", "application/pdf"),
                    (
                        "content-disposition",
                        "attachment; filename=\"Attention Is All You Need.pdf\"",
                    ),
                ],
                PDF.to_vec(),
            )
            .route(
                "https://arxiv.org/fake.pdf",
                200,
                &[("content-type", "application/pdf")],
                b"<html>not a pdf</html>".to_vec(),
            )
            .route(
                "https://arxiv.org/big.pdf",
                200,
                &[
                    ("content-type", "application/pdf"),
                    ("content-length", "209715200"),
                ],
                PDF.to_vec(),
            )
            .route(
                "https://arxiv.org/stream.pdf",
                200,
                &[("content-type", "application/pdf")],
                {
                    let mut body = PDF.to_vec();
                    body.resize(usize::try_from(MAX_DOWNLOAD_BYTES).unwrap() + 1, b' ');
                    body
                },
            )
            .route(
                "https://arxiv.org/to-local",
                302,
                &[("location", "http://192.168.1.1/router.pdf")],
                Vec::new(),
            )
            .route(
                "https://arxiv.org/app.exe",
                200,
                &[("content-type", "application/octet-stream")],
                b"MZ\x90\x00".to_vec(),
            )
    }

    async fn setup() -> (testing::TestHost, PathBuf, Arc<AgentHost>) {
        let t = testing::host().await;
        let source = t.dir.path().join("Library");
        std::fs::create_dir_all(&source).unwrap();
        testing::index_folder(&t, &source).await;
        let host = testing::with_web(&t, net().client());
        let source = std::fs::canonicalize(&source)
            .map(|p| strip_verbatim(&p))
            .unwrap();
        (t, source, host)
    }

    fn download(host: &Arc<AgentHost>) -> DownloadFileTool {
        DownloadFileTool { host: host.clone() }
    }

    #[test]
    fn folder_names_are_checked_segment_by_segment() {
        let tool = "create_folder";
        assert_eq!(
            relative_segments(tool, "Research/Transformers").unwrap(),
            vec!["Research", "Transformers"]
        );
        for bad in [
            "..",
            "a/../b",
            "CON",
            "nul.txt",
            "trailing.",
            "trailing ",
            "a:b",
            "a*b",
        ] {
            assert!(relative_segments(tool, bad).is_err(), "{bad}");
        }
        assert!(relative_segments(tool, "/").is_err());
    }

    #[tokio::test]
    async fn create_folder_stays_inside_source_folders() {
        let (t, source, host) = setup().await;
        let tool = CreateFolderTool { host: host.clone() };
        let (ctx, _rx) = testing::ctx();
        let args = json!({"parent": source.display().to_string(), "name": "Research/Transformers"});
        let preview = tool.preview(&args).await.unwrap();
        assert_eq!(
            preview.details["newFolders"],
            json!(["Research", "Transformers"])
        );
        tool.execute(args.clone(), &ctx).await.unwrap();
        assert!(source.join("Research").join("Transformers").is_dir());
        assert!(tool.preview(&args).await.is_err(), "already exists");
        assert_eq!(t.effects.library.lock().unwrap().len(), 1);

        let outside = t.dir.path().join("Elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let err = tool
            .execute(
                json!({"parent": outside.display().to_string(), "name": "x"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Forbidden(_)));
        assert!(!outside.join("x").exists());
        let err = tool
            .execute(
                json!({"parent": source.display().to_string(), "name": "../escape"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_cannot_lead_outside_a_source() {
        let (t, source, host) = setup().await;
        let outside = t.dir.path().join("Outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, source.join("link")).unwrap();
        let (ctx, _rx) = testing::ctx();
        let err = CreateFolderTool { host }
            .execute(
                json!({"parent": source.join("link").display().to_string(), "name": "x"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Forbidden(_)));
        assert!(!outside.join("x").exists());
    }

    #[tokio::test]
    async fn downloads_are_saved_named_hashed_indexed_and_never_overwrite() {
        let (t, source, host) = setup().await;
        let (ctx, _rx) = testing::ctx();
        let folder = source.join("Research").join("Transformers");
        let args = json!({
            "url": "https://arxiv.org/pdf/1706.03762",
            "folder": folder.display().to_string(),
        });
        let preview = download(&host).preview(&args).await.unwrap();
        assert_eq!(preview.details["url"], "https://arxiv.org/pdf/1706.03762");
        let first = download(&host).execute(args.clone(), &ctx).await.unwrap();
        let second = download(&host).execute(args, &ctx).await.unwrap();
        let p1 = first.detail.unwrap()["path"].as_str().unwrap().to_string();
        let detail = second.detail.unwrap();
        let p2 = detail["path"].as_str().unwrap().to_string();
        assert!(p1.ends_with("Attention Is All You Need.pdf"), "{p1}");
        assert!(p2.ends_with("Attention Is All You Need (2).pdf"), "{p2}");
        assert_eq!(std::fs::read(&p1).unwrap(), PDF);
        assert_eq!(detail["sha256"].as_str().unwrap().len(), 64);
        let leftovers = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .count();
        assert_eq!(leftovers, 0, "no partial files remain");
        let indexed = t.effects.indexed_files.lock().unwrap();
        assert_eq!(indexed.len(), 2);
        assert_eq!(indexed[0].source_id, "source-1");
    }

    #[tokio::test]
    async fn bad_downloads_leave_nothing_behind() {
        let (_t, source, host) = setup().await;
        let (ctx, _rx) = testing::ctx();
        let folder = source.display().to_string();
        let cases = [
            ("https://arxiv.org/fake.pdf", "does not look like"),
            ("https://arxiv.org/big.pdf", "limited to 100 MB"),
            ("https://arxiv.org/stream.pdf", "larger than 100 MB"),
            ("https://arxiv.org/app.exe", "not a downloadable"),
        ];
        for (url, expected) in cases {
            let err = download(&host)
                .execute(json!({"url": url, "folder": folder}), &ctx)
                .await
                .unwrap_err();
            assert!(err.to_string().contains(expected), "{url}: {err}");
        }
        let redirect = download(&host)
            .execute(
                json!({"url": "https://arxiv.org/to-local", "folder": folder}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(redirect, ToolError::Forbidden(_)), "{redirect}");
        let entries = std::fs::read_dir(&source).unwrap().count();
        assert_eq!(entries, 0, "nothing written");
        let elsewhere = std::env::temp_dir().display().to_string();
        let outside = download(&host)
            .preview(&json!({"url": "https://arxiv.org/pdf/1706.03762", "folder": elsewhere}))
            .await;
        assert!(outside.is_err());
    }

    #[tokio::test]
    async fn downloads_respect_the_web_policy() {
        let (t, source, host) = setup().await;
        crate::app_settings::SettingsStore::in_dir(&t.host.data_dir)
            .update(|s| {
                s.policy.local_only = true;
                Ok(())
            })
            .unwrap();
        let (ctx, _rx) = testing::ctx();
        let err = download(&host)
            .execute(
                json!({
                    "url": "https://arxiv.org/pdf/1706.03762",
                    "folder": source.display().to_string(),
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Local-only"));
    }

    #[test]
    fn content_disposition_and_magic_bytes() {
        let encoded = "attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf; filename=\"x.pdf\"";
        assert_eq!(
            content_disposition_name(encoded),
            Some("r\u{e9}sum\u{e9}.pdf".to_string())
        );
        assert_eq!(
            content_disposition_name("inline; filename=\"a b.pdf\""),
            Some("a b.pdf".to_string())
        );
        assert_eq!(content_disposition_name("inline"), None);
        assert!(magic_matches("pdf", PDF));
        assert!(!magic_matches("pdf", b"<html>"));
        assert!(magic_matches("png", b"\x89PNG\r\n\x1a\n...."));
        assert!(magic_matches("docx", b"PK\x03\x04...."));
        assert!(magic_matches("csv", b"a,b\n1,2\n"));
        assert!(!magic_matches("txt", b"MZ\x90\x00\x03"));
        assert_eq!(
            decide_extension("application/pdf", Some("bin")),
            Some("pdf")
        );
        assert_eq!(
            decide_extension("application/octet-stream", Some("pdf")),
            Some("pdf")
        );
        assert_eq!(
            decide_extension("application/octet-stream", Some("exe")),
            None
        );
        assert_eq!(decide_extension("text/html", Some("pdf")), Some("html"));
    }

    #[tokio::test]
    async fn list_directory_is_confined_and_orders_folders_first() {
        let (t, source, host) = setup().await;
        std::fs::create_dir_all(source.join("Beta")).unwrap();
        std::fs::write(source.join("alpha.pdf"), PDF).unwrap();
        std::fs::write(source.join("Beta").join("notes.xyz"), b"x").unwrap();
        let (ctx, _rx) = testing::ctx();
        let tool = ListDirectoryTool { host };
        let out = tool
            .execute(
                json!({"path": source.display().to_string(), "recursive": true}),
                &ctx,
            )
            .await
            .unwrap();
        let entries = out.detail.unwrap()["entries"].clone();
        assert_eq!(entries[0]["name"], "Beta");
        assert_eq!(entries[0]["isDir"], true);
        assert_eq!(entries[1]["name"], "alpha.pdf");
        assert_eq!(entries[1]["supported"], true);
        assert_eq!(entries[2]["name"], "notes.xyz");
        assert_eq!(entries[2]["supported"], false);
        let outside = tool
            .execute(json!({"path": t.dir.path().display().to_string()}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(outside, ToolError::Forbidden(_)));
        let limited = tool
            .execute(
                json!({"path": source.display().to_string(), "recursive": true, "limit": 1}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(limited.detail.unwrap()["truncated"], true);
    }
}
