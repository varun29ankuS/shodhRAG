//! Measures the graph on a real paper folder, optionally with live OpenAlex lookups.
//!
//! `cargo test -p shodh-rag --lib research::citations::corpus_live -- --ignored --nocapture`
//! with `SHODH_RESEARCH_CORPUS` set to a folder of PDFs and `SHODH_GRAPH_DB` to a
//! `shodh.db` path that keeps the OpenAlex cache between runs. Set `SHODH_LIVE_OPENALEX=1`
//! to allow network lookups (through the same paced, cached client the app uses);
//! otherwise only cached answers are read. A folder named `My Papers` and files marked
//! `(annotated)` are skipped. Prints the build report.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::resolve::Resolver;
use super::service::{CitationService, GraphSlot};
use crate::embeddings::EmbeddingModel;
use crate::harness::web::SafeClient;
use crate::research::db::ResearchDb;
use crate::research::results::ResultService;
use crate::statements::testing::{t0, FixedEmbedder, TestClock};
use crate::statements::{DynamicsStore, StatementStore};

/// Hashes words into a fixed number of dimensions (no vocabulary limit).
struct HashEmbedder;

const DIM: usize = 64;

impl EmbeddingModel for HashEmbedder {
    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.embed_document(text)
    }
    fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut v = vec![0.0f32; DIM];
        for word in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let h = word
                .to_lowercase()
                .bytes()
                .fold(1469598103934665603u64, |h, b| {
                    (h ^ u64::from(b)).wrapping_mul(1099511628211)
                });
            v[(h % DIM as u64) as usize] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        Ok(v.into_iter().map(|x| x / norm).collect())
    }
    fn dimension(&self) -> usize {
        DIM
    }
}

fn pdfs_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if path.is_dir() {
            if name != "My Papers" {
                pdfs_under(&path, out);
            }
        } else if name.to_lowercase().ends_with(".pdf") && !name.contains("(annotated)") {
            out.push(path);
        }
    }
}

#[tokio::test]
#[ignore = "needs SHODH_RESEARCH_CORPUS and SHODH_GRAPH_DB"]
async fn measure_the_graph_of_a_corpus() {
    let (Ok(corpus), Ok(db_path)) = (
        std::env::var("SHODH_RESEARCH_CORPUS"),
        std::env::var("SHODH_GRAPH_DB"),
    ) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let ontology = Arc::new(shodh_ontology::Ontology::builtin_with_packs(&["research"]).unwrap());
    let dynamics = Arc::new(DynamicsStore::open(&dir.path().join("dyn.db"), None).unwrap());
    let store = Arc::new(
        StatementStore::open(
            &dir.path().join("lance"),
            DIM,
            ontology,
            dynamics,
            Arc::new(FixedEmbedder(Arc::new(HashEmbedder))),
            TestClock::at(t0()),
        )
        .await
        .unwrap(),
    );
    let db = Arc::new(ResearchDb::open(Path::new(&db_path), None).unwrap());
    let results = ResultService::new(store.clone(), db.clone(), "measure");
    let service = CitationService::new(store, db.clone(), GraphSlot::default());
    let resolver = if std::env::var("SHODH_LIVE_OPENALEX").as_deref() == Ok("1") {
        Resolver::online(Arc::new(SafeClient::system()), db.clone())
            .with_limits(1_500, super::resolve::MIN_INTERVAL)
    } else {
        Resolver::cache_only(db.clone())
    };
    let mut files = Vec::new();
    pdfs_under(Path::new(&corpus), &mut files);
    let files: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let report = service
        .build(&files, &results, &resolver, &|_| {})
        .await
        .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    let graph = service.graph(&results).await.unwrap();
    for paper in graph.papers().iter().filter(|p| p.in_library) {
        println!(
            "library: {} | arXiv {:?} | OpenAlex {:?} | cites {} | cited by {} in library",
            paper.label(),
            paper.arxiv_id,
            paper.openalex_id,
            graph.cited(&paper.id).len(),
            graph.citers(&paper.id).len()
        );
    }
    for hit in graph.most_cited(10, 2) {
        println!(
            "most cited: {} ({} library papers)",
            hit.paper.label(),
            hit.count
        );
    }
}
