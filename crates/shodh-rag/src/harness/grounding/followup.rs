//! What happens after the model finishes an answer: finish the run, or ask
//! for one more turn.
//!
//! Two reasons for a follow-up turn, each bounded:
//! * **repair** (setting, default on): claims flagged `unsupported`,
//!   `uncited_factual` or `invalid_citation` are sent back once, with the
//!   closest passage, to be re-grounded or removed. Never more than
//!   [`MAX_REPAIR_ROUNDS`].
//! * **coverage**: information needs (task-list items marked as needs) that
//!   no retrieved passage covers get targeted searches. At most
//!   [`MAX_COVERAGE_ROUNDS`] extra rounds, and only while the answer's tool
//!   budget has calls left.
//!
//! Both can share one turn. The decision is a pure function of the checks,
//! the rounds so far and the remaining budget.

use crate::harness::events::{ClaimCheck, ClaimOutcome, CoverageState, NeedCheck, RevisionReason};
use crate::harness::truncate_chars;

/// Re-grounding turns per answer.
pub const MAX_REPAIR_ROUNDS: u32 = 1;
/// Extra retrieval rounds for uncovered needs per answer.
pub const MAX_COVERAGE_ROUNDS: u32 = 2;
/// Flagged claims listed in one repair request.
const MAX_LISTED_CLAIMS: usize = 12;

/// Follow-up turns used so far in a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rounds {
    pub repair: u32,
    pub coverage: u32,
}

/// The decision after a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Finish,
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

/// Whether a check outcome is flagged for repair.
pub fn needs_repair(outcome: ClaimOutcome) -> bool {
    matches!(
        outcome,
        ClaimOutcome::Unsupported | ClaimOutcome::UncitedFactual | ClaimOutcome::InvalidCitation
    )
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
        _ if !check.missing_numbers.is_empty() => format!(
            "the cited source does not contain {}",
            check.missing_numbers.join(", ")
        ),
        _ => "the cited source does not support it".to_string(),
    }
}

/// Decide what follows a check.
///
/// `auto_repair` is the user's setting; `calls_left` is the answer's
/// remaining tool budget.
pub fn decide(
    checks: &[ClaimCheck],
    needs: &[NeedCheck],
    rounds: Rounds,
    auto_repair: bool,
    calls_left: u32,
) -> Next {
    let repair: Vec<usize> = if auto_repair && rounds.repair < MAX_REPAIR_ROUNDS {
        checks
            .iter()
            .enumerate()
            .filter(|(_, c)| needs_repair(c.outcome))
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
            let closest = check
                .closest
                .map(|n| format!(" Passage [{n}] comes closest."))
                .unwrap_or_default();
            prompt.push_str(&format!(
                "- \"{}\": {}.{closest}\n",
                truncate_chars(&check.text, 300),
                reason_text(check)
            ));
        }
        prompt.push_str(
            "For each one: cite the passage that actually supports it, correct it to what the \
             passages say, or remove it. Use only passage numbers you were given.",
        );
    }
    prompt.push_str(
        "\n\nThen write your complete revised answer from the beginning, with citations. It \
         replaces the previous one, so do not refer to it or to this check.",
    );
    Next::FollowUp {
        reason,
        repair,
        missing_needs: missing.iter().map(|n| n.id.clone()).collect(),
        prompt,
    }
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
            missing_numbers: if outcome == ClaimOutcome::Unsupported {
                vec!["900".into()]
            } else {
                vec![]
            },
            closest: Some(4),
            closest_score: Some(0.7),
        }
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
    fn a_clean_answer_finishes() {
        let checks = vec![check(ClaimOutcome::Supported), check(ClaimOutcome::Weak)];
        let needs = vec![need("1", CoverageState::Covered)];
        assert_eq!(
            decide(&checks, &needs, Rounds::default(), true, 10),
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
        match decide(&checks, &[], Rounds::default(), true, 10) {
            Next::FollowUp {
                reason,
                repair,
                prompt,
                ..
            } => {
                assert_eq!(reason, RevisionReason::Repair);
                assert_eq!(repair, vec![1, 2, 3]);
                assert!(prompt.contains("does not contain 900"));
                assert!(prompt.contains("cites [9]"));
                assert!(prompt.contains("Passage [4] comes closest"));
                assert!(prompt.contains("complete revised answer"));
            }
            Next::Finish => panic!("expected a repair turn"),
        }
        let after = Rounds {
            repair: 1,
            coverage: 0,
        };
        assert_eq!(
            decide(&checks, &[], after, true, 10),
            Next::Finish,
            "never a second repair"
        );
        assert_eq!(
            decide(&checks, &[], Rounds::default(), false, 10),
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
                    repair: 0,
                    coverage,
                },
                true,
                5,
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
                Next::Finish => panic!("round {coverage} should search again"),
            }
        }
        assert_eq!(
            decide(
                &[],
                &needs,
                Rounds {
                    repair: 0,
                    coverage: MAX_COVERAGE_ROUNDS
                },
                true,
                5
            ),
            Next::Finish
        );
        assert_eq!(
            decide(&[], &needs, Rounds::default(), true, 0),
            Next::Finish,
            "no budget left"
        );
    }

    #[test]
    fn repair_and_coverage_share_a_turn() {
        let checks = vec![check(ClaimOutcome::Unsupported)];
        let needs = vec![need("2", CoverageState::Missing)];
        assert!(matches!(
            decide(&checks, &needs, Rounds::default(), true, 3),
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
}
