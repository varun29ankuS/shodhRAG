//! The fill prompt: the rules, the reply envelope, the numbered passages (with pages)
//! and the columns as a JSON list. Columns are tagged `c1`, `c2`, … in the prompt, not by
//! their ids, so the model has short, unambiguous tags to copy back. Passages are shown
//! with line-break hyphenation joined, so the words the model copies are the words the
//! quote check looks for.

use serde_json::{json, Value};

use super::normalize::join_line_breaks;
use super::validate::{MIN_QUOTE_CHARS, MIN_QUOTE_WORDS};
use super::{collapse, ColumnKind, ColumnSpec, Passage};

const RULES_INTRO: &str =
    "You fill one row of an evidence table from passages of one research paper.\n\
Answer each column's question from the passages only.\n\
Rules:\n";

const RULES_REST: &str = "- \"passage\": the number of the passage the quote is from.\n\
- \"answer\": null when the passages do not answer the question. Never guess.\n\
- kind \"number\": the number exactly as printed in the quote, as a string, with its unit when the quote states one (\"0.912\", \"48 GB\"). Never compute, round or convert.\n\
- kind \"yes_no\": \"yes\" or \"no\".\n\
- kind \"choice\": exactly one of the column's options.\n\
- kind \"text\": a short answer of at most 500 characters.\n\
Reply with one JSON object and nothing else:\n\
{\"cells\":[{\"column\":\"<column id>\",\"answer\":<value or null>,\"quote\":\"<verbatim quote>\",\"passage\":<passage number>}]}\n";

/// The quote rule, with the lengths the quote check enforces.
fn quote_rule() -> String {
    format!(
        "- \"quote\": the words that give the answer, copied exactly as written in one passage: word for word, same letter case, at most 300 characters, no ellipsis, no changes. It must be at least {MIN_QUOTE_WORDS} words and {MIN_QUOTE_CHARS} characters long, so quote the whole phrase or sentence that states the answer, not one word.\n"
    )
}

/// Output tokens allowed for a reply covering `columns` columns.
pub fn reply_budget(columns: usize) -> usize {
    (200 + 160 * columns).min(2_000)
}

fn column_json(tag: &str, spec: &ColumnSpec) -> Value {
    let mut column = json!({
        "id": tag,
        "question": spec.question,
        "kind": spec.kind.as_str(),
    });
    if spec.kind == ColumnKind::Choice {
        column["options"] = json!(spec.options);
    }
    if !spec.instructions.is_empty() {
        column["instructions"] = json!(spec.instructions);
    }
    column
}

/// The prompt for one call. `columns` pairs each tag with its column.
pub fn fill_prompt(passages: &[Passage], columns: &[(&str, &ColumnSpec)]) -> String {
    let mut prompt = String::from(RULES_INTRO);
    prompt.push_str(&quote_rule());
    prompt.push_str(RULES_REST);
    prompt.push_str("\nPassages:\n");
    for (i, passage) in passages.iter().enumerate() {
        let text = collapse(&join_line_breaks(&passage.text));
        match passage.page {
            Some(page) => prompt.push_str(&format!("[{}] (page {page}) {text}\n", i + 1)),
            None => prompt.push_str(&format!("[{}] {text}\n", i + 1)),
        }
    }
    let list: Vec<Value> = columns
        .iter()
        .map(|(tag, spec)| column_json(tag, spec))
        .collect();
    prompt.push_str("\nColumns:\n");
    prompt.push_str(&Value::Array(list).to_string());
    prompt.push('\n');
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_numbers_passages_with_pages_and_lists_columns_as_json() {
        let passages = vec![
            Passage {
                chunk_id: "a".into(),
                text: "Dense Retrieval.\nAbstract: contras-\ntive.".into(),
                page: Some(1),
                bboxes: None,
            },
            Passage {
                chunk_id: "b".into(),
                text: "No page here.".into(),
                page: None,
                bboxes: None,
            },
        ];
        let choice = ColumnSpec {
            question: "Study kind?".into(),
            kind: ColumnKind::Choice,
            options: vec!["Benchmark".into(), "Survey".into()],
            instructions: "pick the main one".into(),
        };
        let text = ColumnSpec {
            question: "Dataset?".into(),
            kind: ColumnKind::Text,
            options: vec![],
            instructions: String::new(),
        };
        let prompt = fill_prompt(&passages, &[("c1", &choice), ("c2", &text)]);
        assert!(prompt.contains("[1] (page 1) Dense Retrieval. Abstract: contrastive.\n"));
        assert!(prompt.contains("[2] No page here.\n"));
        // Key order inside a JSON object depends on serde_json's `preserve_order`
        // feature, so each field is checked on its own.
        for field in [
            r#""id":"c1""#,
            r#""kind":"choice""#,
            r#""question":"Study kind?""#,
            r#""instructions":"pick the main one""#,
        ] {
            assert!(prompt.contains(field), "{field}");
        }
        assert!(prompt.contains(r#""options":["Benchmark","Survey"]"#));
        assert_eq!(
            prompt.matches(r#""options""#).count(),
            1,
            "a text column has no options"
        );
        assert!(prompt.contains("{\"cells\":[{\"column\""));
        assert_eq!(reply_budget(1), 360);
        assert_eq!(reply_budget(20), 2_000);
    }

    #[test]
    fn the_quote_rule_states_the_lengths_the_check_enforces() {
        let prompt = fill_prompt(&[], &[]);
        assert!(prompt.contains(&format!(
            "at least {MIN_QUOTE_WORDS} words and {MIN_QUOTE_CHARS} characters"
        )));
        assert!(prompt.contains("same letter case"));
    }
}
