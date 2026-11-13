//! The agent session saves its own turns, so a conversation is saved by the app and not
//! by the window that asked: an answer is kept when the window is closed, reloaded or
//! hidden, and later when the question came from another device.
//!
//! - **The question** and the answer as it starts are written when `agent_send` takes
//!   the question, before the run starts ([`TurnRecorder::begin`]): an app closed during
//!   the answer shows it as interrupted, with Retry, as before. A retried answer's earlier
//!   try is removed in the same write.
//! - **The answer** is folded from the run's events by the backend's transcript reducer
//!   ([`crate::agent_transcript`]), the same fold as the Ask view's, and written when its
//!   check starts (the text is complete: a checkpoint) and when the run ends
//!   ([`TurnRecorder::observe`]). A run that fails before it starts is written as failed
//!   ([`TurnRecorder::fail`]); a session that ends with a run still open writes it as
//!   interrupted ([`TurnRecorder::session_ended`]).
//! - **What only the app knows**, a steer and an approval answered, joins the run's
//!   actions where it happened ([`TurnRecorder::local`]).
//! - **A steer that starts a new run** (the answer ended as it was sent) is adopted as a
//!   new question and answer with the ids the window gave it ([`TurnRecorder::expect_steer`]).
//! - **Every write** is one read-modify-write of the conversations file under its lock
//!   ([`ConversationStore::write_turns`]), followed by `conversation_changed`.
//!
//! Side-thread sessions never record turns: their questions and answers live in the
//! side thread, which the window saves.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value};
use shodh_rag::harness::AgentEvent;

use crate::agent_transcript::{event_value, Transcript};
use crate::conversation_commands::{ConversationMessage, ConversationStore, TurnWrite};

/// Where turns are written: the conversation store, and every open view told.
pub trait TurnSink: Send + Sync {
    /// Writes `writes` then `removed` into `conversation_id` (created with `create`);
    /// returns whether anything was written.
    fn write(
        &self,
        conversation_id: &str,
        writes: Vec<TurnWrite>,
        removed: &[String],
        create: bool,
    ) -> Result<bool, String>;
}

/// A store and a "changed" signal: the app's sink, and the tests'.
pub struct StoreSink<F: Fn(&str) + Send + Sync> {
    pub store: ConversationStore,
    pub changed: F,
}

impl<F: Fn(&str) + Send + Sync> TurnSink for StoreSink<F> {
    fn write(
        &self,
        conversation_id: &str,
        writes: Vec<TurnWrite>,
        removed: &[String],
        create: bool,
    ) -> Result<bool, String> {
        let written = self
            .store
            .write_turns(conversation_id, writes, removed, create)?
            .is_some();
        if written {
            (self.changed)(conversation_id);
        }
        Ok(written)
    }
}

/// The turn a window sends with its question (`agent_send`'s `turn`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTurn {
    /// The conversation the question was asked in.
    pub conversation_id: String,
    /// The question as the window stores it (`toStored`), with what it carries
    /// (attached text, the files it is about, a side summary). Absent for a retry of the
    /// latest answer, which asks the question before it again.
    #[serde(default)]
    pub question: Option<ConversationMessage>,
    /// The answer as it starts (`startedAnswer`): its id, time and starting transcript
    /// (run id, local start time, mode, fallback, the tries it replaces).
    pub answer: ConversationMessage,
    /// The answer a retry replaces, removed when this one is written.
    #[serde(default)]
    pub replaces: Option<String>,
}

/// The ids a window gives a steer, in case the answer ended before it arrived and it
/// starts a new run (`agent_steer`'s `turn`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerTurn {
    pub question_id: String,
    pub answer_id: String,
}

/// A run whose answer is being recorded.
struct RunTurn {
    conversation_id: String,
    session_id: String,
    /// The question it answers, written before the answer when it is not saved yet.
    question: Option<ConversationMessage>,
    answer: ConversationMessage,
    transcript: Transcript,
}

impl RunTurn {
    /// The answer as it is now: `transcript` in its stored form, and its text.
    fn answer_with(&self, transcript: Value) -> ConversationMessage {
        ConversationMessage {
            content: self.transcript.answer_text(),
            transcript: Some(transcript),
            ..self.answer.clone()
        }
    }

    /// The writes that save the answer: the question first when the answer is new.
    fn writes(&self, transcript: Value) -> Vec<TurnWrite> {
        let mut writes: Vec<TurnWrite> = self
            .question
            .iter()
            .cloned()
            .map(TurnWrite::Insert)
            .collect();
        writes.push(TurnWrite::Answer(self.answer_with(transcript)));
        writes
    }
}

/// A steer sent to a session, waiting to see whether it starts a new run.
struct PendingSteer {
    conversation_id: String,
    turn: SteerTurn,
    text: String,
    at_ms: u64,
    /// The run it started, once `agent_steer` knows it.
    run_id: Option<String>,
}

#[derive(Default)]
struct State {
    runs: HashMap<String, RunTurn>,
    steers: HashMap<String, PendingSteer>,
}

/// The runs whose answers the backend saves (one per question accepted).
#[derive(Default)]
pub struct TurnRecorder {
    state: Mutex<State>,
}

fn log_write(result: Result<bool, String>, what: &str) {
    if let Err(e) = result {
        tracing::warn!(target: "shodh::conversations", error = %e, "{what} was not saved");
    }
}

/// An ISO time of `ms` since the epoch, as the window writes it.
fn iso_time(ms: u64) -> String {
    let ms = i64::try_from(ms).unwrap_or(i64::MAX);
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl TurnRecorder {
    /// The question `turn` was accepted for run `run_id` of `session_id`: writes it and
    /// the answer as it starts (removing the try it replaces), and records the run's
    /// answer from here on. Called before the run starts, so none of its events is missed.
    pub fn begin(
        &self,
        sink: &dyn TurnSink,
        session_id: &str,
        run_id: &str,
        turn: SendTurn,
    ) -> Result<(), String> {
        let SendTurn {
            conversation_id,
            question,
            mut answer,
            replaces,
        } = turn;
        // The answer folds this run's events: its transcript names this run.
        let mut seed = match answer.transcript.take() {
            Some(Value::Object(seed)) => seed,
            _ => serde_json::Map::new(),
        };
        seed.insert("runId".into(), json!(run_id));
        let transcript = Transcript::new(&Value::Object(seed));
        answer.role = "assistant".to_string();
        answer.transcript = Some(transcript.persisted());
        let mut writes: Vec<TurnWrite> = question.iter().cloned().map(TurnWrite::Insert).collect();
        writes.push(TurnWrite::Insert(answer.clone()));
        let removed: Vec<String> = replaces.into_iter().filter(|id| *id != answer.id).collect();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        sink.write(&conversation_id, writes, &removed, true)?;
        state.runs.insert(
            run_id.to_string(),
            RunTurn {
                conversation_id,
                session_id: session_id.to_string(),
                question,
                answer,
                transcript,
            },
        );
        Ok(())
    }

    /// Whether the answer of run `run_id` is being recorded.
    pub fn records(&self, run_id: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.runs.contains_key(run_id)
    }

    /// The recorded run of `session_id` that is still going, if any.
    pub fn active_run(&self, session_id: &str) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .runs
            .iter()
            .find(|(_, run)| run.session_id == session_id)
            .map(|(id, _)| id.clone())
    }

    /// An event of `session_id`'s stream: folded into its run's answer, which is written
    /// when its check starts and when the run ends. A run a steer started is adopted.
    pub fn observe(&self, sink: &dyn TurnSink, session_id: &str, event: &AgentEvent) {
        let run_id = event.run_id().to_string();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let AgentEvent::RunStarted { .. } = event {
            if !state.runs.contains_key(&run_id) {
                self.adopt_steer(&mut state, sink, session_id, &run_id);
            }
        }
        let Some(run) = state.runs.get_mut(&run_id) else {
            return;
        };
        let value = event_value(event);
        run.transcript.apply(&value);
        if matches!(event, AgentEvent::RunFinished { .. }) {
            if let Some(run) = state.runs.remove(&run_id) {
                let writes = run.writes(run.transcript.persisted());
                log_write(
                    sink.write(&run.conversation_id, writes, &[], false),
                    "a finished answer",
                );
            }
        } else if run.transcript.is_live() && run.transcript.starts_check(&value) {
            let writes = run.writes(run.transcript.checkpoint());
            log_write(
                sink.write(&run.conversation_id, writes, &[], false),
                "an answer being checked",
            );
        }
    }

    /// An action only the app knows (`local_steer`, `local_approval`), joined to run
    /// `run_id` where it happened. Nothing for a run not recorded (ended, or a side thread).
    pub fn local(&self, run_id: &str, action: Value) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(run) = state.runs.get_mut(run_id) {
            run.transcript.apply(&action);
        }
    }

    /// Run `run_id` failed before it started (`agent_send` failed after taking the
    /// question): its answer is written failed with the error, as the window showed it.
    pub fn fail(&self, sink: &dyn TurnSink, run_id: &str, error: &str, code: &str, at_ms: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut run) = state.runs.remove(run_id) else {
            return;
        };
        run.transcript.apply(&json!({
            "type": "local_failed",
            "error": error,
            "code": code,
            "atMs": at_ms,
        }));
        let writes = run.writes(run.transcript.persisted());
        log_write(
            sink.write(&run.conversation_id, writes, &[], false),
            "a failed answer",
        );
    }

    /// `session_id` ended (its process stopped): a run still open will never finish and
    /// is written as interrupted at `at_ms`.
    pub fn session_ended(&self, sink: &dyn TurnSink, session_id: &str, at_ms: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.steers.remove(session_id);
        let open: Vec<String> = state
            .runs
            .iter()
            .filter(|(_, run)| run.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for run_id in open {
            let Some(mut run) = state.runs.remove(&run_id) else {
                continue;
            };
            run.transcript
                .apply(&json!({ "type": "local_interrupted", "atMs": at_ms }));
            let writes = run.writes(run.transcript.persisted());
            log_write(
                sink.write(&run.conversation_id, writes, &[], false),
                "an interrupted answer",
            );
        }
    }

    /// A steer of `text` is about to be sent to `session_id` of `conversation_id`: if it
    /// starts a new run, that run is a new question and answer with `turn`'s ids.
    pub fn expect_steer(
        &self,
        session_id: &str,
        conversation_id: &str,
        turn: SteerTurn,
        text: &str,
        at_ms: u64,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.steers.insert(
            session_id.to_string(),
            PendingSteer {
                conversation_id: conversation_id.to_string(),
                turn,
                text: text.to_string(),
                at_ms,
                run_id: None,
            },
        );
    }

    /// The steer sent to `session_id` went to run `run_id`: joined to a recorded run
    /// (nothing to adopt), or a new run, adopted when it starts (now, if it started).
    pub fn steered(&self, session_id: &str, run_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.runs.contains_key(run_id) {
            state.steers.remove(session_id);
        } else if let Some(steer) = state.steers.get_mut(session_id) {
            steer.run_id = Some(run_id.to_string());
        }
    }

    /// The steer sent to `session_id` failed: there is nothing to adopt.
    pub fn steer_failed(&self, session_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.steers.remove(session_id);
    }

    fn adopt_steer(&self, state: &mut State, sink: &dyn TurnSink, session_id: &str, run_id: &str) {
        let matches = state
            .steers
            .get(session_id)
            .is_some_and(|s| s.run_id.as_deref().is_none_or(|id| id == run_id));
        if !matches {
            return;
        }
        let Some(steer) = state.steers.remove(session_id) else {
            return;
        };
        let timestamp = iso_time(steer.at_ms);
        let question = ConversationMessage {
            id: steer.turn.question_id,
            role: "user".to_string(),
            content: steer.text,
            timestamp: timestamp.clone(),
            artifacts: None,
            search_results: None,
            metadata: None,
            run: None,
            transcript: None,
        };
        // As the window adopts it: running, in this session, from when it was sent.
        let transcript = Transcript::new(&json!({
            "runId": run_id,
            "startedAtMs": steer.at_ms,
            "status": "running",
            "sessionId": session_id,
        }));
        let answer = ConversationMessage {
            id: steer.turn.answer_id,
            role: "assistant".to_string(),
            content: String::new(),
            timestamp,
            artifacts: None,
            search_results: None,
            metadata: None,
            run: None,
            transcript: Some(transcript.persisted()),
        };
        let writes = vec![
            TurnWrite::Insert(question.clone()),
            TurnWrite::Insert(answer.clone()),
        ];
        match sink.write(&steer.conversation_id, writes, &[], false) {
            Ok(true) => {
                state.runs.insert(
                    run_id.to_string(),
                    RunTurn {
                        conversation_id: steer.conversation_id,
                        session_id: session_id.to_string(),
                        question: Some(question),
                        answer,
                        transcript,
                    },
                );
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(target: "shodh::conversations", error = %e, "a steered question was not saved")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_commands::ConversationRecord;
    use shodh_rag::harness::RunStatus;
    use std::sync::{Arc, Barrier};

    fn message(id: &str, role: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: "2026-11-13T10:00:00.000Z".to_string(),
            artifacts: None,
            search_results: None,
            metadata: None,
            run: None,
            transcript: None,
        }
    }

    /// The answer a window starts with (`startedAnswer`), as it sends it.
    fn started(id: &str, run_id: &str) -> ConversationMessage {
        ConversationMessage {
            transcript: Some(json!({
                "runId": run_id,
                "startedAtMs": 1_763_000_000_000u64,
                "status": "starting",
                "agentMode": "ask",
            })),
            ..message(id, "assistant", "")
        }
    }

    struct Sink {
        sink: StoreSink<Box<dyn Fn(&str) + Send + Sync>>,
        changed: Arc<Mutex<Vec<String>>>,
    }

    fn sink(dir: &std::path::Path) -> Sink {
        let changed = Arc::new(Mutex::new(Vec::new()));
        let seen = changed.clone();
        Sink {
            sink: StoreSink {
                store: ConversationStore::in_dir(dir),
                changed: Box::new(move |id: &str| {
                    seen.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(id.to_string())
                }),
            },
            changed,
        }
    }

    fn conversation(store: &ConversationStore, id: &str) -> ConversationRecord {
        store
            .get(id)
            .expect("the store reads")
            .expect("the conversation is saved")
    }

    fn turn(conversation: &str, question: &str, answer: &str, run: &str) -> SendTurn {
        SendTurn {
            conversation_id: conversation.to_string(),
            question: Some(message(question, "user", "What is the notice period?")),
            answer: started(answer, run),
            replaces: None,
        }
    }

    fn event(value: Value) -> AgentEvent {
        serde_json::from_value(value).expect("an AgentEvent")
    }

    #[test]
    fn two_senders_lose_no_turns() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::in_dir(dir.path());
        const PER_SENDER: usize = 25;
        let start = Arc::new(Barrier::new(2));
        let senders: Vec<_> = ["desktop", "phone"]
            .into_iter()
            .map(|who| {
                let store = store.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    for n in 0..PER_SENDER {
                        store
                            .append_turn("conv-1", message(&format!("{who}-{n}"), "user", who))
                            .expect("the turn is saved");
                    }
                })
            })
            .collect();
        for sender in senders {
            sender.join().expect("a sender finished");
        }
        let saved = conversation(&store, "conv-1");
        assert_eq!(
            saved.messages.len(),
            2 * PER_SENDER,
            "every message is kept"
        );
        for who in ["desktop", "phone"] {
            let order: Vec<&str> = saved
                .messages
                .iter()
                .filter(|m| m.content == who)
                .map(|m| m.id.as_str())
                .collect();
            let sent: Vec<String> = (0..PER_SENDER).map(|n| format!("{who}-{n}")).collect();
            assert_eq!(
                order, sent,
                "{who}'s turns keep the order they were sent in"
            );
        }
        // The first question names the conversation.
        assert_ne!(saved.title, crate::conversation_commands::NEW_CHAT_TITLE);
    }

    #[test]
    fn an_answer_is_saved_without_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let sink = sink(dir.path());
        let recorder = TurnRecorder::default();
        recorder
            .begin(
                &sink.sink,
                "sess-1",
                "run-1",
                turn("conv-1", "q1", "a1", "run-1"),
            )
            .unwrap();
        // Saved at once: the question, and the answer as it starts.
        let saved = conversation(&sink.sink.store, "conv-1");
        let ids: Vec<&str> = saved.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["q1", "a1"]);
        assert_eq!(saved.title, "What is the notice period?");

        for value in [
            json!({ "type": "run_started", "runId": "run-1", "sessionId": "sess-1", "model": "m", "atMs": 1, "warning": null }),
            json!({ "type": "text_delta", "runId": "run-1", "messageId": "t1", "delta": "Sixty days [1]." }),
            json!({ "type": "grounding_started", "runId": "run-1", "round": 0 }),
        ] {
            recorder.observe(&sink.sink, "sess-1", &event(value));
        }
        // The text is complete and being checked: kept if the app closes now.
        let checking = conversation(&sink.sink.store, "conv-1");
        assert_eq!(checking.messages[1].content, "Sixty days [1].");
        let transcript = checking.messages[1].transcript.clone().unwrap();
        assert_eq!(transcript["savedWhileChecking"], json!(true));
        assert_eq!(transcript["status"], json!("running"));

        recorder.observe(
            &sink.sink,
            "sess-1",
            &AgentEvent::RunFinished {
                run_id: "run-1".into(),
                status: RunStatus::Completed,
                duration_ms: 900,
                error: None,
                provider_error: None,
            },
        );
        // No window ever saved: the store has the finished answer.
        let done = conversation(&sink.sink.store, "conv-1");
        assert_eq!(done.messages.len(), 2);
        let answer = &done.messages[1];
        assert_eq!(answer.content, "Sixty days [1].");
        let transcript = answer.transcript.clone().unwrap();
        assert_eq!(transcript["status"], json!("completed"));
        assert_eq!(transcript["durationMs"], json!(900));
        assert_eq!(transcript["agentMode"], json!("ask"));
        assert!(transcript.get("savedWhileChecking").is_none());
        assert!(!recorder.records("run-1"), "the run is done with");
        // Every write told the open views.
        let told = sink.changed.lock().unwrap().clone();
        assert_eq!(told, ["conv-1", "conv-1", "conv-1"]);
    }

    #[test]
    fn a_window_edit_of_the_answer_is_kept_by_later_writes() {
        let dir = tempfile::tempdir().unwrap();
        let sink = sink(dir.path());
        let recorder = TurnRecorder::default();
        recorder
            .begin(
                &sink.sink,
                "sess-1",
                "run-1",
                turn("conv-1", "q1", "a1", "run-1"),
            )
            .unwrap();
        // A side thread was opened on the answer while it ran.
        let mut edited = conversation(&sink.sink.store, "conv-1").messages[1].clone();
        edited.metadata = Some(json!({ "focusThreads": [{ "id": "t1" }] }));
        sink.sink.store.append_turn("conv-1", edited).unwrap();
        recorder.observe(
            &sink.sink,
            "sess-1",
            &AgentEvent::RunFinished {
                run_id: "run-1".into(),
                status: RunStatus::Aborted,
                duration_ms: 10,
                error: None,
                provider_error: None,
            },
        );
        let answer = conversation(&sink.sink.store, "conv-1").messages[1].clone();
        assert_eq!(answer.transcript.unwrap()["status"], json!("aborted"));
        assert_eq!(
            answer.metadata,
            Some(json!({ "focusThreads": [{ "id": "t1" }] }))
        );
    }

    #[test]
    fn a_failed_send_and_an_ended_session_are_saved_as_the_window_showed_them() {
        let dir = tempfile::tempdir().unwrap();
        let sink = sink(dir.path());
        let recorder = TurnRecorder::default();
        recorder
            .begin(
                &sink.sink,
                "sess-1",
                "run-1",
                turn("conv-1", "q1", "a1", "run-1"),
            )
            .unwrap();
        recorder.fail(
            &sink.sink,
            "run-1",
            "An answer is already running.",
            "busy",
            1_763_000_000_400,
        );
        let failed = conversation(&sink.sink.store, "conv-1").messages[1].clone();
        let transcript = failed.transcript.unwrap();
        assert_eq!(transcript["status"], json!("error"));
        assert_eq!(transcript["errorCode"], json!("busy"));
        assert_eq!(transcript["error"], json!("An answer is already running."));
        assert_eq!(transcript["durationMs"], json!(400));

        // A retry replaces it; the session then stops mid-answer.
        recorder
            .begin(
                &sink.sink,
                "sess-1",
                "run-2",
                SendTurn {
                    conversation_id: "conv-1".into(),
                    question: None,
                    answer: started("a2", "run-2"),
                    replaces: Some("a1".into()),
                },
            )
            .unwrap();
        let ids: Vec<String> = conversation(&sink.sink.store, "conv-1")
            .messages
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(
            ids,
            ["q1", "a2"],
            "the retried answer replaces the failed one"
        );
        recorder.session_ended(&sink.sink, "sess-1", 1_763_000_000_900);
        let stopped = conversation(&sink.sink.store, "conv-1").messages[1].clone();
        assert_eq!(stopped.transcript.unwrap()["status"], json!("aborted"));
        assert!(recorder.active_run("sess-1").is_none());
    }

    #[test]
    fn a_steer_and_an_approval_join_the_answer_and_a_late_steer_is_a_new_turn() {
        let dir = tempfile::tempdir().unwrap();
        let sink = sink(dir.path());
        let recorder = TurnRecorder::default();
        recorder
            .begin(
                &sink.sink,
                "sess-1",
                "run-1",
                turn("conv-1", "q1", "a1", "run-1"),
            )
            .unwrap();
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "step_started", "runId": "run-1", "stepId": "s1", "parentStepId": null, "tool": "create_note", "label": "Creating", "args": {}, "tier": "write", "atMs": 5 })),
        );
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "approval_requested", "runId": "run-1", "stepId": "s1", "tool": "create_note", "label": "Create", "tier": "write", "preview": null })),
        );
        recorder.local(
            "run-1",
            json!({ "type": "local_approval", "stepId": "s1", "approved": true }),
        );
        recorder.local(
            "run-1",
            json!({ "type": "local_steer", "id": "steer-1", "text": "Shorter" }),
        );
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "step_finished", "runId": "run-1", "stepId": "s1", "ok": false, "summary": "No", "detail": null, "durationMs": 1, "invalidArguments": false })),
        );
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "run_finished", "runId": "run-1", "status": "completed", "durationMs": 20, "error": null, "providerError": null })),
        );
        let answer = conversation(&sink.sink.store, "conv-1").messages[1].clone();
        let transcript = answer.transcript.unwrap();
        assert_eq!(
            transcript["steps"]["s1"]["approval"]["decision"],
            json!("approved")
        );
        assert_eq!(
            transcript["blocks"][1],
            json!({ "kind": "steer", "id": "steer-1", "text": "Shorter" })
        );

        // The answer ended as the next steer was sent: it starts run-2, a new turn.
        recorder.expect_steer(
            "sess-1",
            "conv-1",
            SteerTurn {
                question_id: "q2".into(),
                answer_id: "a2".into(),
            },
            "And the fee?",
            1_763_000_001_000,
        );
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "run_started", "runId": "run-2", "sessionId": "sess-1", "model": "m", "atMs": 1, "warning": null })),
        );
        recorder.steered("sess-1", "run-2");
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "text_delta", "runId": "run-2", "messageId": "t", "delta": "None." })),
        );
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "run_finished", "runId": "run-2", "status": "completed", "durationMs": 3, "error": null, "providerError": null })),
        );
        let saved = conversation(&sink.sink.store, "conv-1");
        let ids: Vec<&str> = saved.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["q1", "a1", "q2", "a2"]);
        assert_eq!(saved.messages[2].content, "And the fee?");
        assert_eq!(saved.messages[3].content, "None.");
    }

    #[test]
    fn a_deleted_conversation_is_not_brought_back_by_its_answer() {
        let dir = tempfile::tempdir().unwrap();
        let sink = sink(dir.path());
        let recorder = TurnRecorder::default();
        recorder
            .begin(
                &sink.sink,
                "sess-1",
                "run-1",
                turn("conv-1", "q1", "a1", "run-1"),
            )
            .unwrap();
        sink.sink
            .store
            .update(|all| {
                all.retain(|c| c.id != "conv-1");
                Ok(())
            })
            .unwrap();
        recorder.observe(
            &sink.sink,
            "sess-1",
            &event(json!({ "type": "run_finished", "runId": "run-1", "status": "completed", "durationMs": 3, "error": null, "providerError": null })),
        );
        assert!(sink.sink.store.get("conv-1").unwrap().is_none());
    }
}
