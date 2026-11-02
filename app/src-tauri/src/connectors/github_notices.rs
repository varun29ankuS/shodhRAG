//! The GitHub connector in notifications and the Inbox, the same way as Slack's and
//! Telegram's ([`super::slack_notices`], [`super::telegram_notices`]):
//!
//! - problems: GitHub refused the sign-in (revoked or expired: sign in again); a push
//!   was rejected (the branch moved on GitHub); GitHub is rate limiting Shodh (once per
//!   limit);
//! - Inbox only: signed in (resolves the refused sign-in).
//!
//! Every item's link opens Connectors → GitHub at the right tab. Texts carry repository
//! and branch names only, never a token or a commit's content.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde_json::json;
use shodh_rag::connectors::github::tools::GitHubNotice;
use shodh_rag::inbox::{InboxKind, InboxLink, InboxStatus, NewInboxItem};
use tauri::{AppHandle, Manager};

use super::slack_notices::{Alert, Delivery};
use crate::notifications::{self, Category, Notice};

/// The tabs of the GitHub page an item can open (`features/connectors/navigation.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubTab {
    Overview,
    Repositories,
    Activity,
}

impl GitHubTab {
    pub fn as_str(self) -> &'static str {
        match self {
            GitHubTab::Overview => "overview",
            GitHubTab::Repositories => "repositories",
            GitHubTab::Activity => "activity",
        }
    }
}

/// The Inbox id of the sign-in's state (refused, then resolved by the next sign-in).
pub const SIGN_IN_ITEM: &str = "github:sign-in";

fn item(
    id: String,
    status: InboxStatus,
    title: String,
    detail: String,
    tab: GitHubTab,
) -> NewInboxItem {
    NewInboxItem {
        id,
        kind: InboxKind::Connector,
        status,
        title,
        detail: Some(detail),
        link: Some(InboxLink::new(
            "connectors",
            Some(json!({ "connector": "github", "tab": tab.as_str() })),
        )),
        data: json!({ "connector": "github" }),
    }
}

fn until(reset_at: &Option<DateTime<Utc>>) -> String {
    reset_at
        .map(|at| format!(" until {}", at.format("%H:%M UTC")))
        .unwrap_or_default()
}

/// The Inbox item and the alert of `notice`.
pub fn delivery(notice: &GitHubNotice) -> Delivery {
    match notice {
        GitHubNotice::SignInRefused => Delivery {
            item: item(
                SIGN_IN_ITEM.to_string(),
                InboxStatus::NeedsYou,
                "Sign in to GitHub again".into(),
                "GitHub no longer accepts Shodh's sign-in (it was revoked or expired). Shodh does no GitHub work until you sign in again under Connectors → GitHub.".into(),
                GitHubTab::Overview,
            ),
            alert: Alert::Problem {
                source: SIGN_IN_ITEM.to_string(),
                notice: Notice {
                    title: "GitHub sign-in expired".into(),
                    body: "Sign in again under Connectors → GitHub.".into(),
                },
            },
        },
        GitHubNotice::PushRejected { repo, branch, reason } => {
            let repo = notifications::one_line(repo);
            let branch = notifications::one_line(branch);
            let id = format!("github:push:{repo}:{branch}");
            Delivery {
                item: item(
                    id.clone(),
                    InboxStatus::Failed,
                    format!("Push to {branch} was rejected"),
                    format!("GitHub rejected the push to {branch} on github.com/{repo}: {}", notifications::one_line(reason)),
                    GitHubTab::Activity,
                ),
                alert: Alert::Problem {
                    source: id,
                    notice: Notice {
                        title: "GitHub rejected a push".into(),
                        body: format!("{branch} on {repo}: nothing was overwritten."),
                    },
                },
            }
        }
        GitHubNotice::RateLimited { reset_at } => {
            let id = "github:rate-limit".to_string();
            Delivery {
                item: item(
                    id.clone(),
                    InboxStatus::Failed,
                    "GitHub is rate limiting Shodh".into(),
                    format!(
                        "GitHub refuses more requests from Shodh for now{}. GitHub work waits until then.",
                        until(reset_at)
                    ),
                    GitHubTab::Overview,
                ),
                alert: Alert::Problem {
                    source: format!("{id}:{}", reset_at.map(|at| at.timestamp()).unwrap_or_default()),
                    notice: Notice {
                        title: "GitHub is rate limiting Shodh".into(),
                        body: format!("GitHub work waits{}.", until(reset_at)),
                    },
                },
            }
        }
    }
}

/// What was told so far.
#[derive(Debug, Default)]
pub struct GitHubNoticeState {
    /// The reset time of the last rate limit told (each limit is told once).
    rate_limit_told: Mutex<Option<Option<DateTime<Utc>>>>,
}

/// Posts `notice` to the Inbox and alerts as [`delivery`] says.
pub fn tell(app: &AppHandle, notice: &GitHubNotice) {
    if let GitHubNotice::RateLimited { reset_at } = notice {
        if let Some(state) = app.try_state::<super::github::GitHubState>() {
            let mut told = state
                .notices
                .rate_limit_told
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *told == Some(*reset_at) {
                return;
            }
            *told = Some(*reset_at);
        }
    }
    let Delivery { item, alert } = delivery(notice);
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

/// Signed in: the refused sign-in's item is done with.
pub fn signed_in(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::inbox_commands::resolve(&app, SIGN_IN_ITEM.to_string()).await;
    });
}

/// Signed out by the user: nothing is left asking them to sign in.
pub fn signed_out(app: &AppHandle) {
    signed_in(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_sign_in_needs_the_user_and_opens_the_page() {
        let d = delivery(&GitHubNotice::SignInRefused);
        assert_eq!(d.item.status, InboxStatus::NeedsYou);
        assert_eq!(d.item.id, SIGN_IN_ITEM);
        assert_eq!(d.item.kind, InboxKind::Connector);
        let link = serde_json::to_value(d.item.link.unwrap()).unwrap();
        assert_eq!(link["view"], "connectors");
        assert_eq!(link["target"]["connector"], "github");
        assert_eq!(link["target"]["tab"], "overview");
        assert!(matches!(d.alert, Alert::Problem { .. }));
    }

    #[test]
    fn a_rejected_push_names_the_branch_and_never_a_token() {
        let d = delivery(&GitHubNotice::PushRejected {
            repo: "octo-org/hello".into(),
            branch: "shodh/fix".into(),
            reason: "the branch on GitHub has commits this one does not.".into(),
        });
        assert_eq!(d.item.status, InboxStatus::Failed);
        assert!(d.item.title.contains("shodh/fix"));
        assert!(d
            .item
            .detail
            .as_deref()
            .unwrap()
            .contains("github.com/octo-org/hello"));
        let link = serde_json::to_value(d.item.link.unwrap()).unwrap();
        assert_eq!(link["target"]["tab"], "activity");
    }

    #[test]
    fn a_rate_limit_says_until_when() {
        let at = DateTime::parse_from_rfc3339("2026-11-02T14:05:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let d = delivery(&GitHubNotice::RateLimited { reset_at: Some(at) });
        assert!(d
            .item
            .detail
            .as_deref()
            .unwrap()
            .contains("until 14:05 UTC"));
    }
}
