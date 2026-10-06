//! The model picker's cached model list in `shodh.db` (`model_catalog_cache`).
//!
//! The parsed list is kept per source (e.g. `openrouter`) with the time it was
//! fetched. A list older than its TTL is refreshed when the network allows;
//! until then (and offline) the cached list is used, so prices stay visible.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};

use super::model_catalog::CatalogModel;
use crate::audit::{open_shared_connection, AuditError, AuditKey};

/// How long a fetched model list is used before it is fetched again.
pub const CATALOG_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum CatalogCacheError {
    #[error("The app database could not be opened: {0}")]
    Open(#[from] AuditError),
    #[error("The model list cache failed: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("The cached model list is not valid: {0}")]
    Decode(#[from] serde_json::Error),
}

/// A cached model list.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedCatalog {
    pub models: Vec<CatalogModel>,
    /// Unix milliseconds.
    pub fetched_at_ms: u64,
}

impl CachedCatalog {
    /// Fetched less than `ttl` before `now_ms` (and not in the future).
    pub fn is_fresh(&self, now_ms: u64, ttl: Duration) -> bool {
        let ttl_ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
        now_ms >= self.fetched_at_ms && now_ms - self.fetched_at_ms < ttl_ms
    }
}

pub struct CatalogCache {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for CatalogCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogCache").finish_non_exhaustive()
    }
}

impl CatalogCache {
    /// Open `shodh.db` at `path` with the app database key, if it has one.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> Result<Self, CatalogCacheError> {
        Ok(Self {
            conn: Mutex::new(open_shared_connection(path, key)?),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, source: &str) -> Result<Option<CachedCatalog>, CatalogCacheError> {
        let row: Option<(String, i64)> = self
            .lock()
            .query_row(
                "SELECT models_json, fetched_at_ms FROM model_catalog_cache WHERE source = ?1",
                params![source],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((json, fetched_at)) => Ok(Some(CachedCatalog {
                models: serde_json::from_str(&json)?,
                fetched_at_ms: u64::try_from(fetched_at).unwrap_or(0),
            })),
            None => Ok(None),
        }
    }

    pub fn put(
        &self,
        source: &str,
        models: &[CatalogModel],
        fetched_at_ms: u64,
    ) -> Result<(), CatalogCacheError> {
        let json = serde_json::to_string(models)?;
        let at = i64::try_from(fetched_at_ms).unwrap_or(i64::MAX);
        self.lock().execute(
            "INSERT INTO model_catalog_cache(source, models_json, fetched_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(source) DO UPDATE SET models_json = excluded.models_json,
                 fetched_at_ms = excluded.fetched_at_ms",
            params![source, json, at],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::model_catalog::parse_openrouter;

    #[test]
    fn lists_round_trip_and_age_out() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CatalogCache::open(&dir.path().join("shodh.db"), None).unwrap();
        assert_eq!(cache.get("openrouter").unwrap(), None);
        let models = parse_openrouter(
            r#"{"data":[{"id":"a/b:free","name":"B","context_length":1000,"pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"]}]}"#,
        )
        .unwrap();
        cache.put("openrouter", &models, 1_000).unwrap();
        cache.put("openrouter", &models, 2_000).unwrap();
        let cached = cache.get("openrouter").unwrap().unwrap();
        assert_eq!(cached.models, models);
        assert_eq!(cached.fetched_at_ms, 2_000);
        let ttl = Duration::from_secs(10);
        assert!(cached.is_fresh(2_000, ttl));
        assert!(cached.is_fresh(11_999, ttl));
        assert!(!cached.is_fresh(12_000, ttl));
        assert!(!cached.is_fresh(1_000, ttl), "a clock behind the fetch is not trusted");
    }
}
