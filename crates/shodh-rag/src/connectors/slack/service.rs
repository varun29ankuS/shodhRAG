//! Envelopes in, answers out: routes each acknowledged envelope and runs the answers.
//!
//! Bounded on every side: at most [`CONCURRENT_ANSWERS`] answers run at once, at most
//! [`MAX_WAITING_ANSWERS`] wait for their turn (a question beyond that is told to ask
//! again later, with no model call), and at most [`MAX_TASKS`] tasks exist at all (beyond
//! that an event is dropped and logged: a flood must not turn into a flood of posts).
//! Every task stops when the installation's cancellation token is cancelled, also while
//! it waits for its turn.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::sync::Semaphore;

use super::events::{Envelope, EventRouter, Routed, SeenEntry, SeenStore};
use super::index::{index_event, IndexEvents};
use super::respond::{Outcome, Responder};
use super::socket::EnvelopeSink;

/// Answers written at the same time, across every thread. Each costs a model call; more
/// at once would only meet the provider's rate limit sooner.
pub const CONCURRENT_ANSWERS: usize = 2;
/// Answers waiting for their turn; a question beyond that is told to ask again later.
pub const MAX_WAITING_ANSWERS: usize = 6;
/// Tasks (answers, waiting answers, short replies) at once; beyond that events are
/// dropped.
pub const MAX_TASKS: usize = 16;

/// Hears how every question ended (the app shows a failed post on the card).
pub trait OutcomeSink: Send + Sync {
    fn handled(&self, outcome: &Outcome);
}

/// Writes the router's seen event ids through a [`SeenStore`], newest snapshot only.
pub struct SeenPersister {
    store: Arc<dyn SeenStore>,
    written: Mutex<u64>,
}

impl SeenPersister {
    pub fn new(store: Arc<dyn SeenStore>) -> Self {
        Self {
            store,
            written: Mutex::new(0),
        }
    }

    /// The ids kept (none when they cannot be read: dedupe then starts empty).
    pub fn load(&self) -> Vec<SeenEntry> {
        self.store.load().unwrap_or_else(|e| {
            tracing::warn!(target: "shodh::slack", error = %e, "the seen Slack events could not be read");
            Vec::new()
        })
    }

    /// Writes `entries` unless a newer generation was written already. Blocking.
    pub fn write(&self, generation: u64, entries: &[SeenEntry]) {
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        if generation <= *written {
            return;
        }
        match self.store.save(entries) {
            Ok(()) => *written = generation,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the seen Slack events could not be saved")
            }
        }
    }
}

/// One place in a bounded count, given back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A place in `count` if it is below `max`.
fn take_slot(count: &Arc<AtomicUsize>, max: usize) -> Option<Slot> {
    count
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < max).then_some(n + 1)
        })
        .ok()
        .map(|_| Slot(count.clone()))
}

/// Routes envelopes ([`EventRouter`]) and answers questions ([`Responder`]), each on its
/// own task, so the connection reads (and acknowledges) the next envelope at once.
pub struct AnswerService {
    router: Arc<EventRouter>,
    responder: Arc<Responder>,
    permits: Arc<Semaphore>,
    runtime: tokio::runtime::Handle,
    outcomes: Arc<dyn OutcomeSink>,
    seen: Option<Arc<SeenPersister>>,
    /// The live updates of the indexed channels.
    index: Option<Arc<IndexEvents>>,
    tasks: Arc<AtomicUsize>,
    waiting: Arc<AtomicUsize>,
}

impl AnswerService {
    /// A service routing with `router` (kept across restarts by the app), answering
    /// with `responder` on `runtime`.
    pub fn new(
        router: Arc<EventRouter>,
        responder: Responder,
        runtime: tokio::runtime::Handle,
        outcomes: Arc<dyn OutcomeSink>,
    ) -> Self {
        Self {
            router,
            responder: Arc::new(responder),
            permits: Arc::new(Semaphore::new(CONCURRENT_ANSWERS)),
            runtime,
            outcomes,
            seen: None,
            index: None,
            tasks: Arc::new(AtomicUsize::new(0)),
            waiting: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Keeps the seen event ids on disk after every new one.
    pub fn with_seen_store(mut self, seen: Arc<SeenPersister>) -> Self {
        self.seen = Some(seen);
        self
    }

    /// Sends the indexed channels' events (new messages, edits, deletions, archives) to
    /// `index`.
    pub fn with_index_events(mut self, index: Arc<IndexEvents>) -> Self {
        self.index = Some(index);
        self
    }

    pub fn router(&self) -> &EventRouter {
        &self.router
    }

    /// Tasks running or waiting now.
    pub fn tasks(&self) -> usize {
        self.tasks.load(Ordering::SeqCst)
    }

    fn persist_seen(&self) {
        let Some(seen) = &self.seen else {
            return;
        };
        let (generation, entries) = self.router.seen_snapshot(Utc::now());
        let seen = seen.clone();
        // Off the async workers: a small file write.
        self.runtime
            .spawn_blocking(move || seen.write(generation, &entries));
    }

    /// Runs `work` on its own task, if there is room for one.
    fn run<F>(&self, work: F)
    where
        F: Future<Output = Outcome> + Send + 'static,
    {
        let Some(task) = take_slot(&self.tasks, MAX_TASKS) else {
            tracing::warn!(target: "shodh::slack", "too many Slack questions at once; one was dropped");
            return;
        };
        let outcomes = self.outcomes.clone();
        self.runtime.spawn(async move {
            let _task = task;
            let outcome = work.await;
            log_outcome(&outcome);
            outcomes.handled(&outcome);
        });
    }
}

fn log_outcome(outcome: &Outcome) {
    match outcome {
        Outcome::SlackFailed(e) => {
            tracing::warn!(target: "shodh::slack", error = %e, "Slack refused the reply")
        }
        Outcome::Cancelled => {
            tracing::debug!(target: "shodh::slack", "a Slack answer was cancelled")
        }
        other => tracing::info!(target: "shodh::slack", outcome = ?other, "Slack question handled"),
    }
}

impl EnvelopeSink for AnswerService {
    fn deliver(&self, envelope: Envelope) {
        if let Some(index) = &self.index {
            if let Some(event) = index_event(&envelope, self.router.bot()) {
                let index = index.clone();
                // Off the async workers: a database write.
                self.runtime.spawn_blocking(move || {
                    if let Err(e) = index.apply(&event) {
                        tracing::warn!(target: "shodh::slack", error = %e, "a Slack change could not be recorded for the index");
                    }
                });
            }
        }
        let routed = self.router.route(&envelope);
        if !matches!(routed, Routed::Ignore(_)) {
            self.persist_seen();
        }
        match routed {
            Routed::Ignore(why) => {
                tracing::debug!(target: "shodh::slack", why, "Slack event ignored");
            }
            Routed::Busy(question) => {
                let responder = self.responder.clone();
                self.run(async move { responder.busy(&question).await });
            }
            Routed::Ask(question, claim) => {
                let responder = self.responder.clone();
                let Some(waiting) = take_slot(&self.waiting, MAX_WAITING_ANSWERS) else {
                    drop(claim);
                    self.run(async move { responder.overloaded(&question).await });
                    return;
                };
                let permits = self.permits.clone();
                self.run(async move {
                    // The thread stays claimed while the answer waits for its turn.
                    let _claim = claim;
                    let permit = tokio::select! {
                        permit = permits.acquire_owned() => permit,
                        _ = responder.cancel_token().cancelled() => return Outcome::Cancelled,
                    };
                    drop(waiting);
                    let Ok(_permit) = permit else {
                        return Outcome::Cancelled;
                    };
                    responder.respond(&question).await
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::connectors::slack::answer::{
        AnswerPassage, AnswerScope, LibraryListing, ThreadTurn,
    };
    use crate::connectors::slack::events::{BotIdentity, ThreadKey};
    use crate::connectors::slack::respond::testkit::*;
    use crate::connectors::slack::respond::{AnswerSource, BUSY, OVERLOADED, PLACEHOLDER};
    use crate::connectors::slack::sanitize::PrivateTerms;
    use crate::connectors::slack::store::SlackMemory;
    use crate::research::results::TextModel;

    /// A source whose search waits until released, so a second question meets the first
    /// one in flight.
    struct SlowSource {
        release: tokio::sync::Notify,
        model: Arc<RecordingModel>,
    }

    #[async_trait]
    impl AnswerSource for SlowSource {
        async fn scope(&self) -> Result<Option<AnswerScope>, String> {
            Ok(Some(AnswerScope::default()))
        }
        async fn retrieve(&self, _s: &AnswerScope, _q: &str) -> Result<Vec<AnswerPassage>, String> {
            self.release.notified().await;
            Ok(vec![AnswerPassage {
                n: 1,
                title: "a".into(),
                page: None,
                text: "Sixty days.".into(),
            }])
        }
        async fn listing(&self, _s: &AnswerScope) -> Result<LibraryListing, String> {
            Ok(LibraryListing::default())
        }
        async fn thread_turns(&self, _t: &ThreadKey) -> Result<Vec<ThreadTurn>, String> {
            Ok(Vec::new())
        }
        async fn record_turn(&self, _t: &ThreadKey, _turn: ThreadTurn) -> Result<(), String> {
            Ok(())
        }
        async fn channel_memories(&self, _c: &str) -> Result<Vec<SlackMemory>, String> {
            Ok(Vec::new())
        }
        fn model(&self) -> Result<Arc<dyn TextModel>, String> {
            Ok(self.model.clone())
        }
        fn private_terms(&self) -> PrivateTerms {
            PrivateTerms::default()
        }
    }

    #[derive(Default)]
    struct Outcomes(Mutex<Vec<Outcome>>);
    impl OutcomeSink for Outcomes {
        fn handled(&self, outcome: &Outcome) {
            self.0.lock().unwrap().push(outcome.clone());
        }
    }

    #[derive(Default)]
    struct MemorySeen(Mutex<Vec<Vec<SeenEntry>>>);
    impl SeenStore for MemorySeen {
        fn load(&self) -> Result<Vec<SeenEntry>, String> {
            Ok(self.0.lock().unwrap().last().cloned().unwrap_or_default())
        }
        fn save(&self, entries: &[SeenEntry]) -> Result<(), String> {
            self.0.lock().unwrap().push(entries.to_vec());
            Ok(())
        }
    }

    fn mention(envelope: &str, event: &str, ts: &str, thread: Option<&str>) -> Envelope {
        let mut ev = json!({"type": "app_mention", "user": "U1", "text": "<@UBOT> notice?", "ts": ts, "channel": "C1"});
        if let Some(thread) = thread {
            ev["thread_ts"] = json!(thread);
        }
        Envelope {
            envelope_id: envelope.into(),
            kind: "events_api".into(),
            payload: json!({"type": "event_callback", "event_id": event, "event": ev}),
            retry_attempt: 0,
        }
    }

    struct Rig {
        service: AnswerService,
        slack: Arc<RecordingSlack>,
        model: Arc<RecordingModel>,
        source: Arc<SlowSource>,
        outcomes: Arc<Outcomes>,
        cancel: CancellationToken,
    }

    fn rig() -> Rig {
        let slack = Arc::new(RecordingSlack::default());
        let model = Arc::new(RecordingModel::answering("Sixty days [1]."));
        let source = Arc::new(SlowSource {
            release: tokio::sync::Notify::new(),
            model: model.clone(),
        });
        let outcomes = Arc::new(Outcomes::default());
        let cancel = CancellationToken::new();
        let service = AnswerService::new(
            Arc::new(EventRouter::new(BotIdentity {
                user_id: "UBOT".into(),
                bot_id: None,
            })),
            Responder::new(
                source.clone(),
                memories(),
                open_gate(),
                Arc::new(MemoryQuota::new(10)),
                slack.clone(),
                Arc::new(FixedClock(
                    Utc.with_ymd_and_hms(2026, 10, 23, 9, 0, 0).unwrap(),
                )),
                cancel.clone(),
            ),
            tokio::runtime::Handle::current(),
            outcomes.clone(),
        );
        Rig {
            service,
            slack,
            model,
            source,
            outcomes,
            cancel,
        }
    }

    async fn settle(what: &str, done: impl Fn() -> bool) {
        for _ in 0..400 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn a_second_question_in_a_thread_waits_its_turn_and_costs_nothing() {
        let r = rig();
        r.service.deliver(mention("E1", "Ev1", "100.1", None));
        settle("the placeholder", || {
            r.slack.messages.lock().unwrap().len() >= 1
        })
        .await;
        // The same question again (a retry) and a follow-up in the same thread.
        r.service.deliver(mention("E1b", "Ev1", "100.1", None));
        r.service
            .deliver(mention("E2", "Ev2", "100.5", Some("100.1")));
        settle("the busy reply", || {
            r.slack.messages.lock().unwrap().len() >= 2
        })
        .await;
        r.source.release.notify_one();
        settle("the answer", || {
            r.model.calls() == 1 && r.service.router().in_flight().is_empty()
        })
        .await;
        let texts = r.slack.texts();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].starts_with("Sixty days [1]."), "{texts:?}");
        assert_eq!(texts[1], BUSY);
        assert_ne!(texts[0], PLACEHOLDER);
        assert_eq!(r.model.calls(), 1, "one model call for one question");
        settle("the tasks", || r.service.tasks() == 0).await;
        let outcomes = r.outcomes.0.lock().unwrap().clone();
        assert!(outcomes.contains(&Outcome::Busy) && outcomes.contains(&Outcome::Answered));
    }

    #[tokio::test]
    async fn switching_off_stops_queued_and_running_answers() {
        let r = rig();
        // Two answers run (both searching), a third waits for its turn.
        for (i, ts) in ["100.1", "200.1", "300.1"].iter().enumerate() {
            r.service
                .deliver(mention(&format!("E{i}"), &format!("Ev{i}"), ts, None));
        }
        settle("two placeholders", || {
            r.slack.messages.lock().unwrap().len() >= 2
        })
        .await;
        assert_eq!(r.service.router().in_flight().len(), 3);
        r.cancel.cancel();
        // The waiting one leaves at once, without a post.
        settle("the waiting answer to leave", || {
            r.service.router().in_flight().len() == 2
        })
        .await;
        // The running ones finish their search, then make no call and post nothing more.
        r.source.release.notify_waiters();
        settle("every task", || r.service.tasks() == 0).await;
        assert_eq!(r.model.calls(), 0);
        assert_eq!(
            r.slack.texts(),
            vec![PLACEHOLDER.to_string(), PLACEHOLDER.to_string()]
        );
        assert!(r.service.router().in_flight().is_empty());
        let outcomes = r.outcomes.0.lock().unwrap().clone();
        assert_eq!(outcomes, vec![Outcome::Cancelled; 3]);
    }

    #[tokio::test]
    async fn too_many_waiting_questions_are_told_to_ask_later() {
        let r = rig();
        let ask = |i: usize| {
            r.service.deliver(mention(
                &format!("E{i}"),
                &format!("Ev{i}"),
                &format!("{}.1", 100 + i),
                None,
            ))
        };
        // Two answers take their turn (and are searching), six more wait, one too many.
        for i in 0..CONCURRENT_ANSWERS {
            ask(i);
        }
        settle("the running answers", || {
            r.slack.messages.lock().unwrap().len() >= CONCURRENT_ANSWERS
        })
        .await;
        for i in CONCURRENT_ANSWERS..CONCURRENT_ANSWERS + MAX_WAITING_ANSWERS + 1 {
            ask(i);
        }
        settle("the overload reply", || {
            r.slack.texts().iter().any(|t| t == OVERLOADED)
        })
        .await;
        let texts = r.slack.texts();
        assert_eq!(texts.iter().filter(|t| *t == OVERLOADED).count(), 1);
        assert_eq!(
            texts.iter().filter(|t| *t == PLACEHOLDER).count(),
            CONCURRENT_ANSWERS
        );
        assert_eq!(r.model.calls(), 0);
        r.cancel.cancel();
        r.source.release.notify_waiters();
        settle("every task", || r.service.tasks() == 0).await;
    }

    #[test]
    fn slots_are_bounded_and_given_back() {
        let count = Arc::new(AtomicUsize::new(0));
        let a = take_slot(&count, 2).unwrap();
        let b = take_slot(&count, 2).unwrap();
        assert!(take_slot(&count, 2).is_none());
        drop(a);
        let c = take_slot(&count, 2).unwrap();
        drop((b, c));
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn new_event_ids_are_kept_on_disk() {
        let r = rig();
        let store = Arc::new(MemorySeen::default());
        let service = r
            .service
            .with_seen_store(Arc::new(SeenPersister::new(store.clone())));
        service.deliver(mention("E1", "Ev1", "100.1", None));
        settle("the save", || !store.0.lock().unwrap().is_empty()).await;
        let saved = store.load().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, "Ev1");
        r.cancel.cancel();
        r.source.release.notify_waiters();
        settle("every task", || service.tasks() == 0).await;
    }

    #[test]
    fn an_older_snapshot_never_replaces_a_newer_one() {
        let store = Arc::new(MemorySeen::default());
        let persister = SeenPersister::new(store.clone());
        let entry = |id: &str| SeenEntry {
            id: id.into(),
            at: Utc.with_ymd_and_hms(2026, 10, 23, 9, 0, 0).unwrap(),
        };
        persister.write(2, &[entry("a"), entry("b")]);
        persister.write(1, &[entry("a")]);
        persister.write(2, &[entry("a")]);
        assert_eq!(store.load().unwrap(), vec![entry("a"), entry("b")]);
        persister.write(3, &[entry("c")]);
        assert_eq!(store.load().unwrap(), vec![entry("c")]);
    }
}
