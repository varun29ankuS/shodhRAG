//! Asking Recall: search (pointers and receipts, ranked), scope (every session in a
//! period, complete and unranked) and open (the exact turn, read again from the
//! transcript and redacted, with the turns around it).

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{anyhow, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;

use super::catalog::{FileRecord, FileState, TurnRow};
use super::chunks::{head_and_tail, turn_heading};
use super::discover::{project_name, space_id};
use super::entities::{self, EntityKind};
use super::index::{parse_source, RecallIndex};
use super::redact::Redactor;
use super::settings::RecallSettings;
use super::{clip, read_transcript, ParseBudget, Role, Tool};
use crate::types::{MetadataFilter, SourceSet};

/// Sessions a search returns by default, and at most.
pub const DEFAULT_SESSIONS: usize = 5;
pub const MAX_SESSIONS: usize = 20;
/// Receipts per session at most.
pub const RECEIPTS_PER_SESSION: usize = 3;
/// Characters of a receipt's snippet.
pub const SNIPPET_CHARS: usize = 320;
/// Turns before and after the opened turn, by default and at most.
pub const DEFAULT_AROUND: u32 = 2;
pub const MAX_AROUND: u32 = 10;
/// Characters of an opened turn's text at most (a tool output shows its head and tail).
pub const OPEN_TURN_CHARS: usize = 6000;
/// Sessions a scope lists at most.
pub const MAX_SCOPE_SESSIONS: usize = 1000;
/// Sources a search may be limited to by name; a larger limit is applied to the results.
const MAX_SOURCE_LIMIT: usize = 3000;

/// What to look for, and where.
#[derive(Debug, Clone, Default)]
pub struct RecallQuery {
    pub text: String,
    /// Only these tools (all chosen tools when empty).
    pub tools: Vec<Tool>,
    /// Only projects whose folder or name contains one of these (case-insensitive).
    pub projects: Vec<String>,
    /// Only sessions overlapping this window (Unix seconds).
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Only sessions holding this entity (exact, or a path ending with it).
    pub entity: Option<String>,
    pub entity_kind: Option<EntityKind>,
    pub limit: usize,
}

/// A pointer to one turn: what opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    /// `<file id>:<turn>`, what `open` takes.
    pub id: String,
    pub file_id: i64,
    pub turn: u32,
    pub role: Role,
    pub tool_name: Option<String>,
    pub at: Option<String>,
    pub heading: String,
    /// A little of what matched (redacted).
    pub snippet: String,
}

/// A session a search found.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHit {
    pub tool: Tool,
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub project_dir: String,
    pub branch: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub turns: u32,
    pub score: f32,
    /// The query named an id this session holds exactly.
    pub exact: Vec<String>,
    pub receipts: Vec<Receipt>,
}

/// A search's answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchAnswer {
    pub sessions: Vec<SessionHit>,
    /// Sessions searched (those the filters allowed).
    pub searched: usize,
    pub took_ms: u64,
}

/// A session listed by `scope`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub tool: Tool,
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub project_dir: String,
    pub branch: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub turns: u32,
    /// Subagent threads of the session.
    pub threads: u32,
    /// The session's first turn, to open.
    pub first: Option<String>,
    /// A few ids it holds (PRs, branches, files), for a summary.
    pub entities: Vec<String>,
}

/// Every session in a window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeAnswer {
    pub sessions: Vec<SessionSummary>,
    /// More sessions are in the window than listed ([`MAX_SCOPE_SESSIONS`]).
    pub more: bool,
}

/// One turn as opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedTurn {
    pub turn: u32,
    pub role: Role,
    pub tool_name: Option<String>,
    pub at: Option<String>,
    pub heading: String,
    pub text: String,
    /// Characters of the turn not shown (a long output's middle).
    pub left_out: usize,
    pub kept_back: Option<String>,
    pub line: u64,
    pub target: bool,
}

/// A receipt opened: its session and the turns around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub tool: Tool,
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub project_dir: String,
    pub branch: Option<String>,
    pub file_id: i64,
    /// The transcript on disk.
    pub path: String,
    pub turn: u32,
    pub turns_total: u32,
    pub turns: Vec<OpenedTurn>,
    /// Lines of the transcript that could not be read.
    pub unreadable_lines: u32,
}

fn rfc3339(secs: Option<i64>) -> Option<String> {
    secs.and_then(|s| Utc.timestamp_opt(s, 0).single())
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// A receipt's id.
pub fn receipt_id(file_id: i64, turn: u32) -> String {
    format!("{file_id}:{turn}")
}

/// The file and turn a receipt id names.
pub fn parse_receipt(id: &str) -> Option<(i64, u32)> {
    let (file, turn) = id.trim().split_once(':')?;
    Some((file.parse().ok()?, turn.parse().ok()?))
}

/// The indexed files a query may reach (tool, project and removal filters).
fn reachable(
    index: &RecallIndex,
    settings: &RecallSettings,
    query_tools: &[Tool],
    projects: &[String],
) -> Result<Vec<FileRecord>> {
    let wanted_projects: Vec<String> = projects
        .iter()
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    Ok(index
        .catalog()
        .files()?
        .into_iter()
        .filter(|f| f.state == FileState::Indexed)
        .filter(|f| settings.wants(f.tool, &f.project_key))
        .filter(|f| query_tools.is_empty() || query_tools.contains(&f.tool))
        .filter(|f| {
            wanted_projects.is_empty()
                || wanted_projects
                    .iter()
                    .any(|p| f.project_dir.to_lowercase().contains(p.as_str()))
        })
        .collect())
}

fn overlaps(file: &FileRecord, since: Option<i64>, until: Option<i64>) -> bool {
    let start = file.started_at.unwrap_or(file.mtime);
    let end = file.ended_at.unwrap_or(file.mtime).max(start);
    since.is_none_or(|s| end >= s) && until.is_none_or(|u| start <= u)
}

type SessionKey = (Tool, String);

/// Searches the sessions: hybrid search over the indexed turns, results grouped by
/// session, each with receipts. A session holding an id the query names exactly (a PR
/// number, a SHA, a file) comes first.
pub async fn search(
    index: &RecallIndex,
    settings: &RecallSettings,
    query: &RecallQuery,
) -> Result<SearchAnswer> {
    let started = std::time::Instant::now();
    let limit = if query.limit == 0 {
        DEFAULT_SESSIONS
    } else {
        query.limit.min(MAX_SESSIONS)
    };
    let mut files = reachable(index, settings, &query.tools, &query.projects)?;
    if query.since.is_some() || query.until.is_some() {
        files.retain(|f| overlaps(f, query.since, query.until));
    }
    let catalog = index.catalog();
    if let Some(entity) = query.entity.as_deref().filter(|e| !e.trim().is_empty()) {
        let kind = query
            .entity_kind
            .or_else(|| entity.trim().starts_with('#').then_some(EntityKind::Issue));
        let key = entities::normalize(kind.unwrap_or(EntityKind::File), entity);
        let holding: HashSet<i64> = catalog
            .entity_turns(kind, &key, 5000)?
            .into_iter()
            .map(|(file, _, _)| file)
            .collect();
        files.retain(|f| holding.contains(&f.id));
    }
    let by_id: HashMap<i64, FileRecord> = files.iter().map(|f| (f.id, f.clone())).collect();
    let sessions_searched: HashSet<SessionKey> = files
        .iter()
        .map(|f| (f.tool, f.session_id.clone()))
        .collect();
    if files.is_empty() || query.text.trim().is_empty() {
        return Ok(SearchAnswer {
            sessions: Vec::new(),
            searched: sessions_searched.len(),
            took_ms: started.elapsed().as_millis() as u64,
        });
    }

    // Limit the search to the reachable files: by their spaces (tool and project), and by
    // their parts when a window or an entity narrowed them.
    let narrowed = query.since.is_some() || query.until.is_some() || query.entity.is_some();
    let spaces: Vec<String> = files
        .iter()
        .map(|f| space_id(f.tool, &f.project_key))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let sources: Vec<String> = files
        .iter()
        .flat_map(|f| (0..f.parts).map(move |p| super::index::source_id(f.tool, f.id, p)))
        .collect();
    let filter = if narrowed && sources.len() <= MAX_SOURCE_LIMIT {
        MetadataFilter {
            any_of: Some(SourceSet {
                space_ids: Vec::new(),
                source_paths: sources,
            }),
            ..MetadataFilter::default()
        }
    } else {
        MetadataFilter {
            any_of: Some(SourceSet {
                space_ids: spaces,
                source_paths: Vec::new(),
            }),
            ..MetadataFilter::default()
        }
    };
    // Several chunks per session wanted, so a session's best turns reach the grouping;
    // the source decay spreads them over sessions.
    let chunks_wanted = (limit * 4).max(20);
    let results = index
        .engine()
        .read()
        .await
        .search_comprehensive(&query.text, chunks_wanted, Some(filter))
        .await?;

    // Ids the query names exactly.
    let mut exact: HashMap<SessionKey, Vec<String>> = HashMap::new();
    for entity in entities::extract(&query.text) {
        if matches!(entity.kind, EntityKind::Url | EntityKind::Model) {
            continue;
        }
        for (file_id, _, value) in catalog.entity_turns(Some(entity.kind), &entity.key(), 2000)? {
            if let Some(file) = by_id.get(&file_id) {
                let names = exact
                    .entry((file.tool, file.session_id.clone()))
                    .or_default();
                if !names.contains(&value) {
                    names.push(value);
                }
            }
        }
    }

    struct Group {
        best: f32,
        order: usize,
        receipts: Vec<Receipt>,
        turns_seen: HashSet<(i64, u32)>,
    }
    let mut groups: HashMap<SessionKey, Group> = HashMap::new();
    for (order, result) in results.iter().enumerate() {
        let source = result
            .metadata
            .get("source_file")
            .map(String::as_str)
            .unwrap_or("");
        let Some((file_id, part)) = parse_source(source) else {
            continue;
        };
        let Some(file) = by_id.get(&file_id) else {
            continue;
        };
        if query.since.is_some() || query.until.is_some() {
            if !overlaps(file, query.since, query.until) {
                continue;
            }
        }
        let chunk: u32 = result
            .metadata
            .get("chunk_index")
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let turn = catalog.turn_for_chunk(file_id, part, chunk)?;
        let key = (file.tool, file.session_id.clone());
        let group = groups.entry(key).or_insert(Group {
            best: result.score,
            order,
            receipts: Vec::new(),
            turns_seen: HashSet::new(),
        });
        group.best = group.best.max(result.score);
        let Some(turn) = turn else { continue };
        if group.receipts.len() >= RECEIPTS_PER_SESSION
            || !group.turns_seen.insert((file_id, turn.turn))
        {
            continue;
        }
        group.receipts.push(receipt(&turn, &result.snippet));
    }
    // Sessions named exactly that the search did not reach still come back, with the
    // turn that names the id as their receipt.
    for (key, names) in &exact {
        if groups.contains_key(key) {
            continue;
        }
        let Some(file) = files
            .iter()
            .find(|f| (f.tool, f.session_id.clone()) == *key)
        else {
            continue;
        };
        let mut receipts = Vec::new();
        for name in names.iter().take(1) {
            let found =
                catalog.entity_turns(None, &entities::normalize(EntityKind::File, name), 50)?;
            if let Some((file_id, turn, _)) = found.into_iter().find(|(f, _, _)| {
                by_id
                    .get(f)
                    .is_some_and(|r| r.session_id == file.session_id)
            }) {
                if let Some(row) = catalog.turn(file_id, turn)? {
                    receipts.push(receipt(&row, ""));
                }
            }
        }
        groups.insert(
            key.clone(),
            Group {
                best: f32::MIN,
                order: usize::MAX,
                receipts,
                turns_seen: HashSet::new(),
            },
        );
    }

    let mut ranked: Vec<(SessionKey, Group)> = groups.into_iter().collect();
    ranked.sort_by(|(ka, a), (kb, b)| {
        let ea = exact.get(ka).map_or(0, Vec::len);
        let eb = exact.get(kb).map_or(0, Vec::len);
        eb.cmp(&ea)
            .then(b.best.total_cmp(&a.best))
            .then(a.order.cmp(&b.order))
    });
    let mut sessions = Vec::new();
    for ((tool, session_id), group) in ranked.into_iter().take(limit) {
        let session_files: Vec<&FileRecord> = files
            .iter()
            .filter(|f| f.tool == tool && f.session_id == session_id)
            .collect();
        let Some(main) = session_files
            .iter()
            .find(|f| f.thread.is_none())
            .or(session_files.first())
        else {
            continue;
        };
        let started = session_files.iter().filter_map(|f| f.started_at).min();
        let ended = session_files.iter().filter_map(|f| f.ended_at).max();
        sessions.push(SessionHit {
            tool,
            title: session_title(main),
            project: project_name(&main.project_dir),
            project_dir: main.project_dir.clone(),
            branch: main.branch.clone(),
            started: rfc3339(started),
            ended: rfc3339(ended),
            turns: session_files.iter().map(|f| f.turns).sum(),
            score: if group.best == f32::MIN {
                0.0
            } else {
                group.best
            },
            exact: exact
                .get(&(tool, session_id.clone()))
                .cloned()
                .unwrap_or_default(),
            receipts: group.receipts,
            session_id,
        });
    }
    Ok(SearchAnswer {
        sessions,
        searched: sessions_searched.len(),
        took_ms: started.elapsed().as_millis() as u64,
    })
}

fn session_title(file: &FileRecord) -> String {
    file.title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| {
            format!(
                "{} session in {}",
                file.tool.label(),
                project_name(&file.project_dir)
            )
        })
}

fn receipt(turn: &TurnRow, snippet: &str) -> Receipt {
    let heading = format!(
        "Turn {}, {}",
        turn.turn + 1,
        match (&turn.role, &turn.tool_name) {
            (Role::ToolCall, Some(name)) => format!("tool call {name}"),
            (Role::ToolOutput, Some(name)) => format!("tool output of {name}"),
            (role, _) => role.label().to_lowercase(),
        }
    );
    Receipt {
        id: receipt_id(turn.file_id, turn.turn),
        file_id: turn.file_id,
        turn: turn.turn,
        role: turn.role,
        tool_name: turn.tool_name.clone(),
        at: rfc3339(turn.at),
        heading,
        snippet: clip(snippet.trim(), SNIPPET_CHARS),
    }
}

/// Every session overlapping `[since, until]` (Unix seconds), oldest first, complete and
/// unranked: for "what did I do last week".
pub fn scope(
    index: &RecallIndex,
    settings: &RecallSettings,
    since: Option<i64>,
    until: Option<i64>,
    tools: &[Tool],
    projects: &[String],
) -> Result<ScopeAnswer> {
    let files: Vec<FileRecord> = reachable(index, settings, tools, projects)?
        .into_iter()
        .filter(|f| overlaps(f, since, until))
        .collect();
    let mut by_session: BTreeMap<SessionKey, Vec<FileRecord>> = BTreeMap::new();
    for file in files {
        by_session
            .entry((file.tool, file.session_id.clone()))
            .or_default()
            .push(file);
    }
    let catalog = index.catalog();
    let mut sessions: Vec<SessionSummary> = Vec::new();
    for ((tool, session_id), files) in by_session {
        let Some(main) = files.iter().find(|f| f.thread.is_none()).or(files.first()) else {
            continue;
        };
        let started = files.iter().filter_map(|f| f.started_at).min();
        let ended = files.iter().filter_map(|f| f.ended_at).max();
        let first_turn = match since {
            Some(s) if started.is_some_and(|st| st < s) => {
                catalog.first_turn_from(main.id, s)?.map(|t| t.turn)
            }
            _ => Some(0),
        };
        let entities = catalog
            .file_entities(
                main.id,
                &[EntityKind::Issue, EntityKind::Branch, EntityKind::File],
                8,
            )?
            .into_iter()
            .map(|(_, value)| value)
            .collect();
        sessions.push(SessionSummary {
            tool,
            title: session_title(main),
            project: project_name(&main.project_dir),
            project_dir: main.project_dir.clone(),
            branch: main.branch.clone(),
            started: rfc3339(started),
            ended: rfc3339(ended),
            turns: files.iter().map(|f| f.turns).sum(),
            threads: files.iter().filter(|f| f.thread.is_some()).count() as u32,
            first: first_turn
                .filter(|_| main.turns > 0)
                .map(|t| receipt_id(main.id, t)),
            entities,
            session_id,
        });
    }
    sessions.sort_by(|a, b| {
        a.started
            .cmp(&b.started)
            .then(a.session_id.cmp(&b.session_id))
    });
    let more = sessions.len() > MAX_SCOPE_SESSIONS;
    sessions.truncate(MAX_SCOPE_SESSIONS);
    Ok(ScopeAnswer { sessions, more })
}

/// Opens a receipt: reads the transcript again (redacted with the current rules and the
/// user's patterns) and returns the turn with `around` turns before and after it. Only a
/// transcript Recall indexed, of a tool and project still chosen, opens.
pub fn open(
    index: &RecallIndex,
    settings: &RecallSettings,
    receipt: &str,
    around: u32,
) -> Result<Opened> {
    let (file_id, turn) = parse_receipt(receipt)
        .ok_or_else(|| anyhow!("\"{receipt}\" is not a receipt: receipts look like 12:40"))?;
    let file = index
        .catalog()
        .file(file_id)?
        .filter(|f| f.state == FileState::Indexed && settings.wants(f.tool, &f.project_key))
        .ok_or_else(|| anyhow!("Recall has no session for the receipt {receipt} (it may have been removed or excluded)"))?;
    let redactor = Redactor::new(&settings.patterns).map_err(|e| anyhow!("{e}"))?;
    let path = std::path::PathBuf::from(&file.path);
    if !path.is_file() {
        return Err(anyhow!(
            "The transcript is no longer on disk: {}",
            file.path
        ));
    }
    let transcript = read_transcript(file.tool, &path, &redactor, ParseBudget::default())?;
    let total = transcript.turns.len() as u32;
    if turn >= total {
        return Err(anyhow!(
            "The session has {total} turns; turn {} is not in it (it changed since it was indexed)",
            turn + 1
        ));
    }
    let around = around.min(MAX_AROUND);
    let first = turn.saturating_sub(around);
    let last = (turn + around).min(total.saturating_sub(1));
    let turns = transcript.turns[first as usize..=last as usize]
        .iter()
        .map(|t| {
            let shown = match t.role {
                Role::ToolOutput => head_and_tail_for_open(&t.text),
                _ => clip(&t.text, OPEN_TURN_CHARS),
            };
            let left_out = t.text.chars().count().saturating_sub(shown.chars().count());
            OpenedTurn {
                turn: t.index,
                role: t.role,
                tool_name: t.tool_name.clone(),
                at: t
                    .at
                    .map(|a| a.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                heading: turn_heading(t),
                text: shown,
                left_out,
                kept_back: t.kept_back.map(str::to_string),
                line: t.line,
                target: t.index == turn,
            }
        })
        .collect();
    let title = transcript
        .info
        .title
        .clone()
        .unwrap_or_else(|| session_title(&file));
    Ok(Opened {
        tool: file.tool,
        session_id: file.session_id.clone(),
        title,
        project: project_name(&file.project_dir),
        project_dir: file.project_dir.clone(),
        branch: transcript.info.branch.clone().or(file.branch.clone()),
        file_id,
        path: file.path.clone(),
        turn,
        turns_total: total,
        turns,
        unreadable_lines: transcript.errors.len() as u32,
    })
}

/// A tool output as opened: up to [`OPEN_TURN_CHARS`], its head and tail beyond that.
fn head_and_tail_for_open(text: &str) -> String {
    let count = text.chars().count();
    if count <= OPEN_TURN_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(OPEN_TURN_CHARS * 2 / 3).collect();
    let tail: String = text.chars().skip(count - OPEN_TURN_CHARS / 3).collect();
    format!(
        "{head}\n[… {} characters not shown …]\n{tail}",
        count - head.chars().count() - tail.chars().count()
    )
}

/// The window a period names: `today`, `yesterday`, `this week`, `last week`,
/// `this month`, `last month`, `last N days`, or a date `YYYY-MM-DD` (that day), in UTC.
pub fn period(text: &str, now: DateTime<Utc>) -> Option<(i64, i64)> {
    use chrono::{Datelike, Duration, NaiveDate};
    let t = text.trim().to_lowercase();
    let day_start = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .map(|n| Utc.from_utc_datetime(&n).timestamp())
    };
    let today = now.date_naive();
    let end_of = |d: NaiveDate| day_start(d + Duration::days(1)).map(|s| s - 1);
    match t.as_str() {
        "today" => Some((day_start(today)?, now.timestamp())),
        "yesterday" => {
            let d = today - Duration::days(1);
            Some((day_start(d)?, end_of(d)?))
        }
        "this week" => {
            let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
            Some((day_start(monday)?, now.timestamp()))
        }
        "last week" => {
            let monday =
                today - Duration::days(i64::from(today.weekday().num_days_from_monday()) + 7);
            Some((day_start(monday)?, end_of(monday + Duration::days(6))?))
        }
        "this month" => Some((day_start(today.with_day(1)?)?, now.timestamp())),
        "last month" => {
            let first_this = today.with_day(1)?;
            let last_prev = first_this - Duration::days(1);
            Some((day_start(last_prev.with_day(1)?)?, end_of(last_prev)?))
        }
        _ => {
            if let Some(n) = t
                .strip_prefix("last ")
                .and_then(|r| r.strip_suffix(" days").or_else(|| r.strip_suffix(" day")))
                .and_then(|n| n.trim().parse::<i64>().ok())
                .filter(|n| (1..=3650).contains(n))
            {
                return Some((now.timestamp() - n * 86_400, now.timestamp()));
            }
            let date = NaiveDate::parse_from_str(&t, "%Y-%m-%d").ok()?;
            Some((day_start(date)?, end_of(date)?))
        }
    }
}
