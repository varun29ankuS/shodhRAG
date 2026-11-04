//! Tests of [`super`]: a library with chunks and a memory, switched to another
//! "export" (a deterministic embedder whose vectors differ from the index's).

use super::*;
use crate::statements::testing::{
    ontology, preference, t0, FixedEmbedder, TestClock, WordEmbedder, DIM,
};
use crate::statements::{DynamicsStore, PutIntent, Scope, StatementStore};
use crate::types::{Citation, DocumentFormat};
use crate::{RAGConfig, RAGEngine, SearchModels};
use std::collections::HashMap as Map;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Another "export": its own vocabulary and every vector negated, so its
/// vectors are told from the index's. Counts the passages it embeds.
#[derive(Default)]
struct OtherExport {
    words: WordEmbedder,
    embedded: AtomicUsize,
}

impl OtherExport {
    /// The vector it gives `text`, without counting it.
    fn vector(&self, text: &str) -> Vec<f32> {
        self.words
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

async fn engine(dir: &Path) -> RAGEngine {
    let mut engine = RAGEngine::new(config(dir)).await.unwrap();
    engine
        .attach_search_models(SearchModels::from_embedder(Arc::new(
            WordEmbedder::default(),
        )))
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
    let store = StatementStore::open(
        &dir.join(LANCE_DIR),
        DIM,
        ontology(),
        Arc::new(DynamicsStore::open(&dir.join("shodh.db"), None).unwrap()),
        Arc::new(FixedEmbedder(Arc::new(WordEmbedder::default()))),
        TestClock::at(t0()),
    )
    .await
    .unwrap();
    store
        .put(
            preference("p1", "report format", "PDF", t0()),
            Scope::Global,
            PutIntent::Auto,
        )
        .await
        .unwrap();
    drop(store);
    drop(RAGEngine::new(config(dir)).await.unwrap());
    assert_eq!(
        index_model::read(dir).unwrap().map(|r| r.variant),
        Some(E5Variant::Fp16)
    );
}

fn never() -> impl FnMut() -> std::future::Ready<bool> {
    || std::future::ready(false)
}

#[tokio::test]
async fn a_build_copies_every_row_with_a_vector_of_the_new_export() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    let switch = start(dir.path(), E5Variant::Int8).unwrap();
    assert_eq!(switch.documents_table, "documents_int8");
    assert_eq!(switch.statements_table, "statements_int8");
    assert_eq!(
        start(dir.path(), E5Variant::Int8).unwrap(),
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
            assert_eq!(text, new_text, "the same content");
            assert_eq!(vector, &other.vector(text), "the new export's vector");
            assert_ne!(vector, old_vector, "not the old vector");
        }
    }
    let documents = rows(dir.path(), "documents", Rows::Documents).await;
    for (_, text, _) in &documents {
        assert!(
            text.contains("note://")
                && (text.ends_with("low tide.") || text.ends_with("in writing.")),
            "a chunk is embedded from its stored context and its text: {text:?}"
        );
    }

    // Built again: every row is unchanged, so nothing is embedded.
    let embedded = other.embedded.load(Ordering::SeqCst);
    assert_eq!(
        build(dir.path(), other.clone(), never(), &|_| {})
            .await
            .unwrap(),
        BuildOutcome::Built
    );
    assert_eq!(other.embedded.load(Ordering::SeqCst), embedded);
    // The index in use is untouched.
    let record = index_model::read(dir.path()).unwrap().unwrap();
    assert_eq!(record.variant, E5Variant::Fp16);
    assert_eq!(record.documents_table, "documents");
}

#[tokio::test]
async fn a_stopped_build_keeps_its_rows_and_a_cancel_drops_them() {
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
    start(dir.path(), E5Variant::Int8).unwrap();
    let other = Arc::new(OtherExport::default());
    let asked = AtomicUsize::new(0);
    // Stops when asked a second time: one step of rows is copied.
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
    assert_eq!(
        build(dir.path(), other.clone(), never(), &|_| {})
            .await
            .unwrap(),
        BuildOutcome::Built
    );
    let total = rows(dir.path(), "documents", Rows::Documents).await.len()
        + rows(dir.path(), "statements", Rows::Statements).await.len();
    assert_eq!(other.embedded.load(Ordering::SeqCst), total);

    assert!(cancel(dir.path()).await.unwrap());
    assert_eq!(pending(dir.path()).unwrap(), None);
    let names = table_names(dir.path()).await;
    assert!(!names.contains(&"documents_int8".to_string()), "{names:?}");
    assert!(!names.contains(&"statements_int8".to_string()), "{names:?}");
    assert!(names.contains(&"documents".to_string()));
    assert!(!cancel(dir.path()).await.unwrap(), "nothing left to cancel");
}

#[tokio::test]
async fn the_switch_completes_at_start_with_what_changed_since_the_build() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    start(dir.path(), E5Variant::Int8).unwrap();
    let other = Arc::new(OtherExport::default());
    build(dir.path(), other.clone(), never(), &|_| {})
        .await
        .unwrap();
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
    let loader = other.clone();
    let switched = finish_pending(dir.path(), move |variant| {
        assert_eq!(variant, E5Variant::Int8);
        Ok(loader as Arc<dyn EmbeddingModel>)
    })
    .await
    .unwrap()
    .expect("switched");
    assert_eq!(switched.variant, E5Variant::Int8);
    assert_eq!(switched.documents_table, "documents_int8");
    assert_eq!(switched.statements_table, "statements_int8");
    assert_eq!(switched.reason, "switched from fp16");
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
        !names.contains(&"documents".to_string()),
        "old table dropped: {names:?}"
    );
    assert!(
        !names.contains(&"statements".to_string()),
        "old table dropped: {names:?}"
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

    // The engine opens the new tables.
    let engine = RAGEngine::new(config(dir.path())).await.unwrap();
    assert_eq!(engine.count_documents().await.unwrap(), 2);
    drop(engine);
    // Nothing is left to finish.
    assert_eq!(
        finish_pending(dir.path(), |_| unreachable!("no switch to finish"))
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_switch_starts_only_from_a_recorded_index_in_another_export() {
    let dir = tempfile::tempdir().unwrap();
    assert!(start(dir.path(), E5Variant::Int8).is_err(), "no record");
    index_model::write(dir.path(), E5Variant::Int8, "first vectors").unwrap();
    assert!(start(dir.path(), E5Variant::Int8).is_err(), "already int8");
    index_model::write(dir.path(), E5Variant::Fp16, "built before records").unwrap();
    start(dir.path(), E5Variant::Int8).unwrap();
    assert!(
        start(dir.path(), E5Variant::Fp16).is_err(),
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

#[tokio::test]
async fn tidy_drops_only_the_vector_tables_nothing_names() {
    let dir = tempfile::tempdir().unwrap();
    library(dir.path()).await;
    let db = connect(dir.path()).await.unwrap();
    let schema = db
        .open_table("documents")
        .execute()
        .await
        .unwrap()
        .schema()
        .await
        .unwrap();
    for name in ["documents_int8", "unrelated"] {
        db.create_empty_table(name, schema.clone())
            .execute()
            .await
            .unwrap();
    }
    // In the original tables, without a switch, nothing is looked at.
    tidy(dir.path()).await.unwrap();
    assert!(table_names(dir.path())
        .await
        .contains(&"documents_int8".to_string()));
    // A record naming other tables: the original chunk table is a leftover.
    let mut record = index_model::read(dir.path()).unwrap().unwrap();
    record.documents_table = "documents_int8".to_string();
    index_model::write_record(dir.path(), &record).unwrap();
    tidy(dir.path()).await.unwrap();
    let names = table_names(dir.path()).await;
    assert!(!names.contains(&"documents".to_string()), "{names:?}");
    assert!(names.contains(&"documents_int8".to_string()));
    assert!(
        names.contains(&"unrelated".to_string()),
        "not a vector table"
    );
    assert!(
        names.contains(&"statements".to_string()),
        "named by the record"
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
