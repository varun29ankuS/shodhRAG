//! Envelopes in, answers out: routes each acknowledged envelope and runs the answers.

use std::sync::Arc;

use tokio::sync::Semaphore;

use super::events::{BotIdentity, Envelope, EventRouter, Routed};
use super::respond::{Outcome, Responder};
use super::socket::EnvelopeSink;

/// Answers written at the same time, across every thread. Each costs a model call; more
/// at once would only meet the provider's rate limit sooner.
pub const CONCURRENT_ANSWERS: usize = 2;

/// Routes envelopes ([`EventRouter`]) and answers questions ([`Responder`]), each on its
/// own task, so the connection reads (and acknowledges) the next envelope at once.
pub struct AnswerService {
    router: EventRouter,
    responder: Arc<Responder>,
    permits: Arc<Semaphore>,
    runtime: tokio::runtime::Handle,
}

impl AnswerService {
    /// A service for the bot `bot`, running answers on `runtime`.
    pub fn new(bot: BotIdentity, responder: Responder, runtime: tokio::runtime::Handle) -> Self {
        Self {
            router: EventRouter::new(bot),
            responder: Arc::new(responder),
            permits: Arc::new(Semaphore::new(CONCURRENT_ANSWERS)),
            runtime,
        }
    }

    pub fn router(&self) -> &EventRouter {
        &self.router
    }
}

fn log_outcome(outcome: &Outcome) {
    match outcome {
        Outcome::SlackFailed(e) => {
            tracing::warn!(target: "shodh::slack", error = %e, "Slack refused the reply")
        }
        other => tracing::info!(target: "shodh::slack", outcome = ?other, "Slack question handled"),
    }
}

impl EnvelopeSink for AnswerService {
    fn deliver(&self, envelope: Envelope) {
        match self.router.route(&envelope) {
            Routed::Ignore(why) => {
                tracing::debug!(target: "shodh::slack", why, "Slack event ignored");
            }
            Routed::Busy(question) => {
                let responder = self.responder.clone();
                self.runtime.spawn(async move {
                    log_outcome(&responder.busy(&question).await);
                });
            }
            Routed::Ask(question, claim) => {
                let responder = self.responder.clone();
                let permits = self.permits.clone();
                self.runtime.spawn(async move {
                    // The thread stays claimed while the answer waits for a permit.
                    let _claim = claim;
                    let Ok(_permit) = permits.acquire_owned().await else {
                        return;
                    };
                    log_outcome(&responder.respond(&question).await);
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

    use super::*;
    use crate::connectors::slack::answer::{AnswerPassage, AnswerScope};
    use crate::connectors::slack::respond::testkit::*;
    use crate::connectors::slack::respond::{AnswerSource, BUSY, PLACEHOLDER};
    use crate::connectors::slack::sanitize::PrivateTerms;
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
                title: "a.pdf".into(),
                page: None,
                text: "Sixty days.".into(),
            }])
        }
        fn model(&self) -> Result<Arc<dyn TextModel>, String> {
            Ok(self.model.clone())
        }
        fn private_terms(&self) -> PrivateTerms {
            PrivateTerms::default()
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

    async fn settle(slack: &RecordingSlack, messages: usize) {
        for _ in 0..200 {
            if slack.messages.lock().unwrap().len() >= messages {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("expected {messages} messages");
    }

    #[tokio::test]
    async fn a_second_question_in_a_thread_waits_its_turn_and_costs_nothing() {
        let slack = Arc::new(RecordingSlack::default());
        let model = Arc::new(RecordingModel::answering("Sixty days [1]."));
        let source = Arc::new(SlowSource {
            release: tokio::sync::Notify::new(),
            model: model.clone(),
        });
        let quota = Arc::new(MemoryQuota::new(10));
        let service = AnswerService::new(
            BotIdentity {
                user_id: "UBOT".into(),
                bot_id: None,
            },
            Responder {
                source: source.clone(),
                quota,
                poster: slack.clone(),
                clock: Arc::new(FixedClock(
                    Utc.with_ymd_and_hms(2026, 10, 23, 9, 0, 0).unwrap(),
                )),
            },
            tokio::runtime::Handle::current(),
        );
        service.deliver(mention("E1", "Ev1", "100.1", None));
        settle(&slack, 1).await;
        // The same question again (a retry) and a follow-up in the same thread.
        service.deliver(mention("E1b", "Ev1", "100.1", None));
        service.deliver(mention("E2", "Ev2", "100.5", Some("100.1")));
        settle(&slack, 2).await;
        source.release.notify_one();
        for _ in 0..200 {
            if model.calls() == 1 && service.router().in_flight().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let texts = slack.texts();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].starts_with("Sixty days [1]."), "{texts:?}");
        assert_eq!(texts[1], BUSY);
        assert_ne!(texts[0], PLACEHOLDER);
        assert_eq!(model.calls(), 1, "one model call for one question");
        assert!(service.router().in_flight().is_empty());
    }
}
