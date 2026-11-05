//! Moving an index to another E5 export ("Re-index for faster search", and
//! back) without ever mixing vectors of two exports ([`super::index_model`]).
//!
//! The index in use is never touched. A switch builds new LanceDB tables
//! beside the old ones: every row is copied with its vector embedded again
//! from the text it was embedded from, all other columns unchanged, so the
//! keyword index and every id stay valid. The way back to the standard model
//! builds the original table names (`documents`, `statements`) when they are
//! free, so a library on the standard model stays readable by older releases.
//! The build runs in the background, can be paused and cancelled, and resumes
//! after a restart (a row already copied unchanged is not embedded again).
//! Search keeps using the old tables and the old export meanwhile.
//!
//! The build reads the source a step (32 rows) at a time from its latest
//! version, under a lock per data folder that erasing a memory also takes
//! ([`erase_copies`]): a step never copies a row erased before it, and an
//! erase deletes the copy a step made before it. No read of the source lasts
//! longer than a step, so pruning its old versions never pulls files from
//! under the build.
//!
//! The switch completes at the next start, before anything opens the tables,
//! when few rows changed since the build ([`MAX_CHANGES_AT_START`]; with more,
//! the build goes on in the background and the switch waits for the start
//! after). Completing it brings the changes over, rebuilds the statements'
//! full-text index and prunes old versions (no word or copy of an erased
//! memory stays), checks that both new tables hold exactly the rows of the
//! old ones, and only then replaces the index record, atomically. That
//! rename is the switch. The old tables stay for one start as a way back
//! ([`IndexModel::retired`]) and are dropped at the start after, each only if
//! no one wrote to it since. Nothing is ever dropped by exclusion: only the
//! switch's own new tables (on a cancel) and retired tables.
//!
//! If completing fails, the old index stays in use and the marker keeps the
//! reason (`finish_error`) for the System status, which offers Retry (the
//! next start) or Cancel.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

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
use super::index_model::{self, IndexModel, RetiredTable, DOCUMENTS_TABLE};
use super::EmbeddingModel;
use crate::statements::STATEMENTS_TABLE;

/// The marker's file in the data folder.
pub const SWITCH_FILE: &str = "search-model-switch.json";
/// The LanceDB folder (in the data folder) holding both vector tables.
pub const LANCE_DIR: &str = "lance_data";

/// Rows embedded per step: the build checks for a pause or a cancel between
/// steps (about a second of int8 work on a laptop).
const STEP_ROWS: usize = 32;
/// The id column alone, for reads of ids.
const ID_COLUMN: &[&str] = &["id"];
/// Ids per delete statement.
const DELETE_IDS: usize = 256;
/// Rows changed since the build that a start embeds before the window opens
/// (a few seconds of int8 work); with more, the build catches up in the
/// background and the switch completes at the start after.
pub const MAX_CHANGES_AT_START: usize = 256;

/// Why a switch could not complete; each says what happened in plain words.
#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("the search model it switches to is not installed any more")]
    ModelMissing,
    #[error("the library changed its search model in another way since the re-index began")]
    RecordChanged,
    #[error("the new tables did not match the library's ({0}), so nothing was replaced")]
    Unverified(String),
}

/// A switch in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSwitch {
    pub from: E5Variant,
    pub to: E5Variant,
    /// The tables in use when it started.
    pub from_documents_table: String,
    pub from_statements_table: String,
    /// The tables it builds (its own: a cancel drops them).
    pub documents_table: String,
    pub statements_table: String,
    pub started_at: DateTime<Utc>,
    /// Every row has been embedded once in `to`: the switch completes at the
    /// next start.
    pub built: bool,
    /// Why completing it at the last start failed, in plain words: the old
    /// index stayed in use. The switch is tried again at the next start
    /// (Retry) or cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_error: Option<String>,
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

/// The lock of a data folder's switch tables: each build step and each erase
/// of a memory's copies hold it.
fn lock_for(data_dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: LazyLock<std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(Default::default);
    let key = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key)
        .or_default()
        .clone()
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

/// The table a switch to `to` builds for `base` ("documents", "statements"):
/// the standard model gets `base` itself when no table has that name (so
/// older releases read it), every other case `base_<export>`.
fn table_for(base: &str, to: E5Variant, existing: &[String]) -> String {
    if to == E5Variant::Fp16 && !existing.iter().any(|n| n == base) {
        base.to_string()
    } else {
        format!("{base}_{}", to.label())
    }
}

/// Every name a vector table of the index can have.
fn vector_table_names() -> Vec<String> {
    let mut names = Vec::new();
    for base in [DOCUMENTS_TABLE, STATEMENTS_TABLE] {
        names.push(base.to_string());
        for variant in E5Variant::ALL {
            names.push(format!("{base}_{}", variant.label()));
        }
    }
    names
}

/// Vector tables other than the original two that hold rows: with no index
/// record, they mean the record was lost and the library is in them, so no
/// new library may start over it (see `rag_engine::settle_index_model`).
pub async fn unrecorded_vector_tables(data_dir: &Path) -> Result<Vec<String>> {
    let db = connect(data_dir).await?;
    let existing = db.table_names().execute().await?;
    let mut found = Vec::new();
    for name in vector_table_names() {
        if name == DOCUMENTS_TABLE || name == STATEMENTS_TABLE || !existing.contains(&name) {
            continue;
        }
        if rows_of(&db, &name).await? > 0 {
            found.push(name);
        }
    }
    Ok(found)
}

/// Starts (or continues) a switch of the index in `data_dir` to `to`. The
/// index must be recorded and in another export.
pub async fn start(data_dir: &Path, to: E5Variant) -> Result<PendingSwitch> {
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
    let db = connect(data_dir).await?;
    let existing = db.table_names().execute().await?;
    let switch = PendingSwitch {
        from: record.variant,
        to,
        from_documents_table: record.documents_table.clone(),
        from_statements_table: record.statements_table.clone(),
        documents_table: table_for(DOCUMENTS_TABLE, to, &existing),
        statements_table: table_for(STATEMENTS_TABLE, to, &existing),
        started_at: Utc::now(),
        built: false,
        finish_error: None,
    };
    write_pending(data_dir, &switch)?;
    tracing::info!(
        from = switch.from.label(),
        to = to.label(),
        documents = %switch.documents_table,
        "Search model switch started"
    );
    Ok(switch)
}

/// Cancels the switch in `data_dir`: its own new tables are dropped (never
/// a table the index record names) and the marker removed. Returns whether
/// there was one.
pub async fn cancel(data_dir: &Path) -> Result<bool> {
    let Some(switch) = pending(data_dir)? else {
        return Ok(false);
    };
    let (in_use_documents, in_use_statements) = index_model::tables(data_dir)?;
    let lock = lock_for(data_dir);
    let _step = lock.lock().await;
    let db = connect(data_dir).await?;
    for table in [&switch.documents_table, &switch.statements_table] {
        if *table == in_use_documents || *table == in_use_statements {
            continue;
        }
        drop_table(&db, table).await?;
    }
    std::fs::remove_file(data_dir.join(SWITCH_FILE))
        .with_context(|| format!("removing {SWITCH_FILE}"))?;
    tracing::info!(to = switch.to.label(), "Search model switch cancelled");
    Ok(true)
}

/// Deletes the copies of the erased memory statements `ids` from the switch
/// in progress, if any: an erase reaches the switch's table at once, not
/// when the switch ends. Waits for a build step that is writing.
pub async fn erase_copies(data_dir: &Path, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let Some(switch) = pending(data_dir)? else {
        return Ok(());
    };
    let lock = lock_for(data_dir);
    let _step = lock.lock().await;
    let db = connect(data_dir).await?;
    if !table_exists(&db, &switch.statements_table).await? {
        return Ok(());
    }
    let table = db.open_table(&switch.statements_table).execute().await?;
    for chunk in ids.chunks(DELETE_IDS) {
        table.delete(&id_list(chunk)).await?;
    }
    Ok(())
}

/// Removes erased memories from the switch's statement table on disk as the
/// memory store's purge does for its own table: data files rewritten, the
/// full-text index rebuilt, earlier versions deleted.
pub async fn purge_copies(data_dir: &Path) -> Result<()> {
    let Some(switch) = pending(data_dir)? else {
        return Ok(());
    };
    let lock = lock_for(data_dir);
    let _step = lock.lock().await;
    let db = connect(data_dir).await?;
    purge_statements_table(&db, &switch.statements_table).await
}

/// The statement table `name`'s erased rows removed from its files, its
/// `terms` full-text index rebuilt from the rows left, and every older
/// version deleted ([`crate::statements::lance::StatementTable`]'s purge).
async fn purge_statements_table(db: &lancedb::Connection, name: &str) -> Result<()> {
    if !table_exists(db, name).await? {
        return Ok(());
    }
    let schema = db.open_table(name).execute().await?.schema().await?;
    let dimension = match schema
        .field_with_name("vector")
        .map_err(|_| anyhow!("{name} has no vector column"))?
        .data_type()
    {
        DataType::FixedSizeList(_, size) => *size as usize,
        other => bail!("{name}'s vector column is {other}"),
    };
    let table = crate::statements::lance::StatementTable::open(db, name, dimension)
        .await
        .map_err(|e| anyhow!("opening {name}: {e}"))?;
    table
        .purge_old_versions(true)
        .await
        .map_err(|e| anyhow!("purging {name}: {e}"))?;
    Ok(())
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

/// The source and target tables of a switch, with their kind of rows.
fn table_pairs(switch: &PendingSwitch) -> [(&String, &String, Rows); 2] {
    [
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
    ]
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
    let lock = lock_for(data_dir);
    let mut counter = SwitchProgress {
        done: 0,
        total: (rows_of(&db, &switch.from_documents_table).await?
            + rows_of(&db, &switch.from_statements_table).await?) as u64,
    };
    progress(counter);
    for (source, target, rows) in table_pairs(&switch) {
        let synced = sync_table(
            &db,
            source,
            target,
            rows,
            Some(&embedder),
            &lock,
            &mut stop,
            &mut counter,
            progress,
        )
        .await
        .with_context(|| format!("re-embedding {source} into {target}"))?;
        if synced.stopped {
            return Ok(BuildOutcome::Stopped);
        }
        mirror_indices(&db, source, target).await?;
    }
    switch.built = true;
    switch.finish_error = None;
    write_pending(data_dir, &switch)?;
    tracing::info!(
        to = switch.to.label(),
        rows = counter.total,
        "Search model switch built; it completes at the next start"
    );
    Ok(BuildOutcome::Built)
}

/// At start, before the engine or the memory open their tables: drops the
/// tables retired by the previous start's switch ([`drop_retired`]), then
/// completes a built switch with the model `load` gives (the switch's
/// export). Returns the new record when a switch completed. A failure keeps
/// the old index in use and is kept with the switch, in plain words.
pub async fn finish_pending<L>(data_dir: &Path, load: L) -> Result<Option<IndexModel>>
where
    L: FnOnce(E5Variant) -> Result<Arc<dyn EmbeddingModel>> + Send + 'static,
{
    if let Err(e) = drop_retired(data_dir).await {
        tracing::warn!(error = %format!("{e:#}"), "The previous search model's tables were not removed");
    }
    let finished = finish(data_dir, load).await;
    if let Err(e) = &finished {
        // Kept with the switch, so the System status says why and offers
        // Retry (the next start) or Cancel, instead of "ready, restart".
        if let Ok(Some(mut switch)) = pending(data_dir) {
            if switch.built {
                switch.finish_error = Some(finish_failure_text(e));
                if let Err(write) = write_pending(data_dir, &switch) {
                    tracing::warn!(error = %format!("{write:#}"), "The switch's failure was not saved");
                }
            }
        }
    }
    finished
}

/// What a failed completion says to the user: what happened, in plain words,
/// then the technical reason.
fn finish_failure_text(error: &anyhow::Error) -> String {
    let typed = error
        .chain()
        .find_map(|e| e.downcast_ref::<SwitchError>())
        .map(ToString::to_string);
    let missing = error.chain().any(|e| {
        e.downcast_ref::<crate::embeddings::SearchModelsMissing>()
            .is_some()
    });
    let cause = match (typed, missing) {
        (Some(cause), _) => cause,
        (None, true) => SwitchError::ModelMissing.to_string(),
        (None, false) => "an error stopped it".to_string(),
    };
    format!(
        "Shodh could not finish switching the search model when it started ({cause}), so your library kept its current model. Details: {error:#}"
    )
}

/// Drops the tables the record lists as retired, each only if it is still at
/// the version it had when it was retired; one that changed (an older
/// release wrote to it) is kept and only taken off the list. Nothing happens
/// without a readable record.
pub async fn drop_retired(data_dir: &Path) -> Result<()> {
    let Some(mut record) = index_model::read(data_dir)? else {
        return Ok(());
    };
    if record.retired.is_empty() {
        return Ok(());
    }
    let db = connect(data_dir).await?;
    for retired in std::mem::take(&mut record.retired) {
        if retired.name == record.documents_table || retired.name == record.statements_table {
            continue;
        }
        if !table_exists(&db, &retired.name).await? {
            continue;
        }
        let version = db
            .open_table(&retired.name)
            .execute()
            .await?
            .version()
            .await?;
        if version == retired.version {
            db.drop_table(&retired.name, &[]).await?;
            tracing::info!(table = %retired.name, "Removed the previous search model's table");
        } else {
            tracing::warn!(
                table = %retired.name,
                retired_at = retired.version,
                now = version,
                "A retired search table was written to since (an older release?); it is kept"
            );
        }
    }
    index_model::write_record(data_dir, &record)
}

async fn finish<L>(data_dir: &Path, load: L) -> Result<Option<IndexModel>>
where
    L: FnOnce(E5Variant) -> Result<Arc<dyn EmbeddingModel>> + Send + 'static,
{
    let Some(mut switch) = pending(data_dir)? else {
        return Ok(None);
    };
    if !switch.built {
        return Ok(None);
    }
    let record = index_model::read(data_dir)?
        .ok_or_else(|| anyhow!("a search model switch without an index record"))?;
    if record.variant == switch.to
        && record.documents_table == switch.documents_table
        && record.statements_table == switch.statements_table
    {
        // The record was replaced and the start stopped before the marker
        // went: the switch is done (its old tables are on the retire list).
        std::fs::remove_file(data_dir.join(SWITCH_FILE))?;
        return Ok(None);
    }
    if record.variant != switch.from
        || record.documents_table != switch.from_documents_table
        || record.statements_table != switch.from_statements_table
    {
        return Err(SwitchError::RecordChanged.into());
    }
    let db = connect(data_dir).await?;
    let lock = lock_for(data_dir);

    // Few changes since the build are brought over now; many would hold the
    // window, so the build catches up in the background instead.
    let mut changes = 0;
    for (source, target, rows) in table_pairs(&switch) {
        changes += sync_table(
            &db,
            source,
            target,
            rows,
            None,
            &lock,
            &mut || std::future::ready(false),
            &mut SwitchProgress::default(),
            &|_| {},
        )
        .await?
        .changes;
    }
    if changes > MAX_CHANGES_AT_START {
        tracing::info!(
            changes,
            "Many rows changed since the re-index was built; it catches up in the background first"
        );
        switch.built = false;
        write_pending(data_dir, &switch)?;
        return Ok(None);
    }

    let to = switch.to;
    let embedder = tokio::task::spawn_blocking(move || load(to))
        .await
        .map_err(|e| anyhow!("loading the search model failed: {e}"))??;
    check_embedder(embedder.as_ref(), switch.to)?;
    let mut counter = SwitchProgress::default();
    for (source, target, rows) in table_pairs(&switch) {
        sync_table(
            &db,
            source,
            target,
            rows,
            Some(&embedder),
            &lock,
            &mut || std::future::ready(false),
            &mut counter,
            &|_| {},
        )
        .await
        .with_context(|| format!("bringing {source} up to date in {target}"))?;
        mirror_indices(&db, source, target).await?;
        match rows {
            // Rebuilds the full-text index too: a fragment deleted whole
            // leaves its words in the index otherwise.
            Rows::Statements => purge_statements_table(&db, target).await?,
            Rows::Documents => prune_old_versions(&db, target).await?,
        }
        verify(&db, source, target).await?;
    }

    let mut retired = Vec::new();
    for name in [&switch.from_documents_table, &switch.from_statements_table] {
        if table_exists(&db, name).await? {
            retired.push(RetiredTable {
                name: name.clone(),
                version: db.open_table(name).execute().await?.version().await?,
            });
        }
    }
    let switched = IndexModel {
        variant: switch.to,
        recorded_at: Utc::now(),
        reason: format!("switched from {}", switch.from.label()),
        documents_table: switch.documents_table.clone(),
        statements_table: switch.statements_table.clone(),
        retired,
        ..record
    };
    // The switch.
    index_model::write_record(data_dir, &switched)?;
    tracing::info!(
        from = switch.from.label(),
        to = switch.to.label(),
        "Search model switched; the previous tables are kept until the next start"
    );
    if let Err(e) = std::fs::remove_file(data_dir.join(SWITCH_FILE)) {
        tracing::warn!(error = %e, "The finished switch's marker was not removed");
    }
    Ok(Some(switched))
}

/// Checks `target` holds exactly the rows of `source` (counts and ids): no
/// record is replaced over a table that lost or gained rows.
async fn verify(db: &lancedb::Connection, source: &str, target: &str) -> Result<()> {
    let source_exists = table_exists(db, source).await?;
    let target_exists = table_exists(db, target).await?;
    if !source_exists {
        if target_exists && rows_of(db, target).await? > 0 {
            return Err(
                SwitchError::Unverified(format!("{target} has rows {source} does not")).into(),
            );
        }
        return Ok(());
    }
    if !target_exists {
        return Err(SwitchError::Unverified(format!("{target} is missing")).into());
    }
    let (from, to) = (rows_of(db, source).await?, rows_of(db, target).await?);
    if from != to {
        return Err(
            SwitchError::Unverified(format!("{source} has {from} rows, {target} {to}")).into(),
        );
    }
    let (from_ids, to_ids) = (all_ids(db, source).await?, all_ids(db, target).await?);
    if from_ids != to_ids {
        return Err(
            SwitchError::Unverified(format!("{source} and {target} hold other ids")).into(),
        );
    }
    Ok(())
}

async fn all_ids(db: &lancedb::Connection, name: &str) -> Result<Vec<String>> {
    let table = db.open_table(name).execute().await?;
    let mut stream = table
        .query()
        .select(Select::columns(ID_COLUMN))
        .execute()
        .await?;
    let mut ids = Vec::new();
    while let Some(batch) = stream.try_next().await? {
        let column = string_column(&batch, "id")?;
        ids.extend((0..batch.num_rows()).map(|row| column.value(row).to_string()));
    }
    ids.sort();
    Ok(ids)
}

async fn table_exists(db: &lancedb::Connection, name: &str) -> Result<bool> {
    let names = db.table_names().execute().await?;
    Ok(names.iter().any(|n| n == name))
}

async fn drop_table(db: &lancedb::Connection, name: &str) -> Result<()> {
    if table_exists(db, name).await? {
        db.drop_table(name, &[]).await?;
    }
    Ok(())
}

async fn rows_of(db: &lancedb::Connection, name: &str) -> Result<usize> {
    if !table_exists(db, name).await? {
        return Ok(0);
    }
    Ok(db
        .open_table(name)
        .execute()
        .await?
        .count_rows(None)
        .await?)
}

/// What a sync did.
#[derive(Debug, Default)]
struct Synced {
    /// `stop` stopped it.
    stopped: bool,
    /// Rows that needed copying or removing (all of them counted, none
    /// written, without an embedder).
    changes: usize,
}

/// Makes `target` hold every row of `source` with its vector embedded again
/// by `embedder`: rows already in `target` with the same content are kept,
/// changed ones replaced, missing ones added, and rows `source` no longer has
/// removed. Without an embedder it only counts the rows that would change.
/// The source is read a step at a time from its latest version, under
/// `lock`.
#[allow(clippy::too_many_arguments)]
async fn sync_table<S, F>(
    db: &lancedb::Connection,
    source: &str,
    target: &str,
    rows: Rows,
    embedder: Option<&Arc<dyn EmbeddingModel>>,
    lock: &tokio::sync::Mutex<()>,
    stop: &mut S,
    counter: &mut SwitchProgress,
    progress: &(dyn Fn(SwitchProgress) + Send + Sync),
) -> Result<Synced>
where
    S: FnMut() -> F,
    F: Future<Output = bool>,
{
    let mut synced = Synced::default();
    if !table_exists(db, source).await? {
        // Nothing to copy (the memory was never opened): no target either.
        if embedder.is_some() {
            drop_table(db, target).await?;
        } else {
            synced.changes = rows_of(db, target).await?;
        }
        return Ok(synced);
    }
    let schema = db.open_table(source).execute().await?.schema().await?;
    let vector = schema
        .index_of("vector")
        .map_err(|_| anyhow!("{source} has no vector column"))?;
    let target_table = match (table_exists(db, target).await?, embedder) {
        (true, _) => {
            let table = db.open_table(target).execute().await?;
            if same_columns(&table.schema().await?, &schema) {
                Some(table)
            } else if embedder.is_some() {
                // Another layout (an older build of the table): built again.
                db.drop_table(target, &[]).await?;
                Some(
                    db.create_empty_table(target, schema.clone())
                        .execute()
                        .await?,
                )
            } else {
                None
            }
        }
        (false, Some(_)) => Some(
            db.create_empty_table(target, schema.clone())
                .execute()
                .await?,
        ),
        (false, None) => None,
    };
    let Some(target_table) = target_table else {
        synced.changes = rows_of(db, source).await?;
        return Ok(synced);
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
    drop(stream);

    // The source's ids now; each step reads its rows again, from the latest
    // version, under the lock.
    let mut ids = all_ids(db, source).await?;
    ids.dedup();
    let mut seen: HashSet<String> = HashSet::new();
    for chunk in ids.chunks(STEP_ROWS) {
        if embedder.is_some() && stop().await {
            synced.stopped = true;
            return Ok(synced);
        }
        let _step = lock.lock().await;
        let source_table = db.open_table(source).execute().await?;
        let batches: Vec<RecordBatch> = source_table
            .query()
            .only_if(id_list(chunk))
            .execute()
            .await?
            .try_collect()
            .await?;
        let mut step: Vec<(RecordBatch, usize, bool)> = Vec::new();
        for batch in &batches {
            let batch_ids = string_column(batch, "id")?;
            for row in 0..batch.num_rows() {
                let id = batch_ids.value(row).to_string();
                let print = fingerprint(batch, row)?;
                seen.insert(id.clone());
                match held.remove(&id) {
                    Some(kept) if kept == print => counter.done += 1,
                    replaced => step.push((batch.clone(), row, replaced.is_some())),
                }
            }
        }
        synced.changes += step.len();
        if let Some(embedder) = embedder {
            if !step.is_empty() {
                copy_rows(&target_table, &schema, vector, rows, embedder, &step).await?;
            }
            counter.done += step.len() as u64;
        }
        progress(*counter);
    }
    // Rows the source no longer has (erased, removed, or gone since the ids
    // were read).
    let gone: Vec<String> = held.into_keys().filter(|id| !seen.contains(id)).collect();
    synced.changes += gone.len();
    if embedder.is_some() {
        let _step = lock.lock().await;
        for ids in gone.chunks(DELETE_IDS) {
            target_table.delete(&id_list(ids)).await?;
        }
    }
    Ok(synced)
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
/// statement's text, the way the memory store embedded it. A chunk whose
/// metadata cannot be read is an error, never a guess.
fn embedding_input(rows: Rows, batch: &RecordBatch, row: usize) -> Result<String> {
    let text = string_column(batch, "text")?.value(row);
    match rows {
        Rows::Statements => Ok(text.to_string()),
        Rows::Documents => {
            let metadata = string_column(batch, "metadata_json")?.value(row);
            let title = string_column(batch, "title")?.value(row);
            crate::rag_engine::embedded_text(text, title, metadata).with_context(|| {
                format!(
                    "chunk {} has metadata the switch cannot read",
                    string_column(batch, "id")
                        .map(|ids| ids.value(row).to_string())
                        .unwrap_or_default()
                )
            })
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
    if !table_exists(db, source).await? || !table_exists(db, target).await? {
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
/// from it: no earlier copy of a changed or removed row stays on disk.
async fn prune_old_versions(db: &lancedb::Connection, name: &str) -> Result<()> {
    if !table_exists(db, name).await? {
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
