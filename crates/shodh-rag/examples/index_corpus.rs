//! Index a folder into a throwaway data directory and report timing and
//! chunk statistics. Used to measure the indexing pipeline on a real corpus.
//!
//! ```text
//! cargo run -p shodh-rag --example index_corpus -- <folder> <model_dir> <scratch_data_dir> [query...]
//! ```
//!
//! `scratch_data_dir` is wiped first; never point it at a real index. Each
//! `query` is searched after indexing and its top citations are printed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use shodh_rag::config::RAGConfig;
use shodh_rag::indexing::{index_folder, IndexingOptions, IndexingState};
use shodh_rag::rag_engine::RAGEngine;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        bail!("usage: index_corpus <folder> <model_dir> <scratch_data_dir> [query...]");
    }
    let folder = &args[0];
    let model_dir = PathBuf::from(&args[1]);
    let data_dir = PathBuf::from(&args[2]);
    let queries = &args[3..];

    if data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)
            .with_context(|| format!("clearing {}", data_dir.display()))?;
    }
    let mut config = RAGConfig::default();
    config.data_dir = data_dir;
    config.embedding.model_dir = model_dir;
    config.embedding.use_e5 = true;
    config.embedding.dimension = 768;

    let load = Instant::now();
    let mut engine = RAGEngine::new(config).await?;
    if !engine.has_search_models() {
        bail!("search models did not load from the given model_dir");
    }
    println!(
        "engine+models loaded in {:.1}s",
        load.elapsed().as_secs_f64()
    );

    // Parse-only pass, to separate parsing time from chunking + embedding.
    let parser = shodh_rag::processing::DocumentParser::new();
    let parse_start = Instant::now();
    let mut parsed_files = 0usize;
    for entry in walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if entry.file_type().is_file() && shodh_rag::indexing::is_supported_file_type(&ext) {
            if parser.parse_file(path).is_ok() {
                parsed_files += 1;
            }
        }
    }
    println!(
        "parse-only pass: {parsed_files} files in {:.1}s",
        parse_start.elapsed().as_secs_f64()
    );

    let options = IndexingOptions {
        skip_indexed: false,
        watch_changes: false,
        process_subdirs: true,
        priority: "normal".to_string(),
        file_types: Vec::new(),
    };
    let state = IndexingState::default();
    let start = Instant::now();
    let result = index_folder(folder, "bench", &options, &mut engine, &state, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let secs = start.elapsed().as_secs_f64();
    println!(
        "indexed {} files, {} chunks in {:.1}s",
        result.files_processed, result.total_chunks, secs
    );
    for failure in &result.failures {
        println!("FAILED {}: {}", failure.file, failure.reason);
    }

    let chunks = engine.list_documents(None, 1_000_000).await?;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let (mut paged, mut with_bbox, mut with_section, mut chars) = (0usize, 0usize, 0usize, 0usize);
    for chunk in &chunks {
        chars += chunk.snippet.chars().count();
        let meta = &chunk.metadata;
        if meta.get("page_start").is_some_and(|p| !p.is_empty()) {
            paged += 1;
        }
        if meta.get("bboxes").is_some_and(|b| b != "[]") {
            with_bbox += 1;
        }
        if meta.get("section_path").is_some_and(|s| !s.is_empty()) {
            with_section += 1;
        }
        let kind = meta
            .get("unit_kind")
            .or_else(|| meta.get("chunk_type"))
            .cloned()
            .unwrap_or_else(|| "window".to_string());
        *by_kind.entry(kind).or_default() += 1;
    }
    let total = chunks.len().max(1) as f64;
    println!(
        "chunks={} paged={:.1}% bbox={:.1}% section={:.1}% avg_chars={:.0}",
        chunks.len(),
        100.0 * paged as f64 / total,
        100.0 * with_bbox as f64 / total,
        100.0 * with_section as f64 / total,
        chars as f64 / total
    );
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    for (kind, count) in kinds.iter().take(20) {
        println!("  {kind:>20} {count}");
    }

    for query in queries {
        println!("\nQ: {query}");
        for hit in engine.search(query, 5).await? {
            let page = hit
                .citation
                .as_ref()
                .and_then(|c| c.page_numbers.clone())
                .unwrap_or_else(|| "-".to_string());
            let section = hit
                .metadata
                .get("section_path")
                .cloned()
                .unwrap_or_default();
            let preview: String = hit.text.chars().take(100).collect();
            println!(
                "  {:.3} {} p.{} [{}] {}",
                hit.score,
                hit.title,
                page,
                section,
                preview.replace('\n', " ")
            );
        }
    }
    Ok(())
}
