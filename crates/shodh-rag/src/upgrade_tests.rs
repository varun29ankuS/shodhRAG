//! A library written by Shodh 0.0.10 opens in this build with everything in it.
//!
//! The library is written by 0.0.10's own code: CI's "Upgrade from 0.0.10"
//! job builds `upgrade/v0_0_10/write_library.rs` against the `v0.0.10` tag
//! (lancedb 0.26, Lance 2.0, Tantivy 0.22) and runs it, then runs this test
//! with `SHODH_UPGRADE_FIXTURE` naming its output. The writer read the library
//! back through 0.0.10 into `manifest.json`; this test opens a copy of it the
//! way the app does and compares, at each step of what 0.0.11 does to an old
//! library on its own:
//!
//! 1. opened (`RAGEngine::new`, the memory store as the app opens it);
//! 2. after the store maintenance pass (compaction, pruning, the cosine
//!    index check over 0.0.10's L2 index), which rewrites the table's files
//!    in Lance 14's format: the step that makes the upgrade one-way;
//! 3. after the startup upgrade (files that may hold secrets removed, stored
//!    chunks redacted, documents of the older chunker read again or raised
//!    in place: `folder_sync::upgrade_unowned`);
//! 4. after another pass and a reopen.
//!
//! Each step checks every column of every chunk (the vector by the SHA-256
//! of its bits) against what it must be, the keyword index, the vectors'
//! nearest neighbours, the hybrid search and the memories. A lost row, a
//! changed byte of text or metadata, a changed bit of a vector, a memory or a
//! search result gone fails it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch, StringArray, UInt32Array,
};
use futures::TryStreamExt;
use lancedb::query::ExecutableQuery;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::*;
use crate::audit::AuditLog;
use crate::embeddings::e5::E5Variant;
use crate::embeddings::index_model;
use crate::folder_sync::{sync_folder, upgrade_unowned, ManifestStore};
use crate::statements::testing::FixedEmbedder;
use crate::statements::{DynamicsStore, Scope, StatementStore, SystemClock};
use crate::user_memory::{Actor, ListRequest, MemoryService, RecallMode, RecallRequest};

#[path = "../upgrade/v0_0_10/library.rs"]
mod library;

use library::{ChunkRow, Kind, Manifest};

struct FixtureEmbedder;

impl EmbeddingModel for FixtureEmbedder {
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(library::embed(text))
    }
    fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        Ok(library::embed(text))
    }
    fn dimension(&self) -> usize {
        library::DIMENSION
    }
}

fn config(data_dir: &Path) -> RAGConfig {
    let mut config = RAGConfig::default();
    config.data_dir = data_dir.to_path_buf();
    config.embedding.model_dir = data_dir.join("models");
    config.embedding.use_e5 = false;
    config.embedding.dimension = library::DIMENSION;
    config
}

async fn open(data_dir: &Path) -> RAGEngine {
    let mut engine = RAGEngine::new(config(data_dir)).await.unwrap();
    engine
        .attach_search_models(SearchModels::from_embedder(Arc::new(FixtureEmbedder)))
        .unwrap();
    engine
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A chunk as stored now, with its vector.
struct Stored {
    row: ChunkRow,
    vector: Vec<f32>,
}

fn strings<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .unwrap_or_else(|| panic!("no string column {name}"))
}

/// Every row of the library's documents table, every column, by id.
async fn stored(data_dir: &Path) -> BTreeMap<String, Stored> {
    let (documents, _) = index_model::tables(data_dir).unwrap();
    let lance = data_dir.join(crate::embeddings::model_switch::LANCE_DIR);
    let db = lancedb::connect(&lance.to_string_lossy())
        .execute()
        .await
        .unwrap();
    let table = db.open_table(&documents).execute().await.unwrap();
    let batches: Vec<RecordBatch> = table
        .query()
        .execute()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut rows = BTreeMap::new();
    for batch in &batches {
        let chunk_index = batch
            .column_by_name("chunk_index")
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .expect("chunk_index");
        let created_at = batch
            .column_by_name("created_at")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .expect("created_at");
        let vectors = batch
            .column_by_name("vector")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
            .expect("vector");
        for i in 0..batch.num_rows() {
            let vector = vectors.value(i);
            let vector = vector
                .as_any()
                .downcast_ref::<Float32Array>()
                .expect("f32 vector items")
                .values()
                .to_vec();
            let row = ChunkRow {
                id: strings(batch, "id").value(i).to_string(),
                doc_id: strings(batch, "doc_id").value(i).to_string(),
                chunk_index: chunk_index.value(i),
                text: strings(batch, "text").value(i).to_string(),
                title: strings(batch, "title").value(i).to_string(),
                source: strings(batch, "source").value(i).to_string(),
                heading: strings(batch, "heading").value(i).to_string(),
                vector_sha256: library::vector_digest(&vector, |b| Sha256::digest(b).to_vec()),
                space_id: strings(batch, "space_id").value(i).to_string(),
                metadata_json: strings(batch, "metadata_json").value(i).to_string(),
                citation_json: strings(batch, "citation_json").value(i).to_string(),
                created_at: created_at.value(i),
            };
            let id = row.id.clone();
            assert!(
                rows.insert(id.clone(), Stored { row, vector }).is_none(),
                "chunk {id} is stored twice"
            );
        }
    }
    rows
}

fn rows_only(stored: &BTreeMap<String, Stored>) -> BTreeMap<String, ChunkRow> {
    stored
        .iter()
        .map(|(id, s)| (id.clone(), s.row.clone()))
        .collect()
}

fn metadata(row: &ChunkRow) -> Map<String, Value> {
    match serde_json::from_str(&row.metadata_json) {
        Ok(Value::Object(map)) => map,
        other => panic!("chunk {}: metadata is not an object: {other:?}", row.id),
    }
}

/// `before` with its metadata as the startup upgrade leaves a chunk it keeps:
/// the chunker version raised to this build's (and, for chunks kept from an
/// older chunker on purpose, the version their text is from), the redactor
/// recorded. Everything else, text, vector and id included, is unchanged.
fn kept(before: &ChunkRow, kept_from: Option<&str>) -> Map<String, Value> {
    let mut expected = metadata(before);
    let older = expected
        .insert(
            CHUNKER_VERSION_KEY.to_string(),
            Value::String(CHUNKER_VERSION.to_string()),
        )
        .unwrap_or_else(|| panic!("chunk {} has no chunker version", before.id));
    assert_eq!(older, Value::String("2".to_string()), "0.0.10's chunker");
    if let Some(from) = kept_from {
        expected.insert(
            CHUNKS_KEPT_FROM_KEY.to_string(),
            Value::String(from.to_string()),
        );
    }
    expected
}

/// `actual` equals `expected` but for the redactor entry, which redaction
/// may add to any chunk it looked at.
fn assert_metadata(actual: &ChunkRow, mut expected: Map<String, Value>) {
    let mut actual_map = metadata(actual);
    actual_map.remove(REDACTOR_KEY);
    expected.remove(REDACTOR_KEY);
    assert_eq!(actual_map, expected, "chunk {} metadata", actual.id);
}

/// The source of each file, as this build normalises its path; it must be
/// the one 0.0.10 stored.
fn sources(manifest: &Manifest, files_dir: &Path) -> BTreeMap<String, String> {
    manifest
        .files
        .iter()
        .map(|f| {
            let source = normalize_source_path(&files_dir.join(&f.path));
            assert_eq!(source, f.source, "{} is normalised as 0.0.10 did", f.path);
            (f.path.clone(), source)
        })
        .collect()
}

/// Every chunk is found by its own vector, first, and the keyword index finds
/// exactly the chunks that hold each file's word.
async fn assert_searchable(
    engine: &RAGEngine,
    stored: &BTreeMap<String, Stored>,
    tokens: &[(String, Option<String>)],
) {
    assert_eq!(
        engine.text_search.count().unwrap(),
        stored.len(),
        "every chunk is in the keyword index"
    );
    // Every chunk of the small files, and every 20th of the bulk volumes: a
    // search scans the whole table (the cosine index starts at 20,000 rows).
    let sample = stored
        .iter()
        .enumerate()
        .filter(|(i, (_, s))| i % 20 == 0 || !s.row.source.contains("/archive/"))
        .map(|(_, entry)| entry);
    for (id, chunk) in sample {
        let hits = engine
            .store
            .vector_search(&chunk.vector, 3, None)
            .await
            .unwrap();
        assert_eq!(
            hits.first().map(|h| h.id.as_str()),
            Some(id.as_str()),
            "chunk {id} is its own nearest neighbour"
        );
    }
    for (token, source) in tokens {
        let holding: BTreeSet<&str> = stored
            .values()
            .filter(|s| {
                s.row
                    .text
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| w.eq_ignore_ascii_case(token))
            })
            .map(|s| s.row.id.as_str())
            .collect();
        let found: BTreeSet<String> = engine
            .text_search
            .search(token, stored.len().max(1))
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let found: BTreeSet<&str> = found.iter().map(String::as_str).collect();
        let results = engine.search(token, 5).await.unwrap();
        let returned: Vec<&str> = results.iter().map(|r| r.source.as_str()).collect();
        match source {
            Some(source) => {
                // Every chunk holding the word is found, and only chunks of its
                // file (whose title the keyword index also reads).
                let of_file: BTreeSet<&str> = stored
                    .values()
                    .filter(|s| &s.row.source == source)
                    .map(|s| s.row.id.as_str())
                    .collect();
                assert!(!holding.is_empty(), "{token} is stored");
                assert!(
                    holding.is_subset(&found),
                    "the keyword index for {token} misses {:?}",
                    holding.difference(&found).collect::<Vec<_>>()
                );
                assert!(
                    found.is_subset(&of_file),
                    "the keyword index for {token} finds other files' chunks {:?}",
                    found.difference(&of_file).collect::<Vec<_>>()
                );
                assert!(
                    returned.contains(&source.as_str()),
                    "a search for {token} returns {source}: {returned:?}"
                );
            }
            None => {
                assert!(holding.is_empty(), "{token} was removed: {holding:?}");
                assert!(found.is_empty(), "{token} was removed: {found:?}");
            }
        }
    }
}

/// The memories as the app opens them, equal to what 0.0.10 listed, each
/// recalled by its own text.
async fn assert_memories(data_dir: &Path, manifest: &Manifest) {
    let database = data_dir.join("shodh.db");
    let (_, statements) = index_model::tables(data_dir).unwrap();
    let store = StatementStore::open_table(
        &data_dir.join(crate::embeddings::model_switch::LANCE_DIR),
        &statements,
        library::DIMENSION,
        Arc::new(shodh_ontology::Ontology::builtin_with_packs(&["research"]).unwrap()),
        Arc::new(DynamicsStore::open(&database, None).unwrap()),
        Arc::new(FixedEmbedder(Arc::new(FixtureEmbedder))),
        Arc::new(SystemClock),
    )
    .await
    .unwrap();
    let audit = Arc::new(AuditLog::open(&database, None).unwrap());
    let service = MemoryService::new(Arc::new(store), Some(audit), env!("CARGO_PKG_VERSION"));
    // What the app runs on opening the memory.
    service.maintain().await.unwrap();
    let mut listed = service.list(&ListRequest::default()).await.unwrap();
    listed.sort_by(|a, b| a.id.cmp(&b.id));
    let listed: Vec<(String, String, String, String, String, String)> = listed
        .into_iter()
        .map(|m| {
            let scope = match &m.scope {
                Scope::Global => "global".to_string(),
                Scope::Workspace(id) => id.clone(),
            };
            (
                m.id,
                m.class,
                m.text,
                scope,
                m.source,
                m.created_at.to_rfc3339(),
            )
        })
        .collect();
    let expected: Vec<(String, String, String, String, String, String)> = manifest
        .memories
        .iter()
        .map(|m| {
            (
                m.id.clone(),
                m.class.clone(),
                m.text.clone(),
                m.scope.clone(),
                m.source.clone(),
                m.created_at.clone(),
            )
        })
        .collect();
    assert_eq!(listed, expected, "the memories 0.0.10 saved");
    for memory in &manifest.memories {
        let scope = if memory.scope == "global" {
            Scope::Global
        } else {
            Scope::Workspace(memory.scope.clone())
        };
        let mut request = RecallRequest::new(&memory.text, scope, 3, RecallMode::Inspect);
        request.min_similarity = 0.0;
        let recalled = service.recall(&request, &Actor::ui()).await.unwrap();
        assert_eq!(
            recalled.first().map(|r| r.memory.id.as_str()),
            Some(memory.id.as_str()),
            "{} is recalled",
            memory.token
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the library 0.0.10 wrote (SHODH_UPGRADE_FIXTURE): CI's Upgrade from 0.0.10 job"]
async fn a_library_written_by_0_0_10_keeps_its_documents_searches_and_memories() {
    let fixture = PathBuf::from(std::env::var(library::FIXTURE_ENV).unwrap_or_else(|_| {
        panic!(
            "{} must name the folder upgrade/v0_0_10/write_library.rs wrote with 0.0.10",
            library::FIXTURE_ENV
        )
    }));
    // Made absolute as the writer made it, so the files' paths are spelled as
    // 0.0.10 stored them.
    let fixture = std::path::absolute(&fixture).unwrap();
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(fixture.join(library::MANIFEST_FILE)).expect("the fixture's manifest"),
    )
    .unwrap();
    let files_dir = fixture.join(library::FILES_DIR);
    let files = library::files();

    // The fixture is what it claims: written by 0.0.10, with 0.0.10's vector
    // index, a keyword index of every chunk, and searches that found each file.
    assert_eq!(manifest.written_by, "0.0.10");
    assert_eq!(
        manifest.files.iter().map(|f| &f.path).collect::<Vec<_>>(),
        files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert!(manifest.chunks.len() >= 1_000, "{}", manifest.chunks.len());
    assert!(
        manifest
            .indices
            .iter()
            .any(|(_, kind, columns)| kind.contains("IvfPq") && columns == &["vector"]),
        "0.0.10's L2 IVF-PQ index: {:?}",
        manifest.indices
    );
    // Rows the index covers, and the late file's, which it does not.
    let (indexed, unindexed) = manifest.vector_index_rows;
    assert_eq!(indexed + unindexed, manifest.chunks.len());
    assert!(
        indexed > 0 && unindexed > 0,
        "{:?}",
        manifest.vector_index_rows
    );
    assert_eq!(manifest.keyword_documents, manifest.chunks.len());
    assert_eq!(manifest.memories.len(), library::memories().len());
    let source_of = sources(&manifest, &files_dir);
    let mut tokens: Vec<(String, Option<String>)> = Vec::new();
    for f in &files {
        let live = !f.removed && f.kind() != Kind::Secret;
        tokens.push((f.token.clone(), live.then(|| source_of[&f.path].clone())));
    }
    tokens.push((
        library::NOTE_TOKEN.to_string(),
        Some(library::NOTE_SOURCE.to_string()),
    ));
    for search in &manifest.searches {
        let expected = tokens
            .iter()
            .find(|(token, _)| token == &search.query)
            .unwrap_or_else(|| panic!("a search for an unknown word {}", search.query));
        if let Some(source) = &expected.1 {
            assert!(
                search.sources.contains(source),
                "0.0.10 found {source} for {}: {:?}",
                search.query,
                search.sources
            );
        }
    }
    // The secret file is in 0.0.10's library; 0.0.11 removes it.
    let secret = files
        .iter()
        .find(|f| f.kind() == Kind::Secret)
        .expect("a secret file");
    let secret_source = source_of[&secret.path].clone();
    assert!(manifest.chunks.values().any(|c| c.source == secret_source));

    // A copy: the upgrade is one-way, and the fixture stays as 0.0.10 wrote it.
    let work = tempfile::tempdir().unwrap();
    let data_dir = work.path().join("data");
    copy_dir(&fixture.join(library::DATA_DIR), &data_dir);

    // 1. Opened.
    let engine = open(&data_dir).await;
    let record = index_model::read(&data_dir)
        .unwrap()
        .expect("an index of 0.0.10 is recorded at open");
    assert_eq!(record.variant, E5Variant::Fp16);
    assert_eq!(record.reason, "built before records");
    assert_eq!(
        (
            record.documents_table.as_str(),
            record.statements_table.as_str()
        ),
        (
            index_model::DOCUMENTS_TABLE,
            crate::statements::STATEMENTS_TABLE
        )
    );
    let now = stored(&data_dir).await;
    assert_eq!(rows_only(&now), manifest.chunks, "opened");
    assert_searchable(
        &engine,
        &now,
        &tokens_live_before_upgrade(&tokens, &secret.token, &secret_source),
    )
    .await;
    assert_memories(&data_dir, &manifest).await;

    // 2. The maintenance pass rewrites the files in this build's format. A
    // failed compaction is only logged and leaves the table marked changed.
    engine.store_maintenance().run().await.unwrap();
    assert!(
        !engine.store_maintenance().changed(),
        "the maintenance pass compacted the table"
    );
    let now = stored(&data_dir).await;
    assert_eq!(
        rows_only(&now),
        manifest.chunks,
        "after the maintenance pass"
    );
    assert_searchable(
        &engine,
        &now,
        &tokens_live_before_upgrade(&tokens, &secret.token, &secret_source),
    )
    .await;

    // 3. The startup upgrade, as the app runs it.
    let mut engine = engine;
    let purge = engine.store_maintenance().purge_lock().await;
    let removed = engine.purge_secret_files(&purge).await.unwrap();
    let secret_chunks = manifest
        .chunks
        .values()
        .filter(|c| c.source == secret_source)
        .count();
    assert_eq!(
        removed, secret_chunks,
        "the file that may hold secrets leaves"
    );
    assert_eq!(
        engine.redact_stored_chunks(&purge).await.unwrap(),
        0,
        "no stored text holds a secret"
    );
    drop(purge);
    let rag = tokio::sync::RwLock::new(engine);
    // The Library folder source, synced with the manifest 0.0.10's folder
    // sync left: its files of window chunks are read again, its Markdown
    // only gets this build's chunker version.
    let manifests = ManifestStore::in_dir(data_dir.join(library::FOLDER_MANIFESTS));
    let synced = sync_folder(
        &files_dir.join(library::FOLDER).to_string_lossy(),
        library::FOLDER_SOURCE,
        &manifests,
        &rag,
        || false,
    )
    .await
    .unwrap();
    let window_in_folder = files
        .iter()
        .filter(|f| f.in_folder() && f.kind() == Kind::Window)
        .count();
    assert!(window_in_folder > 0);
    assert_eq!(synced.indexed, window_in_folder, "{synced:?}");
    assert_eq!(synced.rechunked, window_in_folder, "{synced:?}");
    assert_eq!(synced.removed, 0, "{synced:?}");
    assert_eq!(synced.scans_kept, 0, "{synced:?}");
    assert!(synced.failures.is_empty(), "{synced:?}");
    assert!(synced.unreadable.is_empty(), "{synced:?}");
    // Everything else: files indexed one by one and the note.
    let report = upgrade_unowned(
        &rag,
        &BTreeSet::from([library::FOLDER_SOURCE.to_string()]),
        &manifests,
    )
    .await
    .unwrap();
    let window: Vec<&library::LibraryFile> = files
        .iter()
        .filter(|f| f.kind() == Kind::Window && !f.removed && !f.in_folder())
        .collect();
    assert_eq!(report.reindexed, window.len(), "{report:?}");
    assert_eq!(report.in_place, 0, "{report:?}");
    assert_eq!(report.scans_kept, 0, "{report:?}");
    assert!(report.left.is_empty(), "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");

    let upgraded = stored(&data_dir).await;
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for f in &files {
        let source = &source_of[&f.path];
        let before: Vec<&ChunkRow> = manifest
            .chunks
            .values()
            .filter(|c| &c.source == source)
            .collect();
        let after: Vec<&ChunkRow> = upgraded
            .values()
            .map(|s| &s.row)
            .filter(|c| &c.source == source)
            .collect();
        seen.extend(after.iter().map(|c| c.id.as_str()));
        if f.removed || f.kind() == Kind::Secret {
            assert!(after.is_empty(), "{} has no chunks", f.path);
            continue;
        }
        assert!(!before.is_empty(), "{} was indexed by 0.0.10", f.path);
        match f.kind() {
            // Raised in place: the same chunks, only the version changed.
            Kind::Structure => {
                assert_eq!(
                    after.iter().map(|c| &c.id).collect::<BTreeSet<_>>(),
                    before.iter().map(|c| &c.id).collect::<BTreeSet<_>>(),
                    "{} keeps its chunks",
                    f.path
                );
                for b in &before {
                    let a = &upgraded[&b.id].row;
                    assert_eq!(
                        ChunkRow {
                            metadata_json: String::new(),
                            ..a.clone()
                        },
                        ChunkRow {
                            metadata_json: String::new(),
                            ..(*b).clone()
                        },
                        "{} chunk {} is unchanged but for its metadata",
                        f.path,
                        b.id
                    );
                    assert_metadata(a, kept(b, None));
                }
            }
            // Read again from the file: new chunks of this chunker, of the
            // same document and library source, holding the file's word.
            Kind::Window => {
                assert!(!after.is_empty(), "{} is indexed again", f.path);
                for b in &before {
                    assert!(
                        !upgraded.contains_key(&b.id),
                        "{}: 0.0.10's chunk {} was replaced",
                        f.path,
                        b.id
                    );
                }
                for a in &after {
                    let meta = metadata(a);
                    assert_eq!(
                        meta.get(CHUNKER_VERSION_KEY),
                        Some(&Value::String(CHUNKER_VERSION.to_string())),
                        "{} chunk {}",
                        f.path,
                        a.id
                    );
                    assert_eq!(a.space_id, before[0].space_id, "{}", f.path);
                    assert_eq!(a.space_id, f.space, "{}", f.path);
                    assert_eq!(a.title, before[0].title, "{}", f.path);
                }
                assert!(
                    after.iter().any(|a| a.text.contains(f.token.as_str())),
                    "{} still holds {}",
                    f.path,
                    f.token
                );
            }
            Kind::Secret => unreachable!(),
        }
    }
    // The note has no file to read again: its chunks are kept as they are,
    // recorded as kept from 0.0.10's chunker.
    let note: Vec<&ChunkRow> = manifest
        .chunks
        .values()
        .filter(|c| c.source == library::NOTE_SOURCE)
        .collect();
    assert!(!note.is_empty());
    for b in &note {
        let a = &upgraded
            .get(&b.id)
            .unwrap_or_else(|| panic!("the note's chunk {} is kept", b.id))
            .row;
        assert_eq!(
            ChunkRow {
                metadata_json: String::new(),
                ..a.clone()
            },
            ChunkRow {
                metadata_json: String::new(),
                ..(*b).clone()
            },
            "the note's chunk {}",
            b.id
        );
        assert_metadata(a, kept(b, Some("2")));
        seen.insert(a.id.as_str());
    }
    assert_eq!(
        seen.len(),
        upgraded.len(),
        "no chunk of another source appeared"
    );
    {
        let engine = rag.read().await;
        assert_searchable(&engine, &upgraded, &tokens).await;
    }
    assert_memories(&data_dir, &manifest).await;

    // 4. Another pass, then the app opened again.
    {
        let maintenance = rag.read().await.store_maintenance();
        maintenance.run().await.unwrap();
        assert!(
            !maintenance.changed(),
            "the second pass compacted the table"
        );
    }
    drop(rag);
    let engine = open(&data_dir).await;
    let reopened = stored(&data_dir).await;
    assert_eq!(rows_only(&reopened), rows_only(&upgraded), "reopened");
    assert!(
        engine.rechunk_sources().await.unwrap().is_empty(),
        "nothing is left to read again"
    );
    assert!(engine.stale_sources().await.unwrap().is_empty());
    assert_searchable(&engine, &reopened, &tokens).await;
    assert_memories(&data_dir, &manifest).await;

    // 5. What a library of 0.0.10 past 20,000 chunks meets: its L2 index
    // replaced by the cosine index. Built here from 1,000 rows, so this
    // library's index is replaced the same way.
    let (documents, _) = index_model::tables(&data_dir).unwrap();
    let lance = data_dir.join(crate::embeddings::model_switch::LANCE_DIR);
    let db = lancedb::connect(&lance.to_string_lossy())
        .execute()
        .await
        .unwrap();
    crate::storage::StoreMaintenance::new(db.clone(), &documents)
        .index_from(1_000)
        .index_vectors()
        .await
        .unwrap();
    let table = db.open_table(&documents).execute().await.unwrap();
    let index = table
        .list_indices()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.columns.iter().any(|c| c == "vector"))
        .expect("a vector index");
    assert_eq!(index.index_type, lancedb::index::IndexType::IvfHnswSq);
    let stats = table
        .index_stats(&index.name)
        .await
        .unwrap()
        .expect("index statistics");
    assert_eq!(stats.distance_type, Some(lancedb::DistanceType::Cosine));
    assert_eq!(stats.num_indexed_rows, reopened.len());
    assert_eq!(stats.num_unindexed_rows, 0);
    let reindexed = stored(&data_dir).await;
    assert_eq!(
        rows_only(&reindexed),
        rows_only(&reopened),
        "the index build changes no chunk"
    );
    // Through the approximate index, each sampled chunk is among its own ten
    // nearest neighbours (not necessarily first: the graph is approximate).
    for (i, (id, chunk)) in reindexed.iter().enumerate() {
        if i % 20 != 0 && chunk.row.source.contains("/archive/") {
            continue;
        }
        let hits = engine
            .store
            .vector_search(&chunk.vector, 10, None)
            .await
            .unwrap();
        assert!(
            hits.iter().any(|h| &h.id == id),
            "chunk {id} is found by its own vector through the cosine index"
        );
    }
    for (token, source) in &tokens {
        let results = engine.search(token, 5).await.unwrap();
        if let Some(source) = source {
            assert!(
                results.iter().any(|r| &r.source == source),
                "a search for {token} returns {source} through the cosine index"
            );
        }
    }
}

/// Before the startup upgrade the file that may hold secrets is still in the
/// index: its word finds it.
fn tokens_live_before_upgrade(
    tokens: &[(String, Option<String>)],
    secret_token: &str,
    secret_source: &str,
) -> Vec<(String, Option<String>)> {
    tokens
        .iter()
        .map(|(token, source)| {
            if token == secret_token {
                (token.clone(), Some(secret_source.to_string()))
            } else {
                (token.clone(), source.clone())
            }
        })
        .collect()
}
