//! Reading a free model's reply. No provider path supports structured output, so the
//! reply is searched for the first balanced `{…}` that parses as JSON and has a `cells`
//! array: reasoning blocks (`<think>…</think>`), code fences and prose around it are
//! skipped, braces and quotes inside JSON strings do not count, and unknown keys are
//! ignored. A cell without a `column` is dropped; the fill then marks its column
//! `missing_in_reply`.
//!
//! The three states of `answer` stay distinct, as [`RawAnswer`] defines them: a cell with
//! no `answer` key is kept with `answer: None` (the check fails it as `missing_answer`,
//! never as "not found"), an explicit `null` is `Some(None)` (the model found nothing), and
//! any other value is `Some(Some(value))`.

use serde_json::Value;

use super::validate::RawAnswer;

/// One cell of a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCell {
    /// The column tag as the model wrote it, trimmed and lower-cased.
    pub column: String,
    pub raw: RawAnswer,
}

const THINK_END: &str = "</think>";

/// The reply after its last closed reasoning block.
fn after_thinking(reply: &str) -> &str {
    // ASCII lower-casing keeps byte offsets.
    match reply.to_ascii_lowercase().rfind(THINK_END) {
        Some(at) => &reply[at + THINK_END.len()..],
        None => reply,
    }
}

/// Byte length of the balanced object `text` starts with (`text` starts with `{`),
/// skipping braces inside JSON strings.
fn balanced_len(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn passage_number(value: Option<&Value>) -> Option<usize> {
    match value? {
        Value::Number(n) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        Value::String(s) => s
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim()
            .parse()
            .ok(),
        _ => None,
    }
}

fn cell_of(value: &Value) -> Option<ParsedCell> {
    let object = value.as_object()?;
    let column = match object.get("column")? {
        Value::String(s) => s.trim().to_lowercase(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let answer = match object.get("answer") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(other) => Some(Some(other.clone())),
    };
    Some(ParsedCell {
        column,
        raw: RawAnswer {
            answer,
            quote: object
                .get("quote")
                .and_then(Value::as_str)
                .map(str::to_string),
            passage: passage_number(object.get("passage")),
        },
    })
}

/// The cells of a reply, or `None` when it holds no `{"cells":[…]}` envelope.
pub fn parse_reply(reply: &str) -> Option<Vec<ParsedCell>> {
    let text = after_thinking(reply);
    let mut from = 0;
    while let Some(offset) = text[from..].find('{') {
        let start = from + offset;
        if let Some(len) = balanced_len(&text[start..]) {
            if let Ok(Value::Object(envelope)) =
                serde_json::from_str::<Value>(&text[start..start + len])
            {
                if let Some(Value::Array(cells)) = envelope.get("cells") {
                    return Some(cells.iter().filter_map(cell_of).collect());
                }
            }
        }
        from = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_thinking_and_prose_around_the_envelope_are_skipped() {
        let reply = "<think>\nPassage 2 has {the number}.\n</think>\nSure! Here it is:\n```json\n\
            {\"cells\":[{\"column\":\" C1 \",\"answer\":\"0.912\",\"quote\":\"Recall@10 of 0.912\",\"passage\":2,\"confidence\":0.8}]}\n\
            ```\nHope this helps {really}.";
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].column, "c1");
        assert_eq!(cells[0].raw.answer, Some(Some(Value::from("0.912"))));
        assert_eq!(cells[0].raw.passage, Some(2));
    }

    #[test]
    fn braces_and_quotes_inside_strings_do_not_end_the_envelope() {
        let reply = r#"{"cells":[{"column":"c1","answer":"the set \\{x : x > 0\\}","quote":"We define \"positive\" as {x | x > 0}.","passage":"[3]"}]}"#;
        let cells = parse_reply(reply).unwrap();
        assert_eq!(
            cells[0].raw.quote.as_deref(),
            Some("We define \"positive\" as {x | x > 0}.")
        );
        assert_eq!(cells[0].raw.passage, Some(3));
    }

    #[test]
    fn an_earlier_object_without_cells_is_passed_over() {
        let reply = r#"Columns: {"c1": "dataset"}. Answer: {"cells":[{"column":"c1","answer":null,"quote":"","passage":null}]}"#;
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells[0].raw.passage, None);
    }

    #[test]
    fn a_null_answer_is_not_a_missing_answer() {
        let reply = r#"{"cells":[
            {"column":"c1","answer":null,"quote":"","passage":null},
            {"column":"c2","quote":"a quote of several words","passage":1},
            {"column":"c3","answer":"x"}]}"#;
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(
            cells[0].raw.answer,
            Some(None),
            "an explicit null: not found"
        );
        assert_eq!(
            cells[1].raw.answer, None,
            "no key at all: a parse failure, not NotFound"
        );
        assert_eq!(cells[2].raw.answer, Some(Some(Value::from("x"))));
    }

    #[test]
    fn cells_without_a_column_are_dropped() {
        let reply = r#"{"cells":[{"answer":"x"},{"column":"c2","quote":"q"},{"column":3,"answer":true,"quote":"q"},"junk",{"column":[1],"answer":"y"}]}"#;
        let cells = parse_reply(reply).unwrap();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].column, "c2");
        assert_eq!(cells[0].raw.answer, None);
        assert_eq!(cells[1].column, "3");
        assert_eq!(cells[1].raw.answer, Some(Some(Value::Bool(true))));
    }

    #[test]
    fn no_envelope_is_none() {
        for reply in [
            "",
            "I could not find it.",
            "{\"cells\": \"none\"}",
            "{\"cells\":[",
            "</think>",
        ] {
            assert_eq!(parse_reply(reply), None, "{reply}");
        }
    }
}
