use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

use crate::config::RAGConfig;
use crate::embeddings::e5::{E5Config, E5Embeddings};
use crate::embeddings::EmbeddingModel;
use crate::processing::chunker::TextChunker;
use crate::processing::parser::DocumentParser;
use crate::reranking::CrossEncoderReranker;
use crate::search::hybrid::{score_aware_rrf, HybridSource};
use crate::search::TextSearch;
use crate::storage::LanceStore;
use crate::types::{
    ChunkRecord, Citation, ComprehensiveResult, DocumentFormat, MetadataFilter, SimpleSearchResult,
};

pub struct RAGEngine {
    store: LanceStore,
    text_search: TextSearch,
    embeddings: Box<dyn EmbeddingModel>,
    chunker: TextChunker,
    parser: DocumentParser,
    config: RAGConfig,
    reranker: Option<CrossEncoderReranker>,
}

impl RAGEngine {
    pub async fn new(config: RAGConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir).ok();

        let lance_path = config.data_dir.join("lance_data");
        let store = LanceStore::new(
            lance_path.to_str().unwrap_or("./lance_data"),
            config.embedding.dimension,
        )
        .await
        .context("Failed to initialize LanceDB store")?;

        let text_search = TextSearch::new(
            config.data_dir.to_str().unwrap_or("./data"),
        )
        .context("Failed to initialize Tantivy search")?;

        let embeddings: Box<dyn EmbeddingModel> =
            if config.embedding.use_e5 {
                let e5_config = E5Config::auto_detect(&config.embedding.model_dir)
                    .ok_or_else(|| anyhow::anyhow!("E5 model not found at configured path"))?;
                Box::new(E5Embeddings::new(e5_config).context("Failed to load E5 embeddings")?)
            } else {
                return Err(anyhow::anyhow!(
                    "No embedding model available. Place E5 model in: {}",
                    config.embedding.model_dir.display()
                ));
            };

        let chunker = TextChunker::new(
            config.chunking.chunk_size,
            config.chunking.chunk_overlap,
            config.chunking.min_chunk_size,
        );

        // Try to load cross-encoder reranker if enabled and model exists
        let reranker = if config.features.enable_reranking || config.features.enable_cross_encoder {
            let reranker_dir = config.embedding.model_dir.join("ms-marco-MiniLM-L6-v2");
            match CrossEncoderReranker::new(&reranker_dir) {
                Ok(r) => {
                    tracing::info!("Cross-encoder reranker loaded from {}", reranker_dir.display());
                    Some(r)
                }
                Err(e) => {
                    tracing::warn!("Reranker not available ({}), continuing without reranking", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            store,
            text_search,
            embeddings,
            chunker,
            parser: DocumentParser::new(),
            config,
            reranker,
        })
    }

    /// Ingest a document from raw content
    pub async fn add_document(
        &mut self,
        content: &str,
        format: DocumentFormat,
        metadata: HashMap<String, String>,
        citation: Citation,
    ) -> Result<Vec<Uuid>> {
        let title = metadata
            .get("title")
            .cloned()
            .unwrap_or_else(|| "Untitled".to_string());
        let source = metadata
            .get("file_path")
            .or_else(|| metadata.get("source"))
            .cloned()
            .unwrap_or_default();
        let space_id = metadata
            .get("space_id")
            .cloned()
            .unwrap_or_default();

        let doc_id = Uuid::new_v4();

        // Contextual chunking: prepend document-level context to each chunk
        // before embedding for better retrieval (Anthropic's contextual retrieval approach)
        let chunks = self.chunker.chunk_with_context(content, &title, &source);

        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        // Embed the contextualized text (with document context prefix) for better vector representation
        let chunk_texts: Vec<&str> = chunks.iter().map(|c| c.contextualized_text.as_str()).collect();
        let embeddings = self.embeddings.embed_documents(&chunk_texts)?;

        let citation_json =
            serde_json::to_string(&citation).unwrap_or_else(|_| "{}".to_string());
        let metadata_json =
            serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());
        let now = chrono::Utc::now().timestamp();

        let mut chunk_records = Vec::with_capacity(chunks.len());
        let mut fts_batch = Vec::with_capacity(chunks.len());
        let mut chunk_ids = Vec::with_capacity(chunks.len());

        for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.into_iter()).enumerate() {
            let chunk_id = chunk.id;
            chunk_ids.push(chunk_id);

            // Store the original text (without context prefix) for display
            chunk_records.push(ChunkRecord {
                id: chunk_id.to_string(),
                doc_id: doc_id.to_string(),
                chunk_index: i as u32,
                text: chunk.text.clone(),
                title: title.clone(),
                source: source.clone(),
                heading: chunk.heading.clone().unwrap_or_default(),
                vector: embedding,
                space_id: space_id.clone(),
                metadata_json: metadata_json.clone(),
                citation_json: citation_json.clone(),
                created_at: now,
            });

            // Index contextualized text in FTS for richer BM25 matching
            fts_batch.push((
                chunk_id.to_string(),
                chunk.contextualized_text.clone(),
                title.clone(),
                source.clone(),
            ));
        }

        // Insert into LanceDB
        self.store
            .upsert_chunks(chunk_records)
            .await
            .context("Failed to store chunks in LanceDB")?;

        // Index in Tantivy
        self.text_search.index_chunks_batch(&fts_batch)?;
        self.text_search.commit()?;

        tracing::info!(
            "Ingested document '{}' ({} chunks) into space '{}'",
            title,
            chunk_ids.len(),
            space_id,
        );

        Ok(chunk_ids)
    }

    /// Ingest a document from a file path.
    /// Automatically removes any previously indexed chunks for the same file
    /// before inserting, preventing duplicates on re-indexing.
    pub async fn add_document_from_file(
        &mut self,
        path: &Path,
        metadata: HashMap<String, String>,
    ) -> Result<Vec<Uuid>> {
        let source = path.display().to_string();

        // Delete any existing chunks for this source path to prevent duplicates.
        // This makes re-indexing idempotent: the same file always produces a clean
        // replacement rather than accumulating stale copies.
        self.store.delete_by_source(&source).await.ok();
        self.text_search.delete_by_source(&source)?;
        self.text_search.commit()?;

        let parsed = self.parser.parse_file(path)?;

        let mut merged_metadata = parsed.metadata;
        for (k, v) in metadata {
            merged_metadata.insert(k, v);
        }
        // Ensure file_path in metadata matches the canonical source used for
        // deletion above. This prevents mismatches if the caller passes a
        // differently-formatted path string.
        merged_metadata.insert("file_path".to_string(), source.clone());

        let citation = Citation {
            title: parsed.title.clone(),
            source: source.clone(),
            ..Citation::default()
        };

        // Use structure-aware chunking for documents with structured data (PDF forms,
        // spreadsheet tables, relationships). Keeps related data together as atomic units
        // instead of scattering them across naive sliding-window chunks.
        if !parsed.structured_sections.is_empty() {
            let title = merged_metadata
                .get("title")
                .cloned()
                .unwrap_or_else(|| parsed.title.clone());
            let space_id = merged_metadata
                .get("space_id")
                .cloned()
                .unwrap_or_default();

            let chunks = self.chunker.chunk_structured(
                &parsed.structured_sections,
                &title,
                &source,
            );

            if chunks.is_empty() {
                return Ok(Vec::new());
            }

            let doc_id = Uuid::new_v4();
            let chunk_texts: Vec<&str> = chunks.iter().map(|c| c.contextualized_text.as_str()).collect();
            let embeddings = self.embeddings.embed_documents(&chunk_texts)?;

            let citation_json = serde_json::to_string(&citation).unwrap_or_else(|_| "{}".to_string());
            let metadata_json = serde_json::to_string(&merged_metadata).unwrap_or_else(|_| "{}".to_string());
            let now = chrono::Utc::now().timestamp();

            let mut chunk_records = Vec::with_capacity(chunks.len());
            let mut fts_batch = Vec::with_capacity(chunks.len());
            let mut chunk_ids = Vec::with_capacity(chunks.len());

            for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.into_iter()).enumerate() {
                let chunk_id = chunk.id;
                chunk_ids.push(chunk_id);

                let mut per_chunk_meta = merged_metadata.clone();
                if let Some(heading) = &chunk.heading {
                    per_chunk_meta.insert("chunk_type".to_string(), heading.clone());
                }

                let per_chunk_meta_json = serde_json::to_string(&per_chunk_meta)
                    .unwrap_or_else(|_| metadata_json.clone());

                chunk_records.push(ChunkRecord {
                    id: chunk_id.to_string(),
                    doc_id: doc_id.to_string(),
                    chunk_index: i as u32,
                    text: chunk.text.clone(),
                    title: title.clone(),
                    source: source.clone(),
                    heading: chunk.heading.clone().unwrap_or_default(),
                    vector: embedding,
                    space_id: space_id.clone(),
                    metadata_json: per_chunk_meta_json,
                    citation_json: citation_json.clone(),
                    created_at: now,
                });

                fts_batch.push((
                    chunk_id.to_string(),
                    chunk.contextualized_text.clone(),
                    title.clone(),
                    source.clone(),
                ));
            }

            self.store.upsert_chunks(chunk_records).await
                .context("Failed to store structured chunks in LanceDB")?;
            self.text_search.index_chunks_batch(&fts_batch)?;
            self.text_search.commit()?;

            tracing::info!(
                "Ingested structured document '{}' ({} chunks, {} sections) into space '{}'",
                title, chunk_ids.len(), parsed.structured_sections.len(), space_id,
            );

            return Ok(chunk_ids);
        }

        self.add_document(&parsed.content, parsed.format, merged_metadata, citation)
            .await
    }

    /// Search with hybrid vector + FTS fusion
    pub async fn search(
        &self,
        query: &str,
        k: usize,
    ) -> Result<Vec<SimpleSearchResult>> {
        let results = self.search_comprehensive(query, k, None).await?;

        Ok(results
            .into_iter()
            .map(|r| {
                let doc_id = r
                    .metadata
                    .get("doc_id")
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .unwrap_or_default();
                let chunk_id = r
                    .metadata
                    .get("chunk_index")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                let title = r.citation.title.clone();
                let source = r.citation.source.clone();
                SimpleSearchResult {
                    id: r.id,
                    score: r.score,
                    text: r.snippet.clone(),
                    metadata: r.metadata,
                    title,
                    source,
                    heading: None,
                    citation: Some(r.citation),
                    doc_id,
                    chunk_id,
                }
            })
            .collect())
    }

    /// Full search with filters, reranking, and source tracking.
    /// Automatically decomposes multi-part queries into sub-queries for parallel retrieval.
    pub async fn search_comprehensive(
        &self,
        query: &str,
        k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<ComprehensiveResult>> {
        // Decompose complex queries into independent sub-queries
        let decomposed = crate::rag::query_decomposer::decompose_query(query);

        if decomposed.sub_queries.len() > 1 {
            tracing::debug!(
                original = query,
                sub_queries = ?decomposed.sub_queries,
                strategy = ?decomposed.strategy,
                "Query decomposed"
            );

            // Search each sub-query independently
            let mut result_sets = Vec::new();
            for sub_query in &decomposed.sub_queries {
                match self.search_single_query(sub_query, k, filter.clone()).await {
                    Ok(results) => result_sets.push(results),
                    Err(e) => {
                        tracing::warn!(sub_query = sub_query, error = %e, "Sub-query search failed");
                    }
                }
            }

            if result_sets.is_empty() {
                return Ok(Vec::new());
            }

            // Merge with round-robin interleaving and deduplication
            let mut merged = crate::rag::query_decomposer::merge_results(result_sets, k);

            // Expand with neighbors on the merged set
            self.expand_with_neighbors(&mut merged, 1).await;

            return Ok(merged);
        }

        let mut results = self.search_single_query(query, k, filter).await?;
        self.expand_with_neighbors(&mut results, 1).await;
        Ok(results)
    }

    /// Execute a single search query through the full pipeline.
    async fn search_single_query(
        &self,
        query: &str,
        k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<ComprehensiveResult>> {
        // Use same candidate count for both vector and FTS for balanced fusion
        let candidate_count = k * self.config.search.candidate_multiplier;

        // Generate query embedding
        let query_embedding = self.embeddings.embed_query(query)?;

        // Build filter predicate for LanceDB
        let lance_filter = filter.as_ref().and_then(|f| f.to_lance_predicate());

        // Extract source filter for FTS consistency
        let source_filter = filter.as_ref().and_then(|f| f.source_path.as_deref());

        // Vector search via LanceDB
        let vector_hits = self
            .store
            .vector_search(
                &query_embedding,
                candidate_count,
                lance_filter.as_deref(),
            )
            .await?;

        let vector_results: Vec<(String, f32)> = vector_hits
            .iter()
            .map(|h| (h.id.clone(), h.score))
            .collect();

        // Full-text search via Tantivy — use SAME candidate count for balanced fusion
        let fts_results = self.text_search.search_filtered(
            query,
            candidate_count,
            source_filter,
        )?;

        tracing::info!(
            query = query,
            candidate_count = candidate_count,
            vector_hits = vector_hits.len(),
            fts_hits = fts_results.len(),
            "Hybrid search candidates"
        );

        // Score-aware Reciprocal Rank Fusion — preserves original quality signals
        let fused = score_aware_rrf(
            vector_results,
            fts_results,
            self.config.search.rrf_k,
            candidate_count, // Get more candidates for reranking
            self.config.search.score_weight,
        );

        tracing::info!(
            fused_count = fused.len(),
            threshold = self.config.search.min_score_threshold,
            "RRF fusion complete"
        );

        // Build hit_map from vector results for fast lookup
        let hit_map: HashMap<String, &crate::storage::SearchHit> =
            vector_hits.iter().map(|h| (h.id.clone(), h)).collect();

        // Collect IDs that are FTS-only (not in vector results)
        let fts_only_ids: Vec<String> = fused
            .iter()
            .filter(|(id, _, _)| !hit_map.contains_key(id))
            .map(|(id, _, _)| id.clone())
            .collect();

        // Fetch full data for FTS-only results from LanceDB
        let fts_only_hits = if !fts_only_ids.is_empty() {
            self.store.get_by_ids(&fts_only_ids).await?
        } else {
            Vec::new()
        };
        let fts_only_map: HashMap<String, &crate::storage::SearchHit> =
            fts_only_hits.iter().map(|h| (h.id.clone(), h)).collect();

        // Log top fused scores for diagnostics
        if let Some((top_id, top_score, _)) = fused.first() {
            tracing::info!(
                top_fused_id = top_id,
                top_fused_score = top_score,
                hit_map_size = hit_map.len(),
                fts_only_map_size = fts_only_map.len(),
                "Fused score diagnostics"
            );
        }

        // Build result objects
        let mut results = Vec::with_capacity(fused.len());

        for (id, score, source) in &fused {
            let source_label = match source {
                HybridSource::Vector => "lance",
                HybridSource::TextSearch => "tantivy",
                HybridSource::Both => "hybrid",
            };

            // Look up in vector hits first, then FTS-only hits
            let hit = hit_map.get(id).or_else(|| fts_only_map.get(id));

            if let Some(hit) = hit {
                let metadata: HashMap<String, String> =
                    serde_json::from_str(&hit.metadata_json).unwrap_or_default();
                let citation: Citation =
                    serde_json::from_str(&hit.citation_json).unwrap_or_default();

                let mut full_metadata = metadata;
                full_metadata.insert("doc_id".to_string(), hit.doc_id.clone());
                full_metadata.insert("chunk_index".to_string(), hit.chunk_index.to_string());
                full_metadata.insert("source_file".to_string(), hit.source.clone());
                full_metadata.insert("space_id".to_string(), hit.space_id.clone());

                results.push(ComprehensiveResult {
                    id: Uuid::parse_str(&hit.id).unwrap_or_default(),
                    score: *score,
                    metadata: full_metadata,
                    citation,
                    snippet: hit.text.clone(),
                    source_index: source_label.to_string(),
                });
            }
            // Skip results where we can't find full data (shouldn't happen now)
        }

        // Filter by minimum score threshold
        let threshold = self.config.search.min_score_threshold;
        let pre_filter_count = results.len();
        results.retain(|r| r.score >= threshold);
        tracing::info!(
            pre_filter = pre_filter_count,
            post_filter = results.len(),
            threshold = threshold,
            top_score = results.first().map(|r| r.score).unwrap_or(0.0),
            "Score threshold filter"
        );

        // Deduplicate near-identical chunks (from overlapping windows)
        Self::deduplicate_results(&mut results, 0.75);

        // Apply cross-encoder reranking if available (before MMR so diversity uses final scores)
        if let Some(reranker) = &self.reranker {
            if results.len() > 1 {
                let candidates: Vec<(String, String)> = results
                    .iter()
                    .map(|r| (r.id.to_string(), r.snippet.clone()))
                    .collect();

                match reranker.rerank(query, &candidates, candidates.len()) {
                    Ok(reranked) => {
                        let rerank_scores: HashMap<String, f32> =
                            reranked.into_iter().collect();

                        // Update scores where reranking succeeded; keep original score
                        // for any candidates the cross-encoder couldn't tokenize.
                        for result in &mut results {
                            if let Some(&new_score) = rerank_scores.get(&result.id.to_string()) {
                                result.score = new_score;
                            }
                        }
                        results.sort_by(|a, b| {
                            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Reranking failed, using fusion scores: {}", e);
                    }
                }
            }
        }

        // MMR diversity: penalize repeated doc_id to spread results across documents
        // Applied after reranking so diversity uses the final quality scores
        Self::apply_mmr_diversity(&mut results, 0.7);

        // Final truncation to requested k
        results.truncate(k);

        Ok(results)
    }

    /// Expand top-k results with neighboring chunks from the same document.
    /// For each result, fetches ±window adjacent chunks by chunk_index and
    /// concatenates them in reading order (prev + current + next).
    async fn expand_with_neighbors(&self, results: &mut Vec<ComprehensiveResult>, window: u32) {
        for result in results.iter_mut() {
            let doc_id = match result.metadata.get("doc_id") {
                Some(id) if !id.is_empty() => id.clone(),
                _ => continue,
            };
            let chunk_index: u32 = result
                .metadata
                .get("chunk_index")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            match self.store.get_neighbors(&doc_id, chunk_index, window).await {
                Ok(neighbors) if !neighbors.is_empty() => {
                    let mut before = String::new();
                    let mut after = String::new();

                    for neighbor in &neighbors {
                        if neighbor.chunk_index < chunk_index {
                            if !before.is_empty() {
                                before.push_str("\n");
                            }
                            before.push_str(&neighbor.text);
                        } else if neighbor.chunk_index > chunk_index {
                            if !after.is_empty() {
                                after.push_str("\n");
                            }
                            after.push_str(&neighbor.text);
                        }
                    }

                    let mut expanded = String::new();
                    if !before.is_empty() {
                        expanded.push_str(&before);
                        expanded.push_str("\n");
                    }
                    expanded.push_str(&result.snippet);
                    if !after.is_empty() {
                        expanded.push_str("\n");
                        expanded.push_str(&after);
                    }

                    result.snippet = expanded;
                }
                Ok(_) => {} // No neighbors found
                Err(e) => {
                    tracing::debug!("Neighbor expansion failed for doc {}: {}", doc_id, e);
                }
            }
        }
    }

    /// Delete all documents from a specific source/folder
    pub async fn delete_by_source(&mut self, source: &str) -> Result<usize> {
        let deleted = self.store.delete_by_source(source).await?;
        self.text_search.delete_by_source(source)?;
        self.text_search.commit()?;
        Ok(deleted)
    }

    /// Delete all chunks belonging to a specific space.
    /// Removes from both the vector store (LanceDB) and the text search index (Tantivy).
    pub async fn delete_by_space_id(&mut self, space_id: &str) -> Result<usize> {
        // First, get all chunk IDs for this space so we can remove them from Tantivy
        let predicate = format!("space_id = '{}'", space_id.replace('\'', "''"));
        let chunks = self.store.list_chunks(Some(&predicate), 1_000_000).await?;

        // Delete each chunk from the Tantivy text search index by ID
        for chunk in &chunks {
            let _ = self.text_search.delete_by_id(&chunk.id);
        }
        self.text_search.commit()?;

        // Delete from LanceDB by space_id
        let deleted = self.store.delete_by_space_id(space_id).await?;

        tracing::info!(
            space_id = %space_id,
            deleted_chunks = deleted,
            "Deleted space data from vector store and text index"
        );
        Ok(deleted)
    }

    /// Clear all data
    pub async fn clear_all_data(&mut self) -> Result<()> {
        self.store.clear().await?;
        self.text_search.clear()?;
        Ok(())
    }

    /// Get statistics about the current state
    pub async fn get_statistics(&self) -> Result<HashMap<String, String>> {
        let mut stats = HashMap::new();
        let chunk_count = self.store.count().await?;
        let doc_count = self.store.count_documents().await.unwrap_or(0);
        let fts_count = self.text_search.count()?;

        stats.insert("total_chunks".to_string(), chunk_count.to_string());
        stats.insert("total_documents".to_string(), doc_count.to_string());
        stats.insert("fts_indexed".to_string(), fts_count.to_string());
        stats.insert(
            "embedding_dimension".to_string(),
            self.embeddings.dimension().to_string(),
        );
        stats.insert(
            "data_dir".to_string(),
            self.config.data_dir.display().to_string(),
        );

        Ok(stats)
    }

    /// Count distinct documents in the index
    pub async fn count_documents(&self) -> Result<usize> {
        self.store.count_documents().await
    }

    /// Get document metadata for corpus stats: (doc_id, title, source)
    pub async fn get_document_info(&self) -> Result<Vec<(String, String, String)>> {
        self.store.get_document_info().await
    }

    /// List all chunks matching an optional filter predicate (no vector search).
    /// This is the correct way to enumerate documents — NOT search_comprehensive("").
    /// Returns ComprehensiveResult for API compatibility.
    pub async fn list_documents(
        &self,
        filter: Option<MetadataFilter>,
        limit: usize,
    ) -> Result<Vec<ComprehensiveResult>> {
        let predicate = filter.as_ref().and_then(|f| f.to_lance_predicate());

        let hits = self
            .store
            .list_chunks(predicate.as_deref(), limit)
            .await?;

        let mut results = Vec::with_capacity(hits.len());
        for hit in hits {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&hit.metadata_json).unwrap_or_default();
            let citation: Citation =
                serde_json::from_str(&hit.citation_json).unwrap_or_default();

            let mut full_metadata = metadata;
            full_metadata.insert("doc_id".to_string(), hit.doc_id.clone());
            full_metadata.insert("chunk_index".to_string(), hit.chunk_index.to_string());
            full_metadata.insert("source_file".to_string(), hit.source.clone());
            full_metadata.insert("space_id".to_string(), hit.space_id.clone());

            results.push(ComprehensiveResult {
                id: Uuid::parse_str(&hit.id).unwrap_or_default(),
                score: 0.0,
                metadata: full_metadata,
                citation,
                snippet: hit.text.clone(),
                source_index: "list".to_string(),
            });
        }

        Ok(results)
    }

    /// Access to the embedding model for external use
    pub fn embeddings(&self) -> &dyn EmbeddingModel {
        self.embeddings.as_ref()
    }

    /// Access to config
    pub fn config(&self) -> &RAGConfig {
        &self.config
    }

    /// Trigger index creation if needed (after large ingestion)
    pub async fn optimize(&self) -> Result<()> {
        self.store.create_index_if_needed().await
    }

    /// Remove near-duplicate results using Jaccard similarity on word sets.
    /// Chunks with overlapping windows often produce near-identical content.
    fn deduplicate_results(results: &mut Vec<ComprehensiveResult>, threshold: f32) {
        use std::collections::HashSet;
        let word_sets: Vec<HashSet<&str>> = results
            .iter()
            .map(|r| r.snippet.split_whitespace().collect::<HashSet<_>>())
            .collect();

        let mut keep = Vec::new();
        for i in 0..results.len() {
            let mut is_dup = false;
            for &j in &keep {
                let intersection = word_sets[i].intersection(&word_sets[j]).count();
                let union = word_sets[i].union(&word_sets[j]).count();
                if union > 0 && (intersection as f32 / union as f32) > threshold {
                    is_dup = true;
                    break;
                }
            }
            if !is_dup {
                keep.push(i);
            }
        }

        // Remove duplicates in reverse index order to preserve positions.
        let keep_set: HashSet<usize> = keep.into_iter().collect();
        let mut i = results.len();
        while i > 0 {
            i -= 1;
            if !keep_set.contains(&i) {
                results.swap_remove(i);
            }
        }
        // Restore original order by score (descending)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Maximal Marginal Relevance — penalize repeated documents to ensure diversity.
    /// Each subsequent chunk from the same doc_id gets score *= lambda^count.
    fn apply_mmr_diversity(results: &mut Vec<ComprehensiveResult>, lambda: f32) {
        let mut doc_seen: HashMap<String, u32> = HashMap::new();
        for result in results.iter_mut() {
            let doc_id = result.metadata.get("doc_id").cloned().unwrap_or_default();
            let count = doc_seen.entry(doc_id).or_insert(0);
            if *count > 0 {
                result.score *= lambda.powi(*count as i32);
            }
            *count += 1;
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
}
