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
    /// Held by whatever rewrites or removes the table's files: a pass, an
    /// index build, a purge ([`StoreMaintenance::exclusive`]). A pass's
    /// compaction and index build write files no version names until they
    /// commit; a purge deletes every such file, so one running during the
    /// other leaves the table naming files that are gone.
    running: tokio::sync::Mutex<()>,
    #[cfg(test)]
    probe: std::sync::Mutex<Option<Arc<tests::Probe>>>,
}

/// What takes the table's files exclusively (logged when it has to wait).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holder {
    Pass,
    IndexBuild,
    Purge,
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
                #[cfg(test)]
                probe: std::sync::Mutex::new(None),
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
        let _files = self.exclusive(Holder::Pass).await;
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
        let _files = self.exclusive(Holder::IndexBuild).await;
        let table = self.db.open_table(&self.table_name).execute().await?;
        index_vectors(&table, self.index_min_rows).await
    }

    /// Takes the table's files for `holder` until the guard drops, waiting
    /// for a pass, index build or purge already running (in the order they
    /// asked), never skipping. Not reentrant: the holder must not ask again.
    pub(crate) async fn exclusive(&self, holder: Holder) -> tokio::sync::MutexGuard<'_, ()> {
        let guard = match self.writes.running.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::debug!(
                    ?holder,
                    table = %self.table_name,
                    "waiting for the chunk store's files"
                );
                #[cfg(test)]
                self.writes.probe_event(tests::Event::Waiting(holder));
                self.writes.running.lock().await
            }
        };
        #[cfg(test)]
        self.writes.probe_holding(holder).await;
        guard
    }
}

#[cfg(test)]
impl Writes {
    fn probe(&self) -> Option<Arc<tests::Probe>> {
        self.probe.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn probe_event(&self, event: tests::Event) {
        if let Some(probe) = self.probe() {
            probe.events.send(event).ok();
        }
    }

    /// Reports `holder` holding the files and, if the probe pauses the first
    /// holder, waits there until the test releases it.
    async fn probe_holding(&self, holder: Holder) {
        let Some(probe) = self.probe() else {
            return;
        };
        probe.events.send(tests::Event::Holding(holder)).ok();
        let release = probe
            .pause_first
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(release) = release {
            release.await.ok();
        }
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Event {
        /// Asked for the table's files while another held them.
        Waiting(Holder),
        /// Holds the table's files.
        Holding(Holder),
        /// Returned (sent by the test's task).
        Done(Holder),
    }

    /// Reports who waits for and holds the table's files, and pauses the
    /// first holder until the test releases it.
    pub(super) struct Probe {
        pub(super) events: tokio::sync::mpsc::UnboundedSender<Event>,
        pub(super) pause_first: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    struct Probed {
        events: tokio::sync::mpsc::UnboundedReceiver<Event>,
        sender: tokio::sync::mpsc::UnboundedSender<Event>,
        release: tokio::sync::oneshot::Sender<()>,
    }

    fn probe(maintenance: &StoreMaintenance) -> Probed {
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        let (release, paused) = tokio::sync::oneshot::channel();
        *maintenance
            .writes
            .probe
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(Probe {
            events: sender.clone(),
            pause_first: std::sync::Mutex::new(Some(paused)),
        }));
        Probed {
            events,
            sender,
            release,
        }
    }

    fn spawn_pass(
        maintenance: &StoreMaintenance,
        done: &tokio::sync::mpsc::UnboundedSender<Event>,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let (maintenance, done) = (maintenance.clone(), done.clone());
        tokio::spawn(async move {
            let ran = maintenance.run().await;
            done.send(Event::Done(Holder::Pass)).ok();
            ran
        })
    }

    fn spawn_purge(
        store: &Arc<LanceStore>,
        done: &tokio::sync::mpsc::UnboundedSender<Event>,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let (store, done) = (store.clone(), done.clone());
        tokio::spawn(async move {
            let purged = store.purge_history().await;
            done.send(Event::Done(Holder::Purge)).ok();
            purged
        })
    }

    /// Every event left once both tasks returned.
    fn rest(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
        std::iter::from_fn(|| events.try_recv().ok()).collect()
    }

    /// 64 chunks with d0's 4 deleted, so the purge and pass have work.
    async fn store_with_deletions(dir: &tempfile::TempDir) -> Arc<LanceStore> {
        let store = store(dir).await;
        store.upsert_chunks(chunks(0, 64)).await.unwrap();
        store.delete_by_doc_id("d0").await.unwrap();
        Arc::new(store)
    }

    /// The table reads whole after the purge: every file a version names is
    /// there, in this handle and a fresh one.
    async fn assert_readable(dir: &tempfile::TempDir, store: &LanceStore) {
        assert_eq!(store.count().await.unwrap(), 60);
        assert_eq!(store.list_chunks(None, 1000).await.unwrap().len(), 60);
        let query = chunks(5, 1).remove(0).vector;
        assert_eq!(store.vector_search(&query, 3, None).await.unwrap().len(), 3);
        let reopened = self::store(dir).await;
        assert_eq!(reopened.list_chunks(None, 1000).await.unwrap().len(), 60);
        reopened.maintenance().run().await.unwrap();
    }

    // A purge deletes every file no version names yet, which includes a
    // running pass's new files. The pass is paused while it holds the files;
    // the purge must report waiting there, then run once the pass ends.
    // Without the lock the purge never waits: its next event is Done.
    #[tokio::test]
    async fn a_purge_waits_for_a_running_pass_then_runs() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_deletions(&dir).await;
        let maintenance = store.maintenance();
        let mut probed = probe(&maintenance);

        let pass = spawn_pass(&maintenance, &probed.sender);
        assert_eq!(
            probed.events.recv().await,
            Some(Event::Holding(Holder::Pass))
        );
        let purge = spawn_purge(&store, &probed.sender);
        assert_eq!(
            probed.events.recv().await,
            Some(Event::Waiting(Holder::Purge)),
            "the purge ran while a pass held the table's files"
        );

        probed.release.send(()).unwrap();
        pass.await.unwrap().unwrap();
        purge.await.unwrap().unwrap();
        let rest = rest(&mut probed.events);
        assert_eq!(rest.len(), 3, "{rest:?}");
        assert!(rest.contains(&Event::Done(Holder::Pass)), "{rest:?}");
        assert!(rest.contains(&Event::Holding(Holder::Purge)), "{rest:?}");
        assert_eq!(rest.last(), Some(&Event::Done(Holder::Purge)), "{rest:?}");
        assert_readable(&dir, &store).await;
    }

    // The reverse order: a pass (or index build) asked for during a purge
    // waits for it. Without the lock in the purge it never holds the files,
    // so the first event is the purge's Done.
    #[tokio::test]
    async fn a_pass_waits_for_a_running_purge() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_deletions(&dir).await;
        let maintenance = store.maintenance();
        let mut probed = probe(&maintenance);

        let purge = spawn_purge(&store, &probed.sender);
        assert_eq!(
            probed.events.recv().await,
            Some(Event::Holding(Holder::Purge)),
            "the purge did not take the table's files"
        );
        let pass = spawn_pass(&maintenance, &probed.sender);
        assert_eq!(
            probed.events.recv().await,
            Some(Event::Waiting(Holder::Pass)),
            "a pass ran while the purge held the table's files"
        );
        let index = {
            let maintenance = maintenance.clone();
            tokio::spawn(async move { maintenance.index_vectors().await })
        };
        assert_eq!(
            probed.events.recv().await,
            Some(Event::Waiting(Holder::IndexBuild)),
            "an index build ran while the purge held the table's files"
        );

        probed.release.send(()).unwrap();
        purge.await.unwrap().unwrap();
        pass.await.unwrap().unwrap();
        index.await.unwrap().unwrap();
        let rest = rest(&mut probed.events);
        assert_eq!(rest.len(), 4, "{rest:?}");
        for event in [
            Event::Done(Holder::Purge),
            Event::Holding(Holder::Pass),
            Event::Done(Holder::Pass),
            Event::Holding(Holder::IndexBuild),
        ] {
            assert!(rest.contains(&event), "{event:?} missing from {rest:?}");
        }
        assert_readable(&dir, &store).await;
    }

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
