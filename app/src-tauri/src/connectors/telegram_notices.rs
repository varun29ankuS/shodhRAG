//! The Telegram connector in notifications and the Inbox, the same way as Slack's
//! ([`super::slack_notices`]):
//!
//! - problems: the connection lost and not back within
//!   [`super::slack_notices::DOWN_NOTICE_AFTER`] (once, then once "back"), or stopped for a
//!   reason the user must fix (a refused token, another program reading the bot); the
//!   model failing an answer; the daily cap of others' answers reached (once a UTC day);
//! - **needs you** (Inbox): a group added the bot and waits to be allowed;
//! - Inbox only: someone used an invite and can ask now.
//!
//! Every item's link opens Connectors → Telegram at the right tab. Texts carry names and
//! counts only, never a message's text. Nothing per answered question.

use std::sync::Mutex;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::json;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use tauri::{AppHandle, Manager};

use super::slack_notices::{Alert, Delivery, DOWN_NOTICE_AFTER};
use crate::notifications::{self, Category, Notice};

/// The tabs of the Telegram page an item can open (`features/connectors/navigation.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramTab {
    Overview,
    People,
    Settings,
}

impl TelegramTab {
    pub fn as_str(self) -> &'static str {
        match self {
            TelegramTab::Overview => "overview",
            TelegramTab::People => "people",
            TelegramTab::Settings => "settings",
        }
    }
}

/// Something the user is told about Telegram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramNotice {
    ConnectionLost,
    ConnectionStopped,
    ConnectionBack,
    CapReached {
        day: NaiveDate,
        cap: usize,
    },
    ModelFailed,
    /// The bot was added to a group, which waits for the owner to allow it.
    GroupWaiting {
        group: i64,
        title: String,
        by: Option<String>,
    },
    /// Someone used an invite link.
    PersonJoined {
        person: i64,
        name: String,
    },
}

/// What one state change tells.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Observed {
    pub notices: Vec<TelegramNotice>,
    pub recheck_in: Option<std::time::Duration>,
}

/// What the user was told, to tell only what changed. The first state after a start only
/// seeds it (a restart never repeats an announcement).
#[derive(Debug, Default)]
pub struct TelegramWatch {
    down_since: Option<DateTime<Utc>>,
    lost_told: bool,
    stopped_told: bool,
    cap_day: Option<NaiveDate>,
}

impl TelegramWatch {
    /// The notices of the card's state word `state` seen at `now`.
    pub fn observe(&mut self, state: &str, now: DateTime<Utc>) -> Observed {
        let mut observed = Observed::default();
        match state {
            "connected" => {
                if self.lost_told || self.stopped_told {
                    observed.notices.push(TelegramNotice::ConnectionBack);
                }
                self.down_since = None;
                self.lost_told = false;
                self.stopped_told = false;
            }
            "connecting" | "reconnecting" => {
                let since = *self.down_since.get_or_insert(now);
                let down_for = now - since;
                if !self.lost_told && !self.stopped_told {
                    if down_for >= DOWN_NOTICE_AFTER {
                        self.lost_told = true;
                        observed.notices.push(TelegramNotice::ConnectionLost);
                    } else if since == now {
                        observed.recheck_in = (DOWN_NOTICE_AFTER - down_for).to_std().ok();
                    }
                }
            }
            "error" => {
                if !self.stopped_told {
                    self.stopped_told = true;
                    observed.notices.push(TelegramNotice::ConnectionStopped);
                }
                self.down_since = None;
            }
            _ => {
                self.down_since = None;
                self.lost_told = false;
                self.stopped_told = false;
            }
        }
        observed
    }

    /// Others' answers met the daily cap at `now`: told once a UTC day.
    pub fn cap_reached(&mut self, cap: usize, now: DateTime<Utc>) -> Option<TelegramNotice> {
        let day = now.date_naive();
        if self.cap_day == Some(day) {
            return None;
        }
        self.cap_day = Some(day);
        Some(TelegramNotice::CapReached { day, cap })
    }
}

fn item(
    id: String,
    status: InboxStatus,
    title: String,
    detail: String,
    tab: TelegramTab,
) -> NewInboxItem {
    NewInboxItem {
        id,
        kind: InboxKind::Connector,
        status,
        title,
        detail: Some(detail),
        link: Some(InboxLink::new(
            "connectors",
            Some(json!({ "connector": "telegram", "tab": tab.as_str() })),
        )),
        data: json!({ "connector": "telegram" }),
    }
}

/// The Inbox id of a group waiting to be allowed (removed once it is decided).
pub fn group_item_id(group: i64) -> String {
    format!("telegram:group:{group}")
}

/// The Inbox item and the alert of `notice` for the bot `@bot`.
pub fn delivery(bot: &str, notice: &TelegramNotice) -> Delivery {
    let bot = format!("@{}", notifications::one_line(bot));
    let connection = "telegram:connection".to_string();
    match notice {
        TelegramNotice::ConnectionLost => Delivery {
            item: item(
                connection.clone(),
                InboxStatus::Failed,
                "Telegram connection lost".into(),
                format!(
                    "Shodh has not reached Telegram for {} minutes. It keeps trying; messages to {bot} wait until it is back.",
                    DOWN_NOTICE_AFTER.num_minutes()
                ),
                TelegramTab::Overview,
            ),
            alert: Alert::Problem {
                source: connection,
                notice: Notice {
                    title: "Telegram connection lost".into(),
                    body: format!("{bot}: Shodh keeps trying to reconnect."),
                },
            },
        },
        TelegramNotice::ConnectionStopped => Delivery {
            item: item(
                connection.clone(),
                InboxStatus::Failed,
                "Telegram connection stopped".into(),
                format!("Shodh stopped reading {bot}. Open the Telegram page to see what to fix."),
                TelegramTab::Overview,
            ),
            alert: Alert::Problem {
                source: connection,
                notice: Notice {
                    title: "Telegram connection stopped".into(),
                    body: format!("{bot}: open Connectors → Telegram to fix it."),
                },
            },
        },
        TelegramNotice::ConnectionBack => Delivery {
            item: item(
                connection,
                InboxStatus::Done,
                "Telegram connection is back".into(),
                format!("Reading {bot} again."),
                TelegramTab::Overview,
            ),
            alert: Alert::Resolved(Notice {
                title: "Telegram connection is back".into(),
                body: format!("Reading {bot} again."),
            }),
        },
        TelegramNotice::CapReached { day, cap } => {
            let id = "telegram:cap".to_string();
            Delivery {
                item: item(
                    id.clone(),
                    InboxStatus::Failed,
                    "Telegram answers stopped for today".into(),
                    format!(
                        "People and groups you allowed used all {cap} of today’s answers; {bot} says so until midnight UTC. Your own chat is not limited. Raise the limit under Settings."
                    ),
                    TelegramTab::Settings,
                ),
                alert: Alert::Problem {
                    source: format!("{id}:{day}"),
                    notice: Notice {
                        title: "Telegram answers stopped for today".into(),
                        body: format!("{bot} reached the daily limit of {cap} answers."),
                    },
                },
            }
        }
        TelegramNotice::ModelFailed => {
            let id = "telegram:model".to_string();
            Delivery {
                item: item(
                    id.clone(),
                    InboxStatus::Failed,
                    "A Telegram answer failed".into(),
                    "The model set in Settings → Model could not answer a message in Telegram. The person who asked was told.".into(),
                    TelegramTab::Overview,
                ),
                alert: Alert::Problem {
                    source: id,
                    notice: Notice {
                        title: "A Telegram answer failed".into(),
                        body: format!("{bot}: the model could not answer. Check Settings → Model."),
                    },
                },
            }
        }
        TelegramNotice::GroupWaiting { group, title, by } => {
            let title = notifications::one_line(title);
            let added = by
                .as_deref()
                .map(|name| format!(" {} added {bot} to it.", notifications::one_line(name)))
                .unwrap_or_default();
            Delivery {
                item: item(
                    group_item_id(*group),
                    InboxStatus::NeedsYou,
                    format!("Allow “{title}” to ask Shodh?"),
                    format!("A Telegram group wants answers.{added} Nobody there gets an answer until you allow it under People & groups."),
                    TelegramTab::People,
                ),
                alert: Alert::None,
            }
        }
        TelegramNotice::PersonJoined { person, name } => Delivery {
            item: item(
                format!("telegram:person:{person}"),
                InboxStatus::Done,
                format!("{} can ask Shodh in Telegram", notifications::one_line(name)),
                "They used your invite link. They get answers from the workspace you chose, never your memories.".into(),
                TelegramTab::People,
            ),
            alert: Alert::None,
        },
    }
}

/// What was told so far.
#[derive(Debug, Default)]
pub struct TelegramNoticeState {
    pub(super) watch: Mutex<TelegramWatch>,
}

/// Posts `notice` to the Inbox and alerts as [`delivery`] says.
pub fn tell(app: &AppHandle, bot: &str, notice: &TelegramNotice) {
    let Delivery { item, alert } = delivery(bot, notice);
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

/// The group's Inbox item is done with (allowed or removed).
pub fn group_decided(app: &AppHandle, group: i64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::inbox_commands::resolve(&app, group_item_id(group)).await;
    });
}

/// The card's state changed: tells what changed, and looks again when the connection
/// went down.
pub fn observe_state(app: &AppHandle, bot: &str, state: &str) {
    let Some(telegram) = app.try_state::<super::telegram::TelegramState>() else {
        return;
    };
    let observed = telegram
        .notices
        .watch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .observe(state, Utc::now());
    for notice in &observed.notices {
        tell(app, bot, notice);
    }
    if let Some(delay) = observed.recheck_in {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay + std::time::Duration::from_secs(1)).await;
            super::telegram::emit_status(&app);
        });
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 27, 9, minute, 0).unwrap()
    }

    #[test]
    fn down_for_three_minutes_is_told_once_and_back_once() {
        let mut watch = TelegramWatch::default();
        assert!(watch.observe("connected", at(0)).notices.is_empty());
        let first = watch.observe("reconnecting", at(1));
        assert!(first.notices.is_empty());
        assert_eq!(first.recheck_in, Some(std::time::Duration::from_secs(180)));
        assert!(watch.observe("reconnecting", at(2)).notices.is_empty());
        assert_eq!(
            watch.observe("reconnecting", at(4)).notices,
            vec![TelegramNotice::ConnectionLost]
        );
        assert!(
            watch.observe("reconnecting", at(9)).notices.is_empty(),
            "once"
        );
        assert_eq!(
            watch.observe("connected", at(10)).notices,
            vec![TelegramNotice::ConnectionBack]
        );
        // A short drop says nothing.
        watch.observe("reconnecting", at(11));
        assert!(watch.observe("connected", at(12)).notices.is_empty());
    }

    #[test]
    fn a_stop_is_told_at_once_and_the_users_choices_never() {
        let mut watch = TelegramWatch::default();
        assert_eq!(
            watch.observe("error", at(0)).notices,
            vec![TelegramNotice::ConnectionStopped]
        );
        assert!(watch.observe("error", at(1)).notices.is_empty());
        assert!(watch.observe("off", at(2)).notices.is_empty());
        assert!(watch.observe("local_only", at(3)).notices.is_empty());
        assert!(
            watch.observe("connected", at(4)).notices.is_empty(),
            "off is not a problem"
        );
    }

    #[test]
    fn the_cap_is_told_once_a_day() {
        let mut watch = TelegramWatch::default();
        assert!(watch.cap_reached(20, at(0)).is_some());
        assert!(watch.cap_reached(20, at(5)).is_none());
        let tomorrow = Utc.with_ymd_and_hms(2026, 10, 28, 0, 1, 0).unwrap();
        assert!(watch.cap_reached(20, tomorrow).is_some());
    }

    #[test]
    fn a_waiting_group_needs_the_user_and_links_to_people() {
        let d = delivery(
            "shodh_notes_bot",
            &TelegramNotice::GroupWaiting {
                group: -100_200,
                title: "Deals".into(),
                by: Some("Ann Lee".into()),
            },
        );
        assert_eq!(d.item.status, InboxStatus::NeedsYou);
        assert_eq!(d.item.id, "telegram:group:-100200");
        assert_eq!(d.item.kind, InboxKind::Connector);
        assert!(d.item.title.contains("Deals"));
        assert!(d
            .item
            .detail
            .as_deref()
            .unwrap()
            .contains("Ann Lee added @shodh_notes_bot"));
        let link = serde_json::to_value(d.item.link.unwrap()).unwrap();
        assert_eq!(link["target"]["connector"], "telegram");
        assert_eq!(link["target"]["tab"], "people");
        assert_eq!(d.alert, Alert::None);
    }

    #[test]
    fn problems_alert_and_name_only_the_bot() {
        for notice in [
            TelegramNotice::ConnectionLost,
            TelegramNotice::ConnectionStopped,
            TelegramNotice::ModelFailed,
            TelegramNotice::CapReached {
                day: at(0).date_naive(),
                cap: 20,
            },
        ] {
            let d = delivery("shodh_notes_bot", &notice);
            assert!(matches!(d.alert, Alert::Problem { .. }), "{notice:?}");
            assert_eq!(d.item.status, InboxStatus::Failed);
        }
        let back = delivery("shodh_notes_bot", &TelegramNotice::ConnectionBack);
        assert_eq!(back.item.status, InboxStatus::Done);
        assert_eq!(back.item.id, "telegram:connection");
    }
}
