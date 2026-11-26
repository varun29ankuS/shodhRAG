//! What a step gave for inputs it has seen, in `shodh.db` (`engine_cache`, migration 23):
//! a run whose step has the same inputs again takes the stored result instead of calling
//! the model.
//!
//! - **The key** ([`cache_key`]) is the SHA-256 of the automation's revision, the step's
//!   index and the step's inputs as canonical JSON (object keys sorted). The caller puts
//!   in the inputs everything the result depends on: the occurrence's date, the sources
//!   that changed, what an event brought. So unchanged inputs make no call, and a new
//!   day or a changed source always does.
//! - **Rows go** with their job ([`EngineStore::delete_job`]) and after
//!   [`CACHE_RETENTION_DAYS`] ([`EngineStore::prune`]).

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::lease::timestamp;
use super::store::{EngineError, EngineResult, EngineStore};

/// Days a cached result is kept.
pub const CACHE_RETENTION_DAYS: i64 = 90;

/// `value` as JSON text with every object's keys in order, so equal values give equal
/// text whatever order they were built in.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The key of a step's result: SHA-256 (hex) of the automation's `revision`, the step's
/// index and its `inputs`.
pub fn cache_key(revision: u32, step_idx: u32, inputs: &Value) -> String {
    let mut text = format!("{revision}\n{step_idx}\n");
    canonical(inputs, &mut text);
    hex::encode(Sha256::digest(text.as_bytes()).as_slice())
}

impl EngineStore {
    /// What the job's step gave for the inputs of `input_key`, if it has seen them.
    pub fn cached(&self, job_id: &str, input_key: &str) -> EngineResult<Option<Value>> {
        let conn = self.conn.read()?;
        let text: Option<String> = conn
            .query_row(
                "SELECT output_json FROM engine_cache WHERE job_id = ?1 AND input_key = ?2",
                params![job_id, input_key],
                |r| r.get(0),
            )
            .optional()?;
        text.map(|t| serde_json::from_str(&t).map_err(|e| EngineError::Invalid(e.to_string())))
            .transpose()
    }

    /// Keeps what the job's step gave for the inputs of `input_key` (the latest result
    /// replaces an earlier one).
    pub fn cache(
        &self,
        job_id: &str,
        input_key: &str,
        output: &Value,
        now: DateTime<Utc>,
    ) -> EngineResult<()> {
        let at = timestamp(now);
        self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO engine_cache (job_id, input_key, output_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(job_id, input_key) DO UPDATE SET
                     output_json = excluded.output_json, created_at = excluded.created_at",
                params![job_id, input_key, output.to_string(), at],
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::store::{CatchUp, CreatedBy, JobKind, NewJob, Overlap};
    use chrono::{TimeDelta, TimeZone};
    use serde_json::json;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, 9, 0, 0).unwrap()
    }

    fn store() -> (tempfile::TempDir, EngineStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = EngineStore::open(&dir.path().join("shodh.db"), None).unwrap();
        (dir, store)
    }

    fn job(store: &EngineStore, id: &str) {
        store
            .create_job(
                &NewJob {
                    id: id.into(),
                    kind: JobKind::Playbook,
                    subject: id.into(),
                    title: "Weekly digest".into(),
                    schedule: None,
                    input: json!({}),
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 3,
                    created_by: CreatedBy::User,
                    provenance: json!({}),
                },
                at(1),
            )
            .unwrap();
    }

    #[test]
    fn the_key_follows_the_revision_the_step_and_the_inputs_not_their_order() {
        let a = json!({ "date": "2026-11-24", "changed": [{ "id": "s1", "modified": "m1" }] });
        let same: Value = serde_json::from_str(
            r#"{"changed":[{"modified":"m1","id":"s1"}],"date":"2026-11-24"}"#,
        )
        .unwrap();
        let key = cache_key(3, 0, &a);
        assert_eq!(key.len(), 64);
        assert_eq!(key, cache_key(3, 0, &same), "key order does not matter");
        assert_ne!(key, cache_key(4, 0, &a), "a changed automation");
        assert_ne!(key, cache_key(3, 1, &a), "another step");
        let next_day =
            json!({ "date": "2026-11-25", "changed": [{ "id": "s1", "modified": "m1" }] });
        assert_ne!(key, cache_key(3, 0, &next_day), "a new occurrence");
        let changed =
            json!({ "date": "2026-11-24", "changed": [{ "id": "s1", "modified": "m2" }] });
        assert_ne!(key, cache_key(3, 0, &changed), "a changed source");
        let reordered = json!({ "date": "2026-11-24", "changed": [] });
        assert_ne!(key, cache_key(3, 0, &reordered));
    }

    #[test]
    fn a_result_is_read_back_replaced_and_kept_per_job() {
        let (_dir, store) = store();
        assert_eq!(store.cached("j1", "k").unwrap(), None);
        store
            .cache("j1", "k", &json!({ "summary": "first" }), at(1))
            .unwrap();
        store
            .cache("j1", "k", &json!({ "summary": "second" }), at(2))
            .unwrap();
        assert_eq!(
            store.cached("j1", "k").unwrap(),
            Some(json!({ "summary": "second" }))
        );
        assert_eq!(store.cached("j2", "k").unwrap(), None, "another job's key");
        assert_eq!(store.cached("j1", "other").unwrap(), None);
    }

    #[test]
    fn results_go_with_their_job_and_after_ninety_days() {
        let (_dir, store) = store();
        job(&store, "j1");
        job(&store, "j2");
        store.cache("j1", "old", &json!({}), at(1)).unwrap();
        store.cache("j2", "kept", &json!({}), at(1)).unwrap();
        assert!(store.delete_job("j1").unwrap());
        assert_eq!(store.cached("j1", "old").unwrap(), None);
        assert!(store.cached("j2", "kept").unwrap().is_some());
        store
            .prune(at(1) + TimeDelta::days(CACHE_RETENTION_DAYS) - TimeDelta::hours(1))
            .unwrap();
        assert!(store.cached("j2", "kept").unwrap().is_some());
        store
            .prune(at(1) + TimeDelta::days(CACHE_RETENTION_DAYS) + TimeDelta::hours(1))
            .unwrap();
        assert_eq!(store.cached("j2", "kept").unwrap(), None);
    }
}
