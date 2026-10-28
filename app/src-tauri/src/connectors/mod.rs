//! Connectors: Shodh working inside other tools (the Connectors view). One module per
//! connector; their settings live under `connectors` in `app_settings.json`.

pub mod conversations;
pub mod slack;
pub mod slack_index;
pub mod slack_notices;
mod slack_usage;
pub mod telegram;
pub mod telegram_notices;

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Every connector's settings. User-only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorsPrefs {
    pub slack: slack::SlackPrefs,
    pub telegram: telegram::TelegramPrefs,
}

/// Waits up to `limit` for a connector's loop that was told to stop to end, and aborts it
/// when it has not: dropping the handle instead would leave it running, polling or holding
/// a socket next to the loop that replaces it (two pollers on one Telegram token conflict;
/// two Slack sockets split the events). True when the loop ended by itself.
pub(crate) async fn end_task<T>(
    mut task: tauri::async_runtime::JoinHandle<T>,
    limit: Duration,
) -> bool {
    match tokio::time::timeout(limit, &mut task).await {
        Ok(_) => true,
        Err(_) => {
            task.abort();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use tauri::async_runtime::JoinHandle;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_loop_that_stops_in_time_is_waited_for() {
        let task = JoinHandle::Tokio(tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }));
        assert!(end_task(task, Duration::from_secs(3)).await);
    }

    /// The loop ignores its stop signal: after the wait it is aborted, never left running.
    #[tokio::test(start_paused = true)]
    async fn a_loop_that_ignores_its_stop_is_aborted() {
        let still_running = Arc::new(AtomicBool::new(false));
        let flag = still_running.clone();
        let task = JoinHandle::Tokio(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
            flag.store(true, Ordering::SeqCst);
        }));
        let started = tokio::time::Instant::now();
        assert!(!end_task(task, Duration::from_secs(3)).await);
        let waited = started.elapsed();
        assert!(waited >= Duration::from_secs(3) && waited < Duration::from_secs(4));
        // Past the hour the loop would have reached: it never runs again.
        tokio::time::sleep(Duration::from_secs(7_200)).await;
        assert!(!still_running.load(Ordering::SeqCst));
    }
}
