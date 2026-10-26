//! How indexed Slack content is named in the index.
//!
//! - Each indexed channel is its own index space, `slack:<team>:<channel>`: a Shodh workspace
//!   lists it as a folder source, and forgetting the channel deletes the space.
//! - Each thread (a message and its replies) is one document,
//!   `slack://<team>/<channel>/<thread_ts>`, so one thread is replaced or deleted by its
//!   source.
//! - A memory saved from Slack carries `slack-memory://<team>/<channel>/<ts>` as its
//!   provenance (never a `slack://` document source).

/// The start of every Slack index space id.
pub const SPACE_PREFIX: &str = "slack:";
/// The start of every Slack thread's document source.
pub const SOURCE_PREFIX: &str = "slack://";
/// The start of the provenance source of a memory saved from Slack.
pub const MEMORY_SOURCE_PREFIX: &str = crate::user_memory::SLACK_MEMORY_SOURCE_PREFIX;

/// Whether `part` can stand in an id: Slack's ids and timestamps are ASCII letters,
/// digits and dots.
fn plain(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// The index space of channel `channel` of Slack workspace `team`.
pub fn space_id(team: &str, channel: &str) -> String {
    format!("{SPACE_PREFIX}{team}:{channel}")
}

/// Whether `space` is a Slack channel's index space.
pub fn is_slack_space(space: &str) -> bool {
    space.starts_with(SPACE_PREFIX) && !space.starts_with(SOURCE_PREFIX)
}

/// The Slack workspace and channel of index space `space`.
pub fn parse_space(space: &str) -> Option<(String, String)> {
    let rest = space.strip_prefix(SPACE_PREFIX)?;
    let (team, channel) = rest.split_once(':')?;
    (plain(team) && plain(channel)).then(|| (team.to_string(), channel.to_string()))
}

/// The document source of the thread `thread_ts` of `channel`.
pub fn thread_source(team: &str, channel: &str, thread_ts: &str) -> String {
    format!("{SOURCE_PREFIX}{team}/{channel}/{thread_ts}")
}

/// What every thread source of `channel` starts with.
pub fn channel_source_prefix(team: &str, channel: &str) -> String {
    format!("{SOURCE_PREFIX}{team}/{channel}/")
}

/// The Slack workspace, channel and thread of document source `source`.
pub fn parse_thread_source(source: &str) -> Option<(String, String, String)> {
    let rest = source.strip_prefix(SOURCE_PREFIX)?;
    let mut parts = rest.split('/');
    let (Some(team), Some(channel), Some(thread), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    (plain(team) && plain(channel) && plain(thread))
        .then(|| (team.to_string(), channel.to_string(), thread.to_string()))
}

/// The provenance source of a memory saved from message `ts` of `channel`.
pub fn memory_source(team: &str, channel: &str, ts: &str) -> String {
    format!("{MEMORY_SOURCE_PREFIX}{team}/{channel}/{ts}")
}

/// Whether `source` is a well-formed [`memory_source`] (the memory guard's own check).
pub fn is_memory_source(source: &str) -> bool {
    crate::user_memory::is_slack_memory_source(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_reject_what_is_not_theirs() {
        let space = space_id("T1", "C1");
        assert_eq!(space, "slack:T1:C1");
        assert!(is_slack_space(&space));
        assert_eq!(parse_space(&space), Some(("T1".into(), "C1".into())));
        assert!(!is_slack_space("space-a"));
        assert!(!is_slack_space("slack://T1/C1/1.2"));
        assert_eq!(parse_space("slack:T1"), None);
        assert_eq!(parse_space("slack:T1:C1:x"), None, "no third part");

        let source = thread_source("T1", "C1", "1700000000.000100");
        assert_eq!(source, "slack://T1/C1/1700000000.000100");
        assert!(source.starts_with(&channel_source_prefix("T1", "C1")));
        assert!(!source.starts_with(&channel_source_prefix("T1", "C10")));
        assert_eq!(
            parse_thread_source(&source),
            Some(("T1".into(), "C1".into(), "1700000000.000100".into()))
        );
        assert_eq!(parse_thread_source("slack://T1/C1"), None);
        assert_eq!(parse_thread_source("slack://T1/C1/1.2/x"), None);
        assert_eq!(parse_thread_source("note://n1"), None);

        let memory = memory_source("T1", "C1", "1.2");
        assert_eq!(memory, "slack-memory://T1/C1/1.2");
        assert!(is_memory_source(&memory));
        assert!(!is_memory_source("slack-memory://T1/C1"));
        assert!(!is_memory_source("slack-memory://T1/C1/1.2/x"));
        assert!(!is_memory_source("slack-memory://T1/C 1/1.2"));
        assert!(!is_memory_source(&source));
    }
}
