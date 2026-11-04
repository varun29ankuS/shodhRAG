//! Which E5 export built an index, so its vectors are never mixed with
//! another export's.
//!
//! The fp16 and int8 exports of multilingual-e5-base give close but not equal
//! vectors, and both are 768-dimensional, so no dimension check would catch a
//! mix. The rule: **an index keeps the export it was built with.** Every
//! vector Shodh stores under one data folder (the library's chunks and the
//! memory statements, both in `lance_data`) and every query against them is
//! embedded with that export, on whatever can run it: fp16 on a graphics card
//! or, when the card is slower or fails, on the CPU; int8 on the CPU only.
//! Changing the export means embedding everything again.
//!
//! The record is written once, before the first vector is stored:
//! - a data folder that already holds vectors and has no record was built
//!   before records existed, with fp16 (the only export shipped then);
//! - an empty one gets the export chosen for new libraries: fp16, the default,
//!   or int8 when the user opted in to faster search on a processor that runs it exactly.
//!
//! It is replaced, atomically, only when everything has been embedded again
//! in another export ([`super::model_switch`]): the record also names the
//! LanceDB tables that hold the vectors, so a switch builds new tables beside
//! the old ones and the record's rename is the moment search moves to them.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::e5::{E5Variant, E5_DIR};

/// The record's file in the data folder.
pub const INDEX_MODEL_FILE: &str = "search-index-model.json";
/// The library's chunk table before any switch.
pub const DOCUMENTS_TABLE: &str = "documents";

fn documents_table() -> String {
    DOCUMENTS_TABLE.to_string()
}

fn statements_table() -> String {
    crate::statements::STATEMENTS_TABLE.to_string()
}

/// The export an index was built with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexModel {
    /// The model's folder name ("multilingual-e5-base").
    pub model: String,
    pub variant: E5Variant,
    pub recorded_at: DateTime<Utc>,
    /// Why it was recorded ("first vectors", "built before records").
    pub reason: String,
    /// The LanceDB table (in `lance_data`) of the library's chunks.
    #[serde(default = "documents_table")]
    pub documents_table: String,
    /// The LanceDB table (in `lance_data`) of the memory statements.
    #[serde(default = "statements_table")]
    pub statements_table: String,
}

impl IndexModel {
    /// A record of `variant` in the original tables.
    pub fn new(variant: E5Variant, reason: &str) -> Self {
        Self {
            model: E5_DIR.to_string(),
            variant,
            recorded_at: Utc::now(),
            reason: reason.to_string(),
            documents_table: documents_table(),
            statements_table: statements_table(),
        }
    }
}

/// The chunk and statement tables of the index in `data_dir`: the record's,
/// or the original ones when there is no record yet.
pub fn tables(data_dir: &Path) -> Result<(String, String)> {
    Ok(match read(data_dir)? {
        Some(record) => (record.documents_table, record.statements_table),
        None => (documents_table(), statements_table()),
    })
}

/// The record in `data_dir`; `None` when there is none. A record that exists
/// but cannot be read is an error, never a guess: guessing could mix exports.
pub fn read(data_dir: &Path) -> Result<Option<IndexModel>> {
    let path = data_dir.join(INDEX_MODEL_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", path.display()));
        }
    };
    let record: IndexModel =
        serde_json::from_slice(&bytes).with_context(|| format!("{} is damaged", path.display()))?;
    if record.model != E5_DIR {
        anyhow::bail!(
            "{} names the model {:?}; this build runs only {E5_DIR}",
            path.display(),
            record.model
        );
    }
    Ok(Some(record))
}

/// Records `variant` (in the original tables) as the export of the index in
/// `data_dir`.
pub fn write(data_dir: &Path, variant: E5Variant, reason: &str) -> Result<IndexModel> {
    let record = IndexModel::new(variant, reason);
    write_record(data_dir, &record)?;
    Ok(record)
}

/// Writes `record` in `data_dir`: beside the old one, flushed, then renamed
/// over it, so a crash leaves the old record or the new one.
pub fn write_record(data_dir: &Path, record: &IndexModel) -> Result<()> {
    write_json_atomically(data_dir, INDEX_MODEL_FILE, record)?;
    tracing::info!(
        variant = record.variant.label(),
        reason = %record.reason,
        documents = %record.documents_table,
        statements = %record.statements_table,
        "Recorded the search model this index is built with"
    );
    Ok(())
}

/// Writes `value` as `name` in `dir` through a flushed `.part` file and a
/// rename.
pub(crate) fn write_json_atomically<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(name);
    let partial = dir.join(format!("{name}.part"));
    let json = serde_json::to_vec_pretty(value)?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&partial)
            .with_context(|| format!("writing {}", partial.display()))?;
        file.write_all(&json)?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, &path).with_context(|| format!("writing {}", path.display()))
}

/// The E5 export search setup installs:
/// - the index's own export, when it is recorded (`index`): nothing else can
///   search it;
/// - otherwise (a new library) the export chosen for new libraries
///   (`new_library`): fp16 by default; int8 only when the user opted in and
///   this processor runs it exactly (`int8_exact`, [`super::e5::int8_exact`]).
pub fn setup_variants(
    index: Option<E5Variant>,
    new_library: E5Variant,
    int8_exact: bool,
) -> Vec<E5Variant> {
    match index {
        Some(variant) => vec![variant],
        None if new_library == E5Variant::Int8 && int8_exact => vec![E5Variant::Int8],
        None => vec![E5Variant::Fp16],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_installs_the_index_s_export_or_fp16_unless_int8_was_chosen() {
        for exact in [true, false] {
            assert_eq!(
                setup_variants(Some(E5Variant::Fp16), E5Variant::Int8, exact),
                vec![E5Variant::Fp16],
                "an existing fp16 library needs no int8 download"
            );
            assert_eq!(
                setup_variants(Some(E5Variant::Int8), E5Variant::Fp16, exact),
                vec![E5Variant::Int8]
            );
            assert_eq!(
                setup_variants(None, E5Variant::Fp16, exact),
                vec![E5Variant::Fp16],
                "first-run setup downloads fp16 by default"
            );
        }
        assert_eq!(
            setup_variants(None, E5Variant::Int8, true),
            vec![E5Variant::Int8],
            "int8 only when chosen"
        );
        assert_eq!(
            setup_variants(None, E5Variant::Int8, false),
            vec![E5Variant::Fp16],
            "and only where it runs exactly"
        );
    }

    #[test]
    fn a_record_round_trips_and_a_missing_one_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()).unwrap(), None);
        let written = write(dir.path(), E5Variant::Int8, "first vectors").unwrap();
        assert_eq!(read(dir.path()).unwrap(), Some(written.clone()));
        assert!(!dir.path().join(format!("{INDEX_MODEL_FILE}.part")).exists());
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(INDEX_MODEL_FILE)).unwrap())
                .unwrap();
        assert_eq!(json["variant"], "int8");
        assert_eq!(json["model"], "multilingual-e5-base");
        let replaced = write(dir.path(), E5Variant::Fp16, "built before records").unwrap();
        assert_eq!(read(dir.path()).unwrap(), Some(replaced));
    }

    /// A damaged record or one of another model stops the load: Shodh never
    /// guesses which export built an index.
    #[test]
    fn a_damaged_or_foreign_record_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(INDEX_MODEL_FILE), b"{\"model\":").unwrap();
        assert!(read(dir.path()).is_err());
        std::fs::write(
            dir.path().join(INDEX_MODEL_FILE),
            br#"{"model":"bge-m3","variant":"fp16","recordedAt":"2026-11-01T00:00:00Z","reason":"x"}"#,
        )
        .unwrap();
        assert!(read(dir.path()).is_err());
        std::fs::write(
            dir.path().join(INDEX_MODEL_FILE),
            br#"{"model":"multilingual-e5-base","variant":"fp32","recordedAt":"2026-11-01T00:00:00Z","reason":"x"}"#,
        )
        .unwrap();
        assert!(read(dir.path()).is_err(), "an unknown export");
    }
}
