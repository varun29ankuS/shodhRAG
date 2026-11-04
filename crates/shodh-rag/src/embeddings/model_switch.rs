//! Moving an index to another E5 export ("Re-index for faster search")
//! without ever mixing vectors of two exports ([`super::index_model`]).
//!
//! The index in use is never touched. A switch builds new LanceDB tables
//! beside the old ones (`documents_int8`, `statements_int8`): every row is
//! copied with its vector embedded again from the text it was embedded from,
//! all other columns unchanged, so the keyword index and every id stay valid.
//! The build runs in the background, can be paused and cancelled, and resumes
//! after a restart (a row already copied unchanged is not embedded again).
//! Search keeps using the old tables and the old export meanwhile.
//!
//! The switch completes at the next start, before anything opens the tables:
//! rows changed since the build (files indexed or removed, memories added or
//! forgotten) are brought over, the new tables' old versions are pruned (no
//! copy of a forgotten memory stays in their history), and the index record is
//! replaced, atomically, by one naming the new export and tables. That rename
//! is the switch; a crash before it leaves the old index in use and the
//! switch is finished at the following start. The old tables are dropped
//! after it.
//!
//! The marker (`search-model-switch.json`) says a switch was started and
//! whether its build is complete; without one, tables a cancelled or finished
//! switch left behind are dropped at start ([`tidy`]).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, Int32Array, Int64Array, RecordBatch,
    RecordBatchIterator, StringArray, UInt32Array,
};
use arrow_schema::{DataType, FieldRef, SchemaRef};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::table::{CompactionOptions, Duration as LanceDuration, OptimizeAction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::e5::E5Variant;
use super::index_model::{self, IndexModel, DOCUMENTS_TABLE};
use super::EmbeddingModel;

/// The marker's file in the data folder.
pub const SWITCH_FILE: &str = "search-model-switch.json";
/// The LanceDB folder (in the data folder) holding both vector tables.
pub const LANCE_DIR: &str = "lance_data";

/// Rows embedded per step: the build checks for a pause or a cancel between
/// steps (about a second of int8 work on a laptop).
const STEP_ROWS: usize = 32;
/// Ids per delete statement.
const DELETE_IDS: usize = 256;

/// A switch in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSwitch {
    pub from: E5Variant,
    pub to: E5Variant,
    /// The tables in use when it started.
    pub from_documents_table: String,
    pub from_statements_table: String,
    /// The tables it builds.
    pub documents_table: String,
    pub statements_table: String,
    pub started_at: DateTime<Utc>,
    /// Every row has been embedded once in `to`: the switch completes at the
    /// next start.
    pub built: bool,
}

/// How far a build is: rows looked at (copied or found unchanged) of all the
/// rows of both tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchProgress {
    pub done: u64,
    pub total: u64,
}

/// How a build ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildOutcome {
    /// Every row is in the new tables: the switch completes at the next start.
    Built,
    /// It was cancelled or paused for good; what it copied stays for a resume.
    Stopped,
}

/// The tables of one kind of row and how its vectors were embedded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rows {
    /// Library chunks: their context and text, as passages.
    Documents,
    /// Memory statements: their text, as passages.
    Statements,
}

fn lance_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(LANCE_DIR)
}

async fn connect(data_dir: &Path) -> Result<lancedb::Connection> {
    let dir = lance_dir(data_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(lancedb::connect(&dir.to_string_lossy()).execute().await?)
}

/// The switch in progress in `data_dir`, if any.
pub fn pending(data_dir: &Path) -> Result<Option<PendingSwitch>> {
    let path = data_dir.join(SWITCH_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                format!("{} is damaged", path.display())
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_pending(data_dir: &Path, switch: &PendingSwitch) -> Result<()> {
    index_model::write_json_atomically(data_dir, SWITCH_FILE, switch)
}

/// The table a switch to `to` builds for `base` ("documents", "statements").
fn table_for(base: &str, to: E5Variant) -> String {
    format!("{base}_{}", to.label())
}

/// Starts (or continues) a switch of the index in `data_dir` to `to`. The
/// index must be recorded and in another export.
pub fn start(data_dir: &Path, to: E5Variant) -> Result<PendingSwitch> {
    let record = index_model::read(data_dir)?
        .ok_or_else(|| anyhow!("this library's search model is not recorded yet"))?;
    if record.variant == to {
        bail!(
            "this library is already indexed with the {} search model",
            to.label()
        );
    }
    if let Some(existing) = pending(data_dir)? {
        if existing.to == to && existing.from == record.variant {
            return Ok(existing);
        }
        bail!(
            "a switch to the {} search model is already in progress",
            existing.to.label()
        );
    }
    let switch = PendingSwitch {
        from: record.variant,
        to,
        from_documents_table: record.documents_table.clone(),
        from_statements_table: record.statements_table.clone(),
        documents_table: table_for(DOCUMENTS_TABLE, to),
        statements_table: table_for(crate::statements::STATEMENTS_TABLE, to),
        started_at: Utc::now(),
        built: false,
    };
    write_pending(data_dir, &switch)?;
    tracing::info!(
        from = switch.from.label(),
        to = to.label(),
        "Search model switch started"
    );
    Ok(switch)
}

/// Cancels the switch in `data_dir`: its tables are dropped and the marker
/// removed. Returns whether there was one.
pub async fn cancel(data_dir: &Path) -> Result<bool> {
    let Some(switch) = pending(data_dir)? else {
        return Ok(false);
    };
    let db = connect(data_dir).await?;
    for table in [&switch.documents_table, &switch.statements_table] {
        drop_table(&db, table).await?;
    }
    std::fs::remove_file(data_dir.join(SWITCH_FILE))
        .with_context(|| format!("removing {SWITCH_FILE}"))?;
    tracing::info!(to = switch.to.label(), "Search model switch cancelled");
    Ok(true)
}

/// Checks `embedder` embeds in `to`. Only a test embedder (no E5 export) is
/// let through without one, in tests.
fn check_embedder(embedder: &dyn EmbeddingModel, to: E5Variant) -> Result<()> {
    match embedder.e5_variant() {
        Some(variant) if variant == to => Ok(()),
        None if cfg!(test) => Ok(()),
        other => bail!(
            "the switch to {} was given a model of {:?}",
            to.label(),
            other.map(E5Variant::label)
        ),
    }
}

/// Builds the new tables of the switch in `data_dir` with `embedder` (the
/// switch's export). `stop` is asked before every step and may wait (while
/// indexing is paused); `true` stops the build, keeping what it copied.
/// `progress` hears how far it is after every step.
pub async fn build<S, F>(
    data_dir: &Path,
    embedder: Arc<dyn EmbeddingModel>,
    mut stop: S,
    progress: &(dyn Fn(SwitchProgress) + Send + Sync),
) -> Result<BuildOutcome>
where
    S: FnMut() -> F,
    F: Future<Output = bool>,
{
    let mut switch =
        pending(data_dir)?.ok_or_else(|| anyhow!("no search model switch was started"))?;
    check_embedder(embedder.as_ref(), switch.to)?;
    let db = connect(data_dir).await?;
    let mut counter = SwitchProgress {
        done: 0,
        total: (rows_of(&db, &switch.from_documents_table).await?
            + rows_of(&db, &switch.from_statements_table).await?) as u64,
    };
    progress(counter);
    for (source, target, rows) in [
        (
            &switch.from_documents_table,
            &switch.documents_table,
            Rows::Documents,
        ),
        (
            &switch.from_statements_table,
            &switch.statements_table,
            Rows::Statements,
        ),
    ] {
        let stopped = sync_table(
            &db,
            source,
            target,
            rows,
            &embedder,
            &mut stop,
            &mut counter,
            progress,
        )
        .await
        .with_context(|| format!("re-embedding {source} into {target}"))?;
        if stopped {
            return Ok(BuildOutcome::Stopped);
        }
        mirror_indices(&db, source, target).await?;
    }
    switch.built = true;
    write_pending(data_dir, &switch)?;
    tracing::info!(
        to = switch.to.label(),
        rows = counter.total,
        "Search model switch built; it completes at the next start"
    );
    Ok(BuildOutcome::Built)
}

/// Completes a built switch in `data_dir` (at start, before the engine or the
/// memory open their tables): brings over what changed since the build with
/// the model `load` gives (the switch's export), prunes the new tables' old
/// versions, then replaces the index record. Returns the new record when a
/// switch completed. Tables no record or switch names are dropped
/// ([`tidy`]) whatever happens.
pub async fn finish_pending<L>(data_dir: &Path, load: L) -> Result<Option<IndexModel>>
where
    L: FnOnce(E5Variant) -> Result<Arc<dyn EmbeddingModel>> + Send + 'static,
{
    let finished = finish(data_dir, load).await;
    if let Err(e) = tidy(data_dir).await {
        tracing::warn!(error = %format!("{e:#}"), "Leftover search tables were not removed");
    }
    finished
}

async fn finish<L>(data_dir: &Path, load: L) -> Result<Option<IndexModel>>
where
    L: FnOnce(E5Variant) -> Result<Arc<dyn EmbeddingModel>> + Send + 'static,
{
    let Some(switch) = pending(data_dir)? else {
        return Ok(None);
    };
    if !switch.built {
        return Ok(None);
    }
    let record = index_model::read(data_dir)?
        .ok_or_else(|| anyhow!("a search model switch without an index record"))?;
    let db = connect(data_dir).await?;
    if record.variant == switch.to
        && record.documents_table == switch.documents_table
        && record.statements_table == switch.statements_table
    {
        // The record was replaced and the start stopped before the rest.
        drop_old_tables(&db, &switch).await?;
        std::fs::remove_file(data_dir.join(SWITCH_FILE))?;
        return Ok(None);
    }
    if record.variant != switch.from
        || record.documents_table != switch.from_documents_table
        || record.statements_table != switch.from_statements_table
    {
        bail!(
            "the index record ({}, {}) is not the one the switch started from ({}, {})",
            record.variant.label(),
            record.documents_table,
            switch.from.label(),
            switch.from_documents_table
        );
    }
    let to = switch.to;
    let embedder = tokio::task::spawn_blocking(move || load(to))
        .await
        .map_err(|e| anyhow!("loading the search model failed: {e}"))??;
    check_embedder(embedder.as_ref(), switch.to)?;
    let mut counter = SwitchProgress::default();
    for (source, target, rows) in [
        (
            &switch.from_documents_table,
            &switch.documents_table,
            Rows::Documents,
        ),
        (
            &switch.from_statements_table,
            &switch.statements_table,
            Rows::Statements,
        ),
    ] {
        sync_table(
            &db,
            source,
            target,
            rows,
            &embedder,
            &mut || std::future::ready(false),
            &mut counter,
            &|_| {},
        )
        .await
        .with_context(|| format!("bringing {source} up to date in {target}"))?;
        mirror_indices(&db, source, target).await?;
        prune_old_versions(&db, target).await?;
    }
    let switched = IndexModel {
        variant: switch.to,
        recorded_at: Utc::now(),
        reason: format!("switched from {}", switch.from.label()),
        documents_table: switch.documents_table.clone(),
        statements_table: switch.statements_table.clone(),
        ..record
    };
    // The switch.
    index_model::write_record(data_dir, &switched)?;
    tracing::info!(
        from = switch.from.label(),
        to = switch.to.label(),
        "Search model switched"
    );
    if let Err(e) = drop_old_tables(&db, &switch).await {
        tracing::warn!(error = %format!("{e:#}"), "The old search tables were not removed yet");
    }
    if let Err(e) = std::fs::remove_file(data_dir.join(SWITCH_FILE)) {
        tracing::warn!(error = %e, "The finished switch's marker was not removed");
    }
    Ok(Some(switched))
}

async fn drop_old_tables(db: &lancedb::Connection, switch: &PendingSwitch) -> Result<()> {
    for table in [&switch.from_documents_table, &switch.from_statements_table] {
        drop_table(db, table).await?;
    }
    Ok(())
}

/// Every name a vector table of the index can have.
fn vector_table_names() -> Vec<String> {
    let mut names = Vec::new();
    for base in [DOCUMENTS_TABLE, crate::statements::STATEMENTS_TABLE] {
        names.push(base.to_string());
        for variant in E5Variant::ALL {
            names.push(table_for(base, variant));
        }
    }
    names
}

/// Drops the vector tables in `data_dir` that neither the index record nor a
/// switch in progress names: a cancelled switch's, or a finished switch's old
/// tables. Nothing else in the folder is touched.
pub async fn tidy(data_dir: &Path) -> Result<()> {
    let (documents, statements) = index_model::tables(data_dir)?;
    let switch = pending(data_dir)?;
    // Only a switch makes other tables (and drops them itself; a crash can
    // leave them): an index in its original tables without a switch has
    // nothing to tidy, and the start does not open LanceDB for it.
    if switch.is_none()
        && documents == DOCUMENTS_TABLE
        && statements == crate::statements::STATEMENTS_TABLE
    {
        return Ok(());
    }
    let mut keep = vec![documents, statements];
    if let Some(switch) = switch {
        keep.extend([
            switch.documents_table,
            switch.statements_table,
            switch.from_documents_table,
            switch.from_statements_table,
        ]);
    }
    let db = connect(data_dir).await?;
    let existing = db.table_names().execute().await?;
    for name in vector_table_names() {
        if existing.contains(&name) && !keep.contains(&name) {
            drop_table(&db, &name).await?;
            tracing::info!(table = %name, "Removed a search table no index uses");
        }
    }
    Ok(())
}

async fn drop_table(db: &lancedb::Connection, name: &str) -> Result<()> {
    let names = db.table_names().execute().await?;
    if names.iter().any(|n| n == name) {
        db.drop_table(name, &[]).await?;
    }
    Ok(())
}

async fn rows_of(db: &lancedb::Connection, name: &str) -> Result<usize> {
    let names = db.table_names().execute().await?;
    if !names.iter().any(|n| n == name) {
        return Ok(0);
    }
    Ok(db
        .open_table(name)
        .execute()
        .await?
        .count_rows(None)
        .await?)
}

/// Makes `target` hold every row of `source` with its vector embedded again
/// by `embedder`: rows already in `target` with the same content are kept,
/// changed ones replaced, missing ones added, and rows `source` no longer has
/// removed. Returns whether `stop` stopped it.
#[allow(clippy::too_many_arguments)]
async fn sync_table<S, F>(
    db: &lancedb::Connection,
    source: &str,
    target: &str,
    rows: Rows,
    embedder: &Arc<dyn EmbeddingModel>,
    stop: &mut S,
    counter: &mut SwitchProgress,
    progress: &(dyn Fn(SwitchProgress) + Send + Sync),
) -> Result<bool>
where
    S: FnMut() -> F,
    F: Future<Output = bool>,
{
    let names = db.table_names().execute().await?;
    if !names.iter().any(|n| n == source) {
        // Nothing to copy (the memory was never opened): no target either.
        drop_table(db, target).await?;
        return Ok(false);
    }
    let source_table = db.open_table(source).execute().await?;
    let schema = source_table.schema().await?;
    let vector = schema
        .index_of("vector")
        .map_err(|_| anyhow!("{source} has no vector column"))?;
    let target_table = if names.iter().any(|n| n == target) {
        let table = db.open_table(target).execute().await?;
        if same_columns(&table.schema().await?, &schema) {
            table
        } else {
            // Another layout (an older build of the table): built again.
            db.drop_table(target, &[]).await?;
            db.create_empty_table(target, schema.clone())
                .execute()
                .await?
        }
    } else {
        db.create_empty_table(target, schema.clone())
            .execute()
            .await?
    };

    // What the target holds: each row's content fingerprint.
    let content: Vec<String> = schema
        .fields()
        .iter()
        .filter(|f| f.name() != "vector")
        .map(|f| f.name().clone())
        .collect();
    let mut held: HashMap<String, [u8; 32]> = HashMap::new();
    let mut stream = target_table
        .query()
        .select(Select::columns(content.as_slice()))
        .execute()
        .await?;
    while let Some(batch) = stream.try_next().await? {
        let ids = string_column(&batch, "id")?;
        for row in 0..batch.num_rows() {
            held.insert(ids.value(row).to_string(), fingerprint(&batch, row)?);
        }
    }

    // One version of the source, read in order.
    let mut stream = source_table.query().execute().await?;
    let mut step: Vec<(RecordBatch, usize, bool)> = Vec::with_capacity(STEP_ROWS);
    while let Some(batch) = stream.try_next().await? {
        let ids = string_column(&batch, "id")?;
        for row in 0..batch.num_rows() {
            let id = ids.value(row);
            let print = fingerprint(&batch, row)?;
            match held.remove(id) {
                Some(kept) if kept == print => {
                    counter.done += 1;
                }
                replaced => {
                    step.push((batch.clone(), row, replaced.is_some()));
                    if step.len() == STEP_ROWS {
                        if stop().await {
                            return Ok(true);
                        }
                        copy_rows(&target_table, &schema, vector, rows, embedder, &step).await?;
                        counter.done += step.len() as u64;
                        step.clear();
                        progress(*counter);
                    }
                }
            }
        }
        progress(*counter);
    }
    if !step.is_empty() {
        if stop().await {
            return Ok(true);
        }
        copy_rows(&target_table, &schema, vector, rows, embedder, &step).await?;
        counter.done += step.len() as u64;
        progress(*counter);
    }
    // Rows the source no longer has.
    let gone: Vec<String> = held.into_keys().collect();
    for ids in gone.chunks(DELETE_IDS) {
        target_table.delete(&id_list(ids)).await?;
    }
    Ok(false)
}

/// Writes the rows of `step` (a batch and a row in it, and whether the
/// target held an older copy) into `target` with vectors `embedder` gives.
async fn copy_rows(
    target: &lancedb::Table,
    schema: &SchemaRef,
    vector: usize,
    rows: Rows,
    embedder: &Arc<dyn EmbeddingModel>,
    step: &[(RecordBatch, usize, bool)],
) -> Result<()> {
    let inputs: Vec<String> = step
        .iter()
        .map(|(batch, row, _)| embedding_input(rows, batch, *row))
        .collect::<Result<_>>()?;
    let model = embedder.clone();
    let vectors = tokio::task::spawn_blocking(move || {
        let texts: Vec<&str> = inputs.iter().map(String::as_str).collect();
        model.embed_documents(&texts)
    })
    .await
    .map_err(|e| anyhow!("the embedding task failed: {e}"))??;
    if vectors.len() != step.len() {
        bail!(
            "the model returned {} vectors for {} rows",
            vectors.len(),
            step.len()
        );
    }
    let (item, dimension) = match schema.field(vector).data_type() {
        DataType::FixedSizeList(item, size) => (item.clone(), *size),
        other => bail!("the vector column is {other}"),
    };
    let replaced: Vec<String> = step
        .iter()
        .filter(|(_, _, replaced)| *replaced)
        .map(|(batch, row, _)| string_column(batch, "id").map(|ids| ids.value(*row).to_string()))
        .collect::<Result<_>>()?;
    if !replaced.is_empty() {
        target.delete(&id_list(&replaced)).await?;
    }
    let mut batches = Vec::with_capacity(step.len());
    for ((batch, row, _), values) in step.iter().zip(vectors) {
        batches.push(with_vector(
            schema,
            &batch.slice(*row, 1),
            vector,
            &item,
            dimension,
            values,
        )?);
    }
    let reader = RecordBatchIterator::new(batches.into_iter().map(Ok), schema.clone());
    target.add(Box::new(reader)).execute().await?;
    Ok(())
}

/// Whether two tables have the same columns (names and types, in order).
fn same_columns(a: &SchemaRef, b: &SchemaRef) -> bool {
    a.fields().len() == b.fields().len()
        && a.fields()
            .iter()
            .zip(b.fields().iter())
            .all(|(x, y)| x.name() == y.name() && x.data_type() == y.data_type())
}

/// `one` (a one-row batch of a table with `schema`) with `values` as its
/// vector.
fn with_vector(
    schema: &SchemaRef,
    one: &RecordBatch,
    vector: usize,
    item: &FieldRef,
    dimension: i32,
    values: Vec<f32>,
) -> Result<RecordBatch> {
    if values.len() != dimension as usize {
        bail!(
            "a {}-dimensional vector for a {dimension}-dimensional column",
            values.len()
        );
    }
    let list = FixedSizeListArray::try_new(
        item.clone(),
        dimension,
        Arc::new(Float32Array::from(values)) as Arc<dyn Array>,
        None,
    )?;
    let mut columns = one.columns().to_vec();
    columns[vector] = Arc::new(list);
    Ok(RecordBatch::try_new(schema.clone(), columns)?)
}

/// A filter for the rows with these ids.
fn id_list(ids: &[String]) -> String {
    let quoted: Vec<String> = ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect();
    format!("id IN ({})", quoted.join(", "))
}

fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("no text column {name}"))
}

/// The text a row's vector was embedded from, as a passage: a chunk's context
/// and text, the way the indexer embedded it ([`crate::rag_engine`]), or a
/// statement's text, the way the memory store embedded it.
fn embedding_input(rows: Rows, batch: &RecordBatch, row: usize) -> Result<String> {
    let text = string_column(batch, "text")?.value(row);
    match rows {
        Rows::Statements => Ok(text.to_string()),
        Rows::Documents => {
            let metadata = string_column(batch, "metadata_json")?.value(row);
            let title = string_column(batch, "title")?.value(row);
            Ok(crate::rag_engine::embedded_text(text, title, metadata))
        }
    }
}

/// A digest of every column of `row` but the vector: equal digests, equal
/// content.
fn fingerprint(batch: &RecordBatch, row: usize) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        if field.name() == "vector" {
            continue;
        }
        hash.update(field.name().as_bytes());
        hash.update([0]);
        if column.is_null(row) {
            hash.update([0]);
            continue;
        }
        hash.update([1]);
        let any = column.as_any();
        if let Some(values) = any.downcast_ref::<StringArray>() {
            let value = values.value(row).as_bytes();
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value);
        } else if let Some(values) = any.downcast_ref::<Int64Array>() {
            hash.update(values.value(row).to_le_bytes());
        } else if let Some(values) = any.downcast_ref::<UInt32Array>() {
            hash.update(values.value(row).to_le_bytes());
        } else if let Some(values) = any.downcast_ref::<Int32Array>() {
            hash.update(values.value(row).to_le_bytes());
        } else if let Some(values) = any.downcast_ref::<BooleanArray>() {
            hash.update([u8::from(values.value(row))]);
        } else {
            bail!(
                "column {} has a type the switch cannot compare ({})",
                field.name(),
                field.data_type()
            );
        }
    }
    Ok(hash.finalize().into())
}

/// Gives `target` the indices `source` has: the vector index of the chunks
/// (built when the library is large) and the full-text index of the
/// statements' terms.
async fn mirror_indices(db: &lancedb::Connection, source: &str, target: &str) -> Result<()> {
    let names = db.table_names().execute().await?;
    if !names.iter().any(|n| n == source) || !names.iter().any(|n| n == target) {
        return Ok(());
    }
    let source = db.open_table(source).execute().await?;
    let target = db.open_table(target).execute().await?;
    let present = target.list_indices().await?;
    for index in source.list_indices().await? {
        if present.iter().any(|p| p.columns == index.columns) {
            continue;
        }
        let columns: Vec<&str> = index.columns.iter().map(String::as_str).collect();
        let kind = match columns.as_slice() {
            ["vector"] => Index::Auto,
            ["terms"] => Index::FTS(Default::default()),
            other => bail!(
                "{} has an index the switch does not rebuild: {other:?}",
                index.name
            ),
        };
        if target.count_rows(None).await? == 0 {
            continue;
        }
        target
            .create_index(columns.as_slice(), kind)
            .execute()
            .await?;
    }
    Ok(())
}

/// Removes every version of `name` but the current one, with the rows deleted
/// from it: no earlier copy of a changed or forgotten row stays on disk.
async fn prune_old_versions(db: &lancedb::Connection, name: &str) -> Result<()> {
    let names = db.table_names().execute().await?;
    if !names.iter().any(|n| n == name) {
        return Ok(());
    }
    let table = db.open_table(name).execute().await?;
    table
        .optimize(OptimizeAction::Compact {
            options: CompactionOptions {
                materialize_deletions: true,
                materialize_deletions_threshold: 0.0,
                ..CompactionOptions::default()
            },
            remap_options: None,
        })
        .await?;
    table
        .optimize(OptimizeAction::Prune {
            older_than: Some(LanceDuration::zero()),
            delete_unverified: Some(true),
            error_if_tagged_old_versions: Some(false),
        })
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "model_switch_tests.rs"]
mod tests;
