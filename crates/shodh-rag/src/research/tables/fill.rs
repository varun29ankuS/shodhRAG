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
//!
//! Gate. Every fill goes through a [`CallGate`]: there is no ungated way to call the
//! model. The gate is asked once a paper's calls are planned (it can refuse a paper whose
//! planned calls and retry no longer fit the day's cap) and before every model call (it
//! counts the call, or halts the fill: cancelled, or the cap reached). A halt, like a
//! quota error, leaves every cell the fill had not answered yet exactly as it was.
//!
//! Results survive failures. A refill that fails over a cell holding a result (a stale
//! answer being refreshed, or a cut `not_found` asked again) keeps the value, quote and
//! status, and only records `refill_failed:<code>` as the cell's error (see
//! [`TableStore::store_fill`]); the next fill tries it again.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::budget::{plan_calls, MAX_CALLS_PER_PAPER, PASSAGES_PER_COLUMN};
use super::parse::{parse_cut_reply, parse_reply, ParsedReply};
use super::passages::{result_hint_terms, PaperIndex, RESULT_HINTS_PER_COLUMN};
use super::prompt::{fill_prompt, format_correction, missing_columns_prompt, reply_budget};
use super::store::{CacheEntry, TableStore, REFILL_FAILED};
use super::validate::{
    check, ground, Checked, RawAnswer, NOT_FOUND_PARTIAL, NOT_FOUND_PARTIAL_RETRIED,
};
use super::{collapse, Cell, CellStatus, Column, ColumnKind, ColumnSpec, Passage, TableRow};
use crate::harness::grounding::verify::ScorerSet;
use crate::harness::provider_error::{classify, reset_epoch_ms, safe_detail, ProviderErrorKind};
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

/// Passages per column searched for a refill of one cell by hand ([`Filler::refill_cell`]):
/// twice a fill's [`PASSAGES_PER_COLUMN`], so a refill can find an answer the first
/// search missed (spec §10).
pub const REFILL_PASSAGES: usize = 8;

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

/// The window of a quota or rate limit, as far as the provider's message tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitWindow {
    /// A per-minute (or otherwise short) rate limit: `free-models-per-min`, "requests per
    /// minute", an upstream provider's temporary limit.
    Short,
    /// A daily limit or an exhausted quota: `free-models-per-day`, "daily", no credits.
    Daily,
    /// A rate limit that does not say which (a bare 429).
    Unknown,
}

impl LimitWindow {
    /// The window as a paused job's `last_error` names it (`model_quota:<window>`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Daily => "daily",
            Self::Unknown => "unknown",
        }
    }
}

/// A failed model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFailure {
    pub kind: ModelFailureKind,
    /// For a quota or rate-limit failure, which kind of limit it was (the provider
    /// classifier's rate-limited and quota-exhausted kept apart); `Unknown` otherwise.
    pub window: LimitWindow,
    /// Seconds the provider asked to wait, when it said (capped at an hour).
    pub retry_after_secs: Option<u32>,
    /// The provider's `X-RateLimit-Reset` (Unix milliseconds) as it was sent, past or
    /// future: a daily quota keeps its real reset time here, which `retry_after_secs` does
    /// not carry, and a reset that reads as past (a clock running fast) is still told
    /// apart from no header at all.
    pub reset_at_ms: Option<u64>,
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
        let (window, reset_at_ms) = if kind == ModelFailureKind::Quota {
            (limit_window(found.kind, message), header_reset_ms(message))
        } else {
            (LimitWindow::Unknown, None)
        };
        let failure = Self {
            kind,
            window,
            retry_after_secs: found.retry_after_secs,
            reset_at_ms,
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

/// Which limit a quota or rate-limit error is: an exhausted quota or a message that names
/// a day is daily; one that names a minute, or an upstream provider's temporary limit, is
/// short; a bare rate limit is unknown.
fn limit_window(kind: ProviderErrorKind, message: &str) -> LimitWindow {
    let lower = message.to_ascii_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if kind == ProviderErrorKind::QuotaExhausted || says(&["per-day", "per day", "daily"]) {
        LimitWindow::Daily
    } else if says(&[
        "per-min",
        "per min",
        "per minute",
        "rate-limited upstream",
        "temporarily rate-limited",
    ]) {
        LimitWindow::Short
    } else {
        LimitWindow::Unknown
    }
}

/// The `X-RateLimit-Reset` a provider message carries (OpenRouter puts the response
/// headers in its error body), as Unix milliseconds, whether past or future.
fn header_reset_ms(message: &str) -> Option<u64> {
    const MARKER: &str = "x-ratelimit-reset";
    let lower = message.to_ascii_lowercase();
    let at = lower.find(MARKER)?;
    reset_epoch_ms(&lower[at + MARKER.len()..])
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
            cell.error
                .as_deref()
                .is_some_and(|e| e.starts_with(REFILL_FAILED))
                || cell.column_revision != Some(column.revision)
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

/// What a fill did: the cells it stored (as stored: a failed refill over a result is
/// that result with its `refill_failed` error), the model calls it made (a retry
/// included), the columns answered from the cache, the model's error when a call failed,
/// the gate's halt when it stopped the fill, and the local cause when the whole paper
/// could not be filled (`paper_unreadable`, `paper_not_indexed`, `search_failed`, with
/// detail).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaperFill {
    pub cells: Vec<Cell>,
    pub calls: usize,
    pub cache_hits: usize,
    pub model_error: Option<ModelFailure>,
    pub halted: Option<Halt>,
    pub paper_error: Option<String>,
}

/// Why a [`CallGate`] stopped a fill before a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Halt {
    /// The job was cancelled (or is no longer this run's).
    Cancelled,
    /// The call would pass the day's cap of table calls.
    DailyCap,
}

/// Asked before a paper's calls and before every model call of a fill. `None` lets the
/// fill go on; `Some` stops it: no more calls, and the cells it had not answered are left
/// as they are. An error stops the fill with that error, with no call made.
#[async_trait::async_trait]
pub trait CallGate: Send + Sync {
    /// Once the paper's calls are planned and before the first: `calls` is the most the
    /// paper can still spend (its planned calls and the one retry).
    async fn before_paper(&self, calls: usize) -> ResearchResult<Option<Halt>>;
    /// Before every model call; `None` means the gate has counted the call.
    async fn before_call(&self) -> ResearchResult<Option<Halt>>;
}

/// What a fill of one paper would do before any search: the paper's hash (`None` when
/// the file cannot be read), the columns that need a fill, and those of them the cache
/// cannot answer. Indices are into [`PaperRequest::columns`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Survey {
    pub(super) hash: Option<String>,
    pub(super) needing: Vec<usize>,
    pub(super) misses: Vec<usize>,
}

/// The search query of a column: its question and instructions and, for a `number`
/// column, the words of the paper's accepted Results that match it (they say where its
/// numbers are, and steer the search to the real table text).
fn column_query(column: &Column, accepted: &[ResultRecord]) -> String {
    let query = collapse(&format!(
        "{} {}",
        column.spec.question, column.spec.instructions
    ));
    if column.spec.kind != ColumnKind::Number {
        return query;
    }
    let hints = result_hint_terms(
        accepted,
        &column.spec.question,
        &column.spec.instructions,
        RESULT_HINTS_PER_COLUMN,
    );
    if hints.is_empty() {
        query
    } else {
        format!("{query} {hints}")
    }
}

/// The accepted Results of a request (the only ones used, and only as search hints).
fn accepted_results(request: &PaperRequest) -> Vec<ResultRecord> {
    request
        .results
        .iter()
        .filter(|r| r.status == ResultStatus::Accepted)
        .cloned()
        .collect()
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
    /// The gate stopped the fill before the call; the halt is in `PaperFill::halted`.
    Halted,
}

impl Filler {
    /// Fills one paper, asking `gate` before its calls and before every model call.
    pub async fn fill_paper(
        &self,
        request: &PaperRequest,
        gate: &dyn CallGate,
    ) -> ResearchResult<PaperFill> {
        self.fill(request, gate, None).await
    }

    /// Refills one cell by hand: the column `column_id` of the request's paper is asked
    /// again even when its cell holds a current result, without the cache (it would give
    /// back the answer being replaced) and with [`REFILL_PASSAGES`] passages searched. A
    /// failed refill keeps the result the cell holds ([`TableStore::store_fill`]). An
    /// edited cell is refused: a refill is never stored over the user's own value.
    pub async fn refill_cell(
        &self,
        request: &PaperRequest,
        column_id: &str,
        gate: &dyn CallGate,
    ) -> ResearchResult<PaperFill> {
        let Some(column) = request.columns.iter().find(|c| c.id == column_id) else {
            return Err(ResearchError::NotFound(format!("No column {column_id}.")));
        };
        let edited = request.cells.iter().any(|c| {
            c.row_id == request.row.id && c.column_id == column.id && c.status == CellStatus::Edited
        });
        if edited {
            return Err(ResearchError::Invalid(
                "This cell holds your own value. Clear it before asking the model again."
                    .to_string(),
            ));
        }
        self.fill(request, gate, Some(column_id)).await
    }

    /// The fill of one paper; `only` forces a refill of that one column (see
    /// [`Self::refill_cell`]).
    async fn fill(
        &self,
        request: &PaperRequest,
        gate: &dyn CallGate,
        only: Option<&str>,
    ) -> ResearchResult<PaperFill> {
        let row = &request.row;
        let per_column = if only.is_some() {
            REFILL_PASSAGES
        } else {
            PASSAGES_PER_COLUMN
        };
        let asked: Vec<&Column> = request
            .columns
            .iter()
            .filter(|c| only.is_none_or(|id| c.id == id))
            .collect();
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
                let columns: Vec<&Column> = asked
                    .iter()
                    .copied()
                    .filter(|c| {
                        let cell = existing.get(c.id.as_str()).copied();
                        !cell.is_some_and(|cell| cell.status == CellStatus::Edited)
                            && !holds_result(cell)
                    })
                    .collect();
                let error = with_detail(PAPER_UNREADABLE, &e.to_string());
                self.fail(row, &columns, None, &model_id, &error, &mut fill)
                    .await?;
                fill.paper_error = Some(error);
                return Ok(fill);
            }
        };
        let columns: Vec<&Column> = asked
            .iter()
            .copied()
            .filter(|c| {
                let cell = existing.get(c.id.as_str()).copied();
                if only.is_some() {
                    // A forced refill asks again whatever the cell holds, but an edit.
                    !cell.is_some_and(|cell| cell.status == CellStatus::Edited)
                } else {
                    needs_fill(cell, c, &hash)
                }
            })
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
        let accepted = accepted_results(request);
        let (first, ranked) = match self.retrieve(&path, &columns, &accepted, per_column).await {
            Ok(found) => found,
            Err(e) => {
                let error = with_detail(SEARCH_FAILED, &e);
                self.fail(row, &columns, Some(&hash), &model_id, &error, &mut fill)
                    .await?;
                fill.paper_error = Some(error);
                return Ok(fill);
            }
        };
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
            fill.paper_error = Some(error);
            return Ok(fill);
        }

        // 2. The cache: an unchanged column of an unchanged paper costs no call. An entry
        // lives only while a cell of some table uses it, so this serves a second table
        // asking the same question of the same paper.
        let mut misses: Vec<usize> = Vec::new();
        for (i, column) in columns.iter().enumerate() {
            if only.is_some() {
                // A refill asks the model again: the cache holds the answer it replaces.
                misses.push(i);
                continue;
            }
            match self.cached_fill(&hash, column, &model_id).await? {
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
        let plans = plan_calls(first.as_ref(), &miss_ranked);
        if !plans.is_empty() {
            if let Some(halt) = gate.before_paper(plans.len() + 1).await? {
                fill.halted = Some(halt);
            }
        }
        for plan in plans {
            let call_columns: Vec<&Column> =
                plan.columns.iter().map(|&m| columns[misses[m]]).collect();
            if fill.halted.is_some() {
                // The gate stopped the fill: these cells are left as they are.
                continue;
            }
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
            let mut attempt = self
                .ask(&prompt, call_columns.len(), gate, &mut fill)
                .await?;
            if matches!(attempt, Attempt::Unparsable) && !retried {
                retried = true;
                attempt = self
                    .ask(
                        &format_correction(&prompt),
                        call_columns.len(),
                        gate,
                        &mut fill,
                    )
                    .await?;
            }
            let reply = match attempt {
                Attempt::Cells(reply) => reply,
                Attempt::Halted => continue,
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
                    if let Attempt::Cells(more) = self
                        .ask(&retry_prompt, missing.len(), gate, &mut fill)
                        .await?
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
                    if fill.halted.is_some() {
                        // The retry that would have answered it was stopped by the gate.
                        continue;
                    }
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

    /// One model call and what came of it, after the gate let it go. A failure is
    /// recorded in `fill.model_error`, a halt in `fill.halted`.
    async fn ask(
        &self,
        prompt: &str,
        columns: usize,
        gate: &dyn CallGate,
        fill: &mut PaperFill,
    ) -> ResearchResult<Attempt> {
        if let Some(halt) = gate.before_call().await? {
            fill.halted = Some(halt);
            return Ok(Attempt::Halted);
        }
        fill.calls += 1;
        Ok(
            match self
                .model
                .complete_reply(prompt, reply_budget(columns))
                .await
            {
                Ok(reply) => {
                    // A reply the provider cut off is read as cut whatever its shape:
                    // the columns it did not reach are asked again, never failed as
                    // left out, and an envelope drafted in unfinished reasoning is
                    // never taken for the answer.
                    let parsed = if reply.truncated {
                        parse_cut_reply(&reply.text)
                    } else {
                        parse_reply(&reply.text)
                    };
                    match parsed {
                        Some(parsed) => Attempt::Cells(parsed),
                        None => Attempt::Unparsable,
                    }
                }
                Err(message) => {
                    fill.model_error = Some(ModelFailure::classify(&message));
                    Attempt::Failed
                }
            },
        )
    }

    /// The paper's first passage and each column's passages (best first, at most
    /// `per_column`), searched locally. An error is the index's message.
    async fn retrieve(
        &self,
        path: &str,
        columns: &[&Column],
        accepted: &[ResultRecord],
        per_column: usize,
    ) -> Result<(Option<Passage>, Vec<Vec<Passage>>), String> {
        let first = self.index.first_passage(path).await?;
        let mut ranked: Vec<Vec<Passage>> = Vec::with_capacity(columns.len());
        for column in columns {
            let query = column_query(column, accepted);
            let mut passages = self.index.search_paper(path, &query, per_column).await?;
            passages.truncate(per_column);
            ranked.push(passages);
        }
        Ok((first, ranked))
    }

    /// The cache entry of a column of a paper for this model, when there is a readable one.
    async fn cached_fill(
        &self,
        hash: &str,
        column: &Column,
        model: &str,
    ) -> ResearchResult<Option<CachedFill>> {
        let store = self.store.clone();
        let (h, k, m) = (hash.to_string(), column.key.clone(), model.to_string());
        let cached = blocking(move || store.cached(&h, &k, &m)).await?;
        Ok(cached.and_then(|json| serde_json::from_str::<CachedFill>(&json).ok()))
    }

    /// What a fill of this paper would do before any search ([`Survey`]): the same hash,
    /// [`needs_fill`] and cache lookups the fill makes. No model call, no search.
    pub(super) async fn survey(&self, request: &PaperRequest) -> ResearchResult<Survey> {
        let row = &request.row;
        let path = row.document_id.clone();
        let Ok(hash) = blocking(move || paper_hash(&path)).await else {
            return Ok(Survey::default());
        };
        let model_id = self.model.model_id();
        let mut survey = Survey {
            hash: Some(hash.clone()),
            ..Survey::default()
        };
        for (i, column) in request.columns.iter().enumerate() {
            let cell = request
                .cells
                .iter()
                .find(|c| c.row_id == row.id && c.column_id == column.id);
            if !needs_fill(cell, column, &hash) {
                continue;
            }
            survey.needing.push(i);
            if self.cached_fill(&hash, column, &model_id).await?.is_none() {
                survey.misses.push(i);
            }
        }
        Ok(survey)
    }

    /// The model calls a fill of this paper would make, its possible retry aside, found
    /// with local retrieval only: [`plan_calls`] over the passages of the columns the
    /// cache misses. A paper the fill makes no call for (unreadable, not indexed, not seen
    /// by the search, or answered from the cache) needs 0.
    pub(super) async fn planned_calls(&self, request: &PaperRequest, survey: &Survey) -> usize {
        if survey.hash.is_none() || survey.misses.is_empty() {
            return 0;
        }
        let accepted = accepted_results(request);
        let columns: Vec<&Column> = survey
            .needing
            .iter()
            .filter_map(|&i| request.columns.get(i))
            .collect();
        let Ok((first, ranked)) = self
            .retrieve(
                &request.row.document_id,
                &columns,
                &accepted,
                PASSAGES_PER_COLUMN,
            )
            .await
        else {
            return 0;
        };
        if ranked.iter().all(Vec::is_empty) {
            return 0;
        }
        let miss_ranked: Vec<Vec<Passage>> = survey
            .needing
            .iter()
            .zip(ranked)
            .filter(|(i, _)| survey.misses.contains(*i))
            .map(|(_, passages)| passages)
            .collect();
        plan_calls(first.as_ref(), &miss_ranked).len()
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
    /// the cell, or changed or removed the column or row, since the fill began, and keeps
    /// a result a failure would replace; the cell is listed as the store holds it.
    async fn store_cell(
        &self,
        cell: Cell,
        cache: Option<CacheEntry>,
        fill: &mut PaperFill,
    ) -> ResearchResult<()> {
        let store = self.store.clone();
        let stored = blocking(move || {
            if !store.store_fill(&cell, cache.as_ref())? {
                return Ok(None);
            }
            if cell.status != CellStatus::Failed {
                return Ok(Some(cell));
            }
            Ok(Some(
                store.cell(&cell.row_id, &cell.column_id)?.unwrap_or(cell),
            ))
        })
        .await?;
        fill.cells.extend(stored);
        Ok(())
    }
}

#[cfg(test)]
#[path = "fill_tests.rs"]
mod tests;
