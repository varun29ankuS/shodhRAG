//! Messages that reach Shodh late, and messages that are only small talk: what every
//! connector says about them before (or instead of) searching and calling the model.
//!
//! - **Late.** Telegram keeps a bot's updates while Shodh is off (for at most 24 hours,
//!   <https://core.telegram.org/bots/api#getting-updates>) and hands them over when it
//!   polls again. An answer to a message older than [`LATE_AFTER`] starts with one line
//!   saying when it was sent ([`late_line`]); a message older than [`STALE_AFTER`] gets no
//!   model call, only a short note ([`stale_reply`]), once per chat for a whole backlog
//!   ([`StaleNotes`]). Times are said in the computer's local time.
//! - **Small talk.** "Hi", "thanks", "how's it going shodh" need no library search
//!   ([`is_small_talk`]). The test is conservative: a message is small talk only when
//!   every word of it is a greeting, thanks or a name for Shodh; anything else is searched
//!   as before.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, FixedOffset, Local, Offset, TimeZone, Utc};

/// A message older than this when answered was sent while Shodh was off.
pub const LATE_AFTER: Duration = Duration::minutes(10);
/// A message older than this is not answered any more, only noted.
pub const STALE_AFTER: Duration = Duration::hours(24);
/// One note per chat for the stale messages of one backlog: a stale message within this
/// long of the chat's last note gets none.
pub const STALE_NOTE_EVERY: Duration = Duration::minutes(30);

/// How long ago a message was sent, as the answer treats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lateness {
    /// Answered as usual.
    OnTime,
    /// Answered, with a line saying when it was sent.
    Late,
    /// Not answered: a short note instead.
    Stale,
}

/// How late a message sent at `sent` is at `now` (a time ahead of `now`, from a clock
/// that runs ahead, is on time).
pub fn lateness(sent: DateTime<Utc>, now: DateTime<Utc>) -> Lateness {
    let age = now - sent;
    if age > STALE_AFTER {
        Lateness::Stale
    } else if age > LATE_AFTER {
        Lateness::Late
    } else {
        Lateness::OnTime
    }
}

/// The offset of the computer's time zone at `at` (daylight saving included).
pub fn local_offset(at: DateTime<Utc>) -> FixedOffset {
    Local.offset_from_utc_datetime(&at.naive_utc()).fix()
}

/// When `sent` was, as a person reads it at `now`: "14:20" the same day, "Tue 14:20"
/// within the week, "22 Oct 14:20" before that; in the zone of `offset_of`.
fn when(
    sent: DateTime<Utc>,
    now: DateTime<Utc>,
    offset_of: fn(DateTime<Utc>) -> FixedOffset,
) -> String {
    let sent_local = sent.with_timezone(&offset_of(sent));
    let now_local = now.with_timezone(&offset_of(now));
    let days = (now_local.date_naive() - sent_local.date_naive()).num_days();
    if days == 0 {
        sent_local.format("%H:%M").to_string()
    } else if (1..7).contains(&days) {
        sent_local.format("%a %H:%M").to_string()
    } else {
        sent_local.format("%-d %b %H:%M").to_string()
    }
}

/// The first line of the answer to a late message: "(You sent this at 14:20, while Shodh
/// was offline.)" ("on Tue at 14:20" when it was another day).
pub fn late_line(
    sent: DateTime<Utc>,
    now: DateTime<Utc>,
    offset_of: fn(DateTime<Utc>) -> FixedOffset,
) -> String {
    let at = when(sent, now, offset_of);
    match at.split_once(' ') {
        Some((day, time)) => {
            format!("(You sent this on {day} at {time}, while Shodh was offline.)")
        }
        None => format!("(You sent this at {at}, while Shodh was offline.)"),
    }
}

/// The note sent instead of an answer to a stale message.
pub fn stale_reply(
    sent: DateTime<Utc>,
    now: DateTime<Utc>,
    offset_of: fn(DateTime<Utc>) -> FixedOffset,
) -> String {
    format!(
        "I was offline when you sent this ({}). Ask again if you still need it.",
        when(sent, now, offset_of)
    )
}

/// `answer` as sent for a message sent at `sent`: with [`late_line`] first when it is
/// [`Lateness::Late`] at `now`.
pub fn with_late_line(
    answer: &str,
    sent: DateTime<Utc>,
    now: DateTime<Utc>,
    offset_of: fn(DateTime<Utc>) -> FixedOffset,
) -> String {
    if lateness(sent, now) == Lateness::Late {
        format!("{}\n\n{answer}", late_line(sent, now, offset_of))
    } else {
        answer.to_string()
    }
}

/// The time of a Slack message from its `ts` (`"1730000000.123456"`).
pub fn slack_ts_time(ts: &str) -> Option<DateTime<Utc>> {
    let (seconds, micros) = ts.split_once('.').unwrap_or((ts, "0"));
    let seconds: i64 = seconds.parse().ok()?;
    let micros: u32 = format!("{micros:0<6}").get(..6)?.parse().ok()?;
    DateTime::from_timestamp(seconds, micros.checked_mul(1_000)?)
}

/// Which chats were sent a stale note, and when: one note per chat for a backlog.
#[derive(Debug, Default)]
pub struct StaleNotes(Mutex<HashMap<String, DateTime<Utc>>>);

impl StaleNotes {
    /// Whether `chat` gets a note at `now` (none in the last [`STALE_NOTE_EVERY`]), and
    /// marks it noted. Atomic: the messages of a backlog are answered side by side.
    pub fn take(&self, chat: &str, now: DateTime<Utc>) -> bool {
        let mut noted = self.0.lock().unwrap_or_else(|e| e.into_inner());
        noted.retain(|_, at| now - *at < STALE_NOTE_EVERY);
        if noted.contains_key(chat) {
            return false;
        }
        noted.insert(chat.to_string(), now);
        true
    }
}

/// Greetings, thanks and farewells, as words (after lower-casing and dropping
/// punctuation); a phrase of several words matches only as a whole.
const SMALL_TALK: [&str; 46] = [
    "hi",
    "hello",
    "hey",
    "hiya",
    "howdy",
    "yo",
    "greetings",
    "namaste",
    "good morning",
    "good afternoon",
    "good evening",
    "good night",
    "morning",
    "evening",
    "how are you",
    "how are you doing",
    "how are things",
    "how is it going",
    "hows it going",
    "how's it going",
    "how you doing",
    "how do you do",
    "whats up",
    "what's up",
    "sup",
    "wassup",
    "all good",
    "thanks",
    "thank you",
    "thanks a lot",
    "thank you so much",
    "thx",
    "ty",
    "cheers",
    "ok",
    "okay",
    "cool",
    "great",
    "nice",
    "awesome",
    "perfect",
    "got it",
    "bye",
    "goodbye",
    "see you",
    "good job",
];

/// Words that only address Shodh or soften a greeting.
const ADDRESS: [&str; 9] = [
    "shodh", "@shodh", "there", "bot", "buddy", "friend", "again", "and", "so",
];

/// The longest message read as small talk, in words.
const SMALL_TALK_MAX_WORDS: usize = 10;

/// Whether `text` is only small talk: every word of it a greeting, thanks or farewell
/// ([`SMALL_TALK`]) or a word that addresses Shodh, with at least one of the first. A
/// question mark is allowed ("how are you?"); a word outside both lists, a number, or a
/// longer message is not small talk.
pub fn is_small_talk(text: &str) -> bool {
    let cleaned: String = text
        .to_lowercase()
        .replace(['\u{2019}', '\u{2018}'], "'")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' || c == '@' || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    if words.is_empty() || words.len() > SMALL_TALK_MAX_WORDS {
        return false;
    }
    let phrases: Vec<Vec<&str>> = SMALL_TALK.iter().map(|p| p.split(' ').collect()).collect();
    let mut i = 0;
    let mut greeted = false;
    while i < words.len() {
        // The longest phrase that starts here.
        let matched = phrases
            .iter()
            .filter(|p| words[i..].starts_with(p))
            .map(Vec::len)
            .max();
        if let Some(n) = matched {
            greeted = true;
            i += n;
        } else if ADDRESS.contains(&words[i]) || words[i].starts_with('@') {
            i += 1;
        } else {
            return false;
        }
    }
    greeted
}

/// What a person other than the owner is told for small talk: no search, no model call.
pub const SMALL_TALK_REPLY: &str =
    "Hi! Ask me a question about the documents I answer from, and I'll answer with sources.";

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 27, h, m, 0).unwrap()
    }

    fn utc(_: DateTime<Utc>) -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    /// India: +05:30, no daylight saving.
    fn ist(_: DateTime<Utc>) -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 1800).unwrap()
    }

    #[test]
    fn fresh_late_and_stale_messages() {
        let now = at(12, 0);
        assert_eq!(lateness(at(11, 55), now), Lateness::OnTime);
        assert_eq!(
            lateness(at(11, 50), now),
            Lateness::OnTime,
            "10 minutes exactly"
        );
        assert_eq!(lateness(at(11, 49), now), Lateness::Late);
        assert_eq!(lateness(now - Duration::hours(24), now), Lateness::Late);
        assert_eq!(
            lateness(now - Duration::hours(24) - Duration::minutes(1), now),
            Lateness::Stale
        );
        // A sender's clock ahead of ours: on time.
        assert_eq!(lateness(at(12, 5), now), Lateness::OnTime);
    }

    #[test]
    fn the_late_line_says_when_in_local_time() {
        let now = at(12, 0);
        assert_eq!(
            late_line(at(8, 50), now, utc),
            "(You sent this at 08:50, while Shodh was offline.)"
        );
        // 08:50 UTC is 14:20 in India.
        assert_eq!(
            late_line(at(8, 50), now, ist),
            "(You sent this at 14:20, while Shodh was offline.)"
        );
        // The day before, in local time: 20:00 UTC on the 26th is 01:30 on the 27th in
        // India (the same day there), and Mon 20:00 in UTC.
        let yesterday = Utc.with_ymd_and_hms(2026, 10, 26, 20, 0, 0).unwrap();
        assert_eq!(
            late_line(yesterday, now, ist),
            "(You sent this at 01:30, while Shodh was offline.)"
        );
        assert_eq!(
            late_line(yesterday, now, utc),
            "(You sent this on Mon at 20:00, while Shodh was offline.)"
        );
        assert_eq!(
            with_late_line("Sixty days [1].", at(8, 50), now, ist),
            "(You sent this at 14:20, while Shodh was offline.)\n\nSixty days [1]."
        );
        assert_eq!(
            with_late_line("Sixty days [1].", at(11, 58), now, ist),
            "Sixty days [1]."
        );
    }

    #[test]
    fn the_stale_note_names_the_day() {
        let now = at(12, 0);
        // The 27th is a Tuesday: a week before, the weekday alone would be ambiguous.
        let week_ago = Utc.with_ymd_and_hms(2026, 10, 20, 8, 50, 0).unwrap();
        assert_eq!(
            stale_reply(week_ago, now, ist),
            "I was offline when you sent this (20 Oct 14:20). Ask again if you still need it."
        );
        let saturday = Utc.with_ymd_and_hms(2026, 10, 24, 8, 50, 0).unwrap();
        assert_eq!(
            stale_reply(saturday, now, ist),
            "I was offline when you sent this (Sat 14:20). Ask again if you still need it."
        );
    }

    #[test]
    fn a_backlog_of_stale_messages_gets_one_note_per_chat() {
        let notes = StaleNotes::default();
        let now = at(12, 0);
        assert!(notes.take("chat-1", now));
        assert!(!notes.take("chat-1", now));
        assert!(!notes.take("chat-1", now + Duration::minutes(5)));
        assert!(notes.take("chat-2", now), "another chat gets its own");
        assert!(
            notes.take("chat-1", now + STALE_NOTE_EVERY),
            "a later backlog gets a note again"
        );
    }

    #[test]
    fn slack_timestamps_are_read() {
        assert_eq!(
            slack_ts_time("1730000000.123456"),
            DateTime::from_timestamp(1_730_000_000, 123_456_000)
        );
        assert_eq!(
            slack_ts_time("1730000000"),
            DateTime::from_timestamp(1_730_000_000, 0)
        );
        assert_eq!(slack_ts_time("x.1"), None);
    }

    #[test]
    fn greetings_are_small_talk_and_questions_are_not() {
        for text in [
            "How's it going shodh",
            "How’s it going Shodh?",
            "hi",
            "Hello there!",
            "hey shodh, how are you?",
            "Good morning",
            "thanks!",
            "Thank you so much",
            "ok cool",
            "@shodh hi",
            "bye",
        ] {
            assert!(is_small_talk(text), "{text}");
        }
        for text in [
            "",
            "shodh",
            "hi, what is the notice period?",
            "How is the lease going?",
            "thanks, and the deposit?",
            "What's up with the Q3 numbers",
            "hey can you summarise the paper",
            "good morning, any news on the merger",
            "ok 42",
            "remember that I like tea",
            "how are you different from ChatGPT",
        ] {
            assert!(!is_small_talk(text), "{text}");
        }
    }
}
