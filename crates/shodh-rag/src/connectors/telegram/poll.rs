//! The long-polling loop: `getUpdates` from this computer, one call at a time, each
//! waiting up to [`super::api::LONG_POLL_SECONDS`] for something new.
//!
//! - **Checkpoint.** Telegram keeps an update until a later call's `offset` confirms it.
//!   The next offset is saved ([`OffsetStore`]) *before* the updates are handed on, so an
//!   update is answered at most once: a crash right after the save loses that update
//!   instead of answering it twice. A restart resumes from the saved offset.
//! - **Never waits on an answer.** Updates go to an [`UpdateHandler`] that starts them in
//!   the background and returns at once.
//! - **Telegram's limits.** A 429 waits the `retry_after` Telegram gives. A network error
//!   or a 5xx waits with exponential backoff ([`PollTiming`]), which starts over after a
//!   call succeeds.
//! - **What the user must fix stops the loop**, with the reason on the card, instead of
//!   retrying forever: a refused token (401), and a conflict (409: a webhook is set on the
//!   bot, or another program polls with the same token).
//! - **Stopping** (switched off, disconnected, Local-only mode, quit) ends the loop at
//!   once, even in the middle of a long poll or a wait.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::Rng;
use tokio_util::sync::CancellationToken;

use super::api::{BotApi, TelegramError, LONG_POLL_SECONDS};
use super::respond::{Dispatcher, Room};
use super::updates::{parse_update, Update};

/// Where the next offset is kept between runs.
pub trait OffsetStore: Send + Sync {
    /// The saved next offset (`None`: never polled).
    fn load(&self) -> Result<Option<i64>, String>;
    fn save(&self, next: i64) -> Result<(), String>;
}

/// Takes updates and starts them. Taking waits while there is no room, so an update is
/// confirmed to Telegram only once it was taken; what was not taken stays with Telegram.
#[async_trait]
pub trait UpdateHandler: Send + Sync {
    /// Room for `update` (`None`: `stop` came first, and the update is not taken).
    async fn room(&self, update: &Update, stop: &CancellationToken) -> Option<Room>;
    /// Starts `update` in `room`; returns at once.
    fn start(&self, update: Update, room: Room);
}

#[async_trait]
impl UpdateHandler for Dispatcher {
    async fn room(&self, update: &Update, stop: &CancellationToken) -> Option<Room> {
        Dispatcher::room(self, update, stop).await
    }

    fn start(&self, update: Update, room: Room) {
        Dispatcher::start(self, update, room);
    }
}

/// The connection as the card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollState {
    Connecting,
    Connected,
    /// Waiting before the next call.
    Waiting {
        next_try_at: DateTime<Utc>,
        reason: String,
    },
    /// Stopped for a reason the user must fix.
    Stopped(String),
    Off,
}

/// Told about the connection.
pub trait PollSink: Send + Sync {
    fn state(&self, state: PollState);
    /// An update arrived at `at`.
    fn update_seen(&self, at: DateTime<Utc>);
}

/// The waits of the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollTiming {
    /// Seconds each `getUpdates` waits.
    pub long_poll: u32,
    pub backoff_start: Duration,
    pub backoff_max: Duration,
    /// ±20 % random spread on backoff waits (off in tests).
    pub jitter: bool,
}

impl Default for PollTiming {
    fn default() -> Self {
        Self {
            long_poll: LONG_POLL_SECONDS,
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(300),
            jitter: true,
        }
    }
}

/// What the card says when the token was refused.
pub const TOKEN_REFUSED: &str = "Telegram refused the bot token: it was revoked in @BotFather, or the bot was deleted. Connect again with the bot's current token.";
/// What the card says on a conflict.
pub const CONFLICT: &str = "Another program is reading this bot's messages: a webhook another tool set on the bot, or Shodh on another computer. Stop the other one (or make a new token with /revoke in @BotFather and connect with it), then switch the connector on again here.";

fn spread(wait: Duration, jitter: bool) -> Duration {
    if !jitter {
        return wait;
    }
    let factor = rand::thread_rng().gen_range(0.8..1.2);
    wait.mul_f64(factor)
}

/// Polls `bot` until `stop`, handing every update to `handler` and saving the offset in
/// `offsets`.
pub async fn run(
    bot: Arc<dyn BotApi>,
    offsets: Arc<dyn OffsetStore>,
    handler: Arc<dyn UpdateHandler>,
    sink: Arc<dyn PollSink>,
    stop: CancellationToken,
    timing: PollTiming,
) {
    // The checkpoint is a file: read and written off the async workers.
    let loader = offsets.clone();
    let mut offset = tokio::task::spawn_blocking(move || loader.load())
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .unwrap_or_else(|e| {
            tracing::warn!(target: "shodh::telegram", error = %e, "the Telegram checkpoint could not be read; starting from what Telegram keeps");
            None
        });
    let mut backoff = timing.backoff_start;
    let mut connected = false;
    sink.state(PollState::Connecting);
    loop {
        let result = tokio::select! {
            result = bot.updates(offset, timing.long_poll) => result,
            _ = stop.cancelled() => break,
        };
        let wait = match result {
            Ok(updates) => {
                backoff = timing.backoff_start;
                if !connected {
                    connected = true;
                    sink.state(PollState::Connected);
                }
                let parsed: Vec<Update> = updates
                    .iter()
                    .filter_map(parse_update)
                    .filter(|u| offset.is_none_or(|o| u.id() >= o))
                    .collect();
                if !parsed.is_empty() {
                    sink.update_seen(Utc::now());
                }
                for update in parsed {
                    // Room first: while there is none, nothing more is confirmed, and
                    // Telegram keeps the rest for the next call.
                    let Some(room) = handler.room(&update, &stop).await else {
                        break;
                    };
                    let next = update.id() + 1;
                    offset = Some(next);
                    // Saved before the answer starts: at most once.
                    let saver = offsets.clone();
                    let saved = tokio::task::spawn_blocking(move || saver.save(next))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                    if let Err(e) = saved {
                        tracing::warn!(target: "shodh::telegram", error = %e, "the Telegram checkpoint could not be saved");
                    }
                    handler.start(update, room);
                }
                if stop.is_cancelled() {
                    break;
                }
                continue;
            }
            Err(TelegramError::Unauthorized) => {
                sink.state(PollState::Stopped(TOKEN_REFUSED.to_string()));
                return;
            }
            Err(TelegramError::Conflict) => {
                sink.state(PollState::Stopped(CONFLICT.to_string()));
                return;
            }
            Err(TelegramError::RateLimited { retry_after }) => {
                connected = false;
                (
                    retry_after,
                    "Telegram asked Shodh to wait (rate limited)".to_string(),
                )
            }
            Err(error) => {
                connected = false;
                let wait = spread(backoff, timing.jitter);
                backoff = (backoff * 2).min(timing.backoff_max);
                tracing::info!(target: "shodh::telegram", error = %error, wait_ms = wait.as_millis() as u64, "Telegram could not be reached; trying again");
                (wait, error.to_string())
            }
        };
        let (delay, reason) = wait;
        sink.state(PollState::Waiting {
            next_try_at: Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default(),
            reason,
        });
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = stop.cancelled() => break,
        }
    }
    sink.state(PollState::Off);
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::connectors::telegram::updates::tests::message_value;

    /// A Telegram answering `getUpdates` from a script; once the script is done, a long
    /// poll that never returns.
    struct ScriptedBot {
        script: Mutex<VecDeque<Result<Vec<Value>, TelegramError>>>,
        offsets: Mutex<Vec<Option<i64>>>,
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl BotApi for ScriptedBot {
        async fn updates(
            &self,
            offset: Option<i64>,
            _wait: u32,
        ) -> Result<Vec<Value>, TelegramError> {
            self.offsets.lock().unwrap().push(offset);
            self.log.lock().unwrap().push(format!("poll {offset:?}"));
            let next = self.script.lock().unwrap().pop_front();
            match next {
                Some(result) => result,
                None => {
                    tokio::time::sleep(Duration::from_secs(3_600)).await;
                    Ok(Vec::new())
                }
            }
        }
        async fn send(
            &self,
            _c: i64,
            _t: &str,
            _h: bool,
            _r: Option<i64>,
        ) -> Result<i64, TelegramError> {
            Ok(1)
        }
        async fn typing(&self, _c: i64) -> Result<(), TelegramError> {
            Ok(())
        }
    }

    struct MemoryOffsets {
        saved: Mutex<Option<i64>>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl OffsetStore for MemoryOffsets {
        fn load(&self) -> Result<Option<i64>, String> {
            Ok(*self.saved.lock().unwrap())
        }
        fn save(&self, next: i64) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("save {next}"));
            *self.saved.lock().unwrap() = Some(next);
            Ok(())
        }
    }

    struct Recorder {
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl UpdateHandler for Recorder {
        async fn room(&self, _update: &Update, _stop: &CancellationToken) -> Option<Room> {
            Some(Room::unlimited())
        }
        fn start(&self, update: Update, _room: Room) {
            self.log
                .lock()
                .unwrap()
                .push(format!("handle {}", update.id()));
        }
    }

    #[derive(Default)]
    struct States(Mutex<Vec<PollState>>);

    impl PollSink for States {
        fn state(&self, state: PollState) {
            self.0.lock().unwrap().push(state);
        }
        fn update_seen(&self, _at: DateTime<Utc>) {}
    }

    struct Run {
        bot: Arc<ScriptedBot>,
        offsets: Arc<MemoryOffsets>,
        states: Arc<States>,
        log: Arc<Mutex<Vec<String>>>,
        stop: CancellationToken,
    }

    fn timing() -> PollTiming {
        PollTiming {
            long_poll: 30,
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(4),
            jitter: false,
        }
    }

    fn start(
        script: Vec<Result<Vec<Value>, TelegramError>>,
        saved: Option<i64>,
    ) -> (Run, tokio::task::JoinHandle<()>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let bot = Arc::new(ScriptedBot {
            script: Mutex::new(script.into()),
            offsets: Mutex::new(Vec::new()),
            log: log.clone(),
        });
        let offsets = Arc::new(MemoryOffsets {
            saved: Mutex::new(saved),
            log: log.clone(),
        });
        let states = Arc::new(States::default());
        let stop = CancellationToken::new();
        let task = tokio::spawn(run(
            bot.clone(),
            offsets.clone(),
            Arc::new(Recorder { log: log.clone() }),
            states.clone(),
            stop.clone(),
            timing(),
        ));
        (
            Run {
                bot,
                offsets,
                states,
                log,
                stop,
            },
            task,
        )
    }

    /// Lets the loop run until it waits on the never-ending poll.
    async fn settle() {
        tokio::time::sleep(Duration::from_secs(600)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_offset_is_saved_before_the_updates_are_answered() {
        let (r, task) = start(
            vec![Ok(vec![
                message_value(5, 42, 42, "a"),
                message_value(6, 42, 42, "b"),
            ])],
            None,
        );
        settle().await;
        assert_eq!(*r.bot.offsets.lock().unwrap(), vec![None, Some(7)]);
        assert_eq!(*r.offsets.saved.lock().unwrap(), Some(7));
        assert_eq!(
            r.log.lock().unwrap()[..4],
            ["poll None", "save 7", "handle 5", "handle 6"]
        );
        r.stop.cancel();
        task.await.unwrap();
        assert_eq!(r.states.0.lock().unwrap().last(), Some(&PollState::Off));
    }

    #[tokio::test(start_paused = true)]
    async fn a_restart_resumes_from_the_checkpoint_and_skips_what_was_answered() {
        let (r, task) = start(
            vec![Ok(vec![
                message_value(6, 42, 42, "already answered"),
                message_value(7, 42, 42, "new"),
            ])],
            Some(7),
        );
        settle().await;
        assert_eq!(r.bot.offsets.lock().unwrap()[0], Some(7));
        let handled: Vec<String> = r
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("handle"))
            .cloned()
            .collect();
        assert_eq!(handled, ["handle 7"]);
        r.stop.cancel();
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_waits_telegrams_retry_after() {
        let (r, task) = start(
            vec![
                Err(TelegramError::RateLimited {
                    retry_after: Duration::from_secs(12),
                }),
                Ok(vec![]),
            ],
            None,
        );
        let started = tokio::time::Instant::now();
        // Not yet polled again before the wait is over.
        tokio::time::sleep(Duration::from_secs(11)).await;
        assert_eq!(r.bot.offsets.lock().unwrap().len(), 1);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            r.bot.offsets.lock().unwrap().len(),
            3,
            "polled again after 12 s"
        );
        assert!(started.elapsed() >= Duration::from_secs(12));
        let states = r.states.0.lock().unwrap().clone();
        assert!(
            matches!(states[1], PollState::Waiting { ref reason, .. } if reason.contains("rate limited"))
        );
        assert_eq!(states[2], PollState::Connected);
        r.stop.cancel();
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn network_errors_back_off_and_start_over_after_success() {
        let net = || Err(TelegramError::Network("connection reset".into()));
        let (r, task) = start(vec![net(), net(), net(), net(), Ok(vec![]), net()], None);
        settle().await;
        let waits: Vec<String> = r
            .states
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|s| match s {
                PollState::Waiting { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(waits.len(), 5);
        // 1 + 2 + 4 + 4 (capped) seconds before the success, then 1 again.
        assert_eq!(r.bot.offsets.lock().unwrap().len(), 7);
        r.stop.cancel();
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_conflict_or_a_refused_token_stops_with_the_reason() {
        for (error, reason) in [
            (TelegramError::Conflict, CONFLICT),
            (TelegramError::Unauthorized, TOKEN_REFUSED),
        ] {
            let (r, task) = start(vec![Err(error)], None);
            task.await.unwrap();
            assert_eq!(
                r.states.0.lock().unwrap().last(),
                Some(&PollState::Stopped(reason.to_string()))
            );
            assert_eq!(r.bot.offsets.lock().unwrap().len(), 1, "no retry loop");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stopping_ends_a_long_poll_at_once() {
        let (r, task) = start(vec![], None);
        tokio::time::sleep(Duration::from_secs(1)).await;
        r.stop.cancel();
        tokio::time::timeout(Duration::from_millis(10), task)
            .await
            .expect("stopped at once")
            .unwrap();
        assert_eq!(r.states.0.lock().unwrap().last(), Some(&PollState::Off));
    }

    /// A handler with room for one update, never given back.
    struct OneRoom {
        taken: Mutex<Vec<i64>>,
    }

    #[async_trait]
    impl UpdateHandler for OneRoom {
        async fn room(&self, _update: &Update, stop: &CancellationToken) -> Option<Room> {
            if self.taken.lock().unwrap().is_empty() {
                return Some(Room::unlimited());
            }
            stop.cancelled().await;
            None
        }
        fn start(&self, update: Update, _room: Room) {
            self.taken.lock().unwrap().push(update.id());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn updates_without_room_stay_with_telegram() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let bot = Arc::new(ScriptedBot {
            script: Mutex::new(
                vec![Ok(vec![
                    message_value(5, 42, 42, "a"),
                    message_value(6, 43, 43, "b"),
                    message_value(7, 44, 44, "c"),
                ])]
                .into(),
            ),
            offsets: Mutex::new(Vec::new()),
            log: log.clone(),
        });
        let offsets = Arc::new(MemoryOffsets {
            saved: Mutex::new(None),
            log: log.clone(),
        });
        let handler = Arc::new(OneRoom {
            taken: Mutex::new(Vec::new()),
        });
        let stop = CancellationToken::new();
        let task = tokio::spawn(run(
            bot.clone(),
            offsets.clone(),
            handler.clone(),
            Arc::new(States::default()),
            stop.clone(),
            timing(),
        ));
        settle().await;
        // One taken and confirmed; the loop waits for room instead of dropping the rest.
        assert_eq!(*handler.taken.lock().unwrap(), vec![5]);
        assert_eq!(*offsets.saved.lock().unwrap(), Some(6));
        assert_eq!(
            bot.offsets.lock().unwrap().len(),
            1,
            "no new poll while full"
        );
        stop.cancel();
        task.await.unwrap();
        // 6 and 7 were never confirmed: Telegram gives them again from offset 6.
        assert_eq!(*offsets.saved.lock().unwrap(), Some(6));
    }
}
