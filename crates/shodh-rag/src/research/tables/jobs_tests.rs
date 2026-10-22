use std::path::PathBuf;

use chrono::NaiveDate;

use super::*;
use crate::harness::grounding::verify::ScorerSet;
use crate::research::results::TextModel;
use crate::research::tables::calls::{day_key, CallCounter};
use crate::research::tables::fill::paper_hash;
use crate::research::tables::testkit::{big_index, paper_index, utc, Index, ManualClock, Replies};
use crate::research::tables::validate::{NOT_FOUND_PARTIAL, NOT_FOUND_PARTIAL_RETRIED};
use crate::research::tables::{CellStatus, Column, ColumnSpec};
use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

const CORRECT: &str = include_str!("fixtures/reply_correct.txt");
/// A well-formed reply that finds nothing for the first two columns of a call.
const NOTHING_2: &str =
    r#"{"cells":[{"column":"c1","answer":null},{"column":"c2","answer":null}]}"#;
const AUTH_ERROR: &str = "HTTP 401 Unauthorized: invalid API key";
const TIMEOUT_ERROR: &str = "the provider did not answer in time";
const QUOTA_ERROR: &str = "HTTP 429 Too Many Requests: daily limit";

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

/// A workspace with one table of the four columns above and `papers` papers, each a file
/// of its own content.
struct Env {
    dir: tempfile::TempDir,
    workspace: String,
    store: Arc<TableStore>,
    counter: Arc<CallCounter>,
    clock: Arc<ManualClock>,
    table: String,
    columns: Vec<Column>,
    rows: Vec<TableRow>,
    papers: Vec<PathBuf>,
}

fn env_at(papers: usize, clock: ManualClock) -> Env {
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
    let papers: Vec<PathBuf> = (0..papers)
        .map(|i| {
            let path = dir.path().join(format!("paper-{i}.pdf"));
            std::fs::write(&path, format!("%PDF-1.4 paper {i}")).unwrap();
            path
        })
        .collect();
    let rows = store
        .add_rows(
            &table,
            &papers
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let counter = Arc::new(CallCounter::open(&db, None).unwrap());
    Env {
        dir,
        workspace,
        store,
        counter,
        clock: Arc::new(clock),
        table,
        columns,
        rows,
        papers,
    }
}

fn env(papers: usize) -> Env {
    env_at(papers, ManualClock::utc_at(utc(2026, 10, 22, 9, 0)))
}

struct NoResults;

#[async_trait::async_trait]
impl ResultSource for NoResults {
    async fn accepted(&self, _path: &str) -> ResearchResult<Vec<ResultRecord>> {
        Ok(Vec::new())
    }
}

fn filler(e: &Env, index: Index, model: Arc<dyn TextModel>) -> Filler {
    Filler {
        store: e.store.clone(),
        index: Arc::new(index),
        model,
        scorers: ScorerSet::default(),
    }
}

fn runner(e: &Env, index: Index, model: Arc<dyn TextModel>, cap: usize, token: &str) -> JobRunner {
    JobRunner::new(
        filler(e, index, model),
        e.counter.clone(),
        Arc::new(NoResults),
        cap,
    )
    .unwrap()
    .with_clock(e.clock.clone())
    .with_token(token)
}

fn replies(list: Vec<Result<&str, &str>>) -> Arc<Replies> {
    Arc::new(Replies::new(list))
}

/// The statuses of a row's cells in column order (a missing cell is `empty`).
fn statuses(e: &Env, row: usize) -> Vec<CellStatus> {
    e.columns
        .iter()
        .map(|c| {
            e.store
                .cell(&e.rows[row].id, &c.id)
                .unwrap()
                .map_or(CellStatus::Empty, |cell| cell.status)
        })
        .collect()
}

fn errors(e: &Env, row: usize) -> Vec<Option<String>> {
    e.columns
        .iter()
        .map(|c| {
            e.store
                .cell(&e.rows[row].id, &c.id)
                .unwrap()
                .and_then(|cell| cell.error)
        })
        .collect()
}

fn row_cells(e: &Env, row: usize) -> Vec<Cell> {
    let mut cells: Vec<Cell> = e
        .store
        .cells(&e.table)
        .unwrap()
        .into_iter()
        .filter(|c| c.row_id == e.rows[row].id)
        .collect();
    cells.sort_by(|a, b| a.column_id.cmp(&b.column_id));
    cells
}

fn all(status: CellStatus) -> Vec<CellStatus> {
    vec![status; 4]
}

fn day(e: &Env) -> NaiveDate {
    local_day(e.clock.as_ref(), e.clock.now())
}

fn queued_cells(e: &Env) -> usize {
    e.store
        .cells(&e.table)
        .unwrap()
        .iter()
        .filter(|c| c.status == CellStatus::Queued)
        .count()
}

fn job_of(e: &Env, id: &str) -> FillJob {
    e.store.fill_job(id).unwrap().unwrap()
}

fn request(e: &Env, row: usize) -> PaperRequest {
    PaperRequest {
        row: e.rows[row].clone(),
        columns: e.store.columns(&e.table).unwrap(),
        cells: e.store.cells(&e.table).unwrap(),
        results: Vec::new(),
    }
}

#[tokio::test]
async fn a_restarted_job_resumes_where_it_stopped_and_never_refills_done_cells() {
    let e = env(3);
    let first = runner(&e, paper_index(), replies(vec![]), 30, "process-1");
    let job = first.start(&e.table, 3).await.unwrap();
    // Process 1 claims the job, fills the first paper and dies: its job stays `running`.
    assert!(matches!(
        e.store.claim_fill_job(&job.id, "process-1").unwrap(),
        Claim::Claimed(_)
    ));
    filler(&e, paper_index(), replies(vec![Ok(CORRECT)]))
        .fill_paper(&request(&e, 0))
        .await
        .unwrap();
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(statuses(&e, 1), all(CellStatus::Queued));
    let done_before = row_cells(&e, 0);

    // A runner of the same process never takes it again while it runs there.
    assert!(first.resume_due().await.unwrap().is_empty());
    assert_eq!(
        first.run(&job.id).await.unwrap().outcome,
        JobOutcome::NotClaimed {
            state: Some(JobState::Running)
        }
    );

    // After the restart, the job is due and is resumed where it stopped.
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let second = runner(&e, paper_index(), model.clone(), 30, "process-2");
    assert_eq!(second.resume_due().await.unwrap(), vec![job.id.clone()]);
    let report = second.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(report.calls, 2, "one call for each paper left");
    assert_eq!(model.prompts().len(), 2);
    assert_eq!(
        row_cells(&e, 0),
        done_before,
        "the done paper is never refilled"
    );
    assert_eq!(statuses(&e, 1), all(CellStatus::Verified));
    assert_eq!(statuses(&e, 2), all(CellStatus::Verified));
    let finished = job_of(&e, &job.id);
    assert_eq!(finished.state, JobState::Done);
    assert_eq!(finished.done_calls, 2);
    assert_eq!(e.counter.count(day(&e)).unwrap(), 2);
    assert!(second.resume_due().await.unwrap().is_empty());
    assert_eq!(queued_cells(&e), 0);
}

#[tokio::test]
async fn a_quota_error_pauses_until_the_reset_and_resumes_after_it() {
    let start = utc(2099, 1, 1, 0, 0);
    let reset = utc(2099, 1, 1, 6, 0);
    let e = env_at(2, ManualClock::utc_at(start));
    let quota = format!(
        "HTTP 429 Too Many Requests: free-models-per-day. X-RateLimit-Reset: {}",
        reset.timestamp_millis()
    );
    let model = replies(vec![Ok(CORRECT), Err(quota.as_str()), Ok(CORRECT)]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();

    let report = r.run(&job.id).await.unwrap();
    assert_eq!(
        report.outcome,
        JobOutcome::Paused {
            reason: PauseReason::Quota,
            until: Some(reset)
        }
    );
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(
        statuses(&e, 1),
        all(CellStatus::Queued),
        "cells the quota stopped are left queued"
    );
    let paused = job_of(&e, &job.id);
    assert_eq!(paused.state, JobState::Paused);
    assert_eq!(paused.pause_reason, Some(PauseReason::Quota));
    assert_eq!(paused.last_error.as_deref(), Some(MODEL_QUOTA));
    assert_eq!(
        e.counter.count(day(&e)).unwrap(),
        2,
        "the refused call counts"
    );

    e.clock.set(reset - TimeDelta::seconds(1));
    assert!(r.resume_due().await.unwrap().is_empty());
    e.clock.set(reset);
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(report.calls, 1);
    assert_eq!(statuses(&e, 1), all(CellStatus::Verified));
    assert_eq!(model.prompts().len(), 3);
}

#[tokio::test]
async fn a_quota_error_without_a_reset_time_pauses_until_the_next_local_midnight() {
    // 23:00 UTC is 01:00 the next day at +02:00, so the pause runs to 22:00 UTC.
    let e = env_at(
        1,
        ManualClock::new(utc(2026, 10, 22, 23, 0), &[(utc(2000, 1, 1, 0, 0), 7_200)]),
    );
    let r = runner(&e, paper_index(), replies(vec![Err(QUOTA_ERROR)]), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(
        report.outcome,
        JobOutcome::Paused {
            reason: PauseReason::Quota,
            until: Some(utc(2026, 10, 23, 22, 0))
        }
    );
    assert_eq!(
        e.counter
            .count(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap())
            .unwrap(),
        1,
        "the call counts on the local date"
    );
}

#[tokio::test]
async fn cancel_stops_after_the_current_call_and_keeps_finished_cells() {
    let e = env(2);
    let job = runner(&e, big_index(), replies(vec![]), 30, "p")
        .start(&e.table, 4)
        .await
        .unwrap();
    // The user cancels while the first of the paper's two calls is with the model.
    let mut model = Replies::new(vec![Ok(NOTHING_2), Ok(NOTHING_2)]);
    let (store, id) = (e.store.clone(), job.id.clone());
    model.before_reply = Some(Box::new(move || {
        store.cancel_fill_job(&id).unwrap();
    }));
    let model = Arc::new(model);
    let r = runner(&e, big_index(), model.clone(), 30, "p");

    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Cancelled);
    assert_eq!(report.calls, 1);
    assert_eq!(model.prompts().len(), 1, "no call after the cancel");
    let first = statuses(&e, 0);
    assert_eq!(&first[..2], &[CellStatus::NotFound, CellStatus::NotFound]);
    assert_eq!(
        &first[2..],
        &[CellStatus::Empty, CellStatus::Empty],
        "the second call's cells go back to empty"
    );
    assert_eq!(statuses(&e, 1), all(CellStatus::Empty));
    assert_eq!(job_of(&e, &job.id).state, JobState::Cancelled);
    assert_eq!(queued_cells(&e), 0);
    assert!(r.resume_due().await.unwrap().is_empty());
    assert_eq!(e.counter.count(day(&e)).unwrap(), 1);
}

#[tokio::test]
async fn cancelling_a_paused_job_empties_its_queued_cells_at_once() {
    let e = env(2);
    let r = runner(&e, paper_index(), replies(vec![Err(AUTH_ERROR)]), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    r.run(&job.id).await.unwrap();
    assert_eq!(statuses(&e, 1), all(CellStatus::Queued));
    let cancelled = r.cancel(&job.id).await.unwrap().unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert_eq!(statuses(&e, 1), all(CellStatus::Empty));
    assert_eq!(
        r.run(&job.id).await.unwrap().outcome,
        JobOutcome::NotClaimed {
            state: Some(JobState::Cancelled)
        }
    );
    // A new fill of the table can start.
    assert_ne!(r.start(&e.table, 2).await.unwrap().id, job.id);
}

#[tokio::test]
async fn the_daily_cap_is_never_exceeded() {
    let e = env(3);
    let cap = 4;
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let r = runner(&e, paper_index(), model.clone(), cap, "process-1");
    let job = r.start(&e.table, 3).await.unwrap();
    let today = day(&e);
    let midnight = utc(2026, 10, 23, 0, 0);

    // Papers 1 and 2 fit (0 + 3 and 1 + 3 are within 4); paper 3 would not (2 + 3).
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(
        report.outcome,
        JobOutcome::Paused {
            reason: PauseReason::DailyCap,
            until: Some(midnight)
        }
    );
    assert_eq!(report.calls, 2);
    assert_eq!(e.counter.count(today).unwrap(), 2);
    assert_eq!(statuses(&e, 2), all(CellStatus::Queued));

    // A restart on the same day makes no call, whether resumed or started by hand.
    let restarted_model = replies(vec![Ok(CORRECT)]);
    let restarted = runner(&e, paper_index(), restarted_model.clone(), cap, "process-2");
    e.clock.set(midnight - TimeDelta::minutes(1));
    assert!(restarted.resume_due().await.unwrap().is_empty());
    let again = restarted.run(&job.id).await.unwrap();
    assert!(matches!(
        again.outcome,
        JobOutcome::Paused {
            reason: PauseReason::DailyCap,
            ..
        }
    ));
    assert_eq!(again.calls, 0);
    assert!(restarted_model.prompts().is_empty());
    assert!(e.counter.count(today).unwrap() <= cap);

    // After local midnight the job is due and counts on the new day.
    e.clock.set(midnight);
    assert_eq!(restarted.resume_due().await.unwrap(), vec![job.id.clone()]);
    let report = restarted.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(statuses(&e, 2), all(CellStatus::Verified));
    assert_eq!(e.counter.count(today).unwrap(), 2);
    assert_eq!(e.counter.count(day(&e)).unwrap(), 1);
    assert_eq!(day_key(day(&e)), "2026-10-23");
    assert_eq!(model.prompts().len(), 2);
}

#[tokio::test]
async fn the_cap_holds_when_other_calls_take_the_room_mid_paper() {
    let e = env(1);
    let cap = 4;
    let today = day(&e);
    // While the paper's first call is out, another table's job takes three calls.
    let mut model = Replies::new(vec![Ok(NOTHING_2), Ok(NOTHING_2)]);
    let counter = e.counter.clone();
    model.before_reply = Some(Box::new(move || {
        counter.record(today, 3).unwrap();
    }));
    let model = Arc::new(model);
    let r = runner(&e, big_index(), model.clone(), cap, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert!(matches!(
        report.outcome,
        JobOutcome::Paused {
            reason: PauseReason::DailyCap,
            ..
        }
    ));
    assert_eq!(model.prompts().len(), 1, "the second call was refused");
    assert_eq!(e.counter.count(today).unwrap(), cap);
    let first = statuses(&e, 0);
    assert_eq!(&first[2..], &[CellStatus::Queued, CellStatus::Queued]);
}

#[tokio::test]
async fn estimate_counts_only_cache_misses() {
    let e = env(2);
    let filled = runner(
        &e,
        paper_index(),
        replies(vec![Ok(CORRECT), Ok(CORRECT)]),
        30,
        "p",
    );
    let job = filled.start(&e.table, 2).await.unwrap();
    assert_eq!(filled.run(&job.id).await.unwrap().outcome, JobOutcome::Done);

    // A second table over the same two papers and a third one asks the same questions:
    // the two papers are answered from the cache, the new one needs a call.
    let other = e.store.create_table(&e.workspace, "Again").unwrap().id;
    for spec in specs() {
        e.store.add_column(&other, &spec).unwrap();
    }
    let third = e.dir.path().join("paper-new.pdf");
    std::fs::write(&third, b"%PDF-1.4 a third paper").unwrap();
    let mut docs: Vec<String> = e
        .papers
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    docs.push(third.to_string_lossy().into_owned());
    e.store.add_rows(&other, &docs).unwrap();

    let model = replies(vec![]);
    let small = runner(&e, paper_index(), model.clone(), 30, "p");
    assert_eq!(
        small.estimate(&other).await.unwrap(),
        FillEstimate {
            papers: 3,
            columns: 4,
            cells: 12,
            calls: 1,
            cached: 8,
        }
    );
    // Passages that do not fit one call need two.
    let big = runner(&e, big_index(), model.clone(), 30, "p");
    assert_eq!(big.estimate(&other).await.unwrap().calls, 2);

    // In the first table everything is filled; a `not_found` that missed passages is
    // filled once more (it is never cached), and once retried it is left alone.
    assert_eq!(
        small.estimate(&e.table).await.unwrap(),
        FillEstimate {
            columns: 4,
            ..FillEstimate::default()
        }
    );
    let hash = paper_hash(&e.papers[1].to_string_lossy()).unwrap();
    for (column, error) in [
        (&e.columns[0], NOT_FOUND_PARTIAL),
        (&e.columns[1], NOT_FOUND_PARTIAL_RETRIED),
    ] {
        let cell = Cell {
            table_id: e.table.clone(),
            row_id: e.rows[1].id.clone(),
            column_id: column.id.clone(),
            status: CellStatus::NotFound,
            value: None,
            quote: None,
            chunk_id: None,
            page: None,
            bboxes: None,
            model: Some("recorded-model".to_string()),
            column_revision: Some(column.revision),
            paper_hash: Some(hash.clone()),
            filled_at: Some("2026-10-22T09:00:00.000Z".to_string()),
            edited_at: None,
            error: Some(error.to_string()),
        };
        assert!(e.store.store_fill(&cell, None).unwrap());
        e.store
            .lock()
            .execute(
                "DELETE FROM fill_cache WHERE paper_hash = ?1 AND column_key = ?2",
                params![hash, column.key],
            )
            .unwrap();
    }
    assert_eq!(
        small.estimate(&e.table).await.unwrap(),
        FillEstimate {
            papers: 1,
            columns: 4,
            cells: 1,
            calls: 1,
            cached: 0,
        }
    );
    assert!(model.prompts().is_empty(), "an estimate calls no model");
}

#[tokio::test]
async fn one_active_job_per_table() {
    let e = env(2);
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let a = r.start(&e.table, 2).await.unwrap();
    let b = r.start(&e.table, 5).await.unwrap();
    assert_eq!(a.id, b.id, "starting again returns the active job");
    assert_eq!(e.store.active_fill_job(&e.table).unwrap(), Some(a.clone()));
    let second = e.store.lock().execute(
        "INSERT INTO table_fill_job(id, table_id, created_at, state, planned_calls, done_calls)
         VALUES ('second', ?1, '2026-10-22T09:00:00.000Z', 'paused', 0, 0)",
        params![e.table],
    );
    assert!(second.is_err(), "the database refuses a second active job");

    // Two runs of one job at once: one claims it, the other is turned away.
    let (one, two) = tokio::join!(r.run(&a.id), r.run(&a.id));
    let outcomes = [one.unwrap().outcome, two.unwrap().outcome];
    assert_eq!(
        outcomes.iter().filter(|o| **o == JobOutcome::Done).count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, JobOutcome::NotClaimed { .. }))
            .count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(model.prompts().len(), 2, "each paper was asked once");

    // A finished job makes room for the next.
    let c = r.start(&e.table, 0).await.unwrap();
    assert_ne!(c.id, a.id);
    assert_eq!(c.state, JobState::Queued);
    assert!(matches!(
        r.start("no-such-table", 1).await,
        Err(ResearchError::NotFound(_))
    ));
}

#[tokio::test]
async fn an_auth_error_pauses_the_job_until_the_user_starts_it_again() {
    let e = env(2);
    let r = runner(&e, paper_index(), replies(vec![Err(AUTH_ERROR)]), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(
        report.outcome,
        JobOutcome::Paused {
            reason: PauseReason::Auth,
            until: None
        }
    );
    assert_eq!(report.calls, 1);
    assert_eq!(statuses(&e, 0), all(CellStatus::Failed));
    assert_eq!(errors(&e, 0), vec![Some(MODEL_AUTH.to_string()); 4]);
    assert_eq!(
        statuses(&e, 1),
        all(CellStatus::Queued),
        "the other papers are not failed for it"
    );
    let paused = job_of(&e, &job.id);
    assert_eq!(paused.next_attempt_at, None);
    assert_eq!(paused.pause_reason, Some(PauseReason::Auth));

    // It is never due by itself.
    e.clock.advance(TimeDelta::days(3));
    assert!(r.resume_due().await.unwrap().is_empty());

    // The user fixes the key and starts the fill again: the same job goes on.
    let fixed = runner(
        &e,
        paper_index(),
        replies(vec![Ok(CORRECT), Ok(CORRECT)]),
        30,
        "p",
    );
    assert_eq!(fixed.start(&e.table, 2).await.unwrap().id, job.id);
    let report = fixed.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(statuses(&e, 1), all(CellStatus::Verified));
}

#[tokio::test]
async fn search_failed_stops_the_job_and_unqueues_its_cells() {
    let e = env(2);
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let r = runner(
        &e,
        paper_index().failing("the search models are not installed"),
        model.clone(),
        30,
        "p",
    );
    let job = r.start(&e.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    let JobOutcome::Failed { error } = &report.outcome else {
        panic!("{:?}", report.outcome);
    };
    assert!(error.starts_with(SEARCH_FAILED), "{error}");
    assert_eq!(report.calls, 0);
    assert!(model.prompts().is_empty());
    assert_eq!(statuses(&e, 0), all(CellStatus::Failed));
    assert_eq!(
        statuses(&e, 1),
        all(CellStatus::Empty),
        "the job stops after the first paper"
    );
    let failed = job_of(&e, &job.id);
    assert_eq!(failed.state, JobState::Failed);
    assert!(failed
        .last_error
        .as_deref()
        .is_some_and(|e| e.starts_with(SEARCH_FAILED)));
    assert_eq!(queued_cells(&e), 0);
}

#[tokio::test]
async fn an_unindexed_paper_is_skipped_and_the_job_goes_on() {
    let e = env(2);
    let model = replies(vec![Ok(CORRECT)]);
    let index = paper_index().without(&e.papers[0].to_string_lossy());
    let r = runner(&e, index, model.clone(), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(report.not_indexed, 1);
    assert_eq!(report.calls, 1);
    assert_eq!(statuses(&e, 0), all(CellStatus::Failed));
    assert_eq!(errors(&e, 0), vec![Some(PAPER_NOT_INDEXED.to_string()); 4]);
    assert_eq!(statuses(&e, 1), all(CellStatus::Verified));
}

#[tokio::test]
async fn two_unreachable_papers_in_a_row_pause_the_job() {
    let e = env(3);
    let model = replies(vec![
        Err(TIMEOUT_ERROR),
        Err(TIMEOUT_ERROR),
        Ok(CORRECT),
        Ok(CORRECT),
        Ok(CORRECT),
    ]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 3).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    let until = e.clock.now() + TimeDelta::minutes(UNREACHABLE_PAUSE_MINUTES);
    assert_eq!(
        report.outcome,
        JobOutcome::Paused {
            reason: PauseReason::Unreachable,
            until: Some(until)
        }
    );
    assert_eq!(report.calls, 2);
    assert_eq!(errors(&e, 0), vec![Some(MODEL_TIMEOUT.to_string()); 4]);
    assert_eq!(statuses(&e, 2), all(CellStatus::Queued));

    e.clock.set(until - TimeDelta::seconds(1));
    assert!(r.resume_due().await.unwrap().is_empty());
    e.clock.set(until);
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(report.calls, 3, "the failed papers are filled again");
    for row in 0..3 {
        assert_eq!(statuses(&e, row), all(CellStatus::Verified));
    }
}

#[tokio::test]
async fn one_unreachable_paper_does_not_stop_the_job() {
    let e = env(2);
    let r = runner(
        &e,
        paper_index(),
        replies(vec![Err(TIMEOUT_ERROR), Ok(CORRECT)]),
        30,
        "p",
    );
    let job = r.start(&e.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(statuses(&e, 0), all(CellStatus::Failed));
    assert_eq!(errors(&e, 0), vec![Some(MODEL_TIMEOUT.to_string()); 4]);
    assert_eq!(statuses(&e, 1), all(CellStatus::Verified));
}

#[tokio::test]
async fn an_edit_during_a_run_is_never_requeued_or_overwritten() {
    let e = env(2);
    // While the first paper is with the model, the user edits a cell of that paper and
    // a cell of the next one.
    let mut model = Replies::new(vec![Ok(CORRECT), Ok(CORRECT)]);
    let store = e.store.clone();
    let (row0, row1) = (e.rows[0].id.clone(), e.rows[1].id.clone());
    let (col0, col3) = (e.columns[0].id.clone(), e.columns[3].id.clone());
    model.before_reply = Some(Box::new(move || {
        store.edit_cell(&row0, &col3, "Survey").unwrap();
        store.edit_cell(&row1, &col0, "my dataset").unwrap();
    }));
    let model = Arc::new(model);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    assert_eq!(r.run(&job.id).await.unwrap().outcome, JobOutcome::Done);

    let first = statuses(&e, 0);
    assert_eq!(&first[..3], &all(CellStatus::Verified)[..3]);
    assert_eq!(
        first[3],
        CellStatus::Edited,
        "the fill did not overwrite it"
    );
    let edited = e
        .store
        .cell(&e.rows[1].id, &e.columns[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(edited.status, CellStatus::Edited);
    assert_eq!(
        edited.value.map(|v| v.display()).as_deref(),
        Some("my dataset")
    );
    assert!(
        !model.prompts()[1].contains("Which pre-training dataset is used?"),
        "the edited column is not asked about"
    );

    // A later fill neither queues nor asks about the edits.
    let again = runner(&e, paper_index(), replies(vec![]), 30, "p");
    let next = again.start(&e.table, 0).await.unwrap();
    assert!(matches!(
        e.store.claim_fill_job(&next.id, "p").unwrap(),
        Claim::Claimed(_)
    ));
    assert_eq!(statuses(&e, 0)[3], CellStatus::Edited);
    assert_eq!(statuses(&e, 1)[0], CellStatus::Edited);
    assert_eq!(queued_cells(&e), 0);
}

/// A model that fails hard, as a bug in a provider client would.
struct Panics;

#[async_trait::async_trait]
impl TextModel for Panics {
    fn model_id(&self) -> String {
        "panics".to_string()
    }
    async fn complete(&self, _prompt: &str, _max_tokens: usize) -> Result<String, String> {
        panic!("the model client failed hard");
    }
}

#[tokio::test]
async fn a_run_that_panics_releases_its_job_for_resume() {
    let e = env(1);
    let r = runner(&e, paper_index(), Arc::new(Panics), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    assert!(matches!(r.run(&job.id).await, Err(ResearchError::Task(_))));
    let released = job_of(&e, &job.id);
    assert_eq!(released.state, JobState::Queued);
    assert_eq!(statuses(&e, 0), all(CellStatus::Queued));
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    let fixed = runner(&e, paper_index(), replies(vec![Ok(CORRECT)]), 30, "p");
    assert_eq!(fixed.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
}

#[test]
fn a_cap_below_one_paper_is_refused() {
    let e = env(1);
    let result = JobRunner::new(
        filler(&e, paper_index(), replies(vec![])),
        e.counter.clone(),
        Arc::new(NoResults),
        CALLS_PER_PAPER_RESERVE - 1,
    );
    assert!(matches!(result, Err(ResearchError::Invalid(_))));
}

#[test]
fn states_and_reasons_round_trip_their_stored_names() {
    for state in [
        JobState::Queued,
        JobState::Running,
        JobState::Paused,
        JobState::Done,
        JobState::Cancelled,
        JobState::Failed,
    ] {
        assert_eq!(JobState::parse(state.as_str()), Some(state));
    }
    for reason in [
        PauseReason::DailyCap,
        PauseReason::Quota,
        PauseReason::Auth,
        PauseReason::Unreachable,
    ] {
        assert_eq!(PauseReason::parse(reason.as_str()), Some(reason));
    }
    assert_eq!(JobState::parse("paused-ish"), None);
    assert!(JobState::Paused.is_active());
    assert!(!JobState::Done.is_active());
}
