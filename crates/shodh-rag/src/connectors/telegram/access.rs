//! Who is answered, and how. Decided by the chat a message is in, never by what it says
//! or by who sends it in a group:
//!
//! - **The owner's private chat** (the linked account): full Shodh, the whole library and
//!   the owner's memories ([`Route::Owner`]).
//! - **Someone allowed** (linked with an invite code), in their private chat: the one
//!   workspace chosen on the card, never memories ([`Route::Guest`]).
//! - **An allowed group**: the same as a guest, and only for messages meant for the bot
//!   ([`super::updates::addressed`]). The owner asking in a group gets the group's answer,
//!   not their own: what is said in a group is seen by the group.
//! - **A group not allowed yet**: the card lists it as waiting; a message meant for the
//!   bot gets a short note.
//! - **Anyone else** in a private chat gets a short refusal, unless their message is
//!   `/start <code>` with a code from the card ([`Route::Link`]).
//! - Channels, bots, and messages without a sender are ignored.

use super::updates::{addressed, command, BotSelf, Message};

/// The card's choices, read at every message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    pub bot: Option<BotSelf>,
    /// The linked account (its user id is its private chat's id).
    pub owner: Option<i64>,
    /// People allowed to ask in their private chat.
    pub people: Vec<i64>,
    /// Groups allowed to ask.
    pub groups: Vec<i64>,
}

/// What to do with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/start <code>` in a private chat: link the sender with that code.
    Link { code: String },
    /// The owner's private chat.
    Owner,
    /// An allowed person's private chat.
    Guest,
    /// A question meant for the bot in an allowed group.
    Group { question: String },
    /// A question meant for the bot in a group the owner has not allowed.
    GroupNotAllowed,
    /// A private chat with someone not linked.
    Refuse,
    /// Nothing to answer (a group message not meant for the bot, a channel post, a bot).
    Ignore(&'static str),
}

/// What to do with `message` under `rules`.
pub fn route(message: &Message, rules: &Rules) -> Route {
    let Some(bot) = &rules.bot else {
        return Route::Ignore("the bot is not set up");
    };
    let Some(from) = &message.from else {
        return Route::Ignore("no sender");
    };
    if from.is_bot || from.id == bot.id {
        return Route::Ignore("a bot");
    }
    if message.chat.kind.is_group() {
        let Some(question) = addressed(message, bot) else {
            return Route::Ignore("not meant for the bot");
        };
        return if rules.groups.contains(&message.chat.id) {
            Route::Group { question }
        } else {
            Route::GroupNotAllowed
        };
    }
    if message.chat.kind != super::updates::ChatKind::Private {
        return Route::Ignore("a channel");
    }
    // A private chat's id is the person's id; anything else is not a private chat.
    if message.chat.id != from.id {
        return Route::Ignore("not a private chat");
    }
    if let Some(cmd) = command(&message.text, bot) {
        if cmd.name == "start" && !cmd.rest.is_empty() {
            return Route::Link { code: cmd.rest };
        }
    }
    if rules.owner == Some(from.id) {
        Route::Owner
    } else if rules.people.contains(&from.id) {
        Route::Guest
    } else {
        Route::Refuse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::telegram::updates::tests::{bot, message};
    use crate::connectors::telegram::updates::ReplyTo;

    const OWNER: i64 = 42;
    const GUEST: i64 = 77;
    const GROUP: i64 = -100_200;

    fn rules() -> Rules {
        Rules {
            bot: Some(bot()),
            owner: Some(OWNER),
            people: vec![GUEST],
            groups: vec![GROUP],
        }
    }

    #[test]
    fn the_owners_private_chat_is_full_shodh_and_others_are_guests_or_refused() {
        let rules = rules();
        assert_eq!(route(&message(1, OWNER, OWNER, "hi"), &rules), Route::Owner);
        assert_eq!(route(&message(2, GUEST, GUEST, "hi"), &rules), Route::Guest);
        assert_eq!(route(&message(3, 9, 9, "hi"), &rules), Route::Refuse);
        // Nobody linked yet: everyone is refused.
        let unlinked = Rules {
            owner: None,
            people: Vec::new(),
            ..rules.clone()
        };
        assert_eq!(
            route(&message(4, OWNER, OWNER, "hi"), &unlinked),
            Route::Refuse
        );
    }

    #[test]
    fn a_start_code_links_from_a_private_chat_only() {
        let rules = rules();
        assert_eq!(
            route(&message(1, 9, 9, "/start abcDEF123"), &rules),
            Route::Link {
                code: "abcDEF123".into()
            }
        );
        // In a group, /start is not a link (and not meant for the bot).
        assert_eq!(
            route(&message(2, GROUP, 9, "/start abcDEF123"), &rules),
            Route::Ignore("not meant for the bot")
        );
        // A plain /start from the owner is the owner's chat.
        assert_eq!(
            route(&message(3, OWNER, OWNER, "/start"), &rules),
            Route::Owner
        );
    }

    #[test]
    fn a_group_is_decided_by_the_group_never_by_who_asks() {
        let rules = rules();
        let q = "/ask@shodh_notes_bot what is the rent?";
        // The owner asking in a group gets the group's answer.
        assert_eq!(
            route(&message(1, GROUP, OWNER, q), &rules),
            Route::Group {
                question: "what is the rent?".into()
            }
        );
        assert_eq!(
            route(&message(2, -555, OWNER, q), &rules),
            Route::GroupNotAllowed
        );
        assert_eq!(
            route(&message(3, GROUP, OWNER, "what is the rent?"), &rules),
            Route::Ignore("not meant for the bot"),
            "only on a mention or a reply"
        );
        let mut reply = message(4, GROUP, GUEST, "and the deposit?");
        reply.reply_to = Some(ReplyTo {
            message_id: 1,
            from: Some(bot().id),
        });
        assert!(matches!(route(&reply, &rules), Route::Group { .. }));
    }

    #[test]
    fn bots_and_messages_without_a_sender_are_ignored() {
        let rules = rules();
        let mut m = message(1, OWNER, OWNER, "hi");
        m.from.as_mut().unwrap().is_bot = true;
        assert_eq!(route(&m, &rules), Route::Ignore("a bot"));
        m.from = None;
        assert_eq!(route(&m, &rules), Route::Ignore("no sender"));
        let mut odd = message(2, OWNER, GUEST, "hi");
        odd.chat.id = OWNER;
        assert_eq!(route(&odd, &rules), Route::Ignore("not a private chat"));
        assert_eq!(
            route(&message(3, OWNER, OWNER, "hi"), &Rules::default()),
            Route::Ignore("the bot is not set up")
        );
    }
}
