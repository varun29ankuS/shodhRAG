//! The research tables of `shodh.db` (schema version 5): snippet images, the last Result
//! extraction per paper and the Results the user rejected.
//!
//! Why images are here and not in the LanceDB statement rows: `shodh.db` is encrypted
//! (SQLCipher) when the app has a key and LanceDB is not; statement rows are appended on
//! every edit (title, note, tags), which would copy the bytes each time; and the
//! `statements` table of existing installs needs no schema change. Images are
//! content-addressed (SHA-256 of the PNG bytes), so the same crop saved twice is stored
//! once, and the statement references it as `shodh-blob:sha256:<hex>`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::{ResearchError, ResearchResult};
use crate::audit::{open_shared_connection, AuditKey};

/// Largest PNG accepted for one snippet, in bytes.
pub const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;
/// Largest side of a snippet image, in pixels (the viewer renders at most 4096).
pub const MAX_IMAGE_SIDE: u32 = 8192;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// URI scheme of a stored image.
const BLOB_PREFIX: &str = "shodh-blob:sha256:";

/// A stored image: its hash, size and the URI statements reference it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub hash: String,
    pub width: u32,
    pub height: u32,
}

impl ImageInfo {
    /// `shodh-blob:sha256:<hex>`.
    pub fn uri(&self) -> String {
        format!("{BLOB_PREFIX}{}", self.hash)
    }
}

/// The hash in a `shodh-blob:sha256:<hex>` URI.
pub fn hash_of_uri(uri: &str) -> Option<&str> {
    uri.strip_prefix(BLOB_PREFIX)
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Width and height of a PNG from its header, after checking the signature and the
/// IHDR chunk. Anything else is rejected.
pub fn png_size(bytes: &[u8]) -> ResearchResult<(u32, u32)> {
    let invalid = |reason: &str| ResearchError::Invalid(format!("The snippet image {reason}."));
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(invalid(&format!(
            "is larger than {} MB",
            MAX_IMAGE_BYTES / (1024 * 1024)
        )));
    }
    if bytes.len() < 33 || &bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return Err(invalid("is not a PNG"));
    }
    let read =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let (width, height) = (read(16), read(20));
    if width == 0 || height == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return Err(invalid(&format!(
            "has an unsupported size ({width}×{height} pixels)"
        )));
    }
    Ok((width, height))
}

/// The research tables of `shodh.db`.
pub struct ResearchDb {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for ResearchDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchDb").finish_non_exhaustive()
    }
}

fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl ResearchDb {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> ResearchResult<Self> {
        let conn = open_shared_connection(path, key)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores a PNG (once per content) and returns its hash and size.
    pub fn put_image(&self, png: &[u8]) -> ResearchResult<ImageInfo> {
        let (width, height) = png_size(png)?;
        let hash = hex::encode(Sha256::digest(png));
        self.lock().execute(
            "INSERT OR IGNORE INTO snippet_images(hash, png, width, height, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hash, png, width, height, now_text()],
        )?;
        Ok(ImageInfo {
            hash,
            width,
            height,
        })
    }

    /// The PNG bytes of a stored image.
    pub fn image(&self, hash: &str) -> ResearchResult<Option<Vec<u8>>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT png FROM snippet_images WHERE hash = ?1",
                params![hash],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Removes a stored image (callers check that nothing references it).
    pub fn delete_image(&self, hash: &str) -> ResearchResult<bool> {
        Ok(self
            .lock()
            .execute("DELETE FROM snippet_images WHERE hash = ?1", params![hash])?
            > 0)
    }

    /// Saves the report of the last extraction of `file_path` (JSON).
    pub fn put_report(&self, file_path: &str, report_json: &str) -> ResearchResult<()> {
        self.lock().execute(
            "INSERT INTO result_extractions(file_path, report_json, extracted_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(file_path) DO UPDATE SET
               report_json = excluded.report_json, extracted_at = excluded.extracted_at",
            params![file_path, report_json, now_text()],
        )?;
        Ok(())
    }

    /// The report of the last extraction of `file_path`, if it was ever extracted.
    pub fn report(&self, file_path: &str) -> ResearchResult<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT report_json FROM result_extractions WHERE file_path = ?1",
                params![file_path],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every stored extraction report as `(file path, JSON)`.
    pub fn reports(&self) -> ResearchResult<Vec<(String, String)>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT file_path, report_json FROM result_extractions ORDER BY file_path")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<(String, String)>, _>>()?;
        Ok(rows)
    }

    /// Remembers that the user rejected the Result with this fingerprint.
    pub fn reject(&self, fingerprint: &str, file_path: &str) -> ResearchResult<()> {
        self.lock().execute(
            "INSERT OR IGNORE INTO result_rejections(fingerprint, file_path, rejected_at)
             VALUES (?1, ?2, ?3)",
            params![fingerprint, file_path, now_text()],
        )?;
        Ok(())
    }

    /// Fingerprints of the Results of `file_path` the user rejected.
    pub fn rejected(&self, file_path: &str) -> ResearchResult<HashSet<String>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT fingerprint FROM result_rejections WHERE file_path = ?1")?;
        let rows = stmt
            .query_map(params![file_path], |r| r.get(0))?
            .collect::<Result<HashSet<String>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A valid 2×1 PNG.
    pub(crate) fn tiny_png() -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&2u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        png.extend_from_slice(&[0, 0, 0, 0]);
        png
    }

    #[test]
    fn images_are_content_addressed_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let db = ResearchDb::open(&dir.path().join("shodh.db"), None).unwrap();
        let png = tiny_png();
        let first = db.put_image(&png).unwrap();
        let again = db.put_image(&png).unwrap();
        assert_eq!(first, again);
        assert_eq!((first.width, first.height), (2, 1));
        assert_eq!(hash_of_uri(&first.uri()), Some(first.hash.as_str()));
        assert_eq!(db.image(&first.hash).unwrap(), Some(png.clone()));
        assert!(db.delete_image(&first.hash).unwrap());
        assert_eq!(db.image(&first.hash).unwrap(), None);

        assert!(matches!(
            db.put_image(b"GIF89a....."),
            Err(ResearchError::Invalid(_))
        ));
        let mut huge = png.clone();
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(matches!(
            db.put_image(&huge),
            Err(ResearchError::Invalid(_))
        ));
        assert_eq!(hash_of_uri("shodh-blob:sha256:xyz"), None);
    }

    #[test]
    fn reports_and_rejections_persist() {
        let dir = tempfile::tempdir().unwrap();
        let db = ResearchDb::open(&dir.path().join("shodh.db"), None).unwrap();
        assert_eq!(db.report("a.pdf").unwrap(), None);
        db.put_report("a.pdf", "{\"added\":1}").unwrap();
        db.put_report("a.pdf", "{\"added\":2}").unwrap();
        assert_eq!(
            db.report("a.pdf").unwrap().as_deref(),
            Some("{\"added\":2}")
        );
        assert_eq!(db.reports().unwrap().len(), 1);
        db.reject("fp-1", "a.pdf").unwrap();
        db.reject("fp-1", "a.pdf").unwrap();
        assert_eq!(db.rejected("a.pdf").unwrap().len(), 1);
        assert!(db.rejected("b.pdf").unwrap().is_empty());
    }
}
