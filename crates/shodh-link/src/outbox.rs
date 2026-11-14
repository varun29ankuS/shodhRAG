//! What the phone keeps to send while the computer is asleep or away:
//! questions, files and the user's own task and calendar edits, in the order
//! they were made. The queue is saved (sealed, [`crate::sealed`]) before
//! every change is reported, so a kill of the app loses nothing, and it is
//! sent strictly in order when the computer is back: one item at a time,
//! the next only once the one before it is answered, sent or failed.
//!
//! Each item goes `waiting → sending → sent → answered` (a question), or
//! `waiting → sending → sent` (a file or an edit), or ends `failed` with the
//! computer's reason. An item found `sending` after a restart may or may not
//! have reached the computer: the phone asks the computer before sending it
//! again, and every item carries the ids it was first sent with, so a second
//! send is the same request, never a second question.

use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sealed::SealedFile;

/// The most items kept waiting at once.
pub const MAX_WAITING: usize = 200;

/// Finished items kept to show how they ended, the newest first.
pub const KEEP_FINISHED: usize = 20;

/// Where an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueState {
    /// Not sent yet.
    Waiting,
    /// Being sent; after a restart, perhaps sent.
    Sending,
    /// The computer took it (a question: its answer is on the way).
    Sent,
    /// A question's answer is in the conversation.
    Answered,
    /// The computer refused it, or it could not be sent; `error` says why.
    Failed,
}

impl QueueState {
    pub fn finished(self) -> bool {
        matches!(self, QueueState::Answered | QueueState::Failed)
    }

    /// Whether an item may go from `self` to `next`.
    fn may_become(self, next: QueueState) -> bool {
        use QueueState::*;
        matches!(
            (self, next),
            (Waiting, Sending)
                | (Sending, Waiting)
                | (Sending, Sent)
                | (Sent, Answered)
                | (Waiting | Sending | Sent, Failed)
                | (Failed, Waiting)
        )
    }
}

/// A question asked while the computer was away, with the ids it is sent
/// with (the same on every try).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedQuestion {
    pub conversation_id: String,
    pub text: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    pub request_id: String,
    pub question_id: String,
    pub answer_id: String,
    /// Unix milliseconds the question was asked: its time in the conversation.
    pub asked_at: u64,
    /// Text the question is about, as the desktop's `attachedContext` takes it
    /// (a photo's text, text shared from another app).
    #[serde(default)]
    pub attached: Vec<Value>,
    /// Passages the answer may cite as its own (a photo, as "Your photo").
    #[serde(default)]
    pub passages: Vec<Value>,
}

/// A file going to the computer, copied into the phone's own storage when it
/// was added (a share or a picker's permission to read it ends with the app).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedFile {
    /// The copy in the app's storage.
    pub local_path: String,
    /// The file's own name, as the computer keeps it.
    pub name: String,
    pub size: u64,
    /// SHA-256 of the file, hex.
    pub sha256: String,
    /// The workspace it goes to; `None`: "From my phone".
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// One of the user's own edits on the phone (a task added or ticked, an
/// event added or moved), made while the computer was away: one of the
/// computer's commands the link allows, with its arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedEdit {
    pub command: String,
    pub args: Value,
    /// Words for the queue's list ("Add the task “Call Asha”").
    pub label: String,
}

/// What an item sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Work {
    Question(QueuedQuestion),
    File(QueuedFile),
    Edit(QueuedEdit),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedItem {
    pub id: String,
    pub queued_at: u64,
    pub state: QueueState,
    pub work: Work,
    /// Why it failed, in plain words.
    #[serde(default)]
    pub error: Option<String>,
    /// Unix milliseconds of the last change of state.
    #[serde(default)]
    pub changed_at: u64,
}

/// Where the queue is kept.
pub trait OutboxStore: Send + Sync {
    fn load(&self) -> Result<Vec<QueuedItem>, String>;
    fn save(&self, items: &[QueuedItem]) -> Result<(), String>;
}

impl OutboxStore for SealedFile {
    fn load(&self) -> Result<Vec<QueuedItem>, String> {
        self.read::<Vec<QueuedItem>>()
            .map(Option::unwrap_or_default)
            .map_err(|e| e.to_string())
    }

    fn save(&self, items: &[QueuedItem]) -> Result<(), String> {
        self.write(&items)
    }
}

/// Why the queue refused a change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    #[error(
        "Your phone already has {MAX_WAITING} things waiting for your computer. Wait until it is back."
    )]
    Full,
    #[error("This is no longer waiting for your computer.")]
    Unknown,
    #[error("This was already queued.")]
    Duplicate,
    #[error("This cannot change from {from:?} to {to:?}.")]
    Transition { from: QueueState, to: QueueState },
    #[error("Only something waiting or finished can be removed.")]
    Busy,
    #[error("{0}")]
    Store(String),
}

/// The phone's outbox.
pub struct Outbox<S: OutboxStore> {
    store: S,
    items: Mutex<Vec<QueuedItem>>,
}

impl<S: OutboxStore> Outbox<S> {
    /// The queue as it was saved (empty the first time).
    pub fn open(store: S) -> Result<Self, OutboxError> {
        let items = store.load().map_err(OutboxError::Store)?;
        Ok(Self {
            store,
            items: Mutex::new(items),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Vec<QueuedItem>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Apply `change` to a copy, save it, and only then keep it: what the
    /// caller is told happened is on disk.
    fn change<T>(
        &self,
        change: impl FnOnce(&mut Vec<QueuedItem>) -> Result<T, OutboxError>,
    ) -> Result<T, OutboxError> {
        let mut items = self.lock();
        let mut next = items.clone();
        let out = change(&mut next)?;
        self.store.save(&next).map_err(OutboxError::Store)?;
        *items = next;
        Ok(out)
    }

    /// Every item, in order.
    pub fn list(&self) -> Vec<QueuedItem> {
        self.lock().clone()
    }

    /// Queue `work` as item `id`, last.
    pub fn add(&self, id: &str, work: Work, now: u64) -> Result<QueuedItem, OutboxError> {
        self.change(|items| {
            let open = items.iter().filter(|i| !i.state.finished()).count();
            if open >= MAX_WAITING {
                return Err(OutboxError::Full);
            }
            if items.iter().any(|i| i.id == id) {
                return Err(OutboxError::Duplicate);
            }
            let item = QueuedItem {
                id: id.to_string(),
                queued_at: now,
                state: QueueState::Waiting,
                work,
                error: None,
                changed_at: now,
            };
            items.push(item.clone());
            Ok(item)
        })
    }

    /// The item to work on now: the first one not finished. Everything after
    /// it waits, so the computer receives them in the order they were made.
    pub fn next(&self) -> Option<QueuedItem> {
        self.lock().iter().find(|i| !i.state.finished()).cloned()
    }

    /// Move item `id` to `state` (with the reason of a failure).
    pub fn mark(
        &self,
        id: &str,
        state: QueueState,
        error: Option<String>,
        now: u64,
    ) -> Result<QueuedItem, OutboxError> {
        self.change(|items| {
            let item = items
                .iter_mut()
                .find(|i| i.id == id)
                .ok_or(OutboxError::Unknown)?;
            if item.state == state {
                return Ok(item.clone());
            }
            if !item.state.may_become(state) {
                return Err(OutboxError::Transition {
                    from: item.state,
                    to: state,
                });
            }
            item.state = state;
            item.error = if state == QueueState::Failed {
                error
            } else {
                None
            };
            item.changed_at = now;
            let changed = item.clone();
            prune(items);
            Ok(changed)
        })
    }

    /// Remove item `id`: one still waiting (the user changed their mind) or
    /// one finished. One being sent is the computer's now.
    pub fn remove(&self, id: &str) -> Result<QueuedItem, OutboxError> {
        self.change(|items| {
            let at = items
                .iter()
                .position(|i| i.id == id)
                .ok_or(OutboxError::Unknown)?;
            if !matches!(
                items[at].state,
                QueueState::Waiting | QueueState::Answered | QueueState::Failed
            ) {
                return Err(OutboxError::Busy);
            }
            Ok(items.remove(at))
        })
    }
}

/// Keep the newest [`KEEP_FINISHED`] finished items, and every open one.
fn prune(items: &mut Vec<QueuedItem>) {
    let finished = items.iter().filter(|i| i.state.finished()).count();
    let mut drop = finished.saturating_sub(KEEP_FINISHED);
    if drop == 0 {
        return;
    }
    let mut order: Vec<(u64, usize)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.state.finished())
        .map(|(at, i)| (i.changed_at, at))
        .collect();
    order.sort();
    let mut gone: Vec<usize> = Vec::new();
    for (_, at) in order {
        if drop == 0 {
            break;
        }
        gone.push(at);
        drop -= 1;
    }
    let mut at = 0;
    items.retain(|_| {
        let keep = !gone.contains(&at);
        at += 1;
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sealed::tests::temp_dir;
    use crate::sealed::DataKey;

    fn question(n: u32) -> Work {
        Work::Question(QueuedQuestion {
            conversation_id: "c1".into(),
            text: format!("Question {n}"),
            workspace_id: None,
            request_id: format!("r{n}"),
            question_id: format!("q{n}"),
            answer_id: format!("a{n}"),
            asked_at: u64::from(n),
            attached: Vec::new(),
            passages: Vec::new(),
        })
    }

    fn sealed(dir: &std::path::Path) -> SealedFile {
        SealedFile::new(dir, "outbox.sealed", &DataKey::from_bytes([9; 32]))
    }

    fn texts(outbox: &Outbox<SealedFile>) -> Vec<(String, QueueState)> {
        outbox
            .list()
            .into_iter()
            .map(|i| match i.work {
                Work::Question(q) => (q.text, i.state),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn queued_questions_survive_a_kill_and_send_in_order() {
        let dir = temp_dir("outbox");
        {
            let outbox = Outbox::open(sealed(&dir)).unwrap();
            for n in 1..=3 {
                outbox
                    .add(&format!("i{n}"), question(n), u64::from(n))
                    .unwrap();
            }
            // The first was being sent when the app was killed.
            outbox.mark("i1", QueueState::Sending, None, 10).unwrap();
        }
        // The app starts again: the queue is as it was, in order.
        let outbox = Outbox::open(sealed(&dir)).unwrap();
        assert_eq!(
            texts(&outbox),
            vec![
                ("Question 1".into(), QueueState::Sending),
                ("Question 2".into(), QueueState::Waiting),
                ("Question 3".into(), QueueState::Waiting),
            ]
        );
        // Strictly in order: the second waits until the first is answered.
        let mut sent = Vec::new();
        while let Some(item) = outbox.next() {
            let Work::Question(q) = &item.work else {
                unreachable!()
            };
            if item.state == QueueState::Waiting {
                outbox
                    .mark(&item.id, QueueState::Sending, None, 20)
                    .unwrap();
            }
            outbox.mark(&item.id, QueueState::Sent, None, 21).unwrap();
            assert_eq!(outbox.next().unwrap().id, item.id, "the next waits");
            outbox
                .mark(&item.id, QueueState::Answered, None, 22)
                .unwrap();
            sent.push(q.request_id.clone());
        }
        assert_eq!(sent, ["r1", "r2", "r3"]);
        // And the states are kept across another restart.
        let again = Outbox::open(sealed(&dir)).unwrap();
        assert!(again.list().iter().all(|i| i.state == QueueState::Answered));
        assert!(again.next().is_none());
    }

    #[test]
    fn only_the_queue_s_own_steps_are_taken() {
        let dir = temp_dir("outbox-steps");
        let outbox = Outbox::open(sealed(&dir)).unwrap();
        outbox.add("i1", question(1), 1).unwrap();
        assert_eq!(
            outbox.mark("i1", QueueState::Answered, None, 2),
            Err(OutboxError::Transition {
                from: QueueState::Waiting,
                to: QueueState::Answered
            })
        );
        let failed = outbox
            .mark("i1", QueueState::Failed, Some("Refused".into()), 3)
            .unwrap();
        assert_eq!(failed.error.as_deref(), Some("Refused"));
        assert!(
            outbox.next().is_none(),
            "a failed item does not hold up the rest"
        );
        // Try again: waiting, without the old reason.
        let again = outbox.mark("i1", QueueState::Waiting, None, 4).unwrap();
        assert_eq!(again.error, None);
        outbox.mark("i1", QueueState::Sending, None, 5).unwrap();
        assert_eq!(outbox.remove("i1"), Err(OutboxError::Busy));
        assert_eq!(
            outbox.mark("nope", QueueState::Sent, None, 6),
            Err(OutboxError::Unknown)
        );
        assert!(
            outbox.add("i1", question(1), 7).is_err(),
            "an id is used once"
        );
    }

    #[test]
    fn a_full_queue_refuses_more_and_finished_items_are_pruned() {
        let dir = temp_dir("outbox-full");
        let outbox = Outbox::open(sealed(&dir)).unwrap();
        for n in 0..MAX_WAITING {
            outbox
                .add(&format!("i{n}"), question(n as u32), n as u64)
                .unwrap();
        }
        assert_eq!(outbox.add("more", question(0), 0), Err(OutboxError::Full));
        for n in 0..MAX_WAITING {
            outbox
                .mark(&format!("i{n}"), QueueState::Failed, None, 1_000 + n as u64)
                .unwrap();
        }
        let left = outbox.list();
        assert_eq!(left.len(), KEEP_FINISHED);
        assert_eq!(
            left[0].id,
            format!("i{}", MAX_WAITING - KEEP_FINISHED),
            "the newest are kept"
        );
    }

    #[test]
    fn a_failed_save_changes_nothing() {
        struct Refusing;
        impl OutboxStore for Refusing {
            fn load(&self) -> Result<Vec<QueuedItem>, String> {
                Ok(Vec::new())
            }
            fn save(&self, _: &[QueuedItem]) -> Result<(), String> {
                Err("disk full".into())
            }
        }
        let outbox = Outbox::open(Refusing).unwrap();
        assert_eq!(
            outbox.add("i1", question(1), 1),
            Err(OutboxError::Store("disk full".into()))
        );
        assert!(outbox.list().is_empty());
    }
}
