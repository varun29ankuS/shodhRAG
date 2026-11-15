use anyhow::{Context, Result};
use std::path::Path;
use tantivy::collector::{DocSetCollector, TopDocs};
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    self, IndexRecordOption, Schema, Value as TantivyValue, STORED, STRING, TEXT,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

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
    fn build_schema() -> (
        Schema,
        schema::Field,
        schema::Field,
        schema::Field,
        schema::Field,
    ) {
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

        // Manual: every write path here (`commit`, deletions, `clear`) reloads the reader
        // itself, and this is the index's only writer (Tantivy locks it). The other
        // policy, OnCommitWithDelay, starts a thread that opens `meta.json` every 500 ms
        // to see whether it changed; on Windows a commit replacing `meta.json` while that
        // handle is open fails with "Access is denied", so a file failed to index now
        // and then for no reason of its own.
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
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

    pub fn index_chunks_batch(&self, chunks: &[(String, String, String, String)]) -> Result<()> {
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

    /// Drops what was added or deleted since the last commit.
    pub fn rollback(&self) -> Result<()> {
        let mut writer = self.writer.lock();
        writer.rollback().context("Tantivy rollback failed")?;
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
        let top_docs = searcher.search(
            &parsed_query,
            &TopDocs::with_limit(fetch_limit).order_by_score(),
        )?;

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
        let term_query =
            tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let top_docs = searcher.search(&term_query, &TopDocs::with_limit(1).order_by_score())?;
        if let Some((_score, addr)) = top_docs.first() {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(*addr) {
                if let Some(text_val) = doc.get_first(self.text_field) {
                    return Ok(text_val.as_str().map(|s| s.to_string()));
                }
            }
        }
        Ok(None)
    }

    /// Removes deleted documents' text from disk now: commits, merges every
    /// segment (a merge leaves deleted documents out) and deletes the files no
    /// longer used. A delete alone only marks a document; its stored text stays in
    /// its segment until a merge.
    pub fn expunge_deletes(&self) -> Result<()> {
        let mut writer = self.writer.lock();
        writer
            .commit()
            .context("Tantivy commit before merge failed")?;
        let segments = self.index.searchable_segment_ids()?;
        if segments.len() > 1 || self.has_deletes()? {
            writer
                .merge(&segments)
                .wait()
                .context("Tantivy merge failed")?;
        }
        writer
            .garbage_collect_files()
            .wait()
            .context("Tantivy garbage collection failed")?;
        drop(writer);
        self.reader.reload()?;
        Ok(())
    }

    /// Whether any segment holds deleted documents.
    fn has_deletes(&self) -> Result<bool> {
        self.reader.reload()?;
        Ok(self
            .reader
            .searcher()
            .segment_readers()
            .iter()
            .any(|s| s.num_deleted_docs() > 0))
    }

    pub fn delete_by_id(&self, id: &str) -> Result<()> {
        let writer = self.writer.lock();
        let term = tantivy::Term::from_field_text(self.id_field, id);
        writer.delete_term(term);
        Ok(())
    }

    /// Documents that hold every token of `source` in their source field: a superset
    /// of the documents whose source is exactly `source`, found from the postings
    /// instead of by reading every stored document.
    fn source_candidates(&self, source: &str) -> Result<Box<dyn Query>> {
        let mut analyzer = self.index.tokenizer_for_field(self.source_field)?;
        let mut stream = analyzer.token_stream(source);
        let mut tokens = Vec::new();
        while stream.advance() {
            tokens.push(stream.token().text.clone());
        }
        tokens.sort_unstable();
        tokens.dedup();
        // A source with no indexable token (empty, or only tokens the length filter
        // drops) has nothing to look up, so every document is a candidate.
        if tokens.is_empty() {
            return Ok(Box::new(AllQuery));
        }
        let clauses = tokens
            .iter()
            .map(|token| {
                let term = Term::from_field_text(self.source_field, token);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                (Occur::Must, query)
            })
            .collect();
        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    /// Delete every document whose source is `source`.
    pub fn delete_by_source(&self, source: &str) -> Result<()> {
        self.reader.reload().ok();
        let searcher = self.reader.searcher();
        let candidates = searcher.search(&*self.source_candidates(source)?, &DocSetCollector)?;
        let mut writer = self.writer.lock();
        let mut deleted_count = 0usize;

        for address in candidates {
            let doc = searcher.doc::<TantivyDocument>(address)?;
            let same_source = doc
                .get_first(self.source_field)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == source);
            if !same_source {
                continue;
            }
            if let Some(id_text) = doc.get_first(self.id_field).and_then(|v| v.as_str()) {
                writer.delete_term(Term::from_field_text(self.id_field, id_text));
                deleted_count += 1;
            }
        }

        // Commit deletions and reload reader immediately so subsequent
        // searches never return the deleted documents.
        if deleted_count > 0 {
            writer
                .commit()
                .context("Tantivy commit after delete failed")?;
            self.reader.reload()?;
            tracing::info!(
                source = %crate::log_safe::file(source),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader reloads only when told (no polling thread): every write path
    /// must show its change to the next search at once.
    #[test]
    fn writes_are_visible_to_the_next_search_without_polling() {
        let dir = tempfile::tempdir().unwrap();
        let index = TextSearch::new(dir.path().to_str().unwrap()).unwrap();
        index
            .index_chunk("c1", "the lease notice period", "lease", "a/lease.txt")
            .unwrap();
        index
            .index_chunks_batch(&[(
                "c2".into(),
                "visitors park on the street".into(),
                "parking".into(),
                "a/parking.txt".into(),
            )])
            .unwrap();
        index.commit().unwrap();
        assert_eq!(index.count().unwrap(), 2);
        assert_eq!(index.search("lease", 5).unwrap()[0].0, "c1");

        index.delete_by_source("a/lease.txt").unwrap();
        assert!(index.search("lease", 5).unwrap().is_empty());
        assert_eq!(index.count().unwrap(), 1);

        index.delete_by_id("c2").unwrap();
        index.commit().unwrap();
        assert_eq!(index.count().unwrap(), 0);

        index
            .index_chunk("c3", "quiet hours", "rules", "a/rules.txt")
            .unwrap();
        index.commit().unwrap();
        index.clear().unwrap();
        assert!(index.is_empty());
    }

    /// The source lookup matches tokens, so sources sharing every token of the
    /// deleted one must survive; a source with no indexable token still deletes.
    #[test]
    fn delete_by_source_removes_that_source_only() {
        let dir = tempfile::tempdir().unwrap();
        let index = TextSearch::new(dir.path().to_str().unwrap()).unwrap();
        let sources = [
            ("c1", "a/lease.txt"),
            ("c2", "a/lease.txt"),
            ("c3", "b/a/lease.txt"),
            ("c4", "a/lease.txt.bak"),
            ("c5", "A/Lease.TXT"),
            ("c6", "a/txt/lease"),
            ("c7", "—"),
            ("c8", "–"),
        ];
        for (id, source) in sources {
            index
                .index_chunk(id, "the lease notice period", "lease", source)
                .unwrap();
        }
        index.commit().unwrap();

        index.delete_by_source("a/lease.txt").unwrap();
        index.delete_by_source("—").unwrap();
        index.delete_by_source("absent.txt").unwrap();

        let mut left: Vec<String> = index
            .search("lease", 10)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        left.sort();
        assert_eq!(left, ["c3", "c4", "c5", "c6", "c8"]);
    }
}
