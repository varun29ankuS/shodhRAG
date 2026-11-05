//! Tests of [`super`]: a library with chunks and memories, switched to another
//! "export" (a deterministic embedder whose vectors differ from the index's).

use super::*;
use crate::statements::testing::{
    ontology, preference, t0, FixedEmbedder, HashEmbedder, TestClock, DIM,
};
use crate::statements::{DynamicsStore, PutIntent, Scope, StatementStore};
use crate::types::{Citation, DocumentFormat};
use crate::{RAGConfig, RAGEngine, SearchModels};
use std::collections::HashMap as Map;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Another "export": every vector of [`HashEmbedder`] negated, so its vectors
/// are told from the index's. Counts the passages it embeds.
#[derive(Default)]
struct OtherExport {
    embedded: AtomicUsize,
}

impl OtherExport {
    /// The vector it gives `text`, without counting it.
    fn vector(&self, text: &str) -> Vec<f32> {
        HashEmbedder
            .embed_document(text)
            .unwrap()
            .into_iter()
            .map(|x| -x)
            .collect()
    }
}

impl EmbeddingModel for OtherExport {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_document(text)
    }
    fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        self.embedded.fetch_add(1, Ordering::SeqCst);
        Ok(self.vector(text))
    }
    fn dimension(&self) -> usize {
        DIM
    }
}

fn config(dir: &Path) -> RAGConfig {
    let mut config = RAGConfig::default();
    config.data_dir = dir.to_path_buf();
    config.embedding.model_dir = dir.join("models");
    config.embedding.use_e5 = false;
    config.embedding.dimension = DIM;
    config
}

/// The library's engine, embedding with the stateless [`HashEmbedder`]: a
/// stored chunk's vector is the hash vector of exactly the text the indexer
/// embedded.
async fn engine(dir: &Path) -> RAGEngine {
    let mut engine = RAGEngine::new(config(dir)).await.unwrap();
    engine
        .attach_search_models(SearchModels::from_embedder(Arc::new(HashEmbedder)))
        .unwrap();
    engine
}

async fn add(engine: &mut RAGEngine, source: &str, text: &str) {
    let mut metadata = Map::new();
    metadata.insert("source".to_string(), source.to_string());
    metadata.insert("title".to_string(), source.to_string());
    engine
        .add_document(text, DocumentFormat::TXT, metadata, Citation::default())
        .await
        .unwrap();
}

/// The memory of the library in `dir`, embedding with [`HashEmbedder`].
async fn memory(dir: &Path) -> StatementStore {
    let (_, statements) = index_model::tables(dir).unwrap();
    StatementStore::open_table(
        &dir.join(LANCE_DIR),
        &statements,
        DIM,
        ontology(),
        Arc::new(DynamicsStore::open(&dir.join("shodh.db"), None).unwrap()),
        Arc::new(FixedEmbedder(Arc::new(HashEmbedder))),
        TestClock::at(t0()),
    )
    .await
    .unwrap()
}

async fn remember(store: &StatementStore, id: &str, topic: &str) {
    store
        .put(
            preference(id, topic, "PDF", t0()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
}

type Stored = (String, String, Vec<f32>);

/// Every row of `table`: its id, the text its vector came from, the vector.
async fn rows(dir: &Path, table: &str, kind: Rows) -> Vec<Stored> {
    let db = connect(dir).await.unwrap();
    let table = db.open_table(table).execute().await.unwrap();
    let batches: Vec<RecordBatch> = table
        .query()
        .execute()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut out = Vec::new();
    for batch in &batches {
        let ids = string_column(batch, "id").unwrap();
        let vectors = batch
            .column_by_name("vector")
            .unwrap()
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .unwrap();
        for row in 0..batch.num_rows() {
            let values = vectors.value(row);
            let values = values.as_any().downcast_ref::<Float32Array>().unwrap();
            out.push((
                ids.value(row).to_string(),
                embedding_input(kind, batch, row).unwrap(),
                values.values().to_vec(),
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn ids(rows: &[Stored]) -> Vec<String> {
    rows.iter().map(|r| r.0.clone()).collect()
}

async fn table_names(dir: &Path) -> Vec<String> {
    connect(dir)
        .await
        .unwrap()
        .table_names()
        .execute()
        .await
        .unwrap()
}

/// A library of two notes and one memory, built before records: opening it
/// records fp16.
async fn library(dir: &Path) {
    {
        let mut engine = engine(dir).await;
        add(
            &mut engine,
            "note://tide",
            "Tide pools hold anemones and crabs at low tide.",
        )
        .await;
        add(
            &mut engine,
            "note://lease",
            "The lease notice period is sixty days in writing.",
        )
        .await;
    }
    std::fs::remove_file(dir.join(index_model::INDEX_MODEL_FILE)).ok();
    remember(&memory(dir).await, "p1", "report format").await;
    drop(RAGEngine::new(config(dir)).await.unwrap());
    assert_eq!(
        index_model::read(dir).unwrap().map(|r| r.variant),
        Some(E5Variant::Fp16)
    );
}

fn never() -> impl FnMut() -> std::future::Ready<bool> {
    || std::future::ready(false)
}

async fn built(dir: &Path, other: &Arc<OtherExport>) {
    assert_eq!(
        build(dir, other.clone(), never(), &|_| {}).await.unwrap(),
        BuildOutcome::Built
    );
}

async fn finished(dir: &Path, other: &Arc<OtherExport>) -> IndexModel {
    let loader = other.clone();
    finish_pending(dir, move |_| Ok(loader as Arc<dyn EmbeddingModel>))
        .await
        .unwrap()
        .expect("switched")
}

/// One row of `name` again with the id `id`, as an older release would add
/// a row.
async fn add_row_like(dir: &Path, name: &str, id: &str) {
    let db = connect(dir).await.unwrap();
    let table = db.open_table(name).execute().await.unwrap();
    let schema = table.schema().await.unwrap();
    let one: Vec<RecordBatch> = table
        .query()
        .limit(1)
        .execute()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut columns = one[0].columns().to_vec();
    columns[schema.index_of("id").unwrap()] = Arc::new(StringArray::from(vec![id]));
    let row = RecordBatch::try_new(schema.clone(), columns).unwrap();
    table
        .add(Box::new(RecordBatchIterator::new([Ok(row)], schema)))
        .execute()
        .await
        .unwrap();
}

#[tokio::test]
async fn a_build_copies_every_row_with_a_vector_of_the_new_export() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    let switch = start(dir.path(), E5Variant::Int8).await.unwrap();
    assert_eq!(switch.documents_table, "documents_int8");
    assert_eq!(switch.statements_table, "statements_int8");
    assert_eq!(
        start(dir.path(), E5Variant::Int8).await.unwrap(),
        switch,
        "starting again continues it"
    );
    let other = Arc::new(OtherExport::default());
    let seen = std::sync::Mutex::new(Vec::new());
    let outcome = build(dir.path(), other.clone(), never(), &|p| {
        seen.lock().unwrap().push(p)
    })
    .await
    .unwrap();
    assert_eq!(outcome, BuildOutcome::Built);
    assert!(pending(dir.path()).unwrap().unwrap().built);
    let last = *seen.lock().unwrap().last().unwrap();
    assert_eq!(last.done, last.total);
    assert!(last.total >= 3);

    for (old, new, kind) in [
        ("documents", "documents_int8", Rows::Documents),
        ("statements", "statements_int8", Rows::Statements),
    ] {
        let before = rows(dir.path(), old, kind).await;
        let after = rows(dir.path(), new, kind).await;
        assert!(!before.is_empty(), "{old} has rows");
        assert_eq!(ids(&before), ids(&after), "{new}: the same rows");
        for ((_, text, old_vector), (_, new_text, vector)) in before.iter().zip(&after) {
            // The text rebuilt from the stored row is exactly what the indexer
            // and the memory embedded: its vector is the stored one.
            assert_eq!(
                old_vector,
                &HashEmbedder.embed_document(text).unwrap(),
                "{old}: the stored vector is of the rebuilt text {text:?}"
            );
            assert_eq!(text, new_text, "the same content");
            assert_eq!(vector, &other.vector(text), "the new export's vector");
        }
    }

    // Built again: every row is unchanged, so nothing is embedded.
    let embedded = other.embedded.load(Ordering::SeqCst);
    built(dir.path(), &other).await;
    assert_eq!(other.embedded.load(Ordering::SeqCst), embedded);
    // The index in use is untouched.
    let record = index_model::read(dir.path()).unwrap().unwrap();
    assert_eq!(record.variant, E5Variant::Fp16);
    assert_eq!(record.documents_table, "documents");
}

#[tokio::test]
async fn a_stopped_build_keeps_its_rows_and_a_cancel_drops_only_its_own_tables() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    {
        let mut engine = engine(dir.path()).await;
        for i in 0..40 {
            add(
                &mut engine,
                &format!("note://{i}"),
                &format!("Note {i} says the meeting moved to room {i} on Friday afternoon."),
            )
            .await;
        }
    }
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    let asked = AtomicUsize::new(0);
    // Stops when asked the second time: one step of rows is copied.
    let outcome = build(
        dir.path(),
        other.clone(),
        || std::future::ready(asked.fetch_add(1, Ordering::SeqCst) >= 1),
        &|_| {},
    )
    .await
    .unwrap();
    assert_eq!(outcome, BuildOutcome::Stopped);
    assert!(!pending(dir.path()).unwrap().unwrap().built);
    assert_eq!(
        rows(dir.path(), "documents_int8", Rows::Documents)
            .await
            .len(),
        STEP_ROWS
    );
    assert_eq!(other.embedded.load(Ordering::SeqCst), STEP_ROWS);

    // Continued: only the rest is embedded.
    built(dir.path(), &other).await;
    let total = rows(dir.path(), "documents", Rows::Documents).await.len()
        + rows(dir.path(), "statements", Rows::Statements).await.len();
    assert_eq!(other.embedded.load(Ordering::SeqCst), total);

    assert!(cancel(dir.path()).await.unwrap());
    assert_eq!(pending(dir.path()).unwrap(), None);
    let names = table_names(dir.path()).await;
    assert!(!names.contains(&"documents_int8".to_string()), "{names:?}");
    assert!(!names.contains(&"statements_int8".to_string()), "{names:?}");
    assert!(names.contains(&"documents".to_string()));
    assert!(names.contains(&"statements".to_string()));
    assert!(!cancel(dir.path()).await.unwrap(), "nothing left to cancel");
}

/// The switch completes at start with what changed since the build, after
/// checking the new tables; the old tables stay one start as a way back and
/// go at the start after.
#[tokio::test]
async fn the_switch_completes_at_start_and_keeps_the_old_tables_one_start() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    // After the build: a note removed, another added.
    {
        let mut engine = engine(dir.path()).await;
        assert!(engine.delete_by_source("note://tide").await.unwrap() > 0);
        add(
            &mut engine,
            "note://kettle",
            "The kettle descales with citric acid every month.",
        )
        .await;
    }
    let live = rows(dir.path(), "documents", Rows::Documents).await;
    let switched = finished(dir.path(), &other).await;
    assert_eq!(switched.variant, E5Variant::Int8);
    assert_eq!(switched.documents_table, "documents_int8");
    assert_eq!(switched.statements_table, "statements_int8");
    assert_eq!(switched.reason, "switched from fp16");
    let retired: Vec<&str> = switched.retired.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(retired, ["documents", "statements"]);
    assert_eq!(index_model::read(dir.path()).unwrap(), Some(switched));
    assert_eq!(pending(dir.path()).unwrap(), None);

    let now = rows(dir.path(), "documents_int8", Rows::Documents).await;
    assert_eq!(
        ids(&now),
        ids(&live),
        "exactly the rows in use at the switch"
    );
    for (_, text, vector) in &now {
        assert_eq!(vector, &other.vector(text));
    }
    let names = table_names(dir.path()).await;
    assert!(
        names.contains(&"documents".to_string()),
        "kept one start: {names:?}"
    );
    let versions = connect(dir.path())
        .await
        .unwrap()
        .open_table("statements_int8")
        .execute()
        .await
        .unwrap()
        .list_versions()
        .await
        .unwrap();
    assert_eq!(versions.len(), 1, "no earlier copy of a row stays");

    // The next start: the engine opens the new tables, the old ones go.
    let engine = RAGEngine::new(config(dir.path())).await.unwrap();
    assert_eq!(engine.count_documents().await.unwrap(), 2);
    drop(engine);
    let names = table_names(dir.path()).await;
    assert!(!names.contains(&"documents".to_string()), "{names:?}");
    assert!(!names.contains(&"statements".to_string()), "{names:?}");
    assert!(index_model::read(dir.path())
        .unwrap()
        .unwrap()
        .retired
        .is_empty());
}

/// A retired table an older release wrote to (a downgrade between two
/// starts) is kept; after the re-upgrade nothing drops a table no switch
/// retired, so what was written while downgraded survives. The way back to
/// the standard model uses the original names when they are free.
#[tokio::test]
async fn a_downgrade_s_writes_survive_and_the_way_back_uses_the_original_names() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    finished(dir.path(), &other).await;
    // An older release writes into the original tables (it knows no others).
    add_row_like(dir.path(), "documents", "written-while-downgraded").await;
    // Re-upgraded: the start keeps the written table, and later starts too.
    for _ in 0..3 {
        drop(RAGEngine::new(config(dir.path())).await.unwrap());
    }
    let db = connect(dir.path()).await.unwrap();
    assert!(all_ids(&db, "documents")
        .await
        .unwrap()
        .contains(&"written-while-downgraded".to_string()));
    assert!(index_model::read(dir.path())
        .unwrap()
        .unwrap()
        .retired
        .is_empty());

    // With the original names taken, the way back builds `*_fp16`; once
    // they are free, the original names.
    let back = start(dir.path(), E5Variant::Fp16).await.unwrap();
    assert_eq!(back.documents_table, "documents_fp16");
    assert!(cancel(dir.path()).await.unwrap());
    assert!(table_names(dir.path())
        .await
        .contains(&"documents".to_string()));
    db.drop_table("documents", &[]).await.unwrap();
    drop_table(&db, "statements").await.unwrap();
    let back = start(dir.path(), E5Variant::Fp16).await.unwrap();
    assert_eq!(back.documents_table, "documents");
    assert_eq!(back.statements_table, "statements");
    built(dir.path(), &other).await;
    let record = finished(dir.path(), &other).await;
    assert_eq!(record.documents_table, "documents");
    assert_eq!(record.statements_table, "statements");
}

/// A lost record never starts a new library over the switched one, and
/// nothing is dropped.
#[tokio::test]
async fn a_lost_record_drops_nothing_and_starts_no_new_library() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    finished(dir.path(), &other).await;
    drop(RAGEngine::new(config(dir.path())).await.unwrap());
    let before = rows(dir.path(), "documents_int8", Rows::Documents).await;
    std::fs::remove_file(dir.path().join(index_model::INDEX_MODEL_FILE)).unwrap();
    for _ in 0..2 {
        drop(RAGEngine::new(config(dir.path())).await.unwrap());
    }
    assert_eq!(
        index_model::read(dir.path()).unwrap(),
        None,
        "no new record over the library"
    );
    assert_eq!(
        rows(dir.path(), "documents_int8", Rows::Documents).await,
        before
    );
    assert_eq!(
        unrecorded_vector_tables(dir.path()).await.unwrap(),
        ["documents_int8", "statements_int8"]
    );
    drop_retired(dir.path()).await.unwrap();
    assert_eq!(
        rows(dir.path(), "documents_int8", Rows::Documents).await,
        before
    );
}

/// A start that stopped after the record was replaced and before the marker
/// went finishes the bookkeeping and drops nothing.
#[tokio::test]
async fn a_crash_after_the_record_is_replaced_completes_without_dropping() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    let switch = start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    let mut record = index_model::read(dir.path()).unwrap().unwrap();
    record.variant = E5Variant::Int8;
    record.documents_table = switch.documents_table.clone();
    record.statements_table = switch.statements_table.clone();
    index_model::write_record(dir.path(), &record).unwrap();
    assert_eq!(
        finish_pending(dir.path(), |_| unreachable!("nothing left to embed"))
            .await
            .unwrap(),
        None
    );
    assert_eq!(pending(dir.path()).unwrap(), None);
    let names = table_names(dir.path()).await;
    for table in [
        "documents",
        "statements",
        "documents_int8",
        "statements_int8",
    ] {
        assert!(
            names.contains(&table.to_string()),
            "{table} kept: {names:?}"
        );
    }
}

#[tokio::test]
async fn a_switch_starts_only_from_a_recorded_index_in_another_export() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        start(dir.path(), E5Variant::Int8).await.is_err(),
        "no record"
    );
    index_model::write(dir.path(), E5Variant::Int8, "first vectors").unwrap();
    assert!(
        start(dir.path(), E5Variant::Int8).await.is_err(),
        "already int8"
    );
    index_model::write(dir.path(), E5Variant::Fp16, "built before records").unwrap();
    start(dir.path(), E5Variant::Int8).await.unwrap();
    assert!(
        start(dir.path(), E5Variant::Fp16).await.is_err(),
        "another switch is in progress"
    );
    // A switch that is not built does not complete at start.
    assert_eq!(
        finish_pending(dir.path(), |_| unreachable!("not built"))
            .await
            .unwrap(),
        None
    );
}

/// A start that cannot complete the switch keeps the old index, and the
/// switch says why in plain words; a later start that can, completes it.
#[tokio::test]
async fn a_failed_completion_keeps_the_old_index_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    let failed = finish_pending(dir.path(), |_| {
        Err(crate::embeddings::SearchModelsMissing.into())
    })
    .await;
    assert!(failed.is_err());
    let record = index_model::read(dir.path()).unwrap().unwrap();
    assert_eq!(
        record.variant,
        E5Variant::Fp16,
        "the old index stays in use"
    );
    assert_eq!(record.documents_table, "documents");
    let switch = pending(dir.path())
        .unwrap()
        .expect("kept for Retry or Cancel");
    assert!(switch.built);
    let reason = switch.finish_error.expect("the reason is kept");
    assert!(
        reason.starts_with("Shodh could not finish switching the search model")
            && reason.contains("not installed any more"),
        "{reason}"
    );
    assert!(table_names(dir.path())
        .await
        .contains(&"documents_int8".to_string()));
    assert_eq!(finished(dir.path(), &other).await.variant, E5Variant::Int8);
    assert_eq!(pending(dir.path()).unwrap(), None);
}

/// No record is replaced over new tables that do not hold exactly the old
/// tables' rows.
#[tokio::test]
async fn new_tables_that_do_not_match_are_never_switched_to() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    let db = connect(dir.path()).await.unwrap();
    verify(&db, "documents", "documents_int8").await.unwrap();
    add_row_like(dir.path(), "documents_int8", "a-stray-row").await;
    let error = verify(&db, "documents", "documents_int8")
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<SwitchError>().is_some(), "{error:#}");
    let text = finish_failure_text(&error);
    assert!(text.contains("did not match"), "{text}");
}

/// Erasing a memory reaches the switch at once: a copy already made is
/// deleted (and purged), and a step after the erase never copies it back.
#[tokio::test]
async fn an_erased_memory_leaves_no_copy_in_a_switch() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    let store = memory(dir.path()).await;
    remember(&store, "p2", "meeting room").await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());

    // Paused before the memories' step (the documents are copied first).
    let asked = AtomicUsize::new(0);
    let outcome = build(
        dir.path(),
        other.clone(),
        || std::future::ready(asked.fetch_add(1, Ordering::SeqCst) >= 1),
        &|_| {},
    )
    .await
    .unwrap();
    assert_eq!(outcome, BuildOutcome::Stopped);
    store.erase(&["p2".to_string()]).await.unwrap();
    built(dir.path(), &other).await;
    let copied = ids(&rows(dir.path(), "statements_int8", Rows::Statements).await);
    assert_eq!(copied, ["p1"], "p2 erased before its step: never copied");

    // After the build (a Ready switch): the copy goes with the erase.
    store.erase(&["p1".to_string()]).await.unwrap();
    assert!(rows(dir.path(), "statements_int8", Rows::Statements)
        .await
        .is_empty());
    store.purge_history().await.unwrap();
    let versions = connect(dir.path())
        .await
        .unwrap()
        .open_table("statements_int8")
        .execute()
        .await
        .unwrap()
        .list_versions()
        .await
        .unwrap();
    assert_eq!(versions.len(), 1, "the purge reached the switch's table");
}

/// Memories erased between the build and the switch, a whole build step of
/// them, leave nothing in the new table's full-text index: it covers exactly
/// the rows left.
#[tokio::test]
async fn the_switched_memory_s_word_index_covers_only_the_rows_left() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    let store = memory(dir.path()).await;
    let erased: Vec<String> = (0..STEP_ROWS).map(|i| format!("e{i:02}")).collect();
    for (i, id) in erased.iter().enumerate() {
        remember(&store, id, &format!("topic number {i} zanzibarword")).await;
    }
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    store.erase(&erased).await.unwrap();
    drop(store);
    finished(dir.path(), &other).await;
    let table = connect(dir.path())
        .await
        .unwrap()
        .open_table("statements_int8")
        .execute()
        .await
        .unwrap();
    let rows_left = table.count_rows(None).await.unwrap();
    assert_eq!(rows_left, 1, "only p1 is left");
    let index = table
        .list_indices()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.columns == ["terms"])
        .expect("the terms index");
    let stats = table
        .index_stats(&index.name)
        .await
        .unwrap()
        .expect("index statistics");
    assert_eq!(
        stats.num_indexed_rows, rows_left,
        "the index was rebuilt over the rows left"
    );
    assert_eq!(stats.num_unindexed_rows, 0);
}

/// Old versions of the table being copied are pruned (and rows deleted)
/// while the build runs, as compaction and the memory's purge do. Each step
/// reads the latest version, so the build finishes with exactly the rows in
/// use.
#[tokio::test]
async fn pruning_the_source_during_a_build_never_leaves_a_wrong_table() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    {
        let mut engine = engine(dir.path()).await;
        for i in 0..70 {
            add(
                &mut engine,
                &format!("note://{i}"),
                &format!("Note {i} says the meeting moved to room {i} on Friday afternoon."),
            )
            .await;
        }
    }
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    let db = connect(dir.path()).await.unwrap();
    // The documents table is copied in this many steps; the steps after them
    // copy the memory, while notes are still deleted from the documents.
    let document_steps = rows(dir.path(), "documents", Rows::Documents)
        .await
        .len()
        .div_ceil(STEP_ROWS);
    let step = AtomicUsize::new(0);
    let first = build(
        dir.path(),
        other.clone(),
        || {
            let db = db.clone();
            let n = step.fetch_add(1, Ordering::SeqCst);
            async move {
                let documents = db.open_table("documents").execute().await.unwrap();
                documents
                    .delete(&format!("source = 'note://{n}'"))
                    .await
                    .unwrap();
                prune_old_versions(&db, "documents").await.unwrap();
                false
            }
        },
        &|_| {},
    )
    .await
    .expect("each step reads the latest version");
    assert_eq!(first, BuildOutcome::Built);
    let deletions = step.load(Ordering::SeqCst);
    assert!(
        deletions > document_steps,
        "notes are also deleted after the documents were copied"
    );
    let live = rows(dir.path(), "documents", Rows::Documents).await;
    let copied = rows(dir.path(), "documents_int8", Rows::Documents).await;
    // Every row in use is copied with the new export's vector. Rows deleted
    // while the documents were copied are gone from the copy; only notes
    // deleted after that (while the memory was copied) are still there, for
    // the completion to bring over.
    let live_ids: HashSet<String> = ids(&live).into_iter().collect();
    for (_, text, vector) in &copied {
        assert_eq!(vector, &other.vector(text));
    }
    for id in ids(&live) {
        assert!(ids(&copied).contains(&id), "row {id} in use is copied");
    }
    let late: Vec<String> = (document_steps..deletions)
        .map(|n| format!("Note {n} says"))
        .collect();
    for (id, text, _) in &copied {
        if !live_ids.contains(id) {
            assert!(
                late.iter().any(|note| text.contains(note.as_str())),
                "only a note deleted after the documents were copied stays: {text}"
            );
        }
    }
    let finished = finished(dir.path(), &other).await;
    assert_eq!(finished.documents_table, "documents_int8");
    let switched = rows(dir.path(), "documents_int8", Rows::Documents).await;
    assert_eq!(ids(&switched), ids(&live), "exactly the rows in use");
    verify(&db, "documents", "documents_int8").await.unwrap();
}

/// Many changes since the build are not embedded at start: the switch waits
/// and the build catches up in the background.
#[tokio::test]
async fn many_changes_since_the_build_are_caught_up_in_the_background_not_at_start() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    {
        let mut engine = engine(dir.path()).await;
        for i in 0..=MAX_CHANGES_AT_START {
            add(
                &mut engine,
                &format!("note://late-{i}"),
                &format!("Late note {i} about the garden shed."),
            )
            .await;
        }
    }
    let embedded = other.embedded.load(Ordering::SeqCst);
    let loader = other.clone();
    assert_eq!(
        finish_pending(dir.path(), move |_| Ok(loader as Arc<dyn EmbeddingModel>))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        other.embedded.load(Ordering::SeqCst),
        embedded,
        "nothing embedded at start"
    );
    assert!(
        !pending(dir.path()).unwrap().unwrap().built,
        "the build goes on"
    );
    assert_eq!(
        index_model::read(dir.path()).unwrap().unwrap().variant,
        E5Variant::Fp16
    );
    built(dir.path(), &other).await;
    assert_eq!(finished(dir.path(), &other).await.variant, E5Variant::Int8);
}

/// A reset discards a switch: no copy of the library stays on disk.
#[tokio::test]
async fn a_cancelled_switch_and_a_clear_leave_no_copy_of_the_library() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).await.unwrap();
    let other = Arc::new(OtherExport::default());
    built(dir.path(), &other).await;
    assert!(cancel(dir.path()).await.unwrap());
    engine(dir.path()).await.clear_all_data().await.unwrap();
    let names = table_names(dir.path()).await;
    assert!(
        !names.iter().any(|n| n.ends_with("_int8")),
        "no switch table left: {names:?}"
    );
}

/// Every column of the real chunk and statement tables can be compared, and
/// a chunk whose metadata cannot be read stops the switch instead of being
/// embedded from a guess.
#[tokio::test]
async fn the_real_tables_columns_are_compared_and_bad_metadata_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    let db = connect(dir.path()).await.unwrap();
    for name in ["documents", "statements"] {
        let batches: Vec<RecordBatch> = db
            .open_table(name)
            .execute()
            .await
            .unwrap()
            .query()
            .execute()
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        for batch in &batches {
            for row in 0..batch.num_rows() {
                fingerprint(batch, row).unwrap();
            }
        }
    }
    assert!(crate::rag_engine::embedded_text("text", "title", "{not json").is_err());
    assert!(crate::rag_engine::embedded_text("text", "title", r#"{"n":1}"#).is_err());
    assert_eq!(
        crate::rag_engine::embedded_text("text", "title", r#"{"context":"C. "}"#).unwrap(),
        "C. text"
    );
}

#[test]
fn a_fingerprint_covers_every_column_but_the_vector() {
    use arrow_schema::{Field, Schema};
    let item = || Arc::new(Field::new("item", DataType::Float32, true));
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("n", DataType::Int64, true),
        Field::new("vector", DataType::FixedSizeList(item(), 2), true),
    ]));
    let batch = |n: Option<i64>, v: [f32; 2]| {
        RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a"])) as Arc<dyn Array>,
                Arc::new(Int64Array::from(vec![n])),
                Arc::new(
                    FixedSizeListArray::try_new(
                        item(),
                        2,
                        Arc::new(Float32Array::from(v.to_vec())),
                        None,
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap()
    };
    let print = |b: RecordBatch| fingerprint(&b, 0).unwrap();
    assert_eq!(
        print(batch(Some(1), [0.0, 1.0])),
        print(batch(Some(1), [1.0, 0.0]))
    );
    assert_ne!(
        print(batch(Some(1), [0.0, 1.0])),
        print(batch(Some(2), [0.0, 1.0]))
    );
    assert_ne!(
        print(batch(Some(0), [0.0, 1.0])),
        print(batch(None, [0.0, 1.0]))
    );
    assert_eq!(
        id_list(&["a'b".to_string(), "c".to_string()]),
        "id IN ('a''b', 'c')"
    );
}

/// The real model end to end: a small library is searched with fp16, moved
/// to int8 and back to fp16 with the switch, every search finds the right
/// note throughout, and the vectors after the way back are the original fp16
/// ones.
#[tokio::test]
#[cfg_attr(
    not(shodh_test_models),
    ignore = "requires SHODH_TEST_MODELS containing multilingual-e5-base/"
)]
async fn the_real_model_switches_fp16_to_int8_and_back_and_search_stays_right() {
    let models =
        std::path::PathBuf::from(std::env::var_os("SHODH_TEST_MODELS").expect("SHODH_TEST_MODELS"));
    let dir = tempfile::tempdir().unwrap();
    let real = |dir: &Path| {
        let mut config = RAGConfig::default();
        config.data_dir = dir.to_path_buf();
        config.embedding.model_dir = models.clone();
        config.embedding.use_e5 = true;
        config.embedding.dimension = crate::embeddings::e5::E5_DIMENSION;
        config.embedding.compute = crate::compute::provider::ComputePreference::Cpu;
        config
    };
    let notes = [
        (
            "note://tide",
            "Tide pools hold anemones and crabs at low tide.",
        ),
        (
            "note://lease",
            "The lease notice period is sixty days, given in writing to the landlord.",
        ),
        (
            "note://kettle",
            "The kettle is descaled with citric acid once a month.",
        ),
    ];
    let questions = [
        (
            "How long is the notice period of the lease?",
            "note://lease",
        ),
        ("What lives in tide pools?", "note://tide"),
        ("How do I descale the kettle?", "note://kettle"),
    ];
    async fn searches_right(engine: &RAGEngine, questions: &[(&str, &str)]) {
        for (question, source) in questions {
            let results = engine.search(question, 3).await.unwrap();
            assert_eq!(
                results
                    .first()
                    .and_then(|r| r.metadata.get("source"))
                    .map(String::as_str),
                Some(*source),
                "{question}"
            );
        }
    }
    {
        let mut engine = RAGEngine::new(real(dir.path())).await.unwrap();
        for (source, text) in notes {
            add(&mut engine, source, text).await;
        }
        searches_right(&engine, &questions).await;
    }
    assert_eq!(
        index_model::read(dir.path()).unwrap().unwrap().variant,
        E5Variant::Fp16,
        "a new library is fp16 by default"
    );
    let original = rows(dir.path(), "documents", Rows::Documents).await;
    for to in [E5Variant::Int8, E5Variant::Fp16] {
        start(dir.path(), to).await.unwrap();
        let model = crate::rag_engine::switch_model(&models, to).unwrap();
        assert_eq!(
            build(dir.path(), model, never(), &|_| {}).await.unwrap(),
            BuildOutcome::Built
        );
        let loader_models = models.clone();
        let switched = finish_pending(dir.path(), move |v| {
            crate::rag_engine::switch_model(&loader_models, v)
        })
        .await
        .unwrap()
        .expect("switched");
        assert_eq!(switched.variant, to);
        // The next start opens it (and drops the tables before it).
        let engine = RAGEngine::new(real(dir.path())).await.unwrap();
        searches_right(&engine, &questions).await;
        assert_eq!(
            engine.embeddings().unwrap().e5_variant(),
            Some(to),
            "queries use the library's export"
        );
    }
    let record = index_model::read(dir.path()).unwrap().unwrap();
    assert_eq!(
        record.documents_table, "documents",
        "the original name again"
    );
    let back = rows(dir.path(), "documents", Rows::Documents).await;
    assert_eq!(ids(&back), ids(&original));
    for ((id, text, before), (_, after_text, after)) in original.iter().zip(&back) {
        assert_eq!(text, after_text);
        let cosine: f32 = before.iter().zip(after).map(|(a, b)| a * b).sum();
        assert!(
            cosine >= 0.9999,
            "{id}: the fp16 vector moved, cos={cosine}"
        );
    }
}
