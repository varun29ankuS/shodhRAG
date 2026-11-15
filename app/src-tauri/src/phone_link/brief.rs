//! "A brief before your meetings" (phone): for one event of the calendar,
//! what was discussed last time and what in the library is about it, each
//! cited. Retrieval only, with no model: the conversations whose title
//! shares a significant word with the event, and the notes and documents a
//! search for the event finds that really mention it. An event nothing
//! matches has no brief.
//!
//! The phone asks only while the user turned on calendar sync and "Let Shodh
//! use my calendar" (the brief reads the event's title, and the phone's own
//! events reach the computer only then).

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::rag_commands::RagState;

/// Conversations listed in a brief.
const MAX_CONVERSATIONS: usize = 3;
/// Passages cited in a brief.
const MAX_SOURCES: usize = 4;
/// Passages searched for.
const SEARCHED: usize = 12;
/// Characters of a cited passage sent to the phone.
const PASSAGE_CHARS: usize = 360;

/// Words that never make a match on their own.
const STOP_WORDS: &[&str] = &[
    "about", "after", "again", "also", "and", "call", "catch", "chat", "daily", "event", "first",
    "from", "have", "into", "meet", "meeting", "monthly", "next", "notes", "only", "over", "quick",
    "review", "session", "sync", "that", "their", "them", "then", "there", "this", "time", "today",
    "until", "update", "weekly", "what", "when", "with", "your",
];

/// The words of `text` that can make a match: four letters or more, not a stop word.
pub fn significant_words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 4 && !STOP_WORDS.contains(&w.as_str()))
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

fn mentions(text: &str, words: &[String]) -> bool {
    let own: HashSet<String> = significant_words(text).into_iter().collect();
    words.iter().any(|w| own.contains(w))
}

/// A conversation the brief may list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefConversation {
    pub id: String,
    pub title: String,
    pub updated_at: String,
}

/// A passage a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub file: String,
    pub path: String,
    pub page: Option<String>,
    pub text: String,
}

/// A cited source of the brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefSource {
    pub n: u32,
    pub file: String,
    pub path: String,
    pub page: Option<String>,
    pub text: String,
    /// `note` or `document`.
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    pub event_id: String,
    pub title: String,
    pub conversations: Vec<BriefConversation>,
    pub sources: Vec<BriefSource>,
}

/// The brief for event `title` (with `description`), or `None` when nothing matches.
pub fn compose_brief(
    event_id: &str,
    title: &str,
    description: &str,
    conversations: &[BriefConversation],
    found: &[Found],
) -> Option<Brief> {
    let words = significant_words(&format!("{title} {description}"));
    if words.is_empty() {
        return None;
    }
    let mut listed: Vec<BriefConversation> = conversations
        .iter()
        .filter(|c| mentions(&c.title, &words))
        .cloned()
        .collect();
    listed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    listed.truncate(MAX_CONVERSATIONS);

    let mut sources = Vec::new();
    let mut paths = HashSet::new();
    for passage in found {
        // Calendar entries are the event itself; only notes and documents are sources.
        let note = passage.path.starts_with("note://");
        if (!note && passage.path.contains("://")) || sources.len() >= MAX_SOURCES {
            continue;
        }
        if !mentions(&format!("{} {}", passage.file, passage.text), &words)
            || !paths.insert(passage.path.clone())
        {
            continue;
        }
        sources.push(BriefSource {
            n: sources.len() as u32 + 1,
            file: passage.file.clone(),
            path: passage.path.clone(),
            page: passage.page.clone(),
            text: passage.text.trim().chars().take(PASSAGE_CHARS).collect(),
            kind: if note { "note" } else { "document" },
        });
    }
    if listed.is_empty() && sources.is_empty() {
        return None;
    }
    Some(Brief {
        event_id: event_id.to_string(),
        title: title.to_string(),
        conversations: listed,
        sources,
    })
}

fn unavailable(message: impl Into<String>) -> Value {
    json!({ "code": "unavailable", "message": message.into() })
}

/// `meeting_brief`: the brief of the computer's event `eventId`, or `null`.
pub async fn meeting_brief(app: &AppHandle, raw: Value) -> Result<Value, Value> {
    let id = raw
        .get("eventId")
        .and_then(Value::as_str)
        .ok_or_else(|| json!({ "code": "invalid_request", "message": "Which event?" }))?
        .to_string();
    let events = crate::calendar_commands::load_events(app.clone())
        .await
        .map_err(unavailable)?;
    let event = events
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| shodh_link::bridge::refused("This event is no longer in Shodh."))?;
    let conversations: Vec<BriefConversation> = crate::conversation_commands::load_conversations(
        app.clone(),
        app.state(),
        app.state(),
        app.state(),
    )
    .await
    .map_err(unavailable)?
    .into_iter()
    .map(|c| BriefConversation {
        id: c.id,
        title: c.title,
        updated_at: c.updated_at,
    })
    .collect();
    let query = format!("{} {}", event.title, event.description);
    let found: Vec<Found> = {
        let state = app.state::<RagState>();
        if !*state.rag_initialized.read().await {
            Vec::new()
        } else {
            state
                .rag
                .read()
                .await
                .search(query.trim(), SEARCHED)
                .await
                .map_err(|e| unavailable(format!("{e:#}")))?
                .into_iter()
                .map(|r| Found {
                    file: if r.title.trim().is_empty() {
                        r.source
                            .rsplit(['/', '\\'])
                            .next()
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        r.title.clone()
                    },
                    page: r
                        .metadata
                        .get("page")
                        .or_else(|| r.metadata.get("page_number"))
                        .cloned(),
                    path: r
                        .metadata
                        .get(shodh_rag::rag_engine::DISPLAY_PATH_KEY)
                        .cloned()
                        .unwrap_or(r.source),
                    text: r.text,
                })
                .collect()
        }
    };
    Ok(compose_brief(
        &event.id,
        &event.title,
        &event.description,
        &conversations,
        &found,
    )
    .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
    .unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(id: &str, title: &str, updated: &str) -> BriefConversation {
        BriefConversation {
            id: id.into(),
            title: title.into(),
            updated_at: updated.into(),
        }
    }

    fn found(file: &str, path: &str, page: Option<&str>, text: &str) -> Found {
        Found {
            file: file.into(),
            path: path.into(),
            page: page.map(str::to_string),
            text: text.into(),
        }
    }

    #[test]
    fn a_brief_cites_its_sources() {
        let brief = compose_brief(
            "e1",
            "HNSW tuning with Asha",
            "",
            &[
                conversation("c1", "How should HNSW ef be tuned?", "2026-11-10T09:00:00Z"),
                conversation("c2", "Lunch places", "2026-11-12T09:00:00Z"),
                conversation("c3", "Asha's recall numbers", "2026-11-12T10:00:00Z"),
            ],
            &[
                found(
                    "hnsw.pdf",
                    "/papers/hnsw.pdf",
                    Some("4"),
                    "HNSW builds a layered graph; ef trades recall for speed.",
                ),
                found(
                    "Reading group",
                    "calendar://event/e1",
                    None,
                    "HNSW tuning with Asha",
                ),
                found("Asha", "note://n7", None, "Asha wants ef=200 for the eval."),
                found(
                    "unrelated.pdf",
                    "/papers/unrelated.pdf",
                    Some("2"),
                    "Bloom filters in storage engines.",
                ),
                found("hnsw.pdf", "/papers/hnsw.pdf", Some("5"), "HNSW again."),
            ],
        )
        .expect("a brief");
        // The latest matching conversation first; an unrelated one is not listed.
        assert_eq!(
            brief
                .conversations
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["c3", "c1"]
        );
        // Sources numbered from 1, each with where it is: the event itself and passages
        // that do not mention it are left out, and a file is cited once.
        assert_eq!(
            brief
                .sources
                .iter()
                .map(|s| (s.n, s.path.as_str(), s.page.as_deref(), s.kind))
                .collect::<Vec<_>>(),
            [
                (1, "/papers/hnsw.pdf", Some("4"), "document"),
                (2, "note://n7", None, "note")
            ]
        );
    }

    #[test]
    fn no_brief_for_an_event_with_no_matches() {
        let conversations = [conversation("c2", "Lunch places", "2026-11-12T09:00:00Z")];
        let passages = [found(
            "unrelated.pdf",
            "/papers/unrelated.pdf",
            Some("2"),
            "Bloom filters.",
        )];
        assert_eq!(
            compose_brief("e9", "Dentist", "", &conversations, &passages),
            None
        );
        // A title of only common words matches nothing.
        assert_eq!(
            compose_brief(
                "e9",
                "Weekly sync",
                "",
                &conversations,
                &[found("x.md", "/x.md", None, "weekly sync notes")]
            ),
            None
        );
    }

    #[test]
    fn significant_words_ignore_case_short_and_common_words() {
        assert_eq!(
            significant_words("Meet ASHA re: HNSW, the HNSW eval"),
            ["asha", "hnsw", "eval"]
        );
    }
}
