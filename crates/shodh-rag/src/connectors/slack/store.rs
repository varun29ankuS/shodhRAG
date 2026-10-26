//! What the Slack connector keeps in `shodh.db` (migration 17): the channels indexed and
//! their sync checkpoints, the indexed threads and which thread each message belongs to,
//! the memories saved from Slack, and the questions and answers of Slack threads (for
//! follow-ups in the same thread). Every write is one transaction in the database's write
//! turn ([`SharedConnection::write`]); the calls block and are made off the async workers.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::answer::ThreadTurn;
use crate::audit::{open_shared_connection, AuditKey, SharedConnection};

/// Questions and answers kept per thread.
pub const TURNS_KEPT_PER_THREAD: usize = 6;
/// Days a thread's questions and answers are kept.
pub const TURNS_KEPT_DAYS: i64 = 30;
/// Channel-scoped memories put in one answer's prompt (the most recent).
pub const CHANNEL_MEMORIES_IN_PROMPT: usize = 10;
/// Channel-scoped memories one channel keeps at most (each up to
/// [`super::remember::MAX_MEMORY_CHARS`]); more are refused until some are deleted.
pub const MAX_CHANNEL_MEMORIES: usize = 100;
/// Channel-scoped memories one person may save in one channel in a day.
pub const MAX_MEMORIES_PER_PERSON_PER_DAY: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum SlackStoreError {
    #[error("Slack data could not be read or saved: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Slack data could not be opened: {0}")]
    Open(String),
    #[error("{0}")]
    Invalid(String),
    /// A limit was reached; the message is for the person who asked.
    #[error("{0}")]
    Limit(String),
}

pub type SlackStoreResult<T> = Result<T, SlackStoreError>;

/// Where an indexed channel's sync stands (besides a rate-limit pause, which is a time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelState {
    /// Backfilling or up to date.
    Active,
    /// Archived in Slack: no more syncing; what was indexed stays until forgotten.
    Archived,
    /// The bot left the channel or was removed from it.
    Removed,
    /// Indexed while not shared with another organization, shared now: paused until the
    /// user confirms it again.
    NowShared,
    /// Stopped on an error the user must fix (a missing scope, a refused token).
    Error,
    /// Being forgotten.
    Forgetting,
}

impl ChannelState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelState::Active => "active",
            ChannelState::Archived => "archived",
            ChannelState::Removed => "removed",
            ChannelState::NowShared => "now_shared",
            ChannelState::Error => "error",
            ChannelState::Forgetting => "forgetting",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "active" => ChannelState::Active,
            "archived" => ChannelState::Archived,
            "removed" => ChannelState::Removed,
            "now_shared" => ChannelState::NowShared,
            "error" => ChannelState::Error,
            "forgetting" => ChannelState::Forgetting,
            _ => return None,
        })
    }
}

/// A channel to index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIndexedChannel {
    pub team_id: String,
    pub channel_id: String,
    pub name: String,
    /// The Shodh workspace its threads go into.
    pub workspace_id: String,
    pub shared: bool,
    pub private: bool,
    /// How far back the first backfill reaches.
    pub window_days: u32,
}

/// An indexed channel and its sync checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedChannel {
    pub team_id: String,
    pub channel_id: String,
    pub name: String,
    pub workspace_id: String,
    pub shared: bool,
    pub private: bool,
    pub window_days: u32,
    pub state: ChannelState,
    /// The fixed `latest` bound of the backfill (set at its first page).
    pub backfill_latest: Option<String>,
    /// The cursor of the backfill's next page (`None` before the first page and when done).
    pub backfill_cursor: Option<String>,
    pub backfill_done: bool,
    /// The oldest message the backfill reached (its progress through the window).
    pub oldest_seen: Option<String>,
    /// The newest message seen: incremental polls ask for what came after it.
    pub newest_ts: Option<String>,
    /// Paused (Slack's rate limit, or a passing error) until then.
    pub paused_until: Option<DateTime<Utc>>,
    /// Why it is paused or stopped, in plain words.
    pub error: Option<String>,
    /// `https://<workspace>.slack.com/archives/<channel>/`, when a permalink had that shape.
    pub permalink_base: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_poll_at: Option<DateTime<Utc>>,
    pub last_slim_at: Option<DateTime<Utc>>,
    /// An incremental poll that takes more than one step: its fixed `latest` bound, the
    /// cursor of its next page, and the newest message it read so far. `newest_ts` moves
    /// only when the poll read everything up to its bound.
    pub poll_latest: Option<String>,
    pub poll_cursor: Option<String>,
    pub poll_newest: Option<String>,
    /// A slim pass that takes more than one step continues below this message (`None`:
    /// the next pass starts from now).
    pub slim_latest: Option<String>,
    pub started_at: DateTime<Utc>,
    /// Threads indexed now.
    pub threads: usize,
}

impl IndexedChannel {
    /// The channel's index space.
    pub fn space_id(&self) -> String {
        super::ids::space_id(&self.team_id, &self.channel_id)
    }

    /// Paused at `now` (until [`IndexedChannel::paused_until`]).
    pub fn paused_at(&self, now: DateTime<Utc>) -> bool {
        self.paused_until.is_some_and(|until| until > now)
    }
}

/// What a memory saved from Slack is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackMemoryKind {
    /// `remember: <text>`, as typed.
    Text,
    /// `remember this thread`: a summary of the thread.
    Thread,
}

impl SlackMemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SlackMemoryKind::Text => "text",
            SlackMemoryKind::Thread => "thread",
        }
    }

    fn parse(text: &str) -> Self {
        if text == "thread" {
            SlackMemoryKind::Thread
        } else {
            SlackMemoryKind::Text
        }
    }
}

/// A memory saved from Slack, with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackMemory {
    pub id: String,
    pub team_id: String,
    pub channel_id: String,
    /// `#name`, or "a direct message".
    pub channel_name: String,
    pub author_id: String,
    /// The author's Slack display name (never an e-mail address).
    pub author_name: String,
    pub text: String,
    pub kind: SlackMemoryKind,
    /// The message that asked Shodh to remember.
    pub message_ts: String,
    pub permalink: Option<String>,
    pub saved_at: DateTime<Utc>,
    /// Saved by the owner: it is in the user's memories (`memory_id`), not channel-scoped.
    pub owner: bool,
    /// Its id in the user's memories (the owner's, or one the user promoted).
    pub memory_id: Option<String>,
}

/// What forgetting a channel (or a whole Slack workspace) removed that lives elsewhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forgotten {
    /// Ids in the user's memories of memories saved from the forgotten channels.
    pub memory_ids: Vec<String>,
    pub channels: usize,
    pub threads: usize,
    pub memories: usize,
}

/// A thread Shodh has read ([`SlackStore::threads`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRecord {
    /// The thread's latest reply and reply count as Slack listed them when it was read.
    pub latest_reply: Option<String>,
    pub reply_count: usize,
    /// It has a document in the index (`false`: read with nothing to index).
    pub indexed: bool,
}

impl ThreadRecord {
    /// Whether Slack lists the thread with other replies than when it was read (a reply
    /// added, deleted or edited away).
    pub fn changed(&self, latest_reply: Option<&str>, reply_count: usize) -> bool {
        self.latest_reply.as_deref() != latest_reply || self.reply_count != reply_count
    }
}

fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(text: Option<String>) -> Option<DateTime<Utc>> {
    text.and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.with_timezone(&Utc))
}

/// A Slack timestamp as a number, for comparing ("1700000000.000100").
pub fn ts_value(ts: &str) -> f64 {
    ts.trim().parse::<f64>().unwrap_or(0.0)
}

/// The later of two Slack timestamps.
fn later(a: Option<String>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) if ts_value(b) > ts_value(&a) => Some(b.to_string()),
        (Some(a), _) => Some(a),
        (None, b) => b.map(str::to_string),
    }
}

/// The earlier of two Slack timestamps.
fn earlier(a: Option<String>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) if ts_value(b) < ts_value(&a) => Some(b.to_string()),
        (Some(a), _) => Some(a),
        (None, b) => b.map(str::to_string),
    }
}

const CHANNEL_COLUMNS: &str = "c.team_id, c.channel_id, c.name, c.workspace_id, c.shared, \
     c.private, c.window_days, c.state, c.backfill_latest, c.backfill_cursor, c.backfill_done, \
     c.oldest_seen, c.newest_ts, c.paused_until, c.error, c.permalink_base, c.last_sync_at, \
     c.last_poll_at, c.last_slim_at, c.poll_latest, c.poll_cursor, c.poll_newest,      c.slim_latest, c.started_at, \
     (SELECT count(*) FROM slack_index_thread t \
       WHERE t.team_id = c.team_id AND t.channel_id = c.channel_id AND t.indexed_at IS NOT NULL)";

fn read_channel(row: &Row<'_>) -> rusqlite::Result<IndexedChannel> {
    let state: String = row.get(7)?;
    let window: i64 = row.get(6)?;
    let threads: i64 = row.get(24)?;
    Ok(IndexedChannel {
        team_id: row.get(0)?,
        channel_id: row.get(1)?,
        name: row.get(2)?,
        workspace_id: row.get(3)?,
        shared: row.get::<_, i64>(4)? != 0,
        private: row.get::<_, i64>(5)? != 0,
        window_days: u32::try_from(window).unwrap_or(1),
        state: ChannelState::parse(&state).unwrap_or(ChannelState::Error),
        backfill_latest: row.get(8)?,
        backfill_cursor: row.get(9)?,
        backfill_done: row.get::<_, i64>(10)? != 0,
        oldest_seen: row.get(11)?,
        newest_ts: row.get(12)?,
        paused_until: parse_time(row.get(13)?),
        error: row.get(14)?,
        permalink_base: row.get(15)?,
        last_sync_at: parse_time(row.get(16)?),
        last_poll_at: parse_time(row.get(17)?),
        last_slim_at: parse_time(row.get(18)?),
        poll_latest: row.get(19)?,
        poll_cursor: row.get(20)?,
        poll_newest: row.get(21)?,
        slim_latest: row.get(22)?,
        started_at: parse_time(row.get(23)?).unwrap_or_else(Utc::now),
        threads: usize::try_from(threads).unwrap_or(0),
    })
}

const MEMORY_COLUMNS: &str = "id, team_id, channel_id, channel_name, author_id, author_name, \
     text, kind, message_ts, permalink, saved_at, owner, memory_id";

fn read_memory(row: &Row<'_>) -> rusqlite::Result<SlackMemory> {
    let kind: String = row.get(7)?;
    Ok(SlackMemory {
        id: row.get(0)?,
        team_id: row.get(1)?,
        channel_id: row.get(2)?,
        channel_name: row.get(3)?,
        author_id: row.get(4)?,
        author_name: row.get(5)?,
        text: row.get(6)?,
        kind: SlackMemoryKind::parse(&kind),
        message_ts: row.get(8)?,
        permalink: row.get(9)?,
        saved_at: parse_time(row.get(10)?).unwrap_or_else(Utc::now),
        owner: row.get::<_, i64>(11)? != 0,
        memory_id: row.get(12)?,
    })
}

fn channel_in(
    conn: &Connection,
    team: &str,
    channel: &str,
) -> rusqlite::Result<Option<IndexedChannel>> {
    conn.query_row(
        &format!(
            "SELECT {CHANNEL_COLUMNS} FROM slack_index_channel c
             WHERE c.team_id = ?1 AND c.channel_id = ?2"
        ),
        params![team, channel],
        read_channel,
    )
    .optional()
}

/// The ids in the user's memories of the Slack memories `filter` selects (a `WHERE`
/// clause over `slack_memory` with `?1` the team and optionally `?2` the channel).
fn linked_memory_ids(
    conn: &Connection,
    team: &str,
    channel: Option<&str>,
) -> rusqlite::Result<Vec<String>> {
    let mut ids = Vec::new();
    match channel {
        Some(channel) => {
            let mut stmt = conn.prepare(
                "SELECT memory_id FROM slack_memory
                 WHERE team_id = ?1 AND channel_id = ?2 AND memory_id IS NOT NULL",
            )?;
            for id in stmt.query_map(params![team, channel], |r| r.get::<_, String>(0))? {
                ids.push(id?);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT memory_id FROM slack_memory WHERE team_id = ?1 AND memory_id IS NOT NULL",
            )?;
            for id in stmt.query_map(params![team], |r| r.get::<_, String>(0))? {
                ids.push(id?);
            }
        }
    }
    Ok(ids)
}

/// The Slack connector's tables in `shodh.db`.
pub struct SlackStore {
    conn: SharedConnection,
}

impl std::fmt::Debug for SlackStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackStore").finish_non_exhaustive()
    }
}

impl SlackStore {
    /// Opens `shodh.db` at `path` with the audit database key (if it is encrypted),
    /// applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> SlackStoreResult<Self> {
        Ok(Self {
            conn: open_shared_connection(path, key)
                .map_err(|e| SlackStoreError::Open(e.to_string()))?,
        })
    }

    // ── Channels ────────────────────────────────────────────────────

    /// Starts indexing a channel, or changes its workspace or sharing. A channel indexed
    /// before keeps its checkpoint (it resumes); its pause and error are cleared. A channel
    /// being forgotten is refused: choosing it again would keep it as it is forgotten.
    pub fn add_channel(
        &self,
        channel: &NewIndexedChannel,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<IndexedChannel> {
        if channel.window_days == 0 {
            return Err(SlackStoreError::Invalid(
                "the history window is at least one day".into(),
            ));
        }
        let started = stamp(now);
        self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO slack_index_channel
                    (team_id, channel_id, name, workspace_id, shared, private, window_days,
                     state, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8)
                 ON CONFLICT(team_id, channel_id) DO UPDATE SET
                    name = excluded.name,
                    workspace_id = excluded.workspace_id,
                    shared = excluded.shared,
                    private = excluded.private,
                    state = 'active',
                    paused_until = NULL,
                    error = NULL
                 WHERE slack_index_channel.state <> 'forgetting'",
                params![
                    channel.team_id,
                    channel.channel_id,
                    channel.name,
                    channel.workspace_id,
                    i64::from(channel.shared),
                    i64::from(channel.private),
                    i64::from(channel.window_days),
                    started
                ],
            )?;
            let saved = channel_in(tx, &channel.team_id, &channel.channel_id)?
                .ok_or_else(|| SlackStoreError::Invalid("the channel was not saved".into()))?;
            if saved.state == ChannelState::Forgetting {
                return Err(SlackStoreError::Invalid(format!(
                    "#{} is being forgotten. Choose it again once that is done.",
                    saved.name
                )));
            }
            Ok(saved)
        })
    }

    /// The channels of Slack workspace `team`, by name.
    pub fn channels(&self, team: &str) -> SlackStoreResult<Vec<IndexedChannel>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CHANNEL_COLUMNS} FROM slack_index_channel c
             WHERE c.team_id = ?1 ORDER BY c.name COLLATE NOCASE, c.channel_id"
        ))?;
        let rows = stmt
            .query_map(params![team], read_channel)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn channel(&self, team: &str, channel: &str) -> SlackStoreResult<Option<IndexedChannel>> {
        let conn = self.conn.read()?;
        Ok(channel_in(&conn, team, channel)?)
    }

    /// Sets the state of a channel, with the reason shown for it (`None` clears it). A
    /// channel being forgotten stays so: an event or a step that ends after the forget
    /// began never turns it back into one whose data is kept.
    pub fn set_state(
        &self,
        team: &str,
        channel: &str,
        state: ChannelState,
        reason: Option<&str>,
    ) -> SlackStoreResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET state = ?3, error = ?4, paused_until = NULL
                 WHERE team_id = ?1 AND channel_id = ?2
                   AND (state <> 'forgetting' OR ?3 = 'forgetting')",
                params![team, channel, state.as_str(), reason],
            )
        })?;
        Ok(())
    }

    /// Stops every active channel of `team` with `reason` (a refused token, a missing
    /// scope): they wait for the user.
    pub fn stop_all(&self, team: &str, reason: &str) -> SlackStoreResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET state = 'error', error = ?2, paused_until = NULL
                 WHERE team_id = ?1 AND state = 'active'",
                params![team, reason],
            )
        })?;
        Ok(())
    }

    /// Lets channels of `team` stopped on an error try again (new tokens, the connector
    /// switched on again).
    pub fn retry_stopped(&self, team: &str) -> SlackStoreResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET state = 'active', error = NULL, paused_until = NULL
                 WHERE team_id = ?1 AND state = 'error'",
                params![team],
            )
        })?;
        Ok(())
    }

    /// Pauses a channel until `until` (Slack's `Retry-After`, or the backoff after a
    /// passing error), with the reason.
    pub fn pause(
        &self,
        team: &str,
        channel: &str,
        until: DateTime<Utc>,
        reason: &str,
    ) -> SlackStoreResult<()> {
        let until = stamp(until);
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET paused_until = ?3, error = ?4
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, until, reason],
            )
        })?;
        Ok(())
    }

    /// Pauses every active channel of `team` until `until` (Slack's rate limit is per
    /// method and workspace).
    pub fn pause_all(
        &self,
        team: &str,
        until: DateTime<Utc>,
        reason: &str,
    ) -> SlackStoreResult<()> {
        let until = stamp(until);
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET paused_until = ?2, error = ?3
                 WHERE team_id = ?1 AND state = 'active'",
                params![team, until, reason],
            )
        })?;
        Ok(())
    }

    /// Records a backfill page: the `latest` bound it used, the cursor of the next page
    /// (`None`: the backfill is done), and the oldest and newest messages it held. A
    /// successful page clears a pause.
    #[allow(clippy::too_many_arguments)]
    pub fn save_backfill_page(
        &self,
        team: &str,
        channel: &str,
        latest: &str,
        next_cursor: Option<&str>,
        oldest: Option<&str>,
        newest: Option<&str>,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let now = stamp(now);
        self.conn.write(|tx| {
            let Some(current) = channel_in(tx, team, channel)? else {
                return Ok(());
            };
            let oldest = earlier(current.oldest_seen, oldest);
            let newest = later(current.newest_ts, newest);
            tx.execute(
                "UPDATE slack_index_channel SET backfill_latest = ?3, backfill_cursor = ?4,
                    backfill_done = ?5, oldest_seen = ?6, newest_ts = ?7, last_sync_at = ?8,
                    paused_until = NULL, error = NULL
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![
                    team,
                    channel,
                    latest,
                    next_cursor,
                    i64::from(next_cursor.is_none()),
                    oldest,
                    newest,
                    now
                ],
            )?;
            Ok::<_, SlackStoreError>(())
        })
    }

    /// Records an incremental poll that read everything up to its bound, the newest
    /// message being `newest` (with the newest its earlier steps read): later polls ask
    /// for what came after it. Ends a poll in progress.
    pub fn save_poll(
        &self,
        team: &str,
        channel: &str,
        newest: Option<&str>,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let now = stamp(now);
        self.conn.write(|tx| {
            let Some(current) = channel_in(tx, team, channel)? else {
                return Ok(());
            };
            let newest = later(
                later(current.newest_ts, current.poll_newest.as_deref()),
                newest,
            );
            tx.execute(
                "UPDATE slack_index_channel SET newest_ts = ?3, last_poll_at = ?4,
                    last_sync_at = ?4, poll_latest = NULL, poll_cursor = NULL,
                    poll_newest = NULL, paused_until = NULL, error = NULL
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, newest, now],
            )?;
            Ok::<_, SlackStoreError>(())
        })
    }

    /// Records one step of an incremental poll that has more to read: its fixed `latest`
    /// bound, the cursor of its next page and the newest message read. `newest_ts` stays
    /// (the pages left lie between it and the bound), and the poll stays due.
    pub fn save_poll_part(
        &self,
        team: &str,
        channel: &str,
        latest: &str,
        next_cursor: &str,
        newest: Option<&str>,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let now = stamp(now);
        self.conn.write(|tx| {
            let Some(current) = channel_in(tx, team, channel)? else {
                return Ok(());
            };
            let newest = later(current.poll_newest, newest);
            tx.execute(
                "UPDATE slack_index_channel SET poll_latest = ?3, poll_cursor = ?4,
                    poll_newest = ?5, last_sync_at = ?6, paused_until = NULL, error = NULL
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, latest, next_cursor, newest, now],
            )?;
            Ok::<_, SlackStoreError>(())
        })
    }

    /// Records a finished slim pass (it listed the whole window).
    pub fn save_slim(&self, team: &str, channel: &str, now: DateTime<Utc>) -> SlackStoreResult<()> {
        let now = stamp(now);
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET last_slim_at = ?3, last_sync_at = ?3,
                    slim_latest = NULL, paused_until = NULL, error = NULL
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, now],
            )
        })?;
        Ok(())
    }

    /// Records one step of a slim pass that has more to list: the next step lists what
    /// is older than message `latest` (what is newer was listed), and the pass stays due.
    pub fn save_slim_part(
        &self,
        team: &str,
        channel: &str,
        latest: &str,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let now = stamp(now);
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET slim_latest = ?3, last_sync_at = ?4,
                    paused_until = NULL, error = NULL
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, latest, now],
            )
        })?;
        Ok(())
    }

    /// Drops the saved cursors of a channel (Slack refused one as stale): the backfill and
    /// a poll in progress read again from their bounds, which are kept, or with `bounds`
    /// (Slack refused a bound) from now. What was indexed is kept; reading a page again
    /// skips its threads that did not change.
    pub fn reset_cursors(&self, team: &str, channel: &str, bounds: bool) -> SlackStoreResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET backfill_cursor = NULL, poll_cursor = NULL,
                    backfill_latest = CASE WHEN ?3 THEN NULL ELSE backfill_latest END,
                    poll_latest = CASE WHEN ?3 THEN NULL ELSE poll_latest END,
                    slim_latest = CASE WHEN ?3 THEN NULL ELSE slim_latest END
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, bounds],
            )
        })?;
        Ok(())
    }

    /// Records a thread fetched again after an event.
    pub fn touch_sync(
        &self,
        team: &str,
        channel: &str,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let now = stamp(now);
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET last_sync_at = ?3, paused_until = NULL, error = NULL
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, now],
            )
        })?;
        Ok(())
    }

    pub fn set_permalink_base(
        &self,
        team: &str,
        channel: &str,
        base: &str,
    ) -> SlackStoreResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_index_channel SET permalink_base = ?3
                 WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel, base],
            )
        })?;
        Ok(())
    }

    // ── Threads ─────────────────────────────────────────────────────

    /// Records thread `thread_ts` as indexed now, with its messages.
    #[allow(clippy::too_many_arguments)]
    pub fn record_thread(
        &self,
        team: &str,
        channel: &str,
        thread_ts: &str,
        latest_reply: Option<&str>,
        reply_count: usize,
        messages: &[String],
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let now = stamp(now);
        let replies = i64::try_from(reply_count).unwrap_or(i64::MAX);
        self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO slack_index_thread
                    (team_id, channel_id, thread_ts, latest_reply, reply_count, dirty, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
                 ON CONFLICT(team_id, channel_id, thread_ts) DO UPDATE SET
                    latest_reply = excluded.latest_reply,
                    reply_count = excluded.reply_count,
                    dirty = 0,
                    dirty_at = NULL,
                    indexed_at = excluded.indexed_at",
                params![team, channel, thread_ts, latest_reply, replies, now],
            )?;
            tx.execute(
                "DELETE FROM slack_index_message
                 WHERE team_id = ?1 AND channel_id = ?2 AND thread_ts = ?3",
                params![team, channel, thread_ts],
            )?;
            for ts in messages {
                tx.execute(
                    "INSERT OR REPLACE INTO slack_index_message (team_id, channel_id, ts, thread_ts)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![team, channel, ts, thread_ts],
                )?;
            }
            Ok::<_, SlackStoreError>(())
        })
    }

    /// Records thread `thread_ts` as read with nothing to index (only bots, or nothing left),
    /// with the replies Slack listed for it: the slim pass fetches it again only when
    /// they change.
    pub fn record_skipped_thread(
        &self,
        team: &str,
        channel: &str,
        thread_ts: &str,
        latest_reply: Option<&str>,
        reply_count: usize,
    ) -> SlackStoreResult<()> {
        let replies = i64::try_from(reply_count).unwrap_or(i64::MAX);
        self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO slack_index_thread
                    (team_id, channel_id, thread_ts, latest_reply, reply_count, dirty, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL)
                 ON CONFLICT(team_id, channel_id, thread_ts) DO UPDATE SET
                    latest_reply = excluded.latest_reply,
                    reply_count = excluded.reply_count,
                    dirty = 0,
                    dirty_at = NULL,
                    indexed_at = NULL",
                params![team, channel, thread_ts, latest_reply, replies],
            )?;
            tx.execute(
                "DELETE FROM slack_index_message
                 WHERE team_id = ?1 AND channel_id = ?2 AND thread_ts = ?3",
                params![team, channel, thread_ts],
            )?;
            Ok::<_, SlackStoreError>(())
        })
    }

    /// Marks thread `thread_ts` to be fetched again (a new message, an edit, a deletion).
    /// Only in a channel being indexed (active): checked in the same write, so a mark
    /// never outlives a forget that ran between an event and its write.
    pub fn mark_dirty(
        &self,
        team: &str,
        channel: &str,
        thread_ts: &str,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let now = stamp(now);
        self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO slack_index_thread (team_id, channel_id, thread_ts, dirty, dirty_at)
                 SELECT ?1, ?2, ?3, 1, ?4
                  WHERE EXISTS (SELECT 1 FROM slack_index_channel
                                 WHERE team_id = ?1 AND channel_id = ?2 AND state = 'active')
                 ON CONFLICT(team_id, channel_id, thread_ts) DO UPDATE SET
                    dirty = 1,
                    dirty_at = COALESCE(slack_index_thread.dirty_at, excluded.dirty_at)",
                params![team, channel, thread_ts, now],
            )
        })?;
        Ok(())
    }

    /// Threads to fetch again in active channels of `team`, oldest mark first.
    pub fn dirty_threads(
        &self,
        team: &str,
        limit: usize,
    ) -> SlackStoreResult<Vec<(String, String)>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(
            "SELECT t.channel_id, t.thread_ts FROM slack_index_thread t
             JOIN slack_index_channel c
               ON c.team_id = t.team_id AND c.channel_id = t.channel_id
             WHERE t.team_id = ?1 AND t.dirty = 1 AND c.state = 'active'
             ORDER BY t.dirty_at, t.thread_ts LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![team, limit], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The thread message `ts` belongs to, when it was indexed (a thread's first message
    /// is its own thread).
    pub fn thread_of_message(
        &self,
        team: &str,
        channel: &str,
        ts: &str,
    ) -> SlackStoreResult<Option<String>> {
        let conn = self.conn.read()?;
        let found: Option<String> = conn
            .query_row(
                "SELECT thread_ts FROM slack_index_message
                 WHERE team_id = ?1 AND channel_id = ?2 AND ts = ?3",
                params![team, channel, ts],
                |r| r.get(0),
            )
            .optional()?;
        if found.is_some() {
            return Ok(found);
        }
        Ok(conn
            .query_row(
                "SELECT thread_ts FROM slack_index_thread
                 WHERE team_id = ?1 AND channel_id = ?2 AND thread_ts = ?3",
                params![team, channel, ts],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Whether thread `thread_ts` has a document in the index now.
    pub fn is_indexed(&self, team: &str, channel: &str, thread_ts: &str) -> SlackStoreResult<bool> {
        let conn = self.conn.read()?;
        let indexed: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM slack_index_thread
                 WHERE team_id = ?1 AND channel_id = ?2 AND thread_ts = ?3
                   AND indexed_at IS NOT NULL",
                params![team, channel, thread_ts],
                |r| r.get(0),
            )
            .optional()?;
        Ok(indexed.is_some())
    }

    /// Forgets the record of thread `thread_ts` (its document was deleted).
    pub fn remove_thread(
        &self,
        team: &str,
        channel: &str,
        thread_ts: &str,
    ) -> SlackStoreResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "DELETE FROM slack_index_thread
                 WHERE team_id = ?1 AND channel_id = ?2 AND thread_ts = ?3",
                params![team, channel, thread_ts],
            )?;
            tx.execute(
                "DELETE FROM slack_index_message
                 WHERE team_id = ?1 AND channel_id = ?2 AND thread_ts = ?3",
                params![team, channel, thread_ts],
            )?;
            Ok::<_, SlackStoreError>(())
        })
    }

    /// The threads of `channel` Shodh has read: the indexed ones and those read with
    /// nothing to index, each with the replies Slack listed for it when read. A thread only
    /// marked to be fetched, never read yet, is not one.
    pub fn threads(
        &self,
        team: &str,
        channel: &str,
    ) -> SlackStoreResult<HashMap<String, ThreadRecord>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(
            "SELECT thread_ts, latest_reply, reply_count, indexed_at IS NOT NULL
               FROM slack_index_thread
             WHERE team_id = ?1 AND channel_id = ?2 AND (indexed_at IS NOT NULL OR dirty = 0)",
        )?;
        let rows = stmt
            .query_map(params![team, channel], |r| {
                let replies: i64 = r.get(2)?;
                Ok((
                    r.get::<_, String>(0)?,
                    ThreadRecord {
                        latest_reply: r.get(1)?,
                        reply_count: usize::try_from(replies).unwrap_or(0),
                        indexed: r.get(3)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<HashMap<String, ThreadRecord>>>()?;
        Ok(rows)
    }

    // ── Forgetting ──────────────────────────────────────────────────

    /// The ids in the user's memories of the memories saved in `channel` (`None`: in any
    /// channel of `team`). Read before forgetting: they are forgotten in the user's
    /// memories first, and only then are the rows that hold them deleted.
    pub fn linked_memories(
        &self,
        team: &str,
        channel: Option<&str>,
    ) -> SlackStoreResult<Vec<String>> {
        let conn = self.conn.read()?;
        Ok(linked_memory_ids(&conn, team, channel)?)
    }

    /// Removes everything kept about `channel`: its sync state, threads, messages, its
    /// Slack memories and its threads' questions and answers. Returns the ids in the
    /// user's memories of its memories, which the caller forgets there.
    pub fn forget_channel(&self, team: &str, channel: &str) -> SlackStoreResult<Forgotten> {
        self.conn.write(|tx| {
            let memory_ids = linked_memory_ids(tx, team, Some(channel))?;
            let channels = tx.execute(
                "DELETE FROM slack_index_channel WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel],
            )?;
            let threads = tx.execute(
                "DELETE FROM slack_index_thread WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel],
            )?;
            tx.execute(
                "DELETE FROM slack_index_message WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel],
            )?;
            let memories = tx.execute(
                "DELETE FROM slack_memory WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel],
            )?;
            tx.execute(
                "DELETE FROM slack_thread_turn WHERE team_id = ?1 AND channel_id = ?2",
                params![team, channel],
            )?;
            Ok::<_, SlackStoreError>(Forgotten {
                memory_ids,
                channels,
                threads,
                memories,
            })
        })
    }

    /// Removes everything kept about Slack workspace `team` (disconnect and forget).
    pub fn forget_team(&self, team: &str) -> SlackStoreResult<Forgotten> {
        self.conn.write(|tx| {
            let memory_ids = linked_memory_ids(tx, team, None)?;
            let channels = tx.execute(
                "DELETE FROM slack_index_channel WHERE team_id = ?1",
                params![team],
            )?;
            let threads = tx.execute(
                "DELETE FROM slack_index_thread WHERE team_id = ?1",
                params![team],
            )?;
            tx.execute(
                "DELETE FROM slack_index_message WHERE team_id = ?1",
                params![team],
            )?;
            let memories =
                tx.execute("DELETE FROM slack_memory WHERE team_id = ?1", params![team])?;
            tx.execute(
                "DELETE FROM slack_thread_turn WHERE team_id = ?1",
                params![team],
            )?;
            Ok::<_, SlackStoreError>(Forgotten {
                memory_ids,
                channels,
                threads,
                memories,
            })
        })
    }

    // ── Memories ────────────────────────────────────────────────────

    /// Saves `memory`. Someone else's than the owner's is refused
    /// ([`SlackStoreError::Limit`]) when its channel keeps [`MAX_CHANNEL_MEMORIES`]
    /// already, or its author saved [`MAX_MEMORIES_PER_PERSON_PER_DAY`] there in the day
    /// before it: they all go into that channel's answers, so nobody fills them without
    /// bound.
    pub fn save_memory(&self, memory: &SlackMemory) -> SlackStoreResult<()> {
        let saved = stamp(memory.saved_at);
        let day_before = stamp(memory.saved_at - chrono::Duration::days(1));
        self.conn.write(|tx| {
            if !memory.owner {
                let kept: i64 = tx.query_row(
                    "SELECT count(*) FROM slack_memory
                     WHERE team_id = ?1 AND channel_id = ?2 AND owner = 0",
                    params![memory.team_id, memory.channel_id],
                    |r| r.get(0),
                )?;
                if usize::try_from(kept).unwrap_or(usize::MAX) >= MAX_CHANNEL_MEMORIES {
                    return Err(SlackStoreError::Limit(format!(
                        "This channel keeps {MAX_CHANNEL_MEMORIES} saved memories already,                          so nothing more was saved. The Shodh app can delete some                          (Connectors → Slack → From Slack)."
                    )));
                }
                let today: i64 = tx.query_row(
                    "SELECT count(*) FROM slack_memory
                     WHERE team_id = ?1 AND channel_id = ?2 AND owner = 0 AND author_id = ?3
                       AND saved_at > ?4",
                    params![
                        memory.team_id,
                        memory.channel_id,
                        memory.author_id,
                        day_before
                    ],
                    |r| r.get(0),
                )?;
                if usize::try_from(today).unwrap_or(usize::MAX) >= MAX_MEMORIES_PER_PERSON_PER_DAY
                {
                    return Err(SlackStoreError::Limit(format!(
                        "You saved {MAX_MEMORIES_PER_PERSON_PER_DAY} memories in this channel                          in the last day, so this one was not saved. Try again tomorrow."
                    )));
                }
            }
            tx.execute(
                &format!(
                    "INSERT INTO slack_memory ({MEMORY_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
                ),
                params![
                    memory.id,
                    memory.team_id,
                    memory.channel_id,
                    memory.channel_name,
                    memory.author_id,
                    memory.author_name,
                    memory.text,
                    memory.kind.as_str(),
                    memory.message_ts,
                    memory.permalink,
                    saved,
                    i64::from(memory.owner),
                    memory.memory_id
                ],
            )?;
            Ok::<_, SlackStoreError>(())
        })
    }

    /// Every memory saved from Slack workspace `team`, newest first.
    pub fn memories(&self, team: &str) -> SlackStoreResult<Vec<SlackMemory>> {
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {MEMORY_COLUMNS} FROM slack_memory WHERE team_id = ?1
             ORDER BY saved_at DESC, id"
        ))?;
        let rows = stmt
            .query_map(params![team], read_memory)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The channel-scoped memories of `channel` (saved there by people other than the
    /// owner), the most recent [`CHANNEL_MEMORIES_IN_PROMPT`], oldest first.
    pub fn channel_memories(
        &self,
        team: &str,
        channel: &str,
    ) -> SlackStoreResult<Vec<SlackMemory>> {
        let limit = i64::try_from(CHANNEL_MEMORIES_IN_PROMPT).unwrap_or(10);
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {MEMORY_COLUMNS} FROM slack_memory
             WHERE team_id = ?1 AND channel_id = ?2 AND owner = 0
             ORDER BY saved_at DESC, id LIMIT ?3"
        ))?;
        let mut rows = stmt
            .query_map(params![team, channel, limit], read_memory)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.reverse();
        Ok(rows)
    }

    pub fn memory(&self, id: &str) -> SlackStoreResult<Option<SlackMemory>> {
        let conn = self.conn.read()?;
        Ok(conn
            .query_row(
                &format!("SELECT {MEMORY_COLUMNS} FROM slack_memory WHERE id = ?1"),
                params![id],
                read_memory,
            )
            .optional()?)
    }

    /// Records the id memory `id` has in the user's memories (promoted).
    pub fn set_memory_id(&self, id: &str, memory_id: &str) -> SlackStoreResult<()> {
        self.conn.write(|tx| {
            tx.execute(
                "UPDATE slack_memory SET memory_id = ?2 WHERE id = ?1",
                params![id, memory_id],
            )
        })?;
        Ok(())
    }

    /// Deletes memory `id`; returns it as it was.
    pub fn delete_memory(&self, id: &str) -> SlackStoreResult<Option<SlackMemory>> {
        self.conn.write(|tx| {
            let memory = tx
                .query_row(
                    &format!("SELECT {MEMORY_COLUMNS} FROM slack_memory WHERE id = ?1"),
                    params![id],
                    read_memory,
                )
                .optional()?;
            tx.execute("DELETE FROM slack_memory WHERE id = ?1", params![id])?;
            Ok::<_, SlackStoreError>(memory)
        })
    }

    // ── Thread turns ────────────────────────────────────────────────

    /// The questions and answers of one thread, oldest first (the most recent
    /// [`TURNS_KEPT_PER_THREAD`]).
    pub fn thread_turns(
        &self,
        team: &str,
        channel: &str,
        thread: &str,
    ) -> SlackStoreResult<Vec<ThreadTurn>> {
        let limit = i64::try_from(TURNS_KEPT_PER_THREAD).unwrap_or(6);
        let conn = self.conn.read()?;
        let mut stmt = conn.prepare(
            "SELECT question, answer FROM slack_thread_turn
             WHERE team_id = ?1 AND channel_id = ?2 AND thread_key = ?3
             ORDER BY at DESC, rowid DESC LIMIT ?4",
        )?;
        let mut rows = stmt
            .query_map(params![team, channel, thread, limit], |r| {
                Ok(ThreadTurn {
                    question: r.get(0)?,
                    answer: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Adds a question and its answer to a thread; keeps the thread's last
    /// [`TURNS_KEPT_PER_THREAD`] and drops every turn older than [`TURNS_KEPT_DAYS`].
    pub fn add_turn(
        &self,
        team: &str,
        channel: &str,
        thread: &str,
        turn: &ThreadTurn,
        now: DateTime<Utc>,
    ) -> SlackStoreResult<()> {
        let at_now = stamp(now);
        let cutoff = stamp(now - chrono::Duration::days(TURNS_KEPT_DAYS));
        let keep = i64::try_from(TURNS_KEPT_PER_THREAD).unwrap_or(6);
        self.conn.write(|tx| {
            tx.execute(
                "INSERT INTO slack_thread_turn (team_id, channel_id, thread_key, at, question, answer)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![team, channel, thread, at_now, turn.question, turn.answer],
            )?;
            tx.execute(
                "DELETE FROM slack_thread_turn
                 WHERE team_id = ?1 AND channel_id = ?2 AND thread_key = ?3
                   AND rowid NOT IN (
                     SELECT rowid FROM slack_thread_turn
                      WHERE team_id = ?1 AND channel_id = ?2 AND thread_key = ?3
                      ORDER BY at DESC, rowid DESC LIMIT ?4)",
                params![team, channel, thread, keep],
            )?;
            tx.execute(
                "DELETE FROM slack_thread_turn WHERE at < ?1",
                params![cutoff],
            )?;
            Ok::<_, SlackStoreError>(())
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn store() -> (tempfile::TempDir, SlackStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SlackStore::open(&dir.path().join("shodh.db"), None).unwrap();
        (dir, store)
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap()
    }

    fn channel(id: &str) -> NewIndexedChannel {
        NewIndexedChannel {
            team_id: "T1".into(),
            channel_id: id.into(),
            name: format!("name-{id}"),
            workspace_id: "w1".into(),
            shared: false,
            private: false,
            window_days: 90,
        }
    }

    fn memory(id: &str, channel: &str, owner: bool, at: DateTime<Utc>) -> SlackMemory {
        SlackMemory {
            id: id.into(),
            team_id: "T1".into(),
            channel_id: channel.into(),
            channel_name: format!("#{channel}"),
            author_id: "U2".into(),
            author_name: "Ann".into(),
            text: format!("memory {id}"),
            kind: SlackMemoryKind::Text,
            message_ts: "1.1".into(),
            permalink: Some("https://x.slack.com/archives/C1/p11".into()),
            saved_at: at,
            owner,
            memory_id: owner.then(|| format!("mem-{id}")),
        }
    }

    #[test]
    fn a_channel_keeps_its_checkpoint_when_saved_again() {
        let (_dir, store) = store();
        let added = store.add_channel(&channel("C1"), now()).unwrap();
        assert_eq!(added.state, ChannelState::Active);
        assert!(!added.backfill_done && added.backfill_cursor.is_none());
        store
            .save_backfill_page(
                "T1",
                "C1",
                "200.0",
                Some("cur2"),
                Some("150.0"),
                Some("199.0"),
                now(),
            )
            .unwrap();
        store
            .save_backfill_page(
                "T1",
                "C1",
                "200.0",
                Some("cur3"),
                Some("120.0"),
                Some("160.0"),
                now(),
            )
            .unwrap();
        store
            .pause(
                "T1",
                "C1",
                now() + chrono::Duration::seconds(30),
                "rate limited",
            )
            .unwrap();
        let mut again = channel("C1");
        again.workspace_id = "w2".into();
        let saved = store.add_channel(&again, now()).unwrap();
        assert_eq!(saved.workspace_id, "w2");
        assert_eq!(saved.backfill_cursor.as_deref(), Some("cur3"), "resumes");
        assert_eq!(saved.backfill_latest.as_deref(), Some("200.0"));
        assert_eq!(saved.oldest_seen.as_deref(), Some("120.0"), "the earliest");
        assert_eq!(saved.newest_ts.as_deref(), Some("199.0"), "the latest");
        assert!(saved.paused_until.is_none() && saved.error.is_none());
        store
            .save_backfill_page("T1", "C1", "200.0", None, Some("100.0"), None, now())
            .unwrap();
        assert!(store.channel("T1", "C1").unwrap().unwrap().backfill_done);
        assert!(store
            .add_channel(
                &NewIndexedChannel {
                    window_days: 0,
                    ..channel("C2")
                },
                now()
            )
            .is_err());
    }

    #[test]
    fn threads_messages_and_dirty_marks() {
        let (_dir, store) = store();
        store.add_channel(&channel("C1"), now()).unwrap();
        store
            .record_thread(
                "T1",
                "C1",
                "10.0",
                Some("12.0"),
                2,
                &["10.0".into(), "11.0".into(), "12.0".into()],
                now(),
            )
            .unwrap();
        assert_eq!(
            store
                .thread_of_message("T1", "C1", "11.0")
                .unwrap()
                .as_deref(),
            Some("10.0")
        );
        assert_eq!(
            store
                .thread_of_message("T1", "C1", "10.0")
                .unwrap()
                .as_deref(),
            Some("10.0")
        );
        assert_eq!(store.thread_of_message("T1", "C1", "99.0").unwrap(), None);
        assert_eq!(store.channel("T1", "C1").unwrap().unwrap().threads, 1);
        store.mark_dirty("T1", "C1", "10.0", now()).unwrap();
        store
            .mark_dirty("T1", "C1", "20.0", now() + chrono::Duration::seconds(1))
            .unwrap();
        assert_eq!(
            store.dirty_threads("T1", 10).unwrap(),
            vec![
                ("C1".to_string(), "10.0".to_string()),
                ("C1".to_string(), "20.0".to_string())
            ]
        );
        assert_eq!(
            store.channel("T1", "C1").unwrap().unwrap().threads,
            1,
            "a mark is not a thread"
        );
        // A channel that is not active gives none.
        store
            .set_state("T1", "C1", ChannelState::Archived, None)
            .unwrap();
        assert!(store.dirty_threads("T1", 10).unwrap().is_empty());
        store
            .set_state("T1", "C1", ChannelState::Active, None)
            .unwrap();
        store
            .record_thread(
                "T1",
                "C1",
                "10.0",
                Some("13.0"),
                3,
                &["10.0".into(), "13.0".into()],
                now(),
            )
            .unwrap();
        assert_eq!(
            store.thread_of_message("T1", "C1", "11.0").unwrap(),
            None,
            "replaced"
        );
        assert_eq!(store.dirty_threads("T1", 10).unwrap().len(), 1);
        assert_eq!(
            store.threads("T1", "C1").unwrap().get("10.0"),
            Some(&ThreadRecord {
                latest_reply: Some("13.0".into()),
                reply_count: 3,
                indexed: true
            })
        );
        store.remove_thread("T1", "C1", "10.0").unwrap();
        assert!(store.threads("T1", "C1").unwrap().is_empty());
        assert_eq!(store.thread_of_message("T1", "C1", "13.0").unwrap(), None);
    }

    #[test]
    fn forgetting_a_channel_removes_everything_of_it_and_nothing_else() {
        let (_dir, store) = store();
        for c in ["C1", "C2"] {
            store.add_channel(&channel(c), now()).unwrap();
            store
                .record_thread("T1", c, "10.0", None, 0, &["10.0".into()], now())
                .unwrap();
            store
                .save_memory(&memory(&format!("m-{c}"), c, false, now()))
                .unwrap();
            store
                .save_memory(&memory(&format!("o-{c}"), c, true, now()))
                .unwrap();
            store
                .add_turn(
                    "T1",
                    c,
                    "10.0",
                    &ThreadTurn {
                        question: "q".into(),
                        answer: "a".into(),
                    },
                    now(),
                )
                .unwrap();
        }
        let forgotten = store.forget_channel("T1", "C1").unwrap();
        assert_eq!(forgotten.memory_ids, vec!["mem-o-C1".to_string()]);
        assert_eq!(
            (forgotten.channels, forgotten.threads, forgotten.memories),
            (1, 1, 2)
        );
        assert!(store.channel("T1", "C1").unwrap().is_none());
        assert!(store.threads("T1", "C1").unwrap().is_empty());
        assert!(store.thread_turns("T1", "C1", "10.0").unwrap().is_empty());
        assert!(store
            .memories("T1")
            .unwrap()
            .iter()
            .all(|m| m.channel_id == "C2"));
        assert!(
            store.channel("T1", "C2").unwrap().is_some(),
            "the other channel stays"
        );
        assert_eq!(store.thread_turns("T1", "C2", "10.0").unwrap().len(), 1);

        let all = store.forget_team("T1").unwrap();
        assert_eq!(all.memory_ids, vec!["mem-o-C2".to_string()]);
        assert!(store.channels("T1").unwrap().is_empty());
        assert!(store.memories("T1").unwrap().is_empty());
    }

    #[test]
    fn channel_memories_are_the_channels_own_and_not_the_owners() {
        let (_dir, store) = store();
        for i in 0..12 {
            store
                .save_memory(&memory(
                    &format!("m{i:02}"),
                    "C1",
                    false,
                    now() + chrono::Duration::minutes(i),
                ))
                .unwrap();
        }
        store.save_memory(&memory("o1", "C1", true, now())).unwrap();
        store
            .save_memory(&memory("other", "C2", false, now()))
            .unwrap();
        let used = store.channel_memories("T1", "C1").unwrap();
        assert_eq!(used.len(), CHANNEL_MEMORIES_IN_PROMPT);
        assert_eq!(
            used.first().unwrap().id,
            "m02",
            "the most recent ten, oldest first"
        );
        assert_eq!(used.last().unwrap().id, "m11");
        assert!(used.iter().all(|m| !m.owner && m.channel_id == "C1"));
        store.set_memory_id("m05", "mem-x").unwrap();
        assert_eq!(
            store.memory("m05").unwrap().unwrap().memory_id.as_deref(),
            Some("mem-x")
        );
        let deleted = store.delete_memory("m05").unwrap().unwrap();
        assert_eq!(
            deleted.permalink.as_deref(),
            Some("https://x.slack.com/archives/C1/p11")
        );
        assert!(store.memory("m05").unwrap().is_none());
        assert_eq!(store.delete_memory("m05").unwrap(), None);
    }

    #[test]
    fn thread_turns_are_bounded_per_thread_and_by_age() {
        let (_dir, store) = store();
        let turn = |i: usize| ThreadTurn {
            question: format!("q{i}"),
            answer: format!("a{i}"),
        };
        for i in 0..8 {
            store
                .add_turn(
                    "T1",
                    "C1",
                    "10.0",
                    &turn(i),
                    now() + chrono::Duration::seconds(i as i64),
                )
                .unwrap();
        }
        store
            .add_turn("T1", "C1", "20.0", &turn(99), now())
            .unwrap();
        let turns = store.thread_turns("T1", "C1", "10.0").unwrap();
        assert_eq!(turns.len(), TURNS_KEPT_PER_THREAD);
        assert_eq!(turns[0].question, "q2");
        assert_eq!(turns.last().unwrap().question, "q7");
        assert_eq!(
            store.thread_turns("T1", "C1", "20.0").unwrap(),
            vec![turn(99)]
        );
        assert!(store.thread_turns("T1", "C9", "10.0").unwrap().is_empty());
        // A month later every old turn is gone.
        store
            .add_turn(
                "T1",
                "C1",
                "30.0",
                &turn(1),
                now() + chrono::Duration::days(TURNS_KEPT_DAYS + 1),
            )
            .unwrap();
        assert!(store.thread_turns("T1", "C1", "10.0").unwrap().is_empty());
        assert_eq!(store.thread_turns("T1", "C1", "30.0").unwrap().len(), 1);
    }

    #[test]
    fn errors_stop_and_retry() {
        let (_dir, store) = store();
        store.add_channel(&channel("C1"), now()).unwrap();
        store.add_channel(&channel("C2"), now()).unwrap();
        store
            .set_state("T1", "C2", ChannelState::Archived, None)
            .unwrap();
        store.stop_all("T1", "Slack refused the bot token").unwrap();
        let c1 = store.channel("T1", "C1").unwrap().unwrap();
        assert_eq!(c1.state, ChannelState::Error);
        assert_eq!(
            store.channel("T1", "C2").unwrap().unwrap().state,
            ChannelState::Archived
        );
        store.retry_stopped("T1").unwrap();
        assert_eq!(
            store.channel("T1", "C1").unwrap().unwrap().state,
            ChannelState::Active
        );
    }
}
