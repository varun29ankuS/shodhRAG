//! Audit log state and the Usage & Audit commands (spec §8 "Audit").
//!
//! The log lives in `<app_data_dir>/shodh.db`. When the build links SQLCipher
//! (`audit-encryption` feature), the database key is a random 256-bit key
//! kept in the OS credential store (service `com.shodh.rag-app`, user
//! `audit-db-key`). Without that feature the database is plain SQLite and
//! `audit_stats` reports `encrypted: false`.
//!
//! Recording never blocks the caller: events are queued to the log's writer
//! thread. Reads, verification and export run on the blocking pool. The one
//! exception is `audit_record_citation_correction`, which waits on the
//! blocking pool until its event is written: the app changes the answer only
//! once the correction is recorded.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Datelike, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shodh_rag::audit::{
    encryption_compiled, AuditEventType, AuditKey, AuditLog, AuditQuery, AuditRecord, AuditRow,
    AuditScope, AuditStats, VerifyReport, LOCAL_OWNER,
};
use shodh_rag::harness::events::GroundingSummary;
use shodh_rag::harness::tools::ToolAudit;
use tauri::State;

use crate::api_key_store;

/// File name of the app database (audit tables today).
pub const DB_FILE: &str = "shodh.db";

/// Managed state: the open log, or why it could not be opened.
pub struct AuditState {
    log: Result<Arc<AuditLog>, String>,
    /// The database path and key the log was opened with, for other tables in
    /// `shodh.db` (statement dynamics). `None` when the log could not be opened.
    database: Option<(PathBuf, Option<AuditKey>)>,
}

/// Fetch the database key from the OS credential store, creating it on
/// first run.
fn database_key() -> Result<AuditKey, String> {
    match api_key_store::load_secret(api_key_store::AUDIT_DB_KEY_USER)? {
        Some(hex) => AuditKey::from_hex(&hex).map_err(|e| e.to_string()),
        None => {
            let key = AuditKey::generate();
            api_key_store::store_secret(api_key_store::AUDIT_DB_KEY_USER, &key.to_hex())?;
            Ok(key)
        }
    }
}

impl AuditState {
    /// Open the log at `<app_data_dir>/shodh.db`. Never fails: an unusable
    /// log is reported by the audit commands and in the app log.
    pub fn open(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join(DB_FILE);
        let opened = (|| {
            let key = if encryption_compiled() {
                Some(database_key()?)
            } else {
                None
            };
            let log = AuditLog::open(&path, key.as_ref()).map_err(|e| e.to_string())?;
            Ok::<_, String>((log, key))
        })();
        let (log, database) = match opened {
            Ok((log, key)) => (Ok(log), Some((path.clone(), key))),
            Err(e) => (Err(e), None),
        };
        match &log {
            Ok(log) => tracing::info!(
                target: "shodh::audit",
                path = %path.display(),
                encrypted = log.is_encrypted(),
                "audit log opened"
            ),
            Err(e) => {
                tracing::error!(target: "shodh::audit", error = %e, "audit log unavailable; events will not be recorded")
            }
        }
        Self {
            log: log.map(Arc::new),
            database,
        }
    }

    /// Path and key of `shodh.db`, when the log opened (other components open
    /// their tables with the same key).
    pub fn database(&self) -> Option<(PathBuf, Option<AuditKey>)> {
        self.database.clone()
    }

    pub fn log(&self) -> Option<Arc<AuditLog>> {
        self.log.as_ref().ok().cloned()
    }

    fn require(&self) -> Result<Arc<AuditLog>, String> {
        self.log
            .as_ref()
            .cloned()
            .map_err(|e| format!("The audit log is unavailable: {e}"))
    }

    /// Queue an event. Never blocks.
    pub fn record(&self, record: AuditRecord) {
        if let Ok(log) = &self.log {
            log.submit(record);
        }
    }

    /// Audit sink for one conversation's agent session.
    pub fn tool_audit(&self, conversation_id: &str, profile_id: &str) -> Option<ToolAudit> {
        self.log().map(|log| ToolAudit {
            log,
            scope: AuditScope {
                principal: LOCAL_OWNER.to_string(),
                conversation_id: conversation_id.to_string(),
                profile_id: profile_id.to_string(),
            },
        })
    }

    /// Apply the stored retention period. Run once at startup.
    pub fn spawn_retention(&self) {
        let Some(log) = self.log() else { return };
        let spawned = std::thread::Builder::new()
            .name("shodh-audit-retention".to_string())
            .spawn(move || {
                let result = log
                    .retention_days()
                    .and_then(|days| log.apply_retention(days));
                match result {
                    Ok(0) => {}
                    Ok(deleted) => {
                        tracing::info!(target: "shodh::audit", deleted, "audit retention applied")
                    }
                    Err(e) => {
                        tracing::warn!(target: "shodh::audit", error = %e, "audit retention failed")
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(target: "shodh::audit", error = %e, "could not start audit retention");
        }
    }
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| format!("Audit task failed: {e}"))?
}

/// One page of events plus the number of matching events.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditPage {
    pub rows: Vec<AuditRow>,
    pub total: u64,
}

#[tauri::command]
pub async fn audit_query(
    query: Option<AuditQuery>,
    audit: State<'_, AuditState>,
) -> Result<AuditPage, String> {
    let log = audit.require()?;
    let query = query.unwrap_or_default();
    blocking(move || {
        let rows = log.query(&query).map_err(|e| e.to_string())?;
        let total = log.count(&query).map_err(|e| e.to_string())?;
        Ok(AuditPage { rows, total })
    })
    .await
}

/// Longest id accepted in a citation correction (conversation, run, message).
const MAX_CORRECTION_ID_LEN: usize = 200;
/// Most citation numbers a corrected claim can have cited.
const MAX_CORRECTION_NUMBERS: usize = 64;

/// The user moved one miscited claim's citation to the passage the grounding
/// check found supporting it ("Cite N instead"). Ids, numbers and counts
/// only: unknown fields are refused, so no answer or claim text can reach
/// the log through it (the rule `audit::tap` follows for grounding).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationCorrection {
    pub conversation_id: String,
    /// The run that wrote the answer, when the stored transcript has it.
    #[serde(default)]
    pub run_id: Option<String>,
    /// The text block of the answer the claim is in.
    pub message_id: String,
    /// The claim's position among the checked claims of the answer's final report.
    pub claim_index: u32,
    /// The passage numbers the claim cited.
    pub from: Vec<u32>,
    /// The passage it cites now.
    pub to: u32,
    /// The support score the check gave the claim against `to`.
    #[serde(default)]
    pub suggested_score: Option<f32>,
    /// The answer's grounding counts and score after the correction.
    pub summary: GroundingSummary,
}

fn valid_id(kind: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > MAX_CORRECTION_ID_LEN {
        Err(format!("Invalid {kind}"))
    } else {
        Ok(())
    }
}

fn valid_unit(kind: &str, value: Option<f32>) -> Result<(), String> {
    match value {
        Some(v) if !(0.0..=1.0).contains(&v) => Err(format!("Invalid {kind}")),
        _ => Ok(()),
    }
}

/// The audit record of a citation correction, after checking it.
fn citation_correction_record(c: &CitationCorrection) -> Result<AuditRecord, String> {
    valid_id("conversation id", &c.conversation_id)?;
    valid_id("message id", &c.message_id)?;
    if let Some(run) = &c.run_id {
        valid_id("run id", run)?;
    }
    if c.from.is_empty() || c.from.len() > MAX_CORRECTION_NUMBERS || c.from.contains(&0) {
        return Err("Invalid corrected citation".to_string());
    }
    if c.to == 0 || c.from.contains(&c.to) {
        return Err("Invalid new citation".to_string());
    }
    valid_unit("support score", c.suggested_score)?;
    valid_unit("grounding score", c.summary.score)?;
    let s = &c.summary;
    let record = AuditRecord::new(
        AuditEventType::CitationCorrected,
        json!({
            "message_id": c.message_id,
            "claim_index": c.claim_index,
            "from": c.from,
            "to": c.to,
            "suggested_score": c.suggested_score,
            "checked": s.checked,
            "supported": s.supported,
            "weak": s.weak,
            "unsupported": s.unsupported,
            "uncited": s.uncited,
            "invalid": s.invalid,
            "unchecked": s.unchecked,
            "score": s.score,
        }),
    )
    .conversation(c.conversation_id.clone());
    Ok(match &c.run_id {
        Some(run) => record.run(run.clone()),
        None => record,
    })
}

/// Record a citation the user corrected in an answer (`CitationCorrection`).
/// Waits until the event is written: the app changes the answer only after
/// this succeeds, so a correction never goes unrecorded.
#[tauri::command]
pub async fn audit_record_citation_correction(
    correction: CitationCorrection,
    audit: State<'_, AuditState>,
) -> Result<(), String> {
    let record = citation_correction_record(&correction)?;
    let log = audit.require()?;
    let id = blocking(move || log.append(record).map_err(|e| e.to_string())).await?;
    tracing::info!(target: "shodh::audit", event = "citation_corrected", id, from = ?correction.from, to = correction.to, "citation corrected by the user");
    Ok(())
}

#[tauri::command]
pub async fn audit_verify(audit: State<'_, AuditState>) -> Result<VerifyReport, String> {
    let log = audit.require()?;
    blocking(move || log.verify().map_err(|e| e.to_string())).await
}

fn export_path(path: &str, format: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err("Choose where to save the export".to_string());
    }
    match path.parent() {
        Some(parent) if parent.is_dir() => {}
        _ => return Err(format!("The folder for {} does not exist", path.display())),
    }
    if path.is_dir() {
        return Err(format!("{} is a folder", path.display()));
    }
    let expected = if format == "jsonl" { "jsonl" } else { "csv" };
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if extension.as_deref() != Some(expected) {
        return Ok(path.with_extension(expected));
    }
    Ok(path)
}

/// Export events matching `query` (all pages, oldest first). `path` comes
/// from the save dialog. Returns the number of events written.
#[tauri::command]
pub async fn audit_export(
    format: String,
    path: String,
    query: Option<AuditQuery>,
    audit: State<'_, AuditState>,
) -> Result<u64, String> {
    let log = audit.require()?;
    if format != "jsonl" && format != "csv" {
        return Err(format!(
            "Unknown export format {format:?}; use jsonl or csv"
        ));
    }
    let path = export_path(&path, &format)?;
    let mut query = query.unwrap_or_default();
    query.limit = None;
    query.offset = 0;
    blocking(move || {
        let written = if format == "jsonl" {
            log.export_jsonl(&path, &query)
        } else {
            log.export_csv(&path, &query)
        };
        written.map_err(|e| format!("Export to {} failed: {e}", path.display()))
    })
    .await
}

/// Persist the retention period (audited) and apply it now. Returns the
/// number of events deleted.
#[tauri::command]
pub async fn audit_set_retention_days(
    days: u32,
    audit: State<'_, AuditState>,
) -> Result<u64, String> {
    let log = audit.require()?;
    blocking(move || {
        log.set_retention_days(days).map_err(|e| e.to_string())?;
        log.apply_retention(days).map_err(|e| e.to_string())
    })
    .await
}

/// Start and end of the current calendar month in local time, as UTC.
fn local_month_bounds() -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    let now = Local::now();
    let (year, month) = (now.year(), now.month());
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let start = Local
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| now.with_timezone(&Utc));
    let end = Local
        .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| now.with_timezone(&Utc));
    (start, end)
}

#[tauri::command]
pub async fn audit_stats(audit: State<'_, AuditState>) -> Result<AuditStats, String> {
    let log = audit.require()?;
    let (start, end) = local_month_bounds();
    blocking(move || log.stats(start, end).map_err(|e| e.to_string())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_bounds_cover_now() {
        let (start, end) = local_month_bounds();
        let now = Utc::now();
        assert!(start <= now && now < end);
        assert!((end - start).num_days() >= 28);
    }

    fn correction() -> serde_json::Value {
        serde_json::json!({
            "conversationId": "conv-1",
            "runId": "run-1",
            "messageId": "m1",
            "claimIndex": 1,
            "from": [3],
            "to": 2,
            "suggestedScore": 0.91,
            "summary": {
                "checked": 2, "supported": 2, "weak": 0, "unsupported": 0,
                "uncited": 0, "invalid": 0, "unchecked": 0, "score": 1.0
            }
        })
    }

    #[test]
    fn a_citation_correction_records_numbers_and_counts_never_text() {
        let c: CitationCorrection = serde_json::from_value(correction()).unwrap();
        let record = citation_correction_record(&c).unwrap();
        assert_eq!(record.event_type, AuditEventType::CitationCorrected);
        assert_eq!(record.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(record.run_id.as_deref(), Some("run-1"));
        let payload = record.payload.as_object().unwrap();
        let keys: Vec<&str> = payload.keys().map(String::as_str).collect();
        let mut expected = vec![
            "checked",
            "claim_index",
            "from",
            "invalid",
            "message_id",
            "score",
            "suggested_score",
            "supported",
            "to",
            "unchecked",
            "uncited",
            "unsupported",
            "weak",
        ];
        expected.sort_unstable();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, expected);
        // The only string is the message id.
        let strings: Vec<&str> = payload.values().filter_map(|v| v.as_str()).collect();
        assert_eq!(strings, vec!["m1"]);
        assert_eq!(payload["from"], serde_json::json!([3]));
        assert_eq!(payload["to"], 2);
    }

    #[test]
    fn a_citation_correction_refuses_text_and_bad_numbers() {
        let mut with_text = correction();
        with_text["claimText"] = "SPANN expands posting lists.".into();
        assert!(serde_json::from_value::<CitationCorrection>(with_text).is_err());
        let mut no_run = correction();
        no_run.as_object_mut().unwrap().remove("runId");
        let c: CitationCorrection = serde_json::from_value(no_run).unwrap();
        assert_eq!(citation_correction_record(&c).unwrap().run_id, None);
        for (key, value) in [
            ("to", serde_json::json!(3)),
            ("to", serde_json::json!(0)),
            ("from", serde_json::json!([])),
            ("suggestedScore", serde_json::json!(1.5)),
            ("conversationId", serde_json::json!(" ")),
        ] {
            let mut bad = correction();
            bad[key] = value;
            let c: CitationCorrection = serde_json::from_value(bad).unwrap();
            assert!(citation_correction_record(&c).is_err(), "{key}");
        }
    }

    #[test]
    fn export_paths_get_the_format_extension() {
        let dir = std::env::temp_dir();
        let p = export_path(&dir.join("audit.txt").display().to_string(), "csv").unwrap();
        assert_eq!(p.extension().unwrap(), "csv");
        let p = export_path(&dir.join("audit.JSONL").display().to_string(), "jsonl").unwrap();
        assert_eq!(p.file_name().unwrap(), "audit.JSONL");
        assert!(export_path("relative.csv", "csv").is_err());
        assert!(export_path(&dir.display().to_string(), "csv").is_err());
    }
}
