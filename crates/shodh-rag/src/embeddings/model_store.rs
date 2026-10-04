//! Pinned search models (E5 embeddings + cross-encoder reranker): status,
//! verified download and install.
//!
//! Every artifact is pinned to an exact Hugging Face revision, byte size and
//! SHA-256. Downloads stream into `<file>.part`, resume from an existing
//! `.part` with an HTTP `Range` request, are hashed while they arrive and are
//! moved into place atomically only after the hash matches. A `.part` that
//! fails verification is deleted.
//!
//! Hashing the 555 MB E5 model on every start would cost seconds, so after a
//! successful verification a sidecar `<file>.verified` stamp records the
//! file's size, modification time and the pinned hash. A later status check
//! trusts the file while all three still match; a changed pin, size or mtime
//! forces a full re-hash.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Directory (under the model root) holding the E5 embedding model.
pub const E5_DIR: &str = "multilingual-e5-base";
/// Directory (under the model root) holding the cross-encoder reranker.
pub const RERANKER_DIR: &str = "ms-marco-MiniLM-L6-v2";

/// Default inactivity limit for a single read from the network.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

const PART_SUFFIX: &str = "part";
const STAMP_SUFFIX: &str = "verified";
const HASH_BUFFER: usize = 1 << 20;

/// One pinned file of the search model set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelArtifact {
    /// Human-readable name, used in progress events and errors.
    pub name: String,
    /// Revision-pinned download URL.
    pub url: String,
    /// Lower-case hex SHA-256 of the file contents.
    pub sha256: String,
    /// Exact size in bytes.
    pub size: u64,
    /// Destination relative to the model root (forward slashes).
    pub relative_path: String,
}

impl ModelArtifact {
    fn new(name: &str, url: &str, sha256: &str, size: u64, relative_path: &str) -> Self {
        Self {
            name: name.to_string(),
            url: url.to_string(),
            sha256: sha256.to_string(),
            size,
            relative_path: relative_path.to_string(),
        }
    }
}

/// The pinned E5 + reranker artifacts the search engine needs (≈618 MB).
///
/// Hashes are SHA-256 of the file contents, measured from these exact
/// revision URLs (they equal Hugging Face's LFS object ids, not the Xet
/// storage ids that appear in CDN redirect URLs).
pub fn search_model_artifacts() -> Vec<ModelArtifact> {
    vec![
        ModelArtifact::new(
            "E5 embedding model",
            "https://huggingface.co/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/onnx/model_O4.onnx",
            "f60256a833caee5c75a3903e589116752ee016ca7bc16f9b96e4db09984c5703",
            554_948_118,
            "multilingual-e5-base/model_O4.onnx",
        ),
        ModelArtifact::new(
            "E5 tokenizer",
            "https://huggingface.co/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/onnx/tokenizer.json",
            "62c24cdc13d4c9952d63718d6c9fa4c287974249e16b7ade6d5a85e7bbb75626",
            17_082_660,
            "multilingual-e5-base/tokenizer.json",
        ),
        ModelArtifact::new(
            "Reranker model",
            "https://huggingface.co/cross-encoder/ms-marco-MiniLM-L6-v2/resolve/233902d25c440f23af6f7d6e94d2946bac0bee0a/onnx/model_O4.onnx",
            "b232c2eeedd97a593edc177e3ce4cbd1d6c8f6d8f61a5c201cd0cdeb8134da18",
            45_516_616,
            "ms-marco-MiniLM-L6-v2/model_O4.onnx",
        ),
        ModelArtifact::new(
            "Reranker tokenizer",
            "https://huggingface.co/cross-encoder/ms-marco-MiniLM-L6-v2/resolve/233902d25c440f23af6f7d6e94d2946bac0bee0a/tokenizer.json",
            "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
            711_396,
            "ms-marco-MiniLM-L6-v2/tokenizer.json",
        ),
    ]
}

/// Directory (under the model root) holding the answer-checking (NLI) model.
pub const ANSWER_CHECK_DIR: &str = "nli-deberta-v3-xsmall";

/// The pinned answer-checking model (≈96 MB): `cross-encoder/nli-deberta-v3-xsmall`,
/// its quantised ONNX export, tokenizer and config (the config carries the
/// label order the loader checks). Optional: without it, answers are checked
/// with the reranker and number checks only.
///
/// The ONNX hash is Hugging Face's LFS object id at this revision; the
/// tokenizer and config are plain git files, hashed after download from the
/// same revision URLs.
pub fn answer_check_artifacts() -> Vec<ModelArtifact> {
    const REVISION: &str = "https://huggingface.co/cross-encoder/nli-deberta-v3-xsmall/resolve/a150876415327c80daeff35ca6f68f5ed8cf5c24";
    vec![
        ModelArtifact::new(
            "Answer checking model",
            &format!("{REVISION}/onnx/model_quint8_avx2.onnx"),
            "21b14751a95520953bfcc607ceeb617de7cbeaeb6d60f4c8966716c743985337",
            87_377_068,
            "nli-deberta-v3-xsmall/model_quint8_avx2.onnx",
        ),
        ModelArtifact::new(
            "Answer checking tokenizer",
            &format!("{REVISION}/tokenizer.json"),
            "5124ef2ead1a10a717703bc436de7f353da76d6340e4587719b42b1693707964",
            8_656_624,
            "nli-deberta-v3-xsmall/tokenizer.json",
        ),
        ModelArtifact::new(
            "Answer checking config",
            &format!("{REVISION}/config.json"),
            "8d9f07bf7ba54a6fc3b1962483056f94c39dcf188db4cf61843e1c88f94b2342",
            1_053,
            "nli-deberta-v3-xsmall/config.json",
        ),
    ]
}

/// Errors from checking or installing the search models.
#[derive(Debug, thiserror::Error)]
pub enum ModelStoreError {
    #[error("file system error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("downloading {artifact} failed: {message}")]
    Download { artifact: String, message: String },
    #[error("downloading {artifact} stalled: no data for {seconds} s")]
    Stalled { artifact: String, seconds: u64 },
    #[error("{artifact} is {actual} bytes, expected {expected}")]
    SizeMismatch {
        artifact: String,
        expected: u64,
        actual: u64,
    },
    #[error("{artifact} failed its checksum: expected sha256 {expected}, got {actual}")]
    ChecksumMismatch {
        artifact: String,
        expected: String,
        actual: String,
    },
    #[error("background task failed: {0}")]
    Task(String),
}

impl ModelStoreError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// A byte stream returned by a [`ByteSource`].
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>>;

/// An opened download.
pub struct OpenedStream {
    /// Offset of the first byte in `stream`. Equals the requested offset when
    /// the source honoured the range, `0` when it sent the whole file.
    pub start: u64,
    pub stream: ByteStream,
}

/// Where artifact bytes come from (HTTP in the app, in-memory in tests).
#[async_trait]
pub trait ByteSource: Send + Sync {
    /// Open `url`, asking for bytes from `offset` onwards.
    async fn open(&self, url: &str, offset: u64) -> Result<OpenedStream, String>;
}

/// HTTPS byte source with resume support (`Range` requests).
pub struct HttpByteSource {
    client: reqwest::Client,
}

impl HttpByteSource {
    pub fn new() -> Result<Self, ModelStoreError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(30))
            .user_agent(concat!("shodh/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ModelStoreError::Download {
                artifact: "search models".to_string(),
                message: e.to_string(),
            })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl ByteSource for HttpByteSource {
    async fn open(&self, url: &str, offset: u64) -> Result<OpenedStream, String> {
        let mut request = self.client.get(url);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        let start = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            offset
        } else if status.is_success() {
            0
        } else if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && offset > 0 {
            // The partial file no longer matches the remote; start over.
            return self.open(url, 0).await;
        } else {
            return Err(format!("HTTP {status}"));
        };
        let stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(|e| e.to_string()));
        Ok(OpenedStream {
            start,
            stream: Box::pin(stream),
        })
    }
}

/// State of one artifact on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactState {
    /// Not on disk.
    Missing,
    /// An interrupted download (`.part`) exists; install resumes it.
    Partial,
    /// On disk with the right size but not hashed yet (quick checks only).
    Unverified,
    /// Hash matches the pin.
    Verified,
    /// Wrong size or hash; install replaces it.
    Corrupt,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStatus {
    pub name: String,
    pub relative_path: String,
    pub size: u64,
    pub sha256: String,
    pub state: ArtifactState,
    /// Bytes already on disk (final file or `.part`).
    pub present_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreStatus {
    pub artifacts: Vec<ArtifactStatus>,
    pub total_bytes: u64,
    /// Every artifact is verified.
    pub complete: bool,
}

/// Install phase reported through progress callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallPhase {
    Checking,
    Downloading,
    Verifying,
    Verified,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub artifact: String,
    pub phase: InstallPhase,
    pub artifact_bytes: u64,
    pub artifact_total: u64,
    pub overall_bytes: u64,
    pub overall_total: u64,
}

/// Progress callback for [`ModelStore::install`].
pub type ProgressFn = dyn Fn(&InstallProgress) + Send + Sync;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledArtifact {
    pub name: String,
    pub sha256: String,
    /// `false` when an already-verified file was kept.
    pub downloaded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub artifacts: Vec<InstalledArtifact>,
}

/// Sidecar record of a successful verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct VerifiedStamp {
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    sha256: String,
}

/// Search model files under one root directory.
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
    artifacts: Vec<ModelArtifact>,
    idle_timeout: Duration,
}

impl ModelStore {
    /// The pinned search models under `root`.
    pub fn search_models(root: impl Into<PathBuf>) -> Self {
        Self::with_artifacts(root, search_model_artifacts())
    }

    /// The pinned answer-checking model under `root`.
    pub fn answer_check_model(root: impl Into<PathBuf>) -> Self {
        Self::with_artifacts(root, answer_check_artifacts())
    }

    pub fn with_artifacts(root: impl Into<PathBuf>, artifacts: Vec<ModelArtifact>) -> Self {
        Self {
            root: root.into(),
            artifacts,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    /// Fail a download when no bytes arrive for `timeout`.
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn artifacts(&self) -> &[ModelArtifact] {
        &self.artifacts
    }

    pub fn total_bytes(&self) -> u64 {
        self.artifacts.iter().map(|a| a.size).sum()
    }

    fn path_of(&self, artifact: &ModelArtifact) -> PathBuf {
        artifact
            .relative_path
            .split('/')
            .fold(self.root.clone(), |p, part| p.join(part))
    }

    /// Status of every artifact. With `verify`, files without a valid stamp
    /// are hashed (slow for the E5 model; call from a blocking context) and
    /// stamped when they match. Without it, they are reported `Unverified`.
    pub fn status(&self, verify: bool) -> Result<StoreStatus, ModelStoreError> {
        let mut artifacts = Vec::with_capacity(self.artifacts.len());
        for artifact in &self.artifacts {
            let path = self.path_of(artifact);
            let (state, present_bytes) = match check_file(&path, artifact, verify)? {
                FileCheck::Missing => {
                    let part = with_suffix(&path, PART_SUFFIX);
                    match file_len(&part)? {
                        Some(len) if len > 0 => (ArtifactState::Partial, len),
                        _ => (ArtifactState::Missing, 0),
                    }
                }
                FileCheck::Unverified => (ArtifactState::Unverified, artifact.size),
                FileCheck::Verified => (ArtifactState::Verified, artifact.size),
                FileCheck::Corrupt(len) => (ArtifactState::Corrupt, len),
            };
            artifacts.push(ArtifactStatus {
                name: artifact.name.clone(),
                relative_path: artifact.relative_path.clone(),
                size: artifact.size,
                sha256: artifact.sha256.clone(),
                state,
                present_bytes,
            });
        }
        let complete = artifacts.iter().all(|a| a.state == ArtifactState::Verified);
        Ok(StoreStatus {
            artifacts,
            total_bytes: self.total_bytes(),
            complete,
        })
    }

    /// Download and verify every artifact that is not already verified.
    pub async fn install(
        &self,
        source: &dyn ByteSource,
        progress: &ProgressFn,
    ) -> Result<InstallReport, ModelStoreError> {
        let overall_total = self.total_bytes();
        let mut overall_done: u64 = 0;
        let mut report = Vec::with_capacity(self.artifacts.len());

        for artifact in &self.artifacts {
            let path = self.path_of(artifact);
            let emit = |phase: InstallPhase, artifact_bytes: u64, overall_bytes: u64| {
                progress(&InstallProgress {
                    artifact: artifact.name.clone(),
                    phase,
                    artifact_bytes,
                    artifact_total: artifact.size,
                    overall_bytes,
                    overall_total,
                });
            };
            emit(InstallPhase::Checking, 0, overall_done);

            let check = {
                let path = path.clone();
                let artifact = artifact.clone();
                tokio::task::spawn_blocking(move || check_file(&path, &artifact, true))
                    .await
                    .map_err(|e| ModelStoreError::Task(e.to_string()))??
            };
            match check {
                FileCheck::Verified => {
                    overall_done += artifact.size;
                    emit(InstallPhase::Verified, artifact.size, overall_done);
                    report.push(InstalledArtifact {
                        name: artifact.name.clone(),
                        sha256: artifact.sha256.clone(),
                        downloaded: false,
                    });
                    continue;
                }
                FileCheck::Corrupt(_) | FileCheck::Unverified => {
                    tracing::warn!(
                        path = %path.display(),
                        "search model file failed verification; downloading it again"
                    );
                    remove_if_exists(&path)?;
                    remove_if_exists(&with_suffix(&path, STAMP_SUFFIX))?;
                }
                FileCheck::Missing => {}
            }

            self.download(artifact, &path, source, &|bytes| {
                emit(InstallPhase::Downloading, bytes, overall_done + bytes)
            })
            .await?;
            emit(
                InstallPhase::Verifying,
                artifact.size,
                overall_done + artifact.size,
            );
            write_stamp(&path, artifact)?;
            overall_done += artifact.size;
            emit(InstallPhase::Verified, artifact.size, overall_done);
            report.push(InstalledArtifact {
                name: artifact.name.clone(),
                sha256: artifact.sha256.clone(),
                downloaded: true,
            });
        }
        Ok(InstallReport { artifacts: report })
    }

    /// Stream one artifact into `<path>.part`, verify it and move it to `path`.
    async fn download(
        &self,
        artifact: &ModelArtifact,
        path: &Path,
        source: &dyn ByteSource,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ModelStoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ModelStoreError::io(parent, e))?;
        }
        let part = with_suffix(path, PART_SUFFIX);

        // Resume: hash the bytes already on disk so the final digest covers
        // the whole file.
        let (mut hasher, mut offset) = {
            let part = part.clone();
            let size = artifact.size;
            tokio::task::spawn_blocking(move || resume_state(&part, size))
                .await
                .map_err(|e| ModelStoreError::Task(e.to_string()))??
        };

        if offset < artifact.size {
            let opened = source
                .open(&artifact.url, offset)
                .await
                .map_err(|message| ModelStoreError::Download {
                    artifact: artifact.name.clone(),
                    message,
                })?;
            let mut file = if opened.start == offset && offset > 0 {
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&part)
                    .await
                    .map_err(|e| ModelStoreError::io(&part, e))?
            } else {
                // Fresh download, or the source ignored the range request.
                hasher = Sha256::new();
                offset = 0;
                tokio::fs::File::create(&part)
                    .await
                    .map_err(|e| ModelStoreError::io(&part, e))?
            };
            on_bytes(offset);

            let mut stream = opened.stream;
            let mut last_reported = offset;
            loop {
                let next = tokio::time::timeout(self.idle_timeout, stream.next())
                    .await
                    .map_err(|_| ModelStoreError::Stalled {
                        artifact: artifact.name.clone(),
                        seconds: self.idle_timeout.as_secs(),
                    })?;
                let Some(chunk) = next else { break };
                let chunk = chunk.map_err(|message| ModelStoreError::Download {
                    artifact: artifact.name.clone(),
                    message,
                })?;
                let new_offset = offset + chunk.len() as u64;
                if new_offset > artifact.size {
                    drop(file);
                    remove_if_exists(&part)?;
                    return Err(ModelStoreError::SizeMismatch {
                        artifact: artifact.name.clone(),
                        expected: artifact.size,
                        actual: new_offset,
                    });
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|e| ModelStoreError::io(&part, e))?;
                hasher.update(&chunk);
                offset = new_offset;
                if offset - last_reported >= HASH_BUFFER as u64 || offset == artifact.size {
                    last_reported = offset;
                    on_bytes(offset);
                }
            }
            file.flush()
                .await
                .map_err(|e| ModelStoreError::io(&part, e))?;
            file.sync_all()
                .await
                .map_err(|e| ModelStoreError::io(&part, e))?;
        }

        if offset != artifact.size {
            // The stream ended early. The `.part` is a valid prefix; keep it
            // so the next attempt resumes.
            return Err(ModelStoreError::Download {
                artifact: artifact.name.clone(),
                message: format!(
                    "connection closed after {offset} of {} bytes; retry to resume",
                    artifact.size
                ),
            });
        }

        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(&artifact.sha256) {
            remove_if_exists(&part)?;
            return Err(ModelStoreError::ChecksumMismatch {
                artifact: artifact.name.clone(),
                expected: artifact.sha256.clone(),
                actual,
            });
        }
        tokio::fs::rename(&part, path)
            .await
            .map_err(|e| ModelStoreError::io(path, e))?;
        Ok(())
    }
}

enum FileCheck {
    Missing,
    Unverified,
    Verified,
    /// Wrong size or hash; carries the size on disk.
    Corrupt(u64),
}

fn check_file(
    path: &Path,
    artifact: &ModelArtifact,
    verify: bool,
) -> Result<FileCheck, ModelStoreError> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileCheck::Missing),
        Err(e) => return Err(ModelStoreError::io(path, e)),
    };
    if meta.len() != artifact.size {
        return Ok(FileCheck::Corrupt(meta.len()));
    }
    let current = stamp_for(&meta, artifact).map_err(|e| ModelStoreError::io(path, e))?;
    if read_stamp(path).as_ref() == Some(&current) {
        return Ok(FileCheck::Verified);
    }
    if !verify {
        return Ok(FileCheck::Unverified);
    }
    let actual = hash_file(path)?;
    if actual.eq_ignore_ascii_case(&artifact.sha256) {
        write_stamp(path, artifact)?;
        Ok(FileCheck::Verified)
    } else {
        remove_if_exists(&with_suffix(path, STAMP_SUFFIX))?;
        Ok(FileCheck::Corrupt(meta.len()))
    }
}

/// Hasher and offset for resuming `part`. A `.part` larger than the pinned
/// size cannot be a prefix of the file and is deleted.
fn resume_state(part: &Path, size: u64) -> Result<(Sha256, u64), ModelStoreError> {
    let mut hasher = Sha256::new();
    let len = match file_len(part)? {
        Some(len) => len,
        None => return Ok((hasher, 0)),
    };
    if len > size {
        remove_if_exists(part)?;
        return Ok((hasher, 0));
    }
    let mut file = std::fs::File::open(part).map_err(|e| ModelStoreError::io(part, e))?;
    let mut buf = vec![0u8; HASH_BUFFER];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| ModelStoreError::io(part, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok((hasher, len))
}

fn hash_file(path: &Path) -> Result<String, ModelStoreError> {
    let mut file = std::fs::File::open(path).map_err(|e| ModelStoreError::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUFFER];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| ModelStoreError::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn stamp_for(meta: &std::fs::Metadata, artifact: &ModelArtifact) -> std::io::Result<VerifiedStamp> {
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(VerifiedStamp {
        size: meta.len(),
        mtime_secs: mtime.as_secs(),
        mtime_nanos: mtime.subsec_nanos(),
        sha256: artifact.sha256.to_ascii_lowercase(),
    })
}

fn read_stamp(path: &Path) -> Option<VerifiedStamp> {
    let raw = std::fs::read(with_suffix(path, STAMP_SUFFIX)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Record that `path` matched its pin, atomically.
fn write_stamp(path: &Path, artifact: &ModelArtifact) -> Result<(), ModelStoreError> {
    let meta = std::fs::metadata(path).map_err(|e| ModelStoreError::io(path, e))?;
    let stamp = stamp_for(&meta, artifact).map_err(|e| ModelStoreError::io(path, e))?;
    let stamp_path = with_suffix(path, STAMP_SUFFIX);
    let tmp = with_suffix(&stamp_path, PART_SUFFIX);
    let body = serde_json::to_vec(&stamp).map_err(|e| {
        ModelStoreError::io(
            &stamp_path,
            std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        )
    })?;
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| ModelStoreError::io(&tmp, e))?;
        file.write_all(&body)
            .map_err(|e| ModelStoreError::io(&tmp, e))?;
        file.sync_all().map_err(|e| ModelStoreError::io(&tmp, e))?;
    }
    std::fs::rename(&tmp, &stamp_path).map_err(|e| ModelStoreError::io(&stamp_path, e))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn file_len(path: &Path) -> Result<Option<u64>, ModelStoreError> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.len())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ModelStoreError::io(path, e)),
    }
}

fn remove_if_exists(path: &Path) -> Result<(), ModelStoreError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ModelStoreError::io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// In-memory byte source. `honour_range` = false simulates a server that
    /// ignores `Range`; `cut_at` ends the stream early (dropped connection).
    struct MemorySource {
        files: HashMap<String, Vec<u8>>,
        honour_range: bool,
        cut_at: Option<usize>,
        chunk: usize,
        opens: AtomicUsize,
        offsets: Mutex<Vec<u64>>,
    }

    impl MemorySource {
        fn new(files: &[(&str, &[u8])]) -> Self {
            Self {
                files: files
                    .iter()
                    .map(|(u, b)| (u.to_string(), b.to_vec()))
                    .collect(),
                honour_range: true,
                cut_at: None,
                chunk: 7,
                opens: AtomicUsize::new(0),
                offsets: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ByteSource for MemorySource {
        async fn open(&self, url: &str, offset: u64) -> Result<OpenedStream, String> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.offsets.lock().unwrap().push(offset);
            let body = self.files.get(url).ok_or("404")?.clone();
            let start = if self.honour_range {
                offset as usize
            } else {
                0
            };
            let end = self.cut_at.unwrap_or(body.len()).min(body.len());
            let slice = if start < end {
                body[start..end].to_vec()
            } else {
                Vec::new()
            };
            let chunks: Vec<Result<Bytes, String>> = slice
                .chunks(self.chunk)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect();
            Ok(OpenedStream {
                start: start as u64,
                stream: Box::pin(futures::stream::iter(chunks)),
            })
        }
    }

    fn sha(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn artifact(name: &str, body: &[u8]) -> ModelArtifact {
        ModelArtifact::new(
            name,
            &format!("mem://{name}"),
            &sha(body),
            body.len() as u64,
            &format!("dir/{name}.bin"),
        )
    }

    fn no_progress() -> impl Fn(&InstallProgress) + Send + Sync {
        |_| {}
    }

    const BODY_A: &[u8] = b"the quick brown fox jumps over the lazy dog, repeatedly and verifiably";
    const BODY_B: &[u8] = b"tokenizer contents";

    #[test]
    fn pinned_artifacts_are_well_formed() {
        let artifacts = search_model_artifacts();
        assert_eq!(artifacts.len(), 4);
        let answer_check = answer_check_artifacts();
        assert_eq!(answer_check.len(), 3);
        assert!(answer_check
            .iter()
            .all(|a| a.relative_path.starts_with(&format!("{ANSWER_CHECK_DIR}/"))));
        assert_eq!(
            ModelStore::answer_check_model("x").total_bytes(),
            96_034_745
        );
        for a in artifacts.iter().chain(&answer_check) {
            assert_eq!(a.sha256.len(), 64, "{}", a.name);
            assert!(a
                .sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(a.url.starts_with("https://huggingface.co/"));
            // Revision-pinned: a 40-hex commit id follows /resolve/.
            let rev = a
                .url
                .split("/resolve/")
                .nth(1)
                .unwrap()
                .split('/')
                .next()
                .unwrap();
            assert_eq!(rev.len(), 40, "{}", a.url);
            assert!(a.size > 0);
        }
        let store = ModelStore::search_models("x");
        assert_eq!(store.total_bytes(), 618_258_790);
    }

    #[tokio::test]
    async fn install_downloads_verifies_and_reports_status() {
        let tmp = tempfile::tempdir().unwrap();
        let a = artifact("a", BODY_A);
        let b = artifact("b", BODY_B);
        let store = ModelStore::with_artifacts(tmp.path(), vec![a.clone(), b.clone()]);

        let before = store.status(true).unwrap();
        assert!(!before.complete);
        assert!(before
            .artifacts
            .iter()
            .all(|s| s.state == ArtifactState::Missing));

        let source = MemorySource::new(&[("mem://a", BODY_A), ("mem://b", BODY_B)]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let report = store
            .install(&source, &move |p: &InstallProgress| {
                sink.lock().unwrap().push(p.clone())
            })
            .await
            .unwrap();
        assert!(report.artifacts.iter().all(|a| a.downloaded));
        assert_eq!(
            std::fs::read(tmp.path().join("dir").join("a.bin")).unwrap(),
            BODY_A
        );
        assert!(!tmp.path().join("dir").join("a.bin.part").exists());
        assert!(tmp.path().join("dir").join("a.bin.verified").exists());

        let events = events.lock().unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.phase, InstallPhase::Verified);
        assert_eq!(last.overall_bytes, last.overall_total);
        assert_eq!(last.overall_total, (BODY_A.len() + BODY_B.len()) as u64);

        let after = store.status(false).unwrap();
        assert!(after.complete);

        // A second install keeps verified files and downloads nothing.
        let again = MemorySource::new(&[("mem://a", BODY_A), ("mem://b", BODY_B)]);
        let report = store.install(&again, &no_progress()).await.unwrap();
        assert!(report.artifacts.iter().all(|a| !a.downloaded));
        assert_eq!(again.opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn checksum_mismatch_is_an_error_and_removes_the_part_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut a = artifact("a", BODY_A);
        a.sha256 = sha(b"something else entirely");
        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);
        let source = MemorySource::new(&[("mem://a", BODY_A)]);

        let err = store.install(&source, &no_progress()).await.unwrap_err();
        assert!(
            matches!(err, ModelStoreError::ChecksumMismatch { ref actual, .. } if *actual == sha(BODY_A)),
            "{err}"
        );
        let dir = tmp.path().join("dir");
        assert!(!dir.join("a.bin.part").exists());
        assert!(!dir.join("a.bin").exists());
        assert!(!dir.join("a.bin.verified").exists());
    }

    #[tokio::test]
    async fn oversized_stream_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut a = artifact("a", BODY_A);
        a.size = 10;
        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);
        let source = MemorySource::new(&[("mem://a", BODY_A)]);
        let err = store.install(&source, &no_progress()).await.unwrap_err();
        assert!(matches!(err, ModelStoreError::SizeMismatch { .. }), "{err}");
        assert!(!tmp.path().join("dir").join("a.bin.part").exists());
    }

    #[tokio::test]
    async fn interrupted_download_resumes_with_a_range_request() {
        let tmp = tempfile::tempdir().unwrap();
        let a = artifact("a", BODY_A);
        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);

        let mut first = MemorySource::new(&[("mem://a", BODY_A)]);
        first.cut_at = Some(30);
        let err = store.install(&first, &no_progress()).await.unwrap_err();
        assert!(matches!(err, ModelStoreError::Download { .. }), "{err}");
        let part = tmp.path().join("dir").join("a.bin.part");
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 30);
        let status = store.status(true).unwrap();
        assert_eq!(status.artifacts[0].state, ArtifactState::Partial);
        assert_eq!(status.artifacts[0].present_bytes, 30);

        let second = MemorySource::new(&[("mem://a", BODY_A)]);
        store.install(&second, &no_progress()).await.unwrap();
        assert_eq!(*second.offsets.lock().unwrap(), vec![30]);
        assert_eq!(
            std::fs::read(tmp.path().join("dir").join("a.bin")).unwrap(),
            BODY_A
        );
    }

    #[tokio::test]
    async fn source_ignoring_range_restarts_from_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let a = artifact("a", BODY_A);
        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);
        let dir = tmp.path().join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin.part"), &BODY_A[..20]).unwrap();

        let mut source = MemorySource::new(&[("mem://a", BODY_A)]);
        source.honour_range = false;
        store.install(&source, &no_progress()).await.unwrap();
        assert_eq!(std::fs::read(dir.join("a.bin")).unwrap(), BODY_A);
    }

    #[tokio::test]
    async fn corrupt_part_prefix_fails_checksum_and_is_discarded() {
        let tmp = tempfile::tempdir().unwrap();
        let a = artifact("a", BODY_A);
        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);
        let dir = tmp.path().join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin.part"), b"XXXXXXXXXX").unwrap();

        let source = MemorySource::new(&[("mem://a", BODY_A)]);
        let err = store.install(&source, &no_progress()).await.unwrap_err();
        assert!(
            matches!(err, ModelStoreError::ChecksumMismatch { .. }),
            "{err}"
        );
        assert!(!dir.join("a.bin.part").exists());
        // The retry starts clean and succeeds.
        store.install(&source, &no_progress()).await.unwrap();
        assert_eq!(std::fs::read(dir.join("a.bin")).unwrap(), BODY_A);
    }

    #[tokio::test]
    async fn verified_stamp_is_reused_without_rehashing() {
        let tmp = tempfile::tempdir().unwrap();
        let a = artifact("a", BODY_A);
        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);
        let source = MemorySource::new(&[("mem://a", BODY_A)]);
        store.install(&source, &no_progress()).await.unwrap();

        // Same length, different bytes, original mtime restored: only a
        // re-hash could notice, so a Verified result proves the stamp was used.
        let path = tmp.path().join("dir").join("a.bin");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let tampered = vec![b'z'; BODY_A.len()];
        std::fs::write(&path, &tampered).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let status = store.status(true).unwrap();
        assert_eq!(status.artifacts[0].state, ArtifactState::Verified);

        // A changed mtime invalidates the stamp; the hash then catches it.
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime + Duration::from_secs(5))
            .unwrap();
        let quick = store.status(false).unwrap();
        assert_eq!(quick.artifacts[0].state, ArtifactState::Unverified);
        let full = store.status(true).unwrap();
        assert_eq!(full.artifacts[0].state, ArtifactState::Corrupt);
        assert!(!tmp.path().join("dir").join("a.bin.verified").exists());
    }

    #[test]
    fn stamp_from_a_different_pin_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let a = artifact("a", BODY_A);
        let dir = tmp.path().join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.bin");
        std::fs::write(&path, BODY_A).unwrap();
        let mut old_pin = a.clone();
        old_pin.sha256 = sha(b"old release");
        write_stamp(&path, &old_pin).unwrap();

        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);
        let quick = store.status(false).unwrap();
        assert_eq!(quick.artifacts[0].state, ArtifactState::Unverified);
        let full = store.status(true).unwrap();
        assert_eq!(full.artifacts[0].state, ArtifactState::Verified);
    }

    #[tokio::test]
    async fn wrong_size_file_is_reported_corrupt_and_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let a = artifact("a", BODY_A);
        let dir = tmp.path().join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), b"short").unwrap();
        let store = ModelStore::with_artifacts(tmp.path(), vec![a]);
        assert_eq!(
            store.status(false).unwrap().artifacts[0].state,
            ArtifactState::Corrupt
        );
        let source = MemorySource::new(&[("mem://a", BODY_A)]);
        store.install(&source, &no_progress()).await.unwrap();
        assert_eq!(std::fs::read(dir.join("a.bin")).unwrap(), BODY_A);
    }

    #[tokio::test]
    async fn stalled_stream_times_out() {
        struct Stalling;
        #[async_trait]
        impl ByteSource for Stalling {
            async fn open(&self, _url: &str, _offset: u64) -> Result<OpenedStream, String> {
                Ok(OpenedStream {
                    start: 0,
                    stream: Box::pin(futures::stream::pending()),
                })
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::with_artifacts(tmp.path(), vec![artifact("a", BODY_A)])
            .with_idle_timeout(Duration::from_millis(50));
        let err = store.install(&Stalling, &no_progress()).await.unwrap_err();
        assert!(matches!(err, ModelStoreError::Stalled { .. }), "{err}");
    }
}
