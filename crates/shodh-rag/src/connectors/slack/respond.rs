//! One question, start to finish: the checks that cost nothing come first, the model call
//! comes last and only when everything before it passed.
//!
//! 1. No workspace selected (or it no longer exists): "isn't set up", no model call.
//! 2. Empty question (only the mention): a hint, no model call.
//! 3. Today's cap reached: the limit message, no model call.
//! 4. A placeholder ("Searching your library…") is posted in the thread.
//! 5. Retrieval over the selected workspace. Nothing found: the placeholder says so, no
//!    model call.
//! 6. One call is reserved against the cap (counted before it is made), then made.
//! 7. The placeholder is edited into the answer, or into a short, honest reason when the
//!    model failed.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};

use super::answer::{answer_prompt, slack_answer, AnswerPassage, AnswerScope, MAX_ANSWER_TOKENS};
use super::events::Question;
use super::sanitize::{sanitize, PrivateTerms};
use super::web::SlackApiError;
use crate::harness::provider_error::{classify, ProviderErrorKind};
use crate::research::results::TextModel;
use crate::research::tables::calls::{next_utc_midnight, utc_day, Clock};

pub const NOT_SET_UP: &str = "Shodh isn't set up to answer here yet.";
pub const PLACEHOLDER: &str = "Searching your library…";
pub const BUSY: &str = "I'm still answering the previous question in this thread.";
pub const EMPTY_QUESTION: &str =
    "Ask me a question after the mention, for example: “What does the report say about costs?”";
pub const NOTHING_FOUND: &str =
    "I couldn't find anything about that in the documents Shodh answers from here.";
pub const SEARCH_FAILED: &str =
    "Shodh couldn't search its documents just now. Please try again later.";
pub const LIMIT_UNKNOWN: &str =
    "Shodh couldn't check today's answer limit, so it didn't answer. Please try again later.";

/// The cap message, with the reset time in UTC.
pub fn at_cap_message(resets_at: DateTime<Utc>) -> String {
    format!(
        "Shodh has used today's answer limit; it resets at {} UTC.",
        resets_at.format("%H:%M")
    )
}

/// What a failed model call becomes in Slack: short and honest, no details (those go to
/// the app log).
pub fn model_failure_message(kind: ProviderErrorKind) -> &'static str {
    match kind {
        ProviderErrorKind::RateLimited => {
            "The model is busy right now (rate limited). Please try again in a minute."
        }
        ProviderErrorKind::QuotaExhausted => {
            "The model's quota for today is used up, so Shodh can't answer until it resets."
        }
        ProviderErrorKind::Auth => {
            "Shodh can't use its model right now (the API key was refused). The owner needs to fix it in Shodh."
        }
        ProviderErrorKind::Timeout | ProviderErrorKind::Network => {
            "Shodh couldn't reach its model. Please try again shortly."
        }
        ProviderErrorKind::ModelUnavailable => {
            "The model Shodh uses is unavailable right now. Please try again later."
        }
        ProviderErrorKind::ContextTooLong => {
            "That question needs more text than the model accepts. Try a narrower question."
        }
        ProviderErrorKind::ContentFiltered => "The model declined to answer that question.",
        _ => "Shodh couldn't answer that just now. Please try again later.",
    }
}

/// Posts and edits messages in Slack.
#[async_trait]
pub trait SlackPoster: Send + Sync {
    /// Posts `text` in `channel` (in the thread `thread_ts` when given); returns the
    /// message's `ts`.
    async fn post(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackApiError>;
    /// Replaces the text of message `ts`.
    async fn update(&self, channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError>;
}

/// Where answers come from. The app implements it over the selected workspace.
#[async_trait]
pub trait AnswerSource: Send + Sync {
    /// The scope of the selected workspace; `None` while none is selected or when the
    /// selected one no longer exists.
    async fn scope(&self) -> Result<Option<AnswerScope>, String>;
    /// Passages for `question` within `scope`
    /// ([`super::answer::retrieve_passages`]).
    async fn retrieve(
        &self,
        scope: &AnswerScope,
        question: &str,
    ) -> Result<Vec<AnswerPassage>, String>;
    /// The model answers are written with (no tools).
    fn model(&self) -> Result<Arc<dyn TextModel>, String>;
    /// This computer's home folder and user name, never to appear in Slack.
    fn private_terms(&self) -> PrivateTerms;
}

/// The daily count of Slack answers (UTC days).
pub trait AnswerQuota: Send + Sync {
    /// The cap now.
    fn cap(&self) -> usize;
    /// Answers counted on `day`.
    fn used(&self, day: NaiveDate) -> Result<usize, String>;
    /// Counts one answer on `day` if that stays within the cap. `Ok(false)`: at the cap,
    /// nothing counted.
    fn try_reserve(&self, day: NaiveDate) -> Result<bool, String>;
}

/// How a question ended (for the log and tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    NotSetUp,
    EmptyQuestion,
    AtCap,
    NothingFound,
    Answered,
    ModelFailed(ProviderErrorKind),
    SearchFailed,
    LimitUnknown,
    /// Slack refused a post or an edit; nothing more can be said there.
    SlackFailed(String),
}

/// Answers questions with an [`AnswerSource`], counted against an [`AnswerQuota`].
pub struct Responder {
    pub source: Arc<dyn AnswerSource>,
    pub quota: Arc<dyn AnswerQuota>,
    pub poster: Arc<dyn SlackPoster>,
    pub clock: Arc<dyn Clock>,
}

impl Responder {
    async fn say(&self, question: &Question, text: &str) -> Outcome {
        match self
            .poster
            .post(&question.channel, question.reply_thread_ts.as_deref(), text)
            .await
        {
            Ok(_) => Outcome::Answered,
            Err(e) => Outcome::SlackFailed(e.to_string()),
        }
    }

    /// Posts a reply that needs no model call and reports `outcome` (or Slack's refusal).
    async fn reply(&self, question: &Question, text: &str, outcome: Outcome) -> Outcome {
        match self.say(question, text).await {
            Outcome::Answered => outcome,
            failed => failed,
        }
    }

    async fn edit(&self, question: &Question, ts: &str, text: &str, outcome: Outcome) -> Outcome {
        match self.poster.update(&question.channel, ts, text).await {
            Ok(()) => outcome,
            Err(e) => Outcome::SlackFailed(e.to_string()),
        }
    }

    /// Answers `question` (see the module docs for the order of the steps).
    pub async fn respond(&self, question: &Question) -> Outcome {
        let scope = match self.source.scope().await {
            Ok(Some(scope)) => scope,
            Ok(None) => return self.reply(question, NOT_SET_UP, Outcome::NotSetUp).await,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the selected workspace could not be read");
                return self.reply(question, NOT_SET_UP, Outcome::NotSetUp).await;
            }
        };
        if question.text.trim().is_empty() {
            return self
                .reply(question, EMPTY_QUESTION, Outcome::EmptyQuestion)
                .await;
        }
        let now = self.clock.now();
        let day = utc_day(now);
        let resets_at = next_utc_midnight(now);
        match self.quota.used(day) {
            Ok(used) if used >= self.quota.cap() => {
                return self
                    .reply(question, &at_cap_message(resets_at), Outcome::AtCap)
                    .await
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the answer count could not be read");
                return self
                    .reply(question, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await;
            }
        }
        let placeholder = match self
            .poster
            .post(
                &question.channel,
                question.reply_thread_ts.as_deref(),
                PLACEHOLDER,
            )
            .await
        {
            Ok(ts) => ts,
            Err(e) => return Outcome::SlackFailed(e.to_string()),
        };
        let passages = match self.source.retrieve(&scope, &question.text).await {
            Ok(passages) => passages,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "search for a Slack question failed");
                return self
                    .edit(question, &placeholder, SEARCH_FAILED, Outcome::SearchFailed)
                    .await;
            }
        };
        if passages.is_empty() {
            return self
                .edit(question, &placeholder, NOTHING_FOUND, Outcome::NothingFound)
                .await;
        }
        let model = match self.source.model() {
            Ok(model) => model,
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "no model for a Slack answer");
                let kind = ProviderErrorKind::ModelUnavailable;
                return self
                    .edit(
                        question,
                        &placeholder,
                        model_failure_message(kind),
                        Outcome::ModelFailed(kind),
                    )
                    .await;
            }
        };
        match self.quota.try_reserve(day) {
            Ok(true) => {}
            Ok(false) => {
                return self
                    .edit(
                        question,
                        &placeholder,
                        &at_cap_message(resets_at),
                        Outcome::AtCap,
                    )
                    .await
            }
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the answer could not be counted");
                return self
                    .edit(question, &placeholder, LIMIT_UNKNOWN, Outcome::LimitUnknown)
                    .await;
            }
        }
        let private = self.source.private_terms();
        let prompt = answer_prompt(&passages, &question.text, &private);
        match model.complete(&prompt, MAX_ANSWER_TOKENS).await {
            Ok(text) if !text.trim().is_empty() => {
                let message = slack_answer(&text, &passages, &private);
                self.edit(question, &placeholder, &message, Outcome::Answered)
                    .await
            }
            Ok(_) => {
                let kind = ProviderErrorKind::EmptyResponse;
                tracing::warn!(target: "shodh::slack", "the model returned an empty answer");
                self.edit(
                    question,
                    &placeholder,
                    model_failure_message(kind),
                    Outcome::ModelFailed(kind),
                )
                .await
            }
            Err(e) => {
                let now_ms = u64::try_from(now.timestamp_millis()).unwrap_or(0);
                let kind = classify(&e, None, now_ms).kind;
                tracing::warn!(
                    target: "shodh::slack",
                    kind = ?kind,
                    error = %sanitize(&e, &private),
                    "the model call for a Slack answer failed"
                );
                self.edit(
                    question,
                    &placeholder,
                    model_failure_message(kind),
                    Outcome::ModelFailed(kind),
                )
                .await
            }
        }
    }

    /// Tells the asker an answer is already in flight in this thread.
    pub async fn busy(&self, question: &Question) -> Outcome {
        self.reply(question, BUSY, Outcome::Answered).await
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// A Slack that records what was posted and edited.
    #[derive(Default)]
    pub struct RecordingSlack {
        pub messages: Mutex<Vec<(String, Option<String>, String)>>,
        pub edits: Mutex<HashMap<String, String>>,
    }

    impl RecordingSlack {
        /// Every message as it reads now (edits applied), in order.
        pub fn texts(&self) -> Vec<String> {
            let edits = self.edits.lock().unwrap();
            self.messages
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, (_, _, text))| edits.get(&i.to_string()).cloned().unwrap_or(text.clone()))
                .collect()
        }
    }

    #[async_trait]
    impl SlackPoster for RecordingSlack {
        async fn post(
            &self,
            channel: &str,
            thread_ts: Option<&str>,
            text: &str,
        ) -> Result<String, SlackApiError> {
            let mut messages = self.messages.lock().unwrap();
            messages.push((channel.into(), thread_ts.map(str::to_string), text.into()));
            Ok((messages.len() - 1).to_string())
        }
        async fn update(&self, _channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError> {
            self.edits.lock().unwrap().insert(ts.into(), text.into());
            Ok(())
        }
    }

    /// A model that records its prompts and answers with `reply`.
    pub struct RecordingModel {
        pub prompts: Mutex<Vec<String>>,
        pub reply: Result<String, String>,
    }

    impl RecordingModel {
        pub fn answering(reply: &str) -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
                reply: Ok(reply.into()),
            }
        }
        pub fn failing(error: &str) -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
                reply: Err(error.into()),
            }
        }
        pub fn calls(&self) -> usize {
            self.prompts.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl TextModel for RecordingModel {
        fn model_id(&self) -> String {
            "test-model".into()
        }
        async fn complete(&self, prompt: &str, _max_tokens: usize) -> Result<String, String> {
            self.prompts.lock().unwrap().push(prompt.into());
            self.reply.clone()
        }
    }

    /// A quota in memory.
    pub struct MemoryQuota {
        pub cap: usize,
        pub used: Mutex<HashMap<NaiveDate, usize>>,
    }

    impl MemoryQuota {
        pub fn new(cap: usize) -> Self {
            Self {
                cap,
                used: Mutex::new(HashMap::new()),
            }
        }
    }

    impl AnswerQuota for MemoryQuota {
        fn cap(&self) -> usize {
            self.cap
        }
        fn used(&self, day: NaiveDate) -> Result<usize, String> {
            Ok(*self.used.lock().unwrap().get(&day).unwrap_or(&0))
        }
        fn try_reserve(&self, day: NaiveDate) -> Result<bool, String> {
            let mut used = self.used.lock().unwrap();
            let n = used.entry(day).or_insert(0);
            if *n >= self.cap {
                return Ok(false);
            }
            *n += 1;
            Ok(true)
        }
    }

    pub struct FixedClock(pub DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::TimeZone;

    use super::testkit::*;
    use super::*;
    use crate::connectors::slack::events::{QuestionKind, ThreadKey};

    /// A source with fixed passages (or none selected).
    struct FixedSource {
        scope: Option<AnswerScope>,
        passages: Vec<AnswerPassage>,
        model: Arc<RecordingModel>,
        searches: Mutex<usize>,
    }

    #[async_trait]
    impl AnswerSource for FixedSource {
        async fn scope(&self) -> Result<Option<AnswerScope>, String> {
            Ok(self.scope.clone())
        }
        async fn retrieve(
            &self,
            _scope: &AnswerScope,
            _q: &str,
        ) -> Result<Vec<AnswerPassage>, String> {
            *self.searches.lock().unwrap() += 1;
            Ok(self.passages.clone())
        }
        fn model(&self) -> Result<Arc<dyn TextModel>, String> {
            Ok(self.model.clone())
        }
        fn private_terms(&self) -> PrivateTerms {
            PrivateTerms::default()
        }
    }

    fn question(text: &str) -> Question {
        Question {
            event_id: "Ev1".into(),
            kind: QuestionKind::Mention,
            channel: "C1".into(),
            reply_thread_ts: Some("100.1".into()),
            thread: ThreadKey {
                channel: "C1".into(),
                thread: "100.1".into(),
            },
            user: "U1".into(),
            text: text.into(),
        }
    }

    fn passage() -> AnswerPassage {
        AnswerPassage {
            n: 1,
            title: "lease.pdf".into(),
            page: Some("3".into()),
            text: "The notice period is sixty days.".into(),
        }
    }

    struct Setup {
        responder: Responder,
        slack: Arc<RecordingSlack>,
        model: Arc<RecordingModel>,
        quota: Arc<MemoryQuota>,
        source: Arc<FixedSource>,
    }

    fn setup(
        scope: Option<AnswerScope>,
        passages: Vec<AnswerPassage>,
        model: RecordingModel,
        cap: usize,
    ) -> Setup {
        let slack = Arc::new(RecordingSlack::default());
        let model = Arc::new(model);
        let quota = Arc::new(MemoryQuota::new(cap));
        let source = Arc::new(FixedSource {
            scope,
            passages,
            model: model.clone(),
            searches: Mutex::new(0),
        });
        let clock = Arc::new(FixedClock(
            Utc.with_ymd_and_hms(2026, 10, 23, 18, 30, 0).unwrap(),
        ));
        Setup {
            responder: Responder {
                source: source.clone(),
                quota: quota.clone(),
                poster: slack.clone(),
                clock,
            },
            slack,
            model,
            quota,
            source,
        }
    }

    fn scope() -> Option<AnswerScope> {
        Some(AnswerScope {
            source_ids: vec!["space-a".into()],
            files: vec![],
        })
    }

    #[tokio::test]
    async fn without_a_selected_workspace_nothing_is_searched_or_spent() {
        let s = setup(None, vec![passage()], RecordingModel::answering("x [1]"), 5);
        assert_eq!(
            s.responder.respond(&question("notice period?")).await,
            Outcome::NotSetUp
        );
        assert_eq!(s.slack.texts(), vec![NOT_SET_UP.to_string()]);
        assert_eq!(s.model.calls(), 0);
        assert_eq!(*s.source.searches.lock().unwrap(), 0);
        assert!(s.quota.used.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_answer_replaces_the_placeholder_in_the_thread_and_is_counted() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("It is **sixty days** [1]."),
            5,
        );
        assert_eq!(
            s.responder.respond(&question("notice period?")).await,
            Outcome::Answered
        );
        let messages = s.slack.messages.lock().unwrap().clone();
        assert_eq!(messages.len(), 1, "one message: the placeholder, edited");
        assert_eq!(
            messages[0].1.as_deref(),
            Some("100.1"),
            "in the question's thread"
        );
        assert_eq!(messages[0].2, PLACEHOLDER);
        let text = s.slack.texts()[0].clone();
        assert!(text.starts_with("It is *sixty days* [1]."), "{text}");
        assert!(text.contains("[1] lease.pdf, p. 3"));
        assert_eq!(s.model.calls(), 1);
        let day = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        assert_eq!(s.quota.used(day).unwrap(), 1);
    }

    #[tokio::test]
    async fn at_the_cap_no_model_call_is_made() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            1,
        );
        assert_eq!(
            s.responder.respond(&question("one?")).await,
            Outcome::Answered
        );
        assert_eq!(s.responder.respond(&question("two?")).await, Outcome::AtCap);
        assert_eq!(s.model.calls(), 1);
        let texts = s.slack.texts();
        assert_eq!(
            texts[1],
            "Shodh has used today's answer limit; it resets at 00:00 UTC."
        );
        assert_eq!(
            *s.source.searches.lock().unwrap(),
            1,
            "no search at the cap either"
        );
    }

    #[tokio::test]
    async fn the_cap_is_rechecked_when_the_call_is_reserved() {
        let s = setup(
            scope(),
            vec![passage()],
            RecordingModel::answering("x [1]"),
            1,
        );
        // Another answer took the last call while this one was searching.
        let day = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        struct TakeLast(Arc<MemoryQuota>, NaiveDate);
        #[async_trait]
        impl AnswerSource for TakeLast {
            async fn scope(&self) -> Result<Option<AnswerScope>, String> {
                Ok(Some(AnswerScope::default()))
            }
            async fn retrieve(
                &self,
                _s: &AnswerScope,
                _q: &str,
            ) -> Result<Vec<AnswerPassage>, String> {
                assert!(self.0.try_reserve(self.1).unwrap());
                Ok(vec![passage()])
            }
            fn model(&self) -> Result<Arc<dyn TextModel>, String> {
                Ok(Arc::new(RecordingModel::failing("never called")))
            }
            fn private_terms(&self) -> PrivateTerms {
                PrivateTerms::default()
            }
        }
        let responder = Responder {
            source: Arc::new(TakeLast(s.quota.clone(), day)),
            ..s.responder
        };
        assert_eq!(responder.respond(&question("q?")).await, Outcome::AtCap);
        assert_eq!(
            s.slack.texts(),
            vec!["Shodh has used today's answer limit; it resets at 00:00 UTC.".to_string()]
        );
        assert_eq!(
            s.quota.used(day).unwrap(),
            1,
            "the refused call is not counted"
        );
    }

    #[tokio::test]
    async fn nothing_found_and_empty_questions_cost_nothing() {
        let s = setup(scope(), vec![], RecordingModel::answering("x"), 5);
        assert_eq!(
            s.responder.respond(&question("unknown?")).await,
            Outcome::NothingFound
        );
        assert_eq!(
            s.responder.respond(&question("   ")).await,
            Outcome::EmptyQuestion
        );
        assert_eq!(s.model.calls(), 0);
        assert!(s.quota.used.lock().unwrap().is_empty());
        assert_eq!(
            s.slack.texts(),
            vec![NOTHING_FOUND.to_string(), EMPTY_QUESTION.to_string()]
        );
    }

    #[tokio::test]
    async fn model_failures_become_short_honest_replies() {
        for (error, expected) in [
            (
                "HTTP 429 Too Many Requests: rate limit exceeded",
                ProviderErrorKind::RateLimited,
            ),
            (
                "HTTP 401 Unauthorized: invalid api key",
                ProviderErrorKind::Auth,
            ),
            (
                "HTTP 402: insufficient credits",
                ProviderErrorKind::QuotaExhausted,
            ),
        ] {
            let s = setup(scope(), vec![passage()], RecordingModel::failing(error), 5);
            let outcome = s.responder.respond(&question("q?")).await;
            let Outcome::ModelFailed(kind) = outcome else {
                panic!("{error}: {outcome:?}")
            };
            assert_eq!(kind, expected, "{error}");
            let text = s.slack.texts()[0].clone();
            assert_eq!(text, model_failure_message(kind));
            assert!(!text.contains("HTTP"), "no details in Slack");
        }
    }

    #[test]
    fn every_failure_kind_has_a_reply() {
        for kind in [
            ProviderErrorKind::RateLimited,
            ProviderErrorKind::QuotaExhausted,
            ProviderErrorKind::ModelUnavailable,
            ProviderErrorKind::Auth,
            ProviderErrorKind::Timeout,
            ProviderErrorKind::Network,
            ProviderErrorKind::InvalidRequest,
            ProviderErrorKind::ContextTooLong,
            ProviderErrorKind::ContentFiltered,
            ProviderErrorKind::EmptyResponse,
            ProviderErrorKind::Other,
        ] {
            assert!(!model_failure_message(kind).is_empty());
        }
    }
}
