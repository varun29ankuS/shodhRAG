//! Creator mode's creations: the folder each Creator conversation builds in,
//! the versions Shodh keeps of it, and what the Artifacts tab reads.
//!
//! - **The folder.** `<Documents>/Shodh Creations/<name>`, made fresh for the
//!   conversation's first request ([`CreationStore::prepare`]): created with
//!   an exclusive `create_dir`, never an existing folder adopted, under a
//!   `Shodh Creations` folder that must not be a link. The session works in
//!   it as its code folder: the guard keeps every file tool inside it, and on
//!   macOS the sandbox's writable folder is this folder.
//! - **Versions.** At the end of each answer the folder's files are copied
//!   into Shodh's data folder ([`CreationStore::snapshot`]): content-addressed
//!   blobs and one manifest per version, so a later change never rewrites an
//!   earlier version. Dependency and cache folders (`node_modules`, `.git`,
//!   `target`, ...) and links are left out, with caps on count and size.
//! - **Reads.** Everything Shodh reads from the folder outside the sandbox
//!   (the file list, a file, the preview) goes by the creation's id and a
//!   relative path: links are never followed, and a file must resolve inside
//!   the folder, checked again once it is open.
//! - **Delete** removes the folder only when it is still a direct child of
//!   `Shodh Creations` and not a link, then its versions and its record.

pub mod diff;
pub mod handle;
pub mod preview;
pub mod preview_window;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent_tools::files::is_reserved_windows_name;
use crate::durable_file::{read_with_backup, write_atomic, write_with_backup, Origin};

/// The folder in Documents that holds every creation.
pub const CREATIONS_FOLDER: &str = "Shodh Creations";
/// Shodh's own records and versions, in its data folder.
const STORE_DIR: &str = "creations";
const STORE_FILE: &str = "creations.json";
const STORE_VERSION: u32 = 1;

/// Folders never copied into a version or listed: dependencies, caches,
/// build tool state and git's own folder.
pub const SKIPPED_DIRS: [&str; 13] = [
    "node_modules",
    ".git",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".cache",
    ".parcel-cache",
    ".turbo",
    ".pytest_cache",
    ".mypy_cache",
];
/// Most files a version keeps (the rest are counted as left out).
pub const MAX_FILES: usize = 2_000;
/// Largest file a version keeps.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Most bytes one version keeps.
pub const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;
/// Largest file shown as text in the Artifacts tab.
pub const MAX_VIEW_BYTES: usize = 512 * 1024;
/// Longest creation title, in characters.
pub const MAX_TITLE_CHARS: usize = 80;
/// Longest folder name made from a request, in characters.
const MAX_SLUG_CHARS: usize = 48;
/// Suffixes tried for a free folder name.
const MAX_SLUG_SUFFIX: u32 = 999;

/// Where a creation is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CreationStatus {
    /// The folder exists; no answer has finished in it yet.
    Building,
    /// At least one answer finished: a version exists.
    Ready,
    /// The user chose Keep on the finish card.
    Kept,
}

/// One saved version of a creation's folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// From 1, in order.
    pub number: u32,
    pub created_at: String,
    /// The answer that ended with it.
    pub run_id: Option<String>,
    pub files: usize,
    pub bytes: u64,
    /// Files left out by the caps.
    pub left_out: usize,
}

/// A creation's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Creation {
    /// `cr_` and 32 hex digits.
    pub id: String,
    /// The conversation that builds it (the Artifacts tab links back to it).
    pub conversation_id: String,
    pub title: String,
    /// The creation folder (resolved).
    pub folder: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: CreationStatus,
    /// What the agent said it built (`finish_creation`).
    #[serde(default)]
    pub summary: Option<String>,
    /// The choices the agent made that the user did not specify.
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub versions: Vec<VersionInfo>,
    /// The folder's identity on disk when it was made ([`handle::identity`]):
    /// a folder later made under the same name is not this creation's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
}

impl Creation {
    pub fn latest_version(&self) -> Option<u32> {
        self.versions.last().map(|v| v.number)
    }

    /// The creation's folder, resolved, while it is still a direct child of
    /// `root`, not a link, and the very folder it was made in.
    pub fn checked_folder(&self, root: &Path) -> Result<PathBuf, String> {
        let folder = checked_folder(root, Path::new(&self.folder))?;
        let Some(expected) = &self.folder_id else {
            return Err(format!(
                "Shodh cannot tell whether {} is still this creation's folder, so it leaves it \
                 alone",
                self.folder
            ));
        };
        match handle::identity(&folder) {
            Ok(now) if &now == expected => Ok(folder),
            Ok(_) => Err(format!(
                "{} is now another folder than the one this creation was made in, so Shodh \
                 leaves it alone",
                self.folder
            )),
            Err(e) => Err(format!("{} could not be checked ({e})", self.folder)),
        }
    }
}

/// One file of a version's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    /// Relative to the folder, `/`-separated.
    pub path: String,
    /// SHA-256 of its content, hex.
    pub hash: String,
    pub size: u64,
}

/// A file as the Artifacts tab lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
}

/// A file's content as the Artifacts tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub path: String,
    pub size: u64,
    /// The text, when the file is text (cut at [`MAX_VIEW_BYTES`]).
    pub text: Option<String>,
    pub binary: bool,
    pub truncated: bool,
}

/// How one file differs between two versions (the shared diff's own type).
pub use diff::FileChange;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    creations: Vec<Creation>,
}

/// One read-modify-write of the store at a time, across every caller.
static STORE_LOCK: Mutex<()> = Mutex::new(());

/// Whether `id` has the form of a creation id: it names a folder in the
/// store, so nothing else is accepted.
pub fn valid_id(id: &str) -> bool {
    id.len() == 35
        && id.starts_with("cr_")
        && id[3..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn new_id() -> String {
    format!("cr_{}", uuid::Uuid::new_v4().simple())
}

/// The title of a request: its first line, on one line, at most
/// [`MAX_TITLE_CHARS`].
pub fn title_of(request: &str) -> String {
    let line = request
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let flat: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title: String = flat.chars().take(MAX_TITLE_CHARS).collect();
    if title.is_empty() {
        title = "Untitled creation".to_string();
    }
    title
}

/// Words of a request that say little about what is made.
const FILLER: [&str; 22] = [
    "a", "an", "the", "make", "build", "create", "me", "please", "can", "you", "for", "of", "with",
    "and", "to", "i", "want", "need", "write", "some", "my", "that",
];

/// A folder name from a request: lowercase ASCII words joined by `-`, at
/// most [`MAX_SLUG_CHARS`], never a reserved Windows name and never ending in
/// a dot or space; `creation` when nothing usable is left.
pub fn slug_of(request: &str) -> String {
    let words: Vec<String> = request
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let meaningful: Vec<&String> = words
        .iter()
        .filter(|w| !FILLER.contains(&w.as_str()))
        .collect();
    let chosen: Vec<&String> = if meaningful.is_empty() {
        words.iter().collect()
    } else {
        meaningful
    };
    let mut slug = String::new();
    for word in chosen {
        let next = if slug.is_empty() {
            word.clone()
        } else {
            format!("{slug}-{word}")
        };
        if next.len() > MAX_SLUG_CHARS {
            break;
        }
        slug = next;
    }
    if slug.is_empty() {
        slug = "creation".to_string();
    }
    if is_reserved_windows_name(&slug) {
        slug.push_str("-creation");
    }
    slug
}

/// `\\?\C:\x` → `C:\x`, as the code folder spells it.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// Whether the entry at `path` is a link (a symlink, or a junction on
/// Windows), without following it.
fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// `relative` as a path of plain parts (no root, drive, `.` or `..`), or
/// `None`.
pub fn plain_relative(relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains('\0') || relative.contains('\\') {
        return None;
    }
    let path = Path::new(relative);
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => out.push(name),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// The resolved `Shodh Creations` folder in `documents`, created when
/// missing. Refused when it is a link: a creation folder must be where it
/// says it is.
pub fn creations_root(documents: &Path) -> Result<PathBuf, String> {
    let root = documents.join(CREATIONS_FOLDER);
    fs::create_dir_all(&root)
        .map_err(|e| format!("{} could not be created ({e})", root.display()))?;
    if is_link(&root) {
        return Err(format!(
            "{} is a link to another folder, so Shodh does not make creations in it",
            root.display()
        ));
    }
    fs::canonicalize(&root)
        .map(plain)
        .map_err(|e| format!("{} could not be opened ({e})", root.display()))
}

/// `folder` resolved, when it is still a direct child of `root` (resolved)
/// and not a link: what Shodh may delete or read as a creation folder.
pub fn checked_folder(root: &Path, folder: &Path) -> Result<PathBuf, String> {
    if is_link(folder) {
        return Err(format!(
            "{} is now a link to another folder, so Shodh leaves it alone",
            folder.display()
        ));
    }
    let resolved = fs::canonicalize(folder)
        .map(plain)
        .map_err(|e| format!("{} could not be opened ({e})", folder.display()))?;
    if resolved.parent() != Some(root) || !resolved.is_dir() {
        return Err(format!(
            "{} is no longer a folder in {CREATIONS_FOLDER}, so Shodh leaves it alone",
            folder.display()
        ));
    }
    Ok(resolved)
}

/// The files of `folder` a version keeps and lists: regular files only (no
/// link is followed or listed), outside [`SKIPPED_DIRS`], with UTF-8 paths,
/// sorted, within the caps. Returns them with the count left out.
fn walk(folder: &Path) -> (Vec<(String, PathBuf, u64)>, usize) {
    let mut found = Vec::new();
    let mut left_out = 0usize;
    let mut total = 0u64;
    let entries = walkdir::WalkDir::new(folder)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !(entry.file_type().is_dir()
                    && SKIPPED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()))
        });
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(folder) else {
            continue;
        };
        let parts: Option<Vec<&str>> = relative
            .components()
            .map(|c| match c {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect();
        let Some(parts) = parts else {
            left_out += 1;
            continue;
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
        if found.len() >= MAX_FILES
            || size > MAX_FILE_BYTES
            || total.saturating_add(size) > MAX_TOTAL_BYTES
        {
            left_out += 1;
            continue;
        }
        total += size;
        found.push((parts.join("/"), entry.path().to_path_buf(), size));
    }
    (found, left_out)
}

/// Read `relative` inside `root` (resolved), at most `max` bytes, with its
/// full size. A link along the path is refused at once (a plain message);
/// what decides is the open file itself ([`handle::open_inside`]): a regular
/// file with one name whose real place is inside `root`, so a folder of the
/// path swapped for a link after this check still reads nothing outside.
fn read_inside(root: &Path, relative: &Path, max: u64) -> io::Result<(Vec<u8>, u64)> {
    read_inside_with(root, relative, max, || {})
}

/// [`read_inside`], with `before_open` run between the path check and the
/// open (tests swap a folder for a link there).
fn read_inside_with(
    root: &Path,
    relative: &Path,
    max: u64,
    before_open: impl FnOnce(),
) -> io::Result<(Vec<u8>, u64)> {
    let mut at = root.to_path_buf();
    for part in relative.components() {
        at.push(part);
        if is_link(&at) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the path goes through a link",
            ));
        }
    }
    before_open();
    let (file, size) = handle::open_inside(root, &at)?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes)?;
    Ok((bytes, size))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Whether `bytes` read as text: UTF-8 without NUL bytes (a cut may split
/// the last character, which is allowed).
fn as_text(bytes: &[u8], cut: bool) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_string()),
        Err(e) if cut && e.error_len().is_none() => {
            Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

/// Creations' records and versions, in Shodh's data folder.
#[derive(Debug, Clone)]
pub struct CreationStore {
    dir: PathBuf,
}

impl CreationStore {
    pub fn in_dir(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(STORE_DIR),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(STORE_FILE)
    }

    fn versions_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    fn manifest_path(&self, id: &str, number: u32) -> PathBuf {
        self.versions_dir(id).join(format!("v{number}.json"))
    }

    fn blob_path(&self, id: &str, hash: &str) -> PathBuf {
        self.versions_dir(id).join("blobs").join(hash)
    }

    fn load(&self) -> Result<(StoreFile, Origin), String> {
        let loaded = read_with_backup(
            &self.path(),
            |text| serde_json::from_str::<StoreFile>(text).map_err(|e| e.to_string()),
            StoreFile::default,
        )
        .map_err(|e| e.to_string())?;
        Ok((loaded.value, loaded.origin))
    }

    /// Change the records under the store's lock and save them.
    fn update<R>(
        &self,
        change: impl FnOnce(&mut Vec<Creation>) -> Result<R, String>,
    ) -> Result<R, String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut file, origin) = self.load()?;
        let result = change(&mut file.creations)?;
        file.version = STORE_VERSION;
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        write_with_backup(&self.path(), &bytes, &origin)
            .map_err(|e| format!("the creations could not be saved ({e})"))?;
        Ok(result)
    }

    /// Every creation, the most recently changed first.
    pub fn list(&self) -> Result<Vec<Creation>, String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut creations = self.load()?.0.creations;
        creations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(creations)
    }

    pub fn get(&self, id: &str) -> Result<Option<Creation>, String> {
        Ok(self.list()?.into_iter().find(|c| c.id == id))
    }

    pub fn for_conversation(&self, conversation_id: &str) -> Result<Option<Creation>, String> {
        Ok(self
            .list()?
            .into_iter()
            .find(|c| c.conversation_id == conversation_id))
    }

    /// The creation of `conversation_id`: its existing one, or a new folder
    /// in `root` ([`creations_root`]) named from `request`.
    pub fn prepare(
        &self,
        root: &Path,
        conversation_id: &str,
        request: &str,
        now: &str,
    ) -> Result<Creation, String> {
        self.update(|creations| {
            if let Some(existing) = creations
                .iter()
                .find(|c| c.conversation_id == conversation_id)
            {
                return Ok(existing.clone());
            }
            let folder = new_folder(root, &slug_of(request))?;
            let folder_id = handle::identity(&folder)
                .map_err(|e| format!("{} could not be checked ({e})", folder.display()))?;
            let creation = Creation {
                id: new_id(),
                conversation_id: conversation_id.to_string(),
                title: title_of(request),
                folder: folder.display().to_string(),
                created_at: now.to_string(),
                updated_at: now.to_string(),
                status: CreationStatus::Building,
                summary: None,
                choices: Vec::new(),
                versions: Vec::new(),
                folder_id: Some(folder_id),
            };
            creations.push(creation.clone());
            Ok(creation)
        })
    }

    /// Record what the agent said it built and chose (`finish_creation`).
    pub fn record_finish(
        &self,
        conversation_id: &str,
        summary: &str,
        choices: Vec<String>,
        now: &str,
    ) -> Result<(), String> {
        self.update(|creations| {
            if let Some(creation) = creations
                .iter_mut()
                .find(|c| c.conversation_id == conversation_id)
            {
                creation.summary = Some(summary.to_string());
                creation.choices = choices;
                creation.updated_at = now.to_string();
            }
            Ok(())
        })
    }

    /// The user chose Keep.
    pub fn keep(&self, id: &str, now: &str) -> Result<Creation, String> {
        self.update(|creations| {
            let creation = creations
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or_else(|| "That creation no longer exists.".to_string())?;
            creation.status = CreationStatus::Kept;
            creation.updated_at = now.to_string();
            Ok(creation.clone())
        })
    }

    /// Save the folder of `conversation_id`'s creation as a new version,
    /// unless nothing changed since the last one. Returns the new version.
    pub fn snapshot(
        &self,
        root: &Path,
        conversation_id: &str,
        run_id: Option<&str>,
        now: &str,
    ) -> Result<Option<VersionInfo>, String> {
        let Some(creation) = self.for_conversation(conversation_id)? else {
            return Ok(None);
        };
        let folder = creation.checked_folder(root)?;
        let (files, mut left_out) = walk(&folder);
        let mut manifest = Vec::with_capacity(files.len());
        let mut bytes_total = 0u64;
        for (relative, _, _) in &files {
            let Some(path) = plain_relative(relative) else {
                left_out += 1;
                continue;
            };
            let (bytes, size) = match read_inside(&folder, &path, MAX_FILE_BYTES + 1) {
                Ok(read) if read.1 <= MAX_FILE_BYTES => read,
                _ => {
                    left_out += 1;
                    continue;
                }
            };
            let hash = sha256_hex(&bytes);
            let blob = self.blob_path(&creation.id, &hash);
            if !blob.is_file() {
                write_atomic(&blob, &bytes)
                    .map_err(|e| format!("a version could not be saved ({e})"))?;
            }
            bytes_total += size;
            manifest.push(ManifestEntry {
                path: relative.clone(),
                hash,
                size,
            });
        }
        if let Some(last) = creation.latest_version() {
            if self.manifest(&creation.id, last)? == manifest {
                return Ok(None);
            }
        }
        let number = creation.latest_version().map_or(1, |n| n + 1);
        let encoded = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
        write_atomic(&self.manifest_path(&creation.id, number), &encoded)
            .map_err(|e| format!("a version could not be saved ({e})"))?;
        let info = VersionInfo {
            number,
            created_at: now.to_string(),
            run_id: run_id.map(str::to_string),
            files: manifest.len(),
            bytes: bytes_total,
            left_out,
        };
        let id = creation.id.clone();
        let saved = info.clone();
        self.update(move |creations| {
            if let Some(creation) = creations.iter_mut().find(|c| c.id == id) {
                creation.versions.push(saved);
                if creation.status == CreationStatus::Building {
                    creation.status = CreationStatus::Ready;
                }
                creation.updated_at = now.to_string();
            }
            Ok(())
        })?;
        Ok(Some(info))
    }

    /// The files of version `number` of `id`.
    pub fn manifest(&self, id: &str, number: u32) -> Result<Vec<ManifestEntry>, String> {
        let path = self.manifest_path(id, number);
        let bytes = fs::read(&path).map_err(|e| format!("version {number} is missing ({e})"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("version {number} is damaged ({e})"))
    }

    fn creation(&self, id: &str) -> Result<Creation, String> {
        if !valid_id(id) {
            return Err("That is not a creation id.".to_string());
        }
        self.get(id)?
            .ok_or_else(|| "That creation no longer exists.".to_string())
    }

    fn version_of(creation: &Creation, number: u32) -> Result<u32, String> {
        creation
            .versions
            .iter()
            .any(|v| v.number == number)
            .then_some(number)
            .ok_or_else(|| format!("{} has no version {number}.", creation.title))
    }

    /// The files of a version (`Some`), or of the folder as it is now.
    pub fn files(
        &self,
        root: &Path,
        id: &str,
        version: Option<u32>,
    ) -> Result<Vec<FileEntry>, String> {
        let creation = self.creation(id)?;
        match version {
            Some(number) => Ok(self
                .manifest(id, Self::version_of(&creation, number)?)?
                .into_iter()
                .map(|entry| FileEntry {
                    path: entry.path,
                    size: entry.size,
                })
                .collect()),
            None => {
                let folder = creation.checked_folder(root)?;
                Ok(walk(&folder)
                    .0
                    .into_iter()
                    .map(|(path, _, size)| FileEntry { path, size })
                    .collect())
            }
        }
    }

    /// The bytes of `relative` in a version (`Some`) or in the folder now, at
    /// most `max`, with its full size.
    pub fn read(
        &self,
        root: &Path,
        id: &str,
        version: Option<u32>,
        relative: &str,
        max: u64,
    ) -> Result<(Vec<u8>, u64), String> {
        let creation = self.creation(id)?;
        let path = plain_relative(relative).ok_or_else(|| format!("{relative} is not a path"))?;
        match version {
            Some(number) => {
                let manifest = self.manifest(id, Self::version_of(&creation, number)?)?;
                let entry = manifest
                    .iter()
                    .find(|entry| entry.path == relative)
                    .ok_or_else(|| format!("{relative} is not in version {number}"))?;
                let blob = self.blob_path(id, &entry.hash);
                let file = fs::File::open(&blob).map_err(|e| format!("{relative}: {e}"))?;
                let mut bytes = Vec::new();
                file.take(max)
                    .read_to_end(&mut bytes)
                    .map_err(|e| format!("{relative}: {e}"))?;
                Ok((bytes, entry.size))
            }
            None => {
                let folder = creation.checked_folder(root)?;
                if path
                    .components()
                    .any(|c| SKIPPED_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref()))
                {
                    return Err(format!("{relative} is in a folder Shodh does not show"));
                }
                read_inside(&folder, &path, max).map_err(|e| format!("{relative}: {e}"))
            }
        }
    }

    /// A file as the Artifacts tab shows it.
    pub fn file(
        &self,
        root: &Path,
        id: &str,
        version: Option<u32>,
        relative: &str,
    ) -> Result<FileContent, String> {
        let (bytes, size) = self.read(root, id, version, relative, MAX_VIEW_BYTES as u64)?;
        let truncated = size > bytes.len() as u64;
        let text = as_text(&bytes, truncated);
        Ok(FileContent {
            path: relative.to_string(),
            size,
            binary: text.is_none(),
            text,
            truncated,
        })
    }

    /// How version `to` differs from version `from` (0: from nothing).
    pub fn diff(&self, id: &str, from: u32, to: u32) -> Result<Vec<FileChange>, String> {
        let creation = self.creation(id)?;
        let new = self.manifest(id, Self::version_of(&creation, to)?)?;
        let old = if from == 0 {
            Vec::new()
        } else {
            self.manifest(id, Self::version_of(&creation, from)?)?
        };
        let old: BTreeMap<&str, &ManifestEntry> =
            old.iter().map(|e| (e.path.as_str(), e)).collect();
        let new: BTreeMap<&str, &ManifestEntry> =
            new.iter().map(|e| (e.path.as_str(), e)).collect();
        // `None`: too large to compare; `Some(None)`: not text.
        let text_of = |entry: &ManifestEntry| -> Option<Option<String>> {
            if entry.size > MAX_VIEW_BYTES as u64 {
                return None;
            }
            Some(
                fs::read(self.blob_path(id, &entry.hash))
                    .ok()
                    .and_then(|bytes| as_text(&bytes, false)),
            )
        };
        let mut paths: Vec<&str> = old.keys().chain(new.keys()).copied().collect();
        paths.sort_unstable();
        paths.dedup();
        let mut changes = Vec::new();
        for path in paths {
            let (status, before, after) = match (old.get(path), new.get(path)) {
                (None, Some(after)) => ("added", None, Some(*after)),
                (Some(before), None) => ("removed", Some(*before), None),
                (Some(before), Some(after)) if before.hash != after.hash => {
                    ("changed", Some(*before), Some(*after))
                }
                _ => continue,
            };
            // Each side: absent (the file is new or removed), too large, binary or text.
            let old_text = before.map(text_of);
            let new_text = after.map(text_of);
            let binary =
                matches!(old_text, Some(Some(None))) || matches!(new_text, Some(Some(None)));
            let large = matches!(old_text, Some(None)) || matches!(new_text, Some(None));
            let (hunks, too_large) = if binary || large {
                (Vec::new(), large && !binary)
            } else {
                let a = old_text.flatten().flatten().unwrap_or_default();
                let b = new_text.flatten().flatten().unwrap_or_default();
                match diff::hunks(&a, &b) {
                    Some(hunks) => (hunks, false),
                    None => (Vec::new(), true),
                }
            };
            changes.push(FileChange {
                path: path.to_string(),
                status,
                binary,
                too_large,
                hunks,
                note: None,
            });
        }
        Ok(changes)
    }

    /// Write version `version` (the latest when `None`) of `id` as a zip
    /// file in `folder`, never over an existing file. Returns its path.
    pub fn export(&self, id: &str, version: Option<u32>, folder: &Path) -> Result<PathBuf, String> {
        let creation = self.creation(id)?;
        let number = match version {
            Some(number) => Self::version_of(&creation, number)?,
            None => creation.latest_version().ok_or_else(|| {
                "Nothing is finished yet: a version is saved when an answer ends.".to_string()
            })?,
        };
        let manifest = self.manifest(id, number)?;
        fs::create_dir_all(folder)
            .map_err(|e| format!("{} could not be created ({e})", folder.display()))?;
        let stem = format!(
            "{} v{number}",
            crate::agent_tools::files::sanitize_name(&creation.title, "creation")
        );
        let (path, file) = crate::agent_tools::files::create_unique(folder, &stem, "zip")
            .map_err(|e| format!("the export could not be created ({e})"))?;
        let written = (|| -> Result<(), String> {
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for entry in &manifest {
                let bytes = fs::read(self.blob_path(id, &entry.hash))
                    .map_err(|e| format!("{} is missing from the version ({e})", entry.path))?;
                zip.start_file(entry.path.as_str(), options)
                    .map_err(|e| e.to_string())?;
                io::Write::write_all(&mut zip, &bytes).map_err(|e| e.to_string())?;
            }
            zip.finish().map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&path);
            return Err(format!("the export could not be written ({e})"));
        }
        Ok(path)
    }

    /// Remove `id`'s folder (when it is still this creation's folder in
    /// `root`), its versions and its record. A folder already gone is not an
    /// error, and neither is one that another folder has since replaced under
    /// the same name: that folder is not this creation's, so it stays.
    pub fn delete(&self, root: &Path, id: &str) -> Result<Creation, String> {
        let creation = self.creation(id)?;
        let folder = Path::new(&creation.folder);
        if fs::symlink_metadata(folder).is_ok() {
            let checked = checked_folder(root, folder)?;
            let expected = creation.folder_id.as_deref().ok_or_else(|| {
                format!(
                    "Shodh cannot tell whether {} is still this creation's folder, so it leaves \
                     it alone",
                    creation.folder
                )
            })?;
            let now = handle::identity(&checked)
                .map_err(|e| format!("{} could not be checked ({e})", creation.folder))?;
            if now == expected {
                fs::remove_dir_all(&checked)
                    .map_err(|e| format!("{} could not be removed ({e})", checked.display()))?;
            }
        }
        let versions = self.versions_dir(id);
        if versions.exists() {
            fs::remove_dir_all(&versions)
                .map_err(|e| format!("the saved versions could not be removed ({e})"))?;
        }
        self.update(|creations| {
            creations.retain(|c| c.id != id);
            Ok(())
        })?;
        Ok(creation)
    }
}

/// A new, empty folder in `root` named `slug` (or `slug-2`, `slug-3`, ...):
/// each attempt is an exclusive create, so an existing folder is never used.
fn new_folder(root: &Path, slug: &str) -> Result<PathBuf, String> {
    for n in 1..=MAX_SLUG_SUFFIX {
        let name = if n == 1 {
            slug.to_string()
        } else {
            format!("{slug}-{n}")
        };
        let candidate = root.join(&name);
        match fs::create_dir(&candidate) {
            Ok(()) => {
                return fs::canonicalize(&candidate)
                    .map(plain)
                    .map_err(|e| format!("{} could not be opened ({e})", candidate.display()))
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "{} could not be created ({e})",
                    candidate.display()
                ))
            }
        }
    }
    Err(format!("{} has no free name for {slug}", root.display()))
}

#[cfg(test)]
mod tests;
