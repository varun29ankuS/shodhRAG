//! Paper alerts: a topic the user follows, checked on arXiv, OpenAlex and Semantic
//! Scholar when it is turned on (over the last 14 days) and then every Monday at 09:00
//! (a `paper_alert` engine job). New papers go to Library → What's new; a check that
//! found some posts one Info item, and one that found none posts nothing.
//!
//! - **Consent.** Turning one on shows the consent line ([`consent_line`]): the exact
//!   words sent, and to whom. Nothing else is sent: no email, name or key.
//! - **Local-only mode, or web access off, sends nothing.** Creating or rewording an
//!   alert is refused, and every check looks at the policy first: a blocked check is
//!   recorded as skipped (never failed, so it never puts the alert on hold) and asks
//!   nothing.
//! - **Each API fails alone.** The papers of those that answered are posted at once;
//!   the others are asked again an hour later (or when they said to come back), at most
//!   three times, and then the check ends with what it found. A check fails only when no
//!   API answered for a reason other than the connection; three failed weeks in a row
//!   put the alert on hold with an Attention item (its fix: Resume). Being offline only
//!   skips a week.
//! - A check after the computer woke waits two minutes first, for the network.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDate, NaiveTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shodh_rag::engine::events::EventTrigger;
use shodh_rag::engine::runner::{Engine, Handled, JobHandler};
use shodh_rag::engine::schedule::{Schedule, WeekDay};
use shodh_rag::engine::store::{
    parse_instant, CatchUp, CreatedBy, EngineStore, HoldReason, Job, JobKind, JobPatch, NewJob,
    Overlap, Run, RunState, Trigger, WaitReason,
};
use shodh_rag::harness::web::papers::{title_key, Paper, PaperApi};
use shodh_rag::harness::web::relevance::SharedScorer;
use shodh_rag::harness::web::SafeClient;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use shodh_rag::whats_new::alerts::{
    check_alert, clean_words, consent_line, windows, ApiOutcome, Failure, MAX_RETRY_AFTER_SECS,
};
use shodh_rag::whats_new::library::known_paper;
use shodh_rag::whats_new::pace::ApiPacer;
use shodh_rag::whats_new::store::{FeedItem, FeedKind, FeedStore, NewFeedItem};
use shodh_rag::whats_new::topics::{normalize_topic, suggest_topics, TopicInputs, TopicSuggestion};

use crate::whats_new::WhatsNewSink;

/// Most paper alerts at once.
pub const MAX_ALERTS: usize = 10;
/// Retries of the APIs that did not answer, per check.
pub const MAX_API_RETRIES: u32 = 3;
/// The Inbox id of a check's item: `<prefix><job>:<run>`.
pub const PAPERS_ITEM_PREFIX: &str = "whats_new:papers:";
/// The Inbox id of an alert on hold: `<prefix><job>`.
pub const ALERT_HOLD_PREFIX: &str = "paper-alert-hold:";
/// Error code of a wait after the computer woke.
const AFTER_WAKE: &str = "after_wake";
const API_UNAVAILABLE: &str = "api_unavailable";
const OFFLINE: &str = "offline";

/// What a paper alert needs from the app.
#[async_trait]
pub trait AlertEnv: Send + Sync {
    /// Why nothing may be sent now (Local-only mode, web access off), if so.
    fn blocked(&self) -> Option<String>;
    /// The web client the APIs are asked through.
    fn client(&self) -> SafeClient;
    /// The papers already in the library.
    async fn library_papers(&self) -> Vec<Paper>;
    /// The local cross-encoder, when installed.
    async fn scorer(&self) -> Option<SharedScorer>;
    /// What the library offers to suggest topics from.
    async fn topic_inputs(&self) -> TopicInputs;
}

/// The consent the user gave: the line they saw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Consent {
    text: String,
    at: String,
}

/// How far the current check got (kept across its retries).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    run_id: String,
    /// APIs that answered in this check.
    apis: BTreeSet<PaperApi>,
    retries: u32,
    /// Papers found in this check.
    found: usize,
    /// APIs that did not answer, and why.
    failed: BTreeMap<PaperApi, String>,
    /// When the next try is.
    retry_at: Option<String>,
}

/// An alert's job input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlertInput {
    words: String,
    consent: Consent,
    #[serde(default)]
    checked_through: BTreeMap<PaperApi, NaiveDate>,
    #[serde(default)]
    progress: Option<Progress>,
}

fn input_of(job: &Job) -> AlertInput {
    serde_json::from_value(job.input.clone()).unwrap_or_default()
}

fn names(apis: impl IntoIterator<Item = PaperApi>) -> String {
    let names: Vec<&str> = apis.into_iter().map(PaperApi::name).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn papers_title(count: usize, words: &str) -> String {
    if count == 1 {
        format!("1 new paper for “{words}”")
    } else {
        format!("{count} new papers for “{words}”")
    }
}

fn found_summary(count: usize) -> String {
    match count {
        0 => "Nothing new".to_string(),
        1 => "Found 1 new paper".to_string(),
        n => format!("Found {n} new papers"),
    }
}

fn local_clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

/// The schedule of every alert: Mondays at 09:00.
pub fn weekly() -> Schedule {
    Schedule::Weekly {
        days: vec![WeekDay::Mon],
        at: NaiveTime::from_hms_opt(9, 0, 0).expect("09:00"),
    }
}

/// A paper's row key, per alert: two alerts that find one paper each keep their own
/// row, so deleting one alert never hides a paper the other found.
fn paper_ref(job_id: &str, p: &Paper) -> String {
    let id = if let Some(doi) = &p.doi {
        format!("doi:{doi}")
    } else if let Some(id) = &p.arxiv_id {
        format!("arxiv:{id}")
    } else {
        format!("title:{}", title_key(&p.title))
    };
    format!("{job_id}|{id}")
}

/// A paper an alert found, as an automation's agent is given it: what the APIs said of
/// it (title, authors, year, venue, link, the start of its abstract), nothing else.
fn paper_subject(row: &FeedItem) -> Value {
    json!({
        "kind": "paper",
        "title": row.title,
        "authors": row.data["authors"],
        "year": row.data["year"],
        "venue": row.data["venue"],
        "doi": row.data["doi"],
        "url": row.data["landingUrl"],
        "abstract": row.data["abstract"],
    })
}

/// A paper found before, as the comparison reads it.
fn found_paper(row: &FeedItem) -> Paper {
    let text = |k: &str| row.data.get(k).and_then(Value::as_str);
    known_paper(&row.title, text("doi"), text("arxivId"))
}

/// The state an alert is in, as the section words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertState {
    Checking,
    WaitingRetry,
    Offline,
    Active,
    Off,
    Held,
    Blocked,
}

/// A paper alert, for the section and the assistant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertView {
    pub id: String,
    pub words: String,
    pub consent_text: String,
    pub state: AlertState,
    pub last_checked_at: Option<String>,
    pub last_summary: Option<String>,
    pub next_check_at: Option<String>,
    /// APIs that did not answer in the current check.
    pub failed_apis: Vec<String>,
    pub retry_at: Option<String>,
    pub blocked_reason: Option<String>,
}

struct Core {
    engine: Arc<Engine>,
    feed: Arc<FeedStore>,
    env: Arc<dyn AlertEnv>,
    sink: Arc<dyn WhatsNewSink>,
    pacer: &'static ApiPacer,
}

impl Core {
    fn store(&self) -> &Arc<EngineStore> {
        self.engine.store()
    }

    fn save_input(&self, job: &Job, input: &AlertInput) -> Result<(), String> {
        let value = serde_json::to_value(input).map_err(|e| e.to_string())?;
        self.store()
            .update_job(
                &job.id,
                &JobPatch {
                    input: Some(value),
                    ..JobPatch::default()
                },
                self.engine.now(),
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The papers `job` must not offer as new: the library's (for every alert) and those
    /// this alert has found or set aside before.
    async fn known_papers(&self, job: &Job) -> Result<Vec<Paper>, String> {
        let mut known = self.env.library_papers().await;
        let found = self.feed.papers().map_err(|e| e.to_string())?;
        known.extend(found.iter().filter(|r| r.job_id == job.id).map(found_paper));
        Ok(known)
    }

    /// Saves what a check of `job` (for `words`) learned: re-reads the job, so words or
    /// a consent the user changed meanwhile are never written back. When the words
    /// changed, nothing is saved: the dates and progress were for the old words.
    fn save_progress(
        &self,
        job: &Job,
        words: &str,
        checked_through: &BTreeMap<PaperApi, NaiveDate>,
        progress: Option<&Progress>,
    ) -> Result<(), String> {
        let current = self
            .store()
            .job(&job.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "The alert is gone.".to_string())?;
        let mut input = input_of(&current);
        if input.words != words {
            return Ok(());
        }
        input.checked_through = checked_through.clone();
        input.progress = progress.cloned();
        self.save_input(&current, &input)
    }

    fn rows_of(&self, job: &Job, words: &str, papers: &[Paper]) -> Vec<NewFeedItem> {
        papers
            .iter()
            .map(|p| {
                let mut detail: Vec<String> = Vec::new();
                if let Some(year) = p.year {
                    detail.push(year.to_string());
                }
                detail.push(p.sources.join(", "));
                NewFeedItem {
                    kind: FeedKind::Paper,
                    source_ref: paper_ref(&job.id, p),
                    job_id: job.id.clone(),
                    title: p.title.clone(),
                    detail: Some(detail.join(" · ")),
                    link: json!({ "view": "library", "target": { "kind": "library", "section": "whats_new" } }),
                    data: json!({
                        "doi": p.doi,
                        "arxivId": p.arxiv_id,
                        "titleKey": title_key(&p.title),
                        "year": p.year,
                        "authors": p.authors,
                        "venue": p.venue,
                        "abstract": p.abstract_snippet,
                        "pdfUrl": p.pdf_url,
                        "landingUrl": p.link(),
                        "sources": p.sources,
                        "relevance": p.relevance,
                        "alertId": job.id,
                        "words": words,
                    }),
                }
            })
            .collect()
    }

    /// Posts (or updates) the check's item and marks what it tells of.
    async fn post(&self, job: &Job, run: &Run, words: &str, progress: &Progress, retrying: bool) {
        if progress.found == 0 {
            return;
        }
        let answered = names(progress.apis.iter().copied());
        let mut detail = if answered.is_empty() {
            String::new()
        } else {
            format!("From {answered}.")
        };
        if !progress.failed.is_empty() {
            let missing = names(progress.failed.keys().copied());
            let when = progress
                .retry_at
                .as_deref()
                .and_then(parse_instant)
                .filter(|_| retrying)
                .map(|at| format!("; Shodh tries again at {}", local_clock(at)))
                .unwrap_or_else(|| " this week".to_string());
            detail.push_str(&format!(" {missing} didn't answer{when}."));
        }
        self.sink
            .post(NewInboxItem {
                id: format!("{PAPERS_ITEM_PREFIX}{}:{}", job.id, run.id),
                kind: InboxKind::WhatsNew,
                status: InboxStatus::Done,
                title: papers_title(progress.found, words),
                detail: Some(detail.trim().to_string()),
                link: Some(InboxLink::new(
                    "library",
                    Some(json!({ "kind": "library", "section": "whats_new" })),
                )),
                data: json!({ "jobId": job.id, "count": progress.found, "announce": false }),
            })
            .await;
        if let Ok(rows) = self.feed.unposted(&job.id) {
            let ids: Vec<String> = rows.into_iter().map(|r| r.id).collect();
            if let Err(e) = self.feed.mark_posted(&ids, &run.id) {
                tracing::warn!(job = %job.id, error = %e, "found papers not marked as told");
            }
        }
    }

    async fn check(&self, job: &Job, run: &Run) -> Handled {
        let mut input = input_of(job);
        if input.words.is_empty() {
            return Handled::Permanent {
                code: "no_words".into(),
                detail: "The alert has no words to look for.".into(),
            };
        }
        let now = self.engine.now();
        let mut progress = match input.progress.take() {
            Some(p) if p.run_id == run.id => p,
            _ => {
                let fresh = Progress {
                    run_id: run.id.clone(),
                    ..Progress::default()
                };
                if run.trigger == Trigger::CatchUp {
                    // The computer just woke: give the network a moment.
                    let at = now + TimeDelta::minutes(2);
                    let waiting = Progress {
                        retry_at: Some(at.to_rfc3339()),
                        ..fresh
                    };
                    if let Err(e) = self.save_progress(
                        job,
                        &input.words,
                        &input.checked_through,
                        Some(&waiting),
                    ) {
                        return Handled::Transient {
                            code: "alert_unsaved".into(),
                            detail: e,
                        };
                    }
                    return Handled::RetryAt {
                        at,
                        reason: WaitReason::Backoff,
                        code: AFTER_WAKE.into(),
                        detail: "Checking once the computer has been awake for two minutes.".into(),
                    };
                }
                fresh
            }
        };
        let apis: BTreeSet<PaperApi> = PaperApi::ALL
            .into_iter()
            .filter(|a| !progress.apis.contains(a))
            .collect();
        let known = match self.known_papers(job).await {
            Ok(known) => known,
            Err(e) => {
                return Handled::Transient {
                    code: "feed_unavailable".into(),
                    detail: e,
                }
            }
        };
        let since = windows(&input.checked_through, now);
        let found = check_alert(
            &self.env.client(),
            self.pacer,
            &input.words,
            &since,
            &apis,
            now,
            &known,
            self.env.scorer().await,
        )
        .await;
        // Papers that cannot be stored are not counted as asked: the retry asks again,
        // and the retry is counted toward the three.
        let unstored = |e: String, progress: &mut Progress| {
            progress.retries += 1;
            if let Err(e) =
                self.save_progress(job, &input.words, &input.checked_through, Some(progress))
            {
                tracing::warn!(job = %job.id, error = %e, "a paper alert's progress was not saved");
            }
            Handled::Transient {
                code: "feed_unavailable".into(),
                detail: e,
            }
        };
        let rows = self.rows_of(job, &input.words, &found.new);
        match self.feed.insert_if_absent(&rows, now) {
            Ok(inserted) => {
                progress.found += inserted.len();
                if !inserted.is_empty() {
                    // The automations this alert starts get the papers, once: only the
                    // ones it had not found before.
                    self.sink
                        .found(
                            EventTrigger::PaperFound {
                                alert_id: job.id.clone(),
                            },
                            inserted.iter().map(paper_subject).collect(),
                        )
                        .await;
                }
            }
            Err(e) => return unstored(e.to_string(), &mut progress),
        }
        // Weak matches and those past the best few are kept as seen (dismissed, never
        // listed), so a later check whose answers rank them higher does not offer them
        // as new.
        let mut set_aside = self.rows_of(job, &input.words, &found.set_aside);
        for row in &mut set_aside {
            row.data["setAside"] = json!(true);
        }
        match self.feed.insert_if_absent(&set_aside, now) {
            Ok(inserted) => {
                for row in inserted {
                    if let Err(e) = self.feed.set_dismissed(&row.id, true, now) {
                        tracing::warn!(error = %e, "a paper was not set aside");
                    }
                }
            }
            Err(e) => return unstored(e.to_string(), &mut progress),
        }
        let mut wait_secs: i64 = 3600;
        let mut all_offline = true;
        for (api, outcome) in &found.outcomes {
            match outcome {
                ApiOutcome::Answered { full, oldest, .. } => {
                    progress.apis.insert(*api);
                    progress.failed.remove(api);
                    // A full page may leave older papers of the window out: the window
                    // is covered only back to the oldest paper it held.
                    let through = if !*full {
                        Some(now.date_naive())
                    } else {
                        let before = input.checked_through.get(api).copied();
                        match (before, *oldest) {
                            (Some(before), Some(oldest)) => Some(before.max(oldest)),
                            (None, Some(oldest)) => Some(oldest),
                            (before, None) => before,
                        }
                    };
                    if let Some(day) = through {
                        input.checked_through.insert(*api, day);
                    }
                }
                ApiOutcome::Failed { failure, detail } => {
                    progress.failed.insert(*api, detail.clone());
                    if !matches!(failure, Failure::Offline) {
                        all_offline = false;
                    }
                    if let Some(secs) = failure.retry_after_secs() {
                        let secs = secs.min(MAX_RETRY_AFTER_SECS);
                        wait_secs = wait_secs.max(i64::try_from(secs).unwrap_or(i64::MAX));
                    }
                }
            }
        }
        let some_answered = !progress.apis.is_empty();
        let can_retry = progress.retries < MAX_API_RETRIES;
        let handled = if progress.failed.is_empty() {
            Handled::Succeeded {
                summary: found_summary(progress.found),
                scheduled_for: None,
            }
        } else if can_retry {
            progress.retries += 1;
            let offline = !some_answered && all_offline;
            let wait = if offline {
                TimeDelta::minutes(15)
            } else {
                TimeDelta::try_seconds(wait_secs).unwrap_or(TimeDelta::hours(24))
            };
            let at = now
                .checked_add_signed(wait)
                .unwrap_or(now + TimeDelta::hours(24));
            progress.retry_at = Some(at.to_rfc3339());
            Handled::RetryAt {
                at,
                reason: if offline {
                    WaitReason::Offline
                } else {
                    WaitReason::Backoff
                },
                code: if offline { OFFLINE } else { API_UNAVAILABLE }.into(),
                detail: format!("{} didn't answer", names(progress.failed.keys().copied())),
            }
        } else if some_answered {
            Handled::Succeeded {
                summary: format!(
                    "{}; {} didn't answer",
                    found_summary(progress.found),
                    names(progress.failed.keys().copied())
                ),
                scheduled_for: None,
            }
        } else if all_offline {
            Handled::Skipped {
                summary: "Not checked: no internet connection".into(),
            }
        } else {
            Handled::Permanent {
                code: API_UNAVAILABLE.into(),
                detail: format!("{} didn't answer", names(progress.failed.keys().copied())),
            }
        };
        let retrying = matches!(handled, Handled::RetryAt { .. });
        self.post(job, run, &input.words, &progress, retrying).await;
        let kept = retrying.then_some(&progress);
        if let Err(e) = self.save_progress(job, &input.words, &input.checked_through, kept) {
            tracing::warn!(job = %job.id, error = %e, "a paper alert's progress was not saved");
        }
        if let Err(e) = self.feed.prune(now) {
            tracing::warn!(error = %e, "What's new could not be pruned");
        }
        self.sink.changed();
        handled
    }
}

#[async_trait]
impl JobHandler for Core {
    fn kind(&self) -> JobKind {
        JobKind::PaperAlert
    }

    async fn run(&self, job: &Job, run: &Run) -> Handled {
        if let Some(reason) = self.env.blocked() {
            // Nothing is sent, and a week skipped never puts the alert on hold.
            let why = if reason.starts_with("Local-only") {
                "Local-only mode is on".to_string()
            } else {
                "web access is off".to_string()
            };
            self.sink.changed();
            return Handled::Skipped {
                summary: format!("Not checked: {why}."),
            };
        }
        self.check(job, run).await
    }

    /// A paper alert has no waiting decision to fold.
    async fn folded(&self, _job: &Job, _run: &Run) {}

    async fn held(&self, job: &Job, _run: &Run) {
        let input = input_of(job);
        self.sink
            .post(NewInboxItem {
                id: format!("{ALERT_HOLD_PREFIX}{}", job.id),
                kind: InboxKind::Automation,
                status: InboxStatus::NeedsYou,
                title: format!("The alert for “{}” stopped", input.words),
                detail: Some(
                    "arXiv, OpenAlex and Semantic Scholar failed 3 weeks in a row. Resume it to try again."
                        .into(),
                ),
                link: Some(InboxLink::new(
                    "library",
                    Some(json!({ "kind": "library", "section": "whats_new", "item": format!("alert:{}", job.id) })),
                )),
                data: json!({ "jobId": job.id, "fix": "resume", "resume": "paper_alert" }),
            })
            .await;
        self.sink.changed();
    }
}

/// Paper alerts over the engine and the feed.
pub struct PaperAlerts {
    core: Arc<Core>,
    handler: Arc<dyn JobHandler>,
}

impl std::fmt::Debug for PaperAlerts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaperAlerts").finish_non_exhaustive()
    }
}

impl PaperAlerts {
    pub fn new(
        engine: Arc<Engine>,
        feed: Arc<FeedStore>,
        env: Arc<dyn AlertEnv>,
        sink: Arc<dyn WhatsNewSink>,
    ) -> Self {
        Self::with_pacer(engine, feed, env, sink, ApiPacer::shared())
    }

    fn with_pacer(
        engine: Arc<Engine>,
        feed: Arc<FeedStore>,
        env: Arc<dyn AlertEnv>,
        sink: Arc<dyn WhatsNewSink>,
        pacer: &'static ApiPacer,
    ) -> Self {
        let core = Arc::new(Core {
            engine,
            feed,
            env,
            sink,
            pacer,
        });
        let handler: Arc<dyn JobHandler> = core.clone();
        Self { core, handler }
    }

    /// The handler of paper alerts, for the engine's loop.
    pub fn handler(&self) -> Arc<dyn JobHandler> {
        self.handler.clone()
    }

    fn store(&self) -> &Arc<EngineStore> {
        self.core.store()
    }

    fn jobs(&self) -> Result<Vec<Job>, String> {
        self.store()
            .jobs(JobKind::PaperAlert)
            .map_err(|e| e.to_string())
    }

    fn alert(&self, id: &str) -> Result<Job, String> {
        self.store()
            .job(id)
            .map_err(|e| e.to_string())?
            .filter(|j| j.kind == JobKind::PaperAlert)
            .ok_or_else(|| format!("There is no paper alert {id}."))
    }

    /// Why alerts can send nothing now, if so.
    pub fn blocked(&self) -> Option<String> {
        self.core.env.blocked()
    }

    /// Makes an alert for `words` (refused while nothing may be sent); its first check
    /// is [`Self::check_now`].
    pub fn create(&self, words: &str, by: CreatedBy) -> Result<AlertView, String> {
        if let Some(reason) = self.blocked() {
            return Err(reason);
        }
        let words = clean_words(words)?;
        let jobs = self.jobs()?;
        if jobs.len() >= MAX_ALERTS {
            return Err(format!(
                "You have {MAX_ALERTS} paper alerts. Remove one to add another."
            ));
        }
        if jobs
            .iter()
            .any(|j| normalize_topic(&input_of(j).words) == normalize_topic(&words))
        {
            return Err(format!("You already follow “{words}”."));
        }
        let now = self.core.engine.now();
        let input = AlertInput {
            consent: Consent {
                text: consent_line(&words),
                at: now.to_rfc3339(),
            },
            words: words.clone(),
            ..AlertInput::default()
        };
        let job = self
            .store()
            .create_job(
                &NewJob {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind: JobKind::PaperAlert,
                    subject: words.clone(),
                    title: words,
                    schedule: Some(weekly()),
                    input: serde_json::to_value(&input).map_err(|e| e.to_string())?,
                    overlap: Overlap::Skip,
                    catch_up: CatchUp::Once,
                    max_attempts: 5,
                    created_by: by,
                    provenance: json!({}),
                },
                now,
            )
            .map_err(|e| e.to_string())?;
        self.core.sink.changed();
        self.view(&job)
    }

    /// Runs a check of the alert `id` now (its first one, when it was turned on).
    pub async fn check_now(&self, id: &str) -> Result<(), String> {
        self.core
            .engine
            .event(&self.handler, id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Rewords the alert (a new consent; refused while nothing may be sent) and turns it
    /// on or off; turning it on resumes one on hold.
    pub async fn update(
        &self,
        id: &str,
        words: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<AlertView, String> {
        let job = self.alert(id)?;
        let now = self.core.engine.now();
        let mut patch = JobPatch::default();
        if let Some(words) = words {
            if let Some(reason) = self.blocked() {
                return Err(reason);
            }
            let words = clean_words(words)?;
            let mut input = input_of(&job);
            input.consent = Consent {
                text: consent_line(&words),
                at: now.to_rfc3339(),
            };
            if input.words != words {
                // New words start afresh: their first check covers the last 14 days.
                input.checked_through.clear();
                input.progress = None;
            }
            input.words = words.clone();
            patch.input = Some(serde_json::to_value(&input).map_err(|e| e.to_string())?);
            patch.title = Some(words.clone());
            patch.subject = Some(words);
        }
        if let Some(on) = enabled {
            patch.enabled = Some(on);
            if on {
                patch.hold_reason = Some(None::<HoldReason>);
                patch.reset_failures = true;
            }
        }
        let job = self
            .store()
            .update_job(&job.id, &patch, now)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("There is no paper alert {id}."))?;
        if enabled == Some(true) {
            self.core
                .sink
                .resolve(&format!("{ALERT_HOLD_PREFIX}{}", job.id))
                .await;
        }
        self.core.sink.changed();
        self.view(&job)
    }

    /// Whether a change asks for a check now: new words, or turning the alert on.
    pub fn check_needed(words: Option<&str>, enabled: Option<bool>) -> bool {
        words.is_some() || enabled == Some(true)
    }

    /// Deletes the alert, the papers it found and its items.
    pub async fn delete(&self, id: &str) -> Result<bool, String> {
        let job = self.alert(id)?;
        self.core
            .feed
            .delete_job(&job.id)
            .map_err(|e| e.to_string())?;
        let deleted = self
            .store()
            .delete_job(&job.id)
            .map_err(|e| e.to_string())?;
        self.core
            .sink
            .resolve_prefixed(
                InboxKind::WhatsNew,
                &format!("{PAPERS_ITEM_PREFIX}{}:", job.id),
            )
            .await;
        self.core
            .sink
            .resolve(&format!("{ALERT_HOLD_PREFIX}{}", job.id))
            .await;
        self.core
            .sink
            .gone(EventTrigger::PaperFound {
                alert_id: job.id.clone(),
            })
            .await;
        self.core.sink.changed();
        Ok(deleted)
    }

    fn view(&self, job: &Job) -> Result<AlertView, String> {
        let input = input_of(job);
        let blocked = self.blocked();
        let active = self
            .store()
            .active_run(&job.id)
            .map_err(|e| e.to_string())?;
        let last = self
            .store()
            .runs(&job.id, 20)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| !r.state.is_active());
        let progress = input
            .progress
            .clone()
            .filter(|p| active.as_ref().is_some_and(|run| run.id == p.run_id));
        let state = if !job.enabled {
            AlertState::Off
        } else if blocked.is_some() {
            AlertState::Blocked
        } else if job.hold_reason.is_some() {
            AlertState::Held
        } else {
            match &active {
                Some(run) if run.state == RunState::Waiting => match run.wait_reason {
                    Some(WaitReason::Offline) => AlertState::Offline,
                    _ if run.error_code.as_deref() == Some(AFTER_WAKE) => AlertState::Checking,
                    _ => AlertState::WaitingRetry,
                },
                Some(_) => AlertState::Checking,
                None if last.is_none() => AlertState::Checking,
                None => AlertState::Active,
            }
        };
        Ok(AlertView {
            id: job.id.clone(),
            words: input.words,
            consent_text: input.consent.text,
            state,
            last_checked_at: last.as_ref().and_then(|r| r.ended_at.clone()),
            last_summary: last.and_then(|r| r.summary),
            next_check_at: job.next_due_at.clone(),
            failed_apis: progress
                .as_ref()
                .map(|p| p.failed.keys().map(|a| a.name().to_string()).collect())
                .unwrap_or_default(),
            retry_at: progress.and_then(|p| p.retry_at),
            blocked_reason: blocked,
        })
    }

    /// Every alert, oldest first.
    pub fn list(&self) -> Result<Vec<AlertView>, String> {
        self.jobs()?.iter().map(|j| self.view(j)).collect()
    }

    /// Topics suggested from the library, without those already followed.
    pub async fn suggestions(&self) -> Result<Vec<TopicSuggestion>, String> {
        let existing: Vec<String> = self.jobs()?.iter().map(|j| input_of(j).words).collect();
        let inputs = self.core.env.topic_inputs().await;
        Ok(suggest_topics(&inputs, &existing))
    }

    /// The paper row `item_id`, its open-access PDF and a name to save it under.
    pub fn pdf_of(&self, item_id: &str) -> Result<(FeedItem, url::Url, String), String> {
        if let Some(reason) = self.blocked() {
            return Err(reason);
        }
        let row = self
            .core
            .feed
            .item(item_id)
            .map_err(|e| e.to_string())?
            .filter(|r| r.kind == FeedKind::Paper)
            .ok_or_else(|| "That paper is no longer in What's new.".to_string())?;
        if let Some(saved) = row.data.get("addedPath").and_then(Value::as_str) {
            return Err(format!("This paper is already in your library: {saved}"));
        }
        let pdf = row
            .data
            .get("pdfUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| "This paper has no open-access PDF.".to_string())?;
        let url = shodh_rag::harness::web::client::check_url(pdf).map_err(|e| e.to_string())?;
        if url.scheme() != "https" {
            return Err(format!(
                "Shodh saves only PDFs offered over https; this one is at {url}."
            ));
        }
        let mut name: String = title_key(&row.title).chars().take(80).collect();
        if name.trim().is_empty() {
            name = "paper".into();
        }
        Ok((row, url, format!("{}.pdf", name.trim())))
    }

    /// Saves the open-access PDF of the paper row `item_id` into `folder` (a watched
    /// folder): https only, every redirect hop included, and only a PDF (its type and
    /// its first bytes). The row names the file before it is written, so the folder's
    /// sync never lists it as a new file too. Returns where it was saved.
    pub async fn add_to_library(&self, item_id: &str, folder: &Path) -> Result<PathBuf, String> {
        let (row, url, name) = self.pdf_of(item_id)?;
        let planned = folder.join(&name);
        let before = row.data.clone();
        let mut pending = before.clone();
        pending["addedPath"] = json!(planned.display().to_string());
        self.core.feed.set_data(&row.id, &pending).map_err(|e| {
            format!("Nothing was saved: What's new could not note the download ({e}).")
        })?;
        let client = self
            .core
            .env
            .client()
            .https_only()
            .with_timeout(std::time::Duration::from_secs(600));
        let target = folder.to_path_buf();
        let downloaded =
            crate::agent_tools::download_into(&client, &url, Some(&name), true, move || Ok(target))
                .await;
        let saved = match downloaded {
            Ok(downloaded) => downloaded.path,
            Err(e) => {
                if let Err(undo) = self.core.feed.set_data(&row.id, &before) {
                    tracing::warn!(error = %undo, "a failed download's note was not undone");
                }
                self.core.sink.changed();
                return Err(e.to_string());
            }
        };
        if saved != planned {
            let mut done = before;
            done["addedPath"] = json!(saved.display().to_string());
            if let Err(e) = self.core.feed.set_data(&row.id, &done) {
                return Err(format!(
                    "Saved to {}, but What's new could not note it ({e}): it may also show as a new file.",
                    saved.display()
                ));
            }
        }
        self.core.sink.changed();
        Ok(saved)
    }
}

/// The app's side of paper alerts: the policy, the web client, the library.
pub(crate) struct TauriAlertEnv {
    pub(crate) app: tauri::AppHandle,
}

#[async_trait]
impl AlertEnv for TauriAlertEnv {
    fn blocked(&self) -> Option<String> {
        match crate::profile::app_data_dir(&self.app) {
            Ok(dir) => match crate::app_settings::SettingsStore::in_dir(&dir).load() {
                Ok(settings) => settings.policy.web_block_reason().map(str::to_string),
                Err(e) => Some(format!(
                    "Paper alerts send nothing because the settings could not be read ({e})."
                )),
            },
            Err(e) => Some(format!(
                "Paper alerts send nothing because the app data folder is unavailable ({e})."
            )),
        }
    }

    fn client(&self) -> SafeClient {
        SafeClient::system()
    }

    async fn library_papers(&self) -> Vec<Paper> {
        let (scans, files) = library_sources(&self.app).await;
        shodh_rag::whats_new::library::library_papers(&scans, &files)
    }

    async fn scorer(&self) -> Option<SharedScorer> {
        use tauri::Manager;
        let slot = self
            .app
            .state::<crate::rag_commands::RagState>()
            .rag
            .read()
            .await
            .reranker_handle();
        let scorer: SharedScorer = tokio::task::spawn_blocking(move || slot.get())
            .await
            .ok()??;
        Some(scorer)
    }

    async fn topic_inputs(&self) -> TopicInputs {
        use tauri::Manager;
        let (scans, files) = library_sources(&self.app).await;
        let mut titles = shodh_rag::whats_new::library::scan_titles(&scans);
        let scanned: BTreeSet<String> = scans
            .iter()
            .filter_map(|s| {
                let v: Value = serde_json::from_str(s).ok()?;
                v.get("filePath")?.as_str().map(|p| p.to_lowercase())
            })
            .collect();
        for file in &files {
            if scanned.contains(&file.to_lowercase()) {
                continue;
            }
            let lower = file.to_lowercase();
            if !lower.ends_with(".pdf") {
                continue;
            }
            if let Some(name) = Path::new(file).file_name().map(|n| n.to_string_lossy()) {
                if let Some(title) = shodh_rag::whats_new::library::title_from_file_name(&name) {
                    titles.push(title);
                }
            }
        }
        let methods = self
            .app
            .state::<crate::research_commands::ResearchState>()
            .graph()
            .map(|g| {
                g.method_use_counts()
                    .into_iter()
                    .map(|(m, n)| (m.label.clone(), n))
                    .collect()
            })
            .unwrap_or_default();
        TopicInputs { titles, methods }
    }
}

/// The stored scans' JSON and the indexed files, read on this computer.
async fn library_sources(app: &tauri::AppHandle) -> (Vec<String>, Vec<String>) {
    use tauri::Manager;
    let scans = match app.state::<crate::audit_commands::AuditState>().database() {
        Some((path, key)) => tokio::task::spawn_blocking(move || {
            shodh_rag::research::db::ResearchDb::open(&path, key.as_ref())
                .and_then(|db| db.citation_scan_jsons())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default(),
        None => Vec::new(),
    };
    let files = match app
        .state::<crate::rag_commands::RagState>()
        .rag
        .read()
        .await
        .document_sources()
        .await
    {
        Ok(rows) => rows.into_iter().map(|r| r.source).collect(),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "indexed files unreadable for paper alerts");
            Vec::new()
        }
    };
    (scans, files)
}

/// Turns on an alert for `words` (the user saw the consent line) and starts its first
/// check.
#[tauri::command]
pub async fn paper_alert_create(app: tauri::AppHandle, words: String) -> Result<AlertView, String> {
    let alerts = crate::engine_commands::paper_alerts_for_command(&app).await?;
    let view = alerts.create(&words, CreatedBy::User)?;
    let id = view.id.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = alerts.check_now(&id).await {
            tracing::warn!(alert = %id, error = %e, "a paper alert's first check did not start");
        }
    });
    Ok(view)
}

/// Rewords an alert (shown its new consent line) or turns it on or off.
#[tauri::command]
pub async fn paper_alert_update(
    app: tauri::AppHandle,
    id: String,
    words: Option<String>,
    enabled: Option<bool>,
) -> Result<AlertView, String> {
    let alerts = crate::engine_commands::paper_alerts_for_command(&app).await?;
    let view = alerts.update(&id, words.as_deref(), enabled).await?;
    if PaperAlerts::check_needed(words.as_deref(), enabled) && view.state != AlertState::Off {
        let id = view.id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = alerts.check_now(&id).await {
                tracing::warn!(alert = %id, error = %e, "a paper alert's check did not start");
            }
        });
    }
    crate::engine_commands::wake(&app);
    Ok(view)
}

/// Deletes an alert and the papers it found.
#[tauri::command]
pub async fn paper_alert_delete(app: tauri::AppHandle, id: String) -> Result<bool, String> {
    let alerts = crate::engine_commands::paper_alerts_for_command(&app).await?;
    alerts.delete(&id).await
}

/// Topics suggested from the library (computed here; nothing is sent).
#[tauri::command]
pub async fn paper_alert_suggestions(
    app: tauri::AppHandle,
) -> Result<Vec<TopicSuggestion>, String> {
    let alerts = crate::engine_commands::paper_alerts_for_command(&app).await?;
    alerts.suggestions().await
}

/// Downloads a found paper's open-access PDF into a watched folder.
#[tauri::command]
pub async fn whats_new_add_to_library(
    app: tauri::AppHandle,
    item_id: String,
    source_id: String,
) -> Result<String, String> {
    use tauri::Manager;
    let alerts = crate::engine_commands::paper_alerts_for_command(&app).await?;
    let folder = app
        .state::<crate::file_watcher::FolderSyncState>()
        .library_path(&source_id)
        .ok_or_else(|| {
            "That folder is not in the Library, or is not kept in sync now.".to_string()
        })?;
    let saved = alerts.add_to_library(&item_id, Path::new(&folder)).await?;
    Ok(saved.display().to_string())
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use bytes::Bytes;
    use shodh_rag::harness::web::client::{HttpRequest, HttpResponse, HttpTransport, Resolver};
    use shodh_rag::harness::web::WebError;
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::sync::Mutex;

    pub const ARXIV_XML: &str = include_str!(
        "../../../crates/shodh-rag/src/harness/fixtures/papers/linear-attention/arxiv.xml"
    );
    pub const OPENALEX_JSON: &str = include_str!(
        "../../../crates/shodh-rag/src/harness/fixtures/papers/linear-attention/openalex.json"
    );
    pub const SEMANTIC_JSON: &str = include_str!(
        "../../../crates/shodh-rag/src/harness/fixtures/papers/linear-attention/semantic-scholar.json"
    );

    /// One canned answer: status, headers, body.
    pub type Answer = (u16, Vec<(String, String)>, Vec<u8>);

    /// In-process DNS and HTTP: each host answers what it is set to, a full URL set in
    /// `urls` answers before its host, a host in `silent` resolves but never answers
    /// (a timeout), and every request is recorded. Any other host is unreachable.
    #[derive(Default)]
    pub struct ApiNet {
        pub answers: Mutex<HashMap<String, Answer>>,
        pub urls: Mutex<HashMap<String, Answer>>,
        pub silent: Mutex<Vec<String>>,
        pub sent: Mutex<Vec<HttpRequest>>,
    }

    impl ApiNet {
        /// Every API answering with its fixture.
        pub fn answering() -> Arc<Self> {
            let net = Arc::new(Self::default());
            net.set("export.arxiv.org", 200, ARXIV_XML);
            net.set("api.openalex.org", 200, OPENALEX_JSON);
            net.set("api.semanticscholar.org", 200, SEMANTIC_JSON);
            net
        }

        pub fn set(&self, host: &str, status: u16, body: &str) {
            self.set_with(
                host,
                status,
                &[("content-type", "application/json")],
                body.as_bytes(),
            );
        }

        pub fn set_with(&self, host: &str, status: u16, headers: &[(&str, &str)], body: &[u8]) {
            self.answers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(host.to_string(), answer(status, headers, body));
        }

        /// `url` answers this, whatever its host answers.
        pub fn route(&self, url: &str, status: u16, headers: &[(&str, &str)], body: &[u8]) {
            self.urls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(url.to_string(), answer(status, headers, body));
        }

        /// `host` resolves, but its requests time out.
        pub fn silence(&self, host: &str) {
            self.silent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(host.to_string());
        }

        pub fn unset(&self, host: &str) {
            self.answers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(host);
        }

        pub fn hosts_asked(&self) -> Vec<String> {
            self.sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(|r| r.url.host_str().map(str::to_string))
                .collect()
        }

        pub fn client(self: &Arc<Self>) -> SafeClient {
            SafeClient::new(self.clone(), self.clone())
        }
    }

    fn answer(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Answer {
        (
            status,
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body.to_vec(),
        )
    }

    #[async_trait]
    impl Resolver for ApiNet {
        async fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            let known = self
                .answers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(host)
                || self
                    .silent
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .any(|h| h == host)
                || self
                    .urls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .keys()
                    .any(|u| {
                        url::Url::parse(u)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_string))
                            .as_deref()
                            == Some(host)
                    });
            if known {
                Ok(vec!["93.184.216.34".parse().expect("an address")])
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "offline"))
            }
        }
    }

    #[async_trait]
    impl HttpTransport for ApiNet {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, WebError> {
            let host = request.url.host_str().unwrap_or_default().to_string();
            let by_url = self
                .urls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(request.url.as_str())
                .cloned();
            let answer = by_url.or_else(|| {
                self.answers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&host)
                    .cloned()
            });
            self.sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request);
            let (status, headers, body) = answer.ok_or(WebError::Timeout)?;
            Ok(HttpResponse {
                status,
                headers,
                body: Box::pin(futures::stream::iter(vec![Ok(Bytes::from(body))])),
            })
        }
    }

    /// A client with no network at all.
    pub fn offline_client() -> SafeClient {
        Arc::new(ApiNet::default()).client()
    }

    /// An alert environment over in-process HTTP: `blocked` is what the policy says.
    pub struct TestEnv {
        pub blocked: Mutex<Option<String>>,
        pub client: SafeClient,
        pub library: Mutex<Vec<Paper>>,
        pub topics: Mutex<TopicInputs>,
        /// Run once while a check reads the library: something the user does mid-run.
        pub mid_check: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    #[async_trait]
    impl AlertEnv for TestEnv {
        fn blocked(&self) -> Option<String> {
            self.blocked
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn client(&self) -> SafeClient {
            self.client.clone()
        }
        async fn library_papers(&self) -> Vec<Paper> {
            let hook = self
                .mid_check
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(hook) = hook {
                hook();
            }
            self.library
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        async fn scorer(&self) -> Option<SharedScorer> {
            None
        }
        async fn topic_inputs(&self) -> TopicInputs {
            self.topics
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    /// A pacer that never waits (tests ask APIs back to back).
    pub fn quick_pacer() -> &'static ApiPacer {
        static PACER: std::sync::OnceLock<ApiPacer> = std::sync::OnceLock::new();
        PACER.get_or_init(ApiPacer::without_spacing)
    }

    /// Paper alerts over the `shodh.db` of `dir`, on `engine`, with `env`.
    pub fn alerts(
        dir: &Path,
        engine: Arc<Engine>,
        env: Arc<TestEnv>,
        sink: Arc<dyn WhatsNewSink>,
    ) -> PaperAlerts {
        let feed = Arc::new(FeedStore::open(&dir.join("shodh.db"), None).unwrap());
        PaperAlerts::with_pacer(engine, feed, env, sink, quick_pacer())
    }

    /// An environment over `client` with nothing blocked and an empty library.
    pub fn env(client: SafeClient) -> Arc<TestEnv> {
        Arc::new(TestEnv {
            blocked: Mutex::new(None),
            client,
            library: Mutex::new(Vec::new()),
            topics: Mutex::new(TopicInputs::default()),
            mid_check: Mutex::new(None),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::task_repeat::testing::{local, TestClock};
    use shodh_rag::engine::clock::Clock;
    use shodh_rag::engine::store::RunState;
    use std::sync::Mutex;

    /// The query the fixtures were recorded for.
    const WORDS: &str = "linear attention delta rule";
    const LOCAL_ONLY: &str =
        "Local-only mode is on, so nothing may leave this computer. The user can turn it off in Settings → Privacy.";

    #[derive(Default)]
    struct Sink {
        posted: Mutex<Vec<NewInboxItem>>,
        resolved: Mutex<Vec<String>>,
        /// What the automations were told was found, and what was gone.
        found: Mutex<Vec<(EventTrigger, Vec<Value>)>>,
        gone: Mutex<Vec<EventTrigger>>,
    }

    #[async_trait]
    impl WhatsNewSink for Sink {
        async fn post(&self, item: NewInboxItem) {
            self.posted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(item);
        }
        async fn resolve(&self, id: &str) {
            self.resolved
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(id.to_string());
        }
        async fn resolve_prefixed(&self, _kind: InboxKind, prefix: &str) {
            self.resolved
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("{prefix}*"));
        }
        fn changed(&self) {}
        async fn found(&self, trigger: EventTrigger, subjects: Vec<Value>) {
            self.found
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((trigger, subjects));
        }
        async fn gone(&self, trigger: EventTrigger) {
            self.gone
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(trigger);
        }
    }

    struct Env {
        _dir: tempfile::TempDir,
        clock: Arc<TestClock>,
        net: Arc<ApiNet>,
        env: Arc<TestEnv>,
        sink: Arc<Sink>,
        engine: Arc<Engine>,
        store: Arc<EngineStore>,
        alerts: PaperAlerts,
    }

    fn setup(now: DateTime<Utc>) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EngineStore::open(&dir.path().join("shodh.db"), None).unwrap());
        let clock = Arc::new(TestClock(Mutex::new(now)));
        let engine = Arc::new(Engine::with_clock(
            store.clone(),
            clock.clone(),
            "alerts-test",
        ));
        let net = ApiNet::answering();
        let env = env(net.client());
        let sink = Arc::new(Sink::default());
        let alerts = super::testing::alerts(dir.path(), engine.clone(), env.clone(), sink.clone());
        Env {
            _dir: dir,
            clock,
            net,
            env,
            sink,
            engine,
            store,
            alerts,
        }
    }

    impl Env {
        fn block(&self, reason: Option<&str>) {
            *self.env.blocked.lock().unwrap_or_else(|e| e.into_inner()) =
                reason.map(str::to_string);
        }
        fn posted(&self) -> Vec<NewInboxItem> {
            self.sink
                .posted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn runs(&self, id: &str) -> Vec<Run> {
            self.store.runs(id, 50).unwrap()
        }
        async fn tick(&self) {
            self.engine.tick(&self.alerts.handler()).await.unwrap();
        }
        /// Papers found and listed (weak matches are kept as seen, not listed).
        fn papers(&self) -> usize {
            self.alerts
                .core
                .feed
                .papers()
                .unwrap()
                .iter()
                .filter(|p| p.data.get("setAside").is_none())
                .count()
        }
    }

    /// Monday 23 Nov 2026, 10:00 local time.
    fn monday() -> DateTime<Utc> {
        local(2026, 11, 23, 10, 0)
    }

    #[tokio::test]
    async fn an_alert_find_reaches_its_automations() {
        use crate::automations::testing::FakeEnv;
        use crate::automations::{
            templates, AgentOutcome, AutomationState, Automations, ANSWER_ONLY_CALLS,
        };

        let e = setup(monday());
        let alert = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        // The template "Summarise new papers", set up for this alert.
        let engine = Arc::new(Engine::with_clock(
            e.store.clone(),
            e.clock.clone(),
            "alerts-test-automations",
        ));
        let fake = Arc::new(FakeEnv::default());
        let automations = Automations::new(engine.clone(), fake.clone());
        let template = templates()
            .into_iter()
            .find(|t| t.id == "summarise_papers")
            .unwrap();
        let auto = automations
            .create(
                template.draft(Some(&alert.id), None),
                CreatedBy::User,
                json!({}),
            )
            .unwrap();
        assert_eq!(auto.state, AutomationState::Watching);
        assert_eq!(auto.planned_calls, ANSWER_ONLY_CALLS);
        e.alerts.check_now(&alert.id).await.unwrap();
        let listed = e.papers();
        assert!(listed > 0);
        let found = e
            .sink
            .found
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(found.len(), 1, "one event for the check");
        let (told, papers) = found[0].clone();
        assert_eq!(
            told,
            EventTrigger::PaperFound {
                alert_id: alert.id.clone()
            }
        );
        assert_eq!(
            papers.len(),
            listed,
            "the papers listed, not the ones set aside"
        );
        assert!(papers
            .iter()
            .all(|p| p["kind"] == "paper" && p["title"].as_str().is_some_and(|t| !t.is_empty())));
        // A second check finds nothing new: nothing is told twice.
        e.alerts.check_now(&alert.id).await.unwrap();
        assert_eq!(
            e.sink.found.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1
        );
        let asked = e.net.hosts_asked().len();
        // As the app's sink hands it on: one run, a minute later, which answers from the
        // papers alone. Nothing more is asked of any API, and the agent has no tools.
        assert_eq!(automations.event(&told, papers.clone()).await.unwrap(), 1);
        fake.script(AgentOutcome {
            calls: 1,
            answer: "Each paper in two sentences.".into(),
            ..AgentOutcome::default()
        });
        e.clock.set(monday() + TimeDelta::minutes(2));
        engine.tick(&automations.handler()).await.unwrap();
        let request = fake.requests.lock().unwrap()[0].clone();
        assert!(request.answer_only, "no tools");
        assert_eq!(request.planned, ANSWER_ONLY_CALLS);
        let first_title = papers[0]["title"].as_str().unwrap();
        assert!(request.instruction.contains(first_title));
        assert_eq!(
            e.net.hosts_asked().len(),
            asked,
            "the automation asked no API: it sent nothing new off the computer"
        );
        let run = engine.store().runs(&auto.id, 5).unwrap().remove(0);
        assert_eq!(run.state, RunState::Succeeded);
        assert_eq!((run.planned_calls, run.used_calls), (1, 1));
        // The alert is deleted: the automation is put on hold, saying why.
        e.alerts.delete(&alert.id).await.unwrap();
        let gone = e
            .sink
            .gone
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(gone, vec![told]);
        assert_eq!(automations.trigger_gone(&gone[0]).await.unwrap(), 1);
        let view = automations.get(&auto.id).unwrap();
        assert_eq!(view.state, AutomationState::OnHold);
        assert_eq!(
            view.last.and_then(|run| run.summary).as_deref(),
            Some("On hold: its paper alert was deleted.")
        );
    }

    #[tokio::test]
    async fn creating_an_alert_stores_the_consent_and_checks_at_once() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        assert_eq!(view.consent_text, consent_line(WORDS));
        assert!(view.consent_text.starts_with("Shodh sends these words: linear attention delta rule to arXiv, OpenAlex and Semantic Scholar"));
        assert_eq!(view.state, AlertState::Checking, "never checked yet");
        e.alerts.check_now(&view.id).await.unwrap();
        let runs = e.runs(&view.id);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Succeeded);
        assert_eq!(runs[0].trigger, Trigger::Event);
        assert_eq!(e.net.hosts_asked().len(), 3, "one request per API");
        let found = e.papers();
        assert!(found > 0);
        let posted = e.posted();
        assert_eq!(posted.len(), 1, "one Info item for the check");
        assert_eq!(posted[0].kind, InboxKind::WhatsNew);
        assert_eq!(posted[0].status, InboxStatus::Done);
        assert_eq!(posted[0].title, papers_title(found, WORDS));
        assert_eq!(
            posted[0].detail.as_deref(),
            Some("From arXiv, OpenAlex and Semantic Scholar.")
        );
        e.tick().await;
        let view = e.alerts.list().unwrap().remove(0);
        assert_eq!(view.state, AlertState::Active);
        assert!(view.next_check_at.is_some(), "Mondays at 09:00 from now on");
    }

    #[tokio::test]
    async fn a_blocked_run_sends_nothing_and_is_skipped() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.block(Some(LOCAL_ONLY));
        e.alerts.check_now(&view.id).await.unwrap();
        assert!(
            e.net
                .sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "nothing was sent"
        );
        let runs = e.runs(&view.id);
        assert_eq!(runs[0].state, RunState::Skipped);
        assert_eq!(
            runs[0].summary.as_deref(),
            Some("Not checked: Local-only mode is on.")
        );
        assert_eq!(e.store.job(&view.id).unwrap().unwrap().failure_streak, 0);
        assert!(e.posted().is_empty());
        let view = e.alerts.list().unwrap().remove(0);
        assert_eq!(view.state, AlertState::Blocked);
        assert_eq!(view.blocked_reason.as_deref(), Some(LOCAL_ONLY));
    }

    #[tokio::test]
    async fn local_only_refuses_creating_and_rewording_an_alert() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.block(Some(LOCAL_ONLY));
        assert_eq!(
            e.alerts
                .create("state space models", CreatedBy::User)
                .unwrap_err(),
            LOCAL_ONLY
        );
        assert_eq!(
            e.alerts
                .update(&view.id, Some("other words"), None)
                .await
                .unwrap_err(),
            LOCAL_ONLY
        );
        // Turning one off sends nothing, so it is allowed.
        assert_eq!(
            e.alerts
                .update(&view.id, None, Some(false))
                .await
                .unwrap()
                .state,
            AlertState::Off,
            "an alert turned off reads Off, whatever the policy"
        );
        let web_off = "Web access for the agent is turned off. The user can turn it on in Settings → Privacy.";
        e.block(Some(web_off));
        assert_eq!(
            e.alerts
                .create("state space models", CreatedBy::User)
                .unwrap_err(),
            web_off
        );
        assert!(e
            .net
            .sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty());
    }

    #[tokio::test]
    async fn one_api_failing_posts_the_rest_and_retries_only_it() {
        let e = setup(monday());
        e.net.set("api.semanticscholar.org", 503, "");
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let run = e
            .store
            .active_run(&view.id)
            .unwrap()
            .expect("waiting to retry");
        assert_eq!(run.wait_reason, Some(WaitReason::Backoff));
        let retry_at = run
            .next_attempt_at
            .as_deref()
            .and_then(parse_instant)
            .unwrap();
        assert_eq!(retry_at, monday() + TimeDelta::hours(1));
        let first = e.posted();
        assert_eq!(first.len(), 1, "the others' papers are posted at once");
        assert_eq!(
            first[0].detail.as_deref(),
            Some(format!("From arXiv and OpenAlex. Semantic Scholar didn't answer; Shodh tries again at {}.", local_clock(retry_at)).as_str())
        );
        let view_now = e.alerts.list().unwrap().remove(0);
        assert_eq!(view_now.state, AlertState::WaitingRetry);
        assert_eq!(view_now.failed_apis, ["Semantic Scholar"]);
        // An hour later Semantic Scholar answers; only it is asked.
        e.net.set("api.semanticscholar.org", 200, SEMANTIC_JSON);
        let asked_before = e.net.hosts_asked().len();
        e.clock.set(retry_at);
        e.tick().await;
        let asked: Vec<String> = e.net.hosts_asked()[asked_before..].to_vec();
        assert_eq!(asked, ["api.semanticscholar.org"]);
        let runs = e.runs(&view.id);
        assert_eq!(runs[0].state, RunState::Succeeded);
        let posted = e.posted();
        assert_eq!(
            posted.last().unwrap().id,
            first[0].id,
            "the same item is updated"
        );
        assert_eq!(
            posted.last().unwrap().detail.as_deref(),
            Some("From arXiv, OpenAlex and Semantic Scholar.")
        );
    }

    #[tokio::test]
    async fn a_retry_that_returns_the_same_paper_adds_nothing() {
        let e = setup(monday());
        e.net.set("api.semanticscholar.org", 503, "");
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let after_first = e.papers();
        // The retry finds Semantic Scholar answering with arXiv's papers again.
        e.net.set("api.semanticscholar.org", 200, SEMANTIC_JSON);
        e.net.set("export.arxiv.org", 200, ARXIV_XML);
        e.clock.set(monday() + TimeDelta::hours(1));
        e.tick().await;
        let after_retry = e.papers();
        assert!(after_retry >= after_first);
        // A whole new check of the same answers adds nothing.
        let next = local(2026, 11, 30, 9, 0);
        e.clock.set(next);
        e.tick().await;
        assert_eq!(e.papers(), after_retry, "every paper was known");
        let last = e.runs(&view.id).remove(0);
        assert_eq!(last.summary.as_deref(), Some("Nothing new"));
        assert_eq!(
            e.posted()
                .iter()
                .filter(|p| p.id.contains(&last.id))
                .count(),
            0,
            "nothing new posts nothing"
        );
    }

    #[tokio::test]
    async fn three_retries_then_the_run_succeeds_with_what_it_found() {
        let e = setup(monday());
        e.net.set("api.semanticscholar.org", 503, "");
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let mut at = monday();
        for _ in 0..MAX_API_RETRIES {
            at += TimeDelta::hours(1);
            e.clock.set(at);
            e.tick().await;
        }
        let runs = e.runs(&view.id);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, RunState::Succeeded);
        assert!(runs[0]
            .summary
            .as_deref()
            .unwrap()
            .ends_with("; Semantic Scholar didn't answer"));
        let item = e.posted().pop().unwrap();
        assert_eq!(
            item.detail.as_deref(),
            Some("From arXiv and OpenAlex. Semantic Scholar didn't answer this week.")
        );
        assert_eq!(e.store.job(&view.id).unwrap().unwrap().failure_streak, 0);
    }

    #[tokio::test]
    async fn offline_three_weeks_running_never_holds() {
        let e = setup(monday());
        for host in [
            "export.arxiv.org",
            "api.openalex.org",
            "api.semanticscholar.org",
        ] {
            e.net.unset(host);
        }
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let run = e.store.active_run(&view.id).unwrap().unwrap();
        assert_eq!(run.wait_reason, Some(WaitReason::Offline));
        assert_eq!(e.alerts.list().unwrap()[0].state, AlertState::Offline);
        let mut at = monday();
        for week in 0..3 {
            if week > 0 {
                at = local(2026, 11, 23, 9, 0) + TimeDelta::days(7 * week);
                e.clock.set(at);
                e.tick().await;
            }
            for _ in 0..MAX_API_RETRIES {
                at += TimeDelta::minutes(15);
                e.clock.set(at);
                e.tick().await;
            }
        }
        let job = e.store.job(&view.id).unwrap().unwrap();
        assert_eq!(job.hold_reason, None, "being offline never holds an alert");
        let runs = e.runs(&view.id);
        assert!(
            runs.iter().all(|r| r.state == RunState::Skipped),
            "{runs:?}"
        );
        assert_eq!(
            runs[0].summary.as_deref(),
            Some("Not checked: no internet connection")
        );
    }

    #[tokio::test]
    async fn three_failed_weeks_hold_with_one_attention_item_and_resume_clears_it() {
        let e = setup(local(2026, 11, 22, 10, 0));
        for host in [
            "export.arxiv.org",
            "api.openalex.org",
            "api.semanticscholar.org",
        ] {
            e.net.set(host, 500, "");
        }
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        for week in 0..3 {
            let mut at = local(2026, 11, 23, 9, 0) + TimeDelta::days(7 * week);
            e.clock.set(at);
            e.tick().await;
            for _ in 0..MAX_API_RETRIES {
                at += TimeDelta::hours(1);
                e.clock.set(at);
                e.tick().await;
            }
        }
        let job = e.store.job(&view.id).unwrap().unwrap();
        assert_eq!(job.hold_reason, Some(HoldReason::Failing));
        let holds: Vec<NewInboxItem> = e
            .posted()
            .into_iter()
            .filter(|p| p.id == format!("{ALERT_HOLD_PREFIX}{}", view.id))
            .collect();
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].status, InboxStatus::NeedsYou);
        assert_eq!(holds[0].data["resume"], "paper_alert");
        assert_eq!(e.alerts.list().unwrap()[0].state, AlertState::Held);
        let resumed = e.alerts.update(&view.id, None, Some(true)).await.unwrap();
        assert_eq!(resumed.state, AlertState::Active);
        assert!(e
            .sink
            .resolved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&holds[0].id));
    }

    #[tokio::test]
    async fn a_catch_up_run_waits_two_minutes_after_a_wake_and_reads_checking() {
        let e = setup(local(2026, 11, 22, 10, 0));
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        // The computer slept through Monday 09:00 and woke at 11:00.
        let woke = local(2026, 11, 23, 11, 0);
        e.clock.set(woke);
        e.tick().await;
        assert!(
            e.net
                .sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "nothing sent the moment it woke"
        );
        let run = e.store.active_run(&view.id).unwrap().unwrap();
        assert_eq!(run.trigger, Trigger::CatchUp);
        assert_eq!(
            run.next_attempt_at.as_deref().and_then(parse_instant),
            Some(woke + TimeDelta::minutes(2))
        );
        assert_eq!(e.alerts.list().unwrap()[0].state, AlertState::Checking);
        e.clock.set(woke + TimeDelta::minutes(2));
        e.tick().await;
        assert_eq!(e.runs(&view.id)[0].state, RunState::Succeeded);
        assert_eq!(e.net.hosts_asked().len(), 3);
    }

    #[tokio::test]
    async fn ten_alerts_is_the_most_and_one_topic_is_followed_once() {
        let e = setup(monday());
        for i in 0..MAX_ALERTS {
            e.alerts
                .create(&format!("topic {i}"), CreatedBy::User)
                .unwrap();
        }
        assert_eq!(
            e.alerts.create("one more", CreatedBy::User).unwrap_err(),
            "You have 10 paper alerts. Remove one to add another."
        );
        let e = setup(monday());
        e.alerts.create(WORDS, CreatedBy::User).unwrap();
        assert!(e
            .alerts
            .create("  Linear attention  DELTA rule ", CreatedBy::User)
            .is_err());
    }

    #[tokio::test]
    async fn deleting_an_alert_removes_its_papers_and_items() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        assert!(e.papers() > 0);
        assert!(e.alerts.delete(&view.id).await.unwrap());
        assert_eq!(e.papers(), 0);
        assert!(e.alerts.list().unwrap().is_empty());
        assert!(e
            .sink
            .resolved
            .lock()
            .unwrap()
            .contains(&format!("{PAPERS_ITEM_PREFIX}{}:*", view.id)));
    }

    #[tokio::test]
    async fn the_suggestions_command_sends_nothing() {
        let e = setup(monday());
        *e.env.topics.lock().unwrap_or_else(|e| e.into_inner()) = TopicInputs {
            titles: vec![
                "Gated Linear Attention Transformers".into(),
                "Linear Attention for Speech".into(),
            ],
            methods: vec![],
        };
        let suggested = e.alerts.suggestions().await.unwrap();
        assert_eq!(suggested[0].words, "linear attention");
        assert!(e
            .net
            .sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty());
        // A topic already followed is not suggested.
        e.alerts
            .create("Linear Attention", CreatedBy::User)
            .unwrap();
        assert!(e.alerts.suggestions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_to_library_needs_an_open_access_pdf_and_refuses_in_local_only() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let rows = e.alerts.core.feed.papers().unwrap();
        let without = rows.iter().find(|r| r.data["pdfUrl"].is_null());
        if let Some(row) = without {
            assert_eq!(
                e.alerts.pdf_of(&row.id).unwrap_err(),
                "This paper has no open-access PDF."
            );
        }
        let with = rows
            .iter()
            .find(|r| {
                r.data["pdfUrl"]
                    .as_str()
                    .is_some_and(|u| u.starts_with("https://"))
            })
            .expect("a paper with an https PDF");
        let (_, url, name) = e.alerts.pdf_of(&with.id).unwrap();
        assert_eq!(
            url.as_str(),
            with.data["pdfUrl"].as_str().unwrap(),
            "only the PDF the API reported"
        );
        assert!(name.ends_with(".pdf"));
        e.block(Some(LOCAL_ONLY));
        assert_eq!(e.alerts.pdf_of(&with.id).unwrap_err(), LOCAL_ONLY);
        assert!(e.alerts.pdf_of("missing").is_err());
        // Saved: the folder's next sync does not list it as a new file.
        e.block(None);
        let mut data = with.data.clone();
        data["addedPath"] = json!("C:/library/Papers/gla.pdf");
        assert!(e.alerts.core.feed.set_data(&with.id, &data).unwrap());
        let saved = e.alerts.core.feed.item(&with.id).unwrap().unwrap();
        assert_eq!(saved.data["addedPath"], "C:/library/Papers/gla.pdf");
        assert!(
            e.alerts
                .pdf_of(&with.id)
                .unwrap_err()
                .starts_with("This paper is already in your library"),
            "a paper is added once"
        );
    }

    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\ntrailer\n%%EOF\n";

    /// A paper row of `alert` whose open-access PDF is at `pdf`.
    fn paper_row(e: &Env, alert: &str, pdf: &str) -> String {
        let rows = e
            .alerts
            .core
            .feed
            .insert_if_absent(
                &[NewFeedItem {
                    kind: FeedKind::Paper,
                    source_ref: format!("{alert}|doi:10.1/{pdf}"),
                    job_id: alert.to_string(),
                    title: "Gated Linear Attention".into(),
                    detail: None,
                    link: json!({}),
                    data: json!({ "pdfUrl": pdf }),
                }],
                e.clock.now(),
            )
            .unwrap();
        rows[0].id.clone()
    }

    fn saved_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|f| f.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn add_to_library_saves_an_https_pdf_and_refuses_http_on_any_hop() {
        let e = setup(monday());
        let folder = tempfile::tempdir().unwrap();
        e.net.route(
            "https://papers.example/gla.pdf",
            200,
            &[("content-type", "application/pdf")],
            PDF,
        );
        let ok = paper_row(&e, "alert-1", "https://papers.example/gla.pdf");
        let saved = e.alerts.add_to_library(&ok, folder.path()).await.unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), PDF);
        let row = e.alerts.core.feed.item(&ok).unwrap().unwrap();
        assert_eq!(row.data["addedPath"], saved.display().to_string());
        assert!(
            e.alerts.add_to_library(&ok, folder.path()).await.is_err(),
            "added once"
        );

        // An http PDF is refused before anything is asked.
        let http = paper_row(&e, "alert-1", "http://papers.example/old.pdf");
        let asked = e.net.sent.lock().unwrap_or_else(|e| e.into_inner()).len();
        let err = e
            .alerts
            .add_to_library(&http, folder.path())
            .await
            .unwrap_err();
        assert!(err.contains("only PDFs offered over https"), "{err}");
        assert_eq!(
            e.net.sent.lock().unwrap_or_else(|e| e.into_inner()).len(),
            asked
        );

        // An https PDF that redirects to http is refused at the hop.
        e.net.route(
            "https://papers.example/moved.pdf",
            302,
            &[("location", "http://mirror.example/moved.pdf")],
            b"",
        );
        e.net.route(
            "http://mirror.example/moved.pdf",
            200,
            &[("content-type", "application/pdf")],
            PDF,
        );
        let moved = paper_row(&e, "alert-1", "https://papers.example/moved.pdf");
        let err = e
            .alerts
            .add_to_library(&moved, folder.path())
            .await
            .unwrap_err();
        assert!(err.contains("not an https address"), "{err}");
        assert!(!e
            .net
            .sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|r| r.url.scheme() == "http"));
        let row = e.alerts.core.feed.item(&moved).unwrap().unwrap();
        assert!(
            row.data.get("addedPath").is_none(),
            "a failed download notes nothing"
        );
        assert_eq!(
            saved_files(folder.path()).len(),
            1,
            "only the first PDF was saved"
        );
    }

    #[tokio::test]
    async fn add_to_library_saves_nothing_but_a_pdf() {
        let e = setup(monday());
        let folder = tempfile::tempdir().unwrap();
        e.net.route(
            "https://papers.example/page.pdf",
            200,
            &[("content-type", "text/html")],
            b"<html>a landing page</html>",
        );
        e.net.route(
            "https://papers.example/fake.pdf",
            200,
            &[("content-type", "application/pdf")],
            b"<html>not a pdf</html>",
        );
        for url in [
            "https://papers.example/page.pdf",
            "https://papers.example/fake.pdf",
        ] {
            let id = paper_row(&e, "alert-1", url);
            let err = e
                .alerts
                .add_to_library(&id, folder.path())
                .await
                .unwrap_err();
            assert!(!err.is_empty());
            let row = e.alerts.core.feed.item(&id).unwrap().unwrap();
            assert!(row.data.get("addedPath").is_none(), "{url}");
        }
        assert!(
            saved_files(folder.path()).is_empty(),
            "no file, not even a partial one"
        );
    }

    #[tokio::test]
    async fn new_words_start_afresh_and_ask_for_a_check() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let before = input_of(&e.store.job(&view.id).unwrap().unwrap());
        assert!(!before.checked_through.is_empty());
        e.alerts
            .update(&view.id, Some("state space models"), None)
            .await
            .unwrap();
        let after = input_of(&e.store.job(&view.id).unwrap().unwrap());
        assert!(
            after.checked_through.is_empty(),
            "the first check of new words covers 14 days"
        );
        assert!(after.progress.is_none());
        assert_eq!(after.consent.text, consent_line("state space models"));
        assert!(PaperAlerts::check_needed(Some("state space models"), None));
        assert!(PaperAlerts::check_needed(None, Some(true)));
        assert!(!PaperAlerts::check_needed(None, Some(false)));
        // The same words again keep what was checked.
        e.alerts.check_now(&view.id).await.unwrap();
        e.alerts
            .update(&view.id, Some("state  space models"), None)
            .await
            .unwrap();
        let same = input_of(&e.store.job(&view.id).unwrap().unwrap());
        assert!(!same.checked_through.is_empty());
    }

    #[tokio::test]
    async fn words_changed_mid_check_are_kept() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        // While the check runs, the user rewords the alert.
        let store = e.store.clone();
        let id = view.id.clone();
        let now = e.clock.now();
        *e.env.mid_check.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(move || {
            let job = store.job(&id).unwrap().unwrap();
            let mut input = input_of(&job);
            input.words = "state space models".into();
            input.consent.text = consent_line("state space models");
            input.checked_through.clear();
            store
                .update_job(
                    &id,
                    &JobPatch {
                        input: Some(serde_json::to_value(&input).unwrap()),
                        ..JobPatch::default()
                    },
                    now,
                )
                .unwrap();
        }));
        e.alerts.check_now(&view.id).await.unwrap();
        let input = input_of(&e.store.job(&view.id).unwrap().unwrap());
        assert_eq!(
            input.words, "state space models",
            "the user's words are kept"
        );
        assert_eq!(input.consent.text, consent_line("state space models"));
        assert!(
            input.checked_through.is_empty(),
            "the old words' dates are not written over the new words"
        );
    }

    #[tokio::test]
    async fn two_alerts_keep_their_own_papers() {
        let e = setup(monday());
        let first = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&first.id).await.unwrap();
        let second = e
            .alerts
            .create("linear attention delta rule models", CreatedBy::User)
            .unwrap();
        e.alerts.check_now(&second.id).await.unwrap();
        let of = |job: &str| {
            e.alerts
                .core
                .feed
                .papers()
                .unwrap()
                .into_iter()
                .filter(|r| r.job_id == job && r.data.get("setAside").is_none())
                .count()
        };
        let second_found = of(&second.id);
        assert!(of(&first.id) > 0);
        assert!(
            second_found > 0,
            "the first alert's papers do not hide the second's"
        );
        e.alerts.delete(&first.id).await.unwrap();
        assert_eq!(
            of(&second.id),
            second_found,
            "deleting one alert hides nothing of the other"
        );
    }

    #[tokio::test]
    async fn at_most_ten_new_papers_are_listed_per_check_and_the_rest_never_return() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let listed = e.papers();
        assert!(listed <= shodh_rag::whats_new::alerts::NEW_PAPERS_PER_RUN);
        // The next week's check of the same answers adds nothing.
        e.clock.set(local(2026, 11, 30, 9, 0));
        e.tick().await;
        assert_eq!(e.papers(), listed);
    }

    #[tokio::test]
    async fn a_503_retry_after_sets_the_retry_time() {
        let e = setup(monday());
        e.net.set_with(
            "api.semanticscholar.org",
            503,
            &[("retry-after", "7200")],
            b"",
        );
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let run = e.store.active_run(&view.id).unwrap().unwrap();
        assert_eq!(
            run.next_attempt_at.as_deref().and_then(parse_instant),
            Some(monday() + TimeDelta::hours(2))
        );
    }

    #[tokio::test]
    async fn a_timeout_is_retried_in_an_hour_not_as_offline() {
        let e = setup(monday());
        e.net.unset("api.semanticscholar.org");
        e.net.silence("api.semanticscholar.org");
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.check_now(&view.id).await.unwrap();
        let run = e.store.active_run(&view.id).unwrap().unwrap();
        assert_eq!(run.wait_reason, Some(WaitReason::Backoff));
        assert_eq!(
            run.next_attempt_at.as_deref().and_then(parse_instant),
            Some(monday() + TimeDelta::hours(1))
        );
        let progress = input_of(&e.store.job(&view.id).unwrap().unwrap())
            .progress
            .unwrap();
        assert_eq!(progress.retries, 1, "counted toward the three");
        assert!(
            progress.apis.contains(&PaperApi::Arxiv),
            "the others are not asked again"
        );
    }

    #[tokio::test]
    async fn an_alert_turned_off_reads_off_even_in_local_only() {
        let e = setup(monday());
        let view = e.alerts.create(WORDS, CreatedBy::User).unwrap();
        e.alerts.update(&view.id, None, Some(false)).await.unwrap();
        e.block(Some(LOCAL_ONLY));
        assert_eq!(e.alerts.list().unwrap()[0].state, AlertState::Off);
    }
}
