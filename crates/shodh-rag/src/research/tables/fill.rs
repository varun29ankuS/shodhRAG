//! Filling one paper's cells: the cells whose column needs it (never filled, failed,
//! queued, stale by column revision or paper hash, or a `not_found` that missed passages
//! once; never edited) are answered from the cache when the column, paper and model are
//! unchanged, else by at most two model calls over passages retrieved locally from the
//! paper, and one retry for the whole paper. Every answer goes through
//! [`super::validate`]; every stored cell says which passage, page and boxes back it.
//!
//! Every write goes through [`TableStore::store_fill`], which refuses a cell the user
//! edited meanwhile, a column that changed since the fill started and a row or column
//! that is gone, and writes the cache entry only together with a cell that was stored. A
//! fill that races with the user therefore never overwrites an edit and leaves no cache
//! entry behind that no cell uses.
//!
//! Calls. A paper costs at most [`MAX_CALLS_PER_PAPER`] planned calls and one retry in
//! all, whatever fails: the retry is spent on a reply with no cells at all (asked again
//! with an explicit format instruction) or on the columns a cut-off reply never reached
//! (asked again alone); after one retry nothing more is retried, and a later call that
//! can only fail the same way is not made.
//!
//! Model errors. A quota or rate-limit error stops the fill and leaves the cells it did
//! not reach exactly as they were, so the caller can pause and resume; other errors fail
//! the cells of the call. A cell only ever holds an error code ([`ModelFailure::code`]),
//! never the provider's text, which is logged with credentials redacted.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::budget::{plan_calls, MAX_CALLS_PER_PAPER, PASSAGES_PER_COLUMN};
use super::parse::{parse_reply, ParsedReply};
use super::passages::{result_hint_terms, PaperIndex, RESULT_HINTS_PER_COLUMN};
use super::prompt::{fill_prompt, format_correction, missing_columns_prompt, reply_budget};
use super::store::{CacheEntry, TableStore};
use super::validate::{
    check, ground, Checked, RawAnswer, NOT_FOUND_PARTIAL, NOT_FOUND_PARTIAL_RETRIED,
};
use super::{collapse, Cell, CellStatus, Column, ColumnKind, ColumnSpec, Passage, TableRow};
use crate::harness::grounding::verify::ScorerSet;
use crate::harness::provider_error::{classify, safe_detail, ProviderErrorKind};
use crate::research::results::{ResultRecord, ResultStatus, TextModel};
use crate::research::{blocking, ResearchError, ResearchResult};

pub const PAPER_UNREADABLE: &str = "paper_unreadable";
pub const PAPER_NOT_INDEXED: &str = "paper_not_indexed";
pub const SEARCH_FAILED: &str = "search_failed";
pub const MODEL_FAILED: &str = "model_failed";
pub const MODEL_TIMEOUT: &str = "model_timeout";
pub const MODEL_AUTH: &str = "model_auth";
pub const MODEL_QUOTA: &str = "model_quota";
pub const UNPARSABLE_RESPONSE: &str = "unparsable_response";
pub const MISSING_IN_REPLY: &str = "missing_in_reply";

/// Longest provider or I/O detail kept (in the log, in a returned [`ModelFailure`], and
/// in the detail of a search or file error on a cell).
const MAX_DETAIL_CHARS: usize = 300;

/// What went wrong with a model call, as far as the fill needs to tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFailureKind {
    /// A rate limit or an exhausted quota: waiting helps, trying the next paper does not.
    Quota,
    /// A timeout, a broken connection or an unavailable model: worth trying again soon.
    Transient,
    /// The provider rejected the API key.
    Auth,
    /// Anything else (a refused request, an unreadable answer from the provider).
    Other,
}

/// A failed model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFailure {
    pub kind: ModelFailureKind,
    /// Seconds the provider asked to wait, when it said.
    pub retry_after_secs: Option<u32>,
    /// The provider's message, without credentials and cut short. For the caller's log and
    /// messages; it is never stored in a cell.
    pub detail: String,
}

/// Prefixes of provider API keys (Google, Groq, Hugging Face, xAI, Perplexity, and the
/// `sk-` family of OpenAI, OpenRouter and Anthropic).
const KEY_PREFIXES: [&str; 6] = ["AIza", "gsk_", "hf_", "xai-", "pplx-", "sk-"];
/// Shortest token with a known key prefix that is taken for a key.
const MIN_KEY_CHARS: usize = 20;
/// Shortest letters-and-digits token of any kind that is taken for a key or a secret.
const MIN_OPAQUE_TOKEN_CHARS: usize = 32;

fn looks_like_key(token: &str) -> bool {
    let length = token.chars().count();
    if length >= MIN_KEY_CHARS && KEY_PREFIXES.iter().any(|p| token.starts_with(p)) {
        return true;
    }
    length >= MIN_OPAQUE_TOKEN_CHARS
        && token.chars().any(|c| c.is_ascii_digit())
        && token.chars().any(|c| c.is_ascii_alphabetic())
}

fn push_token(out: &mut String, token: &mut String) {
    if looks_like_key(token) {
        out.push_str("[REDACTED]");
    } else {
        out.push_str(token);
    }
    token.clear();
}

/// `text` with everything shaped like a credential replaced by `[REDACTED]`: what
/// `safe_detail` masks (`sk-` keys, bearer tokens), a token with a known provider key
/// prefix, and any long token of mixed letters and digits. Over-masking a request id in a
/// log line costs nothing; leaking a key does.
fn redact_key_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
            token.push(c);
        } else {
            push_token(&mut out, &mut token);
            out.push(c);
        }
    }
    push_token(&mut out, &mut token);
    out
}

/// A provider's message made fit for a log and for the caller: credentials masked, cut
/// short.
fn safe_message(message: &str) -> String {
    redact_key_like(&safe_detail(message, None, usize::MAX))
        .chars()
        .take(MAX_DETAIL_CHARS)
        .collect()
}

impl ModelFailure {
    /// Classifies a model's error message (the provider's status line and body).
    pub fn classify(message: &str) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let found = classify(message, None, now_ms);
        let kind = match found.kind {
            ProviderErrorKind::RateLimited | ProviderErrorKind::QuotaExhausted => {
                ModelFailureKind::Quota
            }
            ProviderErrorKind::Timeout
            | ProviderErrorKind::Network
            | ProviderErrorKind::ModelUnavailable
            | ProviderErrorKind::EmptyResponse => ModelFailureKind::Transient,
            ProviderErrorKind::Auth => ModelFailureKind::Auth,
            _ => ModelFailureKind::Other,
        };
        let failure = Self {
            kind,
            retry_after_secs: found.retry_after_secs,
            detail: safe_message(message),
        };
        tracing::warn!(
            kind = ?failure.kind,
            provider_kind = ?found.kind,
            detail = %failure.detail,
            "evidence table model call failed"
        );
        failure
    }

    /// The code a failed cell carries for this error.
    pub fn code(&self) -> &'static str {
        match self.kind {
            ModelFailureKind::Quota => MODEL_QUOTA,
            ModelFailureKind::Transient => MODEL_TIMEOUT,
            ModelFailureKind::Auth => MODEL_AUTH,
            ModelFailureKind::Other => MODEL_FAILED,
        }
    }
}

/// Text with a code in front of it, for the cell errors that name a local cause (a file
/// or a search), cut short.
fn with_detail(code: &str, detail: &str) -> String {
    let detail: String = collapse(detail).chars().take(MAX_DETAIL_CHARS).collect();
    format!("{code}: {detail}")
}

fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// SHA-256 (hex) of the file's bytes: a changed PDF refills its row.
pub fn paper_hash(path: &str) -> ResearchResult<String> {
    let unreadable =
        |e: std::io::Error| ResearchError::Pdf(format!("{path} could not be read: {e}"));
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(unreadable)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Whether a fill should (re)fill this cell. A `not_found` that missed passages
/// ([`NOT_FOUND_PARTIAL`]) is filled once more: the columns left to fill are fewer then,
/// so each gets more of the call's budget. A second miss is marked
/// [`NOT_FOUND_PARTIAL_RETRIED`] and left alone until the paper or the column changes.
pub fn needs_fill(cell: Option<&Cell>, column: &Column, paper_hash: &str) -> bool {
    let Some(cell) = cell else {
        return true;
    };
    match cell.status {
        CellStatus::Edited => false,
        CellStatus::Empty | CellStatus::Queued | CellStatus::Failed => true,
        CellStatus::Verified | CellStatus::Unverified | CellStatus::NotFound => {
            cell.column_revision != Some(column.revision)
                || cell.paper_hash.as_deref() != Some(paper_hash)
                || (cell.status == CellStatus::NotFound
                    && cell.error.as_deref() == Some(NOT_FOUND_PARTIAL))
        }
    }
}

/// Whether the cell holds a model's result (right or stale) rather than nothing or a
/// failure.
fn holds_result(cell: Option<&Cell>) -> bool {
    cell.is_some_and(|c| {
        matches!(
            c.status,
            CellStatus::Verified | CellStatus::Unverified | CellStatus::NotFound
        )
    })
}

/// One paper to fill: its row, the table's columns, the row's current cells and its
/// Results read from its tables (only accepted ones are used, and only as search hints
/// for `number` columns).
#[derive(Debug, Clone)]
pub struct PaperRequest {
    pub row: TableRow,
    pub columns: Vec<Column>,
    pub cells: Vec<Cell>,
    pub results: Vec<ResultRecord>,
}

/// What a fill did: the cells it stored, the model calls it made (a retry included),
/// the columns answered from the cache, and the model's error when a call failed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaperFill {
    pub cells: Vec<Cell>,
    pub calls: usize,
    pub cache_hits: usize,
    pub model_error: Option<ModelFailure>,
}

/// Fills papers with one model, index and set of local scorers.
#[derive(Clone)]
pub struct Filler {
    pub store: Arc<TableStore>,
    pub index: Arc<dyn PaperIndex>,
    pub model: Arc<dyn TextModel>,
    pub scorers: ScorerSet,
}

/// The passage a cached answer's quote was found in, kept with the answer so that a replay
/// checks the quote against the very text the original call did.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedPassage {
    chunk_id: String,
    text: String,
    page: Option<u32>,
    bboxes: Option<String>,
}

impl From<&Passage> for CachedPassage {
    fn from(p: &Passage) -> Self {
        Self {
            chunk_id: p.chunk_id.clone(),
            text: p.text.clone(),
            page: p.page,
            bboxes: p.bboxes.clone(),
        }
    }
}

impl From<CachedPassage> for Passage {
    fn from(p: CachedPassage) -> Self {
        Self {
            chunk_id: p.chunk_id,
            text: p.text,
            page: p.page,
            bboxes: p.bboxes,
        }
    }
}

/// What a fill cache entry holds: the model's raw answer, and the passage its quote was
/// located in.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedFill {
    raw: RawAnswer,
    located: Option<CachedPassage>,
}

/// The first chunk, the column's passages and, for a cache replay, the passage the
/// original quote was found in: a chunk once.
fn pool_of(first: Option<&Passage>, ranked: &[Passage], located: Option<Passage>) -> Vec<Passage> {
    let mut pool: Vec<Passage> = Vec::with_capacity(ranked.len() + 2);
    for p in first
        .into_iter()
        .chain(ranked.iter())
        .chain(located.as_ref())
    {
        if !pool.iter().any(|q| q.chunk_id == p.chunk_id) {
            pool.push(p.clone());
        }
    }
    pool
}

/// The outcome of one model call.
enum Attempt {
    Cells(ParsedReply),
    /// A reply with no cells at all.
    Unparsable,
    /// The call failed; the failure is in `PaperFill::model_error`.
    Failed,
}

impl Filler {
    pub async fn fill_paper(&self, request: &PaperRequest) -> ResearchResult<PaperFill> {
        let row = &request.row;
        let path = row.document_id.clone();
        let model_id = self.model.model_id();
        let existing: HashMap<&str, &Cell> = request
            .cells
            .iter()
            .filter(|c| c.row_id == row.id)
            .map(|c| (c.column_id.as_str(), c))
            .collect();
        let mut fill = PaperFill::default();

        let hash = {
            let p = path.clone();
            blocking(move || paper_hash(&p)).await
        };
        let hash = match hash {
            Ok(hash) => hash,
            Err(e) => {
                // Without the file's hash a result cannot be told stale, so a cell that
                // holds one is left as it is; the others fail.
                let columns: Vec<&Column> = request
                    .columns
                    .iter()
                    .filter(|c| {
                        let cell = existing.get(c.id.as_str()).copied();
                        !cell.is_some_and(|cell| cell.status == CellStatus::Edited)
                            && !holds_result(cell)
                    })
                    .collect();
                let error = with_detail(PAPER_UNREADABLE, &e.to_string());
                self.fail(row, &columns, None, &model_id, &error, &mut fill)
                    .await?;
                return Ok(fill);
            }
        };
        let columns: Vec<&Column> = request
            .columns
            .iter()
            .filter(|c| needs_fill(existing.get(c.id.as_str()).copied(), c, &hash))
            .collect();
        if columns.is_empty() {
            return Ok(fill);
        }
        // Columns whose `not_found` already missed passages once: if they miss again they
        // are marked as retried, and the next fill leaves them be.
        let second_try: HashSet<&str> = columns
            .iter()
            .filter(|c| {
                existing.get(c.id.as_str()).is_some_and(|cell| {
                    cell.status == CellStatus::NotFound
                        && cell.error.as_deref() == Some(NOT_FOUND_PARTIAL)
                        && cell.column_revision == Some(c.revision)
                        && cell.paper_hash.as_deref() == Some(hash.as_str())
                })
            })
            .map(|c| c.id.as_str())
            .collect();

        // 1. Local retrieval: no model call.
        let first = match self.index.first_passage(&path).await {
            Ok(first) => first,
            Err(e) => {
                let error = with_detail(SEARCH_FAILED, &e);
                self.fail(row, &columns, Some(&hash), &model_id, &error, &mut fill)
                    .await?;
                return Ok(fill);
            }
        };
        let accepted: Vec<ResultRecord> = request
            .results
            .iter()
            .filter(|r| r.status == ResultStatus::Accepted)
            .cloned()
            .collect();
        let mut ranked: Vec<Vec<Passage>> = Vec::with_capacity(columns.len());
        for column in &columns {
            let mut query = collapse(&format!(
                "{} {}",
                column.spec.question, column.spec.instructions
            ));
            if column.spec.kind == ColumnKind::Number {
                // The Results already read from the paper's tables say where its numbers
                // are; their words steer the search to the real table text.
                let hints = result_hint_terms(
                    &accepted,
                    &column.spec.question,
                    &column.spec.instructions,
                    RESULT_HINTS_PER_COLUMN,
                );
                if !hints.is_empty() {
                    query = format!("{query} {hints}");
                }
            }
            let mut passages = match self
                .index
                .search_paper(&path, &query, PASSAGES_PER_COLUMN)
                .await
            {
                Ok(passages) => passages,
                Err(e) => {
                    let error = with_detail(SEARCH_FAILED, &e);
                    self.fail(row, &columns, Some(&hash), &model_id, &error, &mut fill)
                        .await?;
                    return Ok(fill);
                }
            };
            passages.truncate(PASSAGES_PER_COLUMN);
            ranked.push(passages);
        }
        if ranked.iter().all(Vec::is_empty) {
            // No chunk at all: not indexed (yet). A first chunk but no hit for any
            // column: the search cannot see this paper (a path spelling, an index in the
            // middle of a rebuild), and a call over its abstract alone would answer
            // `null` everywhere and cache it.
            let error = if first.is_none() {
                PAPER_NOT_INDEXED.to_string()
            } else {
                with_detail(SEARCH_FAILED, "no passage of the paper matched any column")
            };
            self.fail(row, &columns, Some(&hash), &model_id, &error, &mut fill)
                .await?;
            return Ok(fill);
        }

        // 2. The cache: an unchanged column of an unchanged paper costs no call. An entry
        // lives only while a cell of some table uses it, so this serves a second table
        // asking the same question of the same paper.
        let mut misses: Vec<usize> = Vec::new();
        for (i, column) in columns.iter().enumerate() {
            let store = self.store.clone();
            let (h, k, m) = (hash.clone(), column.key.clone(), model_id.clone());
            let cached = blocking(move || store.cached(&h, &k, &m)).await?;
            match cached.and_then(|json| serde_json::from_str::<CachedFill>(&json).ok()) {
                Some(cached) => {
                    // The replay is checked against what the original call was: its
                    // passage numbers belong to another call's numbering and are ignored,
                    // and the passage the quote was found in is part of the pool, even
                    // when this column's own search did not retrieve it.
                    let pool = pool_of(
                        first.as_ref(),
                        &ranked[i],
                        cached.located.map(Passage::from),
                    );
                    let raw = RawAnswer {
                        passage: None,
                        ..cached.raw
                    };
                    let checked = check(&column.spec, &raw, &pool);
                    fill.cache_hits += 1;
                    self.finish(
                        row, column, checked, &pool, &model_id, &hash, None, &mut fill,
                    )
                    .await?;
                }
                None => misses.push(i),
            }
        }

        // 3. At most two calls over the missed columns, and one retry for the paper.
        let miss_ranked: Vec<Vec<Passage>> = misses.iter().map(|&i| ranked[i].clone()).collect();
        let mut retried = false;
        let mut format_given_up = false;
        for plan in plan_calls(first.as_ref(), &miss_ranked) {
            let call_columns: Vec<&Column> =
                plan.columns.iter().map(|&m| columns[misses[m]]).collect();
            if let Some(failure) = fill.model_error.clone() {
                self.stop(row, &call_columns, &hash, &model_id, &failure, &mut fill)
                    .await?;
                continue;
            }
            if format_given_up {
                self.fail(
                    row,
                    &call_columns,
                    Some(&hash),
                    &model_id,
                    UNPARSABLE_RESPONSE,
                    &mut fill,
                )
                .await?;
                continue;
            }
            let tags: Vec<String> = (1..=call_columns.len()).map(|n| format!("c{n}")).collect();
            let specs: Vec<(&str, &ColumnSpec)> = tags
                .iter()
                .map(String::as_str)
                .zip(call_columns.iter().map(|c| &c.spec))
                .collect();
            let prompt = fill_prompt(&plan.passages, &specs);
            let mut attempt = self.ask(&prompt, call_columns.len(), &mut fill).await;
            if matches!(attempt, Attempt::Unparsable) && !retried {
                retried = true;
                attempt = self
                    .ask(&format_correction(&prompt), call_columns.len(), &mut fill)
                    .await;
            }
            let reply = match attempt {
                Attempt::Cells(reply) => reply,
                Attempt::Unparsable => {
                    format_given_up = true;
                    self.fail(
                        row,
                        &call_columns,
                        Some(&hash),
                        &model_id,
                        UNPARSABLE_RESPONSE,
                        &mut fill,
                    )
                    .await?;
                    continue;
                }
                Attempt::Failed => {
                    if let Some(failure) = fill.model_error.clone() {
                        self.stop(row, &call_columns, &hash, &model_id, &failure, &mut fill)
                            .await?;
                    }
                    continue;
                }
            };
            // A column repeated in the reply keeps its first entry.
            let mut by_tag: HashMap<String, RawAnswer> = HashMap::new();
            for cell in reply.cells {
                by_tag.entry(cell.column).or_insert(cell.raw);
            }
            if reply.cut_off && !retried {
                let missing: Vec<usize> = (0..call_columns.len())
                    .filter(|&n| !by_tag.contains_key(&tags[n]))
                    .collect();
                if !missing.is_empty() {
                    retried = true;
                    let retry_tags: Vec<String> =
                        (1..=missing.len()).map(|n| format!("c{n}")).collect();
                    let retry_specs: Vec<(&str, &ColumnSpec)> = retry_tags
                        .iter()
                        .map(String::as_str)
                        .zip(missing.iter().map(|&n| &call_columns[n].spec))
                        .collect();
                    let retry_prompt = missing_columns_prompt(&plan.passages, &retry_specs);
                    if let Attempt::Cells(more) =
                        self.ask(&retry_prompt, missing.len(), &mut fill).await
                    {
                        for cell in more.cells {
                            if let Some(at) = retry_tags.iter().position(|t| *t == cell.column) {
                                by_tag.entry(tags[missing[at]].clone()).or_insert(cell.raw);
                            }
                        }
                    }
                }
            }
            for (n, column) in call_columns.iter().enumerate() {
                let Some(raw) = by_tag.remove(&tags[n]) else {
                    match fill.model_error.clone() {
                        Some(failure) => {
                            self.stop(row, &[*column], &hash, &model_id, &failure, &mut fill)
                                .await?
                        }
                        None => {
                            self.fail(
                                row,
                                &[*column],
                                Some(&hash),
                                &model_id,
                                MISSING_IN_REPLY,
                                &mut fill,
                            )
                            .await?
                        }
                    }
                    continue;
                };
                let mut checked = check(&column.spec, &raw, &plan.passages);
                let truncated = plan.truncated.contains(&plan.columns[n]);
                let partial = checked.status == CellStatus::NotFound && truncated;
                if partial {
                    checked.error = Some(
                        if second_try.contains(column.id.as_str()) {
                            NOT_FOUND_PARTIAL_RETRIED
                        } else {
                            NOT_FOUND_PARTIAL
                        }
                        .to_string(),
                    );
                }
                // A reply that did not answer (no `answer` key) is a failure to retry, and
                // a "not found" over passages that were cut short is not final: neither is
                // cached as the model's answer.
                let cache = if checked.status != CellStatus::Failed && !partial {
                    let located = checked
                        .passage
                        .and_then(|i| plan.passages.get(i))
                        .map(CachedPassage::from);
                    serde_json::to_string(&CachedFill {
                        raw: raw.clone(),
                        located,
                    })
                    .ok()
                    .map(|response_json| CacheEntry {
                        paper_hash: hash.clone(),
                        column_key: column.key.clone(),
                        model: model_id.clone(),
                        response_json,
                    })
                } else {
                    None
                };
                self.finish(
                    row,
                    column,
                    checked,
                    &plan.passages,
                    &model_id,
                    &hash,
                    cache,
                    &mut fill,
                )
                .await?;
            }
        }
        debug_assert!(fill.calls <= MAX_CALLS_PER_PAPER + 1);
        Ok(fill)
    }

    /// One model call and what came of it. A failure is recorded in `fill.model_error`.
    async fn ask(&self, prompt: &str, columns: usize, fill: &mut PaperFill) -> Attempt {
        fill.calls += 1;
        match self.model.complete(prompt, reply_budget(columns)).await {
            Ok(reply) => match parse_reply(&reply) {
                Some(parsed) => Attempt::Cells(parsed),
                None => Attempt::Unparsable,
            },
            Err(message) => {
                fill.model_error = Some(ModelFailure::classify(&message));
                Attempt::Failed
            }
        }
    }

    /// What a model failure does to the cells of the call that met it, or was due to make
    /// it: a quota or rate-limit error leaves them exactly as they were (the caller pauses
    /// and resumes); any other error fails them with its code.
    async fn stop(
        &self,
        row: &TableRow,
        columns: &[&Column],
        hash: &str,
        model: &str,
        failure: &ModelFailure,
        fill: &mut PaperFill,
    ) -> ResearchResult<()> {
        if failure.kind == ModelFailureKind::Quota {
            return Ok(());
        }
        self.fail(row, columns, Some(hash), model, failure.code(), fill)
            .await
    }

    /// Runs the grounding check (a local model, on a blocking thread) and stores the cell.
    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        row: &TableRow,
        column: &Column,
        checked: Checked,
        passages: &[Passage],
        model: &str,
        hash: &str,
        cache: Option<CacheEntry>,
        fill: &mut PaperFill,
    ) -> ResearchResult<()> {
        let question = column.spec.question.clone();
        let scorers = self.scorers.clone();
        let checked = tokio::task::spawn_blocking(move || ground(checked, &question, &scorers))
            .await
            .map_err(|e| ResearchError::Task(e.to_string()))?;
        let matched = checked.passage.and_then(|i| passages.get(i));
        let cell = Cell {
            table_id: row.table_id.clone(),
            row_id: row.id.clone(),
            column_id: column.id.clone(),
            status: checked.status,
            value: checked.value,
            quote: checked.quote,
            chunk_id: matched.map(|p| p.chunk_id.clone()),
            page: matched.and_then(|p| p.page),
            bboxes: matched.and_then(|p| p.bboxes.clone()),
            model: Some(model.to_string()),
            column_revision: Some(column.revision),
            paper_hash: Some(hash.to_string()),
            filled_at: Some(now_text()),
            edited_at: None,
            error: checked.error,
        };
        self.store_cell(cell, cache, fill).await
    }

    /// Stores `failed` cells for `columns` with `error`.
    async fn fail(
        &self,
        row: &TableRow,
        columns: &[&Column],
        hash: Option<&str>,
        model: &str,
        error: &str,
        fill: &mut PaperFill,
    ) -> ResearchResult<()> {
        for column in columns {
            let cell = Cell {
                table_id: row.table_id.clone(),
                row_id: row.id.clone(),
                column_id: column.id.clone(),
                status: CellStatus::Failed,
                value: None,
                quote: None,
                chunk_id: None,
                page: None,
                bboxes: None,
                model: Some(model.to_string()),
                column_revision: Some(column.revision),
                paper_hash: hash.map(str::to_string),
                filled_at: Some(now_text()),
                edited_at: None,
                error: Some(error.to_string()),
            };
            self.store_cell(cell, None, fill).await?;
        }
        Ok(())
    }

    /// Writes one cell. The store refuses it (and its cache entry) when the user edited
    /// the cell, or changed or removed the column or row, since the fill began.
    async fn store_cell(
        &self,
        cell: Cell,
        cache: Option<CacheEntry>,
        fill: &mut PaperFill,
    ) -> ResearchResult<()> {
        let store = self.store.clone();
        let stored = cell.clone();
        let written = blocking(move || store.store_fill(&stored, cache.as_ref())).await?;
        if written {
            fill.cells.push(cell);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "fill_tests.rs"]
mod tests;
