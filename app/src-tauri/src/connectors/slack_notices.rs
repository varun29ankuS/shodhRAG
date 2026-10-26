//! The Slack connector in notifications and the Inbox.
//!
//! **What is said** (and nothing per answered question):
//! - problems: the connection lost and not back within [`DOWN_NOTICE_AFTER`] (once, then
//!   once "back"), or stopped for a reason the user must fix; the model failing a Slack
//!   answer; the Slack app missing permissions (after Shodh's manifest grew); an indexed
//!   channel that stopped (Shodh removed, archived, shared with other organizations now,
//!   an error it cannot retry); the daily cap of answers reached (once a UTC day);
//! - work finished: a channel's first backfill ("#deals: 214 threads indexed");
//! - Inbox only, never a desktop alert: someone other than the owner saved a memory for a
//!   channel, so the owner sees what is stored without a review step.
//!
//! **Where**: every notice is an Inbox item of kind `connector` whose link opens
//! Connectors → Slack at the right tab (a desktop notification's click does not reach the
//! app, see [`crate::notifications`]). Item ids are stable per subject, so a later state
//! replaces an earlier one ("lost" becomes "back"). Problems and finished work also go to
//! [`crate::notifications`], which applies the user's switches, the window rule, the
//! problem cooldown and the one-a-minute batching.
//!
//! **When**: [`SlackWatch`] compares each status the card is sent with the last one. The
//! first status it sees only seeds it, so a restart never announces again what was already
//! so (a channel indexed last week, a channel stopped yesterday). Pure, unit-tested.
//!
//! **Text**: names (the Slack workspace, a channel, a person) and counts only; never a
//! memory's text or Slack's error messages.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::json;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use tauri::{AppHandle, Manager};

use super::slack::{emit_status, SlackState, SlackStatus};
use crate::notifications::{self, Category, Notice, WorkDone, WorkKind};

/// How long the connection may be down before the user is told.
pub const DOWN_NOTICE_AFTER: chrono::Duration = chrono::Duration::minutes(3);

/// The tabs of the Slack page an item can open (`features/connectors/navigation.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackTab {
    Overview,
    Answering,
    Indexing,
    Memories,
}

impl SlackTab {
    pub fn as_str(self) -> &'static str {
        match self {
            SlackTab::Overview => "overview",
            SlackTab::Answering => "answering",
            SlackTab::Indexing => "indexing",
            SlackTab::Memories => "memories",
        }
    }
}

/// Why an indexed channel stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStop {
    Removed,
    Archived,
    NowShared,
    Error,
}

impl ChannelStop {
    /// From the card's state word of a channel; `None` while it is still indexed.
    pub fn of(state: &str) -> Option<Self> {
        match state {
            "removed" => Some(ChannelStop::Removed),
            "archived" => Some(ChannelStop::Archived),
            "now_shared" => Some(ChannelStop::NowShared),
            "error" => Some(ChannelStop::Error),
            _ => None,
        }
    }

    fn words(self) -> &'static str {
        match self {
            ChannelStop::Removed => "Shodh was removed from the channel. Invite it again, then index the channel again.",
            ChannelStop::Archived => "The channel was archived in Slack.",
            ChannelStop::NowShared => {
                "The channel is shared with other organizations now. Index it again to confirm."
            }
            ChannelStop::Error => {
                "Indexing stopped with an error Shodh cannot retry on its own. The Slack page says why."
            }
        }
    }

    fn short(self) -> &'static str {
        match self {
            ChannelStop::Removed => "Shodh was removed from it",
            ChannelStop::Archived => "archived in Slack",
            ChannelStop::NowShared => "shared with other organizations now",
            ChannelStop::Error => "stopped with an error",
        }
    }
}

/// What the card's status says about one indexed channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSnapshot {
    pub id: String,
    pub name: String,
    /// The card's state word (`backfilling`, `up_to_date`, `removed`, …).
    pub state: String,
    pub backfill_done: bool,
    pub threads: usize,
}

/// What [`SlackWatch`] reads from one status of the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub team_id: String,
    pub team_name: String,
    /// The card's state word (`connected`, `reconnecting`, `error`, `off`, …).
    pub state: String,
    /// Permissions the Slack app lacks, for answering or for indexing.
    pub missing_scopes: Vec<String>,
    /// The indexed channels; `None` when they could not be read for this status (an empty
    /// list then would look like every channel gone, and back again on the next status).
    pub channels: Option<Vec<ChannelSnapshot>>,
}

impl Snapshot {
    /// The snapshot of `status`; `None` when nothing is installed.
    pub fn of(status: &SlackStatus) -> Option<Self> {
        let team_id = status.team_id.clone()?;
        let installation = status.installation.as_ref()?;
        let mut missing_scopes: Vec<String> = status
            .missing_scopes
            .iter()
            .chain(status.index.missing_scopes.iter())
            .cloned()
            .collect();
        missing_scopes.sort();
        missing_scopes.dedup();
        Some(Snapshot {
            team_id,
            team_name: installation.team_name.clone(),
            state: status.state.to_string(),
            missing_scopes,
            channels: status.index.channels_read.then(|| {
                status
                    .index
                    .channels
                    .iter()
                    .map(|c| ChannelSnapshot {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        state: c.state.to_string(),
                        backfill_done: c.backfill_done,
                        threads: c.threads,
                    })
                    .collect()
            }),
        })
    }
}

/// Something the user is told about Slack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackNotice {
    /// Down (connecting or reconnecting) for at least [`DOWN_NOTICE_AFTER`].
    ConnectionLost,
    /// Stopped for a reason the user must fix (the card says which).
    ConnectionStopped,
    /// Connected again after [`SlackNotice::ConnectionLost`] or
    /// [`SlackNotice::ConnectionStopped`].
    ConnectionBack,
    MissingScopes(Vec<String>),
    ChannelStopped {
        id: String,
        name: String,
        stop: ChannelStop,
    },
    BackfillDone {
        id: String,
        name: String,
        threads: usize,
    },
    CapReached {
        day: NaiveDate,
        cap: usize,
    },
    ModelFailed,
    /// Someone other than the owner saved a memory for a channel.
    MemorySaved {
        memory_id: String,
        channel: String,
        author: String,
    },
}

/// What one status tells: notices, and when to look again (the connection went down and
/// is not "lost" yet; nothing else would send a status meanwhile).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Observed {
    pub notices: Vec<SlackNotice>,
    pub recheck_in: Option<std::time::Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChannelSeen {
    stopped: bool,
    backfill_done: bool,
}

#[derive(Debug, Default)]
struct TeamWatch {
    /// The channels were read once: later changes are news.
    channels_seeded: bool,
    down_since: Option<DateTime<Utc>>,
    lost_told: bool,
    stopped_told: bool,
    scopes_told: Vec<String>,
    channels: HashMap<String, ChannelSeen>,
    cap_day: Option<NaiveDate>,
}

/// What the user was told about each Slack workspace, to tell only what changed.
#[derive(Debug, Default)]
pub struct SlackWatch {
    teams: HashMap<String, TeamWatch>,
}

impl SlackWatch {
    /// The notices of `snapshot`, seen at `now`.
    pub fn observe(&mut self, snapshot: &Snapshot, now: DateTime<Utc>) -> Observed {
        let team = self.teams.entry(snapshot.team_id.clone()).or_default();
        let mut observed = Observed::default();
        let notices = &mut observed.notices;

        match snapshot.state.as_str() {
            "connected" => {
                if team.lost_told || team.stopped_told {
                    notices.push(SlackNotice::ConnectionBack);
                }
                team.down_since = None;
                team.lost_told = false;
                team.stopped_told = false;
            }
            "connecting" | "reconnecting" => {
                let since = match team.down_since {
                    Some(since) => since,
                    None => {
                        team.down_since = Some(now);
                        now
                    }
                };
                let down_for = now - since;
                if !team.lost_told && !team.stopped_told {
                    if down_for >= DOWN_NOTICE_AFTER {
                        team.lost_told = true;
                        notices.push(SlackNotice::ConnectionLost);
                    } else if since == now {
                        observed.recheck_in = (DOWN_NOTICE_AFTER - down_for).to_std().ok();
                    }
                }
            }
            "error" => {
                if !team.stopped_told {
                    team.stopped_told = true;
                    notices.push(SlackNotice::ConnectionStopped);
                }
                team.down_since = None;
            }
            // Off, Local-only mode, disconnected: the user's choice, nothing to tell.
            _ => {
                team.down_since = None;
                team.lost_told = false;
                team.stopped_told = false;
            }
        }

        if snapshot.missing_scopes.is_empty() {
            team.scopes_told.clear();
        } else if snapshot.missing_scopes != team.scopes_told {
            team.scopes_told = snapshot.missing_scopes.clone();
            notices.push(SlackNotice::MissingScopes(snapshot.missing_scopes.clone()));
        }

        let Some(channels) = &snapshot.channels else {
            return observed;
        };
        let mut seen = HashMap::with_capacity(channels.len());
        for channel in channels {
            let stop = ChannelStop::of(&channel.state);
            let now_seen = ChannelSeen {
                stopped: stop.is_some(),
                backfill_done: channel.backfill_done,
            };
            if team.channels_seeded {
                // A channel first seen now was chosen in this session: nothing told yet.
                let before = team
                    .channels
                    .get(&channel.id)
                    .copied()
                    .unwrap_or(ChannelSeen {
                        stopped: false,
                        backfill_done: false,
                    });
                if let Some(stop) = stop.filter(|_| !before.stopped) {
                    notices.push(SlackNotice::ChannelStopped {
                        id: channel.id.clone(),
                        name: channel.name.clone(),
                        stop,
                    });
                }
                if channel.backfill_done && !before.backfill_done {
                    notices.push(SlackNotice::BackfillDone {
                        id: channel.id.clone(),
                        name: channel.name.clone(),
                        threads: channel.threads,
                    });
                }
            }
            seen.insert(channel.id.clone(), now_seen);
        }
        team.channels = seen;
        team.channels_seeded = true;
        observed
    }

    /// A question of `team_id` met the daily cap at `now`: told once a UTC day.
    pub fn cap_reached(
        &mut self,
        team_id: &str,
        cap: usize,
        now: DateTime<Utc>,
    ) -> Option<SlackNotice> {
        let team = self.teams.entry(team_id.to_string()).or_default();
        let day = now.date_naive();
        if team.cap_day == Some(day) {
            return None;
        }
        team.cap_day = Some(day);
        Some(SlackNotice::CapReached { day, cap })
    }
}

/// How a notice reaches the user besides its Inbox item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Alert {
    /// [`notifications::problem`], at most once per cooldown per source.
    Problem { source: String, notice: Notice },
    /// [`notifications::work_finished`], batched.
    Finished(WorkDone),
    /// The connection is back: a problems notification, once.
    Resolved(Notice),
    /// Inbox only.
    None,
}

/// What `notice` posts and alerts.
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub item: NewInboxItem,
    pub alert: Alert,
}

fn link(tab: SlackTab) -> InboxLink {
    InboxLink::new(
        "connectors",
        Some(json!({ "connector": "slack", "tab": tab.as_str() })),
    )
}

fn item(
    id: String,
    status: InboxStatus,
    title: String,
    detail: String,
    tab: SlackTab,
) -> NewInboxItem {
    NewInboxItem {
        id,
        kind: InboxKind::Connector,
        status,
        title,
        detail: Some(detail),
        link: Some(link(tab)),
        data: json!({ "connector": "slack" }),
    }
}

fn threads(n: usize) -> String {
    format!("{n} {}", if n == 1 { "thread" } else { "threads" })
}

/// The Inbox item and the alert of `notice` for the Slack workspace `team_id` named
/// `team`.
pub fn delivery(team_id: &str, team: &str, notice: &SlackNotice) -> Delivery {
    let team = notifications::one_line(team);
    let connection = format!("slack:connection:{team_id}");
    match notice {
        SlackNotice::ConnectionLost => Delivery {
            item: item(
                connection.clone(),
                InboxStatus::Failed,
                "Slack connection lost".to_string(),
                format!(
                    "Shodh has not reached {team} for {} minutes. It keeps trying; questions in Slack wait until it is back.",
                    DOWN_NOTICE_AFTER.num_minutes()
                ),
                SlackTab::Overview,
            ),
            alert: Alert::Problem {
                source: connection,
                notice: Notice {
                    title: "Slack connection lost".to_string(),
                    body: format!("{team}: Shodh keeps trying to reconnect."),
                },
            },
        },
        SlackNotice::ConnectionStopped => Delivery {
            item: item(
                connection.clone(),
                InboxStatus::Failed,
                "Slack connection stopped".to_string(),
                format!("Shodh stopped connecting to {team}. Open the Slack page to see what to fix."),
                SlackTab::Overview,
            ),
            alert: Alert::Problem {
                source: connection,
                notice: Notice {
                    title: "Slack connection stopped".to_string(),
                    body: format!("{team}: open Connectors → Slack to fix it."),
                },
            },
        },
        SlackNotice::ConnectionBack => Delivery {
            item: item(
                connection,
                InboxStatus::Done,
                "Slack connection is back".to_string(),
                format!("Connected to {team} again."),
                SlackTab::Overview,
            ),
            alert: Alert::Resolved(Notice {
                title: "Slack connection is back".to_string(),
                body: format!("Connected to {team} again."),
            }),
        },
        SlackNotice::MissingScopes(scopes) => {
            let id = format!("slack:scopes:{team_id}");
            Delivery {
                item: item(
                    id.clone(),
                    InboxStatus::Failed,
                    "The Slack app needs more permissions".to_string(),
                    format!(
                        "Add {} to the Slack app, reinstall it, then connect again with the new bot token.",
                        scopes.join(", ")
                    ),
                    SlackTab::Overview,
                ),
                alert: Alert::Problem {
                    source: id,
                    notice: Notice {
                        title: "The Slack app needs more permissions".to_string(),
                        body: format!("{team}: update the Slack app from Connectors → Slack."),
                    },
                },
            }
        }
        SlackNotice::ChannelStopped { id, name, stop } => {
            let key = format!("slack:channel:{team_id}:{id}");
            let name = notifications::one_line(name);
            Delivery {
                item: item(
                    key.clone(),
                    InboxStatus::Failed,
                    format!("#{name} stopped indexing"),
                    stop.words().to_string(),
                    SlackTab::Indexing,
                ),
                alert: Alert::Problem {
                    source: key,
                    notice: Notice {
                        title: "A Slack channel stopped indexing".to_string(),
                        body: format!("#{name}: {}.", stop.short()),
                    },
                },
            }
        }
        SlackNotice::BackfillDone { id, name, threads: n } => {
            let key = format!("slack:channel:{team_id}:{id}");
            let name = notifications::one_line(name);
            let summary = format!("#{name}: {} indexed", threads(*n));
            Delivery {
                item: item(
                    key.clone(),
                    InboxStatus::Done,
                    summary.clone(),
                    format!("The first read of #{name} is done. New messages are added as they come."),
                    SlackTab::Indexing,
                ),
                alert: Alert::Finished(WorkDone {
                    source: key,
                    kind: WorkKind::SlackChannel,
                    notice: Notice {
                        title: "Slack channel indexed".to_string(),
                        body: summary,
                    },
                    summary: format!("#{name}"),
                }),
            }
        }
        SlackNotice::CapReached { day, cap } => {
            let id = format!("slack:cap:{team_id}");
            Delivery {
                item: item(
                    id.clone(),
                    InboxStatus::Failed,
                    "Slack answers stopped for today".to_string(),
                    format!(
                        "{team} used all {cap} of today’s answers; the bot says so until midnight UTC. Raise the limit under Answering."
                    ),
                    SlackTab::Answering,
                ),
                alert: Alert::Problem {
                    source: format!("{id}:{day}"),
                    notice: Notice {
                        title: "Slack answers stopped for today".to_string(),
                        body: format!("{team} reached the daily limit of {cap} answers."),
                    },
                },
            }
        }
        SlackNotice::ModelFailed => {
            let id = format!("slack:model:{team_id}");
            Delivery {
                item: item(
                    id.clone(),
                    InboxStatus::Failed,
                    "A Slack answer failed".to_string(),
                    format!(
                        "The model set in Settings → Model could not answer a question from {team}. The person who asked was told."
                    ),
                    SlackTab::Overview,
                ),
                alert: Alert::Problem {
                    source: id,
                    notice: Notice {
                        title: "A Slack answer failed".to_string(),
                        body: format!("{team}: the model could not answer. Check Settings → Model."),
                    },
                },
            }
        }
        SlackNotice::MemorySaved {
            memory_id,
            channel,
            author,
        } => {
            let channel = notifications::one_line(channel);
            Delivery {
                item: item(
                    format!("slack:memory:{memory_id}"),
                    InboxStatus::Done,
                    format!("Saved for {channel} by {}", notifications::one_line(author)),
                    format!("Kept for {channel} only. Open it to read, keep or delete it."),
                    SlackTab::Memories,
                ),
                alert: Alert::None,
            }
        }
    }
}

/// What was told so far (kept in [`SlackState`]).
#[derive(Debug, Default)]
pub struct SlackNoticeState {
    watch: Mutex<SlackWatch>,
}

/// Posts `notice` to the Inbox and alerts as [`delivery`] says.
pub fn tell(app: &AppHandle, team_id: &str, team: &str, notice: &SlackNotice) {
    let Delivery { item, alert } = delivery(team_id, team, notice);
    match alert {
        Alert::Problem { source, notice } => {
            notifications::problem(app, source, notice.title, notice.body)
        }
        Alert::Finished(done) => notifications::work_finished(app, done),
        Alert::Resolved(notice) => {
            notifications::notify(app, Category::Problems, notice.title, notice.body)
        }
        Alert::None => {}
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::inbox_commands::post(&app, item).await;
    });
}

/// A status was sent to the card: tells what changed, and looks again when the
/// connection went down.
pub fn observe_status(app: &AppHandle, status: &SlackStatus) {
    let Some(snapshot) = Snapshot::of(status) else {
        return;
    };
    let Some(state) = app.try_state::<SlackState>() else {
        return;
    };
    let observed = state
        .notices
        .watch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .observe(&snapshot, Utc::now());
    for notice in &observed.notices {
        tell(app, &snapshot.team_id, &snapshot.team_name, notice);
    }
    if let Some(delay) = observed.recheck_in {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            // A little past the limit, so the next status is surely past it.
            tokio::time::sleep(delay + std::time::Duration::from_secs(1)).await;
            emit_status(&app);
        });
    }
}

/// A question of `team_id` met the daily cap.
pub fn cap_reached(app: &AppHandle, team_id: &str, team: &str, cap: usize) {
    let Some(state) = app.try_state::<SlackState>() else {
        return;
    };
    let notice = state
        .notices
        .watch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cap_reached(team_id, cap, Utc::now());
    if let Some(notice) = notice {
        tell(app, team_id, team, &notice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 26, 9, minute, 0).unwrap()
    }

    fn channel(id: &str, state: &str, backfill_done: bool, threads: usize) -> ChannelSnapshot {
        ChannelSnapshot {
            id: id.to_string(),
            name: format!("name-{id}"),
            state: state.to_string(),
            backfill_done,
            threads,
        }
    }

    fn snapshot(state: &str, channels: Vec<ChannelSnapshot>) -> Snapshot {
        Snapshot {
            team_id: "T1".to_string(),
            team_name: "Acme".to_string(),
            state: state.to_string(),
            missing_scopes: Vec::new(),
            channels: Some(channels),
        }
    }

    #[test]
    fn the_first_status_only_seeds_what_was_already_so() {
        let mut watch = SlackWatch::default();
        let first = watch.observe(
            &snapshot(
                "connected",
                vec![
                    channel("C1", "up_to_date", true, 214),
                    channel("C2", "removed", true, 9),
                ],
            ),
            at(0),
        );
        assert_eq!(first, Observed::default());
        let again = watch.observe(
            &snapshot(
                "connected",
                vec![
                    channel("C1", "up_to_date", true, 220),
                    channel("C2", "removed", true, 9),
                ],
            ),
            at(1),
        );
        assert_eq!(again, Observed::default());
    }

    #[test]
    fn channels_that_could_not_be_read_are_neither_gone_nor_new() {
        let mut watch = SlackWatch::default();
        let all = || {
            vec![
                channel("C1", "up_to_date", true, 214),
                channel("C2", "removed", true, 9),
            ]
        };
        // The first read fails: nothing is seeded from an empty list.
        let mut unread = snapshot("connected", vec![]);
        unread.channels = None;
        assert_eq!(watch.observe(&unread, at(0)), Observed::default());
        assert_eq!(
            watch.observe(&snapshot("connected", all()), at(1)),
            Observed::default()
        );
        // A read fails mid-session: the channels are not forgotten, so not told again.
        assert_eq!(watch.observe(&unread, at(2)), Observed::default());
        assert_eq!(
            watch.observe(&snapshot("connected", all()), at(3)),
            Observed::default()
        );
        // The connection is still watched while the channels cannot be read.
        unread.state = "error".into();
        assert_eq!(
            watch.observe(&unread, at(4)).notices,
            vec![SlackNotice::ConnectionStopped]
        );
    }

    #[test]
    fn a_first_backfill_is_told_once_when_it_is_done_whatever_paused_it() {
        let mut watch = SlackWatch::default();
        watch.observe(&snapshot("connected", vec![]), at(0));
        // Chosen in this session: backfilling, paused by the rate limit, backfilling, done.
        for state in ["backfilling", "paused", "backfilling"] {
            let seen = watch.observe(
                &snapshot("connected", vec![channel("C1", state, false, 40)]),
                at(1),
            );
            assert!(seen.notices.is_empty(), "{state}");
        }
        let done = watch.observe(
            &snapshot("connected", vec![channel("C1", "paused", true, 214)]),
            at(2),
        );
        assert_eq!(
            done.notices,
            vec![SlackNotice::BackfillDone {
                id: "C1".into(),
                name: "name-C1".into(),
                threads: 214
            }]
        );
        let later = watch.observe(
            &snapshot("connected", vec![channel("C1", "up_to_date", true, 230)]),
            at(3),
        );
        assert!(later.notices.is_empty());
    }

    #[test]
    fn a_channel_that_stops_is_told_once_until_it_runs_again() {
        let mut watch = SlackWatch::default();
        watch.observe(
            &snapshot("connected", vec![channel("C1", "up_to_date", true, 5)]),
            at(0),
        );
        let stopped = watch.observe(
            &snapshot("connected", vec![channel("C1", "removed", true, 5)]),
            at(1),
        );
        assert_eq!(
            stopped.notices,
            vec![SlackNotice::ChannelStopped {
                id: "C1".into(),
                name: "name-C1".into(),
                stop: ChannelStop::Removed
            }]
        );
        let still = watch.observe(
            &snapshot("connected", vec![channel("C1", "removed", true, 5)]),
            at(2),
        );
        assert!(still.notices.is_empty());
        watch.observe(
            &snapshot("connected", vec![channel("C1", "up_to_date", true, 5)]),
            at(3),
        );
        let again = watch.observe(
            &snapshot("connected", vec![channel("C1", "error", true, 5)]),
            at(4),
        );
        assert_eq!(again.notices.len(), 1);
        // Paused and forgetting are not stops.
        for state in ["paused", "forgetting", "backfilling"] {
            assert_eq!(ChannelStop::of(state), None, "{state}");
        }
        assert_eq!(ChannelStop::of("archived"), Some(ChannelStop::Archived));
        assert_eq!(ChannelStop::of("now_shared"), Some(ChannelStop::NowShared));
    }

    #[test]
    fn a_connection_down_for_a_few_minutes_is_told_once_and_back_once() {
        let mut watch = SlackWatch::default();
        watch.observe(&snapshot("connected", vec![]), at(0));
        let down = watch.observe(&snapshot("reconnecting", vec![]), at(1));
        assert!(down.notices.is_empty());
        assert_eq!(down.recheck_in, Some(std::time::Duration::from_secs(180)));
        // Each retry sends a status; none is told before the limit, and none asks again.
        let retry = watch.observe(&snapshot("connecting", vec![]), at(2));
        assert_eq!(retry, Observed::default());
        let lost = watch.observe(&snapshot("reconnecting", vec![]), at(4));
        assert_eq!(lost.notices, vec![SlackNotice::ConnectionLost]);
        for minute in 5..9 {
            assert_eq!(
                watch.observe(&snapshot("reconnecting", vec![]), at(minute)),
                Observed::default()
            );
        }
        let back = watch.observe(&snapshot("connected", vec![]), at(9));
        assert_eq!(back.notices, vec![SlackNotice::ConnectionBack]);
        assert!(watch
            .observe(&snapshot("connected", vec![]), at(10))
            .notices
            .is_empty());
    }

    #[test]
    fn a_short_drop_says_nothing() {
        let mut watch = SlackWatch::default();
        watch.observe(&snapshot("connected", vec![]), at(0));
        watch.observe(&snapshot("reconnecting", vec![]), at(1));
        let back = watch.observe(&snapshot("connected", vec![]), at(2));
        assert_eq!(back, Observed::default());
    }

    #[test]
    fn a_stop_is_told_at_once_and_switching_off_is_not_a_problem() {
        let mut watch = SlackWatch::default();
        let stopped = watch.observe(&snapshot("error", vec![]), at(0));
        assert_eq!(stopped.notices, vec![SlackNotice::ConnectionStopped]);
        assert!(watch
            .observe(&snapshot("error", vec![]), at(1))
            .notices
            .is_empty());
        // New tokens: connecting, then connected.
        assert!(watch
            .observe(&snapshot("connecting", vec![]), at(2))
            .notices
            .is_empty());
        let back = watch.observe(&snapshot("connected", vec![]), at(2));
        assert_eq!(back.notices, vec![SlackNotice::ConnectionBack]);
        // Off and Local-only mode are the user's choice: no notice, and none when it returns.
        watch.observe(&snapshot("reconnecting", vec![]), at(3));
        watch.observe(&snapshot("reconnecting", vec![]), at(7));
        assert!(watch
            .observe(&snapshot("off", vec![]), at(8))
            .notices
            .is_empty());
        assert!(watch
            .observe(&snapshot("local_only", vec![]), at(9))
            .notices
            .is_empty());
        assert!(watch
            .observe(&snapshot("connected", vec![]), at(10))
            .notices
            .is_empty());
    }

    #[test]
    fn missing_permissions_are_told_when_they_appear_or_change() {
        let mut watch = SlackWatch::default();
        let mut s = snapshot("connected", vec![]);
        s.missing_scopes = vec!["channels:history".into()];
        // Shodh's manifest grew in an update: told at the first status too.
        assert_eq!(
            watch.observe(&s, at(0)).notices,
            vec![SlackNotice::MissingScopes(vec!["channels:history".into()])]
        );
        assert!(watch.observe(&s, at(1)).notices.is_empty());
        s.missing_scopes.push("groups:history".into());
        assert_eq!(watch.observe(&s, at(2)).notices.len(), 1);
        s.missing_scopes.clear();
        assert!(watch.observe(&s, at(3)).notices.is_empty());
    }

    #[test]
    fn the_daily_cap_is_told_once_a_utc_day() {
        let mut watch = SlackWatch::default();
        assert!(watch.cap_reached("T1", 20, at(0)).is_some());
        assert!(watch.cap_reached("T1", 20, at(30)).is_none());
        assert!(watch.cap_reached("T2", 20, at(30)).is_some());
        let tomorrow = Utc.with_ymd_and_hms(2026, 10, 27, 0, 5, 0).unwrap();
        assert!(watch.cap_reached("T1", 20, tomorrow).is_some());
    }

    #[test]
    fn every_item_opens_the_slack_page_at_its_tab_and_keeps_one_id_per_subject() {
        let lost = delivery("T1", "Acme", &SlackNotice::ConnectionLost);
        let back = delivery("T1", "Acme", &SlackNotice::ConnectionBack);
        assert_eq!(lost.item.id, back.item.id, "back replaces lost");
        assert_eq!(lost.item.kind, InboxKind::Connector);
        assert_eq!(lost.item.status, InboxStatus::Failed);
        assert_eq!(back.item.status, InboxStatus::Done);
        let link = lost.item.link.clone().unwrap();
        assert_eq!(link.view, "connectors");
        assert_eq!(
            link.target,
            Some(json!({ "connector": "slack", "tab": "overview" }))
        );
        assert!(matches!(back.alert, Alert::Resolved(_)));

        let done = delivery(
            "T1",
            "Acme",
            &SlackNotice::BackfillDone {
                id: "C1".into(),
                name: "deals".into(),
                threads: 214,
            },
        );
        assert_eq!(done.item.title, "#deals: 214 threads indexed");
        assert_eq!(
            done.item.link.unwrap().target,
            Some(json!({ "connector": "slack", "tab": "indexing" }))
        );
        match done.alert {
            Alert::Finished(work) => {
                assert_eq!(work.kind, WorkKind::SlackChannel);
                assert_eq!(work.notice.body, "#deals: 214 threads indexed");
            }
            other => panic!("{other:?}"),
        }

        let cap = delivery(
            "T1",
            "Acme",
            &SlackNotice::CapReached {
                day: at(0).date_naive(),
                cap: 20,
            },
        );
        assert_eq!(
            cap.item.link.unwrap().target,
            Some(json!({ "connector": "slack", "tab": "answering" }))
        );
        assert!(
            matches!(cap.alert, Alert::Problem { ref source, .. } if source == "slack:cap:T1:2026-10-26")
        );
    }

    #[test]
    fn a_memory_saved_by_someone_else_goes_to_the_inbox_only_and_never_quotes_it() {
        let saved = delivery(
            "T1",
            "Acme",
            &SlackNotice::MemorySaved {
                memory_id: "slackmem-1".into(),
                channel: "#deals".into(),
                author: "Alex".into(),
            },
        );
        assert_eq!(saved.alert, Alert::None);
        assert_eq!(saved.item.title, "Saved for #deals by Alex");
        assert_eq!(saved.item.id, "slack:memory:slackmem-1");
        assert_eq!(
            saved.item.link.unwrap().target,
            Some(json!({ "connector": "slack", "tab": "memories" }))
        );
    }

    #[test]
    fn notices_name_things_and_never_carry_slack_errors() {
        let stopped = delivery(
            "T1",
            "Acme",
            &SlackNotice::ChannelStopped {
                id: "C1".into(),
                name: "deals".into(),
                stop: ChannelStop::Error,
            },
        );
        match stopped.alert {
            Alert::Problem { source, notice } => {
                assert_eq!(source, "slack:channel:T1:C1");
                assert_eq!(notice.body, "#deals: stopped with an error.");
            }
            other => panic!("{other:?}"),
        }
        let model = delivery("T1", "Acme", &SlackNotice::ModelFailed);
        assert!(
            matches!(model.alert, Alert::Problem { ref source, .. } if source == "slack:model:T1")
        );
    }
}
