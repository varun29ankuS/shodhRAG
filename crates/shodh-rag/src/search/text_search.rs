use anyhow::{Context, Result};
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{self, Schema, STORED, TEXT, Value as TantivyValue};
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
    pub fn new(path: &str) -> Result<Self> {
        let index_path = Path::new(path).join("tantivy_index");
        std::fs::create_dir_all(&index_path).ok();

        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("id", STORED);
        let text_field = schema_builder.add_text_field("text", TEXT | STORED);
        let title_field = schema_builder.add_text_field("title", TEXT);
        let source_field = schema_builder.add_text_field("source", TEXT | STORED);
        let schema = schema_builder.build();

        let dir = tantivy::directory::MmapDirectory::open(&index_path)?;
        let index = if Index::exists(&dir)? {
            Index::open_in_dir(&index_path)?
        } else {
            Index::create_in_dir(&index_path, schema.clone())?
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
        // Search for all documents with this source, then delete by ID
        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.source_field]);
        if let Ok(query) = query_parser.parse_query(&format!("\"{}\"", source.replace('"', ""))) {
            let top_docs = searcher
                .search(&query, &TopDocs::with_limit(100_000))
                .unwrap_or_default();
            let writer = self.writer.lock();
            for (_, doc_address) in top_docs {
                if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                    if let Some(id_val) = doc.get_first(self.id_field) {
                        if let Some(id_text) = id_val.as_str() {
                            let term = tantivy::Term::from_field_text(self.id_field, id_text);
                            writer.delete_term(term);
                        }
                    }
                }
            }
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
}
