//! Keeping the chunk table's files and vector index up to date after writes.
//!
//! Every insert, update and delete adds files and a table version. Merging
//! the files, pruning old versions and indexing new rows rewrites data and
//! takes seconds on a large library, so no write does it itself: a write
//! marks the table changed, and [`StoreMaintenance::run`] does it once for a
//! batch of changes (the end of an indexing run, or writes gone quiet).
//! The handle holds its own connection, so it runs without the engine lock.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lancedb::table::OptimizeAction;

/// Rows from which the vectors are indexed. Below it an exact scan takes
/// milliseconds; at 475,000 rows it took 0.5–0.8 s per search.
pub const VECTOR_INDEX_MIN_ROWS: usize = 20_000;

/// How long table versions are kept. Only this process reads them, so an
/// hour is ample; without pruning, a library indexed again and again kept
/// 20 GB of old manifests next to 3 GB of data.
const KEEP_VERSIONS_HOURS: i64 = 1;

struct Writes {
    changed: AtomicBool,
    /// Milliseconds after `epoch` of the last write.
    last_ms: AtomicU64,
    epoch: Instant,
    /// One pass at a time.
    running: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct StoreMaintenance {
    db: lancedb::Connection,
    table_name: String,
    writes: Arc<Writes>,
    index_min_rows: usize,
}

impl StoreMaintenance {
    pub(crate) fn new(db: lancedb::Connection, table_name: &str) -> Self {
        Self {
            db,
            table_name: table_name.to_string(),
            writes: Arc::new(Writes {
                changed: AtomicBool::new(false),
                last_ms: AtomicU64::new(0),
                epoch: Instant::now(),
                running: tokio::sync::Mutex::new(()),
            }),
            index_min_rows: VECTOR_INDEX_MIN_ROWS,
        }
    }

    #[cfg(test)]
    fn index_from(mut self, rows: usize) -> Self {
        self.index_min_rows = rows;
        self
    }

    /// The table was written to.
    pub(crate) fn wrote(&self) {
        let now = self.writes.epoch.elapsed().as_millis() as u64;
        self.writes.last_ms.store(now, Ordering::SeqCst);
        self.writes.changed.store(true, Ordering::SeqCst);
    }

    /// Whether the table changed since the last pass.
    pub fn changed(&self) -> bool {
        self.writes.changed.load(Ordering::SeqCst)
    }

    /// Runs a pass when the table changed and nothing was written for
    /// `quiet`, so a burst of writes (a sync removing many files) is
    /// maintained once, after it.
    pub async fn run_when_quiet(&self, quiet: Duration) -> Result<bool> {
        let last = Duration::from_millis(self.writes.last_ms.load(Ordering::SeqCst));
        if !self.changed() || self.writes.epoch.elapsed().saturating_sub(last) < quiet {
            return Ok(false);
        }
        self.run().await?;
        Ok(true)
    }

    /// Checks every `every` and runs a pass once writes have been quiet for
    /// `quiet`, catching changes made outside an indexing run (removals,
    /// moves, folder sync). Never returns.
    pub async fn maintain_when_quiet(self, quiet: Duration, every: Duration) {
        let mut ticks = tokio::time::interval(every);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            if let Err(e) = self.run_when_quiet(quiet).await {
                tracing::warn!(error = %format!("{e:#}"), "chunk store maintenance failed");
            }
        }
    }

    /// Runs a pass when the table changed since the last one.
    pub async fn run_if_changed(&self) -> Result<bool> {
        if !self.changed() {
            return Ok(false);
        }
        self.run().await?;
        Ok(true)
    }

    /// Merges small files and drops deleted rows, removes old versions and
    /// indexes new rows ([`index_vectors`]). Each step is best-effort and
    /// logged; a failed merge leaves the table marked changed.
    pub async fn run(&self) -> Result<()> {
        let _one = self.writes.running.lock().await;
        self.writes.changed.store(false, Ordering::SeqCst);
        let table = self
            .db
            .open_table(&self.table_name)
            .execute()
            .await
            .context("Failed to open table for compaction")?;
        let started = Instant::now();
        let compaction = table
            .optimize(OptimizeAction::Compact {
                options: Default::default(),
                remap_options: None,
            })
            .await;
        if let Err(e) = &compaction {
            self.writes.changed.store(true, Ordering::SeqCst);
            tracing::warn!("LanceDB compaction failed (non-fatal): {}", e);
        }
        if let Err(e) = table
            .optimize(OptimizeAction::Prune {
                older_than: Some(lancedb::table::Duration::hours(KEEP_VERSIONS_HOURS)),
                delete_unverified: None,
                error_if_tagged_old_versions: None,
            })
            .await
        {
            tracing::warn!("LanceDB version pruning failed (non-fatal): {}", e);
        }
        if let Err(e) = index_vectors(&table, self.index_min_rows).await {
            tracing::warn!(error = %format!("{e:#}"), "vector index not updated (non-fatal)");
        }
        tracing::info!(
            compaction = ?compaction.ok().and_then(|s| s.compaction),
            ms = started.elapsed().as_millis() as u64,
            "LanceDB maintenance completed"
        );
        Ok(())
    }

    /// Builds or updates the vector index now, without merging files.
    pub async fn index_vectors(&self) -> Result<()> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        index_vectors(&table, self.index_min_rows).await
    }
}

/// Indexes the vectors once the table is large enough for a scan to be slow:
/// IVF-HNSW with int8 scalar quantisation, by cosine distance, the distance
/// searches ask for. Lance never uses an index built for another distance
/// (it scans instead), so such an index is rebuilt. An existing cosine index
/// takes in the new rows.
async fn index_vectors(table: &lancedb::Table, min_rows: usize) -> Result<()> {
    let existing = table
        .list_indices()
        .await?
        .into_iter()
        .find(|index| index.columns.iter().any(|c| c == "vector"));
    if let Some(index) = &existing {
        let stats = table.index_stats(&index.name).await?;
        if stats.and_then(|s| s.distance_type) == Some(lancedb::DistanceType::Cosine) {
            table
                .optimize(OptimizeAction::Index(Default::default()))
                .await?;
            return Ok(());
        }
    }
    let rows = table.count_rows(None).await?;
    if rows < min_rows {
        return Ok(());
    }
    let started = Instant::now();
    table
        .create_index(
            &["vector"],
            lancedb::index::Index::IvfHnswSq(
                lancedb::index::vector::IvfHnswSqIndexBuilder::default()
                    .distance_type(lancedb::DistanceType::Cosine),
            ),
        )
        .replace(true)
        .execute()
        .await
        .context("building the vector index")?;
    tracing::info!(
        rows,
        replaced = existing.is_some(),
        ms = started.elapsed().as_millis() as u64,
        "Built the cosine vector index"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::LanceStore;
    use crate::types::ChunkRecord;

    const DIMENSION: usize = 16;

    fn chunks(from: usize, count: usize) -> Vec<ChunkRecord> {
        (from..from + count)
            .map(|i| ChunkRecord {
                id: format!("c{i}"),
                doc_id: format!("d{}", i / 4),
                chunk_index: (i % 4) as u32,
                text: format!("chunk {i}"),
                title: String::new(),
                source: format!("/docs/{}.txt", i / 4),
                heading: String::new(),
                vector: (0..DIMENSION)
                    .map(|d| (((i * 31 + d * 7) % 97) as f32 / 97.0) - 0.5)
                    .collect(),
                space_id: "main".into(),
                metadata_json: "{}".into(),
                citation_json: "{}".into(),
                created_at: 0,
            })
            .collect()
    }

    async fn store(dir: &tempfile::TempDir) -> LanceStore {
        LanceStore::open(dir.path().to_str().unwrap(), DIMENSION, "documents")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn writes_mark_the_table_changed_until_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let maintenance = store.maintenance();
        assert!(!maintenance.run_if_changed().await.unwrap());

        store.upsert_chunks(chunks(0, 8)).await.unwrap();
        assert!(maintenance.changed());
        assert!(maintenance.run_if_changed().await.unwrap());
        assert!(!maintenance.changed());

        store.delete_by_doc_id("d0").await.unwrap();
        assert!(maintenance.changed());
        maintenance.run().await.unwrap();
        assert_eq!(store.count().await.unwrap(), 4);
    }

    #[tokio::test]
    async fn a_quiet_pass_waits_for_writes_to_stop() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let maintenance = store.maintenance();
        store.upsert_chunks(chunks(0, 4)).await.unwrap();

        assert!(!maintenance
            .run_when_quiet(Duration::from_secs(3600))
            .await
            .unwrap());
        assert!(maintenance.changed());
        assert!(maintenance.run_when_quiet(Duration::ZERO).await.unwrap());
        assert!(!maintenance.run_when_quiet(Duration::ZERO).await.unwrap());
    }

    async fn vector_index(
        maintenance: &StoreMaintenance,
    ) -> Option<lancedb::index::IndexStatistics> {
        let table = maintenance
            .db
            .open_table("documents")
            .execute()
            .await
            .unwrap();
        let index = table
            .list_indices()
            .await
            .unwrap()
            .into_iter()
            .find(|index| index.columns.iter().any(|c| c == "vector"))?;
        table.index_stats(&index.name).await.unwrap()
    }

    #[tokio::test]
    async fn the_vectors_get_a_cosine_index_past_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let maintenance = store.maintenance().index_from(512);
        store.upsert_chunks(chunks(0, 256)).await.unwrap();
        maintenance.run().await.unwrap();
        assert!(vector_index(&maintenance).await.is_none());

        store.upsert_chunks(chunks(256, 512)).await.unwrap();
        maintenance.run().await.unwrap();
        let stats = vector_index(&maintenance).await.expect("a vector index");
        assert_eq!(stats.distance_type, Some(lancedb::DistanceType::Cosine));

        store.upsert_chunks(chunks(768, 64)).await.unwrap();
        maintenance.run().await.unwrap();
        let stats = vector_index(&maintenance).await.expect("a vector index");
        assert_eq!(stats.num_unindexed_rows, 0);

        let query = chunks(5, 1).remove(0).vector;
        let hits = store.vector_search(&query, 3, None).await.unwrap();
        assert_eq!(hits.len(), 3);
    }
}
