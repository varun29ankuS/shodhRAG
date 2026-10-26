//! The Slack sync against a Slack in memory, a real `shodh.db` and (for the index itself)
//! a real document index. Deterministic: every step is called by the test, the clock is
//! set by the test, and the pace between calls is zero.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;
use tokio::sync::{Notify, RwLock};

use super::*;
use crate::connectors::slack::events::{BotIdentity, Envelope};
use crate::connectors::slack::ids;
use crate::connectors::slack::store::{NewIndexedChannel, SlackStore};
use crate::embeddings::EmbeddingModel;

/// 2026-10-24 09:00 UTC.
const NOW: i64 = 1_792_832_400;
const DAY: i64 = 86_400;
const PAGE: usize = 2;

fn ts(seconds: i64) -> String {
    format!("{seconds}.000100")
}

/// A clock the test moves.
struct TestClock(Mutex<DateTime<Utc>>);

impl TestClock {
    fn at(seconds: i64) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Utc.timestamp_opt(seconds, 0).unwrap())))
    }
    fn advance(&self, by: chrono::Duration) {
        let mut now = self.0.lock().unwrap();
        *now += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

fn person(at: i64, user: &str, text: &str) -> SlackMessage {
    SlackMessage {
        ts: ts(at),
        thread_ts: None,
        user: Some(user.into()),
        bot: false,
        subtype: None,
        text: text.into(),
        files: vec![],
        reply_count: 0,
        latest_reply: None,
        hidden: false,
    }
}

/// A Slack workspace in memory: top-level messages per channel, threads by their first
/// message, and errors to return next.
#[derive(Default)]
struct FakeSlack {
    channels: Mutex<HashMap<String, Vec<SlackMessage>>>,
    threads: Mutex<HashMap<(String, String), Vec<SlackMessage>>>,
    infos: Mutex<HashMap<String, ChannelInfo>>,
    errors: Mutex<VecDeque<(&'static str, SlackApiError)>>,
    calls: Mutex<Vec<String>>,
}

impl FakeSlack {
    fn post(&self, channel: &str, message: SlackMessage) {
        self.channels
            .lock()
            .unwrap()
            .entry(channel.into())
            .or_default()
            .push(message);
    }

    /// A thread: its first message (with the reply count) and its replies.
    fn thread(&self, channel: &str, parent_at: i64, replies: &[(i64, &str, &str)]) {
        let mut parent = person(parent_at, "U1", &format!("thread {parent_at}"));
        parent.thread_ts = Some(ts(parent_at));
        parent.reply_count = replies.len();
        parent.latest_reply = replies.last().map(|(at, _, _)| ts(*at));
        let mut messages = vec![parent.clone()];
        for (at, user, text) in replies {
            let mut reply = person(*at, user, text);
            reply.thread_ts = Some(ts(parent_at));
            messages.push(reply);
        }
        {
            let mut channels = self.channels.lock().unwrap();
            let list = channels.entry(channel.into()).or_default();
            list.retain(|m| m.ts != parent.ts);
            list.push(parent);
        }
        self.threads
            .lock()
            .unwrap()
            .insert((channel.into(), ts(parent_at)), messages);
    }

    fn delete(&self, channel: &str, at: i64) {
        if let Some(list) = self.channels.lock().unwrap().get_mut(channel) {
            list.retain(|m| m.ts != ts(at));
        }
        self.threads
            .lock()
            .unwrap()
            .remove(&(channel.to_string(), ts(at)));
    }

    fn fail_next(&self, method: &'static str, error: SlackApiError) {
        self.errors.lock().unwrap().push_back((method, error));
    }

    fn error_for(&self, method: &str) -> Option<SlackApiError> {
        let mut errors = self.errors.lock().unwrap();
        let at = errors.iter().position(|(m, _)| *m == method)?;
        errors.remove(at).map(|(_, e)| e)
    }

    fn calls(&self, prefix: &str) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with(prefix))
            .cloned()
            .collect()
    }
}

#[async_trait]
impl SlackReader for FakeSlack {
    async fn history(
        &self,
        channel: &str,
        oldest: Option<&str>,
        latest: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError> {
        self.calls.lock().unwrap().push(format!(
            "history {channel} oldest={} latest={} cursor={}",
            oldest.unwrap_or("-"),
            latest.unwrap_or("-"),
            cursor.unwrap_or("-")
        ));
        if let Some(e) = self.error_for("history") {
            return Err(e);
        }
        let low = oldest.map(ts_value).unwrap_or(f64::MIN);
        let high = latest.map(ts_value).unwrap_or(f64::MAX);
        let mut messages: Vec<SlackMessage> = self
            .channels
            .lock()
            .unwrap()
            .get(channel)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| ts_value(&m.ts) > low && ts_value(&m.ts) < high)
            .collect();
        messages.sort_by(|a, b| ts_value(&b.ts).total_cmp(&ts_value(&a.ts)));
        let start: usize = cursor.map(|c| c.parse().unwrap()).unwrap_or(0);
        let page: Vec<SlackMessage> = messages.iter().skip(start).take(PAGE).cloned().collect();
        let next = (start + PAGE < messages.len()).then(|| (start + PAGE).to_string());
        Ok(HistoryPage {
            messages: page,
            next_cursor: next,
        })
    }

    async fn replies(
        &self,
        channel: &str,
        thread_ts: &str,
        cursor: Option<&str>,
    ) -> Result<HistoryPage, SlackApiError> {
        self.calls.lock().unwrap().push(format!(
            "replies {channel} {thread_ts} cursor={}",
            cursor.unwrap_or("-")
        ));
        if let Some(e) = self.error_for("replies") {
            return Err(e);
        }
        if let Some(thread) = self
            .threads
            .lock()
            .unwrap()
            .get(&(channel.to_string(), thread_ts.to_string()))
        {
            return Ok(HistoryPage {
                messages: thread.clone(),
                next_cursor: None,
            });
        }
        match self
            .channels
            .lock()
            .unwrap()
            .get(channel)
            .and_then(|list| list.iter().find(|m| m.ts == thread_ts).cloned())
        {
            Some(alone) => Ok(HistoryPage {
                messages: vec![alone],
                next_cursor: None,
            }),
            None => Err(SlackApiError::Api {
                code: "thread_not_found".into(),
                needed: None,
            }),
        }
    }

    async fn channel_info(&self, channel: &str) -> Result<ChannelInfo, SlackApiError> {
        self.calls.lock().unwrap().push(format!("info {channel}"));
        if let Some(e) = self.error_for("info") {
            return Err(e);
        }
        Ok(self
            .infos
            .lock()
            .unwrap()
            .get(channel)
            .cloned()
            .unwrap_or(ChannelInfo {
                name: channel.to_lowercase(),
                archived: false,
                shared: false,
                private: false,
            }))
    }

    async fn permalink(&self, channel: &str, ts: &str) -> Result<String, SlackApiError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("permalink {channel} {ts}"));
        if let Some(e) = self.error_for("permalink") {
            return Err(e);
        }
        Ok(format!(
            "https://acme.slack.com/archives/{channel}/p{}",
            ts.replace('.', "")
        ))
    }

    async fn user_name(&self, user: &str) -> Result<String, SlackApiError> {
        self.calls.lock().unwrap().push(format!("user {user}"));
        if let Some(e) = self.error_for("user") {
            return Err(e);
        }
        Ok(match user {
            "U1" => "Ann".into(),
            "U2" => "Bob".into(),
            other => other.to_lowercase(),
        })
    }
}

/// An index in memory. A put replaces what its source held, as the real index does.
#[derive(Default)]
struct FakeIndex {
    docs: Mutex<HashMap<String, ThreadDocument>>,
    /// Sources put again (their document replaced).
    replaced: Mutex<Vec<String>>,
    /// Every put, in order.
    puts: Mutex<Vec<String>>,
    deleted_spaces: Mutex<Vec<String>>,
    /// Set: the next put starts a forget waiting for this gate (a click on "Stop
    /// indexing and forget" while a page is being indexed).
    forget_on_put: Mutex<Option<Arc<SyncGate>>>,
    waiting_forget: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl FakeIndex {
    fn sources(&self) -> Vec<String> {
        let mut sources: Vec<String> = self.docs.lock().unwrap().keys().cloned().collect();
        sources.sort();
        sources
    }
    fn doc(&self, channel: &str, at: i64) -> Option<ThreadDocument> {
        self.docs
            .lock()
            .unwrap()
            .get(&ids::thread_source("T1", channel, &ts(at)))
            .cloned()
    }
}

#[async_trait]
impl ThreadIndex for FakeIndex {
    async fn put(&self, doc: &ThreadDocument, _turn: &GateTurn) -> Result<(), String> {
        self.puts.lock().unwrap().push(doc.source.clone());
        let forget = self.forget_on_put.lock().unwrap().take();
        if let Some(gate) = forget {
            let waiting = tokio::spawn(async move {
                let _turn = gate.urgent_turn().await;
            });
            *self.waiting_forget.lock().unwrap() = Some(waiting);
            // The test runtime has one thread: the forget runs now, up to its wait.
            tokio::task::yield_now().await;
        }
        if self
            .docs
            .lock()
            .unwrap()
            .insert(doc.source.clone(), doc.clone())
            .is_some()
        {
            self.replaced.lock().unwrap().push(doc.source.clone());
        }
        Ok(())
    }
    async fn delete_thread(&self, source: &str) -> Result<(), String> {
        self.docs.lock().unwrap().remove(source);
        Ok(())
    }
    async fn delete_space(&self, space_id: &str) -> Result<(), String> {
        self.deleted_spaces.lock().unwrap().push(space_id.into());
        let (team, channel) = ids::parse_space(space_id).unwrap();
        let prefix = ids::channel_source_prefix(&team, &channel);
        self.docs
            .lock()
            .unwrap()
            .retain(|source, _| !source.starts_with(&prefix));
        Ok(())
    }
}

/// What the app would remove outside the connector; `failing` makes the memory removal
/// fail (the memory store cannot be opened).
#[derive(Default)]
struct Observed {
    removed: Mutex<Vec<String>>,
    memories: Mutex<Vec<String>>,
    failing: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl SyncObserver for Observed {
    fn changed(&self) {}
    async fn remove_from_workspace(&self, channel: &IndexedChannel) -> Result<(), String> {
        self.removed
            .lock()
            .unwrap()
            .push(channel.channel_id.clone());
        Ok(())
    }
    async fn forget_memories(&self, memory_ids: &[String]) -> Result<(), String> {
        if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("the memories could not be opened".into());
        }
        self.memories
            .lock()
            .unwrap()
            .extend(memory_ids.iter().cloned());
        Ok(())
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    slack: Arc<FakeSlack>,
    index: Arc<FakeIndex>,
    store: Arc<SlackStore>,
    clock: Arc<TestClock>,
    observed: Arc<Observed>,
    wake: Arc<Notify>,
    gate: Arc<SyncGate>,
}

fn timing() -> SyncTiming {
    SyncTiming {
        pace: Duration::ZERO,
        ..SyncTiming::default()
    }
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("shodh.db");
        Self {
            store: Arc::new(SlackStore::open(&db, None).unwrap()),
            db,
            _dir: dir,
            slack: Arc::new(FakeSlack::default()),
            index: Arc::new(FakeIndex::default()),
            clock: TestClock::at(NOW),
            observed: Arc::new(Observed::default()),
            wake: Arc::new(Notify::new()),
            gate: Arc::new(SyncGate::default()),
        }
    }

    /// A worker over the rig, as after a start of the app (a new worker each time).
    fn worker(&self) -> SyncWorker {
        SyncWorker::new(
            "T1",
            "UBOT",
            self.slack.clone(),
            self.index.clone(),
            // A new connection to the same database: what a restarted app has.
            Arc::new(SlackStore::open(&self.db, None).unwrap()),
            self.clock.clone(),
            self.observed.clone(),
            self.gate.clone(),
            self.wake.clone(),
            timing(),
        )
    }

    fn events(&self) -> IndexEvents {
        IndexEvents::new(
            "T1",
            self.store.clone(),
            self.clock.clone(),
            self.wake.clone(),
        )
    }

    fn index_channel(&self, channel: &str, shared: bool) {
        self.store
            .add_channel(
                &NewIndexedChannel {
                    team_id: "T1".into(),
                    channel_id: channel.into(),
                    name: channel.to_lowercase(),
                    workspace_id: "w1".into(),
                    shared,
                    private: false,
                    window_days: 90,
                },
                self.clock.now(),
            )
            .unwrap();
    }

    fn channel(&self, channel: &str) -> IndexedChannel {
        self.store.channel("T1", channel).unwrap().unwrap()
    }
}

/// Steps until the worker is idle; returns the work done.
async fn run_until_idle(worker: &SyncWorker) -> Vec<Work> {
    let mut done = Vec::new();
    for _ in 0..200 {
        match worker.step().await.unwrap() {
            Step::Worked(work) => done.push(work),
            Step::Idle(_) => return done,
        }
    }
    panic!("the sync never went idle: {done:?}");
}

#[tokio::test]
async fn the_backfill_reads_the_window_and_resumes_from_its_checkpoint() {
    let rig = Rig::new();
    for days_ago in [1, 2, 3, 4] {
        rig.slack.post(
            "C1",
            person(
                NOW - days_ago * DAY,
                "U1",
                &format!("note {days_ago} <@U2>"),
            ),
        );
    }
    rig.slack
        .thread("C1", NOW - 5 * DAY, &[(NOW - 5 * DAY + 60, "U2", "reply")]);
    // Older than the 90-day window: never read.
    rig.slack
        .post("C1", person(NOW - 120 * DAY, "U1", "ancient"));
    rig.index_channel("C1", false);

    // The first page, then the app stops.
    let first = rig.worker();
    assert_eq!(
        first.step().await.unwrap(),
        Step::Worked(Work::BackfillPage {
            channel: "C1".into(),
            threads: 2,
            done: false
        })
    );
    let checkpoint = rig.channel("C1");
    assert_eq!(checkpoint.backfill_cursor.as_deref(), Some("2"));
    assert_eq!(
        checkpoint.backfill_latest.as_deref(),
        Some("1792832400.000000")
    );
    drop(first);

    // A day later (the laptop slept), a new worker resumes at the saved page and keeps
    // the window it started with.
    rig.clock.advance(chrono::Duration::days(1));
    let second = rig.worker();
    let work = run_until_idle(&second).await;
    assert_eq!(
        work.iter()
            .filter(|w| matches!(w, Work::BackfillPage { .. }))
            .count(),
        2
    );
    let histories = rig.slack.calls("history");
    assert!(
        histories[1].ends_with("latest=1792832400.000000 cursor=2"),
        "{histories:?}"
    );
    assert!(histories[1].contains(&format!("oldest={}.000000", NOW - 90 * DAY)));
    assert_eq!(rig.index.sources().len(), 5, "{:?}", rig.index.sources());
    let thread = rig.index.doc("C1", NOW - 5 * DAY).unwrap();
    assert!(
        thread.text.contains("Bob"),
        "the reply is in the thread's document"
    );
    assert_eq!(
        rig.slack.calls("replies").len(),
        1,
        "only the threaded parent"
    );
    let note = rig.index.doc("C1", NOW - DAY).unwrap();
    assert!(note.text.contains("note 1 @Bob"), "{}", note.text);
    assert_eq!(
        note.permalink.as_deref(),
        Some("https://acme.slack.com/archives/C1/p1792746000000100")
    );
    assert_eq!(
        rig.slack.calls("permalink").len(),
        1,
        "the others are derived"
    );
    let done = rig.channel("C1");
    assert!(done.backfill_done);
    assert_eq!(done.threads, 5);
    assert!(
        rig.index.replaced.lock().unwrap().is_empty(),
        "nothing was replaced"
    );
}

#[tokio::test]
async fn a_rate_limit_pauses_the_channel_and_it_resumes_where_it_stopped() {
    let rig = Rig::new();
    for days_ago in 1..=4 {
        rig.slack.post(
            "C1",
            person(NOW - days_ago * DAY, "U1", &format!("m{days_ago}")),
        );
    }
    rig.index_channel("C1", false);
    let worker = rig.worker();
    assert!(matches!(
        worker.step().await.unwrap(),
        Step::Worked(Work::BackfillPage { done: false, .. })
    ));
    rig.slack.fail_next(
        "history",
        SlackApiError::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        },
    );
    let until = Utc.timestamp_opt(NOW + 30, 0).unwrap();
    assert_eq!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Paused {
            channel: "C1".into(),
            until
        })
    );
    let paused = rig.channel("C1");
    assert_eq!(paused.paused_until, Some(until));
    assert_eq!(paused.error.as_deref(), Some("Slack's rate limit"));
    assert_eq!(
        paused.backfill_cursor.as_deref(),
        Some("2"),
        "the checkpoint stays"
    );
    // Before Retry-After nothing is asked of Slack.
    let asked = rig.slack.calls("history").len();
    assert_eq!(worker.step().await.unwrap(), Step::Idle(Some(until)));
    assert_eq!(rig.slack.calls("history").len(), asked);
    rig.clock.advance(chrono::Duration::seconds(31));
    let work = run_until_idle(&worker).await;
    assert!(work.contains(&Work::BackfillPage {
        channel: "C1".into(),
        threads: 2,
        done: true
    }));
    assert!(rig.slack.calls("history")[asked].ends_with("cursor=2"));
    assert_eq!(rig.index.sources().len(), 4);
    assert!(rig.channel("C1").paused_until.is_none());
}

#[tokio::test]
async fn an_incremental_poll_reads_only_what_came_after_the_newest_message() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "old"));
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    assert_eq!(rig.index.sources().len(), 1);
    rig.slack
        .post("C1", person(NOW + 600, "U2", "new while away"));
    rig.clock.advance(chrono::Duration::minutes(16));
    let work = run_until_idle(&worker).await;
    assert!(
        work.contains(&Work::Poll {
            channel: "C1".into(),
            threads: 1,
            done: true
        }),
        "{work:?}"
    );
    let last = rig.slack.calls("history").last().cloned().unwrap();
    assert!(last.contains("oldest=1792832400.000000"), "{last}");
    assert!(rig.index.doc("C1", NOW + 600).is_some());
    assert_eq!(
        rig.channel("C1").newest_ts.as_deref(),
        Some(ts(NOW + 600).as_str())
    );
}

fn envelope(event: serde_json::Value) -> Envelope {
    Envelope {
        envelope_id: "e1".into(),
        kind: "events_api".into(),
        payload: json!({"type": "event_callback", "event_id": "Ev1", "event": event}),
        retry_attempt: 0,
    }
}

fn bot() -> BotIdentity {
    BotIdentity {
        user_id: "UBOT".into(),
        bot_id: Some("BBOT".into()),
    }
}

#[tokio::test]
async fn edits_and_deletions_replace_or_remove_their_thread() {
    let rig = Rig::new();
    rig.slack
        .thread("C1", NOW - DAY, &[(NOW - DAY + 10, "U2", "first reply")]);
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    let events = rig.events();

    // A new reply arrives as an event: the thread is fetched again and replaced.
    rig.slack.thread(
        "C1",
        NOW - DAY,
        &[
            (NOW - DAY + 10, "U2", "first reply"),
            (NOW + 5, "U2", "second reply"),
        ],
    );
    let reply = index_event(
        &envelope(
            json!({"type": "message", "channel": "C1", "channel_type": "channel",
            "user": "U2", "text": "second reply", "ts": ts(NOW + 5), "thread_ts": ts(NOW - DAY)}),
        ),
        &bot(),
    )
    .unwrap();
    assert!(events.apply(&reply).unwrap());
    assert_eq!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Thread {
            channel: "C1".into(),
            thread_ts: ts(NOW - DAY),
            kept: true
        })
    );
    assert!(rig
        .index
        .doc("C1", NOW - DAY)
        .unwrap()
        .text
        .contains("second reply"));
    assert_eq!(
        rig.index.replaced.lock().unwrap().len(),
        1,
        "replaced, not added twice"
    );

    // An edit of the reply.
    rig.slack.thread(
        "C1",
        NOW - DAY,
        &[
            (NOW - DAY + 10, "U2", "first reply"),
            (NOW + 5, "U2", "second reply, edited"),
        ],
    );
    let edit = index_event(
        &envelope(
            json!({"type": "message", "subtype": "message_changed", "channel": "C1",
            "channel_type": "channel", "hidden": true, "ts": ts(NOW + 50),
            "message": {"type": "message", "user": "U2", "text": "second reply, edited",
                "ts": ts(NOW + 5), "thread_ts": ts(NOW - DAY)}}),
        ),
        &bot(),
    )
    .unwrap();
    events.apply(&edit).unwrap();
    run_until_idle(&worker).await;
    assert!(rig
        .index
        .doc("C1", NOW - DAY)
        .unwrap()
        .text
        .contains("edited"));

    // The reply is deleted; the event does not say its thread: the store knows it.
    rig.slack
        .thread("C1", NOW - DAY, &[(NOW - DAY + 10, "U2", "first reply")]);
    let deleted = index_event(
        &envelope(json!({"type": "message", "subtype": "message_deleted", "channel": "C1",
            "channel_type": "channel", "hidden": true, "ts": ts(NOW + 60), "deleted_ts": ts(NOW + 5)})),
        &bot(),
    )
    .unwrap();
    assert_eq!(
        deleted,
        IndexEvent::Deleted {
            channel: "C1".into(),
            ts: ts(NOW + 5),
            thread_ts: None
        }
    );
    assert!(events.apply(&deleted).unwrap());
    run_until_idle(&worker).await;
    let doc = rig.index.doc("C1", NOW - DAY).unwrap();
    assert!(!doc.text.contains("second reply"), "{}", doc.text);

    // The whole thread is deleted: its document goes.
    rig.slack.delete("C1", NOW - DAY);
    let parent_gone = IndexEvent::Deleted {
        channel: "C1".into(),
        ts: ts(NOW - DAY),
        thread_ts: None,
    };
    assert!(events.apply(&parent_gone).unwrap());
    assert_eq!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Thread {
            channel: "C1".into(),
            thread_ts: ts(NOW - DAY),
            kept: false
        })
    );
    assert!(rig.index.doc("C1", NOW - DAY).is_none());
    assert!(rig.store.threads("T1", "C1").unwrap().is_empty());

    // Events of channels that are not indexed change nothing.
    let elsewhere = IndexEvent::Message {
        channel: "C9".into(),
        thread_ts: ts(NOW),
        ext_shared: false,
    };
    assert!(!events.apply(&elsewhere).unwrap());
}

#[tokio::test]
async fn the_slim_pass_deletes_what_slack_lost_and_fetches_what_changed() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "kept"));
    rig.slack
        .post("C1", person(NOW - 2 * DAY, "U1", "deleted while away"));
    rig.slack
        .thread("C1", NOW - 3 * DAY, &[(NOW - 3 * DAY + 5, "U2", "r1")]);
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    assert_eq!(rig.index.sources().len(), 3);

    // While Shodh was not running: one message deleted, one thread got a reply, and a
    // new message was posted.
    rig.slack.delete("C1", NOW - 2 * DAY);
    rig.slack.thread(
        "C1",
        NOW - 3 * DAY,
        &[
            (NOW - 3 * DAY + 5, "U2", "r1"),
            (NOW + 100, "U1", "late reply"),
        ],
    );
    rig.clock.advance(chrono::Duration::hours(7));
    let work = run_until_idle(&worker).await;
    assert!(
        work.iter()
            .any(|w| matches!(w, Work::Slim { deleted: 1, .. })),
        "{work:?}"
    );
    assert!(
        rig.index.doc("C1", NOW - 2 * DAY).is_none(),
        "deleted in Slack"
    );
    assert!(rig
        .index
        .doc("C1", NOW - 3 * DAY)
        .unwrap()
        .text
        .contains("late reply"));
    assert!(rig.index.doc("C1", NOW - DAY).is_some());
}

#[tokio::test]
async fn archived_deleted_and_removed_channels_stop_or_are_forgotten() {
    let rig = Rig::new();
    for c in ["C1", "C2", "C3", "C4"] {
        rig.slack.post(c, person(NOW - DAY, "U1", "hello"));
        rig.index_channel(c, false);
    }
    let worker = rig.worker();
    run_until_idle(&worker).await;
    assert_eq!(rig.index.sources().len(), 4);
    rig.store
        .save_memory(&crate::connectors::slack::store::SlackMemory {
            id: "m1".into(),
            team_id: "T1".into(),
            channel_id: "C2".into(),
            channel_name: "#c2".into(),
            author_id: "U1".into(),
            author_name: "Ann".into(),
            text: "x".into(),
            kind: crate::connectors::slack::store::SlackMemoryKind::Text,
            message_ts: ts(NOW),
            permalink: None,
            saved_at: rig.clock.now(),
            owner: true,
            memory_id: Some("mem-1".into()),
        })
        .unwrap();
    let events = rig.events();

    let archived = index_event(
        &envelope(json!({"type": "channel_archive", "channel": "C1", "user": "U1"})),
        &bot(),
    )
    .unwrap();
    events.apply(&archived).unwrap();
    assert_eq!(rig.channel("C1").state, ChannelState::Archived);
    assert!(
        rig.index.doc("C1", NOW - DAY).is_some(),
        "archived content stays"
    );

    let deleted = index_event(
        &envelope(json!({"type": "channel_deleted", "channel": "C2"})),
        &bot(),
    )
    .unwrap();
    events.apply(&deleted).unwrap();
    assert_eq!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Forgotten {
            channel: "C2".into()
        })
    );
    assert!(rig.store.channel("T1", "C2").unwrap().is_none());
    assert!(rig.index.doc("C2", NOW - DAY).is_none());
    assert_eq!(
        *rig.index.deleted_spaces.lock().unwrap(),
        vec![ids::space_id("T1", "C2")]
    );
    assert_eq!(
        *rig.observed.removed.lock().unwrap(),
        vec!["C2".to_string()],
        "the app removes the channel from its workspace"
    );
    assert_eq!(
        *rig.observed.memories.lock().unwrap(),
        vec!["mem-1".to_string()],
        "the app forgets the owner's memory from that channel"
    );

    let left = index_event(
        &envelope(
            json!({"type": "member_left_channel", "channel": "C3", "user": "UBOT",
            "channel_type": "C", "team": "T1"}),
        ),
        &bot(),
    )
    .unwrap();
    events.apply(&left).unwrap();
    assert_eq!(rig.channel("C3").state, ChannelState::Removed);
    assert!(
        index_event(
            &envelope(json!({"type": "member_left_channel", "channel": "C3", "user": "U7"})),
            &bot()
        )
        .is_none(),
        "someone else leaving changes nothing"
    );

    // Removed without an event: Slack says so when asked.
    rig.slack.post("C4", person(NOW + 10, "U1", "more"));
    rig.slack.fail_next(
        "info",
        SlackApiError::Api {
            code: "not_in_channel".into(),
            needed: None,
        },
    );
    rig.clock.advance(chrono::Duration::minutes(16));
    let work = run_until_idle(&worker).await;
    assert!(work.contains(&Work::Stopped {
        channel: "C4".into(),
        state: ChannelState::Removed
    }));
    // Stopped channels are not asked about again.
    let asked = rig.slack.calls("").len();
    rig.clock.advance(chrono::Duration::hours(7));
    run_until_idle(&worker).await;
    assert_eq!(rig.slack.calls("").len(), asked);
}

#[tokio::test]
async fn a_channel_shared_since_it_was_chosen_stops_until_confirmed() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "hello"));
    rig.index_channel("C1", false);
    run_until_idle(&rig.worker()).await;
    let shared = index_event(
        &envelope(
            json!({"type": "message", "channel": "C1", "channel_type": "channel",
            "user": "UEXT", "text": "hi from a client", "ts": ts(NOW + 1)}),
        ),
        &bot(),
    )
    .map(|e| match e {
        IndexEvent::Message {
            channel, thread_ts, ..
        } => IndexEvent::Message {
            channel,
            thread_ts,
            ext_shared: true,
        },
        other => other,
    })
    .unwrap();
    rig.events().apply(&shared).unwrap();
    let channel = rig.channel("C1");
    assert_eq!(channel.state, ChannelState::NowShared);
    assert_eq!(channel.error.as_deref(), Some(NOW_SHARED));
    assert!(
        rig.store.dirty_threads("T1", 10).unwrap().is_empty(),
        "not fetched"
    );
    // Confirming it (choosing it again as shared) resumes it.
    rig.index_channel("C1", true);
    assert_eq!(rig.channel("C1").state, ChannelState::Active);
}

#[tokio::test]
async fn a_refused_token_or_scope_stops_every_channel() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "hello"));
    rig.index_channel("C1", false);
    rig.index_channel("C2", false);
    rig.slack.fail_next(
        "history",
        SlackApiError::Api {
            code: "missing_scope".into(),
            needed: Some("channels:history".into()),
        },
    );
    let worker = rig.worker();
    assert!(matches!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Stopped {
            state: ChannelState::Error,
            ..
        })
    ));
    for c in ["C1", "C2"] {
        let channel = rig.channel(c);
        assert_eq!(channel.state, ChannelState::Error);
        assert!(channel.error.unwrap().contains("channels:history"));
    }
    assert_eq!(worker.step().await.unwrap(), Step::Idle(None));
    rig.store.retry_stopped("T1").unwrap();
    run_until_idle(&worker).await;
    assert!(rig.channel("C1").backfill_done);
}

#[test]
fn bot_messages_and_shodhs_own_edits_are_not_index_events() {
    for event in [
        json!({"type": "message", "channel": "C1", "channel_type": "channel", "user": "UBOT", "text": "Saved.", "ts": "1.1"}),
        json!({"type": "message", "channel": "C1", "channel_type": "channel", "bot_id": "B9", "text": "deploy", "ts": "1.1"}),
        json!({"type": "message", "subtype": "message_changed", "channel": "C1", "channel_type": "channel",
            "message": {"user": "UBOT", "bot_id": "BBOT", "text": "answer", "ts": "1.1"}}),
        json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "channel_type": "channel",
            "deleted_ts": "1.1", "previous_message": {"bot_id": "BBOT", "ts": "1.1"}}),
        json!({"type": "message", "subtype": "channel_join", "channel": "C1", "channel_type": "channel", "user": "U1", "ts": "1.1"}),
        json!({"type": "message", "channel": "D1", "channel_type": "im", "user": "U1", "text": "dm", "ts": "1.1"}),
        json!({"type": "app_mention", "channel": "C1", "user": "U1", "text": "<@UBOT> hi", "ts": "1.1"}),
    ] {
        assert_eq!(
            index_event(&envelope(event.clone()), &bot()),
            None,
            "{event}"
        );
    }
    assert_eq!(
        index_event(
            &envelope(
                json!({"type": "message", "channel": "G1", "channel_type": "group",
                "user": "U1", "text": "private", "ts": "2.1", "thread_ts": "1.1"})
            ),
            &bot()
        ),
        Some(IndexEvent::Message {
            channel: "G1".into(),
            thread_ts: "1.1".into(),
            ext_shared: false
        })
    );
    assert_eq!(
        index_event(
            &envelope(json!({"type": "group_deleted", "channel": "G1"})),
            &bot()
        ),
        Some(IndexEvent::ChannelDeleted {
            channel: "G1".into()
        })
    );
    assert_eq!(
        index_event(
            &envelope(json!({"type": "channel_left", "channel": "C1"})),
            &bot()
        ),
        Some(IndexEvent::BotLeft {
            channel: "C1".into()
        })
    );
}

#[test]
fn backfill_progress_is_estimated_from_the_window_read() {
    let mut channel = IndexedChannel {
        team_id: "T1".into(),
        channel_id: "C1".into(),
        name: "c1".into(),
        workspace_id: "w1".into(),
        shared: false,
        private: false,
        window_days: 90,
        state: ChannelState::Active,
        backfill_latest: Some(ts(NOW)),
        backfill_cursor: Some("2".into()),
        backfill_done: false,
        oldest_seen: Some(ts(NOW - 30 * DAY)),
        newest_ts: None,
        paused_until: None,
        error: None,
        permalink_base: None,
        last_sync_at: None,
        last_poll_at: None,
        last_slim_at: None,
        poll_latest: None,
        poll_cursor: None,
        poll_newest: None,
        slim_latest: None,
        started_at: Utc.timestamp_opt(NOW, 0).unwrap(),
        threads: 100,
    };
    let progress = backfill_progress(&channel, Duration::from_millis(1_300));
    assert_eq!(progress.done, 100);
    assert_eq!(
        progress.estimated_total,
        Some(300),
        "a third of the window read"
    );
    assert_eq!(progress.estimated_seconds_left, Some(260));
    channel.oldest_seen = Some(ts(NOW - DAY));
    assert_eq!(
        backfill_progress(&channel, Duration::from_secs(1)).estimated_total,
        None,
        "too little read to say"
    );
    channel.backfill_done = true;
    assert_eq!(
        backfill_progress(&channel, Duration::from_secs(1)).estimated_seconds_left,
        Some(0)
    );
    assert!(check_window_days(90).is_ok());
    assert!(check_window_days(3).is_err() && check_window_days(400).is_err());
}

// ── The real index ──────────────────────────────────────────────────

async fn engine(dir: &std::path::Path) -> Arc<RwLock<RAGEngine>> {
    engine_with(
        dir,
        Arc::new(crate::statements::testing::WordEmbedder::default()),
    )
    .await
}

async fn engine_with(
    dir: &std::path::Path,
    embedder: Arc<dyn crate::embeddings::EmbeddingModel>,
) -> Arc<RwLock<RAGEngine>> {
    let mut config = crate::config::RAGConfig::default();
    config.data_dir = dir.join("data");
    config.embedding.model_dir = dir.join("models");
    config.embedding.use_e5 = false;
    config.embedding.dimension = crate::statements::testing::DIM;
    config.search.min_score_threshold = 0.0;
    let mut engine = RAGEngine::new(config).await.unwrap();
    engine
        .attach_search_models(crate::rag_engine::SearchModels::from_embedder(embedder))
        .unwrap();
    Arc::new(RwLock::new(engine))
}

fn doc(channel: &str, at: i64, text: &str) -> ThreadDocument {
    ThreadDocument {
        source: ids::thread_source("T1", channel, &ts(at)),
        space_id: ids::space_id("T1", channel),
        title: format!("#{channel}: {text}"),
        text: text.into(),
        permalink: Some(format!(
            "https://acme.slack.com/archives/{channel}/p{at}000100"
        )),
        channel_name: channel.to_lowercase(),
        thread_ts: ts(at),
        latest_reply: None,
        reply_count: 0,
        message_ts: vec![ts(at)],
    }
}

async fn chunks_of(rag: &Arc<RwLock<RAGEngine>>, source: &str) -> usize {
    rag.read()
        .await
        .document_sources()
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.source == source)
        .map(|row| row.chunks)
        .sum()
}

#[tokio::test]
async fn threads_are_replaced_and_deleted_in_the_real_index_by_their_exact_source() {
    let dir = tempfile::tempdir().unwrap();
    let rag = Box::pin(engine(dir.path())).await;
    let index = EngineIndex { rag: rag.clone() };
    let gate = SyncGate::default();
    let turn = gate.turn().await;
    let first = doc("C1", NOW, "the walrus budget is ten");
    index.put(&first, &turn).await.unwrap();
    index
        .put(&doc("C2", NOW, "the narwhal plan"), &turn)
        .await
        .unwrap();
    assert!(chunks_of(&rag, &first.source).await > 0);
    // The source is kept exactly as given (not lowercased or re-slashed on Windows).
    let titles = rag.read().await.document_titles().await.unwrap();
    assert!(titles
        .iter()
        .any(|t| t.source == first.source && t.title == first.title));

    let edited = doc("C1", NOW, "the walrus budget is twelve");
    index.put(&edited, &turn).await.unwrap();
    let rows: Vec<_> = rag
        .read()
        .await
        .document_sources()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.source == first.source)
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "one document for the thread after the edit: {rows:?}"
    );

    index.delete_thread(&first.source).await.unwrap();
    assert_eq!(
        chunks_of(&rag, &first.source).await,
        0,
        "the thread is gone"
    );
    let other = ids::thread_source("T1", "C2", &ts(NOW));
    assert!(chunks_of(&rag, &other).await > 0, "the other channel stays");
    index
        .delete_space(&ids::space_id("T1", "C2"))
        .await
        .unwrap();
    assert_eq!(chunks_of(&rag, &other).await, 0);
}

// ── The fix round of PR #101 ────────────────────────────────────────

/// A thread whose first message and replies only bots wrote.
fn bot_thread(slack: &FakeSlack, channel: &str, parent_at: i64) {
    let mut parent = person(parent_at, "UB", "deploy started");
    parent.bot = true;
    parent.thread_ts = Some(ts(parent_at));
    parent.reply_count = 1;
    parent.latest_reply = Some(ts(parent_at + 5));
    let mut reply = person(parent_at + 5, "UB", "deploy done");
    reply.bot = true;
    reply.thread_ts = Some(ts(parent_at));
    slack.post(channel, parent.clone());
    slack
        .threads
        .lock()
        .unwrap()
        .insert((channel.into(), ts(parent_at)), vec![parent, reply]);
}

#[tokio::test]
async fn a_paused_channel_whose_poll_is_overdue_waits_for_its_pause() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "hello"));
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    // The poll is due, and Slack fails it with an error that passes.
    rig.clock.advance(chrono::Duration::minutes(16));
    rig.slack.fail_next("info", SlackApiError::Status(503));
    assert!(matches!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Failed { .. })
    ));
    let until = rig.channel("C1").paused_until.unwrap();
    assert!(until > rig.clock.now());
    // The poll is overdue now; the next work is the end of the pause, not a time past
    // (a zero wait: a hot loop for the whole pause).
    assert_eq!(worker.step().await.unwrap(), Step::Idle(Some(until)));
}

#[tokio::test]
async fn a_window_longer_than_one_slim_step_is_listed_whole_over_several_steps() {
    let rig = Rig::new();
    // More messages than one slim step lists (SLIM_PAGES pages of PAGE).
    let count = (SLIM_PAGES * PAGE + 5) as i64;
    for i in 1..=count {
        rig.slack
            .post("C1", person(NOW - i * 600, "U1", &format!("note {i}")));
    }
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    assert_eq!(rig.index.sources().len(), count as usize);
    // Deleted while Shodh was not running, older than what one step lists.
    rig.slack.delete("C1", NOW - count * 600);
    rig.clock.advance(chrono::Duration::hours(7));
    let work = run_until_idle(&worker).await;
    assert!(
        work.contains(&Work::Slim {
            channel: "C1".into(),
            deleted: 0,
            refetch: 0,
            done: false
        }),
        "{work:?}"
    );
    assert!(
        work.contains(&Work::Slim {
            channel: "C1".into(),
            deleted: 1,
            refetch: 0,
            done: true
        }),
        "{work:?}"
    );
    assert!(rig.index.doc("C1", NOW - count * 600).is_none());
    assert_eq!(rig.index.sources().len(), count as usize - 1);
    let channel = rig.channel("C1");
    assert_eq!(channel.last_slim_at, Some(rig.clock.now()), "recorded");
    assert!(channel.slim_latest.is_none());
    // Done: nothing more until the next pass is due.
    assert!(matches!(worker.step().await.unwrap(), Step::Idle(Some(_))));
}

#[tokio::test]
async fn a_poll_with_more_waiting_than_one_step_reads_every_message() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "old"));
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    // While the laptop was closed: one message more than a poll step reads.
    let waiting = (POLL_PAGES * PAGE + 1) as i64;
    for i in 0..waiting {
        rig.slack
            .post("C1", person(NOW + 60 + i, "U2", &format!("new {i}")));
    }
    rig.clock.advance(chrono::Duration::minutes(16));
    let work = run_until_idle(&worker).await;
    assert!(
        work.contains(&Work::Poll {
            channel: "C1".into(),
            threads: POLL_PAGES * PAGE,
            done: false
        }),
        "{work:?}"
    );
    assert!(
        work.contains(&Work::Poll {
            channel: "C1".into(),
            threads: 1,
            done: true
        }),
        "{work:?}"
    );
    assert_eq!(rig.index.sources().len(), 1 + waiting as usize, "no gap");
    assert!(
        rig.index.doc("C1", NOW + 60).is_some(),
        "the oldest one waiting"
    );
    let channel = rig.channel("C1");
    assert_eq!(
        channel.newest_ts.as_deref(),
        Some(ts(NOW + 60 + waiting - 1).as_str())
    );
    assert!(channel.poll_cursor.is_none() && channel.poll_latest.is_none());
}

#[tokio::test]
async fn a_failing_memory_removal_fails_the_forget_and_leaves_what_links_it() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "hello"));
    rig.index_channel("C1", false);
    run_until_idle(&rig.worker()).await;
    rig.store
        .save_memory(&crate::connectors::slack::store::SlackMemory {
            id: "m1".into(),
            team_id: "T1".into(),
            channel_id: "C1".into(),
            channel_name: "#c1".into(),
            author_id: "U1".into(),
            author_name: "Ann".into(),
            text: "x".into(),
            kind: crate::connectors::slack::store::SlackMemoryKind::Text,
            message_ts: ts(NOW),
            permalink: None,
            saved_at: rig.clock.now(),
            owner: true,
            memory_id: Some("mem-1".into()),
        })
        .unwrap();
    rig.observed
        .failing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let channel = rig.channel("C1");
    let failed = forget_channel(
        &rig.store,
        rig.index.as_ref(),
        rig.observed.as_ref(),
        &channel,
    )
    .await;
    assert!(failed.is_err(), "the forget says it failed");
    assert_eq!(
        rig.channel("C1").state,
        ChannelState::Forgetting,
        "it is tried again"
    );
    assert_eq!(
        rig.store.linked_memories("T1", Some("C1")).unwrap(),
        vec!["mem-1".to_string()],
        "what links the memory is kept until the memory is gone"
    );
    // The next step tries again; now it goes.
    rig.observed
        .failing
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        rig.worker().step().await.unwrap(),
        Step::Worked(Work::Forgotten {
            channel: "C1".into()
        })
    );
    assert_eq!(
        *rig.observed.memories.lock().unwrap(),
        vec!["mem-1".to_string()]
    );
    assert!(rig.store.channel("T1", "C1").unwrap().is_none());
    assert!(rig.store.memories("T1").unwrap().is_empty());
}

#[tokio::test]
async fn a_stale_cursor_is_dropped_and_the_backfill_reads_on() {
    let rig = Rig::new();
    for days_ago in 1..=4 {
        rig.slack.post(
            "C1",
            person(NOW - days_ago * DAY, "U1", &format!("m{days_ago}")),
        );
    }
    rig.index_channel("C1", false);
    let worker = rig.worker();
    assert!(matches!(
        worker.step().await.unwrap(),
        Step::Worked(Work::BackfillPage { done: false, .. })
    ));
    rig.slack.fail_next(
        "history",
        SlackApiError::Api {
            code: "invalid_cursor".into(),
            needed: None,
        },
    );
    assert_eq!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Restarted {
            channel: "C1".into()
        })
    );
    let restarted = rig.channel("C1");
    assert!(restarted.backfill_cursor.is_none());
    assert_eq!(
        restarted.backfill_latest.as_deref(),
        Some("1792832400.000000"),
        "the window is kept"
    );
    assert!(restarted.paused_until.is_none(), "no pause");
    run_until_idle(&worker).await;
    assert!(rig.channel("C1").backfill_done);
    assert_eq!(rig.index.sources().len(), 4);
    assert_eq!(
        rig.index.puts.lock().unwrap().len(),
        4,
        "the page read again skips the threads that did not change"
    );
}

#[tokio::test]
async fn a_deleted_sole_reply_is_caught_by_the_slim_pass() {
    let rig = Rig::new();
    rig.slack.thread(
        "C1",
        NOW - DAY,
        &[(NOW - DAY + 10, "U2", "a reply to forget")],
    );
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    assert!(rig
        .index
        .doc("C1", NOW - DAY)
        .unwrap()
        .text
        .contains("a reply to forget"));
    // Deleted while Shodh was not running: the thread has no reply now.
    rig.slack.thread("C1", NOW - DAY, &[]);
    rig.clock.advance(chrono::Duration::hours(7));
    run_until_idle(&worker).await;
    let doc = rig.index.doc("C1", NOW - DAY).unwrap();
    assert!(!doc.text.contains("a reply to forget"), "{}", doc.text);
}

#[tokio::test]
async fn a_thread_only_bots_wrote_is_not_fetched_again_every_slim_pass() {
    let rig = Rig::new();
    bot_thread(&rig.slack, "C1", NOW - DAY);
    rig.slack.post("C1", person(NOW - 2 * DAY, "U1", "hello"));
    rig.index_channel("C1", false);
    let worker = rig.worker();
    run_until_idle(&worker).await;
    let replies = |rig: &Rig| {
        rig.slack
            .calls(&format!("replies C1 {}", ts(NOW - DAY)))
            .len()
    };
    assert_eq!(replies(&rig), 1, "read once by the backfill");
    assert!(rig.index.doc("C1", NOW - DAY).is_none(), "nothing to index");
    for _ in 0..2 {
        rig.clock.advance(chrono::Duration::hours(7));
        run_until_idle(&worker).await;
    }
    assert_eq!(replies(&rig), 1, "unchanged: never fetched again");
}

#[tokio::test]
async fn an_archived_channel_slack_reports_is_archived_not_removed() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U1", "hello"));
    rig.index_channel("C1", false);
    rig.slack.fail_next(
        "history",
        SlackApiError::Api {
            code: "is_archived".into(),
            needed: None,
        },
    );
    assert_eq!(
        rig.worker().step().await.unwrap(),
        Step::Worked(Work::Stopped {
            channel: "C1".into(),
            state: ChannelState::Archived
        })
    );
    let channel = rig.channel("C1");
    assert_eq!(channel.state, ChannelState::Archived);
    assert_eq!(channel.error.as_deref(), Some(ARCHIVED));
}

#[tokio::test]
async fn a_rate_limit_on_a_name_pauses_and_the_thread_keeps_its_names() {
    let rig = Rig::new();
    rig.slack.post("C1", person(NOW - DAY, "U2", "hello"));
    rig.index_channel("C1", false);
    rig.slack.fail_next(
        "user",
        SlackApiError::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        },
    );
    let worker = rig.worker();
    assert!(matches!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Paused { .. })
    ));
    assert!(
        rig.index.sources().is_empty(),
        "not indexed without the name Slack would not give"
    );
    rig.clock.advance(chrono::Duration::seconds(31));
    run_until_idle(&worker).await;
    let doc = rig.index.doc("C1", NOW - DAY).unwrap();
    assert!(doc.text.contains("Bob"), "{}", doc.text);
    assert!(!doc.text.contains("Someone"), "{}", doc.text);
}

#[tokio::test]
async fn a_channel_shared_during_its_backfill_stops_before_the_next_page() {
    let rig = Rig::new();
    for days_ago in 1..=4 {
        rig.slack.post(
            "C1",
            person(NOW - days_ago * DAY, "U1", &format!("m{days_ago}")),
        );
    }
    rig.index_channel("C1", false);
    let worker = rig.worker();
    assert!(matches!(
        worker.step().await.unwrap(),
        Step::Worked(Work::BackfillPage { done: false, .. })
    ));
    rig.slack.infos.lock().unwrap().insert(
        "C1".into(),
        ChannelInfo {
            name: "c1".into(),
            archived: false,
            shared: true,
            private: false,
        },
    );
    rig.clock.advance(chrono::Duration::minutes(16));
    assert_eq!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Stopped {
            channel: "C1".into(),
            state: ChannelState::NowShared
        })
    );
    assert_eq!(
        rig.index.sources().len(),
        PAGE,
        "the next page was not read"
    );
}

#[tokio::test]
async fn a_waiting_forget_stops_a_page_at_its_next_thread() {
    let rig = Rig::new();
    for days_ago in 1..=2 {
        rig.slack.post(
            "C1",
            person(NOW - days_ago * DAY, "U1", &format!("m{days_ago}")),
        );
    }
    rig.index_channel("C1", false);
    *rig.index.forget_on_put.lock().unwrap() = Some(rig.gate.clone());
    let worker = rig.worker();
    assert_eq!(
        worker.step().await.unwrap(),
        Step::Worked(Work::Yielded {
            channel: "C1".into()
        })
    );
    assert_eq!(
        rig.index.puts.lock().unwrap().len(),
        1,
        "one thread, not the page"
    );
    let channel = rig.channel("C1");
    assert!(
        channel.backfill_latest.is_none() && !channel.backfill_done,
        "nothing of the page was checkpointed"
    );
    // The forget had its turn; then the page is read again, the thread done skipped.
    let waiting = rig.index.waiting_forget.lock().unwrap().take().unwrap();
    waiting.await.unwrap();
    run_until_idle(&worker).await;
    assert_eq!(rig.index.sources().len(), 2);
    assert_eq!(rig.index.puts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_channel_goes_into_a_workspace_source_first_and_takes_it_back_when_it_cannot() {
    let rig = Rig::new();
    let workspaces = crate::workspaces::WorkspaceStore::open(&rig.db, None).unwrap();
    let workspace = workspaces
        .create(
            &crate::workspaces::NewWorkspace {
                name: "Clients".into(),
                ..Default::default()
            },
            crate::workspaces::WorkspaceAuthor::User,
        )
        .unwrap();
    let new = |workspace_id: &str| NewIndexedChannel {
        team_id: "T1".into(),
        channel_id: "C1".into(),
        name: "c1".into(),
        workspace_id: workspace_id.into(),
        shared: false,
        private: false,
        window_days: 90,
    };
    let sources = |workspaces: &crate::workspaces::WorkspaceStore| {
        workspaces
            .detail(&workspace.id)
            .unwrap()
            .sources
            .into_iter()
            .map(|s| s.reference)
            .collect::<Vec<_>>()
    };
    let label = "#c1 (Slack)";
    // A workspace that does not exist: no channel indexes into a space nobody holds.
    assert!(choose_channel(
        &rig.store,
        &workspaces,
        &new("ws-gone"),
        label,
        rig.clock.now()
    )
    .is_err());
    assert!(rig.store.channel("T1", "C1").unwrap().is_none());
    // A channel being forgotten cannot be chosen: the source this call added goes again.
    rig.index_channel("C1", false);
    rig.store
        .set_state("T1", "C1", ChannelState::Forgetting, None)
        .unwrap();
    assert!(choose_channel(
        &rig.store,
        &workspaces,
        &new(&workspace.id),
        label,
        rig.clock.now()
    )
    .is_err());
    assert!(sources(&workspaces).is_empty(), "taken back");
    // Chosen: the workspace holds its space.
    rig.store.forget_channel("T1", "C1").unwrap();
    let chosen = choose_channel(
        &rig.store,
        &workspaces,
        &new(&workspace.id),
        label,
        rig.clock.now(),
    )
    .unwrap();
    assert!(chosen.before.is_none());
    assert_eq!(chosen.channel.workspace_id, workspace.id);
    assert_eq!(sources(&workspaces), vec![ids::space_id("T1", "C1")]);
}

/// Embeds as [`crate::statements::testing::WordEmbedder`]; while held, a batch of
/// documents waits until released, saying so first.
#[derive(Default)]
struct HeldEmbedder {
    inner: crate::statements::testing::WordEmbedder,
    held: Mutex<bool>,
    released: std::sync::Condvar,
    entered: Notify,
}

impl HeldEmbedder {
    fn hold(&self) {
        *self.held.lock().unwrap() = true;
    }
    fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.released.notify_all();
    }
}

impl crate::embeddings::EmbeddingModel for HeldEmbedder {
    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.inner.embed_query(text)
    }
    fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.inner.embed_document(text)
    }
    fn embed_documents(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut held = self.held.lock().unwrap();
        if *held {
            self.entered.notify_one();
            while *held {
                held = self.released.wait(held).unwrap();
            }
        }
        drop(held);
        texts.iter().map(|t| self.inner.embed_document(t)).collect()
    }
    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_put_embeds_outside_the_index_lock_and_keeps_the_gate_when_its_step_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let embedder = Arc::new(HeldEmbedder::default());
    let rag = Box::pin(engine_with(dir.path(), embedder.clone())).await;
    let index = Arc::new(EngineIndex { rag: rag.clone() });
    let gate = Arc::new(SyncGate::default());
    let thread = doc("C1", NOW, "the walrus budget is ten");
    embedder.hold();
    let step = {
        let (index, gate, thread) = (index.clone(), gate.clone(), thread.clone());
        tokio::spawn(async move {
            let turn = gate.turn().await;
            index.put(&thread, &turn).await
        })
    };
    embedder.entered.notified().await;
    // Embedding holds no lock on the index: Ask and Slack answers search meanwhile.
    assert!(rag.try_write().is_ok(), "the index is free while embedding");
    // The step is cancelled (a disconnect): its put goes on and keeps the gate, so a
    // forget that follows cannot run before the thread is stored.
    step.abort();
    assert!(step.await.unwrap_err().is_cancelled());
    assert!(
        gate.lock.try_lock().is_err(),
        "the put keeps the gate after its step was cancelled"
    );
    embedder.release();
    let _forget = gate.urgent_turn().await;
    assert_eq!(
        rag.read()
            .await
            .document_sources()
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.source == thread.source)
            .count(),
        1,
        "stored whole before the forget got the gate"
    );
    index
        .delete_space(&ids::space_id("T1", "C1"))
        .await
        .unwrap();
    assert_eq!(
        chunks_of(&rag, &thread.source).await,
        0,
        "and then forgotten"
    );
}

#[tokio::test]
async fn a_thread_put_again_without_its_record_keeps_one_document() {
    // A step cancelled after its put and before its record leaves the document stored
    // and unrecorded: the step that resumes puts the thread again, not knowing.
    let dir = tempfile::tempdir().unwrap();
    let rag = Box::pin(engine(dir.path())).await;
    let index = EngineIndex { rag: rag.clone() };
    let gate = SyncGate::default();
    let turn = gate.turn().await;
    let thread = doc("C1", NOW, "the walrus budget is ten");
    index.put(&thread, &turn).await.unwrap();
    let once = chunks_of(&rag, &thread.source).await;
    assert!(once > 0);
    index.put(&thread, &turn).await.unwrap();
    let rows: Vec<_> = rag
        .read()
        .await
        .document_sources()
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.source == thread.source)
        .collect();
    assert_eq!(rows.len(), 1, "one document: {rows:?}");
    assert_eq!(chunks_of(&rag, &thread.source).await, once);
}

#[tokio::test]
async fn a_source_in_two_spaces_is_listed_in_each() {
    let dir = tempfile::tempdir().unwrap();
    let rag = Box::pin(engine(dir.path())).await;
    // The space read first is not the one listed.
    for space in ["space-b", "space-a"] {
        let mut metadata = HashMap::new();
        metadata.insert("space_id".to_string(), space.to_string());
        metadata.insert("file_path".to_string(), "C:/docs/lease.pdf".to_string());
        metadata.insert("title".to_string(), "Lease".to_string());
        rag.write()
            .await
            .add_document(
                "the notice period is sixty days",
                crate::types::DocumentFormat::TXT,
                metadata,
                Citation::default(),
            )
            .await
            .unwrap();
    }
    let titles = rag.read().await.document_titles().await.unwrap();
    let mut spaces: Vec<&str> = titles
        .iter()
        .filter(|t| t.source == "C:/docs/lease.pdf")
        .map(|t| t.space_id.as_str())
        .collect();
    spaces.sort_unstable();
    assert_eq!(spaces, vec!["space-a", "space-b"]);
    let listing = crate::connectors::slack::answer::library_listing(
        rag.clone(),
        &crate::connectors::slack::answer::AnswerScope {
            source_ids: vec!["space-a".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(listing.titles.len(), 1, "the exact count: {listing:?}");
}

/// Writes the owner's memories from Slack into a real memory service.
struct ServiceWriter(Arc<crate::user_memory::MemoryService>);

#[async_trait]
impl crate::connectors::slack::remember::MemoryWriter for ServiceWriter {
    async fn write(
        &self,
        memory: &crate::connectors::slack::store::SlackMemory,
    ) -> Result<Option<String>, String> {
        let origin = crate::user_memory::Origin::from_slack(
            &ids::memory_source(&memory.team_id, &memory.channel_id, &memory.message_ts),
            shodh_ontology::ExtractorKind::User,
            "test",
            crate::user_memory::WriteAuthority::SlackOwner {
                team_id: memory.team_id.clone(),
                user_id: memory.author_id.clone(),
            },
        );
        let outcome = self
            .0
            .remember(
                crate::user_memory::MemoryContent::Note {
                    text: memory.text.clone(),
                },
                crate::statements::Scope::Global,
                &origin,
                &crate::user_memory::Actor::ui(),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(outcome.created_id().map(str::to_string))
    }

    async fn erase(&self, id: &str) -> Result<(), String> {
        self.0
            .forget(id, None, &crate::user_memory::Actor::ui())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Forgets in a real memory service, as the app does (a memory already gone is done).
struct ServiceObserver(Arc<crate::user_memory::MemoryService>);

#[async_trait]
impl SyncObserver for ServiceObserver {
    fn changed(&self) {}
    async fn remove_from_workspace(&self, _channel: &IndexedChannel) -> Result<(), String> {
        Ok(())
    }
    async fn forget_memories(&self, memory_ids: &[String]) -> Result<(), String> {
        for id in memory_ids {
            match self
                .0
                .forget(id, None, &crate::user_memory::Actor::ui())
                .await
            {
                Ok(_) | Err(crate::user_memory::MemoryError::NotFound(_)) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn forgetting_a_channel_keeps_the_users_own_note_the_owner_repeated_in_slack() {
    let rig = Rig::new();
    let fixture = crate::statements::testing::fixture().await;
    let service = Arc::new(crate::user_memory::MemoryService::new(
        fixture.store.clone(),
        None,
        "test",
    ));
    // The user's own note, from Settings.
    let users = service
        .remember(
            crate::user_memory::MemoryContent::Note {
                text: "The budget review is on Friday".into(),
            },
            crate::statements::Scope::Global,
            &crate::user_memory::Origin::user_interface("test"),
            &crate::user_memory::Actor::ui(),
        )
        .await
        .unwrap()
        .memory
        .unwrap()
        .id;
    rig.index_channel("C1", false);
    let owner_memory =
        |id: &str, text: &str, at: i64| crate::connectors::slack::store::SlackMemory {
            id: id.into(),
            team_id: "T1".into(),
            channel_id: "C1".into(),
            channel_name: "#c1".into(),
            author_id: "U1".into(),
            author_name: "Ann".into(),
            text: text.into(),
            kind: crate::connectors::slack::store::SlackMemoryKind::Text,
            message_ts: ts(at),
            permalink: None,
            saved_at: rig.clock.now(),
            owner: true,
            memory_id: None,
        };
    let writer = ServiceWriter(service.clone());
    // The owner says the same in Slack, then something new.
    crate::connectors::slack::remember::save_owner_memory(
        &rig.store,
        &writer,
        owner_memory("s1", "The budget review is on Friday", NOW),
    )
    .await
    .unwrap();
    crate::connectors::slack::remember::save_owner_memory(
        &rig.store,
        &writer,
        owner_memory("s2", "The walrus contract renews in March", NOW + 1),
    )
    .await
    .unwrap();
    let from_slack = rig
        .store
        .memory("s2")
        .unwrap()
        .unwrap()
        .memory_id
        .expect("what Slack added is linked");
    assert_eq!(
        rig.store.memory("s1").unwrap().unwrap().memory_id,
        None,
        "the user's own note is not linked"
    );
    let channel = rig.channel("C1");
    forget_channel(
        &rig.store,
        rig.index.as_ref(),
        &ServiceObserver(service.clone()),
        &channel,
    )
    .await
    .unwrap();
    assert!(
        service.get(&users).await.is_ok(),
        "the user's own note stays after the channel is forgotten"
    );
    assert!(
        matches!(
            service.get(&from_slack).await,
            Err(crate::user_memory::MemoryError::NotFound(_))
        ),
        "what Slack added is forgotten"
    );
    assert!(rig.store.memories("T1").unwrap().is_empty());
}
