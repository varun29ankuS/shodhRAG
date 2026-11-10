//! Time indexing a folder fresh, indexing it again (every file replaces its
//! previous chunks), and searching with and without the reranker.
//!
//! ```text
//! cargo run --release -p shodh-rag --example bench_speed -- <folder> <model_dir> <scratch_data_dir> <queries_file> [repeats]
//! ```
//!
//! `scratch_data_dir` is wiped first; never point it at a real index.
//! `queries_file` holds one query per line. Each query is searched once to
//! warm up, then `repeats` times (default 5) per reranker setting.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use shodh_rag::config::RAGConfig;
use shodh_rag::indexing::{index_folder, IndexingOptions, IndexingState};
use shodh_rag::rag_engine::RAGEngine;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        bail!(
            "usage: bench_speed <folder> <model_dir> <scratch_data_dir> <queries_file> [repeats]"
        );
    }
    let folder = &args[0];
    let data_dir = PathBuf::from(&args[2]);
    let queries: Vec<String> = std::fs::read_to_string(&args[3])
        .with_context(|| format!("reading {}", args[3]))?
        .lines()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(String::from)
        .collect();
    let repeats: usize = args.get(4).map_or(Ok(5), |r| r.parse())?;

    if data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)
            .with_context(|| format!("clearing {}", data_dir.display()))?;
    }
    let mut config = RAGConfig::default();
    config.data_dir = data_dir;
    config.embedding.model_dir = PathBuf::from(&args[1]);
    config.embedding.use_e5 = true;
    config.embedding.dimension = 768;
    config.features.enable_reranking = true;

    let engine = RAGEngine::new(config).await?;
    if !engine.has_search_models() {
        bail!("search models did not load from the given model_dir");
    }
    let shared = RwLock::new(engine);
    let options = IndexingOptions {
        skip_indexed: false,
        watch_changes: false,
        process_subdirs: true,
        priority: "normal".to_string(),
        file_types: Vec::new(),
    };

    for pass in ["fresh", "again"] {
        let state = IndexingState::default();
        let start = Instant::now();
        let result = index_folder(folder, "bench", &options, &shared, &state, None)
            .await
            .map_err(anyhow::Error::msg)?;
        println!(
            "index {pass}: {} files, {} chunks in {:.2}s ({} failed)",
            result.files_processed,
            result.total_chunks,
            start.elapsed().as_secs_f64(),
            result.failures.len()
        );
    }
    let finish = Instant::now();
    shared.read().await.optimize().await?;
    println!("optimize: {:.2}s", finish.elapsed().as_secs_f64());

    for rerank in [true, false] {
        {
            let mut engine = shared.write().await;
            let search = engine.config().search.clone();
            engine.set_search_config(search, rerank)?;
        }
        let engine = shared.read().await;
        let mut timings = Vec::with_capacity(queries.len() * repeats);
        for query in &queries {
            engine.search(query, 10).await?;
            for _ in 0..repeats {
                let start = Instant::now();
                engine.search(query, 10).await?;
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        timings.sort_by(f64::total_cmp);
        let at = |q: f64| timings[((timings.len() - 1) as f64 * q).round() as usize];
        println!(
            "search rerank={rerank}: n={} p50={:.1}ms p90={:.1}ms max={:.1}ms",
            timings.len(),
            at(0.5),
            at(0.9),
            at(1.0)
        );
    }
    Ok(())
}
