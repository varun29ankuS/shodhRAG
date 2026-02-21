use anyhow::{Context, Result};
use arrow_array::{
    Array, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
    FixedSizeListArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;

use crate::types::ChunkRecord;

pub struct LanceStore {
    db: lancedb::Connection,
    dimension: usize,
    table_name: String,
}

impl LanceStore {
    pub async fn new(path: &str, dimension: usize) -> Result<Self> {
        std::fs::create_dir_all(path).ok();
        let db = lancedb::connect(path)
            .execute()
            .await
            .context("Failed to connect to LanceDB")?;

        let store = Self {
            db,
            dimension,
            table_name: "documents".to_string(),
        };

        store.ensure_table().await?;
        Ok(store)
    }

    fn schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("doc_id", DataType::Utf8, false),
            Field::new("chunk_index", DataType::UInt32, false),
            Field::new("text", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
            Field::new("heading", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    self.dimension as i32,
                ),
                true,
            ),
            Field::new("space_id", DataType::Utf8, false),
            Field::new("metadata_json", DataType::Utf8, false),
            Field::new("citation_json", DataType::Utf8, false),
            Field::new("created_at", DataType::Int64, false),
        ]))
    }

    async fn ensure_table(&self) -> Result<()> {
        let names = self.db.table_names().execute().await?;
        if !names.contains(&self.table_name) {
            // Create with a single empty-ish seed record, then delete it
            let schema = self.schema();
            let seed_vec = vec![0.0f32; self.dimension];
            let values = Float32Array::from(seed_vec);
            let vector_field = Field::new("item", DataType::Float32, true);
            let vector_array = FixedSizeListArray::new(
                Arc::new(vector_field),
                self.dimension as i32,
                Arc::new(values) as Arc<dyn Array>,
                None,
            );

            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(StringArray::from(vec!["__seed__"])) as Arc<dyn Array>,
                    Arc::new(StringArray::from(vec!["__seed__"])),
                    Arc::new(UInt32Array::from(vec![0u32])),
                    Arc::new(StringArray::from(vec![""])),
                    Arc::new(StringArray::from(vec![""])),
                    Arc::new(StringArray::from(vec![""])),
                    Arc::new(StringArray::from(vec![""])),
                    Arc::new(vector_array) as Arc<dyn Array>,
                    Arc::new(StringArray::from(vec![""])),
                    Arc::new(StringArray::from(vec!["{}"])),
                    Arc::new(StringArray::from(vec!["{}"])),
                    Arc::new(Int64Array::from(vec![0i64])),
                ],
            )
            .context("Failed to create seed RecordBatch")?;

            let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
            self.db
                .create_table(&self.table_name, Box::new(batches))
                .execute()
                .await
                .context("Failed to create documents table")?;

            // Remove seed record
            let table = self.db.open_table(&self.table_name).execute().await?;
            table.delete("id = '__seed__'").await.ok();
        }
        Ok(())
    }

    pub async fn upsert_chunks(&self, chunks: Vec<ChunkRecord>) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }

        let table = self
            .db
            .open_table(&self.table_name)
            .execute()
            .await
            .context("Failed to open documents table")?;

        let len = chunks.len();
        let schema = self.schema();

        let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
        let doc_ids: Vec<&str> = chunks.iter().map(|c| c.doc_id.as_str()).collect();
        let chunk_indices: Vec<u32> = chunks.iter().map(|c| c.chunk_index).collect();
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let titles: Vec<&str> = chunks.iter().map(|c| c.title.as_str()).collect();
        let sources: Vec<&str> = chunks.iter().map(|c| c.source.as_str()).collect();
        let headings: Vec<&str> = chunks.iter().map(|c| c.heading.as_str()).collect();
        let space_ids: Vec<&str> = chunks.iter().map(|c| c.space_id.as_str()).collect();
        let metadata_jsons: Vec<&str> = chunks.iter().map(|c| c.metadata_json.as_str()).collect();
        let citation_jsons: Vec<&str> = chunks.iter().map(|c| c.citation_json.as_str()).collect();
        let created_ats: Vec<i64> = chunks.iter().map(|c| c.created_at).collect();

        // Build FixedSizeListArray for vectors
        let flat_vectors: Vec<f32> = chunks.iter().flat_map(|c| c.vector.iter().copied()).collect();
        let values = Float32Array::from(flat_vectors);
        let vector_field = Field::new("item", DataType::Float32, true);
        let vector_array = FixedSizeListArray::new(
            Arc::new(vector_field),
            self.dimension as i32,
            Arc::new(values) as Arc<dyn Array>,
            None,
        );

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)) as Arc<dyn Array>,
                Arc::new(StringArray::from(doc_ids)),
                Arc::new(UInt32Array::from(chunk_indices)),
                Arc::new(StringArray::from(texts)),
                Arc::new(StringArray::from(titles)),
                Arc::new(StringArray::from(sources)),
                Arc::new(StringArray::from(headings)),
                Arc::new(vector_array) as Arc<dyn Array>,
                Arc::new(StringArray::from(space_ids)),
                Arc::new(StringArray::from(metadata_jsons)),
                Arc::new(StringArray::from(citation_jsons)),
                Arc::new(Int64Array::from(created_ats)),
            ],
        )
        .context("Failed to create RecordBatch")?;

        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table
            .add(Box::new(reader))
            .execute()
            .await
            .context("Failed to insert chunks")?;

        tracing::debug!("Inserted {} chunks into LanceDB", len);
        Ok(())
    }

    pub async fn vector_search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        let table = self.db.open_table(&self.table_name).execute().await?;

        let mut query_builder = table.query().nearest_to(query)?;
        query_builder = query_builder
            .distance_type(lancedb::DistanceType::Cosine)
            .limit(k);

        if let Some(predicate) = filter {
            query_builder = query_builder.only_if(predicate);
        }

        let results = query_builder
            .execute()
            .await
            .context("LanceDB vector search failed")?;

        let batches: Vec<RecordBatch> = futures::TryStreamExt::try_collect(results).await?;
        Ok(extract_hits_from_batches(&batches, 0.0))
    }

    pub async fn delete_by_doc_id(&self, doc_id: &str) -> Result<usize> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        let count_before = table.count_rows(None).await.unwrap_or(0);
        let predicate = format!("doc_id = '{}'", doc_id.replace('\'', "''"));
        table.delete(&predicate).await?;
        let count_after = table.count_rows(None).await.unwrap_or(0);
        Ok(count_before - count_after)
    }

    pub async fn delete_by_source(&self, source: &str) -> Result<usize> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        let count_before = table.count_rows(None).await.unwrap_or(0);
        let predicate = format!("source = '{}'", source.replace('\'', "''"));
        table.delete(&predicate).await?;
        let count_after = table.count_rows(None).await.unwrap_or(0);
        Ok(count_before - count_after)
    }

    /// Delete all chunks belonging to a specific space.
    pub async fn delete_by_space_id(&self, space_id: &str) -> Result<usize> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        let count_before = table.count_rows(None).await.unwrap_or(0);
        let predicate = format!("space_id = '{}'", space_id.replace('\'', "''"));
        table.delete(&predicate).await?;
        let count_after = table.count_rows(None).await.unwrap_or(0);
        tracing::info!(
            space_id = %space_id,
            deleted = count_before - count_after,
            "Deleted chunks by space_id"
        );
        Ok(count_before - count_after)
    }

    pub async fn clear(&self) -> Result<()> {
        let names = self.db.table_names().execute().await?;
        if names.contains(&self.table_name) {
            self.db.drop_table(&self.table_name, &[]).await?;
        }
        self.ensure_table().await?;
        Ok(())
    }

    pub async fn count(&self) -> Result<usize> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        let count = table.count_rows(None).await?;
        Ok(count)
    }

    /// Count distinct documents (unique doc_ids) in the store.
    pub async fn count_documents(&self) -> Result<usize> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        let results = table
            .query()
            .select(lancedb::query::Select::columns(&["doc_id"]))
            .execute()
            .await
            .context("Failed to query doc_ids")?;

        let batches: Vec<RecordBatch> = futures::TryStreamExt::try_collect(results).await?;
        let mut doc_ids = std::collections::HashSet::new();

        for batch in &batches {
            if let Some(col) = batch.column_by_name("doc_id").and_then(|c| c.as_any().downcast_ref::<StringArray>()) {
                for i in 0..col.len() {
                    let val = col.value(i);
                    if !val.is_empty() && val != "__seed__" {
                        doc_ids.insert(val.to_string());
                    }
                }
            }
        }

        Ok(doc_ids.len())
    }

    /// Get distinct document metadata: (doc_id, title, source, file_extension) for corpus stats.
    pub async fn get_document_info(&self) -> Result<Vec<(String, String, String)>> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        let results = table
            .query()
            .select(lancedb::query::Select::columns(&["doc_id", "title", "source"]))
            .execute()
            .await
            .context("Failed to query document info")?;

        let batches: Vec<RecordBatch> = futures::TryStreamExt::try_collect(results).await?;
        let mut seen = std::collections::HashSet::new();
        let mut docs = Vec::new();

        for batch in &batches {
            let doc_ids = batch.column_by_name("doc_id").and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let titles = batch.column_by_name("title").and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let sources = batch.column_by_name("source").and_then(|c| c.as_any().downcast_ref::<StringArray>());

            if let (Some(doc_ids), Some(titles), Some(sources)) = (doc_ids, titles, sources) {
                for i in 0..batch.num_rows() {
                    let did = doc_ids.value(i);
                    if !did.is_empty() && did != "__seed__" && seen.insert(did.to_string()) {
                        docs.push((
                            did.to_string(),
                            titles.value(i).to_string(),
                            sources.value(i).to_string(),
                        ));
                    }
                }
            }
        }

        Ok(docs)
    }

    /// List all chunks matching an optional predicate (no vector search).
    /// This is the correct way to enumerate documents — NOT search_comprehensive("").
    pub async fn list_chunks(
        &self,
        predicate: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let table = self.db.open_table(&self.table_name).execute().await?;

        let mut query = table.query();
        if let Some(pred) = predicate {
            query = query.only_if(pred);
        }
        query = query.limit(limit);

        let results = query
            .execute()
            .await
            .context("LanceDB list query failed")?;

        let batches: Vec<RecordBatch> = futures::TryStreamExt::try_collect(results).await?;
        Ok(extract_hits_from_batches(&batches, 0.0))
    }

    pub async fn create_index_if_needed(&self) -> Result<()> {
        let count = self.count().await?;
        if count >= 1_000 {
            let table = self.db.open_table(&self.table_name).execute().await?;
            table
                .create_index(&["vector"], lancedb::index::Index::Auto)
                .execute()
                .await
                .context("Failed to create vector index")?;
            tracing::info!("Created IVF-PQ index on {} rows", count);
        }
        Ok(())
    }

    /// Fetch neighboring chunks (±window) for a given doc_id and chunk_index.
    /// Used for parent-child context expansion: after selecting top-k results,
    /// we expand each with adjacent chunks from the same document to provide
    /// the LLM with more surrounding context.
    pub async fn get_neighbors(
        &self,
        doc_id: &str,
        chunk_index: u32,
        window: u32,
    ) -> Result<Vec<SearchHit>> {
        let table = self.db.open_table(&self.table_name).execute().await?;

        let low = chunk_index.saturating_sub(window);
        let high = chunk_index.saturating_add(window);

        let predicate = format!(
            "doc_id = '{}' AND chunk_index >= {} AND chunk_index <= {} AND chunk_index != {}",
            doc_id.replace('\'', "''"),
            low,
            high,
            chunk_index
        );

        let results = table
            .query()
            .only_if(predicate)
            .execute()
            .await
            .context("LanceDB neighbor lookup failed")?;

        let batches: Vec<RecordBatch> = futures::TryStreamExt::try_collect(results).await?;
        let mut hits = extract_hits_from_batches(&batches, 0.0);
        // Sort by chunk_index for proper reading order
        hits.sort_by_key(|h| h.chunk_index);
        Ok(hits)
    }

    /// Look up chunks by their IDs (for FTS-only results that need full data)
    pub async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<SearchHit>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let table = self.db.open_table(&self.table_name).execute().await?;
        let mut all_hits = Vec::new();

        // Query in batches to avoid overly long SQL predicates
        for chunk in ids.chunks(50) {
            let id_list: Vec<String> = chunk
                .iter()
                .map(|id| format!("'{}'", id.replace('\'', "''")))
                .collect();
            let predicate = format!("id IN ({})", id_list.join(", "));

            let results = table
                .query()
                .only_if(predicate)
                .execute()
                .await
                .context("LanceDB ID lookup failed")?;

            let batches: Vec<RecordBatch> = futures::TryStreamExt::try_collect(results).await?;
            all_hits.extend(extract_hits_from_batches(&batches, 0.0));
        }

        Ok(all_hits)
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub doc_id: String,
    pub chunk_index: u32,
    pub text: String,
    pub title: String,
    pub source: String,
    pub heading: String,
    pub space_id: String,
    pub metadata_json: String,
    pub citation_json: String,
    pub score: f32,
}

/// Extract SearchHit records from Arrow RecordBatches.
/// Centralizes the column extraction logic used by vector_search, list_chunks,
/// get_neighbors, and get_by_ids to avoid code duplication.
fn extract_hits_from_batches(batches: &[RecordBatch], default_score: f32) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for batch in batches {
        let ids = batch.column_by_name("id").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let texts = batch.column_by_name("text").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let titles = batch.column_by_name("title").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let sources = batch.column_by_name("source").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let headings = batch.column_by_name("heading").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let doc_ids = batch.column_by_name("doc_id").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let chunk_indices = batch.column_by_name("chunk_index").and_then(|c| c.as_any().downcast_ref::<UInt32Array>());
        let metadata_jsons = batch.column_by_name("metadata_json").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let citation_jsons = batch.column_by_name("citation_json").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let space_ids = batch.column_by_name("space_id").and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let distances = batch.column_by_name("_distance").and_then(|c| c.as_any().downcast_ref::<Float32Array>());

        let (Some(ids), Some(texts), Some(titles), Some(sources)) = (ids, texts, titles, sources) else {
            continue;
        };

        for i in 0..batch.num_rows() {
            let score = if let Some(d) = distances {
                (1.0 - d.value(i)).max(0.0)
            } else {
                default_score
            };

            hits.push(SearchHit {
                id: ids.value(i).to_string(),
                doc_id: doc_ids.map(|d| d.value(i).to_string()).unwrap_or_default(),
                chunk_index: chunk_indices.map(|c| c.value(i)).unwrap_or(0),
                text: texts.value(i).to_string(),
                title: titles.value(i).to_string(),
                source: sources.value(i).to_string(),
                heading: headings.map(|h| h.value(i).to_string()).unwrap_or_default(),
                space_id: space_ids.map(|s| s.value(i).to_string()).unwrap_or_default(),
                metadata_json: metadata_jsons.map(|m| m.value(i).to_string()).unwrap_or_else(|| "{}".to_string()),
                citation_json: citation_jsons.map(|c| c.value(i).to_string()).unwrap_or_else(|| "{}".to_string()),
                score,
            });
        }
    }
    hits
}
