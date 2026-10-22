//! Which passages go into which call. The paper's first chunk (title and abstract) goes
//! first; then passages are added in rank order, round-robin across columns, until the
//! call holds about [`CALL_TOKEN_BUDGET`] tokens, a passage two columns share counted
//! once. When a column's top passages do not all fit, the columns are split into two
//! calls (the first half, the second half, in column order); a column whose passages
//! still do not all fit gets those that do and is listed as `truncated`.

use std::collections::{BTreeSet, HashSet};

use super::prompt::MAX_COLUMNS_PER_CALL;
use super::Passage;
use crate::embeddings::estimate_tokens;

/// Passages retrieved per column.
pub const PASSAGES_PER_COLUMN: usize = 4;
/// Passage tokens per call.
pub const CALL_TOKEN_BUDGET: usize = 6_000;
/// Most calls for one paper (a retry of an unparsable reply aside).
pub const MAX_CALLS_PER_PAPER: usize = 2;
/// The number and page label of a passage in the prompt.
const PASSAGE_OVERHEAD_TOKENS: usize = 8;

/// Tokens a passage costs in the prompt.
pub fn passage_tokens(passage: &Passage) -> usize {
    estimate_tokens(&passage.text) + PASSAGE_OVERHEAD_TOKENS
}

/// One model call: its columns and passages (indices into the `ranked` input).
#[derive(Debug, Clone, PartialEq)]
pub struct CallPlan {
    pub columns: Vec<usize>,
    pub passages: Vec<Passage>,
    pub truncated: Vec<usize>,
}

fn plan_one(first: Option<&Passage>, ranked: &[Vec<Passage>], columns: &[usize]) -> CallPlan {
    let mut passages = Vec::new();
    let mut seen = HashSet::new();
    let mut used = 0;
    let mut truncated = BTreeSet::new();
    if let Some(p) = first {
        seen.insert(p.chunk_id.clone());
        used += passage_tokens(p);
        passages.push(p.clone());
    }
    let depth = columns.iter().map(|&c| ranked[c].len()).max().unwrap_or(0);
    for rank in 0..depth {
        for &c in columns {
            let Some(p) = ranked[c].get(rank) else {
                continue;
            };
            if seen.contains(&p.chunk_id) {
                continue;
            }
            let cost = passage_tokens(p);
            if used + cost > CALL_TOKEN_BUDGET {
                truncated.insert(c);
                continue;
            }
            seen.insert(p.chunk_id.clone());
            used += cost;
            passages.push(p.clone());
        }
    }
    CallPlan {
        columns: columns.to_vec(),
        passages,
        truncated: truncated.into_iter().collect(),
    }
}

/// The calls for one paper: one when every column's passages fit and the reply of that
/// many columns fits the reply cap ([`MAX_COLUMNS_PER_CALL`]), else two.
pub fn plan_calls(first: Option<&Passage>, ranked: &[Vec<Passage>]) -> Vec<CallPlan> {
    if ranked.is_empty() {
        return Vec::new();
    }
    let all: Vec<usize> = (0..ranked.len()).collect();
    let single = plan_one(first, ranked, &all);
    let reply_fits = ranked.len() <= MAX_COLUMNS_PER_CALL;
    if (single.truncated.is_empty() && reply_fits) || ranked.len() < MAX_CALLS_PER_PAPER {
        return vec![single];
    }
    let mid = ranked.len().div_ceil(MAX_CALLS_PER_PAPER);
    vec![
        plan_one(first, ranked, &all[..mid]),
        plan_one(first, ranked, &all[mid..]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A passage costing exactly `tokens` (estimate: chars / 3 + 2, plus 8 overhead).
    fn passage(id: &str, tokens: usize) -> Passage {
        Passage {
            chunk_id: id.to_string(),
            text: "x".repeat((tokens - 10) * 3),
            page: None,
            bboxes: None,
        }
    }

    fn ranked(columns: usize, per_column: usize, tokens: usize) -> Vec<Vec<Passage>> {
        (0..columns)
            .map(|c| {
                (0..per_column)
                    .map(|r| passage(&format!("{c}-{r}"), tokens))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_test_passage_costs_what_it_says() {
        assert_eq!(passage_tokens(&passage("a", 1_000)), 1_000);
    }

    #[test]
    fn columns_that_fit_share_one_call_with_the_first_chunk_first() {
        let first = passage("first", 500);
        let plans = plan_calls(Some(&first), &ranked(2, 2, 1_000));
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].columns, vec![0, 1]);
        assert!(plans[0].truncated.is_empty());
        let ids: Vec<&str> = plans[0]
            .passages
            .iter()
            .map(|p| p.chunk_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["first", "0-0", "1-0", "0-1", "1-1"],
            "round-robin by rank"
        );
    }

    #[test]
    fn a_shared_passage_is_sent_once() {
        let shared = passage("shared", 1_000);
        let lists = vec![
            vec![shared.clone(), passage("a", 1_000)],
            vec![shared, passage("b", 1_000)],
        ];
        let plans = plan_calls(None, &lists);
        assert_eq!(plans[0].passages.len(), 3);
    }

    #[test]
    fn columns_that_do_not_fit_are_split_into_two_calls() {
        let plans = plan_calls(None, &ranked(4, 2, 1_000));
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].columns, vec![0, 1]);
        assert_eq!(plans[1].columns, vec![2, 3]);
        assert!(plans
            .iter()
            .all(|p| p.truncated.is_empty() && p.passages.len() == 4));
    }

    #[test]
    fn more_columns_than_one_reply_can_hold_are_split_even_when_the_passages_fit() {
        let columns = MAX_COLUMNS_PER_CALL + 1;
        let plans = plan_calls(None, &ranked(columns, 1, 100));
        assert_eq!(plans.len(), 2);
        assert!(plans.iter().all(|p| p.truncated.is_empty()));
        assert!(plans
            .iter()
            .all(|p| p.columns.len() <= MAX_COLUMNS_PER_CALL));
        let all: usize = plans.iter().map(|p| p.columns.len()).sum();
        assert_eq!(all, columns);
        // The most a table may have is two full calls.
        let most = plan_calls(
            None,
            &ranked(MAX_COLUMNS_PER_CALL * MAX_CALLS_PER_PAPER, 1, 100),
        );
        assert_eq!(most.len(), 2);
        assert!(most.iter().all(|p| p.columns.len() == MAX_COLUMNS_PER_CALL));
        let exact = plan_calls(None, &ranked(MAX_COLUMNS_PER_CALL, 1, 100));
        assert_eq!(exact.len(), 1);
    }

    #[test]
    fn beyond_two_calls_columns_get_what_fits_and_are_marked() {
        let plans = plan_calls(None, &ranked(2, 4, 2_000));
        assert_eq!(plans.len(), 2);
        for (i, plan) in plans.iter().enumerate() {
            assert_eq!(plan.truncated, vec![i]);
            assert_eq!(plan.passages.len(), 3);
        }
        let single = plan_calls(None, &ranked(1, 4, 2_000));
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].truncated, vec![0]);
        assert!(plan_calls(None, &[]).is_empty());
    }
}
