//! Filling one paper's cells: the cells whose column needs it (never filled, failed,
//! queued, stale by column revision or paper hash; never edited) are answered from the
//! cache when the column, paper and model are unchanged, else by at most two model calls
//! over passages retrieved locally from the paper. Every answer goes through
//! [`super::validate`]; every stored cell says which passage, page and boxes back it.
//!
//! Every write goes through [`TableStore::store_fill`], which refuses a cell the user
//! edited meanwhile, a column that changed since the fill started and a row or column
//! that is gone, and writes the cache entry only together with a cell that was stored. A
//! fill that races with the user therefore never overwrites an edit and leaves no cache
//! entry behind that no cell uses.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};

use super::budget::{plan_calls, PASSAGES_PER_COLUMN};
use super::parse::parse_reply;
use super::passages::PaperIndex;
use super::prompt::{fill_prompt, reply_budget};
use super::store::{CacheEntry, TableStore};
use super::validate::{check, ground, Checked, RawAnswer};
use super::{collapse, Cell, CellStatus, Column, ColumnKind, ColumnSpec, Passage, TableRow};
use crate::harness::grounding::verify::ScorerSet;
use crate::research::results::TextModel;
use crate::research::{blocking, ResearchError, ResearchResult};

pub const PAPER_UNREADABLE: &str = "paper_unreadable";
pub const PAPER_NOT_INDEXED: &str = "paper_not_indexed";
pub const SEARCH_FAILED: &str = "search_failed";
pub const MODEL_FAILED: &str = "model_failed";
pub const UNPARSABLE_RESPONSE: &str = "unparsable_response";
pub const MISSING_IN_REPLY: &str = "missing_in_reply";
pub const PASSAGES_TRUNCATED: &str = "passages_truncated";

/// Longest provider or I/O detail kept in a cell's error.
const MAX_DETAIL_CHARS: usize = 300;

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

/// Whether a fill should (re)fill this cell.
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

/// One paper to fill: its row, the table's columns, the row's current cells and, for
/// `number` columns, its accepted Results as passages.
#[derive(Debug, Clone)]
pub struct PaperRequest {
    pub row: TableRow,
    pub columns: Vec<Column>,
    pub cells: Vec<Cell>,
    pub result_passages: Vec<Passage>,
}

/// What a fill did: the cells it stored, the model calls it made (a retry included),
/// the columns answered from the cache, and the model's error when a call failed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaperFill {
    pub cells: Vec<Cell>,
    pub calls: usize,
    pub cache_hits: usize,
    pub model_error: Option<String>,
}

/// Fills papers with one model, index and set of local scorers.
#[derive(Clone)]
pub struct Filler {
    pub store: Arc<TableStore>,
    pub index: Arc<dyn PaperIndex>,
    pub model: Arc<dyn TextModel>,
    pub scorers: ScorerSet,
}

/// The first chunk then the column's passages, a chunk once.
fn pool_of(first: Option<&Passage>, ranked: &[Passage]) -> Vec<Passage> {
    let mut pool: Vec<Passage> = Vec::with_capacity(ranked.len() + 1);
    for p in first.into_iter().chain(ranked.iter()) {
        if !pool.iter().any(|q| q.chunk_id == p.chunk_id) {
            pool.push(p.clone());
        }
    }
    pool
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
        let mut ranked: Vec<Vec<Passage>> = Vec::with_capacity(columns.len());
        for column in &columns {
            let query = collapse(&format!(
                "{} {}",
                column.spec.question, column.spec.instructions
            ));
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
            if column.spec.kind == ColumnKind::Number {
                passages.extend(request.result_passages.iter().cloned());
            }
            ranked.push(passages);
        }
        if first.is_none() && ranked.iter().all(Vec::is_empty) {
            self.fail(
                row,
                &columns,
                Some(&hash),
                &model_id,
                PAPER_NOT_INDEXED,
                &mut fill,
            )
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
            match cached.and_then(|json| serde_json::from_str::<RawAnswer>(&json).ok()) {
                Some(raw) => {
                    // The cached passage number belongs to another call's numbering.
                    let pool = pool_of(first.as_ref(), &ranked[i]);
                    let raw = RawAnswer {
                        passage: None,
                        ..raw
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

        // 3. At most two calls over the missed columns.
        let miss_ranked: Vec<Vec<Passage>> = misses.iter().map(|&i| ranked[i].clone()).collect();
        for plan in plan_calls(first.as_ref(), &miss_ranked) {
            let call_columns: Vec<&Column> =
                plan.columns.iter().map(|&m| columns[misses[m]]).collect();
            if let Some(error) = fill.model_error.clone() {
                let error = with_detail(MODEL_FAILED, &error);
                self.fail(
                    row,
                    &call_columns,
                    Some(&hash),
                    &model_id,
                    &error,
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
            let mut parsed = None;
            // One retry, only for a reply with no envelope at all.
            for _ in 0..2 {
                fill.calls += 1;
                match self
                    .model
                    .complete(&prompt, reply_budget(call_columns.len()))
                    .await
                {
                    Ok(reply) => {
                        if let Some(cells) = parse_reply(&reply) {
                            parsed = Some(cells);
                            break;
                        }
                    }
                    Err(e) => {
                        fill.model_error = Some(e);
                        break;
                    }
                }
            }
            let Some(parsed) = parsed else {
                let error = match fill.model_error.as_deref() {
                    Some(e) => with_detail(MODEL_FAILED, e),
                    None => UNPARSABLE_RESPONSE.to_string(),
                };
                self.fail(
                    row,
                    &call_columns,
                    Some(&hash),
                    &model_id,
                    &error,
                    &mut fill,
                )
                .await?;
                continue;
            };
            // A column repeated in the reply keeps its first entry.
            let mut by_tag: HashMap<String, RawAnswer> = HashMap::new();
            for cell in parsed {
                by_tag.entry(cell.column).or_insert(cell.raw);
            }
            for (n, column) in call_columns.iter().enumerate() {
                let Some(raw) = by_tag.remove(&tags[n]) else {
                    self.fail(
                        row,
                        &[*column],
                        Some(&hash),
                        &model_id,
                        MISSING_IN_REPLY,
                        &mut fill,
                    )
                    .await?;
                    continue;
                };
                let mut checked = check(&column.spec, &raw, &plan.passages);
                let truncated = plan.truncated.contains(&plan.columns[n]);
                if checked.status == CellStatus::NotFound && truncated {
                    checked.error = Some(PASSAGES_TRUNCATED.to_string());
                }
                // A reply that did not answer (no `answer` key) is a failure to retry, and
                // a "not found" over passages that were cut short is not final: neither is
                // cached as the model's answer.
                let cacheable = checked.status != CellStatus::Failed
                    && !(checked.status == CellStatus::NotFound && truncated);
                let cache = if cacheable {
                    serde_json::to_string(&raw)
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
        Ok(fill)
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
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::harness::grounding::verify::EntailmentScorer;
    use crate::reranking::Entailment;
    use crate::research::path_key;
    use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

    const CORRECT: &str = include_str!("fixtures/reply_correct.txt");
    const FLAWED: &str = include_str!("fixtures/reply_flawed.txt");
    const UNPARSABLE: &str = include_str!("fixtures/reply_unparsable.txt");

    fn passage(id: &str, page: u32, text: &str) -> Passage {
        Passage {
            chunk_id: id.to_string(),
            text: text.to_string(),
            page: Some(page),
            bboxes: Some(format!(
                r#"[{{"page":{page},"x0":1.0,"y0":2.0,"x1":3.0,"y1":4.0}}]"#
            )),
        }
    }

    /// The same passages for every query; records the sources it was asked about.
    struct Index {
        first: Option<Passage>,
        passages: Vec<Passage>,
        sources: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl PaperIndex for Index {
        async fn search_paper(
            &self,
            source: &str,
            _query: &str,
            k: usize,
        ) -> Result<Vec<Passage>, String> {
            self.sources
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(source.to_string());
            Ok(self.passages.iter().take(k).cloned().collect())
        }
        async fn first_passage(&self, _source: &str) -> Result<Option<Passage>, String> {
            Ok(self.first.clone())
        }
    }

    fn paper_index() -> Index {
        Index {
            first: Some(passage(
                "p0",
                1,
                "Dense Retrieval at Scale. Abstract: We pre-train a bi-encoder on LAION-400M with a contrastive loss.",
            )),
            passages: vec![
                passage(
                    "p1",
                    4,
                    "Table 2 reports Recall@10 of 0.912 on MS MARCO with 48 GB of GPU memory.",
                ),
                passage(
                    "p2",
                    6,
                    "Our code is available at the project page. The study is a benchmark evaluation.",
                ),
            ],
            sources: Mutex::new(Vec::new()),
        }
    }

    /// Replays recorded replies in order and keeps the prompts it was sent. `before_reply`
    /// runs when a call arrives, as the user would act while the model is thinking.
    struct Replies {
        replies: Mutex<VecDeque<Result<String, String>>>,
        prompts: Mutex<Vec<String>>,
        before_reply: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl Replies {
        fn new(replies: Vec<Result<&str, &str>>) -> Self {
            Self {
                replies: Mutex::new(
                    replies
                        .into_iter()
                        .map(|r| r.map(str::to_string).map_err(str::to_string))
                        .collect(),
                ),
                prompts: Mutex::new(Vec::new()),
                before_reply: None,
            }
        }
        fn prompts(&self) -> Vec<String> {
            self.prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl TextModel for Replies {
        fn model_id(&self) -> String {
            "recorded-model".to_string()
        }
        async fn complete(&self, prompt: &str, _max_tokens: usize) -> Result<String, String> {
            self.prompts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(prompt.to_string());
            if let Some(act) = &self.before_reply {
                act();
            }
            self.replies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .unwrap_or_else(|| Err("no recorded reply left".to_string()))
        }
    }

    struct Recorded(Entailment);

    impl EntailmentScorer for Recorded {
        fn entail(
            &self,
            _hypothesis: &str,
            premises: &[String],
        ) -> Result<Vec<Entailment>, String> {
            Ok(premises.iter().map(|_| self.0).collect())
        }
    }

    struct Setup {
        _dir: tempfile::TempDir,
        paper: PathBuf,
        store: Arc<TableStore>,
        workspace: String,
        table: String,
        columns: Vec<Column>,
        row: TableRow,
    }

    fn specs() -> Vec<ColumnSpec> {
        [
            (
                "Which pre-training dataset is used?",
                ColumnKind::Text,
                vec![],
            ),
            ("What Recall@10 is reported?", ColumnKind::Number, vec![]),
            ("Is the code released?", ColumnKind::YesNo, vec![]),
            (
                "What kind of study is it?",
                ColumnKind::Choice,
                vec![
                    "Benchmark".to_string(),
                    "User study".to_string(),
                    "Survey".to_string(),
                ],
            ),
        ]
        .into_iter()
        .map(|(question, kind, options)| ColumnSpec {
            question: question.to_string(),
            kind,
            options,
            instructions: String::new(),
        })
        .collect()
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("shodh.db");
        let workspace = WorkspaceStore::open(&db, None)
            .unwrap()
            .create(
                &NewWorkspace {
                    name: "Survey".to_string(),
                    ..Default::default()
                },
                WorkspaceAuthor::User,
            )
            .unwrap()
            .id;
        let store = Arc::new(TableStore::open(&db, None).unwrap());
        let table = store.create_table(&workspace, "Retrieval").unwrap().id;
        let columns = specs()
            .iter()
            .map(|spec| store.add_column(&table, spec).unwrap())
            .collect();
        let paper = dir.path().join("paper.pdf");
        std::fs::write(&paper, b"%PDF-1.4 dense retrieval at scale, version 1").unwrap();
        let row = store
            .add_rows(&table, &[paper.to_string_lossy().into_owned()])
            .unwrap()
            .remove(0);
        Setup {
            _dir: dir,
            paper,
            store,
            workspace,
            table,
            columns,
            row,
        }
    }

    fn filler(s: &Setup, index: Index, model: Arc<Replies>, scorers: ScorerSet) -> Filler {
        Filler {
            store: s.store.clone(),
            index: Arc::new(index),
            model,
            scorers,
        }
    }

    fn request(s: &Setup) -> PaperRequest {
        PaperRequest {
            row: s.row.clone(),
            columns: s.store.columns(&s.table).unwrap(),
            cells: s.store.cells(&s.table).unwrap(),
            result_passages: Vec::new(),
        }
    }

    fn status_of(s: &Setup, column: usize) -> (CellStatus, Option<String>) {
        let cell = s
            .store
            .cell(&s.row.id, &s.columns[column].id)
            .unwrap()
            .unwrap();
        (cell.status, cell.error)
    }

    #[tokio::test]
    async fn a_correct_reply_verifies_every_cell_at_its_passage_in_one_call() {
        let s = setup();
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 1);
        assert_eq!(fill.cells.len(), 4);
        assert!(
            fill.cells.iter().all(|c| c.status == CellStatus::Verified),
            "{:?}",
            fill.cells
        );
        let recall = s.store.cell(&s.row.id, &s.columns[1].id).unwrap().unwrap();
        assert_eq!(recall.chunk_id.as_deref(), Some("p1"));
        assert_eq!(recall.page, Some(4));
        assert!(recall.bboxes.is_some());
        assert_eq!(
            recall.paper_hash.as_deref(),
            Some(paper_hash(&s.paper.to_string_lossy()).unwrap().as_str())
        );
        let prompt = &model.prompts()[0];
        assert!(prompt.contains("[1] (page 1) Dense Retrieval at Scale."));
        assert!(prompt.contains("[2] (page 4) Table 2 reports"));
        assert!(prompt.contains(r#""id":"c4""#));
        for column in &s.columns {
            assert!(s
                .store
                .cached(
                    recall.paper_hash.as_deref().unwrap(),
                    &column.key,
                    "recorded-model"
                )
                .unwrap()
                .is_some());
        }
    }

    #[tokio::test]
    async fn paraphrases_wrong_numbers_outside_choices_and_nulls_are_not_verified() {
        let s = setup();
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(FLAWED)])),
            ScorerSet::default(),
        );
        f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(
            status_of(&s, 0),
            (CellStatus::Unverified, Some("quote_not_in_passages".into()))
        );
        assert_eq!(
            status_of(&s, 1),
            (CellStatus::Unverified, Some("number_not_in_quote".into()))
        );
        assert_eq!(status_of(&s, 2), (CellStatus::NotFound, None));
        assert_eq!(
            status_of(&s, 3),
            (CellStatus::Unverified, Some("not_an_option".into()))
        );
    }

    #[tokio::test]
    async fn an_unparsable_reply_is_retried_once_and_then_fails_its_cells() {
        let s = setup();
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(UNPARSABLE), Ok(UNPARSABLE)])),
            ScorerSet::default(),
        );
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 2);
        for column in 0..4 {
            assert_eq!(
                status_of(&s, column),
                (CellStatus::Failed, Some(UNPARSABLE_RESPONSE.into()))
            );
        }
        let s = setup();
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(UNPARSABLE), Ok(CORRECT)])),
            ScorerSet::default(),
        );
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 2, "the retry counts");
        assert!(fill.cells.iter().all(|c| c.status == CellStatus::Verified));
    }

    #[tokio::test]
    async fn an_unchanged_paper_and_column_cost_no_call_and_a_change_costs_only_itself() {
        let s = setup();
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        f.fill_paper(&request(&s)).await.unwrap();

        // Filled cells are not refilled.
        let again = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!((again.calls, again.cells.len()), (0, 0));

        // An edited column misses the cache alone: one call, asking only that column.
        let edited = s
            .store
            .update_column(
                &s.columns[0].id,
                &ColumnSpec {
                    question: "Which dataset is largest?".into(),
                    kind: ColumnKind::Text,
                    options: vec![],
                    instructions: String::new(),
                },
            )
            .unwrap();
        let one = r#"{"cells":[{"column":"c1","answer":"LAION-400M","quote":"We pre-train a bi-encoder on LAION-400M","passage":1}]}"#;
        let model = Arc::new(Replies::new(vec![Ok(one)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        let refill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!((refill.calls, refill.cells.len()), (1, 1));
        assert_eq!(refill.cells[0].column_id, edited.id);
        assert!(model.prompts()[0].contains("Which dataset is largest?"));
        assert!(!model.prompts()[0].contains("Is the code released?"));

        // A changed file refills its row.
        std::fs::write(&s.paper, b"%PDF-1.4 dense retrieval at scale, version 2").unwrap();
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        let changed = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(changed.calls, 1);
        assert_eq!(changed.cells.len(), 4);
    }

    #[tokio::test]
    async fn a_second_table_asking_the_same_of_the_same_paper_is_answered_from_the_cache() {
        let s = setup();
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        f.fill_paper(&request(&s)).await.unwrap();
        let hash = paper_hash(&s.paper.to_string_lossy()).unwrap();

        let table = s.store.create_table(&s.workspace, "Again").unwrap().id;
        for spec in specs() {
            s.store.add_column(&table, &spec).unwrap();
        }
        let row = s
            .store
            .add_rows(&table, &[s.paper.to_string_lossy().into_owned()])
            .unwrap()
            .remove(0);
        let second = PaperRequest {
            row: row.clone(),
            columns: s.store.columns(&table).unwrap(),
            cells: s.store.cells(&table).unwrap(),
            result_passages: Vec::new(),
        };
        let none = Arc::new(Replies::new(vec![]));
        let f2 = filler(&s, paper_index(), none.clone(), ScorerSet::default());
        let cached = f2.fill_paper(&second).await.unwrap();
        assert_eq!((cached.calls, cached.cache_hits), (0, 4));
        assert!(none.prompts().is_empty());
        assert!(
            cached
                .cells
                .iter()
                .all(|c| c.status == CellStatus::Verified),
            "{:?}",
            cached.cells
        );

        // The entries live as long as a cell uses them: after both tables' cells are
        // cleared nothing is left, and the same question costs a call again.
        for column in &s.columns {
            s.store.clear_cell(&s.row.id, &column.id).unwrap();
        }
        assert!(s
            .store
            .cached(&hash, &s.columns[0].key, "recorded-model")
            .unwrap()
            .is_some());
        for column in s.store.columns(&table).unwrap() {
            s.store.clear_cell(&row.id, &column.id).unwrap();
        }
        assert!(s
            .store
            .cached(&hash, &s.columns[0].key, "recorded-model")
            .unwrap()
            .is_none());
        let again = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(CORRECT)])),
            ScorerSet::default(),
        );
        let refilled = again.fill_paper(&request(&s)).await.unwrap();
        assert_eq!((refilled.calls, refilled.cache_hits), (1, 0));
    }

    #[tokio::test]
    async fn an_unindexed_paper_costs_no_call_and_fails_its_cells() {
        let s = setup();
        let index = Index {
            first: None,
            passages: Vec::new(),
            sources: Mutex::new(Vec::new()),
        };
        let model = Arc::new(Replies::new(vec![]));
        let f = filler(&s, index, model.clone(), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 0);
        assert!(model.prompts().is_empty());
        for column in 0..4 {
            assert_eq!(
                status_of(&s, column),
                (CellStatus::Failed, Some(PAPER_NOT_INDEXED.into()))
            );
        }
    }

    #[tokio::test]
    async fn a_column_missing_from_the_reply_fails_and_a_repeat_is_ignored() {
        let s = setup();
        let reply = r#"{"cells":[
            {"column":"c1","answer":"LAION-400M","quote":"We pre-train a bi-encoder on LAION-400M","passage":1},
            {"column":"c1","answer":"ImageNet","quote":"ImageNet is the dataset","passage":1}]}"#;
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(reply)])),
            ScorerSet::default(),
        );
        f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(status_of(&s, 0), (CellStatus::Verified, None));
        for column in 1..4 {
            assert_eq!(
                status_of(&s, column),
                (CellStatus::Failed, Some(MISSING_IN_REPLY.into()))
            );
        }
    }

    #[tokio::test]
    async fn only_an_explicit_null_is_not_found_and_a_missing_answer_fails_uncached() {
        let s = setup();
        let reply = r#"{"cells":[
            {"column":"c1","answer":null,"quote":"","passage":null},
            {"column":"c2","quote":"Recall@10 of 0.912 on MS MARCO","passage":2},
            {"column":"c3","answer":"yes","quote":"Our code is available at the project page","passage":3},
            {"column":"c4","answer":"Survey","quote":"The study is a benchmark evaluation","passage":3}]}"#;
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(reply)])),
            ScorerSet::default(),
        );
        f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(status_of(&s, 0), (CellStatus::NotFound, None));
        assert_eq!(
            status_of(&s, 1),
            (CellStatus::Failed, Some("missing_answer".into()))
        );
        assert_eq!(status_of(&s, 2), (CellStatus::Verified, None));
        let hash = paper_hash(&s.paper.to_string_lossy()).unwrap();
        let cached = |column: usize| {
            s.store
                .cached(&hash, &s.columns[column].key, "recorded-model")
                .unwrap()
        };
        assert!(cached(0).is_some(), "a found-nothing answer is cached");
        assert!(cached(1).is_none(), "a failed answer is not");
        assert!(cached(2).is_some());
    }

    #[tokio::test]
    async fn a_not_found_cell_of_a_truncated_column_says_so_and_is_not_cached() {
        let s = setup();
        let big = "word ".repeat(1_200);
        let index = Index {
            first: None,
            passages: (1..=4)
                .map(|i| passage(&format!("big{i}"), i, &big))
                .collect(),
            sources: Mutex::new(Vec::new()),
        };
        let nothing = r#"{"cells":[{"column":"c1","answer":null},{"column":"c2","answer":null}]}"#;
        let model = Arc::new(Replies::new(vec![Ok(nothing), Ok(nothing)]));
        let f = filler(&s, index, model, ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 2, "the columns are split over two calls");
        for column in 0..4 {
            assert_eq!(
                status_of(&s, column),
                (CellStatus::NotFound, Some(PASSAGES_TRUNCATED.into()))
            );
        }
        let hash = paper_hash(&s.paper.to_string_lossy()).unwrap();
        assert!(s
            .store
            .cached(&hash, &s.columns[0].key, "recorded-model")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn a_provider_error_fails_the_cells_and_is_reported() {
        let s = setup();
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Err(
                "HTTP 429 Too Many Requests: daily limit",
            )])),
            ScorerSet::default(),
        );
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 1);
        assert_eq!(
            fill.model_error.as_deref(),
            Some("HTTP 429 Too Many Requests: daily limit")
        );
        let (status, error) = status_of(&s, 0);
        assert_eq!(status, CellStatus::Failed);
        assert_eq!(
            error.as_deref(),
            Some("model_failed: HTTP 429 Too Many Requests: daily limit")
        );
    }

    #[tokio::test]
    async fn edited_cells_are_never_asked_about() {
        let s = setup();
        s.store
            .edit_cell(&s.row.id, &s.columns[2].id, "no")
            .unwrap();
        let model = Arc::new(Replies::new(vec![Ok(r#"{"cells":[]}"#)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        f.fill_paper(&request(&s)).await.unwrap();
        assert!(!model.prompts()[0].contains("Is the code released?"));
        assert_eq!(status_of(&s, 2).0, CellStatus::Edited);
    }

    #[tokio::test]
    async fn an_edit_or_a_column_change_during_the_call_is_not_overwritten() {
        let s = setup();
        let store = s.store.clone();
        let (row_id, code, dataset) = (
            s.row.id.clone(),
            s.columns[2].id.clone(),
            s.columns[0].id.clone(),
        );
        let mut replies = Replies::new(vec![Ok(CORRECT)]);
        replies.before_reply = Some(Box::new(move || {
            store.edit_cell(&row_id, &code, "no").unwrap();
            store
                .update_column(
                    &dataset,
                    &ColumnSpec {
                        question: "Which dataset is largest?".into(),
                        kind: ColumnKind::Text,
                        options: vec![],
                        instructions: String::new(),
                    },
                )
                .unwrap();
        }));
        let old_key = s.columns[0].key.clone();
        let f = filler(&s, paper_index(), Arc::new(replies), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();

        let stored: Vec<&str> = fill.cells.iter().map(|c| c.column_id.as_str()).collect();
        assert_eq!(
            stored,
            vec![s.columns[1].id.as_str(), s.columns[3].id.as_str()]
        );
        assert!(
            s.store.cell(&s.row.id, &s.columns[0].id).unwrap().is_none(),
            "a result for the older column revision is not stored"
        );
        let edited = s.store.cell(&s.row.id, &s.columns[2].id).unwrap().unwrap();
        assert_eq!(edited.status, CellStatus::Edited);
        assert_eq!(
            edited.value,
            Some(crate::research::tables::CellValue::YesNo { yes: false })
        );
        let hash = paper_hash(&s.paper.to_string_lossy()).unwrap();
        for key in [&old_key, &s.columns[2].key] {
            assert!(
                s.store
                    .cached(&hash, key, "recorded-model")
                    .unwrap()
                    .is_none(),
                "no cache entry is left that no cell uses"
            );
        }
    }

    #[tokio::test]
    async fn a_contradicting_quote_downgrades_verified_cells() {
        let s = setup();
        let scorers = ScorerSet {
            relevance: None,
            entailment: Some(Arc::new(Recorded(Entailment {
                contradiction: 0.9,
                entailment: 0.05,
                neutral: 0.05,
            }))),
        };
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(CORRECT)])),
            scorers,
        );
        f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(
            status_of(&s, 2),
            (
                CellStatus::Unverified,
                Some("quote_does_not_support".into())
            )
        );
    }

    #[tokio::test]
    async fn number_columns_see_the_papers_results_and_the_search_gets_the_row_path() {
        let s = setup();
        let index = Arc::new(paper_index());
        let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
        let f = Filler {
            store: s.store.clone(),
            index: index.clone() as Arc<dyn PaperIndex>,
            model: model.clone(),
            scorers: ScorerSet::default(),
        };
        let mut req = request(&s);
        req.result_passages = vec![passage(
            "result:r1",
            7,
            "BM25 on MS MARCO: Recall@10 = 0.355",
        )];
        f.fill_paper(&req).await.unwrap();
        assert!(model.prompts()[0].contains("(page 7) BM25 on MS MARCO: Recall@10 = 0.355"));
        assert_eq!(s.row.document_id, path_key(&s.paper.to_string_lossy()));
        let sources = index
            .sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(sources.len(), 4, "one search per column");
        assert!(sources.iter().all(|source| source == &s.row.document_id));
    }

    #[tokio::test]
    async fn an_unreadable_paper_fails_empty_cells_and_leaves_results_standing() {
        let s = setup();
        let f = filler(
            &s,
            paper_index(),
            Arc::new(Replies::new(vec![Ok(CORRECT)])),
            ScorerSet::default(),
        );
        f.fill_paper(&request(&s)).await.unwrap();
        s.store.clear_cell(&s.row.id, &s.columns[3].id).unwrap();
        std::fs::remove_file(&s.paper).unwrap();
        let model = Arc::new(Replies::new(vec![]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 0);
        assert_eq!(fill.cells.len(), 1);
        assert_eq!(status_of(&s, 0).0, CellStatus::Verified);
        let (status, error) = status_of(&s, 3);
        assert_eq!(status, CellStatus::Failed);
        assert!(error.unwrap().starts_with("paper_unreadable: "));
    }

    #[test]
    fn staleness_follows_revision_and_hash_and_never_touches_edits() {
        let s = setup();
        let column = &s.columns[0];
        let cell = |status, revision, hash: &str| Cell {
            table_id: s.table.clone(),
            row_id: s.row.id.clone(),
            column_id: column.id.clone(),
            status,
            value: None,
            quote: None,
            chunk_id: None,
            page: None,
            bboxes: None,
            model: None,
            column_revision: Some(revision),
            paper_hash: Some(hash.to_string()),
            filled_at: None,
            edited_at: None,
            error: None,
        };
        assert!(needs_fill(None, column, "h"));
        assert!(!needs_fill(
            Some(&cell(CellStatus::Verified, 1, "h")),
            column,
            "h"
        ));
        assert!(needs_fill(
            Some(&cell(CellStatus::Verified, 1, "other")),
            column,
            "h"
        ));
        assert!(needs_fill(
            Some(&cell(CellStatus::NotFound, 0, "h")),
            column,
            "h"
        ));
        assert!(needs_fill(
            Some(&cell(CellStatus::Failed, 1, "h")),
            column,
            "h"
        ));
        assert!(!needs_fill(
            Some(&cell(CellStatus::Edited, 0, "other")),
            column,
            "h"
        ));
    }
}
