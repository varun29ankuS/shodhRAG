//! Who may ask, and where: deny by default, decided before anything else is done with a
//! question (no search, no model call).
//!
//! - **Channels**: a mention is answered only in a channel the user put on the card's
//!   "Answer in these channels" list (empty at first). Anywhere else the bot replies
//!   [`NOT_ALLOWED`]. A channel shared with other organizations (Slack Connect) can be on
//!   the list; the card labels it. A channel put on the list while it was not shared, and
//!   shared since (Slack marks the event `is_ext_shared_channel`,
//!   <https://docs.slack.dev/apis/events-api/>), is refused until the user confirms it
//!   again.
//! - **Direct messages** are answered only for members of the Slack workspace the app is
//!   installed in. Slack puts no documented "external" flag on events: its Slack Connect
//!   guide says to compare the user's team with the installing team and to check
//!   `is_stranger` (<https://docs.slack.dev/apis/slack-connect>). So the asker is looked
//!   up with `users.info` (scope `users:read`) and must have the installing team's
//!   `team_id`, not be a stranger, not be a guest (`is_restricted`, `is_ultra_restricted`)
//!   and not be a bot (<https://docs.slack.dev/reference/objects/user-object>). A team on
//!   the event that is not the installing team, a DM in a shared conversation, or a lookup
//!   that fails are refused too.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lru::LruCache;

use super::events::{Question, QuestionKind};
use super::web::SlackApiError;
use crate::research::tables::calls::Clock;

/// The reply to a question from where, or from whom, Shodh does not answer.
pub const NOT_ALLOWED: &str = "Shodh isn't set to answer in this channel.";

/// How long a looked-up member is trusted before Slack is asked again.
pub const MEMBER_TTL_MINUTES: i64 = 10;
/// Members remembered at once.
pub const MEMBERS_CACHED: usize = 512;

/// A channel answers may be given in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedChannel {
    pub id: String,
    /// It was shared with another organization when the user chose it.
    pub shared: bool,
}

/// Where answers may be given, read at every question (a change on the card applies to
/// the next one).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessRules {
    /// The Slack workspace the app is installed in.
    pub team_id: String,
    pub channels: Vec<AllowedChannel>,
}

/// What Slack says about a person (`users.info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub team_id: String,
    /// The name people see in Slack (display name, else real name, else user name); never
    /// an e-mail address.
    pub name: String,
    /// A multi- or single-channel guest.
    pub guest: bool,
    /// From another workspace than the app's (`is_stranger`).
    pub stranger: bool,
    pub bot: bool,
    pub deleted: bool,
}

/// Why a question was refused (for the log; the asker always reads [`NOT_ALLOWED`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The rules could not be read (settings unreadable, the installation gone).
    RulesUnknown,
    ChannelNotAllowed,
    /// The channel was chosen while not shared and is shared with another organization
    /// now.
    ChannelNowShared,
    /// A DM in a conversation shared with another organization.
    SharedConversation,
    /// The asker belongs to another Slack workspace.
    OtherTeam,
    Guest,
    /// A bot or a deactivated account.
    NotAPerson,
    /// Slack could not say who asked.
    AskerUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Allowed,
    Refused(Refusal),
}

/// The rules of one installation. The app reads them from its settings.
#[async_trait]
pub trait AccessRulesSource: Send + Sync {
    async fn rules(&self) -> Result<AccessRules, String>;
    /// `channel`, chosen while not shared, is shared with another organization now (the
    /// card asks the user to confirm it again).
    fn channel_now_shared(&self, channel: &str);
}

/// Who a Slack user is (`users.info`).
#[async_trait]
pub trait MemberDirectory: Send + Sync {
    async fn member(&self, user: &str) -> Result<Member, SlackApiError>;
}

/// Decides whether a question is answered.
#[async_trait]
pub trait AccessGate: Send + Sync {
    async fn check(&self, question: &Question) -> Access;
    /// The asker, when they are a full member of the installing workspace (what saving a
    /// memory needs, in a channel too: a shared channel on the list lets people of other
    /// organizations ask, never save).
    async fn member(&self, question: &Question) -> Result<Member, Refusal>;
}

/// A mention in a channel: only on the list, and a channel that became shared since it
/// was chosen is refused.
pub fn channel_access(question: &Question, rules: &AccessRules) -> Access {
    match rules.channels.iter().find(|c| c.id == question.channel) {
        None => Access::Refused(Refusal::ChannelNotAllowed),
        Some(channel) if question.ext_shared && !channel.shared => {
            Access::Refused(Refusal::ChannelNowShared)
        }
        Some(_) => Access::Allowed,
    }
}

/// A DM, before the asker is looked up: refused at once when the event itself says the
/// asker or the conversation is from elsewhere; `None` when only a lookup can tell.
pub fn direct_access_from_event(question: &Question, rules: &AccessRules) -> Option<Access> {
    if question.ext_shared {
        return Some(Access::Refused(Refusal::SharedConversation));
    }
    match &question.asker_team {
        Some(team) if *team != rules.team_id => Some(Access::Refused(Refusal::OtherTeam)),
        _ => None,
    }
}

/// A DM from `member`: a full member of the installing workspace only.
pub fn member_access(member: &Member, rules: &AccessRules) -> Access {
    if member.team_id != rules.team_id || member.stranger {
        Access::Refused(Refusal::OtherTeam)
    } else if member.guest {
        Access::Refused(Refusal::Guest)
    } else if member.bot || member.deleted {
        Access::Refused(Refusal::NotAPerson)
    } else {
        Access::Allowed
    }
}

/// The gate of one installation: its rules from the app, its members from Slack
/// (remembered for [`MEMBER_TTL_MINUTES`]; a failed lookup is never remembered).
pub struct SlackAccessGate {
    rules: Arc<dyn AccessRulesSource>,
    directory: Arc<dyn MemberDirectory>,
    clock: Arc<dyn Clock>,
    members: Mutex<LruCache<String, (Member, DateTime<Utc>)>>,
}

impl SlackAccessGate {
    pub fn new(
        rules: Arc<dyn AccessRulesSource>,
        directory: Arc<dyn MemberDirectory>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let capacity = NonZeroUsize::new(MEMBERS_CACHED).unwrap_or(NonZeroUsize::MIN);
        Self {
            rules,
            directory,
            clock,
            members: Mutex::new(LruCache::new(capacity)),
        }
    }

    async fn lookup(&self, user: &str) -> Result<Member, SlackApiError> {
        let now = self.clock.now();
        let cached = self
            .members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(user)
            .filter(|(_, at)| {
                *at <= now && now.signed_duration_since(*at).num_minutes() < MEMBER_TTL_MINUTES
            })
            .map(|(member, _)| member.clone());
        if let Some(member) = cached {
            return Ok(member);
        }
        let member = self.directory.member(user).await?;
        self.members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .put(user.to_string(), (member.clone(), now));
        Ok(member)
    }
}

#[async_trait]
impl AccessGate for SlackAccessGate {
    async fn check(&self, question: &Question) -> Access {
        let rules = match self.rules.rules().await {
            Ok(rules) if !rules.team_id.is_empty() => rules,
            Ok(_) => return Access::Refused(Refusal::RulesUnknown),
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the Slack answer rules could not be read");
                return Access::Refused(Refusal::RulesUnknown);
            }
        };
        match question.kind {
            QuestionKind::Mention => {
                let access = channel_access(question, &rules);
                if access == Access::Refused(Refusal::ChannelNowShared) {
                    self.rules.channel_now_shared(&question.channel);
                }
                access
            }
            QuestionKind::Direct => {
                if let Some(refused) = direct_access_from_event(question, &rules) {
                    return refused;
                }
                match self.lookup(&question.user).await {
                    Ok(member) => member_access(&member, &rules),
                    Err(e) => {
                        tracing::warn!(target: "shodh::slack", error = %e, "who sent a Slack DM could not be checked; not answered");
                        Access::Refused(Refusal::AskerUnknown)
                    }
                }
            }
        }
    }

    async fn member(&self, question: &Question) -> Result<Member, Refusal> {
        self.full_member(question).await
    }
}

impl SlackAccessGate {
    /// The rules, or why they could not be read.
    async fn read_rules(&self) -> Result<AccessRules, Refusal> {
        match self.rules.rules().await {
            Ok(rules) if !rules.team_id.is_empty() => Ok(rules),
            Ok(_) => Err(Refusal::RulesUnknown),
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "the Slack answer rules could not be read");
                Err(Refusal::RulesUnknown)
            }
        }
    }

    /// The asker as a full member of the installing workspace.
    pub async fn full_member(&self, question: &Question) -> Result<Member, Refusal> {
        let rules = self.read_rules().await?;
        if let Some(team) = &question.asker_team {
            if *team != rules.team_id {
                return Err(Refusal::OtherTeam);
            }
        }
        let member = self.lookup(&question.user).await.map_err(|e| {
            tracing::warn!(target: "shodh::slack", error = %e, "who wrote a Slack message could not be checked");
            Refusal::AskerUnknown
        })?;
        match member_access(&member, &rules) {
            Access::Allowed => Ok(member),
            Access::Refused(refusal) => Err(refusal),
        }
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    use super::*;

    /// A gate with fixed rules and members, counting the lookups.
    pub struct FixedRules {
        pub rules: Result<AccessRules, String>,
        pub now_shared: Mutex<Vec<String>>,
    }

    impl FixedRules {
        pub fn allowing(team: &str, channels: &[(&str, bool)]) -> Self {
            Self {
                rules: Ok(AccessRules {
                    team_id: team.into(),
                    channels: channels
                        .iter()
                        .map(|(id, shared)| AllowedChannel {
                            id: (*id).into(),
                            shared: *shared,
                        })
                        .collect(),
                }),
                now_shared: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AccessRulesSource for FixedRules {
        async fn rules(&self) -> Result<AccessRules, String> {
            self.rules.clone()
        }
        fn channel_now_shared(&self, channel: &str) {
            self.now_shared.lock().unwrap().push(channel.into());
        }
    }

    pub struct FixedDirectory {
        pub members: std::collections::HashMap<String, Member>,
        pub lookups: Mutex<usize>,
    }

    impl FixedDirectory {
        pub fn new(members: &[(&str, Member)]) -> Self {
            Self {
                members: members
                    .iter()
                    .map(|(id, m)| ((*id).to_string(), m.clone()))
                    .collect(),
                lookups: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl MemberDirectory for FixedDirectory {
        async fn member(&self, user: &str) -> Result<Member, SlackApiError> {
            *self.lookups.lock().unwrap() += 1;
            self.members
                .get(user)
                .cloned()
                .ok_or_else(|| SlackApiError::Api {
                    code: "user_not_found".into(),
                    needed: None,
                })
        }
    }

    pub fn member(team: &str) -> Member {
        Member {
            team_id: team.into(),
            name: "Ann".into(),
            guest: false,
            stranger: false,
            bot: false,
            deleted: false,
        }
    }

    /// Answers mentions in `C1` and DMs from `U1` of team `T1`: what most tests need.
    pub fn open_gate() -> SlackAccessGate {
        SlackAccessGate::new(
            Arc::new(FixedRules::allowing("T1", &[("C1", false)])),
            Arc::new(FixedDirectory::new(&[("U1", member("T1"))])),
            Arc::new(crate::research::tables::calls::SystemClock),
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::testkit::*;
    use super::*;
    use crate::connectors::slack::events::ThreadKey;

    fn question(kind: QuestionKind, channel: &str, user: &str) -> Question {
        Question {
            event_id: "Ev1".into(),
            kind,
            channel: channel.into(),
            reply_thread_ts: None,
            thread: ThreadKey {
                channel: channel.into(),
                thread: String::new(),
            },
            user: user.into(),
            ts: "1.1".into(),
            in_thread: false,
            asker_team: None,
            ext_shared: false,
            has_files: false,
            text: "q".into(),
        }
    }

    struct StepClock(Mutex<DateTime<Utc>>);
    impl Clock for StepClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    fn make_gate(
        rules: FixedRules,
        directory: FixedDirectory,
    ) -> (
        SlackAccessGate,
        Arc<FixedRules>,
        Arc<FixedDirectory>,
        Arc<StepClock>,
    ) {
        let rules = Arc::new(rules);
        let directory = Arc::new(directory);
        let clock = Arc::new(StepClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 10, 23, 9, 0, 0).unwrap(),
        )));
        (
            SlackAccessGate::new(rules.clone(), directory.clone(), clock.clone()),
            rules,
            directory,
            clock,
        )
    }

    #[tokio::test]
    async fn mentions_are_answered_only_in_the_chosen_channels() {
        let (gate, _, directory, _) = make_gate(
            FixedRules::allowing("T1", &[("C1", false), ("CSHARED", true)]),
            FixedDirectory::new(&[]),
        );
        assert_eq!(
            gate.check(&question(QuestionKind::Mention, "C1", "U1"))
                .await,
            Access::Allowed
        );
        assert_eq!(
            gate.check(&question(QuestionKind::Mention, "C2", "U1"))
                .await,
            Access::Refused(Refusal::ChannelNotAllowed)
        );
        // A shared channel the user chose: people of the other organization may ask.
        let mut external = question(QuestionKind::Mention, "CSHARED", "UEXT");
        external.ext_shared = true;
        external.asker_team = Some("TCLIENT".into());
        assert_eq!(gate.check(&external).await, Access::Allowed);
        assert_eq!(
            *directory.lookups.lock().unwrap(),
            0,
            "no lookup for channels"
        );
    }

    #[tokio::test]
    async fn an_empty_list_answers_nowhere() {
        let (gate, _, _, _) = make_gate(FixedRules::allowing("T1", &[]), FixedDirectory::new(&[]));
        assert_eq!(
            gate.check(&question(QuestionKind::Mention, "C1", "U1"))
                .await,
            Access::Refused(Refusal::ChannelNotAllowed)
        );
    }

    #[tokio::test]
    async fn a_channel_shared_after_it_was_chosen_is_refused_and_reported() {
        let (gate, rules, _, _) = make_gate(
            FixedRules::allowing("T1", &[("C1", false)]),
            FixedDirectory::new(&[]),
        );
        let mut now_shared = question(QuestionKind::Mention, "C1", "U1");
        now_shared.ext_shared = true;
        assert_eq!(
            gate.check(&now_shared).await,
            Access::Refused(Refusal::ChannelNowShared)
        );
        assert_eq!(*rules.now_shared.lock().unwrap(), vec!["C1".to_string()]);
    }

    #[tokio::test]
    async fn only_full_members_may_save_even_in_a_shared_channel_on_the_list() {
        let guest = Member {
            guest: true,
            ..member("T1")
        };
        let (gate, _, _, _) = make_gate(
            FixedRules::allowing("T1", &[("CSHARED", true)]),
            FixedDirectory::new(&[
                ("U1", member("T1")),
                ("UEXT", member("TCLIENT")),
                ("UGUEST", guest),
            ]),
        );
        let in_shared = |user: &str| {
            let mut q = question(QuestionKind::Mention, "CSHARED", user);
            q.ext_shared = true;
            q
        };
        // People of the other organization may ask there…
        assert_eq!(gate.check(&in_shared("UEXT")).await, Access::Allowed);
        // …but not save.
        assert_eq!(
            AccessGate::member(&gate, &in_shared("UEXT")).await,
            Err(Refusal::OtherTeam)
        );
        assert_eq!(
            AccessGate::member(&gate, &in_shared("UGUEST")).await,
            Err(Refusal::Guest)
        );
        assert_eq!(
            AccessGate::member(&gate, &in_shared("UNOBODY")).await,
            Err(Refusal::AskerUnknown)
        );
        let mut from_elsewhere = in_shared("U1");
        from_elsewhere.asker_team = Some("TCLIENT".into());
        assert_eq!(
            AccessGate::member(&gate, &from_elsewhere).await,
            Err(Refusal::OtherTeam)
        );
        let me = AccessGate::member(&gate, &in_shared("U1")).await.unwrap();
        assert_eq!(me.name, "Ann");
    }

    #[tokio::test]
    async fn dms_are_answered_only_for_full_members_of_the_installing_workspace() {
        let guest = Member {
            guest: true,
            ..member("T1")
        };
        let stranger = Member {
            stranger: true,
            ..member("T1")
        };
        let bot = Member {
            bot: true,
            ..member("T1")
        };
        let (gate, _, directory, _) = make_gate(
            FixedRules::allowing("T1", &[]),
            FixedDirectory::new(&[
                ("U1", member("T1")),
                ("UEXT", member("TCLIENT")),
                ("UGUEST", guest),
                ("USTRANGER", stranger),
                ("UBOT2", bot),
            ]),
        );
        let dm = |user: &str| question(QuestionKind::Direct, "D1", user);
        assert_eq!(gate.check(&dm("U1")).await, Access::Allowed);
        assert_eq!(
            gate.check(&dm("UEXT")).await,
            Access::Refused(Refusal::OtherTeam)
        );
        assert_eq!(
            gate.check(&dm("UGUEST")).await,
            Access::Refused(Refusal::Guest)
        );
        assert_eq!(
            gate.check(&dm("USTRANGER")).await,
            Access::Refused(Refusal::OtherTeam)
        );
        assert_eq!(
            gate.check(&dm("UBOT2")).await,
            Access::Refused(Refusal::NotAPerson)
        );
        assert_eq!(
            gate.check(&dm("UNKNOWN")).await,
            Access::Refused(Refusal::AskerUnknown),
            "a failed lookup refuses"
        );
        let looked_up = *directory.lookups.lock().unwrap();
        // The event's own team and a shared conversation refuse without a lookup.
        let mut other_team = dm("U1");
        other_team.asker_team = Some("TCLIENT".into());
        assert_eq!(
            gate.check(&other_team).await,
            Access::Refused(Refusal::OtherTeam)
        );
        let mut shared = dm("U1");
        shared.ext_shared = true;
        assert_eq!(
            gate.check(&shared).await,
            Access::Refused(Refusal::SharedConversation)
        );
        assert_eq!(*directory.lookups.lock().unwrap(), looked_up);
        // The event's team matching is not enough on its own: the lookup still decides.
        let mut same_team_guest = dm("UGUEST");
        same_team_guest.asker_team = Some("T1".into());
        assert_eq!(
            gate.check(&same_team_guest).await,
            Access::Refused(Refusal::Guest)
        );
    }

    #[tokio::test]
    async fn members_are_remembered_for_a_while_and_failures_never() {
        let (gate, _, directory, clock) = make_gate(
            FixedRules::allowing("T1", &[]),
            FixedDirectory::new(&[("U1", member("T1"))]),
        );
        let dm = |user: &str| question(QuestionKind::Direct, "D1", user);
        gate.check(&dm("U1")).await;
        gate.check(&dm("U1")).await;
        assert_eq!(*directory.lookups.lock().unwrap(), 1);
        *clock.0.lock().unwrap() += chrono::Duration::minutes(MEMBER_TTL_MINUTES);
        gate.check(&dm("U1")).await;
        assert_eq!(*directory.lookups.lock().unwrap(), 2, "looked up again");
        gate.check(&dm("UNKNOWN")).await;
        gate.check(&dm("UNKNOWN")).await;
        assert_eq!(*directory.lookups.lock().unwrap(), 4);
    }

    #[tokio::test]
    async fn unreadable_rules_refuse_everything() {
        let (gate, _, _, _) = make_gate(
            FixedRules {
                rules: Err("settings unreadable".into()),
                now_shared: Mutex::new(Vec::new()),
            },
            FixedDirectory::new(&[("U1", member("T1"))]),
        );
        assert_eq!(
            gate.check(&question(QuestionKind::Direct, "D1", "U1"))
                .await,
            Access::Refused(Refusal::RulesUnknown)
        );
        let (gate, _, _, _) = make_gate(
            FixedRules::allowing("", &[("C1", false)]),
            FixedDirectory::new(&[]),
        );
        assert_eq!(
            gate.check(&question(QuestionKind::Mention, "C1", "U1"))
                .await,
            Access::Refused(Refusal::RulesUnknown)
        );
    }
}
