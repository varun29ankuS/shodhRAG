//! The LanceDB `statements` table: content, embedding, full-text index and validity.

use std::sync::Arc;

use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lancedb::index::scalar::FullTextSearchQuery;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::DistanceType;

use super::{StatementError, StatementResult};

/// One stored row, as raw columns.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub id: String,
    pub class: String,
    pub subject: String,
    /// `|token|token|`, or empty.
    pub identity: String,
    pub scope: String,
    pub text: String,
    /// The values as plain words (full-text indexed).
    pub terms: String,
    pub statement_json: String,
    pub properties_json: String,
    pub provenance_json: String,
    pub extractor: String,
    pub source: String,
    pub ontology_source: String,
    pub ontology_version: String,
    pub valid_from: i64,
    pub valid_to: Option<i64>,
    pub superseded_by: Option<String>,
    pub expires_at: Option<i64>,
    pub forgotten_at: Option<i64>,
    pub created_at: i64,
}

/// Microseconds since the epoch.
pub(crate) fn micros(at: DateTime<Utc>) -> i64 {
    at.timestamp_micros()
}

/// The time at `micros`, if representable.
pub(crate) fn from_micros(micros: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_micros(micros)
}

/// A SQL string literal.
pub(crate) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A SQL `LIKE` pattern matching `|token|` anywhere in the identity column. Tokens are hex,
/// so they need no escaping beyond the quote.
pub(crate) fn identity_like(token: &str) -> String {
    format!("identity LIKE {}", quote(&format!("%|{token}|%")))
}

pub(crate) struct StatementTable {
    table: lancedb::Table,
    dimension: usize,
}

impl StatementTable {
    pub(crate) async fn open(
        db: &lancedb::Connection,
        name: &str,
        dimension: usize,
    ) -> StatementResult<Self> {
        let names = db.table_names().execute().await?;
        let table = if names.iter().any(|n| n == name) {
            db.open_table(name).execute().await?
        } else {
            db.create_empty_table(name, schema(dimension))
                .execute()
                .await?
        };
        let schema_ref = table.schema().await?;
        let stored = vector_dimension(schema_ref.as_ref());
        if let Some(stored) = stored {
            if stored != dimension {
                return Err(StatementError::DimensionMismatch {
                    expected: stored,
                    found: dimension,
                });
            }
        }
        Ok(Self { table, dimension })
    }

    pub(crate) fn dimension(&self) -> usize {
        self.dimension
    }

    /// Creates the full-text index on `terms` if it does not exist. Rows appended after the
    /// index was built are still searched (LanceDB flat-searches unindexed fragments).
    async fn ensure_fts_index(&self) -> StatementResult<()> {
        let indices = self.table.list_indices().await?;
        if indices
            .iter()
            .any(|i| i.columns.iter().any(|c| c == "terms"))
        {
            return Ok(());
        }
        self.table
            .create_index(&["terms"], Index::FTS(Default::default()))
            .replace(true)
            .execute()
            .await?;
        Ok(())
    }

    /// Appends `rows` with their `vectors` (same order) in one write.
    pub(crate) async fn insert_many(
        &self,
        rows: &[Row],
        vectors: &[Vec<f32>],
    ) -> StatementResult<()> {
        if rows.is_empty() {
            return Ok(());
        }
        if rows.len() != vectors.len() {
            return Err(StatementError::Embedding(format!(
                "{} vectors for {} statements",
                vectors.len(),
                rows.len()
            )));
        }
        if let Some(v) = vectors.iter().find(|v| v.len() != self.dimension) {
            return Err(StatementError::DimensionMismatch {
                expected: self.dimension,
                found: v.len(),
            });
        }
        let schema = schema(self.dimension);
        let item = Arc::new(Field::new("item", DataType::Float32, true));
        let flat: Vec<f32> = vectors.iter().flatten().copied().collect();
        let vectors = FixedSizeListArray::try_new(
            item,
            self.dimension as i32,
            Arc::new(Float32Array::from(flat)) as Arc<dyn Array>,
            None,
        )?;
        let text = |f: fn(&Row) -> &str| {
            Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>())) as Arc<dyn Array>
        };
        let opt_text = |f: fn(&Row) -> Option<&str>| {
            Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>())) as Arc<dyn Array>
        };
        let int = |f: fn(&Row) -> i64| {
            Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<_>>())) as Arc<dyn Array>
        };
        let opt_int = |f: fn(&Row) -> Option<i64>| {
            Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<_>>())) as Arc<dyn Array>
        };
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                text(|r| &r.id),
                text(|r| &r.class),
                text(|r| &r.subject),
                text(|r| &r.identity),
                text(|r| &r.scope),
                text(|r| &r.text),
                text(|r| &r.terms),
                text(|r| &r.statement_json),
                text(|r| &r.properties_json),
                text(|r| &r.provenance_json),
                text(|r| &r.extractor),
                text(|r| &r.source),
                text(|r| &r.ontology_source),
                text(|r| &r.ontology_version),
                int(|r| r.valid_from),
                opt_int(|r| r.valid_to),
                opt_text(|r| r.superseded_by.as_deref()),
                opt_int(|r| r.expires_at),
                opt_int(|r| r.forgotten_at),
                int(|r| r.created_at),
                Arc::new(vectors) as Arc<dyn Array>,
            ],
        )?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        self.table.add(Box::new(reader)).execute().await?;
        self.ensure_fts_index().await
    }

    /// Closes a statement: sets `valid_to` and `superseded_by`. Content is never rewritten.
    /// The table's version: each write (append, update) makes a new one.
    #[cfg(test)]
    pub(crate) async fn version(&self) -> StatementResult<u64> {
        Ok(self.table.version().await?)
    }

    pub(crate) async fn close(
        &self,
        id: &str,
        valid_to: i64,
        superseded_by: Option<&str>,
    ) -> StatementResult<()> {
        let superseded = superseded_by
            .map(quote)
            .unwrap_or_else(|| "NULL".to_string());
        self.table
            .update()
            .only_if(format!("id = {}", quote(id)))
            .column("valid_to", valid_to.to_string())
            .column("superseded_by", superseded)
            .execute()
            .await?;
        Ok(())
    }

    /// Reopens a closed statement: clears `valid_to` and `superseded_by` (undoing a
    /// supersede or an archive). Content is never rewritten.
    pub(crate) async fn reopen(&self, id: &str) -> StatementResult<()> {
        self.table
            .update()
            .only_if(format!("id = {}", quote(id)))
            .column("valid_to", "CAST(NULL AS BIGINT)")
            .column("superseded_by", "CAST(NULL AS STRING)")
            .execute()
            .await?;
        Ok(())
    }

    /// Soft-deletes a statement.
    pub(crate) async fn forget(&self, id: &str, at: i64) -> StatementResult<u64> {
        let result = self
            .table
            .update()
            .only_if(format!("id = {} AND forgotten_at IS NULL", quote(id)))
            .column("forgotten_at", at.to_string())
            .execute()
            .await?;
        Ok(result.rows_updated)
    }

    pub(crate) async fn scan(
        &self,
        predicate: Option<&str>,
        limit: usize,
    ) -> StatementResult<Vec<Row>> {
        let mut query = self.table.query().limit(limit);
        if let Some(predicate) = predicate {
            query = query.only_if(predicate);
        }
        let batches: Vec<RecordBatch> = query.execute().await?.try_collect().await?;
        Ok(rows(&batches)?.into_iter().map(|(row, _)| row).collect())
    }

    /// Nearest rows by cosine distance, prefiltered. Returns `(row, similarity)`.
    pub(crate) async fn vector_search(
        &self,
        vector: &[f32],
        predicate: Option<&str>,
        k: usize,
    ) -> StatementResult<Vec<(Row, f64)>> {
        let mut query = self
            .table
            .query()
            .nearest_to(vector)?
            .distance_type(DistanceType::Cosine)
            .limit(k);
        if let Some(predicate) = predicate {
            query = query.only_if(predicate);
        }
        let batches: Vec<RecordBatch> = query.execute().await?.try_collect().await?;
        Ok(rows(&batches)?
            .into_iter()
            .map(|(row, distance)| (row, (1.0 - distance.unwrap_or(1.0)).clamp(-1.0, 1.0)))
            .collect())
    }

    /// Full-text (BM25) matches on the values (`terms`), prefiltered, best first.
    pub(crate) async fn text_search(
        &self,
        text: &str,
        predicate: Option<&str>,
        k: usize,
    ) -> StatementResult<Vec<Row>> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let indices = self.table.list_indices().await?;
        if !indices
            .iter()
            .any(|i| i.columns.iter().any(|c| c == "terms"))
        {
            // Nothing was ever written: no index, no rows.
            return Ok(Vec::new());
        }
        let fts = FullTextSearchQuery::new(text.to_string())
            .with_column("terms".to_string())
            .map_err(|e| StatementError::Lance(e.to_string()))?;
        let mut query = self.table.query().full_text_search(fts).limit(k);
        if let Some(predicate) = predicate {
            query = query.only_if(predicate);
        }
        let batches: Vec<RecordBatch> = query.execute().await?.try_collect().await?;
        Ok(rows(&batches)?.into_iter().map(|(row, _)| row).collect())
    }
}

fn schema(dimension: usize) -> SchemaRef {
    let text = |name: &str| Field::new(name, DataType::Utf8, false);
    let opt_text = |name: &str| Field::new(name, DataType::Utf8, true);
    let int = |name: &str| Field::new(name, DataType::Int64, false);
    let opt_int = |name: &str| Field::new(name, DataType::Int64, true);
    Arc::new(Schema::new(vec![
        text("id"),
        text("class"),
        text("subject"),
        text("identity"),
        text("scope"),
        text("text"),
        text("terms"),
        text("statement_json"),
        text("properties_json"),
        text("provenance_json"),
        text("extractor"),
        text("source"),
        text("ontology_source"),
        text("ontology_version"),
        int("valid_from"),
        opt_int("valid_to"),
        opt_text("superseded_by"),
        opt_int("expires_at"),
        opt_int("forgotten_at"),
        int("created_at"),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension as i32,
            ),
            true,
        ),
    ]))
}

fn vector_dimension(schema: &Schema) -> Option<usize> {
    match schema.field_with_name("vector").ok()?.data_type() {
        DataType::FixedSizeList(_, size) => usize::try_from(*size).ok(),
        _ => None,
    }
}

/// Decodes rows (with `_distance` when present).
fn rows(batches: &[RecordBatch]) -> StatementResult<Vec<(Row, Option<f64>)>> {
    let mut out = Vec::new();
    for batch in batches {
        let text = |name: &str| -> StatementResult<&StringArray> {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| StatementError::Lance(format!("column `{name}` is missing")))
        };
        let int = |name: &str| -> StatementResult<&Int64Array> {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
                .ok_or_else(|| StatementError::Lance(format!("column `{name}` is missing")))
        };
        let ids = text("id")?;
        let classes = text("class")?;
        let subjects = text("subject")?;
        let identities = text("identity")?;
        let scopes = text("scope")?;
        let texts = text("text")?;
        let terms = text("terms")?;
        let statements = text("statement_json")?;
        let properties = text("properties_json")?;
        let provenances = text("provenance_json")?;
        let extractors = text("extractor")?;
        let sources = text("source")?;
        let ontology_sources = text("ontology_source")?;
        let ontology_versions = text("ontology_version")?;
        let valid_froms = int("valid_from")?;
        let valid_tos = int("valid_to")?;
        let superseded = text("superseded_by")?;
        let expires = int("expires_at")?;
        let forgotten = int("forgotten_at")?;
        let created = int("created_at")?;
        let distances = batch
            .column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
        let opt_int = |a: &Int64Array, i: usize| (!a.is_null(i)).then(|| a.value(i));
        let opt_text = |a: &StringArray, i: usize| (!a.is_null(i)).then(|| a.value(i).to_string());
        for i in 0..batch.num_rows() {
            out.push((
                Row {
                    id: ids.value(i).to_string(),
                    class: classes.value(i).to_string(),
                    subject: subjects.value(i).to_string(),
                    identity: identities.value(i).to_string(),
                    scope: scopes.value(i).to_string(),
                    text: texts.value(i).to_string(),
                    terms: terms.value(i).to_string(),
                    statement_json: statements.value(i).to_string(),
                    properties_json: properties.value(i).to_string(),
                    provenance_json: provenances.value(i).to_string(),
                    extractor: extractors.value(i).to_string(),
                    source: sources.value(i).to_string(),
                    ontology_source: ontology_sources.value(i).to_string(),
                    ontology_version: ontology_versions.value(i).to_string(),
                    valid_from: valid_froms.value(i),
                    valid_to: opt_int(valid_tos, i),
                    superseded_by: opt_text(superseded, i),
                    expires_at: opt_int(expires, i),
                    forgotten_at: opt_int(forgotten, i),
                    created_at: created.value(i),
                },
                distances
                    .filter(|d| !d.is_null(i))
                    .map(|d| f64::from(d.value(i))),
            ));
        }
    }
    Ok(out)
}
