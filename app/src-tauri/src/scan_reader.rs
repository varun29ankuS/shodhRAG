//! The scan reader (the PP-OCRv6 OCR model, 31 MB) for libraries whose search
//! models were installed before it was installed with them.
//!
//! The indexer reports each scanned file it read without the scan reader
//! ([`ScanReaderRequest`]): on Windows the file was read with Windows OCR
//! instead, on macOS it could not be read. The first report puts one Install
//! item in the Inbox; the offer is made once (a dismissed offer does not come
//! back: Settings → Search → Scan reader installs it, and on macOS so does the
//! Library's failure advice). The reported files are kept in `scan-reader.json`
//! in the app data folder until the reader is installed through the search
//! model installer (by any of those routes), and are then read again
//! ([`reread_after_install`]).
//!
//! Scans indexed before the OCR engine was recorded with their chunks were
//! read with Windows OCR and never reported. Once the reader is installed
//! they are found on disk ([`legacy_ocr_scans`]) and read again with the
//! reported ones, once: at the install, or at the first start with the
//! reader installed ([`reread_on_start`]), for a reader installed before
//! this check existed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::comprehensive_system::ComprehensiveRAG;
use shodh_rag::inbox::{InboxKind, InboxStatus, NewInboxItem};
use shodh_rag::rag_engine::{normalize_source_path, ScanReaderRequest};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::RwLock as AsyncRwLock;

use crate::file_watcher::FolderSyncState;

/// Inbox id of the offer (one item, updated as the install goes on).
pub const INBOX_ID: &str = "install:scan-reader";
/// `data.action` of the offer, which the Inbox shows as Install.
const INSTALL_ACTION: &str = "install_scan_reader";
/// The pending files, in the app data folder.
const PENDING_FILE: &str = "scan-reader.json";

/// What is kept between runs.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Pending {
    /// The Inbox offer was made.
    offered: bool,
    /// Files to read again once the reader is installed: path -> the metadata
    /// they were indexed with (their Library source among it).
    files: BTreeMap<String, HashMap<String, String>>,
    /// Scans indexed before the OCR engine was recorded were looked for with
    /// the reader installed (and handed over to be read again).
    #[serde(default)]
    legacy_checked: bool,
}

/// Managed by Tauri.
pub struct ScanReaderState {
    path: PathBuf,
    /// Held while `scan-reader.json` is read and written.
    lock: tokio::sync::Mutex<()>,
}

impl ScanReaderState {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            path: app_data_dir.join(PENDING_FILE),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    fn load(&self) -> Pending {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "{} is not readable; starting over", self.path.display());
                Pending::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Pending::default(),
            Err(e) => {
                tracing::warn!(error = %e, "{} could not be read", self.path.display());
                Pending::default()
            }
        }
    }

    fn save(&self, pending: &Pending) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(pending).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Records a file read without the reader. Returns whether this is the
    /// first report, for which the offer is made.
    async fn record(&self, request: ScanReaderRequest) -> bool {
        let _guard = self.lock.lock().await;
        let mut pending = self.load();
        pending.files.insert(
            request.path.display().to_string(),
            request.metadata.into_iter().collect(),
        );
        let offer = !pending.offered;
        pending.offered = true;
        if let Err(e) = self.save(&pending) {
            tracing::warn!(error = %e, "scan reader list not saved");
        }
        offer
    }

    /// The files to read again, removed from the list (the offer stays made).
    async fn take_files(&self) -> BTreeMap<String, HashMap<String, String>> {
        let _guard = self.lock.lock().await;
        let mut pending = self.load();
        let files = std::mem::take(&mut pending.files);
        if !files.is_empty() {
            if let Err(e) = self.save(&pending) {
                tracing::warn!(error = %e, "scan reader list not saved");
            }
        }
        files
    }

    /// Whether scans indexed before the OCR engine was recorded were looked for.
    async fn legacy_checked(&self) -> bool {
        let _guard = self.lock.lock().await;
        self.load().legacy_checked
    }

    /// Puts the scans found indexed before the OCR engine was recorded on the
    /// list of files to read again and records that they were looked for, in
    /// one save: an app closed before they are read reads them next time.
    async fn add_legacy(&self, scans: Vec<shodh_rag::folder_sync::LegacyScan>) {
        let _guard = self.lock.lock().await;
        let mut pending = self.load();
        merge_legacy(&mut pending.files, scans);
        pending.legacy_checked = true;
        if let Err(e) = self.save(&pending) {
            tracing::warn!(error = %e, "scan reader list not saved");
        }
    }

    /// Whether files wait to be read again.
    async fn has_files(&self) -> bool {
        let _guard = self.lock.lock().await;
        !self.load().files.is_empty()
    }
}

/// Adds the legacy scans to the files to read again (a file already listed
/// keeps the metadata it was reported with).
fn merge_legacy(
    files: &mut BTreeMap<String, HashMap<String, String>>,
    legacy: Vec<shodh_rag::folder_sync::LegacyScan>,
) {
    let listed: BTreeSet<String> = files
        .keys()
        .map(|path| normalize_source_path(Path::new(path)))
        .collect();
    for scan in legacy {
        if listed.contains(&scan.source) {
            continue;
        }
        let mut metadata = scan.metadata;
        if !scan.space_id.is_empty() {
            metadata
                .entry("space_id".to_string())
                .or_insert(scan.space_id);
        }
        files.entry(scan.source).or_insert(metadata);
    }
}

/// The Install offer, worded for what happened to the scans on this platform.
fn offer_item() -> NewInboxItem {
    let detail = if cfg!(windows) {
        "Scanned pages were read with Windows OCR, which misses text in tables and \
         noisy scans. The scan reader (31 MB) reads them better; they are read \
         again once it is installed."
    } else {
        "Scanned documents cannot be read until the scan reader (31 MB) is \
         installed. They are read automatically once it is."
    };
    NewInboxItem {
        id: INBOX_ID.to_string(),
        kind: InboxKind::Install,
        status: InboxStatus::NeedsYou,
        title: "Install the scan reader for scanned documents".to_string(),
        detail: Some(detail.to_string()),
        link: None,
        data: json!({ "action": INSTALL_ACTION }),
    }
}

/// The item while scans are read again, and when they were.
fn progress_item(status: InboxStatus, files: usize) -> NewInboxItem {
    let documents = if files == 1 { "document" } else { "documents" };
    let (title, detail) = match status {
        InboxStatus::Working => (
            "Reading scanned documents with the scan reader".to_string(),
            format!("{files} {documents}"),
        ),
        _ => (
            "The scan reader is installed".to_string(),
            if files == 0 {
                "Scanned documents are read with it from now on.".to_string()
            } else {
                format!("{files} scanned {documents} read again")
            },
        ),
    };
    NewInboxItem {
        id: INBOX_ID.to_string(),
        kind: InboxKind::Install,
        status,
        title,
        detail: Some(detail),
        link: None,
        data: json!({}),
    }
}

/// Connects `engine` to the worker [`start_worker`] runs. Call once at
/// startup, before the engine is shared.
pub fn connect(engine: &mut ComprehensiveRAG) -> UnboundedReceiver<ScanReaderRequest> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    engine.set_scan_reader_queue(tx);
    rx
}

/// Records the files the indexer read without the reader, and makes the offer
/// on the first.
pub fn start_worker(app: AppHandle, mut rx: UnboundedReceiver<ScanReaderRequest>) {
    tauri::async_runtime::spawn(async move {
        while let Some(request) = rx.recv().await {
            tracing::info!(
                file = %request.path.display(),
                "scanned file read without the scan reader"
            );
            if app.state::<ScanReaderState>().record(request).await {
                crate::inbox_commands::post(&app, offer_item()).await;
            }
        }
    });
}

/// After the reader was installed: reads the recorded files again. A file of
/// a Library folder is handed to the folder's sync (its record is dropped, so
/// the sync reads it as new); any other file is read again here while it is
/// still indexed. Files that are gone, or whose source was removed, are
/// skipped.
pub async fn reread_after_install(app: AppHandle, rag: Arc<AsyncRwLock<ComprehensiveRAG>>) {
    reread(app, rag, true).await;
}

/// At the first start with the reader installed and the Library's folders
/// watched: reads again what an install did not (scans indexed before the OCR
/// engine was recorded, for a reader installed before they were looked for;
/// files recorded while an install was cut short). Says nothing when there
/// is nothing to read.
pub async fn reread_on_start(app: AppHandle, rag: Arc<AsyncRwLock<ComprehensiveRAG>>) {
    if !reader_reads(&rag).await {
        return;
    }
    let state = app.state::<ScanReaderState>();
    if state.legacy_checked().await && !state.has_files().await {
        return;
    }
    reread(app, rag, false).await;
}

/// Whether scans are read with the reader: it is installed and the OCR
/// setting does not choose the system OCR.
async fn reader_reads(rag: &AsyncRwLock<ComprehensiveRAG>) -> bool {
    let engine = rag.read().await;
    engine.ocr_handle().available()
        && engine.config().ocr.engine != shodh_rag::config::OcrEngineChoice::System
}

/// Finds, once with the reader installed, the scans indexed before the OCR
/// engine was recorded and puts them on the list of files to read again. A
/// failure is tried again next time.
async fn queue_legacy_scans(app: &AppHandle, rag: &AsyncRwLock<ComprehensiveRAG>) {
    let state = app.state::<ScanReaderState>();
    if state.legacy_checked().await || !reader_reads(rag).await {
        return;
    }
    match crate::file_watcher::legacy_ocr_scans_in_turn(app).await {
        Ok(scans) => {
            if !scans.is_empty() {
                tracing::info!(
                    scans = scans.len(),
                    "scans read with Windows OCR before the engine was recorded"
                );
            }
            state.add_legacy(scans).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "scans indexed before the OCR engine was recorded not looked for");
        }
    }
}

async fn reread(app: AppHandle, rag: Arc<AsyncRwLock<ComprehensiveRAG>>, announce: bool) {
    queue_legacy_scans(&app, &rag).await;
    let files = app.state::<ScanReaderState>().take_files().await;
    let offered = app.state::<ScanReaderState>().load().offered;
    if files.is_empty() {
        if offered && announce {
            crate::inbox_commands::post(&app, progress_item(InboxStatus::Done, 0)).await;
        }
        return;
    }
    crate::inbox_commands::post(&app, progress_item(InboxStatus::Working, files.len())).await;

    let indexed: BTreeSet<String> = match rag.read().await.document_sources().await {
        Ok(rows) => rows.into_iter().map(|row| row.source).collect(),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "indexed files not listed");
            BTreeSet::new()
        }
    };
    let folders = app.state::<FolderSyncState>();
    let mut to_sync = BTreeSet::new();
    let mut read_again = 0usize;
    for (path, metadata) in files {
        let file = PathBuf::from(&path);
        if !file.is_file() {
            continue;
        }
        let key = normalize_source_path(&file);
        if let Some(source_id) = metadata.get("space_id") {
            if folders.forget_file(source_id, &key).await {
                to_sync.insert(source_id.clone());
                read_again += 1;
                continue;
            }
        }
        if !indexed.contains(&key) {
            continue;
        }
        match shodh_rag::indexing::index_file(&rag, &file, metadata).await {
            Ok(_) => read_again += 1,
            Err(e) => tracing::warn!(file = %path, error = %e, "scan not read again"),
        }
    }
    for source_id in to_sync {
        crate::file_watcher::sync_source_now(&app, &source_id).await;
    }
    crate::inbox_commands::post(&app, progress_item(InboxStatus::Done, read_again)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str, space: &str) -> ScanReaderRequest {
        ScanReaderRequest {
            path: PathBuf::from(path),
            metadata: HashMap::from([("space_id".to_string(), space.to_string())]),
        }
    }

    #[tokio::test]
    async fn the_offer_is_made_once_and_files_wait_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let state = ScanReaderState::new(dir.path());
        assert!(state.record(request("C:/docs/a-scan.pdf", "s1")).await);
        assert!(!state.record(request("C:/docs/b-scan.pdf", "s1")).await);
        assert!(
            !state.record(request("C:/docs/a-scan.pdf", "s1")).await,
            "a file reported again is kept once"
        );

        // A new run reads the same list.
        let again = ScanReaderState::new(dir.path());
        let files = again.take_files().await;
        assert_eq!(
            files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["C:/docs/a-scan.pdf", "C:/docs/b-scan.pdf"]
        );
        assert_eq!(files["C:/docs/a-scan.pdf"]["space_id"], "s1");
        assert!(again.take_files().await.is_empty(), "taken files are gone");
        // After the install, a scan reported again does not offer again.
        assert!(!again.record(request("C:/docs/c-scan.pdf", "s2")).await);
    }

    #[tokio::test]
    async fn the_legacy_scan_check_is_kept_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        // A list written before the check existed reads as not checked.
        std::fs::write(
            dir.path().join(PENDING_FILE),
            r#"{"offered":true,"files":{}}"#,
        )
        .unwrap();
        let state = ScanReaderState::new(dir.path());
        assert!(!state.legacy_checked().await);
        assert!(!state.has_files().await);
        state
            .add_legacy(vec![shodh_rag::folder_sync::LegacyScan {
                source: "c:/docs/old-scan.pdf".into(),
                space_id: "s1".into(),
                metadata: HashMap::new(),
            }])
            .await;
        // A new run (the app closed before they were read) still has them.
        let again = ScanReaderState::new(dir.path());
        assert!(again.legacy_checked().await);
        assert!(again.has_files().await);
        assert!(again.load().offered, "the offer stays made");
        assert!(!again.record(request("C:/docs/a-scan.pdf", "s1")).await);
        let files = again.take_files().await;
        assert_eq!(files.len(), 2);
        assert_eq!(files["c:/docs/old-scan.pdf"]["space_id"], "s1");
        assert!(!again.has_files().await);
        assert!(again.legacy_checked().await, "recording keeps the check");
    }

    #[test]
    fn legacy_scans_join_the_reported_files_once() {
        use shodh_rag::folder_sync::LegacyScan;
        let reported = PathBuf::from("C:/Docs/Lease-Scan.pdf");
        let mut files = BTreeMap::from([(
            reported.display().to_string(),
            HashMap::from([("space_id".to_string(), "s1".to_string())]),
        )]);
        merge_legacy(
            &mut files,
            vec![
                LegacyScan {
                    source: normalize_source_path(&reported),
                    space_id: "s1".into(),
                    metadata: HashMap::new(),
                },
                LegacyScan {
                    source: "c:/docs/old/invoice-scan.pdf".into(),
                    space_id: "s2".into(),
                    metadata: HashMap::from([("title".to_string(), "invoice-scan".to_string())]),
                },
                LegacyScan {
                    source: "c:/elsewhere/receipt.png".into(),
                    space_id: String::new(),
                    metadata: HashMap::new(),
                },
            ],
        );
        assert_eq!(files.len(), 3, "{files:?}");
        let invoice = &files["c:/docs/old/invoice-scan.pdf"];
        assert_eq!(invoice["space_id"], "s2", "its folder syncs it");
        assert_eq!(invoice["title"], "invoice-scan");
        assert!(!files["c:/elsewhere/receipt.png"].contains_key("space_id"));
    }

    #[tokio::test]
    async fn an_unreadable_list_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PENDING_FILE), "{not json").unwrap();
        let state = ScanReaderState::new(dir.path());
        assert!(state.record(request("C:/docs/a-scan.pdf", "s1")).await);
    }

    #[test]
    fn the_offer_installs_and_says_what_happens_to_the_scans() {
        let item = offer_item();
        assert_eq!(item.id, INBOX_ID);
        assert_eq!(item.kind, InboxKind::Install);
        assert_eq!(item.status, InboxStatus::NeedsYou);
        assert_eq!(item.data["action"], INSTALL_ACTION);
        let detail = item.detail.unwrap();
        assert!(detail.contains("31 MB"), "{detail}");
        if cfg!(windows) {
            assert!(detail.contains("Windows OCR"), "{detail}");
        } else {
            assert!(detail.contains("cannot be read"), "{detail}");
        }

        let working = progress_item(InboxStatus::Working, 3);
        assert_eq!(working.id, INBOX_ID);
        assert_eq!(working.detail.as_deref(), Some("3 documents"));
        assert_eq!(working.data, json!({}), "no Install once it runs");
        assert_eq!(
            progress_item(InboxStatus::Done, 1).detail.as_deref(),
            Some("1 scanned document read again")
        );
    }
}
