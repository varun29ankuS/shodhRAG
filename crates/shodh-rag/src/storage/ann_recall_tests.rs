//! Recall of the chunk table's approximate vector search against an exact
//! scan, on a table past [`VECTOR_INDEX_MIN_ROWS`], where every search goes
//! through the cosine IVF-HNSW-SQ index the maintenance pass builds.
//!
//! # The thresholds, from the index's parameters
//!
//! The index is `IvfHnswSqIndexBuilder::default()` by cosine
//! ([`super::maintenance`]), searched as [`LanceStore::vector_search`]
//! searches: no `nprobes`, `ef` or `refine_factor` set. In Lance 14 that is:
//! - **IVF:** the partition count is rows / 2^20 (IVF_HNSW_SQ's target
//!   partition size), at least 1: one partition for any library under a
//!   million chunks, so the coarse step reaches every vector and loses none.
//! - **SQ:** 8 bits per dimension, no refinement. On unit vectors the error
//!   of a quantised dot product is about (dimension range / 255) / sqrt(12)
//!   (the query's own norm is 1), about 3e-4 here, an order of magnitude
//!   below the gaps between successive near neighbours of this data (and of
//!   E5's), so it reorders almost nothing.
//! - **HNSW:** m = 20 (40 links per node on the base layer), ef_construction
//!   = 300, and a search beam of ef = k + k/2 (Lance's default without `ef`).
//!
//! So whatever the index misses, the HNSW beam misses. At k = 10 the beam is
//! 15 wide: HNSW with M of 16 to 32 and a beam that close to k recalls about
//! 0.9 of the true top 10 on million-vector benchmarks (Malkov and Yashunin,
//! 2018; ann-benchmarks' hnswlib runs), and more on a graph this size
//! (24,000 nodes, three or four hops from the entry point), built with a
//! construction beam 20 times the search beam. 0.90 is that floor: below it
//! the beam, not the data, loses neighbours, and searches should set `ef`.
//!
//! The app asks for five times the results it shows (`candidate_multiplier`),
//! so the true top 10 must be within the top 50 the index returns, where the
//! beam is 75 wide, five times the top 10's beam: 0.99.
//!
//! The vectors are clustered like a library's chunks (topics, documents
//! within a topic, chunks within a document) at the production dimension,
//! from a fixed seed; the queries are perturbed points of the same documents
//! that are not in the table.

use lancedb::query::{ExecutableQuery, QueryBase};

use super::maintenance::VECTOR_INDEX_MIN_ROWS;
use super::LanceStore;
use crate::types::ChunkRecord;

/// multilingual-e5-base's dimension.
const DIMENSION: usize = 768;
const TOPICS: usize = 100;
const DOCUMENTS_PER_TOPIC: usize = 8;
const CHUNKS_PER_DOCUMENT: usize = 30;
const ROWS: usize = TOPICS * DOCUMENTS_PER_TOPIC * CHUNKS_PER_DOCUMENT;
const QUERIES: usize = 100;
const K: usize = 10;
/// The app's `candidate_multiplier`.
const POOL: usize = K * 5;

const RECALL_AT_K: f64 = 0.90;
const RECALL_IN_POOL: f64 = 0.99;

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1].
    fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    }

    /// A standard normal (Box-Muller).
    fn normal(&mut self) -> f32 {
        let (u, v) = (self.unit(), self.unit());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    }

    fn around(&mut self, center: &[f32], spread: f32) -> Vec<f32> {
        normalized(center.iter().map(|c| c + spread * self.normal()).collect())
    }
}

fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The table's vectors and the queries: topic centres, documents around
/// them, chunks around their documents. Per-dimension spreads give a chunk a
/// cosine of about 0.8 to its document and 0.6 to its topic.
fn data() -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut rng = Rng(0x0011_A11E_5EED);
    let per_dim = |cosine: f32| ((1.0 / (cosine * cosine) - 1.0) / DIMENSION as f32).sqrt();
    let mut rows = Vec::with_capacity(ROWS);
    let mut documents = Vec::with_capacity(TOPICS * DOCUMENTS_PER_TOPIC);
    for _ in 0..TOPICS {
        let topic = normalized((0..DIMENSION).map(|_| rng.normal()).collect());
        for _ in 0..DOCUMENTS_PER_TOPIC {
            let document = rng.around(&topic, per_dim(0.75));
            for _ in 0..CHUNKS_PER_DOCUMENT {
                rows.push(rng.around(&document, per_dim(0.8)));
            }
            documents.push(document);
        }
    }
    let queries = (0..QUERIES)
        .map(|i| {
            let document = &documents[(i * 37) % documents.len()];
            rng.around(document, per_dim(0.8))
        })
        .collect();
    (rows, queries)
}

fn chunk(i: usize, vector: Vec<f32>) -> ChunkRecord {
    ChunkRecord {
        id: format!("chunk-{i:05}"),
        doc_id: format!("doc-{:04}", i / CHUNKS_PER_DOCUMENT),
        chunk_index: (i % CHUNKS_PER_DOCUMENT) as u32,
        text: String::new(),
        title: String::new(),
        source: format!("/library/doc-{:04}.txt", i / CHUNKS_PER_DOCUMENT),
        heading: String::new(),
        vector,
        space_id: "library".to_string(),
        metadata_json: "{}".to_string(),
        citation_json: "{}".to_string(),
        created_at: 0,
    }
}

/// The exact top `k` of `query` by cosine (the vectors are unit length).
fn exact(rows: &[Vec<f32>], query: &[f32], k: usize) -> Vec<usize> {
    let mut scored: Vec<(f32, usize)> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| (dot(row, query), i))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(k).map(|(_, i)| i).collect()
}

fn row_of(id: &str) -> usize {
    id.trim_start_matches("chunk-").parse().unwrap()
}

// Run by CI's Library store job, with Lance's index code optimised: in an
// unoptimised build the index over 24,000 vectors takes minutes.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "minutes in an unoptimised build: CI's Library store job runs it with Lance optimised"]
async fn ann_search_past_the_index_threshold_recalls_the_exact_neighbours() {
    assert!(ROWS > VECTOR_INDEX_MIN_ROWS, "{ROWS} rows");
    let (rows, queries) = data();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lance_data");
    let path = path.to_str().unwrap();
    let store = LanceStore::new(path, DIMENSION).await.unwrap();
    for (start, batch) in rows.chunks(6_000).enumerate() {
        let records = batch
            .iter()
            .enumerate()
            .map(|(j, v)| chunk(start * 6_000 + j, v.clone()))
            .collect();
        store.upsert_chunks(records).await.unwrap();
    }
    assert_eq!(store.count().await.unwrap(), ROWS);
    store.maintenance().run().await.unwrap();

    // The index the pass built, by cosine, covering every row.
    let db = lancedb::connect(path).execute().await.unwrap();
    let table = db.open_table("documents").execute().await.unwrap();
    let index = table
        .list_indices()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.columns.iter().any(|c| c == "vector"))
        .expect("the maintenance pass indexed the vectors");
    assert_eq!(index.index_type, lancedb::index::IndexType::IvfHnswSq);
    let stats = table
        .index_stats(&index.name)
        .await
        .unwrap()
        .expect("index statistics");
    assert_eq!(stats.distance_type, Some(lancedb::DistanceType::Cosine));
    assert_eq!(stats.num_indexed_rows, ROWS);
    assert_eq!(stats.num_unindexed_rows, 0);

    // A search as vector_search makes it goes through the index, not a scan
    // (a scan would recall everything and prove nothing).
    let plan = table
        .query()
        .nearest_to(queries[0].as_slice())
        .unwrap()
        .distance_type(lancedb::DistanceType::Cosine)
        .limit(K)
        .explain_plan(true)
        .await
        .unwrap();
    assert!(plan.contains("ANNSubIndex"), "{plan}");

    let mut at_k = 0usize;
    let mut in_pool = 0usize;
    for query in &queries {
        let truth = exact(&rows, query, K);
        let found: Vec<usize> = store
            .vector_search(query, K, None)
            .await
            .unwrap()
            .iter()
            .map(|h| row_of(&h.id))
            .collect();
        assert_eq!(found.len(), K);
        at_k += truth.iter().filter(|i| found.contains(i)).count();
        let pool: Vec<usize> = store
            .vector_search(query, POOL, None)
            .await
            .unwrap()
            .iter()
            .map(|h| row_of(&h.id))
            .collect();
        assert_eq!(pool.len(), POOL);
        in_pool += truth.iter().filter(|i| pool.contains(i)).count();
    }
    let recall_at_k = at_k as f64 / (QUERIES * K) as f64;
    let recall_in_pool = in_pool as f64 / (QUERIES * K) as f64;
    eprintln!(
        "ANN recall over {ROWS} rows, {QUERIES} queries: recall@{K} {recall_at_k:.3}, \
         exact top {K} within the top {POOL} {recall_in_pool:.3}"
    );
    assert!(
        recall_at_k >= RECALL_AT_K,
        "recall@{K} {recall_at_k:.3} is below {RECALL_AT_K}"
    );
    assert!(
        recall_in_pool >= RECALL_IN_POOL,
        "the exact top {K} within the top {POOL}: {recall_in_pool:.3} is below {RECALL_IN_POOL}"
    );
}
