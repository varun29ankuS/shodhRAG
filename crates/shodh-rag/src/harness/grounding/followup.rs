//! What happens after the model finishes an answer: finish the run, or ask
//! for one more turn.
//!
//! Three reasons for a follow-up turn, each bounded:
//! * **repair** (setting, default on): claims that are wrong on evidence
//!   the check can stand behind deterministically are sent back once, with
//!   the closest passage, to be re-grounded or removed ([`needs_repair`]): a
//!   citation number no source has, or a number the cited text does not
//!   contain, or a claim that the user has none of their own data that no
//!   tool of the answer read ([`super::absence`]). A miscitation is sent
//!   back too: a claim its cited passages do not support that another
//!   passage of the answer supports by the same check (`suggested`, see
//!   [`super::verify`]); the verdict there is not the model's alone but a
//!   passage that passes it, so the request is concrete ("cite [2], not
//!   [3]"). Other claims the models did not find support for, and uncited
//!   statements, are flagged in the answer but not sent back. On the user's
//!   11 stored sourced answers every answer had some of those, and reading
//!   them showed the models' verdicts unreliable on that text (notation,
//!   invoices, quotes from beyond a passage's length limit; a high
//!   "contradiction" on a sentence the passage states almost word for word),
//!   so a rewrite for them would have doubled the cost of every answer
//!   without a verdict to stand behind. Never more than [`MAX_REPAIR_ROUNDS`].
//! * **coverage**: information needs (task-list items marked as needs) that
//!   no retrieved passage covers get targeted searches. At most
//!   [`MAX_COVERAGE_ROUNDS`] extra rounds, and only while the answer's tool
//!   budget has calls left.
//! * **retrieve** (same setting as repair): an answer that cites passages,
//!   or refuses for lack of documents, although it had no sources of its own
//!   ([`Retrieval::sourced`]: no search, [`RETRIEVAL_TOOLS`], succeeded and
//!   no tool handed it passages). The system prompt asks the model to
//!   search first, but nothing made it: a model that skipped the search
//!   cited `[1][2]` that no source had, and the repair turn that asked it to
//!   use "only passage numbers you were given" (none) turned the answer
//!   into a refusal. Instead the session searches the documents for the
//!   user's question itself, through the registry under the answer's scope,
//!   and asks for the answer again from what it found ([`retrieve_prompt`]).
//!   At most [`MAX_RETRIEVE_ROUNDS`], with its own count, so a repair round
//!   is still left for the answer written from the retrieved passages.
//!
//! Repair and coverage can share one turn; a retrieve turn is on its own
//! (the answer it replaces had no passages to repair or cover against). The
//! decision is a pure function of the checks, the rounds so far, the
//! remaining budget and whether the answer had sources of its own.

use std::collections::HashSet;

use super::absence::{absence_domain, is_document_refusal};
use crate::harness::events::{ClaimCheck, ClaimOutcome, CoverageState, NeedCheck, RevisionReason};
use crate::harness::tools::search::SEARCH_DOCUMENTS;
use crate::harness::truncate_chars;

/// Re-grounding turns per answer.
pub const MAX_REPAIR_ROUNDS: u32 = 1;
/// Extra retrieval rounds for uncovered needs per answer.
pub const MAX_COVERAGE_ROUNDS: u32 = 2;
/// Searches of the documents the session makes for an answer that skipped
/// them.
pub const MAX_RETRIEVE_ROUNDS: u32 = 1;
/// Flagged claims listed in one repair request.
const MAX_LISTED_CLAIMS: usize = 12;

/// Tools whose success in a run means the answer searched the user's
/// documents or papers: the user's indexed documents, the paper library and
/// the public paper search.
pub const RETRIEVAL_TOOLS: [&str; 3] = [
    SEARCH_DOCUMENTS,
    crate::harness::profile::app_tools::FIND_PAPERS,
    crate::harness::tools::web::SEARCH_PAPERS,
];

/// Whether a retrieval tool ([`RETRIEVAL_TOOLS`]) succeeded in the run.
/// Keyed on tools, not on passages: a side discussion starts with the
/// passages of the answer it is about, without having searched anything.
pub fn searched(tools_ok: &HashSet<String>) -> bool {
    RETRIEVAL_TOOLS.iter().any(|t| tools_ok.contains(*t))
}

/// Follow-up turns used so far in a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rounds {
    pub repair: u32,
    pub coverage: u32,
    pub retrieve: u32,
}

/// What the answer's run knows about searching the user's documents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retrieval {
    /// The answer had sources of its own: a retrieval tool succeeded in the
    /// run ([`searched`], which also counts a search that found nothing), or
    /// a tool of the run handed out passages (a web search, a fetched page,
    /// an opened document, a paper or figure tool). Passages carried into a
    /// side discussion do not count: they are what the run started with.
    pub sourced: bool,
    /// `search_documents` is registered and the profile allows it, so the
    /// session can search for the question itself (and the model can be
    /// told to). False in Code mode.
    pub available: bool,
}

/// The decision after a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Finish,
    /// Search the user's documents for the question, then ask for the
    /// answer again from the passages found ([`retrieve_prompt`]).
    Retrieve {
        /// Indexes into the checks of the claims that had no source.
        flagged: Vec<usize>,
    },
    FollowUp {
        reason: RevisionReason,
        /// Indexes into the checks of the claims to repair.
        repair: Vec<usize>,
        /// Ids of the needs still missing.
        missing_needs: Vec<String>,
        /// The message sent to the model.
        prompt: String,
    },
}

/// Whether a checked claim is sent back for repair: a citation number no
/// source has, a number the cited text does not contain, a claim that cites
/// the wrong passage while another one supports it, or a claim that the user
/// has none of some data no tool of the answer read.
pub fn needs_repair(check: &ClaimCheck) -> bool {
    match check.outcome {
        ClaimOutcome::InvalidCitation | ClaimOutcome::UnverifiedAbsence => true,
        ClaimOutcome::Unsupported => !check.missing_numbers.is_empty() || check.suggested.is_some(),
        ClaimOutcome::Supported
        | ClaimOutcome::Weak
        | ClaimOutcome::UncitedFactual
        | ClaimOutcome::Unchecked => false,
    }
}

fn reason_text(check: &ClaimCheck) -> String {
    match check.outcome {
        ClaimOutcome::InvalidCitation => {
            let numbers: Vec<String> = check.invalid.iter().map(|n| format!("[{n}]")).collect();
            format!(
                "cites {}, which no source of this answer has",
                numbers.join("")
            )
        }
        ClaimOutcome::UncitedFactual => "states a fact without citing a source".to_string(),
        ClaimOutcome::UnverifiedAbsence => match absence_domain(&check.text) {
            Some(domain) => format!(
                "says the user has no {} without reading them in this answer: call {} now and \
                 answer from its result",
                domain.name,
                domain.tools.join(" or ")
            ),
            None => "says the user has none of something without checking with a tool in this \
                     answer: call the tool that lists it and answer from its result"
                .to_string(),
        },
        _ if !check.missing_numbers.is_empty() => format!(
            "the cited source does not contain {}",
            check.missing_numbers.join(", ")
        ),
        _ => match check.suggested {
            Some(n) => {
                let cited: String = check.cited.iter().map(|c| format!("[{c}]")).collect();
                format!("{cited} does not state it but passage [{n}] does: cite [{n}], not {cited}")
            }
            None => "the cited source does not support it".to_string(),
        },
    }
}

/// Whether a checked claim shows that the answer took its content from
/// somewhere other than a search: it cites a number no source has, or it
/// refuses in its own words to answer from documents it did not search
/// ([`is_document_refusal`]). Uncited statements do not count: without
/// passages they are not checked at all, and next to passages (web results,
/// a side discussion's carried passages) they are flagged only, as for
/// repair.
fn without_sources(check: &ClaimCheck) -> bool {
    match check.outcome {
        ClaimOutcome::InvalidCitation => true,
        // A refusal about what the documents contain; "nothing is indexed"
        // is repaired with list_sources instead.
        ClaimOutcome::UnverifiedAbsence => is_document_refusal(&check.text),
        ClaimOutcome::Supported
        | ClaimOutcome::Weak
        | ClaimOutcome::Unsupported
        | ClaimOutcome::UncitedFactual
        | ClaimOutcome::Unchecked => false,
    }
}

/// Decide what follows a check.
///
/// `auto_repair` is the user's setting; `calls_left` is the answer's
/// remaining tool budget (zero when follow-up turns are off); `retrieval`
/// says whether the answer had sources of its own and whether the session
/// can search the documents.
pub fn decide(
    checks: &[ClaimCheck],
    needs: &[NeedCheck],
    rounds: Rounds,
    auto_repair: bool,
    calls_left: u32,
    retrieval: Retrieval,
) -> Next {
    // A follow-up turn like a repair, so under the same setting.
    if auto_repair
        && retrieval.available
        && !retrieval.sourced
        && rounds.retrieve < MAX_RETRIEVE_ROUNDS
        && calls_left > 0
    {
        let flagged: Vec<usize> = checks
            .iter()
            .enumerate()
            .filter(|(_, c)| without_sources(c))
            .map(|(i, _)| i)
            .collect();
        if !flagged.is_empty() {
            return Next::Retrieve { flagged };
        }
    }
    let repair: Vec<usize> = if auto_repair && rounds.repair < MAX_REPAIR_ROUNDS {
        checks
            .iter()
            .enumerate()
            .filter(|(_, c)| needs_repair(c))
            .map(|(i, _)| i)
            .collect()
    } else {
        Vec::new()
    };
    let missing: Vec<&NeedCheck> = if rounds.coverage < MAX_COVERAGE_ROUNDS && calls_left > 0 {
        needs
            .iter()
            .filter(|n| n.state == CoverageState::Missing)
            .collect()
    } else {
        Vec::new()
    };
    let reason = match (repair.is_empty(), missing.is_empty()) {
        (true, true) => return Next::Finish,
        (false, true) => RevisionReason::Repair,
        (true, false) => RevisionReason::Coverage,
        (false, false) => RevisionReason::RepairAndCoverage,
    };

    let mut prompt = String::from(
        "[Grounding check by the app, not the user] Your answer was checked against the passages \
         it cites.",
    );
    if !missing.is_empty() {
        prompt.push_str("\n\nNo retrieved passage covers these parts of the question:\n");
        for need in &missing {
            prompt.push_str(&format!("- {}\n", truncate_chars(&need.text, 200)));
        }
        prompt.push_str(&format!(
            "Search for each of them with a specific query (you have {calls_left} tool calls left \
             for this answer). If a part still cannot be found, say plainly that the documents do \
             not cover it."
        ));
    }
    if !repair.is_empty() {
        prompt.push_str("\n\nThese statements are not grounded in their sources:\n");
        for (listed, &i) in repair.iter().enumerate() {
            if listed == MAX_LISTED_CLAIMS {
                prompt.push_str(&format!(
                    "- and {} more\n",
                    repair.len() - MAX_LISTED_CLAIMS
                ));
                break;
            }
            let check = &checks[i];
            // A suggested passage is already named in the reason.
            let closest = check
                .closest
                .filter(|_| check.suggested.is_none())
                .map(|n| format!(" Passage [{n}] comes closest."))
                .unwrap_or_default();
            prompt.push_str(&format!(
                "- \"{}\": {}.{closest}\n",
                truncate_chars(&check.text, 300),
                reason_text(check)
            ));
        }
        // Citations of an answer that had no sources point to no passage:
        // "use only the numbers you were given" (none) would leave removing
        // every statement, a refusal, as the only compliant answer.
        let unretrieved = retrieval.available
            && !retrieval.sourced
            && repair
                .iter()
                .any(|&i| checks[i].outcome == ClaimOutcome::InvalidCitation);
        if unretrieved && calls_left > 0 {
            prompt.push_str(&format!(
                "No search or other tool gave this answer passages, so no passage backs \
                 these citations yet. Call {SEARCH_DOCUMENTS} now with the user's \
                 question, then cite for each statement the passage of its result that \
                 supports it, or correct the statement to what the passages say; cite only \
                 the passage numbers the search returns. If it finds nothing, say plainly \
                 that the documents do not cover the question."
            ));
        } else if unretrieved {
            prompt.push_str(
                "No passage was retrieved in this answer and no tool calls are left, so these \
                 citation numbers point to nothing: drop them, and say plainly that these \
                 statements were not checked against the user's documents.",
            );
        } else {
            prompt.push_str(
                "For each one: cite the passage that actually supports it, correct it to what \
                 the passages say, or remove it. Use only passage numbers you were given.",
            );
        }
        if repair
            .iter()
            .any(|&i| checks[i].outcome == ClaimOutcome::UnverifiedAbsence)
        {
            prompt.push_str(
                " A statement that the user has none of their own data stands only on the \
                 result of the tool named for it, called in this answer.",
            );
        }
    }
    prompt.push_str(
        "\n\nThen write your complete revised answer from the beginning, with citations. It \
         replaces the previous one, so do not refer to it or to this check. Start with the \
         answer's first sentence: no apology, no acknowledgement, no preface.",
    );
    Next::FollowUp {
        reason,
        repair,
        missing_needs: missing.iter().map(|n| n.id.clone()).collect(),
        prompt,
    }
}

/// What the session's own search for the question produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrieveResult {
    /// Passages were found; `text` is the search result as the tool returns
    /// it to the model (numbered passages, marked as data).
    Found { text: String },
    /// The search ran and matched nothing.
    Empty,
    /// The search could not run; `error` says why.
    Failed { error: String },
}

/// The follow-up prompt after the session searched the documents for an
/// answer that skipped the search. `earlier` is true when the run already
/// had passages before the search (a side discussion's, or the web's): they
/// keep their numbers and may still be cited. `limited_to` names what the
/// search covered when the answer is limited to some sources (a workspace,
/// files the user chose), so that "not covered" is said of those sources and
/// never of the whole library; `None` when everything indexed was searched.
pub fn retrieve_prompt(result: &RetrieveResult, earlier: bool, limited_to: Option<&str>) -> String {
    let mut prompt = String::from(
        "[Grounding check by the app, not the user] Your answer cites passages, or says what \
         the user's documents contain, but no search of the user's documents ran in this \
         answer.",
    );
    let allowed = if earlier {
        "these passages and the passages you were given earlier in this answer"
    } else {
        "these passages"
    };
    let searched = limited_to.unwrap_or("the user's documents");
    match result {
        RetrieveResult::Found { text } => {
            let uncovered = limited_to.unwrap_or("the documents");
            prompt.push_str(&format!(
                " The app searched {searched} for the question. The passages it found, under \
                 the numbers to cite them with:\n\n{text}\n\nWrite the complete answer to the \
                 question using only {allowed}. Cite every statement with the number of the \
                 passage that supports it, and use no other numbers. Where they do not cover a \
                 part of the question, say plainly that {uncovered} do not cover that part \
                 instead of answering it from elsewhere."
            ));
        }
        // The earlier passages still stand: the answer is kept on them.
        RetrieveResult::Empty if earlier => {
            prompt.push_str(&format!(
                " The app searched {searched} for the question and no new passage matched. \
                 Write the answer only from the passages you were given earlier in this \
                 answer, citing only their numbers, and say plainly which part of the \
                 question they do not cover."
            ));
        }
        RetrieveResult::Empty => {
            let uncovered = match limited_to {
                Some(limit) => format!(
                    "{limit} do not cover the question (the rest of their library was not \
                     searched)"
                ),
                None => "their documents do not cover the question".to_string(),
            };
            prompt.push_str(&format!(
                " The app searched {searched} for the question and no passage matched. Tell \
                 the user plainly that {uncovered}, and cite no passage numbers. If you add \
                 anything from general knowledge, say clearly that it is not from their \
                 documents."
            ));
        }
        RetrieveResult::Failed { error } => {
            let otherwise = if earlier {
                "answer from its result and the passages you were given earlier in this \
                 answer, citing only their numbers. If it finds nothing, answer from the \
                 earlier passages alone and say plainly which part of the question they do \
                 not cover."
            } else {
                "answer only from its result, citing only the passage numbers it returns. If \
                 it finds nothing, say plainly that the documents do not cover the question."
            };
            prompt.push_str(&format!(
                " The app tried to search {searched} for the question, but the search failed \
                 ({}). Call {SEARCH_DOCUMENTS} now with the user's question and {otherwise}",
                truncate_chars(error, 300)
            ));
        }
    }
    prompt.push_str(
        "\n\nWrite the answer from the beginning. It replaces the previous one, so do not \
         refer to it or to this check. Start with the answer's first sentence: no apology, no \
         acknowledgement, no preface.",
    );
    prompt
}

/// Whether a follow-up turn's text replaces the answer before it: it must be
/// an answer in its own right (at least one checked claim, or at least half
/// the length of the previous text). A reply such as "I could not find
/// more." keeps the earlier answer as the answer.
pub fn replaces_previous(previous_chars: usize, new_chars: usize, new_checked: usize) -> bool {
    new_chars > 0 && (new_checked > 0 || new_chars * 2 >= previous_chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::events::ClaimKind;

    /// A run where the session cannot search the documents (the decisions
    /// before retrieval existed).
    const NO_SEARCH: Retrieval = Retrieval {
        sourced: false,
        available: false,
    };

    fn check(outcome: ClaimOutcome) -> ClaimCheck {
        ClaimCheck {
            message_id: "m1".into(),
            text: "The fee is 900 EUR.".into(),
            anchor: "The fee is 900 EUR [2].".into(),
            kind: ClaimKind::Sentence,
            outcome,
            cited: vec![2],
            invalid: if outcome == ClaimOutcome::InvalidCitation {
                vec![9]
            } else {
                vec![]
            },
            support: Some(0.2),
            contradiction: None,
            missing_numbers: if outcome == ClaimOutcome::Unsupported {
                vec!["900".into()]
            } else {
                vec![]
            },
            closest: Some(4),
            closest_score: Some(0.7),
            suggested: None,
        }
    }

    #[test]
    fn an_unverified_absence_is_sent_back_with_the_tool_that_reads_the_data() {
        let mut absent = check(ClaimOutcome::UnverifiedAbsence);
        absent.text = "You have no tasks.".into();
        absent.cited = vec![];
        absent.missing_numbers = vec![];
        absent.closest = None;
        assert!(needs_repair(&absent));
        let Next::FollowUp {
            reason,
            repair,
            prompt,
            ..
        } = decide(&[absent], &[], Rounds::default(), true, 0, NO_SEARCH)
        else {
            panic!("an unverified absence was not repaired");
        };
        assert_eq!(reason, RevisionReason::Repair);
        assert_eq!(repair, vec![0]);
        assert!(
            prompt.contains("says the user has no tasks without reading them"),
            "{prompt}"
        );
        assert!(
            prompt.contains("call list_tasks or show_calendar now"),
            "{prompt}"
        );
        assert!(
            prompt.contains("stands only on the result of the tool"),
            "{prompt}"
        );
    }

    fn need(id: &str, state: CoverageState) -> NeedCheck {
        NeedCheck {
            id: id.into(),
            text: format!("need {id}"),
            state,
            passages: vec![],
        }
    }

    #[test]
    fn only_claims_the_check_can_stand_behind_are_sent_back() {
        let mut not_found = check(ClaimOutcome::Unsupported);
        not_found.missing_numbers.clear();
        not_found.contradiction = Some(0.99);
        assert!(
            !needs_repair(&not_found),
            "a model verdict alone is flagged, not repaired"
        );
        assert!(
            needs_repair(&check(ClaimOutcome::Unsupported)),
            "a missing number"
        );
        assert!(needs_repair(&check(ClaimOutcome::InvalidCitation)));
        assert!(
            !needs_repair(&check(ClaimOutcome::UncitedFactual)),
            "uncited is flagged, not rewritten"
        );
        assert!(!needs_repair(&check(ClaimOutcome::Weak)));
        let all_uncited: Vec<ClaimCheck> = (0..10)
            .map(|_| check(ClaimOutcome::UncitedFactual))
            .collect();
        assert_eq!(
            decide(&all_uncited, &[], Rounds::default(), true, 10, NO_SEARCH),
            Next::Finish
        );
    }

    #[test]
    fn a_miscitation_is_repaired_with_the_passage_that_supports_it() {
        let mut miscited = check(ClaimOutcome::Unsupported);
        miscited.missing_numbers.clear();
        miscited.cited = vec![3];
        miscited.anchor = "The fee is 900 EUR [3].".into();
        miscited.closest = Some(2);
        miscited.suggested = Some(2);
        assert!(needs_repair(&miscited));
        match decide(
            std::slice::from_ref(&miscited),
            &[],
            Rounds::default(),
            true,
            10,
            SEARCHED,
        ) {
            Next::FollowUp {
                reason,
                repair,
                prompt,
                ..
            } => {
                assert_eq!(reason, RevisionReason::Repair);
                assert_eq!(repair, vec![0]);
                assert!(
                    prompt
                        .contains("[3] does not state it but passage [2] does: cite [2], not [3]"),
                    "{prompt}"
                );
                assert!(!prompt.contains("comes closest"), "{prompt}");
            }
            other => panic!("expected a repair turn, got {other:?}"),
        }
        // A side thread's session has follow-ups off, so the check passes
        // auto-repair off and no budget: the miscitation is flagged only.
        assert_eq!(
            decide(&[miscited], &[], Rounds::default(), false, 0, SEARCHED),
            Next::Finish
        );
    }

    #[test]
    fn a_clean_answer_finishes() {
        let checks = vec![check(ClaimOutcome::Supported), check(ClaimOutcome::Weak)];
        let needs = vec![need("1", CoverageState::Covered)];
        assert_eq!(
            decide(&checks, &needs, Rounds::default(), true, 10, NO_SEARCH),
            Next::Finish
        );
    }

    #[test]
    fn flagged_claims_are_repaired_once() {
        let checks = vec![
            check(ClaimOutcome::Supported),
            check(ClaimOutcome::Unsupported),
            check(ClaimOutcome::InvalidCitation),
            check(ClaimOutcome::UncitedFactual),
        ];
        match decide(&checks, &[], Rounds::default(), true, 10, NO_SEARCH) {
            Next::FollowUp {
                reason,
                repair,
                prompt,
                ..
            } => {
                assert_eq!(reason, RevisionReason::Repair);
                assert_eq!(repair, vec![1, 2], "uncited statements are flagged only");
                assert!(prompt.contains("does not contain 900"));
                assert!(prompt.contains("cites [9]"));
                assert!(prompt.contains("Passage [4] comes closest"));
                assert!(prompt.contains("complete revised answer"));
                assert!(prompt.contains("no apology"));
            }
            other => panic!("expected a repair turn, got {other:?}"),
        }
        let after = Rounds {
            repair: 1,
            ..Rounds::default()
        };
        assert_eq!(
            decide(&checks, &[], after, true, 10, NO_SEARCH),
            Next::Finish,
            "never a second repair"
        );
        assert_eq!(
            decide(&checks, &[], Rounds::default(), false, 10, NO_SEARCH),
            Next::Finish,
            "setting off"
        );
    }

    #[test]
    fn missing_needs_get_at_most_two_rounds_within_the_budget() {
        let needs = vec![
            need("1", CoverageState::Covered),
            need("2", CoverageState::Missing),
        ];
        for coverage in 0..MAX_COVERAGE_ROUNDS {
            match decide(
                &[],
                &needs,
                Rounds {
                    coverage,
                    ..Rounds::default()
                },
                true,
                5,
                NO_SEARCH,
            ) {
                Next::FollowUp {
                    reason,
                    missing_needs,
                    prompt,
                    ..
                } => {
                    assert_eq!(reason, RevisionReason::Coverage);
                    assert_eq!(missing_needs, vec!["2"]);
                    assert!(prompt.contains("need 2"));
                    assert!(prompt.contains("5 tool calls left"));
                }
                other => panic!("round {coverage} should search again, got {other:?}"),
            }
        }
        assert_eq!(
            decide(
                &[],
                &needs,
                Rounds {
                    coverage: MAX_COVERAGE_ROUNDS,
                    ..Rounds::default()
                },
                true,
                5,
                NO_SEARCH,
            ),
            Next::Finish
        );
        assert_eq!(
            decide(&[], &needs, Rounds::default(), true, 0, NO_SEARCH),
            Next::Finish,
            "no budget left"
        );
    }

    #[test]
    fn repair_and_coverage_share_a_turn() {
        let checks = vec![check(ClaimOutcome::Unsupported)];
        let needs = vec![need("2", CoverageState::Missing)];
        assert!(matches!(
            decide(&checks, &needs, Rounds::default(), true, 3, NO_SEARCH),
            Next::FollowUp {
                reason: RevisionReason::RepairAndCoverage,
                ..
            }
        ));
    }

    #[test]
    fn a_non_answer_does_not_replace_the_answer() {
        assert!(replaces_previous(1_000, 900, 4));
        assert!(replaces_previous(1_000, 600, 0));
        assert!(!replaces_previous(1_000, 40, 0));
        assert!(!replaces_previous(1_000, 0, 0));
    }

    const NOT_SEARCHED: Retrieval = Retrieval {
        sourced: false,
        available: true,
    };
    const SEARCHED: Retrieval = Retrieval {
        sourced: true,
        available: true,
    };

    #[test]
    fn only_a_search_tool_counts_as_searching() {
        let tools =
            |names: &[&str]| -> HashSet<String> { names.iter().map(|n| n.to_string()).collect() };
        assert!(!searched(&tools(&[])));
        assert!(!searched(&tools(&[
            "update_plan",
            "list_tasks",
            "web_search"
        ])));
        for tool in ["search_documents", "find_papers", "search_papers"] {
            assert!(searched(&tools(&[tool])), "{tool}");
        }
    }

    #[test]
    fn citations_without_a_search_get_the_documents_searched() {
        let checks = vec![
            check(ClaimOutcome::InvalidCitation),
            check(ClaimOutcome::InvalidCitation),
        ];
        assert_eq!(
            decide(&checks, &[], Rounds::default(), true, 10, NOT_SEARCHED),
            Next::Retrieve {
                flagged: vec![0, 1]
            }
        );
        // Automatic follow-ups off: no search and no turn, as for repair.
        assert_eq!(
            decide(&checks, &[], Rounds::default(), false, 10, NOT_SEARCHED),
            Next::Finish
        );
        // After a search the same citations are repaired as usual.
        match decide(&checks, &[], Rounds::default(), true, 10, SEARCHED) {
            Next::FollowUp { reason, prompt, .. } => {
                assert_eq!(reason, RevisionReason::Repair);
                assert!(
                    prompt.contains("Use only passage numbers you were given."),
                    "{prompt}"
                );
            }
            other => panic!("expected a repair turn, got {other:?}"),
        }
    }

    #[test]
    fn the_search_is_not_made_where_it_cannot_be() {
        let checks = vec![check(ClaimOutcome::InvalidCitation)];
        // Code mode, or no search_documents in the registry.
        assert!(matches!(
            decide(&checks, &[], Rounds::default(), true, 10, NO_SEARCH),
            Next::FollowUp {
                reason: RevisionReason::Repair,
                ..
            }
        ));
        // No tool calls left.
        match decide(&checks, &[], Rounds::default(), true, 0, NOT_SEARCHED) {
            Next::FollowUp { prompt, .. } => {
                assert!(
                    !prompt.contains("Use only passage numbers you were given"),
                    "{prompt}"
                );
                assert!(prompt.contains("point to nothing: drop them"), "{prompt}");
            }
            other => panic!("expected a repair turn, got {other:?}"),
        }
    }

    #[test]
    fn the_search_has_its_own_round_and_leaves_the_repair_round() {
        let checks = vec![check(ClaimOutcome::InvalidCitation)];
        let after_search = Rounds {
            retrieve: MAX_RETRIEVE_ROUNDS,
            ..Rounds::default()
        };
        // The answer written from the retrieved passages can still be repaired.
        assert!(matches!(
            decide(&checks, &[], after_search, true, 10, SEARCHED),
            Next::FollowUp {
                reason: RevisionReason::Repair,
                ..
            }
        ));
        // The session's search failed: never a second one, and the repair
        // asks the model to search instead of to use numbers it never got.
        match decide(&checks, &[], after_search, true, 10, NOT_SEARCHED) {
            Next::FollowUp { reason, prompt, .. } => {
                assert_eq!(reason, RevisionReason::Repair);
                assert!(
                    !prompt.contains("Use only passage numbers you were given"),
                    "{prompt}"
                );
                assert!(prompt.contains("Call search_documents now"), "{prompt}");
            }
            other => panic!("expected a repair turn, got {other:?}"),
        }
        let both_used = Rounds {
            repair: MAX_REPAIR_ROUNDS,
            retrieve: MAX_RETRIEVE_ROUNDS,
            ..Rounds::default()
        };
        assert_eq!(
            decide(&checks, &[], both_used, true, 10, SEARCHED),
            Next::Finish
        );
    }

    #[test]
    fn only_answers_from_unsearched_documents_trigger_the_search() {
        // Uncited statements next to passages are flagged, not re-answered.
        let uncited = vec![check(ClaimOutcome::UncitedFactual)];
        assert_eq!(
            decide(&uncited, &[], Rounds::default(), true, 10, NOT_SEARCHED),
            Next::Finish
        );
        // A tasks answer has its own repair, with list_tasks.
        // A tasks answer, or one saying nothing is indexed, has its own
        // repair, with the tool that lists that data.
        for text in ["You have no tasks.", "Nothing is indexed yet."] {
            let mut absent = check(ClaimOutcome::UnverifiedAbsence);
            absent.text = text.into();
            absent.cited = vec![];
            assert!(
                matches!(
                    decide(&[absent], &[], Rounds::default(), true, 10, NOT_SEARCHED),
                    Next::FollowUp {
                        reason: RevisionReason::Repair,
                        ..
                    }
                ),
                "{text}"
            );
        }
        // The refusal of the reported answer, made without a search.
        let mut no_documents = check(ClaimOutcome::UnverifiedAbsence);
        no_documents.text =
            "I don't have any documents indexed that contain information about SPANN.".into();
        no_documents.cited = vec![];
        assert_eq!(
            decide(
                &[no_documents],
                &[],
                Rounds::default(),
                true,
                10,
                NOT_SEARCHED
            ),
            Next::Retrieve { flagged: vec![0] }
        );
    }

    #[test]
    fn the_retrieve_prompt_carries_what_the_search_found() {
        let found = retrieve_prompt(
            &RetrieveResult::Found {
                text: "[{\"n\": 1, \"text\": \"FreshDiskANN keeps a small in-memory index.\"}]"
                    .into(),
            },
            false,
            None,
        );
        assert!(found.contains("FreshDiskANN keeps a small in-memory index."));
        assert!(found.contains("using only these passages."), "{found}");
        assert!(found.contains("say plainly that the documents do not cover that part"));
        assert!(found.contains("no apology"));
        assert!(!found.contains("Use only passage numbers you were given"));
        let side = retrieve_prompt(&RetrieveResult::Found { text: "[]".into() }, true, None);
        assert!(side.contains("passages you were given earlier in this answer"));

        let empty = retrieve_prompt(&RetrieveResult::Empty, false, None);
        assert!(empty.contains("no passage matched"), "{empty}");
        assert!(empty.contains("their documents do not cover the question"));
        assert!(empty.contains("cite no passage numbers"));

        let failed = retrieve_prompt(
            &RetrieveResult::Failed {
                error: "Search failed: index locked".into(),
            },
            false,
            None,
        );
        assert!(failed.contains("index locked"), "{failed}");
        assert!(failed.contains("Call search_documents now"));
    }

    #[test]
    fn with_earlier_passages_a_search_that_adds_nothing_keeps_them() {
        let empty = retrieve_prompt(&RetrieveResult::Empty, true, None);
        assert!(empty.contains("no new passage matched"), "{empty}");
        assert!(empty.contains("only from the passages you were given earlier"));
        assert!(empty.contains("citing only their numbers"));
        assert!(empty.contains("which part of the question they do not cover"));
        assert!(!empty.contains("cite no passage numbers"), "{empty}");
        assert!(!empty.contains("do not cover the question"), "{empty}");

        let failed = retrieve_prompt(
            &RetrieveResult::Failed {
                error: "Search failed: index locked".into(),
            },
            true,
            None,
        );
        assert!(failed.contains("Call search_documents now"), "{failed}");
        assert!(failed.contains("the passages you were given earlier in this answer"));
        assert!(failed.contains("answer from the earlier passages alone"));
        assert!(
            !failed.contains("documents do not cover the question"),
            "{failed}"
        );
    }

    #[test]
    fn a_limited_search_says_what_it_searched() {
        let thesis = Some("the sources of the workspace \"Thesis\"");
        let empty = retrieve_prompt(&RetrieveResult::Empty, false, thesis);
        assert!(
            empty.contains("searched the sources of the workspace \"Thesis\" for the question"),
            "{empty}"
        );
        assert!(empty.contains(
            "the sources of the workspace \"Thesis\" do not cover the question (the rest of \
             their library was not searched)"
        ));
        assert!(!empty.contains("their documents do not cover the question"));
        let found = retrieve_prompt(&RetrieveResult::Found { text: "[]".into() }, false, thesis);
        assert!(found.contains("the sources of the workspace \"Thesis\" do not cover that part"));
    }
}
