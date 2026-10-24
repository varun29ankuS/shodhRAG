use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::harness::grounding::verify::ScorerSet;
use crate::research::results::TextModel;
use crate::research::tables::calls::{day_key, CallCounter, DailyCap, MIN_DAILY_CAP};
use crate::research::tables::fill::paper_hash;
use crate::research::tables::testkit::{big_index, paper_index, utc, Index, ManualClock, Replies};
use crate::research::tables::validate::NOT_FOUND_PARTIAL;
use crate::research::tables::{Column, ColumnSpec};
use crate::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

const CORRECT: &str = include_str!("fixtures/reply_correct.txt");
/// A well-formed reply that finds nothing for the first two columns of a call.
const NOTHING_2: &str =
    r#"{"cells":[{"column":"c1","answer":null},{"column":"c2","answer":null}]}"#;
/// A well-formed reply that finds nothing for the one column of a call.
const NOTHING_1: &str = r#"{"cells":[{"column":"c1","answer":null}]}"#;
const UNPARSABLE: &str = include_str!("fixtures/reply_unparsable.txt");
const AUTH_ERROR: &str = "HTTP 401 Unauthorized: invalid API key";
const TIMEOUT_ERROR: &str = "the provider did not answer in time";
const BAD_REQUEST: &str = "HTTP 400 Bad Request: invalid parameter";
/// OpenRouter's reply when a free model's upstream provider limits it: no reset header,
/// no retry-after (the shape of `provider_error`'s
/// `openrouter_upstream_rate_limit_of_a_free_model`).
const UPSTREAM_LIMIT: &str = r#"429 Provider returned error {"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"nvidia/nemotron-3-super-120b-a12b:free is temporarily rate-limited upstream. Please retry shortly, or add your own key to accumulate your rate limits: https://openrouter.ai/settings/integrations","provider_name":"Nvidia"}}}"#;
/// A daily free-model limit with no reset header.
const DAILY_LIMIT: &str =
    r#"429 {"error":{"message":"Rate limit exceeded: free-models-per-day.","code":429}}"#;

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

fn env_at(papers: usize, now: DateTime<Utc>) -> Env {
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
    let rows = store.add_rows(&table, &docs(&papers)).unwrap();
    let counter = Arc::new(CallCounter::open(&db, None).unwrap());
    Env {
        dir,
        workspace,
        store,
        counter,
        clock: Arc::new(ManualClock::new(now)),
        table,
        columns,
        rows,
        papers,
    }
}

fn env(papers: usize) -> Env {
    env_at(papers, utc(2026, 10, 22, 9, 0))
}

fn docs(papers: &[PathBuf]) -> Vec<String> {
    papers
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
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

fn runner_with(
    e: &Env,
    index: Index,
    model: Arc<dyn TextModel>,
    counter: Arc<CallCounter>,
    cap: usize,
    token: &str,
) -> JobRunner {
    JobRunner::new(
        filler(e, index, model),
        counter,
        Arc::new(NoResults),
        daily(cap),
    )
    .with_clock(e.clock.clone())
    .with_token(token)
}

/// A shared cap of `calls`.
fn daily(calls: usize) -> DailyCap {
    DailyCap::new(calls).unwrap()
}

fn runner(e: &Env, index: Index, model: Arc<dyn TextModel>, cap: usize, token: &str) -> JobRunner {
    runner_with(e, index, model, e.counter.clone(), cap, token)
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

fn today(e: &Env) -> chrono::NaiveDate {
    utc_day(e.clock.now())
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

/// Another process claims the job now, as if it then died.
fn claimed_by(e: &Env, job: &str, token: &str) {
    let now = e.clock.now();
    assert!(matches!(
        e.store
            .claim_fill_job(job, token, &timestamp(now), &lease_cutoff(now))
            .unwrap(),
        Claim::Claimed(_)
    ));
}

fn paused(reason: PauseReason, until: Option<DateTime<Utc>>) -> JobOutcome {
    JobOutcome::Paused { reason, until }
}

#[tokio::test]
async fn a_restarted_job_resumes_where_it_stopped_and_never_refills_done_cells() {
    let e = env(3);
    let first = runner(&e, paper_index(), replies(vec![]), 30, "process-1");
    let job = first.start(&e.table, 3).await.unwrap();
    // Process 1 claims the job, fills the first paper and dies: its job stays `running`.
    claimed_by(&e, &job.id, "process-1");
    filler(&e, paper_index(), replies(vec![Ok(CORRECT)]))
        .fill_paper(
            &request(&e, 0),
            &CounterGate::new(e.counter.clone(), daily(30)).with_clock(e.clock.clone()),
        )
        .await
        .unwrap();
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(statuses(&e, 1), all(CellStatus::Queued));
    let done_before = row_cells(&e, 0);

    // After the restart, the job is due once process 1's lease has run out.
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let second = runner(&e, paper_index(), model.clone(), 30, "process-2");
    e.clock.advance(TimeDelta::minutes(LEASE_MINUTES));
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
    assert_eq!(
        e.counter.count(today(&e)).unwrap(),
        3,
        "the hand fill counted too"
    );
    assert!(second.resume_due().await.unwrap().is_empty());
    assert_eq!(queued_cells(&e), 0);
}

#[tokio::test]
async fn a_live_lease_is_never_taken_over_and_an_expired_one_is() {
    let e = env(1);
    let r = runner(
        &e,
        paper_index(),
        replies(vec![Ok(CORRECT)]),
        30,
        "process-2",
    );
    let job = r.start(&e.table, 1).await.unwrap();
    claimed_by(&e, &job.id, "process-1");

    // A minute short of the lease, process 1 may still be working.
    e.clock
        .advance(TimeDelta::minutes(LEASE_MINUTES) - TimeDelta::seconds(1));
    assert!(r.resume_due().await.unwrap().is_empty());
    assert_eq!(
        r.run(&job.id).await.unwrap().outcome,
        JobOutcome::NotClaimed {
            state: Some(JobState::Running)
        }
    );
    assert_eq!(statuses(&e, 0), all(CellStatus::Queued));

    e.clock.advance(TimeDelta::seconds(1));
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    assert_eq!(r.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
}

#[tokio::test]
async fn a_cancel_of_a_dead_process_job_is_finished_by_the_next_claim() {
    let e = env(2);
    let r = runner(&e, paper_index(), replies(vec![]), 30, "process-2");
    let job = r.start(&e.table, 2).await.unwrap();
    claimed_by(&e, &job.id, "process-1");
    // Its lease is still live: the cancel is only asked.
    let asked = r.cancel(&job.id).await.unwrap().unwrap();
    assert_eq!(asked.state, JobState::Running);
    assert!(asked.cancel_requested);
    assert_eq!(statuses(&e, 0), all(CellStatus::Queued));

    e.clock.advance(TimeDelta::minutes(LEASE_MINUTES));
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    assert_eq!(
        r.run(&job.id).await.unwrap().outcome,
        JobOutcome::NotClaimed {
            state: Some(JobState::Cancelled)
        }
    );
    assert_eq!(statuses(&e, 0), all(CellStatus::Empty));
    assert_eq!(statuses(&e, 1), all(CellStatus::Empty));
    assert!(r.resume_due().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_quota_error_pauses_until_the_reset_and_resumes_after_it() {
    let start = utc(2099, 1, 1, 0, 0);
    let reset = utc(2099, 1, 1, 6, 0);
    let e = env_at(2, start);
    let quota = format!(
        "HTTP 429 Too Many Requests: free-models-per-day. X-RateLimit-Reset: {}",
        reset.timestamp_millis()
    );
    let model = replies(vec![Ok(CORRECT), Err(quota.as_str()), Ok(CORRECT)]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();

    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, paused(PauseReason::Quota, Some(reset)));
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(
        statuses(&e, 1),
        all(CellStatus::Queued),
        "cells the quota stopped are left queued"
    );
    let job_now = job_of(&e, &job.id);
    assert_eq!(job_now.state, JobState::Paused);
    assert_eq!(job_now.last_error.as_deref(), Some("model_quota:daily"));
    assert_eq!(
        e.counter.count(today(&e)).unwrap(),
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

/// One run of a one-paper job whose call meets `error`; the pause it ends in.
async fn pause_after(error: &str, now: DateTime<Utc>) -> (JobOutcome, FillJob) {
    let e = env_at(1, now);
    let r = runner(&e, paper_index(), replies(vec![Err(error)]), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    let outcome = r.run(&job.id).await.unwrap().outcome;
    (outcome, job_of(&e, &job.id))
}

#[tokio::test]
async fn every_rate_limit_hint_sets_its_own_wait() {
    // Far ahead of the real clock, so a header reset reads as future to the classifier.
    let now = utc(2099, 1, 1, 12, 0);
    let ms = |at: DateTime<Utc>| at.timestamp_millis();

    // A per-minute limit with its reset header: the reset.
    let soon = now + TimeDelta::seconds(40);
    let per_minute = format!(
        r#"429 {{"error":{{"message":"Rate limit exceeded: free-models-per-min.","code":429,"metadata":{{"headers":{{"X-RateLimit-Reset":"{}"}}}}}}}}"#,
        ms(soon)
    );
    assert_eq!(
        pause_after(&per_minute, now).await.0,
        paused(PauseReason::Quota, Some(soon))
    );

    // A reset that reads as past (this computer's clock runs fast): a minute, not midnight.
    let past = now - TimeDelta::seconds(30);
    let stale = format!(
        r#"429 {{"error":{{"message":"Rate limit exceeded: free-models-per-min.","code":429,"metadata":{{"headers":{{"X-RateLimit-Reset":"{}"}}}}}}}}"#,
        ms(past)
    );
    assert_eq!(
        pause_after(&stale, now).await.0,
        paused(
            PauseReason::Quota,
            Some(now + TimeDelta::seconds(STALE_RESET_WAIT_SECS))
        )
    );

    // A retry-after: that wait.
    assert_eq!(
        pause_after(
            "HTTP 429 Too Many Requests: rate limit, retry after 20 seconds",
            now
        )
        .await
        .0,
        paused(PauseReason::Quota, Some(now + TimeDelta::seconds(20)))
    );

    // A daily limit with no hint: the provider's daily reset, 00:00 UTC.
    assert_eq!(
        pause_after(DAILY_LIMIT, now).await.0,
        paused(PauseReason::Quota, Some(utc(2099, 1, 2, 0, 0)))
    );

    // An upstream limit with no hint at all: the first rung of the ladder.
    let (outcome, job) = pause_after(UPSTREAM_LIMIT, now).await;
    assert_eq!(
        outcome,
        paused(PauseReason::Quota, Some(now + TimeDelta::minutes(1)))
    );
    assert_eq!(job.backoff_step, 1);
}

#[tokio::test]
async fn a_headerless_rate_limit_climbs_the_ladder_and_then_waits_for_the_user() {
    let e = env(1);
    let model = replies(vec![Err(UPSTREAM_LIMIT); 9]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    let mut waits = Vec::new();
    loop {
        let report = r.run(&job.id).await.unwrap();
        assert_eq!(report.calls, 1);
        match report.outcome {
            JobOutcome::Paused {
                reason: PauseReason::Quota,
                until: Some(until),
            } => {
                waits.push((until - e.clock.now()).num_minutes());
                e.clock.set(until);
                assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
            }
            other => {
                assert_eq!(other, paused(PauseReason::Quota, None));
                break;
            }
        }
    }
    assert_eq!(waits, BACKOFF_LADDER_MINUTES.to_vec());
    assert_eq!(
        model.prompts().len(),
        BACKOFF_LADDER_MINUTES.len() + 1,
        "one probe per rung and the one that gives up"
    );
    assert_eq!(statuses(&e, 0), all(CellStatus::Queued));
    e.clock.advance(TimeDelta::days(2));
    assert!(
        r.resume_due().await.unwrap().is_empty(),
        "it waits for the user"
    );
}

#[tokio::test]
async fn a_successful_call_resets_the_ladder() {
    let e = env(2);
    let model = replies(vec![
        Err(UPSTREAM_LIMIT),
        Err(UPSTREAM_LIMIT),
        Ok(CORRECT),
        Err(UPSTREAM_LIMIT),
    ]);
    let r = runner(&e, paper_index(), model, 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    for minutes in [1, 2] {
        let outcome = r.run(&job.id).await.unwrap().outcome;
        let until = e.clock.now() + TimeDelta::minutes(minutes);
        assert_eq!(outcome, paused(PauseReason::Quota, Some(until)));
        e.clock.set(until);
    }
    // The first paper answers, the second meets the limit again: back to the first rung.
    let outcome = r.run(&job.id).await.unwrap().outcome;
    assert_eq!(
        outcome,
        paused(
            PauseReason::Quota,
            Some(e.clock.now() + TimeDelta::minutes(1))
        )
    );
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(job_of(&e, &job.id).backoff_step, 1);
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
        store
            .cancel_fill_job(&id, "p", true, "2000-01-01T00:00:00.000Z")
            .unwrap();
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
    assert_eq!(e.counter.count(today(&e)).unwrap(), 1);
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
    assert_ne!(r.start(&e.table, 2).await.unwrap().id, job.id);
}

#[tokio::test]
async fn the_daily_cap_is_never_exceeded() {
    let e = env(3);
    // One planned call and its retry per paper: papers fit while two calls are left.
    let cap = 3;
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let r = runner(&e, paper_index(), model.clone(), cap, "process-1");
    let job = r.start(&e.table, 3).await.unwrap();
    let day = today(&e);
    let midnight = utc(2026, 10, 23, 0, 0);

    // Papers 1 and 2 fit (0 + 2 and 1 + 2 are within 3); paper 3 would not (2 + 2).
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(
        report.outcome,
        paused(PauseReason::DailyCap, Some(midnight))
    );
    assert_eq!(report.calls, 2);
    assert_eq!(e.counter.count(day).unwrap(), 2);
    assert_eq!(statuses(&e, 2), all(CellStatus::Queued));

    // Another process on the same day makes no call, whether resumed or started by hand.
    let restarted_model = replies(vec![Ok(CORRECT)]);
    let restarted = runner(&e, paper_index(), restarted_model.clone(), cap, "process-2");
    e.clock.set(midnight - TimeDelta::minutes(1));
    assert!(restarted.resume_due().await.unwrap().is_empty());
    let again = restarted.run(&job.id).await.unwrap();
    assert_eq!(again.outcome, paused(PauseReason::DailyCap, Some(midnight)));
    assert_eq!(again.calls, 0);
    assert!(restarted_model.prompts().is_empty());
    assert!(e.counter.count(day).unwrap() <= cap);

    // After 00:00 UTC the job is due and counts on the new day.
    e.clock.set(midnight);
    assert_eq!(restarted.resume_due().await.unwrap(), vec![job.id.clone()]);
    let report = restarted.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(statuses(&e, 2), all(CellStatus::Verified));
    assert_eq!(e.counter.count(day).unwrap(), 2);
    assert_eq!(e.counter.count(today(&e)).unwrap(), 1);
    assert_eq!(day_key(today(&e)), "2026-10-23");
    assert_eq!(model.prompts().len(), 2);
}

#[tokio::test]
async fn a_paper_reserves_what_it_can_spend() {
    // Cap 3 with two used: a one-call paper (1 + its retry) fits, a two-call one does not.
    let e = env(1);
    e.counter.record(today(&e), 1).unwrap();
    let r = runner(&e, paper_index(), replies(vec![Ok(CORRECT)]), 3, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    assert_eq!(r.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
    assert_eq!(e.counter.count(today(&e)).unwrap(), 2);

    let big = env(1);
    big.counter.record(today(&big), 1).unwrap();
    let model = replies(vec![Ok(NOTHING_2), Ok(NOTHING_2)]);
    let r = runner(&big, big_index(), model.clone(), 3, "p");
    let job = r.start(&big.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert!(matches!(
        report.outcome,
        JobOutcome::Paused {
            reason: PauseReason::DailyCap,
            ..
        }
    ));
    assert!(model.prompts().is_empty(), "the paper did not start");
}

#[tokio::test]
async fn the_cap_holds_when_other_calls_take_the_room_mid_paper() {
    let e = env(1);
    let cap = 4;
    let day = today(&e);
    // While the paper's first call is out, another table's job takes three calls.
    let mut model = Replies::new(vec![Ok(NOTHING_2), Ok(NOTHING_2)]);
    let counter = e.counter.clone();
    model.before_reply = Some(Box::new(move || {
        counter.record(day, 3).unwrap();
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
    assert_eq!(e.counter.count(day).unwrap(), cap);
    let first = statuses(&e, 0);
    assert_eq!(&first[2..], &[CellStatus::Queued, CellStatus::Queued]);
}

#[tokio::test]
async fn a_counter_gate_counts_a_fill_by_hand_and_refuses_past_the_cap() {
    let e = env(2);
    let gate = CounterGate::new(e.counter.clone(), daily(3)).with_clock(e.clock.clone());
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let f = filler(&e, paper_index(), model.clone());
    let fill = f.fill_paper(&request(&e, 0), &gate).await.unwrap();
    assert_eq!(fill.calls, 1);
    assert_eq!(e.counter.count(today(&e)).unwrap(), 1);
    e.counter.record(today(&e), 1).unwrap();
    let refused = f.fill_paper(&request(&e, 1), &gate).await.unwrap();
    assert_eq!(refused.halted, Some(Halt::DailyCap));
    assert_eq!(refused.calls, 0);
    assert_eq!(model.prompts().len(), 1);
    assert_eq!(statuses(&e, 1), all(CellStatus::Empty));
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
    let mut papers = e.papers.clone();
    papers.push(third);
    e.store.add_rows(&other, &docs(&papers)).unwrap();

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
    let big = runner(&e, big_index(), model.clone(), 30, "p");
    assert_eq!(big.estimate(&other).await.unwrap().calls, 2);
    assert!(model.prompts().is_empty(), "an estimate calls no model");
}

#[tokio::test]
async fn the_estimate_is_what_a_run_spends_on_a_mixed_table() {
    let e = env(4);
    // Paper 0 is answered from the cache: another table asked it the same questions.
    let warm = e.store.create_table(&e.workspace, "Warm").unwrap().id;
    for spec in specs() {
        e.store.add_column(&warm, &spec).unwrap();
    }
    e.store.add_rows(&warm, &docs(&e.papers[..1])).unwrap();
    let warmer = runner(&e, paper_index(), replies(vec![Ok(CORRECT)]), 30, "p");
    let job = warmer.start(&warm, 1).await.unwrap();
    assert_eq!(warmer.run(&job.id).await.unwrap().outcome, JobOutcome::Done);

    // Paper 2's passages need two calls; paper 3 has one cut `not_found` to ask again,
    // its other cells are the user's.
    let index =
        paper_index().with_source(&e.papers[2].to_string_lossy(), None, big_index().passages);
    let hash = paper_hash(&e.papers[3].to_string_lossy()).unwrap();
    let partial = Cell {
        table_id: e.table.clone(),
        row_id: e.rows[3].id.clone(),
        column_id: e.columns[0].id.clone(),
        status: CellStatus::NotFound,
        value: None,
        quote: None,
        chunk_id: None,
        page: None,
        bboxes: None,
        model: Some("recorded-model".to_string()),
        column_revision: Some(e.columns[0].revision),
        paper_hash: Some(hash),
        filled_at: Some("2026-10-22T09:00:00.000Z".to_string()),
        edited_at: None,
        error: Some(NOT_FOUND_PARTIAL.to_string()),
    };
    assert!(e.store.store_fill(&partial, None).unwrap());
    for (column, value) in [(1, "0.5"), (2, "yes"), (3, "Survey")] {
        e.store
            .edit_cell(&e.rows[3].id, &e.columns[column].id, value)
            .unwrap();
    }

    let model = replies(vec![
        Ok(CORRECT),
        Ok(NOTHING_2),
        Ok(NOTHING_2),
        Ok(NOTHING_1),
    ]);
    let r = runner(&e, index, model.clone(), 30, "p");
    let estimate = r.estimate(&e.table).await.unwrap();
    assert_eq!(
        estimate,
        FillEstimate {
            papers: 4,
            columns: 4,
            cells: 13,
            calls: 4,
            cached: 4,
        }
    );
    let job = r.start(&e.table, estimate.calls).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(report.calls, estimate.calls);
    assert_eq!(model.prompts().len(), estimate.calls);
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
    let second = e.store.conn().write(|tx| {
        tx.execute(
            "INSERT INTO table_fill_job(id, table_id, created_at, state, planned_calls, done_calls)
             VALUES ('second', ?1, '2026-10-22T09:00:00.000Z', 'paused', 0, 0)",
            params![e.table],
        )
    });
    assert!(
        matches!(&second, Err(err) if err.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)),
        "the database refuses a second active job: {second:?}"
    );

    // Two runs of one job at once in one process: one runs it, the other is turned away.
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
    assert_eq!(report.outcome, paused(PauseReason::Auth, None));
    assert_eq!(report.calls, 1);
    assert_eq!(statuses(&e, 0), all(CellStatus::Failed));
    assert_eq!(errors(&e, 0), vec![Some(MODEL_AUTH.to_string()); 4]);
    assert_eq!(
        statuses(&e, 1),
        all(CellStatus::Queued),
        "the other papers are not failed for it"
    );
    e.clock.advance(TimeDelta::days(3));
    assert!(r.resume_due().await.unwrap().is_empty());

    let fixed = runner(
        &e,
        paper_index(),
        replies(vec![Ok(CORRECT), Ok(CORRECT)]),
        30,
        "p",
    );
    assert_eq!(fixed.start(&e.table, 2).await.unwrap().id, job.id);
    assert_eq!(fixed.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
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
    assert_eq!(job_of(&e, &job.id).state, JobState::Failed);
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
    assert_eq!(errors(&e, 0), vec![Some(PAPER_NOT_INDEXED.to_string()); 4]);
    assert_eq!(statuses(&e, 1), all(CellStatus::Verified));
}

#[tokio::test]
async fn an_unreachable_model_climbs_the_ladder_and_then_waits_for_the_user() {
    let e = env(2);
    let model = replies(vec![Err(TIMEOUT_ERROR); 5]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    for minutes in [1, 2] {
        let outcome = r.run(&job.id).await.unwrap().outcome;
        let until = e.clock.now() + TimeDelta::minutes(minutes);
        assert_eq!(outcome, paused(PauseReason::Unreachable, Some(until)));
        assert_eq!(errors(&e, 0), vec![Some(MODEL_TIMEOUT.to_string()); 4]);
        e.clock.set(until);
    }
    let outcome = r.run(&job.id).await.unwrap().outcome;
    assert_eq!(outcome, paused(PauseReason::Failing, None));
    // The bound: one call per failing paper, MAX_FAILING_PAPERS in all.
    assert_eq!(model.prompts().len(), MAX_FAILING_PAPERS);
    e.clock.advance(TimeDelta::days(1));
    assert!(r.resume_due().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_model_that_always_fails_stops_after_three_papers() {
    // Refused requests (a wrong model id): one call per paper, three papers.
    let e = env(5);
    let model = replies(vec![Err(BAD_REQUEST); 10]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 5).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, paused(PauseReason::Failing, None));
    assert_eq!(report.calls, MAX_FAILING_PAPERS);
    assert_eq!(errors(&e, 2), vec![Some(MODEL_FAILED.to_string()); 4]);
    assert_eq!(statuses(&e, 3), all(CellStatus::Queued));

    // Replies that never parse: a call and its retry per paper, three papers.
    let e = env(5);
    let model = replies(vec![Ok(UNPARSABLE); 10]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 5).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, paused(PauseReason::Failing, None));
    assert_eq!(report.calls, 2 * MAX_FAILING_PAPERS);
}

#[tokio::test]
async fn one_failing_paper_does_not_stop_the_job() {
    let e = env(2);
    let r = runner(
        &e,
        paper_index(),
        replies(vec![Err(BAD_REQUEST), Ok(CORRECT)]),
        30,
        "p",
    );
    let job = r.start(&e.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!(errors(&e, 0), vec![Some(MODEL_FAILED.to_string()); 4]);
    assert_eq!(statuses(&e, 1), all(CellStatus::Verified));
    assert_eq!(job_of(&e, &job.id).failure_streak, 0);
}

#[tokio::test]
async fn a_failed_refill_keeps_the_stale_results() {
    let e = env(1);
    let r = runner(&e, paper_index(), replies(vec![Ok(CORRECT)]), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    assert_eq!(r.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
    let before = row_cells(&e, 0);
    // The paper changes, so its answers are stale; the model is down for the refill.
    std::fs::write(&e.papers[0], b"%PDF-1.4 paper 0, revised").unwrap();
    let r = runner(
        &e,
        paper_index(),
        replies(vec![Err(TIMEOUT_ERROR)]),
        30,
        "p",
    );
    let job = r.start(&e.table, 1).await.unwrap();
    assert_eq!(
        r.run(&job.id).await.unwrap().outcome,
        paused(
            PauseReason::Unreachable,
            Some(e.clock.now() + TimeDelta::minutes(1))
        )
    );
    let after = row_cells(&e, 0);
    for (old, kept) in before.iter().zip(&after) {
        assert_eq!(kept.status, CellStatus::Verified);
        assert_eq!(kept.value, old.value);
        assert_eq!(kept.quote, old.quote);
        assert_eq!(kept.paper_hash, old.paper_hash);
        assert_eq!(kept.error.as_deref(), Some("refill_failed:model_timeout"));
    }
}

#[tokio::test]
async fn an_edit_during_a_run_is_never_requeued_or_overwritten() {
    let e = env(2);
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

    let next = r.start(&e.table, 0).await.unwrap();
    claimed_by(&e, &next.id, "p");
    assert_eq!(statuses(&e, 0)[3], CellStatus::Edited);
    assert_eq!(statuses(&e, 1)[0], CellStatus::Edited);
    assert_eq!(queued_cells(&e), 0);
}

#[tokio::test]
async fn a_failing_last_write_gives_the_job_back_at_once() {
    let e = env(1);
    let r = runner(&e, paper_index(), replies(vec![Ok(CORRECT)]), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    e.store
        .conn()
        .write(|tx| {
            tx.execute_batch(
                "CREATE TRIGGER refuse_done BEFORE UPDATE OF state ON table_fill_job
                 WHEN NEW.state = 'done' BEGIN SELECT RAISE(ABORT, 'the disk is full'); END;",
            )
        })
        .unwrap();
    assert!(r.run(&job.id).await.is_err());
    assert_eq!(job_of(&e, &job.id).state, JobState::Queued);
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    e.store
        .conn()
        .write(|tx| tx.execute_batch("DROP TRIGGER refuse_done;"))
        .unwrap();
    assert_eq!(r.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
}

#[tokio::test]
async fn a_job_held_here_with_no_live_run_is_resumed_or_cancelled_without_a_restart() {
    let e = env(2);
    let r = runner(&e, paper_index(), replies(vec![Ok(CORRECT)]), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    // Neither the end nor the give-back can be written: the job stays `running` here.
    e.store
        .conn()
        .write(|tx| {
            tx.execute_batch(
                "CREATE TRIGGER stuck BEFORE UPDATE OF state ON table_fill_job
                 WHEN OLD.state = 'running' BEGIN SELECT RAISE(ABORT, 'stuck'); END;",
            )
        })
        .unwrap();
    let model = replies(vec![Ok(CORRECT), Err(BAD_REQUEST)]);
    let stuck = runner(&e, paper_index(), model, 30, "p");
    assert!(stuck.run(&job.id).await.is_err());
    assert_eq!(job_of(&e, &job.id).state, JobState::Running);
    e.store
        .conn()
        .write(|tx| tx.execute_batch("DROP TRIGGER stuck;"))
        .unwrap();

    // No run of it is live here: it is due now, and a cancel takes effect at once.
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    let cancelled = r.cancel(&job.id).await.unwrap().unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
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
async fn a_run_that_keeps_panicking_fails_its_job() {
    let e = env(1);
    let r = runner(&e, paper_index(), Arc::new(Panics), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    assert!(matches!(r.run(&job.id).await, Err(ResearchError::Task(_))));
    let once = job_of(&e, &job.id);
    assert_eq!((once.state, once.panics), (JobState::Queued, 1));
    assert_eq!(statuses(&e, 0), all(CellStatus::Queued));
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);

    assert!(matches!(r.run(&job.id).await, Err(ResearchError::Task(_))));
    let failed = job_of(&e, &job.id);
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.last_error.as_deref(), Some(FILL_PANICKED));
    assert_eq!(statuses(&e, 0), all(CellStatus::Empty));
    assert!(r.resume_due().await.unwrap().is_empty());
    assert_eq!(
        e.counter.count(today(&e)).unwrap(),
        MAX_PANICS,
        "one call per panic, then no more"
    );
}

#[tokio::test]
async fn a_locked_counter_pauses_the_job_for_a_minute() {
    let e = env(1);
    let other = e.dir.path().join("counter.db");
    let counter = Arc::new(CallCounter::open(&other, None).unwrap());
    counter.set_busy_timeout(Duration::from_millis(10));
    let model = replies(vec![Ok(CORRECT)]);
    let r = runner_with(&e, paper_index(), model.clone(), counter, 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();

    let blocker = rusqlite::Connection::open(&other).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(
        report.outcome,
        paused(
            PauseReason::Busy,
            Some(e.clock.now() + TimeDelta::minutes(1))
        )
    );
    assert_eq!(report.calls, 0);
    assert!(model.prompts().is_empty());
    assert_eq!(
        job_of(&e, &job.id).last_error.as_deref(),
        Some(DATABASE_BUSY)
    );
    assert_eq!(statuses(&e, 0), all(CellStatus::Queued));

    blocker.execute_batch("ROLLBACK;").unwrap();
    e.clock.advance(TimeDelta::minutes(1));
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    assert_eq!(r.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
}

#[test]
fn migration_15_keeps_one_active_job_per_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shodh.db");
    drop(crate::audit::open_shared_connection(&path, None).unwrap());
    // A plain connection, as the older app that wrote the database.
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Back to schema 14, with two active jobs for one table (migration 16 runs again
    // after 15 and rebuilds the job table with its rows).
    conn.execute_batch(
        "DROP TABLE table_fill_job;
         DROP TABLE table_calls;
         DROP TABLE table_fill_capture;
         DROP TABLE slack_index_channel;
         DROP TABLE slack_index_thread;
         DROP TABLE slack_index_message;
         DROP TABLE slack_memory;
         DROP TABLE slack_thread_turn;
         DELETE FROM schema_version WHERE version >= 15;
         CREATE TABLE table_fill_job (
            id TEXT PRIMARY KEY,
            table_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN
                ('queued', 'running', 'paused', 'done', 'cancelled', 'failed')),
            planned_calls INTEGER NOT NULL CHECK (planned_calls >= 0),
            done_calls INTEGER NOT NULL CHECK (done_calls >= 0),
            next_attempt_at TEXT NULL,
            last_error TEXT NULL
         );
         CREATE INDEX table_fill_job_table ON table_fill_job(table_id, created_at);
         INSERT INTO table_fill_job VALUES
            ('older', 't1', '2026-10-01T00:00:00.000Z', 'paused', 0, 0, NULL, NULL),
            ('finished', 't1', '2026-10-02T00:00:00.000Z', 'done', 0, 0, NULL, NULL),
            ('newer', 't1', '2026-10-03T00:00:00.000Z', 'queued', 0, 0, NULL, NULL),
            ('alone', 't2', '2026-10-03T00:00:00.000Z', 'running', 0, 0, NULL, NULL);",
    )
    .unwrap();
    drop(conn);
    let conn = crate::audit::open_shared_connection(&path, None).unwrap();
    let states: Vec<(String, String)> = conn
        .read()
        .unwrap()
        .prepare("SELECT id, state FROM table_fill_job ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected: Vec<(String, String)> = [
        ("alone", "running"),
        ("finished", "done"),
        ("newer", "queued"),
        ("older", "cancelled"),
    ]
    .iter()
    .map(|(id, state)| (id.to_string(), state.to_string()))
    .collect();
    assert_eq!(states, expected);
    let duplicate = conn.write(|tx| {
        tx.execute(
            "INSERT INTO table_fill_job(id, table_id, created_at, state, planned_calls, done_calls)
             VALUES ('again', 't1', '2026-10-04T00:00:00.000Z', 'queued', 0, 0)",
            [],
        )
    });
    assert!(
        matches!(&duplicate, Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)),
        "{duplicate:?}"
    );
}

#[test]
fn a_cap_below_a_two_call_paper_is_refused() {
    assert!(matches!(
        DailyCap::new(MIN_DAILY_CAP - 1),
        Err(ResearchError::Invalid(_))
    ));
    let cap = daily(MIN_DAILY_CAP);
    assert!(matches!(
        cap.set(MIN_DAILY_CAP - 1),
        Err(ResearchError::Invalid(_))
    ));
    assert_eq!(cap.get(), MIN_DAILY_CAP);
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
        PauseReason::Failing,
        PauseReason::Busy,
        PauseReason::ModelUnavailable,
    ] {
        assert_eq!(PauseReason::parse(reason.as_str()), Some(reason));
    }
    assert_eq!(JobState::parse("paused-ish"), None);
    assert!(JobState::Paused.is_active());
    assert!(!JobState::Done.is_active());
}

#[tokio::test]
async fn lowering_the_cap_mid_run_pauses_before_the_next_paper() {
    let e = env(2);
    let cap = daily(30);
    // One call was made elsewhere today. While the first paper's call is with the model,
    // the user lowers the cap in Settings to the smallest one.
    e.counter.record(today(&e), 1).unwrap();
    let mut model = Replies::new(vec![Ok(CORRECT), Ok(CORRECT)]);
    let lowered = cap.clone();
    model.before_reply = Some(Box::new(move || lowered.set(MIN_DAILY_CAP).unwrap()));
    let model = Arc::new(model);
    let r = JobRunner::new(
        filler(&e, paper_index(), model.clone()),
        e.counter.clone(),
        Arc::new(NoResults),
        cap.clone(),
    )
    .with_clock(e.clock.clone())
    .with_token("p");
    let job = r.start(&e.table, 2).await.unwrap();

    let report = r.run(&job.id).await.unwrap();
    // The second paper needs its call and the retry: 2 + 2 > 3.
    assert_eq!(
        report.outcome,
        paused(PauseReason::DailyCap, Some(utc(2026, 10, 23, 0, 0)))
    );
    assert_eq!(report.calls, 1);
    assert_eq!(model.prompts().len(), 1, "the second paper did not start");
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
    assert_eq!(statuses(&e, 1), all(CellStatus::Queued));
    assert_eq!(e.counter.count(today(&e)).unwrap(), 2);
}

#[tokio::test]
async fn a_quota_pause_reports_which_limit_it_was() {
    let now = utc(2099, 1, 1, 12, 0);
    for (error, window, stored) in [
        (DAILY_LIMIT, LimitWindow::Daily, "model_quota:daily"),
        (UPSTREAM_LIMIT, LimitWindow::Short, "model_quota:short"),
        (
            "HTTP 429 Too Many Requests: rate limit, retry after 20 seconds",
            LimitWindow::Unknown,
            "model_quota:unknown",
        ),
    ] {
        let e = env_at(1, now);
        let r = runner(&e, paper_index(), replies(vec![Err(error)]), 30, "p");
        let job = r.start(&e.table, 1).await.unwrap();
        let report = r.run(&job.id).await.unwrap();
        assert!(matches!(
            report.outcome,
            JobOutcome::Paused {
                reason: PauseReason::Quota,
                ..
            }
        ));
        assert_eq!(report.quota_window, Some(window));
        // The pause keeps which limit it was, for the interface to word.
        assert_eq!(job_of(&e, &job.id).last_error.as_deref(), Some(stored));
    }
    let e = env(1);
    let r = runner(&e, paper_index(), replies(vec![Err(AUTH_ERROR)]), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    assert_eq!(r.run(&job.id).await.unwrap().quota_window, None);
}

#[tokio::test]
async fn the_latest_job_outlives_its_end_and_a_job_is_cancelled_without_a_runner() {
    let e = env(2);
    assert!(e.store.latest_fill_job(&e.table).unwrap().is_none());
    let r = runner(&e, paper_index(), replies(vec![Err(AUTH_ERROR)]), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    r.run(&job.id).await.unwrap();
    assert_eq!(job_of(&e, &job.id).state, JobState::Paused);

    let cancelled = cancel_job(e.store.clone(), &job.id).await.unwrap().unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert_eq!(statuses(&e, 1), all(CellStatus::Empty));
    assert!(e.store.active_fill_job(&e.table).unwrap().is_none());
    assert_eq!(
        e.store.latest_fill_job(&e.table).unwrap().unwrap().id,
        job.id
    );
    let next = r.start(&e.table, 1).await.unwrap();
    assert_eq!(
        e.store.latest_fill_job(&e.table).unwrap().unwrap().id,
        next.id,
        "the newest job wins, also within one millisecond"
    );
    // A scheduler finds the queued job without a runner.
    assert_eq!(
        resume_due_jobs(e.store.clone()).await.unwrap(),
        vec![next.id.clone()]
    );
    assert!(cancel_job(e.store.clone(), "no-such-job")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_job_held_by_a_process_that_quit_is_given_back_and_taken_up_at_once() {
    let e = env(2);
    let r = runner(&e, paper_index(), replies(vec![]), 30, "process-2");
    let job = r.start(&e.table, 2).await.unwrap();
    // Process 1 holds the job under a live lease.
    claimed_by(&e, &job.id, "process-1");
    let until = e.clock.now() + TimeDelta::minutes(LEASE_MINUTES);
    assert_eq!(r.hold(&job.id).await.unwrap(), Hold::Elsewhere { until });
    assert!(
        r.resume_due().await.unwrap().is_empty(),
        "not due under a live lease"
    );
    // A cancel asked meanwhile waits for the lease.
    let pending = r.cancel(&job.id).await.unwrap().unwrap();
    assert_eq!(pending.state, JobState::Running);
    assert!(pending.cancel_requested);

    // Process 1 quits cleanly and gives its jobs back.
    assert_eq!(e.store.release_fill_jobs_held_by("process-1").unwrap(), 1);
    assert_eq!(e.store.release_fill_jobs_held_by("process-1").unwrap(), 0);
    let released = job_of(&e, &job.id);
    assert_eq!(released.state, JobState::Queued);
    assert_eq!(released.heartbeat_at, None);
    assert_eq!(
        statuses(&e, 0),
        all(CellStatus::Queued),
        "queued cells stay"
    );
    assert_eq!(r.hold(&job.id).await.unwrap(), Hold::Free);
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
    // The next claim finishes the cancel that was asked.
    assert_eq!(
        r.run(&job.id).await.unwrap().outcome,
        JobOutcome::NotClaimed {
            state: Some(JobState::Cancelled)
        }
    );

    // Without a cancel, the next start runs it at once.
    let job = r.start(&e.table, 2).await.unwrap();
    claimed_by(&e, &job.id, "process-1");
    e.store.release_fill_jobs_held_by("process-1").unwrap();
    let fresh = runner(
        &e,
        paper_index(),
        replies(vec![Ok(CORRECT), Ok(CORRECT)]),
        30,
        "process-2",
    );
    assert_eq!(fresh.run(&job.id).await.unwrap().outcome, JobOutcome::Done);
    assert_eq!(fresh.hold(&job.id).await.unwrap(), Hold::Free);
}

#[tokio::test]
async fn a_job_that_cannot_get_a_model_waits_for_the_user_and_keeps_its_queue() {
    let e = env(2);
    let r = runner(&e, paper_index(), replies(vec![]), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    claimed_by(&e, &job.id, "gone");
    e.clock.advance(TimeDelta::minutes(LEASE_MINUTES));
    let blocked = r
        .block(&job.id, "refused_reasoning")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(blocked.state, JobState::Paused);
    assert_eq!(blocked.pause_reason, Some(PauseReason::ModelUnavailable));
    assert_eq!(blocked.last_error.as_deref(), Some("refused_reasoning"));
    assert_eq!(blocked.next_attempt_at, None);
    assert_eq!(statuses(&e, 0), all(CellStatus::Queued));
    assert!(r.resume_due().await.unwrap().is_empty());
    // A live run elsewhere is never paused from here.
    let other = env(1);
    let o = runner(&other, paper_index(), replies(vec![]), 30, "p");
    let live = o.start(&other.table, 1).await.unwrap();
    claimed_by(&other, &live.id, "elsewhere");
    let kept = o.block(&live.id, "no_key").await.unwrap().unwrap();
    assert_eq!(kept.state, JobState::Running);
    // Fixing the model makes it due again.
    let woken = e
        .store
        .wake_paused_fill_jobs(PauseReason::ModelUnavailable, &timestamp(e.clock.now()))
        .unwrap();
    assert_eq!(woken.len(), 1);
    assert_eq!(r.resume_due().await.unwrap(), vec![job.id.clone()]);
}

/// What a scheduler told the app.
#[derive(Default)]
struct Seen {
    changed: Mutex<Vec<String>>,
    ended: Mutex<Vec<(String, JobOutcome, bool)>>,
    blocked: Mutex<Vec<(String, Option<String>)>>,
}

impl crate::research::tables::schedule::RunEvents for Seen {
    fn changed(&self, table_id: &str) {
        self.changed.lock().unwrap().push(table_id.to_string());
    }
    fn ended(&self, job: &FillJob, report: &RunReport, by_user: bool) {
        self.ended
            .lock()
            .unwrap()
            .push((job.id.clone(), report.outcome.clone(), by_user));
    }
    fn blocked(&self, job: &FillJob, _message: &str) {
        self.blocked
            .lock()
            .unwrap()
            .push((job.id.clone(), job.last_error.clone()));
    }
}

/// A runner source that gives `runner`, or says no model can be used.
struct Source {
    runner: Mutex<Option<JobRunner>>,
    asked: Mutex<usize>,
}

impl Source {
    fn blocked() -> Self {
        Self {
            runner: Mutex::new(None),
            asked: Mutex::new(0),
        }
    }
    fn with(runner: JobRunner) -> Self {
        Self {
            runner: Mutex::new(Some(runner)),
            asked: Mutex::new(0),
        }
    }
    fn asked(&self) -> usize {
        *self.asked.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl crate::research::tables::schedule::RunnerSource for Source {
    async fn runner(&self) -> Result<JobRunner, crate::research::tables::schedule::RunnerError> {
        *self.asked.lock().unwrap() += 1;
        self.runner.lock().unwrap().clone().ok_or_else(|| {
            crate::research::tables::schedule::RunnerError::Blocked {
                code: "refused_reasoning",
                message: "this model always reasons".to_string(),
            }
        })
    }
}

async fn finish(tick: crate::research::tables::schedule::Tick) {
    for run in tick.runs {
        run.await.unwrap();
    }
}

#[tokio::test]
async fn a_check_with_nothing_due_resolves_no_model() {
    use crate::research::tables::schedule::Scheduler;
    let e = env(1);
    let scheduler = Scheduler::with("p", e.clock.clone());
    let source = Source::blocked();
    let seen = Arc::new(Seen::default());
    let tick = scheduler
        .tick(e.store.clone(), &source, seen.clone())
        .await
        .unwrap();
    assert_eq!((tick.due, tick.asked), (0, false));
    assert_eq!(source.asked(), 0);
}

#[tokio::test]
async fn a_job_is_run_once_however_often_it_is_started() {
    use crate::research::tables::schedule::Scheduler;
    let e = env(1);
    let model = replies(vec![Ok(CORRECT), Ok(CORRECT)]);
    let r = runner(&e, paper_index(), model.clone(), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    let scheduler = Scheduler::with("p", e.clock.clone());
    let seen = Arc::new(Seen::default());
    // The user's Fill runs it at once; a second start while it runs adds nothing.
    let first = scheduler
        .spawn_run(r.clone(), job.id.clone(), seen.clone(), true)
        .unwrap();
    assert!(scheduler.is_running(&job.id));
    assert!(scheduler
        .spawn_run(r.clone(), job.id.clone(), seen.clone(), true)
        .is_none());
    first.await.unwrap();
    assert!(!scheduler.is_running(&job.id));
    assert_eq!(model.prompts().len(), 1, "one paper, one call");
    assert_eq!(
        seen.ended.lock().unwrap().as_slice(),
        &[(job.id.clone(), JobOutcome::Done, true)]
    );
    assert_eq!(seen.changed.lock().unwrap().as_slice(), &[e.table.clone()]);
}

#[tokio::test]
async fn a_blocked_model_pauses_due_jobs_once_and_a_fixed_one_resumes_them() {
    use crate::research::tables::schedule::Scheduler;
    let e = env(1);
    let r = runner(&e, paper_index(), replies(vec![Ok(CORRECT)]), 30, "p");
    let job = r.start(&e.table, 1).await.unwrap();
    let scheduler = Scheduler::with("p", e.clock.clone());
    let seen = Arc::new(Seen::default());

    let blocked = Source::blocked();
    let tick = scheduler
        .tick(e.store.clone(), &blocked, seen.clone())
        .await
        .unwrap();
    assert_eq!(tick.blocked, vec![job.id.clone()]);
    let paused = job_of(&e, &job.id);
    assert_eq!(paused.pause_reason, Some(PauseReason::ModelUnavailable));
    assert_eq!(paused.next_attempt_at, None);
    // Later checks look again (the user may fix it) but say nothing new.
    for _ in 0..2 {
        let tick = scheduler
            .tick(e.store.clone(), &blocked, seen.clone())
            .await
            .unwrap();
        assert!(tick.blocked.is_empty());
    }
    assert_eq!(blocked.asked(), 3);
    assert_eq!(
        seen.blocked.lock().unwrap().as_slice(),
        &[(job.id.clone(), Some("refused_reasoning".to_string()))],
        "told once"
    );

    // The user chooses a model that tables can use: the next check runs the job.
    let fixed = Source::with(r.clone());
    let tick = scheduler
        .tick(e.store.clone(), &fixed, seen.clone())
        .await
        .unwrap();
    assert_eq!(tick.woken, 1);
    finish(tick).await;
    assert_eq!(job_of(&e, &job.id).state, JobState::Done);
    assert_eq!(statuses(&e, 0), all(CellStatus::Verified));
}

#[tokio::test]
async fn raising_the_cap_wakes_a_job_paused_for_it() {
    use crate::research::tables::schedule::Scheduler;
    let e = env(1);
    let cap = daily(MIN_DAILY_CAP);
    e.counter.record(today(&e), 2).unwrap();
    let model = replies(vec![Ok(CORRECT)]);
    let r = JobRunner::new(
        filler(&e, paper_index(), model.clone()),
        e.counter.clone(),
        Arc::new(NoResults),
        cap.clone(),
    )
    .with_clock(e.clock.clone())
    .with_token("p");
    let job = r.start(&e.table, 1).await.unwrap();
    assert!(matches!(
        r.run(&job.id).await.unwrap().outcome,
        JobOutcome::Paused {
            reason: PauseReason::DailyCap,
            ..
        }
    ));
    let scheduler = Scheduler::with("p", e.clock.clone());
    let source = Source::with(r.clone());
    let seen = Arc::new(Seen::default());
    assert_eq!(
        scheduler
            .tick(e.store.clone(), &source, seen.clone())
            .await
            .unwrap()
            .due,
        0,
        "not due before midnight"
    );
    // The user raises the cap: the paused job is due at once and runs.
    cap.set(30).unwrap();
    let woken = e
        .store
        .wake_paused_fill_jobs(PauseReason::DailyCap, &timestamp(e.clock.now()))
        .unwrap();
    assert_eq!(woken.len(), 1);
    let tick = scheduler
        .tick(e.store.clone(), &source, seen.clone())
        .await
        .unwrap();
    assert_eq!(tick.due, 1);
    finish(tick).await;
    assert_eq!(job_of(&e, &job.id).state, JobState::Done);
    assert_eq!(model.prompts().len(), 1);
}

#[tokio::test]
async fn a_run_counts_the_papers_it_took_up_and_those_not_indexed() {
    let e = env(3);
    let index = paper_index().without(&e.papers[1].to_string_lossy());
    let r = runner(&e, index, replies(vec![Ok(CORRECT), Ok(CORRECT)]), 30, "p");
    let job = r.start(&e.table, 2).await.unwrap();
    let report = r.run(&job.id).await.unwrap();
    assert_eq!(report.outcome, JobOutcome::Done);
    assert_eq!((report.papers, report.not_indexed), (3, 1));
}
