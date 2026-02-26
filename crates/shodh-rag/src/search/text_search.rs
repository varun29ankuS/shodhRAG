use anyhow::{Context, Result};
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{self, Schema, STORED, STRING, TEXT, Value as TantivyValue};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

pub struct TextSearch {
    index: Index,
    reader: IndexReader,
    writer: parking_lot::Mutex<IndexWriter>,
    id_field: schema::Field,
    text_field: schema::Field,
    title_field: schema::Field,
    source_field: schema::Field,
}

impl TextSearch {
    /// Build the canonical schema. `id` must be STRING (indexed, not tokenized)
    /// so that `delete_term` and `TermQuery` lookups work correctly.
    fn build_schema() -> (Schema, schema::Field, schema::Field, schema::Field, schema::Field) {
        let mut sb = Schema::builder();
        let id_field = sb.add_text_field("id", STRING | STORED);
        let text_field = sb.add_text_field("text", TEXT | STORED);
        let title_field = sb.add_text_field("title", TEXT);
        let source_field = sb.add_text_field("source", TEXT | STORED);
        (sb.build(), id_field, text_field, title_field, source_field)
    }

    /// Check whether an existing index has `id` indexed (STRING).
    /// Old indices created `id` as STORED-only, which makes delete_term a no-op.
    fn needs_schema_migration(index: &Index) -> bool {
        let schema = index.schema();
        let id_field = match schema.get_field("id") {
            Ok(f) => f,
            Err(_) => return true,
        };
        let entry = schema.get_field_entry(id_field);
        // If the id field has no indexing (STORED-only), we need to rebuild.
        !entry.is_indexed()
    }

    pub fn new(path: &str) -> Result<Self> {
        let index_path = Path::new(path).join("tantivy_index");
        std::fs::create_dir_all(&index_path).ok();

        let (schema, id_field, text_field, title_field, source_field) = Self::build_schema();

        let needs_rebuild = {
            let dir = tantivy::directory::MmapDirectory::open(&index_path)?;
            if Index::exists(&dir)? {
                let existing = Index::open_in_dir(&index_path)?;
                let migrate = Self::needs_schema_migration(&existing);
                drop(existing);
                migrate
            } else {
                false
            }
        }; // dir dropped here — releases mmap handles on Windows

        let index = if needs_rebuild {
            tracing::warn!(
                "Tantivy index has STORED-only id field — rebuilding with STRING|STORED \
                 so deletions work. Existing full-text data will be re-indexed on next ingest."
            );
            std::fs::remove_dir_all(&index_path).ok();
            std::fs::create_dir_all(&index_path)?;
            Index::create_in_dir(&index_path, schema.clone())?
        } else {
            let dir = tantivy::directory::MmapDirectory::open(&index_path)?;
            if Index::exists(&dir)? {
                Index::open_in_dir(&index_path)?
            } else {
                Index::create_in_dir(&index_path, schema.clone())?
            }
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context("Failed to create Tantivy reader")?;

        let writer = index
            .writer(50_000_000)
            .context("Failed to create Tantivy writer")?;

        Ok(Self {
            index,
            reader,
            writer: parking_lot::Mutex::new(writer),
            id_field,
            text_field,
            title_field,
            source_field,
        })
    }

    pub fn index_chunk(&self, id: &str, text: &str, title: &str, source: &str) -> Result<()> {
        let writer = self.writer.lock();
        writer.add_document(doc!(
            self.id_field => id,
            self.text_field => text,
            self.title_field => title,
            self.source_field => source,
        ))?;
        Ok(())
    }

    pub fn index_chunks_batch(
        &self,
        chunks: &[(String, String, String, String)],
    ) -> Result<()> {
        let writer = self.writer.lock();
        for (id, text, title, source) in chunks {
            writer.add_document(doc!(
                self.id_field => id.as_str(),
                self.text_field => text.as_str(),
                self.title_field => title.as_str(),
                self.source_field => source.as_str(),
            ))?;
        }
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        let mut writer = self.writer.lock();
        writer.commit().context("Tantivy commit failed")?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn search(&self, query: &str, k: usize) -> Result<Vec<(String, f32)>> {
        self.search_filtered(query, k, None)
    }

    /// Search with optional source path filter for consistency with vector search filtering
    pub fn search_filtered(
        &self,
        query: &str,
        k: usize,
        source_filter: Option<&str>,
    ) -> Result<Vec<(String, f32)>> {
        let searcher = self.reader.searcher();
        let query_parser =
            QueryParser::for_index(&self.index, vec![self.text_field, self.title_field]);

        let parsed_query = match query_parser.parse_query(query) {
            Ok(q) => q,
            Err(_) => {
                let escaped_query = query.replace('"', "");
                let fallback_parser = QueryParser::for_index(&self.index, vec![self.text_field]);
                fallback_parser.parse_query(&format!("\"{}\"", escaped_query))?
            }
        };

        // Fetch extra candidates when filtering to compensate for post-filter reduction.
        // Without this, source-filtered queries return fewer results than vector search,
        // causing asymmetric fusion.
        let fetch_limit = if source_filter.is_some() { k * 3 } else { k };
        let top_docs = searcher.search(&parsed_query, &TopDocs::with_limit(fetch_limit))?;

        let mut results = Vec::with_capacity(k);
        for (score, doc_address) in top_docs {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                // Apply source filter if provided
                if let Some(filter_source) = source_filter {
                    let doc_source = doc
                        .get_first(self.source_field)
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !doc_source.contains(filter_source) {
                        continue;
                    }
                }

                if let Some(id_val) = doc.get_first(self.id_field) {
                    if let Some(id_text) = id_val.as_str() {
                        results.push((id_text.to_string(), score));
                        if results.len() >= k {
                            break;
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Retrieve the stored text for a given chunk ID
    pub fn get_text_by_id(&self, id: &str) -> Result<Option<String>> {
        let searcher = self.reader.searcher();
        let term = tantivy::Term::from_field_text(self.id_field, id);
        let term_query = tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let top_docs = searcher.search(&term_query, &TopDocs::with_limit(1))?;
        if let Some((_score, addr)) = top_docs.first() {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(*addr) {
                if let Some(text_val) = doc.get_first(self.text_field) {
                    return Ok(text_val.as_str().map(|s| s.to_string()));
                }
            }
        }
        Ok(None)
    }

    pub fn delete_by_id(&self, id: &str) -> Result<()> {
        let writer = self.writer.lock();
        let term = tantivy::Term::from_field_text(self.id_field, id);
        writer.delete_term(term);
        Ok(())
    }

    pub fn delete_by_source(&self, source: &str) -> Result<()> {
        self.delete_matching_source(source, false)
    }

    /// Delete all documents whose source starts with the given prefix.
    pub fn delete_by_source_prefix(&self, prefix: &str) -> Result<()> {
        self.delete_matching_source(prefix, true)
    }

    fn delete_matching_source(&self, source: &str, prefix_match: bool) -> Result<()> {
        // Reload reader first to get the latest committed state
        self.reader.reload().ok();
        let searcher = self.reader.searcher();
        let mut writer = self.writer.lock();
        let mut deleted_count = 0usize;

        // Iterate all segments to find matching docs by source field
        for segment_reader in searcher.segment_readers() {
            let store_reader = segment_reader.get_store_reader(64)?;
            for doc_id in 0..segment_reader.max_doc() {
                if segment_reader.is_deleted(doc_id) {
                    continue;
                }
                if let Ok(doc) = store_reader.get::<TantivyDocument>(doc_id) {
                    if let Some(source_val) = doc.get_first(self.source_field) {
                        if let Some(source_text) = source_val.as_str() {
                            let matches = if prefix_match {
                                source_text.starts_with(source)
                            } else {
                                source_text == source
                            };
                            if matches {
                                if let Some(id_val) = doc.get_first(self.id_field) {
                                    if let Some(id_text) = id_val.as_str() {
                                        let term = tantivy::Term::from_field_text(self.id_field, id_text);
                                        writer.delete_term(term);
                                        deleted_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Commit deletions and reload reader immediately so subsequent
        // searches never return the deleted documents.
        if deleted_count > 0 {
            writer.commit().context("Tantivy commit after delete failed")?;
            self.reader.reload()?;
            tracing::info!(
                source = %source,
                prefix_match = prefix_match,
                deleted = deleted_count,
                "Tantivy: deleted and committed documents"
            );
        }

        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        let mut writer = self.writer.lock();
        writer.delete_all_documents()?;
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn count(&self) -> Result<usize> {
        let searcher = self.reader.searcher();
        Ok(searcher.num_docs() as usize)
    }

    /// Returns true if the index is empty (e.g. after schema migration).
    /// The caller should rebuild it from LanceDB if there are documents in the vector store.
    pub fn is_empty(&self) -> bool {
        self.count().unwrap_or(0) == 0
    }
}
