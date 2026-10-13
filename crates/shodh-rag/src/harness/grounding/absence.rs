//! Claims that the user has none of their own data ("You have no tasks",
//! "Nothing is indexed") must come from a tool that read that data in the
//! same answer.
//!
//! Such a claim cites no passage, so the passage checks never see it, and a
//! model that answers from the conversation so far, or guesses, states it
//! with confidence: in a long Research chat Nemotron told the user "you have
//! no tasks" without calling `list_tasks` while the user had eight. Here a
//! claim of absence about one of the user's kinds of data ([`DOMAINS`]) is
//! flagged as [`ClaimOutcome::UnverifiedAbsence`] unless a tool that reads
//! that kind of data succeeded in the run, and the flag is repaired like
//! other unsupported claims ([`super::followup`]): the model is asked to call
//! the tool and answer from its result.
//!
//! Absence of something in the documents ("the documents do not mention
//! X") is a statement about what was searched, not about the user's data,
//! and is not matched here.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use super::claims::{inherit_citations, split_claims};
use super::verify::AnswerMessage;
use crate::harness::events::{ClaimCheck, ClaimOutcome};

/// One kind of the user's own data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Domain {
    /// What it is called in a repair request.
    pub name: &'static str,
    /// Words an answer uses for it (lowercase, singular and plural).
    nouns: &'static [&'static str],
    /// Tools whose successful result in the run backs a claim that there is
    /// none of it.
    pub tools: &'static [&'static str],
}

/// The user's kinds of data and the tools that read them. Tool names are
/// those of the assistant profile (a test keeps them registered names).
pub const DOMAINS: &[Domain] = &[
    Domain {
        name: "tasks",
        nouns: &["task", "tasks", "to-do", "to-dos", "todo", "todos"],
        tools: &["list_tasks", "show_calendar"],
    },
    Domain {
        name: "events",
        nouns: &[
            "event",
            "events",
            "meeting",
            "meetings",
            "appointment",
            "appointments",
        ],
        tools: &["list_events", "show_calendar"],
    },
    Domain {
        name: "indexed documents",
        nouns: &[
            "document",
            "documents",
            "file",
            "files",
            "source",
            "sources",
            "folder",
            "folders",
            "paper",
            "papers",
        ],
        tools: &["list_sources", "search_documents", "find_papers"],
    },
    Domain {
        name: "memories",
        nouns: &["memory", "memories"],
        tools: &["recall"],
    },
    Domain {
        name: "snippets",
        nouns: &["snippet", "snippets"],
        tools: &["list_snippets"],
    },
    Domain {
        name: "conversations",
        nouns: &["conversation", "conversations", "chat", "chats"],
        tools: &["search_conversations"],
    },
    Domain {
        name: "workspaces",
        nouns: &["workspace", "workspaces"],
        tools: &["list_workspaces"],
    },
];

/// Up to three words (adjectives, "upcoming", "open", "saved") between the
/// negation and the noun.
const GAP: &str = r"(?:[a-z][a-z'’-]*\s+){0,3}";

/// "You have no (open) tasks", "you don't have any events", "there are no
/// tasks", "I found no files", "No tasks are due", "your task list is
/// empty", "nothing is indexed": the noun is captured.
fn absence_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let nouns: Vec<String> = DOMAINS
            .iter()
            .flat_map(|d| d.nouns.iter().copied())
            .map(regex::escape)
            .collect();
        let noun = format!(r"(?P<noun>{})\b", nouns.join("|"));
        [
            format!(
                r"\byou(?:\s+currently|\s+still)?\s+(?:have|has|['’]ve\s+got|have\s+got)\s+(?:no|zero)\s+{GAP}{noun}"
            ),
            format!(
                r"\byou(?:\s+currently)?\s+(?:do\s+not|don['’]t|dont)\s+(?:currently\s+)?have\s+any\s+{GAP}{noun}"
            ),
            format!(
                r"\bthere\s+(?:are|is|were|was)\s+(?:currently\s+)?(?:no|not\s+any)\s+{GAP}{noun}"
            ),
            format!(r"\b(?:found|see|could\s+find|can\s+see)\s+no\s+{GAP}{noun}"),
            format!(
                r"(?:^|[.:;!]\s+)no\s+{GAP}{noun}\s+(?:(?:are|is|were|was|have\s+been)\s+)?(?:found|indexed|scheduled|due|saved|listed|yet|available)"
            ),
            r"\byour\s+(?P<noun>task|to-do|todo|calendar|memory|snippet|workspace)\s+(?:list\s+)?(?:is|looks)\s+(?:empty|clear)".to_string(),
            r"\bnothing\s+(?:is\s+|has\s+been\s+)?(?P<noun>indexed)\b".to_string(),
            r"\byou\s+have\s+nothing\s+(?P<noun>scheduled|due|planned)\b".to_string(),
        ]
        .iter()
        .filter_map(|p| Regex::new(&format!("(?i){p}")).ok())
        .collect()
    })
}

/// The domain a captured noun belongs to.
fn domain_of_noun(noun: &str) -> Option<&'static Domain> {
    let noun = noun.to_lowercase();
    let by_name = |name: &str| DOMAINS.iter().find(|d| d.name == name);
    match noun.as_str() {
        "indexed" => by_name("indexed documents"),
        "calendar" | "scheduled" | "planned" => by_name("events"),
        "due" => by_name("tasks"),
        _ => DOMAINS.iter().find(|d| d.nouns.contains(&noun.as_str())),
    }
}

/// The kind of the user's data `claim` says the user has none of, if any.
pub fn absence_domain(claim: &str) -> Option<&'static Domain> {
    absence_patterns().iter().find_map(|re| {
        re.captures_iter(claim)
            .filter_map(|c| c.name("noun"))
            .find_map(|noun| domain_of_noun(noun.as_str()))
    })
}

/// Flag the claims of `messages` that say the user has none of some data
/// no successful tool call of the run read (`tools_ok`). A claim that was
/// already checked keeps its place and gets the new outcome; an unchecked
/// one (absence claims cite nothing) is added in reading order.
pub fn flag_unverified_absence(
    messages: &[AnswerMessage],
    tools_ok: &HashSet<String>,
    checks: Vec<ClaimCheck>,
) -> Vec<ClaimCheck> {
    let mut remaining = checks;
    let mut out = Vec::with_capacity(remaining.len());
    for message in messages {
        let mut claims = split_claims(&message.text);
        inherit_citations(&message.text, &mut claims);
        for claim in claims {
            let existing = remaining
                .iter()
                .position(|c| c.message_id == message.id && c.anchor == claim.anchor)
                .map(|i| remaining.remove(i));
            let unbacked = absence_domain(&claim.text)
                .filter(|d| !d.tools.iter().any(|t| tools_ok.contains(*t)));
            match (existing, unbacked) {
                (Some(mut check), Some(_)) => {
                    check.outcome = ClaimOutcome::UnverifiedAbsence;
                    out.push(check);
                }
                (Some(check), None) => out.push(check),
                (None, Some(_)) => out.push(ClaimCheck {
                    message_id: message.id.clone(),
                    text: claim.text.clone(),
                    anchor: claim.anchor.clone(),
                    kind: claim.kind,
                    outcome: ClaimOutcome::UnverifiedAbsence,
                    cited: claim.citations.clone(),
                    invalid: Vec::new(),
                    support: None,
                    contradiction: None,
                    missing_numbers: Vec::new(),
                    closest: None,
                    closest_score: None,
                }),
                (None, None) => {}
            }
        }
    }
    out.extend(remaining);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::profile::AgentProfile;

    fn domain(claim: &str) -> Option<&'static str> {
        absence_domain(claim).map(|d| d.name)
    }

    #[test]
    fn claims_that_the_user_has_none_of_their_data_are_recognised() {
        for (claim, expected) in [
            ("You have no tasks.", "tasks"),
            ("You currently have no open tasks.", "tasks"),
            ("You don't have any upcoming meetings this week.", "events"),
            ("You don’t have any tasks yet.", "tasks"),
            ("You do not have any tasks due today.", "tasks"),
            ("There are no tasks in your list.", "tasks"),
            ("I found no events scheduled for Friday.", "events"),
            ("No tasks are due this week.", "tasks"),
            ("Your task list is empty.", "tasks"),
            ("You have nothing scheduled tomorrow.", "events"),
            ("Nothing is indexed yet.", "indexed documents"),
            ("You have no indexed documents.", "indexed documents"),
            ("There are no saved snippets.", "snippets"),
            ("You have no memories stored about that.", "memories"),
            ("You have no workspaces.", "workspaces"),
        ] {
            assert_eq!(domain(claim), Some(expected), "{claim}");
        }
    }

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(absence_patterns().len(), 8);
    }

    #[test]
    fn statements_about_document_content_are_not_absence_of_user_data() {
        for claim in [
            "The documents do not mention a notice period.",
            "The contract does not say who pays for repairs.",
            "There is no information about the renewal fee in the passages.",
            "HNSW has no global index; it uses a layered graph [2].",
            "No relevant passages were found for the 2019 figures.",
            "You have to give sixty days of notice [1].",
            "The task of the encoder is to map queries to vectors [3].",
        ] {
            assert_eq!(domain(claim), None, "{claim}");
        }
    }

    fn answer(text: &str) -> Vec<AnswerMessage> {
        vec![AnswerMessage {
            id: "m1".into(),
            text: text.into(),
        }]
    }

    #[test]
    fn an_absence_claim_needs_a_tool_result_of_the_same_answer() {
        let messages = answer("You have no tasks right now. Ask me to add one any time.");
        let none = HashSet::new();
        let flagged = flag_unverified_absence(&messages, &none, Vec::new());
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].outcome, ClaimOutcome::UnverifiedAbsence);
        assert_eq!(flagged[0].text, "You have no tasks right now.");
        // Another kind of data read in the run does not back it.
        let other: HashSet<String> = ["list_sources".to_string()].into();
        assert_eq!(
            flag_unverified_absence(&messages, &other, Vec::new()).len(),
            1
        );
        // list_tasks (or the calendar view) read the tasks: it stands.
        for tool in ["list_tasks", "show_calendar"] {
            let read: HashSet<String> = [tool.to_string()].into();
            assert!(flag_unverified_absence(&messages, &read, Vec::new()).is_empty());
        }
    }

    #[test]
    fn checked_claims_keep_their_order_and_absence_takes_their_place() {
        let messages =
            answer("The lease runs to 2027 [1]. You have no events about it. Rent is 900 EUR [2].");
        let check = |anchor: &str, outcome| ClaimCheck {
            message_id: "m1".into(),
            text: anchor.into(),
            anchor: anchor.into(),
            kind: crate::harness::events::ClaimKind::Sentence,
            outcome,
            cited: Vec::new(),
            invalid: Vec::new(),
            support: None,
            contradiction: None,
            missing_numbers: Vec::new(),
            closest: None,
            closest_score: None,
        };
        let claims = split_claims(&messages[0].text);
        let anchors: Vec<String> = claims.iter().map(|c| c.anchor.clone()).collect();
        assert_eq!(anchors.len(), 3, "{anchors:?}");
        let checks = vec![
            check(&anchors[0], ClaimOutcome::Supported),
            check(&anchors[2], ClaimOutcome::Weak),
        ];
        let out = flag_unverified_absence(&messages, &HashSet::new(), checks);
        let outcomes: Vec<ClaimOutcome> = out.iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                ClaimOutcome::Supported,
                ClaimOutcome::UnverifiedAbsence,
                ClaimOutcome::Weak
            ]
        );
    }

    #[test]
    fn the_backing_tools_are_tools_of_the_assistant_profile() {
        let profile = AgentProfile::assistant();
        for d in DOMAINS {
            for tool in d.tools {
                assert!(profile.allows(tool), "{} backs {}", tool, d.name);
            }
        }
    }
}
