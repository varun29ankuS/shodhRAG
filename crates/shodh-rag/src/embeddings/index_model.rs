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
//! - an empty one gets the export the first load chooses (a graphics card
//!   that is faster than the CPU's int8 runs fp16, else int8).
//!
//! It is replaced, atomically, only when everything has been embedded again
//! in another export.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::e5::{E5Variant, E5_DIR};
use crate::compute::provider::{ComputePreference, GpuProbe};

/// The record's file in the data folder.
pub const INDEX_MODEL_FILE: &str = "search-index-model.json";

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

/// Records `variant` as the export of the index in `data_dir`: written beside
/// it, flushed, then renamed over it, so a crash leaves the old record or the
/// new one.
pub fn write(data_dir: &Path, variant: E5Variant, reason: &str) -> Result<IndexModel> {
    let record = IndexModel {
        model: E5_DIR.to_string(),
        variant,
        recorded_at: Utc::now(),
        reason: reason.to_string(),
    };
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let path = data_dir.join(INDEX_MODEL_FILE);
    let partial = data_dir.join(format!("{INDEX_MODEL_FILE}.part"));
    let json = serde_json::to_vec_pretty(&record)?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&partial)
            .with_context(|| format!("writing {}", partial.display()))?;
        file.write_all(&json)?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, &path).with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(
        variant = variant.label(),
        reason,
        "Recorded the search model this index is built with"
    );
    Ok(record)
}

/// The E5 exports search setup installs on this machine:
/// - the index's own export, when it is recorded (`index`): nothing else can
///   search it;
/// - otherwise (a new library) int8 for the processor, plus fp16 when a
///   graphics card may run search (`preference` allows it and `probe` finds a
///   card a provider can use), so the first load can compare the card's fp16
///   with the processor's int8 and keep the faster.
pub fn setup_variants(
    index: Option<E5Variant>,
    preference: ComputePreference,
    probe: impl FnOnce() -> GpuProbe,
) -> Vec<E5Variant> {
    if let Some(variant) = index {
        return vec![variant];
    }
    let card = preference == ComputePreference::Auto
        && matches!(probe(), GpuProbe::Devices(devices) if !devices.is_empty());
    if card {
        vec![E5Variant::Fp16, E5Variant::Int8]
    } else {
        vec![E5Variant::Int8]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::provider::{Accelerator, Device};

    #[test]
    fn setup_installs_the_index_s_export_or_what_this_machine_runs() {
        let card = || {
            GpuProbe::Devices(vec![Device {
                accelerator: Accelerator::DirectMl,
                name: "Radeon".to_string(),
                index: Some(0),
                driver: None,
            }])
        };
        let auto = ComputePreference::Auto;
        assert_eq!(
            setup_variants(Some(E5Variant::Fp16), auto, card),
            vec![E5Variant::Fp16],
            "an existing fp16 library needs no int8 download"
        );
        assert_eq!(
            setup_variants(Some(E5Variant::Int8), auto, card),
            vec![E5Variant::Int8]
        );
        assert_eq!(
            setup_variants(None, auto, card),
            vec![E5Variant::Fp16, E5Variant::Int8]
        );
        assert_eq!(
            setup_variants(None, ComputePreference::Cpu, || unreachable!("no probe")),
            vec![E5Variant::Int8]
        );
        for probe in [
            GpuProbe::Unsupported,
            GpuProbe::Devices(Vec::new()),
            GpuProbe::Failed("DXGI".to_string()),
        ] {
            assert_eq!(
                setup_variants(None, auto, || probe.clone()),
                vec![E5Variant::Int8]
            );
        }
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
