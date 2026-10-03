use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use uuid::Uuid;

use crate::config::RAGConfig;
use crate::embeddings::e5::{E5Config, E5Embeddings};
use crate::embeddings::{EmbeddingModel, SearchModelsMissing};
use crate::processing::chunker::{ContextualChunkResult, TextChunker};
use crate::processing::parser::DocumentParser;
use crate::reranking::CrossEncoderReranker;
use crate::search::hybrid::{score_aware_rrf, HybridSource};
use crate::search::TextSearch;
use crate::storage::LanceStore;
use crate::types::{
    ChunkRecord, Citation, ComprehensiveResult, DocumentFormat, MetadataFilter, SimpleSearchResult,
};

/// One document's chunks, embedded and converted to storage records but not
/// yet written. Lets ingestion finish all fallible work before touching the index.
struct PreparedDocument {
    title: String,
    space_id: String,
    records: Vec<ChunkRecord>,
    fts_batch: Vec<(String, String, String, String)>,
    chunk_ids: Vec<Uuid>,
}

/// Normalize a file path for consistent storage and lookup across Windows/Unix.
/// Converts backslashes to forward slashes and lowercases on Windows so that
/// `delete_by_source` predicates always match regardless of how the path was
/// originally formatted.
pub fn normalize_source_path(path: &Path) -> String {
    let s = path.display().to_string().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

// Compiled-once regex patterns for structured field extraction at ingest time.
// Extracting emails, phones, PAN, GSTIN, amounts, and dates from chunk text
// at zero query-time cost — no LLM needed for field-level extraction.
static RE_EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}").unwrap());
static RE_PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\+91[\s\-]?)?(?:\d[\s\-]?){10}").unwrap());
static RE_PAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Z]{5}\d{4}[A-Z]\b").unwrap());
static RE_GSTIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{2}[A-Z]{5}\d{4}[A-Z]\d[Z][A-Z0-9]\b").unwrap());
static RE_AMOUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:Rs\.?|INR|₹)\s*[\d,]+(?:\.\d{1,2})?").unwrap());
static RE_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{1,2}[/\-\.]\d{1,2}[/\-\.]\d{2,4}\b").unwrap());
static RE_INVOICE_NO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:invoice|inv|bill)[\s.#:_\-]*(?:no\.?|number)?[\s.#:_\-]*([A-Z0-9/\-]+)")
        .unwrap()
});

/// Extract structured fields from chunk text using regex.
/// Returns key-value pairs to merge into chunk metadata.
/// This runs once at ingest time — zero cost at query time.
/// Record a chunk's source page in its metadata. Paged chunks get `page`,
/// `page_start` and `page_end` (a chunk is produced from exactly one page, so
/// start and end are equal; the range keys let consumers treat merged/expanded
/// results uniformly). Unpaged chunks get no page keys.
fn insert_page_metadata(meta: &mut HashMap<String, String>, page: Option<usize>) {
    if let Some(page) = page {
        let page = page.to_string();
        meta.insert("page".to_string(), page.clone());
        meta.insert("page_start".to_string(), page.clone());
        meta.insert("page_end".to_string(), page);
    }
}

/// Human-readable page label (`"3"` or `"3-5"`) from chunk metadata written by
/// [`insert_page_metadata`]. Returns `None` for unpaged chunks.
pub fn page_numbers_from_metadata(meta: &HashMap<String, String>) -> Option<String> {
    let non_empty = |key: &str| meta.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
    match (non_empty("page_start"), non_empty("page_end")) {
        (Some(start), Some(end)) if start != end => Some(format!("{}-{}", start, end)),
        (Some(start), _) => Some(start.to_string()),
        (None, Some(end)) => Some(end.to_string()),
        (None, None) => non_empty("page").map(str::to_string),
    }
}

fn extract_structured_fields(text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();

    // Emails
    let emails: Vec<String> = RE_EMAIL
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !emails.is_empty() {
        fields.insert("extracted_emails".to_string(), emails.join(", "));
    }

    // Phone numbers (basic cleanup: strip non-digit noise)
    let phones: Vec<String> = RE_PHONE
        .find_iter(text)
        .map(|m| {
            m.as_str()
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '+')
                .collect::<String>()
        })
        .filter(|p| p.len() >= 10)
        .collect();
    if !phones.is_empty() {
        fields.insert("extracted_phones".to_string(), phones.join(", "));
    }

    // PAN numbers
    let pans: Vec<String> = RE_PAN
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !pans.is_empty() {
        fields.insert("extracted_pan".to_string(), pans.join(", "));
    }

    // GSTIN
    let gstins: Vec<String> = RE_GSTIN
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !gstins.is_empty() {
        fields.insert("extracted_gstin".to_string(), gstins.join(", "));
    }

    // Amounts
    let amounts: Vec<String> = RE_AMOUNT
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !amounts.is_empty() {
        fields.insert("extracted_amounts".to_string(), amounts.join("; "));
    }

    // Dates
    let dates: Vec<String> = RE_DATE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if !dates.is_empty() {
        fields.insert("extracted_dates".to_string(), dates.join(", "));
    }

    // Invoice numbers
    if let Some(cap) = RE_INVOICE_NO.captures(text) {
        if let Some(inv) = cap.get(1) {
            fields.insert("extracted_invoice_no".to_string(), inv.as_str().to_string());
        }
    }

    fields
}

/// The ONNX models the engine searches with: the E5 embedder (required) and
/// the cross-encoder reranker (optional). Loading takes seconds and reads
/// ~600 MB, so it is separate from [`RAGEngine::new`] and can run on a
/// blocking thread before [`RAGEngine::attach_search_models`].
pub struct SearchModels {
    embeddings: Arc<dyn EmbeddingModel>,
    reranker: Option<CrossEncoderReranker>,
}

impl SearchModels {
    /// Whether the E5 model files exist under `config.embedding.model_dir`.
    pub fn available(config: &RAGConfig) -> bool {
        E5Config::auto_detect(&config.embedding.model_dir).is_some()
    }

    /// Load the models from `config.embedding.model_dir` (blocking).
    /// Fails with [`SearchModelsMissing`] when the E5 files are absent.
    pub fn load(config: &RAGConfig) -> Result<Self> {
        let e5_config =
            E5Config::auto_detect(&config.embedding.model_dir).ok_or(SearchModelsMissing)?;
        let embeddings: Arc<dyn EmbeddingModel> =
            Arc::new(E5Embeddings::new(e5_config).context("Failed to load E5 embeddings")?);

        let reranker = if config.features.enable_reranking || config.features.enable_cross_encoder {
            let reranker_dir = config.embedding.model_dir.join("ms-marco-MiniLM-L6-v2");
            match CrossEncoderReranker::new(&reranker_dir) {
                Ok(r) => {
                    tracing::info!(
                        "Cross-encoder reranker loaded from {}",
                        reranker_dir.display()
                    );
                    Some(r)
                }
                Err(e) => {
                    tracing::warn!(
                        "Reranker not available ({}), continuing without reranking",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            embeddings,
            reranker,
        })
    }

    pub fn dimension(&self) -> usize {
        self.embeddings.dimension()
    }

    pub fn has_reranker(&self) -> bool {
        self.reranker.is_some()
    }
}

pub struct RAGEngine {
    store: LanceStore,
    text_search: TextSearch,
    /// `None` until the search models are installed and attached; search and
    /// indexing then fail with [`SearchModelsMissing`].
    /// Shared (`Arc`) so other stores, such as the statement store, embed with the same
    /// model without holding the engine lock during inference.
    embeddings: Option<Arc<dyn EmbeddingModel>>,
    chunker: TextChunker,
    parser: DocumentParser,
    config: RAGConfig,
    reranker: Option<CrossEncoderReranker>,
}

impl RAGEngine {
    /// Open the stores and, when the model files are present, load the
    /// search models. Without them (first run) the engine starts in a
    /// degraded state: stores, listing and deletion work; search and indexing
    /// return [`SearchModelsMissing`] until [`Self::attach_search_models`].
    /// A model that is present but fails to load is logged and treated the
    /// same way, so a damaged model never prevents the app from starting.
    pub async fn new(config: RAGConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir).ok();

        let lance_path = config.data_dir.join("lance_data");
        let store = LanceStore::new(
            lance_path.to_str().unwrap_or("./lance_data"),
            config.embedding.dimension,
        )
        .await
        .context("Failed to initialize LanceDB store")?;

        let text_search = TextSearch::new(config.data_dir.to_str().unwrap_or("./data"))
            .context("Failed to initialize Tantivy search")?;

        let models = if config.embedding.use_e5 && SearchModels::available(&config) {
            match SearchModels::load(&config) {
                Ok(models) => Some(models),
                Err(e) => {
                    tracing::error!(
                        model_dir = %config.embedding.model_dir.display(),
                        error = %format!("{e:#}"),
                        "Search models failed to load; starting without search"
                    );
                    None
                }
            }
        } else {
            tracing::warn!(
                model_dir = %config.embedding.model_dir.display(),
                "Search models are not installed; starting without search"
            );
            None
        };

        let chunker = TextChunker::new(
            config.chunking.chunk_size,
            config.chunking.chunk_overlap,
            config.chunking.min_chunk_size,
        );

        let mut engine = Self {
            store,
            text_search,
            embeddings: None,
            chunker,
            parser: DocumentParser::new(),
            config,
            reranker: None,
        };
        if let Some(models) = models {
            if let Err(e) = engine.attach_search_models(models) {
                tracing::error!(error = %e, "Search models rejected; starting without search");
            }
        }

        // After schema migration the Tantivy index is empty but LanceDB still
        // has all the chunks.  Rebuild the text index so searches work immediately.
        if engine.text_search.is_empty() {
            let lance_count = engine.store.count().await.unwrap_or(0);
            if lance_count > 0 {
                tracing::info!(
                    lance_chunks = lance_count,
                    "Tantivy index empty — rebuilding from LanceDB"
                );
                if let Err(e) = engine.rebuild_text_index().await {
                    tracing::error!(error = %e, "Failed to rebuild Tantivy index from LanceDB");
                }
            }
        }

        Ok(engine)
    }

    /// Whether the search models are loaded (search and indexing work).
    pub fn has_search_models(&self) -> bool {
        self.embeddings.is_some()
    }

    /// Attach models loaded with [`SearchModels::load`]. Fails when the
    /// embedding dimension differs from the vector store's.
    pub fn attach_search_models(&mut self, models: SearchModels) -> Result<()> {
        let dimension = models.dimension();
        if dimension != self.config.embedding.dimension {
            anyhow::bail!(
                "embedding model produces {dimension}-dimensional vectors but the index uses {}",
                self.config.embedding.dimension
            );
        }
        self.embeddings = Some(models.embeddings);
        self.reranker = models.reranker;
        tracing::info!(
            dimension,
            reranker = self.reranker.is_some(),
            "Search models attached"
        );
        Ok(())
    }

    fn require_embeddings(&self) -> Result<&dyn EmbeddingModel> {
        self.embeddings
            .as_deref()
            .ok_or_else(|| SearchModelsMissing.into())
    }

    /// Ingest a document from raw content
    pub async fn add_document(
        &mut self,
        content: &str,
        _format: DocumentFormat,
        metadata: HashMap<String, String>,
        citation: Citation,
    ) -> Result<Vec<Uuid>> {
        self.require_embeddings()?;
        let title = metadata
            .get("title")
            .cloned()
            .unwrap_or_else(|| "Untitled".to_string());
        let source = metadata
            .get("file_path")
            .or_else(|| metadata.get("source"))
            .cloned()
            .unwrap_or_default();

        // Contextual chunking: prepend document-level context to each chunk
        // before embedding for better retrieval (Anthropic's contextual retrieval approach)
        let chunks = self.chunker.chunk_with_context(content, &title, &source);
        let prepared = self.prepare_chunks(chunks, title, source, &metadata, &citation)?;
        self.store_prepared(prepared).await
    }

    /// Ingest a document from a file path.
    ///
    /// The file is parsed, chunked and embedded *before* any previously indexed
    /// chunks for the same source are removed, so a file that fails to parse or
    /// embed leaves its existing index entries intact. Once the replacement is
    /// ready, old chunks are deleted and the new ones inserted, which keeps
    /// re-indexing idempotent (no duplicate copies of the same file).
    pub async fn add_document_from_file(
        &mut self,
        path: &Path,
        metadata: HashMap<String, String>,
    ) -> Result<Vec<Uuid>> {
        self.require_embeddings()?;
        let source = normalize_source_path(path);

        let parsed = self.parser.parse_file(path)?;

        let mut merged_metadata = parsed.metadata;
        for (k, v) in metadata {
            merged_metadata.insert(k, v);
        }
        // Ensure file_path in metadata matches the canonical source used for
        // deletion below. This prevents mismatches if the caller passes a
        // differently-formatted path string.
        merged_metadata.insert("file_path".to_string(), source.clone());

        let citation = Citation {
            title: parsed.title.clone(),
            source: source.clone(),
            ..Citation::default()
        };
        let title = merged_metadata
            .get("title")
            .cloned()
            .unwrap_or_else(|| parsed.title.clone());

        // Use structure-aware chunking for documents with structured data (PDF forms,
        // spreadsheet tables, relationships). Keeps related data together as atomic units
        // instead of scattering them across naive sliding-window chunks.
        let chunks = if parsed.structured_sections.is_empty() {
            self.chunker
                .chunk_with_context(&parsed.content, &title, &source)
        } else {
            self.chunker
                .chunk_structured(&parsed.structured_sections, &title, &source)
        };
        let prepared =
            self.prepare_chunks(chunks, title, source.clone(), &merged_metadata, &citation)?;

        // Replacement is fully prepared — now drop the previous version of this file.
        self.remove_source_chunks(&source).await?;

        self.store_prepared(prepared).await
    }

    /// Remove every stored chunk for `source` from both LanceDB and Tantivy.
    async fn remove_source_chunks(&mut self, source: &str) -> Result<()> {
        if let Err(e) = self.store.delete_by_source(source).await {
            tracing::warn!(
                source = source,
                error = %e,
                "Failed to delete previous chunks from LanceDB before re-indexing"
            );
        }
        self.text_search.delete_by_source(source)?;
        self.text_search.commit()?;
        Ok(())
    }

    /// Embed chunks and build the storage records for one document without
    /// touching the stores. Fails (with nothing written) if embedding fails.
    fn prepare_chunks(
        &self,
        chunks: Vec<ContextualChunkResult>,
        title: String,
        source: String,
        metadata: &HashMap<String, String>,
        citation: &Citation,
    ) -> Result<PreparedDocument> {
        let space_id = metadata.get("space_id").cloned().unwrap_or_default();
        if chunks.is_empty() {
            return Ok(PreparedDocument {
                title,
                space_id,
                records: Vec::new(),
                fts_batch: Vec::new(),
                chunk_ids: Vec::new(),
            });
        }

        // Embed the contextualized text (with document context prefix) for better
        // vector representation
        let chunk_texts: Vec<&str> = chunks
            .iter()
            .map(|c| c.contextualized_text.as_str())
            .collect();
        let embeddings = self.require_embeddings()?.embed_documents(&chunk_texts)?;

        let doc_id = Uuid::new_v4();
        let metadata_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());
        let now = chrono::Utc::now().timestamp();

        let mut records = Vec::with_capacity(chunks.len());
        let mut fts_batch = Vec::with_capacity(chunks.len());
        let mut chunk_ids = Vec::with_capacity(chunks.len());

        for (i, (chunk, embedding)) in chunks.iter().zip(embeddings).enumerate() {
            let chunk_id = chunk.id;
            chunk_ids.push(chunk_id);

            let mut per_chunk_meta = metadata.clone();
            if let Some(heading) = &chunk.heading {
                per_chunk_meta.insert("chunk_type".to_string(), heading.clone());
                per_chunk_meta.insert("heading".to_string(), heading.clone());
            }
            insert_page_metadata(&mut per_chunk_meta, chunk.page);
            // Extract structured fields (emails, phones, etc.) at ingest time
            for (k, v) in extract_structured_fields(&chunk.text) {
                per_chunk_meta.insert(k, v);
            }
            let per_chunk_meta_json =
                serde_json::to_string(&per_chunk_meta).unwrap_or_else(|_| metadata_json.clone());

            // Citation is stored per chunk so its page survives into search results.
            let chunk_citation = Citation {
                page_numbers: chunk.page.map(|p| p.to_string()),
                ..citation.clone()
            };
            let citation_json =
                serde_json::to_string(&chunk_citation).unwrap_or_else(|_| "{}".to_string());

            // Store the original text (without context prefix) for display
            records.push(ChunkRecord {
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
                citation_json,
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

        Ok(PreparedDocument {
            title,
            space_id,
            records,
            fts_batch,
            chunk_ids,
        })
    }

    /// Write prepared records to LanceDB and Tantivy.
    async fn store_prepared(&mut self, prepared: PreparedDocument) -> Result<Vec<Uuid>> {
        if prepared.chunk_ids.is_empty() {
            return Ok(Vec::new());
        }

        self.store
            .upsert_chunks(prepared.records)
            .await
            .context("Failed to store chunks in LanceDB")?;
        self.text_search.index_chunks_batch(&prepared.fts_batch)?;
        self.text_search.commit()?;

        tracing::info!(
            "Ingested document '{}' ({} chunks) into space '{}'",
            prepared.title,
            prepared.chunk_ids.len(),
            prepared.space_id,
        );

        Ok(prepared.chunk_ids)
    }

    /// Search with hybrid vector + FTS fusion
    pub async fn search(&self, query: &str, k: usize) -> Result<Vec<SimpleSearchResult>> {
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
                let heading = r
                    .metadata
                    .get("heading")
                    .filter(|h| !h.trim().is_empty())
                    .cloned();
                SimpleSearchResult {
                    id: r.id,
                    score: r.score,
                    text: r.snippet.clone(),
                    metadata: r.metadata,
                    title,
                    source,
                    heading,
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
        // Checked before decomposition: sub-query failures are swallowed
        // there, which would turn "not installed" into "no results".
        self.require_embeddings()?;
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
        let query_embedding = self.require_embeddings()?.embed_query(query)?;

        // Build filter predicate for LanceDB
        let lance_filter = filter.as_ref().and_then(|f| f.to_lance_predicate());

        // Extract source filter for FTS consistency
        let source_filter = filter.as_ref().and_then(|f| f.source_path.as_deref());

        // Vector search via LanceDB
        let vector_hits = self
            .store
            .vector_search(&query_embedding, candidate_count, lance_filter.as_deref())
            .await?;

        let vector_results: Vec<(String, f32)> = vector_hits
            .iter()
            .map(|h| (h.id.clone(), h.score))
            .collect();

        // Full-text search via Tantivy — use SAME candidate count for balanced fusion
        let fts_results =
            self.text_search
                .search_filtered(query, candidate_count, source_filter)?;

        // Log source diversity at each stage for diagnostics
        let vector_sources: std::collections::HashSet<&str> =
            vector_hits.iter().map(|h| h.source.as_str()).collect();
        tracing::info!(
            query = query,
            candidate_count = candidate_count,
            vector_hits = vector_hits.len(),
            vector_unique_sources = vector_sources.len(),
            fts_hits = fts_results.len(),
            vector_sources = ?vector_sources,
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
                let mut citation: Citation =
                    serde_json::from_str(&hit.citation_json).unwrap_or_default();

                let mut full_metadata = metadata;
                full_metadata.insert("doc_id".to_string(), hit.doc_id.clone());
                full_metadata.insert("chunk_index".to_string(), hit.chunk_index.to_string());
                full_metadata.insert("source_file".to_string(), hit.source.clone());
                full_metadata.insert("space_id".to_string(), hit.space_id.clone());
                if !hit.heading.trim().is_empty() {
                    full_metadata
                        .entry("heading".to_string())
                        .or_insert_with(|| hit.heading.clone());
                }
                if citation.page_numbers.is_none() {
                    citation.page_numbers = page_numbers_from_metadata(&full_metadata);
                }

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

        // Log source diversity of built results
        {
            let built_sources: std::collections::HashSet<&str> = results
                .iter()
                .filter_map(|r| r.metadata.get("source_file").map(|s| s.as_str()))
                .collect();
            tracing::info!(
                built_results = results.len(),
                unique_sources = built_sources.len(),
                sources = ?built_sources,
                "Results built from fused candidates"
            );
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
                        let rerank_scores: HashMap<String, f32> = reranked.into_iter().collect();

                        // Update scores where reranking succeeded; keep original score
                        // for any candidates the cross-encoder couldn't tokenize.
                        for result in &mut results {
                            if let Some(&new_score) = rerank_scores.get(&result.id.to_string()) {
                                result.score = new_score;
                            }
                        }
                        results.sort_by(|a, b| {
                            b.score
                                .partial_cmp(&a.score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Reranking failed, using fusion scores: {}", e);
                    }
                }
            }
        }

        // MMR diversity: penalize repeated source files to spread results across documents.
        // Each additional chunk from the same source gets score *= lambda^count.
        // This naturally balances depth vs diversity without an artificial hard cap.
        Self::apply_mmr_diversity(&mut results, 0.5);

        // Log final diversity before truncation
        {
            let final_sources: std::collections::HashSet<&str> = results
                .iter()
                .filter_map(|r| r.metadata.get("source_file").map(|s| s.as_str()))
                .collect();
            tracing::info!(
                post_mmr_results = results.len(),
                unique_sources = final_sources.len(),
                scores = ?results.iter().map(|r| format!("{:.3}", r.score)).collect::<Vec<_>>(),
                sources = ?results.iter().filter_map(|r| r.metadata.get("source_file").map(|s| s.as_str())).collect::<Vec<_>>(),
                "Post-MMR+cap diversity"
            );
        }

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

    /// Delete all chunks belonging to a specific document (by doc_id).
    /// Removes from both LanceDB and Tantivy.
    pub async fn delete_by_doc_id(&mut self, doc_id: &str) -> Result<usize> {
        // Get all chunk IDs for this doc so we can remove from Tantivy
        let predicate = format!("doc_id = '{}'", doc_id.replace('\'', "''"));
        let chunks = self.store.list_chunks(Some(&predicate), 100_000).await?;

        for chunk in &chunks {
            let _ = self.text_search.delete_by_id(&chunk.id);
        }
        if !chunks.is_empty() {
            self.text_search.commit()?;
        }

        let deleted = self.store.delete_by_doc_id(doc_id).await?;
        tracing::info!(doc_id = %doc_id, deleted = deleted, "Deleted document by doc_id");
        Ok(deleted)
    }

    /// Delete all documents from a specific source/folder.
    /// The source path is normalized the same way as during indexing so that
    /// Windows backslash / mixed-case paths always match.
    pub async fn delete_by_source(&mut self, source: &str) -> Result<usize> {
        let normalized = normalize_source_path(Path::new(source));
        let deleted = self.store.delete_by_source(&normalized).await?;
        self.text_search.delete_by_source(&normalized)?;
        self.text_search.commit()?;
        Ok(deleted)
    }

    /// Delete all documents whose source starts with the given folder path.
    /// Normalizes the prefix and uses starts_with matching so that a folder
    /// path matches all files indexed under it.
    pub async fn delete_by_source_prefix(&mut self, folder: &str) -> Result<usize> {
        let normalized = normalize_source_path(Path::new(folder));
        let deleted = self.store.delete_by_source_prefix(&normalized).await?;
        self.text_search.delete_by_source_prefix(&normalized)?;
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
            self.config.embedding.dimension.to_string(),
        );
        stats.insert(
            "search_models_loaded".to_string(),
            self.has_search_models().to_string(),
        );
        stats.insert(
            "data_dir".to_string(),
            self.config.data_dir.display().to_string(),
        );

        Ok(stats)
    }

    /// Whether at least one indexed chunk was produced from `path`.
    ///
    /// The path is compared after the same normalization applied at ingest
    /// time (forward slashes; lower-cased on Windows), and also verbatim for
    /// documents added through `add_document` with a caller-supplied
    /// `file_path`. Used to gate file access: only files the user indexed may
    /// be read back for display.
    pub async fn is_indexed_source(&self, path: &Path) -> Result<bool> {
        let normalized = normalize_source_path(path);
        let verbatim = path.display().to_string();
        let mut candidates = vec![normalized];
        if !candidates.contains(&verbatim) {
            candidates.push(verbatim);
        }
        for candidate in candidates {
            let predicate = format!("source = '{}'", candidate.replace('\'', "''"));
            if !self
                .store
                .list_chunks(Some(&predicate), 1)
                .await?
                .is_empty()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Count distinct documents in the index
    pub async fn count_documents(&self) -> Result<usize> {
        self.store.count_documents().await
    }

    /// Get document metadata for corpus stats: (doc_id, title, source)
    pub async fn get_document_info(&self) -> Result<Vec<(String, String, String)>> {
        self.store.get_document_info().await
    }

    /// One row per indexed document with its source path, space and chunk count.
    pub async fn document_sources(&self) -> Result<Vec<crate::storage::DocumentSourceRow>> {
        self.store.document_sources().await
    }

    /// Stored source paths that `path` refers to.
    ///
    /// A full path is normalised like the indexer normalises it and must match
    /// a stored source exactly. A bare file name (no directory separator)
    /// matches every indexed file with that name, so callers can detect
    /// ambiguity. Returns an empty list when nothing is indexed under `path`.
    pub async fn find_indexed_sources(&self, path: &str) -> Result<Vec<String>> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let normalized = normalize_source_path(Path::new(trimmed));
        let predicate = format!("source = '{}'", normalized.replace('\'', "''"));
        let exact = self.store.list_chunks(Some(&predicate), 1).await?;
        if let Some(hit) = exact.into_iter().next() {
            return Ok(vec![hit.source]);
        }
        if normalized.contains('/') {
            return Ok(Vec::new());
        }
        let wanted = normalized.to_lowercase();
        let mut matches: Vec<String> = self
            .store
            .document_sources()
            .await?
            .into_iter()
            .map(|row| row.source)
            .filter(|source| {
                !source.contains("://")
                    && source
                        .rsplit('/')
                        .next()
                        .map(|name| name.to_lowercase() == wanted)
                        .unwrap_or(false)
            })
            .collect();
        matches.sort();
        matches.dedup();
        Ok(matches)
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

        let hits = self.store.list_chunks(predicate.as_deref(), limit).await?;

        let mut results = Vec::with_capacity(hits.len());
        for hit in hits {
            let metadata: HashMap<String, String> =
                serde_json::from_str(&hit.metadata_json).unwrap_or_default();
            let citation: Citation = serde_json::from_str(&hit.citation_json).unwrap_or_default();

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

    /// Raw LanceDB query — returns SearchHit objects without wrapping in ComprehensiveResult.
    /// Useful for ID lookups during deletion.
    pub async fn list_documents_raw(
        &self,
        predicate: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::storage::SearchHit>> {
        self.store.list_chunks(predicate, limit).await
    }

    /// The embedding model, when the search models are attached.
    pub fn embeddings(&self) -> Option<&dyn EmbeddingModel> {
        self.embeddings.as_deref()
    }

    /// A shared handle to the embedding model, for callers that embed outside the
    /// engine lock (inference is blocking; run it on a blocking thread).
    pub fn shared_embeddings(&self) -> Option<Arc<dyn EmbeddingModel>> {
        self.embeddings.clone()
    }

    /// Access to config
    pub fn config(&self) -> &RAGConfig {
        &self.config
    }

    /// Trigger index creation if needed (after large ingestion)
    pub async fn optimize(&self) -> Result<()> {
        // Compact LanceDB to remove tombstoned rows from previous deletions
        self.store.compact().await?;
        // Create vector index if enough rows exist
        self.store.create_index_if_needed().await
    }

    /// Rebuild the Tantivy full-text index from LanceDB.
    /// Used after schema migration wipes the old index, or to repair inconsistencies.
    pub async fn rebuild_text_index(&mut self) -> Result<()> {
        self.text_search.clear()?;

        let all_chunks = self.store.list_chunks(None, 1_000_000).await?;
        let total = all_chunks.len();

        if total == 0 {
            tracing::info!("No chunks in LanceDB — nothing to rebuild");
            return Ok(());
        }

        let batch: Vec<(String, String, String, String)> = all_chunks
            .into_iter()
            .map(|hit| (hit.id, hit.text, hit.title, hit.source))
            .collect();

        self.text_search.index_chunks_batch(&batch)?;
        self.text_search.commit()?;

        let indexed = self.text_search.count().unwrap_or(0);
        tracing::info!(
            total_chunks = total,
            indexed = indexed,
            "Tantivy index rebuilt from LanceDB"
        );
        Ok(())
    }

    /// Remove near-duplicate results using Jaccard similarity on word sets.
    /// Only deduplicates chunks from the SAME source file (overlapping windows).
    /// Chunks from different files are never merged — even if they have similar
    /// boilerplate (e.g. invoices with the same company header).
    fn deduplicate_results(results: &mut Vec<ComprehensiveResult>, threshold: f32) {
        use std::collections::HashSet;
        let word_sets: Vec<HashSet<&str>> = results
            .iter()
            .map(|r| r.snippet.split_whitespace().collect::<HashSet<_>>())
            .collect();

        let sources: Vec<String> = results
            .iter()
            .map(|r| r.metadata.get("source_file").cloned().unwrap_or_default())
            .collect();

        let mut keep = Vec::new();
        for i in 0..results.len() {
            let mut is_dup = false;
            for &j in &keep {
                // Only dedup within the same source file
                if sources[i] != sources[j] {
                    continue;
                }
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
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Hard cap on results per source file to guarantee diversity across documents.
    /// After scoring and MMR, retain at most `max_per_source` chunks from any single file.
    /// Maximal Marginal Relevance — diminishing returns per source file.
    /// Maximal Marginal Relevance — diminishing returns per source file.
    /// Each additional chunk from the same source gets score *= lambda^count.
    /// This naturally balances depth (multiple chunks from one file) vs diversity
    /// (spreading across files) without any hard cap.
    fn apply_mmr_diversity(results: &mut Vec<ComprehensiveResult>, lambda: f32) {
        let mut source_seen: HashMap<String, u32> = HashMap::new();
        for result in results.iter_mut() {
            let source = result
                .metadata
                .get("source_file")
                .cloned()
                .unwrap_or_default();
            let count = source_seen.entry(source).or_insert(0);
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

#[cfg(test)]
mod page_metadata_tests {
    use super::*;

    #[test]
    fn paged_chunk_metadata_round_trips_to_page_label() {
        let mut meta = HashMap::new();
        insert_page_metadata(&mut meta, Some(7));
        assert_eq!(meta.get("page").map(String::as_str), Some("7"));
        assert_eq!(meta.get("page_start").map(String::as_str), Some("7"));
        assert_eq!(meta.get("page_end").map(String::as_str), Some("7"));
        assert_eq!(page_numbers_from_metadata(&meta).as_deref(), Some("7"));
    }

    #[test]
    fn unpaged_chunk_has_no_page_label() {
        let mut meta = HashMap::new();
        insert_page_metadata(&mut meta, None);
        assert!(meta.is_empty());
        assert_eq!(page_numbers_from_metadata(&meta), None);
    }

    #[test]
    fn page_range_and_legacy_page_key() {
        let mut meta = HashMap::new();
        meta.insert("page_start".to_string(), "3".to_string());
        meta.insert("page_end".to_string(), "5".to_string());
        assert_eq!(page_numbers_from_metadata(&meta).as_deref(), Some("3-5"));

        let mut legacy = HashMap::new();
        legacy.insert("page".to_string(), "12".to_string());
        assert_eq!(page_numbers_from_metadata(&legacy).as_deref(), Some("12"));

        let mut blank = HashMap::new();
        blank.insert("page".to_string(), "  ".to_string());
        assert_eq!(page_numbers_from_metadata(&blank), None);
    }
}
