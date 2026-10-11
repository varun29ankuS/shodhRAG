//! What OCR read from a scan, kept next to the index so readers of indexed
//! files (the source viewer, the agent's `open_document`) do not read the
//! scan again: PP-OCRv6 takes seconds a page.
//!
//! One JSON file per scanned file, named by a hash of its normalised path
//! ([`crate::rag_engine::normalize_source_path`]) and written atomically. An
//! entry holds the parse as indexed and is used only while the file has the
//! size and modification time it had then, and while its engine is still the
//! one the parser would use ([`CachedScan::is_current_for`]). The engine
//! removes an entry when its file leaves the index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::processing::document_model::StructuredDocument;
use crate::processing::parser::{
    ParsedDocument, OCR_ENGINE_KEY, OCR_ENGINE_SYSTEM, OCR_READER_MISSING_KEY,
};
use crate::types::DocumentFormat;

/// What identifies a version of a file: its size and modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub size: u64,
    pub modified_ms: u128,
}

impl FileStamp {
    /// The stamp of the file at `path` now.
    pub fn of(path: &Path) -> Result<Self> {
        let meta =
            std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
        let modified = meta
            .modified()
            .context("the file has no modification time")?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        Ok(Self {
            size: meta.len(),
            modified_ms: modified,
        })
    }
}

/// One cached scan: the parse as it was indexed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedScan {
    /// The normalised path.
    pub source: String,
    pub stamp: FileStamp,
    pub content: String,
    pub title: String,
    pub metadata: HashMap<String, String>,
    pub format: DocumentFormat,
    pub document: Option<StructuredDocument>,
}

impl CachedScan {
    /// The cacheable part of `parsed`: a document read by OCR (it has no
    /// structured sections, which are not stored). `None` for other parses.
    pub fn of(source: &str, stamp: FileStamp, parsed: &ParsedDocument) -> Option<Self> {
        if !parsed.metadata.contains_key(OCR_ENGINE_KEY) || !parsed.structured_sections.is_empty() {
            return None;
        }
        Some(Self {
            source: source.to_string(),
            stamp,
            content: parsed.content.clone(),
            title: parsed.title.clone(),
            metadata: parsed.metadata.clone(),
            format: parsed.format,
            document: parsed.document.clone(),
        })
    }

    /// Whether this entry still stands for the file stamped `stamp`, read by
    /// a parser that would use the system OCR only (`system_only`) and has
    /// the scan reader installed or not (`reader_installed`). A scan read with
    /// the system OCR because the reader was missing is read again once it is
    /// installed; one read by the reader is not used where only the system
    /// OCR is chosen.
    pub fn is_current_for(
        &self,
        stamp: FileStamp,
        system_only: bool,
        reader_installed: bool,
    ) -> bool {
        let engine = self.metadata.get(OCR_ENGINE_KEY).map(String::as_str);
        let read_without_reader = self.metadata.contains_key(OCR_READER_MISSING_KEY);
        self.stamp == stamp
            && !(system_only && engine != Some(OCR_ENGINE_SYSTEM))
            && !(read_without_reader && reader_installed)
    }

    pub fn into_parsed(self) -> ParsedDocument {
        ParsedDocument {
            content: self.content,
            title: self.title,
            metadata: self.metadata,
            format: self.format,
            structured_sections: Vec::new(),
            document: self.document,
        }
    }
}

/// The cache directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrCache {
    dir: PathBuf,
}

impl OcrCache {
    /// The cache in `dir` (created on the first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn entry_path(&self, source: &str) -> PathBuf {
        let hash = Sha256::digest(source.as_bytes());
        self.dir.join(format!("{}.json", hex::encode(&hash[..16])))
    }

    /// The entry of `source` (a normalised path), if there is one that reads.
    pub fn get(&self, source: &str) -> Option<CachedScan> {
        let text = std::fs::read(self.entry_path(source)).ok()?;
        match serde_json::from_slice::<CachedScan>(&text) {
            // Two paths with the same hash prefix never share an entry.
            Ok(entry) if entry.source == source => Some(entry),
            Ok(_) => None,
            Err(e) => {
                tracing::debug!(source, error = %e, "unreadable OCR cache entry ignored");
                None
            }
        }
    }

    /// Writes `entry` (replacing any earlier one): to a temporary file, then
    /// renamed over the entry, so a reader never sees half of it.
    pub fn put(&self, entry: &CachedScan) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.entry_path(&entry.source);
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = path.with_extension(format!(
            "{}.{:?}.{nanos}.tmp",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&tmp, serde_json::to_vec(entry)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("replacing {}", path.display()));
        }
        Ok(())
    }

    /// Removes the entry of `source`. Returns whether there was one.
    pub fn remove(&self, source: &str) -> bool {
        std::fs::remove_file(self.entry_path(source)).is_ok()
    }

    /// Removes every entry.
    pub fn clear(&self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, "OCR cache not cleared: {}", self.dir.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(source: &str, stamp: FileStamp, engine: &str, missing: bool) -> CachedScan {
        let mut metadata = HashMap::from([(OCR_ENGINE_KEY.to_string(), engine.to_string())]);
        if missing {
            metadata.insert(OCR_READER_MISSING_KEY.to_string(), "true".to_string());
        }
        CachedScan {
            source: source.to_string(),
            stamp,
            content: "Invoice No: INV-1".to_string(),
            title: "inv".to_string(),
            metadata,
            format: DocumentFormat::PDF,
            document: None,
        }
    }

    const STAMP: FileStamp = FileStamp {
        size: 10,
        modified_ms: 5,
    };

    #[test]
    fn ocr_cache_entries_round_trip_and_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(dir.path().join("ocr_cache"));
        assert!(cache.get("c:/docs/a.pdf").is_none());
        let entry = scan("c:/docs/a.pdf", STAMP, "pp-ocrv6-small", false);
        cache.put(&entry).unwrap();
        assert_eq!(cache.get("c:/docs/a.pdf"), Some(entry.clone()));
        assert!(cache.get("c:/docs/b.pdf").is_none());
        // Rewritten in place; no temporary files are left.
        cache.put(&entry).unwrap();
        let files: Vec<_> = std::fs::read_dir(dir.path().join("ocr_cache"))
            .unwrap()
            .collect();
        assert_eq!(files.len(), 1);
        assert!(cache.remove("c:/docs/a.pdf"));
        assert!(!cache.remove("c:/docs/a.pdf"));
        assert!(cache.get("c:/docs/a.pdf").is_none());
        cache.put(&entry).unwrap();
        cache.clear();
        assert!(cache.get("c:/docs/a.pdf").is_none());
    }

    #[test]
    fn ocr_cache_entries_stand_only_for_the_same_file_and_engine() {
        let reader = scan("a", STAMP, "pp-ocrv6-small", false);
        assert!(reader.is_current_for(STAMP, false, true));
        let changed = FileStamp {
            modified_ms: 6,
            ..STAMP
        };
        assert!(
            !reader.is_current_for(changed, false, true),
            "a stale mtime misses"
        );
        assert!(!reader.is_current_for(FileStamp { size: 11, ..STAMP }, false, true));
        assert!(
            !reader.is_current_for(STAMP, true, true),
            "system OCR chosen"
        );
        // Read without the reader: used until it is installed.
        let fallback = scan("a", STAMP, OCR_ENGINE_SYSTEM, true);
        assert!(fallback.is_current_for(STAMP, false, false));
        assert!(!fallback.is_current_for(STAMP, false, true));
        // Read by the system OCR by design (JBIG2/JPX): kept.
        let by_design = scan("a", STAMP, OCR_ENGINE_SYSTEM, false);
        assert!(by_design.is_current_for(STAMP, false, true));
        assert!(by_design.is_current_for(STAMP, true, true));
    }
}
