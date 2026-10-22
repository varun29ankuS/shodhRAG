use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use shodh_ontology::ExtractorKind;

use super::*;
use crate::harness::grounding::verify::EntailmentScorer;
use crate::reranking::Entailment;
use crate::research::path_key;
use crate::research::results::Region;
use crate::research::tables::validate::NON_ANSWERS;
use crate::research::tables::CellValue;
use crate::statements::Scope;
use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

const CORRECT: &str = include_str!("fixtures/reply_correct.txt");
const FLAWED: &str = include_str!("fixtures/reply_flawed.txt");
const UNPARSABLE: &str = include_str!("fixtures/reply_unparsable.txt");
const CURLY: &str = include_str!("fixtures/reply_curly.txt");
const BARE_ARRAY: &str = include_str!("fixtures/reply_bare_array.txt");
const WRONG_KEYS: &str = include_str!("fixtures/reply_wrong_keys.txt");
const FENCED: &str = include_str!("fixtures/reply_fenced.txt");
const TRUNCATED: &str = include_str!("fixtures/reply_truncated.txt");
const NON_ANSWER: &str = include_str!("fixtures/reply_non_answer.txt");

/// A well-formed reply that finds nothing for the first two columns of a call.
const NOTHING_2: &str =
    r#"{"cells":[{"column":"c1","answer":null},{"column":"c2","answer":null}]}"#;
/// What a call is asked again for after a cut-off reply left out the last two columns.
const RETRY_LAST_TWO: &str = r#"{"cells":[{"column":"c1","answer":"yes","quote":"Our code is available at the project page","passage":3},{"column":"c2","answer":"benchmark","quote":"The study is a benchmark evaluation","passage":3}]}"#;
const CUT_AFTER_ONE: &str = r#"{"cells":[{"column":"c1","answer":null},{"column":"c2","answ"#;
const QUOTA_ERROR: &str = "HTTP 429 Too Many Requests: daily limit";
const TIMEOUT_ERROR: &str = "the provider did not answer in time";

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

/// The same passages for every query unless a keyword of the query selects others;
/// records every search.
struct Index {
    first: Option<Passage>,
    passages: Vec<Passage>,
    by_query: Vec<(&'static str, Vec<Passage>)>,
    searches: Mutex<Vec<(String, String)>>,
}

impl Index {
    fn new(first: Option<Passage>, passages: Vec<Passage>) -> Self {
        Self {
            first,
            passages,
            by_query: Vec::new(),
            searches: Mutex::new(Vec::new()),
        }
    }

    fn with_query(mut self, keyword: &'static str, passages: Vec<Passage>) -> Self {
        self.by_query.push((keyword, passages));
        self
    }

    fn searches(&self) -> Vec<(String, String)> {
        self.searches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait::async_trait]
impl PaperIndex for Index {
    async fn search_paper(
        &self,
        source: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<Passage>, String> {
        self.searches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((source.to_string(), query.to_string()));
        let lower = query.to_lowercase();
        let chosen = self
            .by_query
            .iter()
            .find(|(keyword, _)| lower.contains(keyword))
            .map_or(&self.passages, |(_, passages)| passages);
        Ok(chosen.iter().take(k).cloned().collect())
    }
    async fn first_passage(&self, _source: &str) -> Result<Option<Passage>, String> {
        Ok(self.first.clone())
    }
}

fn paper_index() -> Index {
    Index::new(
        Some(passage(
            "p0",
            1,
            "Dense Retrieval at Scale. Abstract: We pre-train a bi-encoder on LAION-400M with a contrastive loss.",
        )),
        vec![
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
    )
}

/// Four passages of about 2,000 tokens each: more than one call holds for four columns.
fn big_index() -> Index {
    let big = "word ".repeat(1_200);
    Index::new(
        None,
        (1..=4)
            .map(|i| passage(&format!("big{i}"), i, &big))
            .collect(),
    )
}

/// The characters one output token is worth in the double below (the same estimate the
/// budget uses).
const CHARS_PER_TOKEN: usize = 3;

/// Replays recorded replies in order, keeps the prompts and output limits it was sent,
/// and cuts a reply at the limit the way a provider does. `before_reply` runs when a call
/// arrives, as the user would act while the model is thinking.
struct Replies {
    replies: Mutex<VecDeque<Result<String, String>>>,
    prompts: Mutex<Vec<String>>,
    limits: Mutex<Vec<usize>>,
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
            limits: Mutex::new(Vec::new()),
            before_reply: None,
        }
    }
    fn prompts(&self) -> Vec<String> {
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn limits(&self) -> Vec<usize> {
        self.limits
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
    async fn complete(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(prompt.to_string());
        self.limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(max_tokens);
        if let Some(act) = &self.before_reply {
            act();
        }
        let reply = self
            .replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or_else(|| Err("no recorded reply left".to_string()))?;
        Ok(reply.chars().take(max_tokens * CHARS_PER_TOKEN).collect())
    }
}

struct Recorded(Entailment);

impl EntailmentScorer for Recorded {
    fn entail(&self, _hypothesis: &str, premises: &[String]) -> Result<Vec<Entailment>, String> {
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

fn setup_with(columns: &[ColumnSpec]) -> Setup {
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
    let columns = columns
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

fn setup() -> Setup {
    setup_with(&specs())
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
        results: Vec::new(),
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

fn hash_of(s: &Setup) -> String {
    paper_hash(&s.paper.to_string_lossy()).unwrap()
}

fn clear_all(s: &Setup) {
    for column in &s.columns {
        s.store.clear_cell(&s.row.id, &column.id).unwrap();
    }
}

fn result(
    id: &str,
    method: &str,
    dataset: &str,
    metric: &str,
    status: ResultStatus,
) -> ResultRecord {
    ResultRecord {
        id: id.to_string(),
        method: method.to_string(),
        dataset: dataset.to_string(),
        metric: metric.to_string(),
        method_id: "method:x".to_string(),
        dataset_id: "dataset:x".to_string(),
        metric_id: "metric:x".to_string(),
        value: "0.355".to_string(),
        value_text: "0.355".to_string(),
        unit: None,
        spread: None,
        setting: Some("k1 = 0.9".to_string()),
        file_path: "/p/a.pdf".to_string(),
        file_name: "a.pdf".to_string(),
        page: 7,
        region: Some(Region {
            page: 7,
            x0: 10.0,
            y0: 20.0,
            x1: 30.0,
            y1: 40.0,
        }),
        table_caption: Some("Table 2: Retrieval results.".to_string()),
        extractor: ExtractorKind::User,
        confidence: 1.0,
        status,
        scope: Scope::Global,
        extracted_at: Utc::now(),
    }
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
    assert_eq!(recall.paper_hash.as_deref(), Some(hash_of(&s).as_str()));
    let prompt = &model.prompts()[0];
    assert!(prompt.contains("[1] (page 1) Dense Retrieval at Scale."));
    assert!(prompt.contains("[2] (page 4) Table 2 reports"));
    assert!(prompt.contains(r#""id":"c4""#));
    assert_eq!(model.limits(), vec![reply_budget(4)]);
    for column in &s.columns {
        assert!(s
            .store
            .cached(&hash_of(&s), &column.key, "recorded-model")
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
async fn an_unparsable_reply_is_asked_again_with_a_format_instruction_and_then_fails() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![Ok(UNPARSABLE), Ok(UNPARSABLE)]));
    let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 2);
    for column in 0..4 {
        assert_eq!(
            status_of(&s, column),
            (CellStatus::Failed, Some(UNPARSABLE_RESPONSE.into()))
        );
    }
    let prompts = model.prompts();
    assert_ne!(prompts[0], prompts[1], "the retry is not the same prompt");
    assert!(prompts[1].starts_with(&prompts[0]));
    assert!(prompts[1].contains("could not be read"));

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
    let hash = hash_of(&s);

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
        results: Vec::new(),
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
    clear_all(&s);
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
async fn a_replay_is_checked_against_the_passage_the_quote_was_found_in() {
    // Table 1 asks about the dataset and the code. The search for the code column is the
    // one that surfaces the sentence that answers the dataset column.
    let dataset = specs().remove(0);
    let code = specs().remove(2);
    let s = setup_with(&[dataset.clone(), code]);
    let answer = passage(
        "p-answer",
        6,
        "Pre-training uses the LAION-400M image-text corpus for all experiments.",
    );
    let index = |extra: Passage| {
        Index::new(
            Some(passage("p0", 1, "Dense Retrieval at Scale. An abstract.")),
            vec![],
        )
        .with_query(
            "dataset",
            vec![passage(
                "p-other",
                2,
                "The appendix lists the hardware that the experiments ran on.",
            )],
        )
        .with_query(
            "code",
            vec![
                passage("p-code", 3, "Our code is available at the project page."),
                extra,
            ],
        )
    };
    let reply = r#"{"cells":[
        {"column":"c1","answer":"LAION-400M","quote":"Pre-training uses the LAION-400M image-text corpus","passage":4},
        {"column":"c2","answer":"yes","quote":"Our code is available at the project page","passage":3}]}"#;
    let f = filler(
        &s,
        index(answer.clone()),
        Arc::new(Replies::new(vec![Ok(reply)])),
        ScorerSet::default(),
    );
    let first = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(first.calls, 1);
    assert_eq!(status_of(&s, 0), (CellStatus::Verified, None));

    // Table 2 asks the dataset question alone. Its own search never retrieves the answer
    // sentence; the replay must still find the quote where the original call found it.
    let table = s
        .store
        .create_table(&s.workspace, "Datasets only")
        .unwrap()
        .id;
    s.store.add_column(&table, &dataset).unwrap();
    let row = s
        .store
        .add_rows(&table, &[s.paper.to_string_lossy().into_owned()])
        .unwrap()
        .remove(0);
    let second = PaperRequest {
        row: row.clone(),
        columns: s.store.columns(&table).unwrap(),
        cells: s.store.cells(&table).unwrap(),
        results: Vec::new(),
    };
    let none = Arc::new(Replies::new(vec![]));
    let f2 = filler(&s, index(answer), none.clone(), ScorerSet::default());
    let replay = f2.fill_paper(&second).await.unwrap();
    assert_eq!((replay.calls, replay.cache_hits), (0, 1));
    let cell = s
        .store
        .cell(&row.id, &s.store.columns(&table).unwrap()[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(cell.status, CellStatus::Verified, "{cell:?}");
    assert_eq!(cell.chunk_id.as_deref(), Some("p-answer"));
    assert_eq!(cell.page, Some(6));
}

#[tokio::test]
async fn an_unindexed_paper_costs_no_call_and_fails_its_cells() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![]));
    let f = filler(
        &s,
        Index::new(None, Vec::new()),
        model.clone(),
        ScorerSet::default(),
    );
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
async fn a_paper_the_search_cannot_see_costs_no_call_and_says_search_failed() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![]));
    let first = passage("p0", 1, "Dense Retrieval at Scale. An abstract.");
    let f = filler(
        &s,
        Index::new(Some(first), Vec::new()),
        model.clone(),
        ScorerSet::default(),
    );
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 0);
    assert!(model.prompts().is_empty());
    for column in 0..4 {
        let (status, error) = status_of(&s, column);
        assert_eq!(status, CellStatus::Failed);
        assert!(error.unwrap().starts_with("search_failed: "));
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
    let hash = hash_of(&s);
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
async fn a_non_answer_in_words_is_not_found_and_never_verified() {
    let s = setup();
    let f = filler(
        &s,
        paper_index(),
        Arc::new(Replies::new(vec![Ok(NON_ANSWER)])),
        ScorerSet::default(),
    );
    f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(status_of(&s, 0), (CellStatus::NotFound, None));
    assert_eq!(status_of(&s, 1), (CellStatus::Verified, None));
    assert_eq!(status_of(&s, 2), (CellStatus::Verified, None));
    assert_eq!(status_of(&s, 3), (CellStatus::NotFound, None));
    assert!(NON_ANSWERS.contains(&"not specified") && NON_ANSWERS.contains(&"unknown"));
}

#[tokio::test]
async fn a_not_found_over_cut_passages_is_asked_once_more_and_then_left_alone() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![Ok(NOTHING_2), Ok(NOTHING_2)]));
    let f = filler(&s, big_index(), model, ScorerSet::default());
    let first = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(first.calls, 2, "the columns are split over two calls");
    for column in 0..4 {
        assert_eq!(
            status_of(&s, column),
            (CellStatus::NotFound, Some(NOT_FOUND_PARTIAL.into()))
        );
    }
    assert!(s
        .store
        .cached(&hash_of(&s), &s.columns[0].key, "recorded-model")
        .unwrap()
        .is_none());

    // The next run asks once more.
    let model = Arc::new(Replies::new(vec![Ok(NOTHING_2), Ok(NOTHING_2)]));
    let f = filler(&s, big_index(), model.clone(), ScorerSet::default());
    let second = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(second.calls, 2);
    for column in 0..4 {
        assert_eq!(
            status_of(&s, column),
            (CellStatus::NotFound, Some(NOT_FOUND_PARTIAL_RETRIED.into()))
        );
    }

    // And then never again for this paper and these columns: no quota is spent in a loop.
    let model = Arc::new(Replies::new(vec![]));
    let f = filler(&s, big_index(), model.clone(), ScorerSet::default());
    let third = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!((third.calls, third.cells.len()), (0, 0));
    assert!(model.prompts().is_empty());

    // A changed file starts over.
    std::fs::write(&s.paper, b"%PDF-1.4 dense retrieval at scale, version 2").unwrap();
    let model = Arc::new(Replies::new(vec![Ok(NOTHING_2), Ok(NOTHING_2)]));
    let f = filler(&s, big_index(), model, ScorerSet::default());
    assert_eq!(f.fill_paper(&request(&s)).await.unwrap().calls, 2);
    assert_eq!(
        status_of(&s, 0),
        (CellStatus::NotFound, Some(NOT_FOUND_PARTIAL.into()))
    );
}

#[tokio::test]
async fn the_second_try_of_a_cut_not_found_can_find_the_answer() {
    let s = setup();
    let f = filler(
        &s,
        big_index(),
        Arc::new(Replies::new(vec![Ok(NOTHING_2), Ok(NOTHING_2)])),
        ScorerSet::default(),
    );
    f.fill_paper(&request(&s)).await.unwrap();
    let model = Arc::new(Replies::new(vec![Ok(CORRECT)]));
    let f = filler(&s, paper_index(), model, ScorerSet::default());
    let second = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(second.calls, 1);
    assert!(second
        .cells
        .iter()
        .all(|c| c.status == CellStatus::Verified));
}

#[test]
fn a_cut_not_found_is_the_one_settled_cell_that_is_filled_again() {
    let s = setup();
    let column = &s.columns[0];
    let cell = |status, error: Option<&str>| Cell {
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
        column_revision: Some(column.revision),
        paper_hash: Some("h".to_string()),
        filled_at: None,
        edited_at: None,
        error: error.map(str::to_string),
    };
    assert!(needs_fill(
        Some(&cell(CellStatus::NotFound, Some(NOT_FOUND_PARTIAL))),
        column,
        "h"
    ));
    for settled in [
        cell(CellStatus::NotFound, Some(NOT_FOUND_PARTIAL_RETRIED)),
        cell(CellStatus::NotFound, None),
        cell(CellStatus::Verified, None),
    ] {
        assert!(!needs_fill(Some(&settled), column, "h"));
    }
}

#[tokio::test]
async fn a_cut_off_reply_keeps_its_cells_and_only_the_missing_columns_are_asked_again() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![Ok(TRUNCATED), Ok(RETRY_LAST_TWO)]));
    let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 2);
    assert!(
        fill.cells.iter().all(|c| c.status == CellStatus::Verified),
        "{:?}",
        fill.cells
    );
    assert_eq!(fill.cells.len(), 4);
    let prompts = model.prompts();
    assert!(prompts[1].contains("Is the code released?"));
    assert!(prompts[1].contains("What kind of study is it?"));
    assert!(!prompts[1].contains("Which pre-training dataset is used?"));
    assert!(prompts[1].contains("cut off"));
    assert_eq!(model.limits(), vec![reply_budget(4), reply_budget(2)]);
}

#[tokio::test]
async fn a_reply_longer_than_its_token_limit_is_cut_by_the_double_and_salvaged() {
    let s = setup();
    let note = "x".repeat(560);
    let cell = |column: &str, answer: &str, quote: &str, passage: u32| {
        format!(
            r#"{{"column":"{column}","answer":"{answer}","quote":"{quote}","passage":{passage},"note":"{note}"}}"#
        )
    };
    let long = format!(
        r#"{{"cells":[{},{},{},{}]}}"#,
        cell(
            "c1",
            "LAION-400M",
            "We pre-train a bi-encoder on LAION-400M",
            1
        ),
        cell("c2", "0.912", "Recall@10 of 0.912 on MS MARCO", 2),
        cell("c3", "yes", "Our code is available at the project page", 3),
        cell("c4", "benchmark", "The study is a benchmark evaluation", 3),
    );
    assert!(long.len() > reply_budget(4) * CHARS_PER_TOKEN);
    let last_one = r#"{"cells":[{"column":"c1","answer":"benchmark","quote":"The study is a benchmark evaluation","passage":3}]}"#;
    let model = Arc::new(Replies::new(vec![Ok(long.as_str()), Ok(last_one)]));
    let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 2);
    assert_eq!(fill.cells.len(), 4);
    assert!(fill.cells.iter().all(|c| c.status == CellStatus::Verified));
    assert_eq!(model.limits(), vec![reply_budget(4), reply_budget(1)]);
    assert!(model.prompts()[1].contains("What kind of study is it?"));
    assert!(!model.prompts()[1].contains("Is the code released?"));
}

#[tokio::test]
async fn replies_in_the_shapes_free_models_write_are_read() {
    for (name, reply) in [
        ("bare array", BARE_ARRAY),
        ("other keys and a trailing comma", WRONG_KEYS),
        ("preamble, fence and postscript", FENCED),
    ] {
        let s = setup();
        let model = Arc::new(Replies::new(vec![Ok(reply)]));
        let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 1, "{name}");
        assert_eq!(fill.cells.len(), 4, "{name}");
        assert!(
            fill.cells.iter().all(|c| c.status == CellStatus::Verified),
            "{name}: {:?}",
            fill.cells
        );
    }
}

#[tokio::test]
async fn a_quote_in_curly_quotes_and_dashes_matches_a_passage_with_a_ligature() {
    let s = setup_with(&specs()[..1]);
    let index = Index::new(
        Some(passage(
            "p0",
            1,
            "Dense Retrieval at Scale. We don't use \u{fb01}ne-tuning - we only train a linear probe on ImageNet.",
        )),
        vec![passage("p1", 2, "The appendix lists the hardware used.")],
    );
    let model = Arc::new(Replies::new(vec![Ok(CURLY)]));
    let f = filler(&s, index, model, ScorerSet::default());
    f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(status_of(&s, 0), (CellStatus::Verified, None));
    let cell = s.store.cell(&s.row.id, &s.columns[0].id).unwrap().unwrap();
    assert_eq!(cell.chunk_id.as_deref(), Some("p0"));
    // The quote is the model's text as written, curly characters and all.
    assert!(cell.quote.unwrap().contains('\u{2019}'));
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
    assert_eq!(edited.value, Some(CellValue::YesNo { yes: false }));
    let hash = hash_of(&s);
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
async fn results_only_steer_the_search_of_number_columns_and_are_never_quotable() {
    let s = setup();
    let index = Arc::new(paper_index());
    // The model quotes the Result line it was never shown: it is not text of the paper.
    let reply = r#"{"cells":[{"column":"c2","answer":"0.355","quote":"BM25 on MS MARCO: Recall@10 = 0.355","passage":1}]}"#;
    let model = Arc::new(Replies::new(vec![Ok(reply)]));
    let f = Filler {
        store: s.store.clone(),
        index: index.clone() as Arc<dyn PaperIndex>,
        model: model.clone(),
        scorers: ScorerSet::default(),
    };
    let mut req = request(&s);
    req.results = vec![
        result(
            "r1",
            "BM25",
            "MS MARCO",
            "Recall@10",
            ResultStatus::Accepted,
        ),
        result(
            "r2",
            "DPR",
            "Natural Questions",
            "Exact match",
            ResultStatus::Accepted,
        ),
        result(
            "r3",
            "SPLADE",
            "MS MARCO",
            "Recall@10",
            ResultStatus::Review,
        ),
    ];
    f.fill_paper(&req).await.unwrap();

    let searches = index.searches();
    assert_eq!(searches.len(), 4, "one search per column");
    assert!(searches
        .iter()
        .all(|(source, _)| source == &s.row.document_id));
    assert_eq!(s.row.document_id, path_key(&s.paper.to_string_lossy()));
    let number_query = &searches[1].1;
    assert!(number_query.contains("What Recall@10 is reported?"));
    assert!(
        number_query.ends_with("BM25 MS MARCO Recall@10"),
        "{number_query}"
    );
    for (i, (_, query)) in searches.iter().enumerate() {
        if i != 1 {
            assert!(!query.contains("BM25"), "{query}");
        }
    }
    // Neither a Result nor a line made of its cells reaches the prompt.
    assert!(!model.prompts()[0].contains("BM25 on MS MARCO"));
    assert_eq!(
        status_of(&s, 1),
        (CellStatus::Unverified, Some("quote_not_in_passages".into()))
    );
}

#[tokio::test]
async fn results_do_not_change_the_search_of_other_columns() {
    let s = setup_with(&specs()[2..3]);
    let index = Arc::new(paper_index());
    let model = Arc::new(Replies::new(vec![Ok(
        r#"{"cells":[{"column":"c1","answer":"yes","quote":"Our code is available at the project page","passage":3}]}"#,
    )]));
    let f = Filler {
        store: s.store.clone(),
        index: index.clone() as Arc<dyn PaperIndex>,
        model,
        scorers: ScorerSet::default(),
    };
    let mut req = request(&s);
    req.results = vec![result(
        "r1",
        "BM25",
        "MS MARCO",
        "Recall@10",
        ResultStatus::Accepted,
    )];
    f.fill_paper(&req).await.unwrap();
    assert!(!index.searches()[0].1.contains("BM25"));
    assert_eq!(status_of(&s, 0), (CellStatus::Verified, None));
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
fn model_errors_are_classified_and_their_detail_is_redacted() {
    let key = "sk-or-v1-0123456789abcdef0123456789";
    let auth = format!("HTTP 401 Unauthorized: Incorrect API key provided: {key}");
    for (message, kind) in [
        (QUOTA_ERROR, ModelFailureKind::Quota),
        (
            "429 {\"error\":{\"message\":\"Rate limit exceeded: free-models-per-day\"}}",
            ModelFailureKind::Quota,
        ),
        (
            "402 Payment Required: insufficient credits",
            ModelFailureKind::Quota,
        ),
        (TIMEOUT_ERROR, ModelFailureKind::Transient),
        ("HTTP 503 Service Unavailable", ModelFailureKind::Transient),
        (
            "error sending request: connection reset",
            ModelFailureKind::Transient,
        ),
        (auth.as_str(), ModelFailureKind::Auth),
        (
            "HTTP 400 Bad Request: invalid parameter",
            ModelFailureKind::Other,
        ),
        ("something else entirely", ModelFailureKind::Other),
    ] {
        let failure = ModelFailure::classify(message);
        assert_eq!(failure.kind, kind, "{message}");
        assert!(!failure.detail.contains(key), "{}", failure.detail);
        assert!(failure.detail.chars().count() <= 300);
    }
    let auth = ModelFailure::classify(&format!("HTTP 401 Unauthorized: key {key}"));
    assert!(auth.detail.contains("[REDACTED]"), "{}", auth.detail);
    let codes: Vec<&str> = [
        ModelFailureKind::Quota,
        ModelFailureKind::Transient,
        ModelFailureKind::Auth,
        ModelFailureKind::Other,
    ]
    .iter()
    .map(|&kind| {
        ModelFailure {
            kind,
            retry_after_secs: None,
            detail: String::new(),
        }
        .code()
    })
    .collect();
    assert_eq!(
        codes,
        vec!["model_quota", "model_timeout", "model_auth", "model_failed"]
    );
}

#[test]
fn every_shape_of_api_key_is_masked_in_a_failure_detail() {
    // Built from pieces so that no literal in the source looks like a real credential.
    let keys: Vec<String> = [
        ("sk-or-v1-", "0123456789abcdef0123456789"),
        ("AIza", "SyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q"),
        ("gsk_", "abcdefghijklmnopqrstuvwxyz0123456789ABCD"),
        ("hf_", "abcdefghijklmnopqrstuvwxyzABCDEFGH"),
        ("xai-", "abcdefghijklmnopqrstuvwxyz0123456789"),
        ("pplx-", "abcdefghijklmnopqrstuvwxyz0123456789"),
        ("", "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"),
        ("", "Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0LmNbVcXzQw"),
    ]
    .iter()
    .map(|(prefix, rest)| format!("{prefix}{rest}"))
    .collect();
    for key in &keys {
        let message = format!("HTTP 401 Unauthorized: the key {key} is not valid (see docs)");
        let failure = ModelFailure::classify(&message);
        assert!(
            !failure.detail.contains(key.as_str()),
            "{key}: {}",
            failure.detail
        );
        assert!(failure.detail.contains("[REDACTED]"), "{key}");
        assert!(failure.detail.contains("HTTP 401 Unauthorized"), "{key}");
    }
    let bearer = ModelFailure::classify("401: Authorization: Bearer abcdef123456");
    assert!(!bearer.detail.contains("abcdef123456"));
    // Ordinary words and numbers stay readable.
    let plain = ModelFailure::classify(
        "HTTP 429 Too Many Requests: free-models-per-day limit, reset 1760000000000",
    );
    assert_eq!(
        plain.detail,
        "HTTP 429 Too Many Requests: free-models-per-day limit, reset 1760000000000"
    );
}

#[tokio::test]
async fn a_quota_error_leaves_the_cells_untouched_and_is_returned() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![Err(QUOTA_ERROR)]));
    let f = filler(&s, paper_index(), model, ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 1);
    assert!(fill.cells.is_empty());
    let failure = fill.model_error.unwrap();
    assert_eq!(failure.kind, ModelFailureKind::Quota);
    assert!(failure.detail.contains("daily limit"));
    assert!(
        s.store.cells(&s.table).unwrap().is_empty(),
        "nothing written"
    );

    // Cells that held a stale answer keep it.
    let g = filler(
        &s,
        paper_index(),
        Arc::new(Replies::new(vec![Ok(CORRECT)])),
        ScorerSet::default(),
    );
    g.fill_paper(&request(&s)).await.unwrap();
    std::fs::write(&s.paper, b"%PDF-1.4 dense retrieval at scale, version 2").unwrap();
    let before = s.store.cells(&s.table).unwrap();
    let h = filler(
        &s,
        paper_index(),
        Arc::new(Replies::new(vec![Err(QUOTA_ERROR)])),
        ScorerSet::default(),
    );
    let paused = h.fill_paper(&request(&s)).await.unwrap();
    assert!(paused.cells.is_empty());
    assert_eq!(s.store.cells(&s.table).unwrap(), before);
}

#[tokio::test]
async fn a_quota_error_after_a_first_call_keeps_its_cells_and_makes_no_second_call() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![Ok(NOTHING_2), Err(QUOTA_ERROR)]));
    let f = filler(&s, big_index(), model.clone(), ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 2);
    assert_eq!(fill.cells.len(), 2);
    assert_eq!(status_of(&s, 0).0, CellStatus::NotFound);
    for column in 2..4 {
        assert!(s
            .store
            .cell(&s.row.id, &s.columns[column].id)
            .unwrap()
            .is_none());
    }
    assert_eq!(fill.model_error.unwrap().kind, ModelFailureKind::Quota);

    // The first call's error also stops the second call of the plan.
    let s = setup();
    let model = Arc::new(Replies::new(vec![Err(QUOTA_ERROR)]));
    let f = filler(&s, big_index(), model.clone(), ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 1);
    assert!(s.store.cells(&s.table).unwrap().is_empty());
}

#[tokio::test]
async fn other_model_errors_fail_the_cells_with_a_code_and_never_the_provider_text() {
    let key = "sk-or-v1-0123456789abcdef0123456789";
    let auth = format!("HTTP 401 Unauthorized: Incorrect API key provided: {key}");
    for (message, code) in [
        (TIMEOUT_ERROR, "model_timeout"),
        (auth.as_str(), "model_auth"),
        ("HTTP 400 Bad Request: invalid parameter", "model_failed"),
    ] {
        let s = setup();
        let model = Arc::new(Replies::new(vec![Err(message)]));
        let f = filler(&s, big_index(), model, ScorerSet::default());
        let fill = f.fill_paper(&request(&s)).await.unwrap();
        assert_eq!(fill.calls, 1, "the second call is not made: {message}");
        assert_eq!(fill.cells.len(), 4);
        for column in 0..4 {
            assert_eq!(
                status_of(&s, column),
                (CellStatus::Failed, Some(code.into())),
                "{message}"
            );
        }
        assert!(!fill.model_error.unwrap().detail.contains(key));
    }
}

#[tokio::test]
async fn no_combination_of_failures_costs_a_paper_more_than_three_calls() {
    let s = setup();
    let outcomes: [Result<&str, &str>; 5] = [
        Ok(UNPARSABLE),
        Ok(CUT_AFTER_ONE),
        Ok(NOTHING_2),
        Err(TIMEOUT_ERROR),
        Err(QUOTA_ERROR),
    ];
    let mut longest = 0;
    for a in 0..outcomes.len() {
        for b in 0..outcomes.len() {
            for c in 0..outcomes.len() {
                for d in 0..outcomes.len() {
                    let replies = vec![outcomes[a], outcomes[b], outcomes[c], outcomes[d]];
                    let model = Arc::new(Replies::new(replies));
                    // Two plans: the passages do not fit one call.
                    let f = filler(&s, big_index(), model.clone(), ScorerSet::default());
                    let fill = f.fill_paper(&request(&s)).await.unwrap();
                    let label = format!("{a}{b}{c}{d}");
                    assert!(fill.calls <= 3, "{label}: {} calls", fill.calls);
                    assert_eq!(model.prompts().len(), fill.calls, "{label}");
                    longest = longest.max(fill.calls);
                    for cell in s.store.cells(&s.table).unwrap() {
                        let error = cell.error.unwrap_or_default();
                        assert!(
                            !error.contains(' ') && !error.contains("429"),
                            "{label}: {error}"
                        );
                    }
                    clear_all(&s);
                }
            }
        }
    }
    assert_eq!(longest, 3, "a retry really happens in some combination");
}

#[tokio::test]
async fn two_unparsable_calls_in_a_row_do_not_make_a_third_for_the_second_plan() {
    let s = setup();
    let model = Arc::new(Replies::new(vec![
        Ok(UNPARSABLE),
        Ok(UNPARSABLE),
        Ok(UNPARSABLE),
    ]));
    let f = filler(&s, big_index(), model.clone(), ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(
        fill.calls, 2,
        "one call and its retry; the second plan is not tried"
    );
    for column in 0..4 {
        assert_eq!(
            status_of(&s, column),
            (CellStatus::Failed, Some(UNPARSABLE_RESPONSE.into()))
        );
    }
}

#[tokio::test]
async fn more_columns_than_one_reply_holds_are_answered_in_two_calls() {
    let wide: Vec<ColumnSpec> = (0..13)
        .map(|i| ColumnSpec {
            question: format!("Question {i}?"),
            kind: ColumnKind::Text,
            options: vec![],
            instructions: String::new(),
        })
        .collect();
    let s = setup_with(&wide);
    let reply = |tags: std::ops::RangeInclusive<usize>| {
        let cells: Vec<String> = tags
            .map(|n| {
                format!(
                    r#"{{"column":"c{n}","answer":"LAION-400M","quote":"We pre-train a bi-encoder on LAION-400M","passage":1}}"#
                )
            })
            .collect();
        format!(r#"{{"cells":[{}]}}"#, cells.join(","))
    };
    let (first, second) = (reply(1..=7), reply(1..=6));
    let model = Arc::new(Replies::new(vec![Ok(first.as_str()), Ok(second.as_str())]));
    let f = filler(&s, paper_index(), model.clone(), ScorerSet::default());
    let fill = f.fill_paper(&request(&s)).await.unwrap();
    assert_eq!(fill.calls, 2);
    assert_eq!(fill.cells.len(), 13);
    assert!(fill.cells.iter().all(|c| c.status == CellStatus::Verified));
    assert!(model
        .limits()
        .iter()
        .all(|&limit| limit <= crate::research::tables::prompt::REPLY_TOKEN_CAP));
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
