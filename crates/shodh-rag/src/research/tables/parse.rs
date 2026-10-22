//! Reading a free model's reply. No provider path supports structured output, so the
//! reply is searched for the first balanced `{…}` or `[…]` that parses as JSON and holds
//! the cells: reasoning blocks (`<think>…</think>`), code fences and prose around it are
//! skipped, braces and quotes inside JSON strings do not count, and unknown keys are
//! ignored.
//!
//! What is accepted besides `{"cells":[…]}`: the list under `results` or `answers`, a bare
//! top-level array of cells, trailing commas before `]` or `}`, and a short fixed list of
//! key aliases (`key` for `column`, `value`/`result` for `answer`,
//! `evidence`/`source_quote` for `quote`; not `id`, which is the key of the prompt's own
//! column list that a model often echoes before it answers). The column tag may be written `c1`, `1`,
//! `column 1` or `C1.`. Nothing is ever done to a `quote` or an `answer`: they are the
//! model's text exactly, so no repair here can make a quote pass the check.
//!
//! A reply cut off at the token limit has no closing brackets. Its complete cell objects
//! are salvaged (`cut_off` is set), so the fill keeps them and asks again only for the
//! columns the reply never reached.
//!
//! The three states of `answer` stay distinct, as [`RawAnswer`] defines them: a cell with
//! no `answer` key is kept with `answer: None` (the check fails it as `missing_answer`,
//! never as "not found"), an explicit `null` is `Some(None)` (the model found nothing),
//! and any other value is `Some(Some(value))`. A cell with no column tag is dropped; the
//! fill then marks its column `missing_in_reply`.

use serde_json::{Map, Value};

use super::validate::RawAnswer;

/// One cell of a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCell {
    /// The column tag as the model wrote it, normalised to `c<n>` when it is a number or
    /// `column <n>`, else trimmed and lower-cased.
    pub column: String,
    pub raw: RawAnswer,
}

/// What a reply held.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedReply {
    pub cells: Vec<ParsedCell>,
    /// The reply had no complete envelope (it was cut off); `cells` are the complete cell
    /// objects before the cut.
    pub cut_off: bool,
}

const THINK_END: &str = "</think>";
/// Keys the list of cells may sit under.
const LIST_KEYS: [&str; 3] = ["cells", "results", "answers"];
const COLUMN_KEYS: [&str; 2] = ["column", "key"];
const ANSWER_KEYS: [&str; 3] = ["answer", "value", "result"];
const QUOTE_KEYS: [&str; 3] = ["quote", "evidence", "source_quote"];
const PASSAGE_KEYS: [&str; 2] = ["passage", "passage_number"];

/// The reply after its last closed reasoning block.
fn after_thinking(reply: &str) -> &str {
    // ASCII lower-casing keeps byte offsets.
    match reply.to_ascii_lowercase().rfind(THINK_END) {
        Some(at) => &reply[at + THINK_END.len()..],
        None => reply,
    }
}

/// Byte length of the balanced object or array `text` starts with (`text` starts with
/// `{` or `[`), skipping brackets inside JSON strings.
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
            '{' | '[' => depth += 1,
            '}' | ']' => {
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

/// `json` without the commas that stand directly before a `]` or `}` outside strings.
fn strip_trailing_commas(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut in_string = false;
    let mut escaped = false;
    for (at, c) in json.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            out.push(c);
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            ',' if json[at + 1..]
                .trim_start()
                .starts_with(|next: char| next == ']' || next == '}') => {}
            _ => out.push(c),
        }
    }
    out
}

fn first_of<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| object.get(*key))
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

/// The column tag the model wrote, as the prompt's `c<n>`: `1`, `column 1`, `Column_1`
/// and `C1.` are `c1`; anything else is trimmed and lower-cased.
fn column_tag(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let lower = lower.trim_matches(|c: char| matches!(c, '.' | ':' | '"' | '\'' | ' '));
    let rest = ["column", "col", "c"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        .unwrap_or(lower)
        .trim_start_matches([' ', '_', '-', '#']);
    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(n) = rest.parse::<usize>() {
            return format!("c{n}");
        }
    }
    lower.to_string()
}

fn cell_of(value: &Value) -> Option<ParsedCell> {
    let object = value.as_object()?;
    let column = match first_of(object, &COLUMN_KEYS)? {
        Value::String(s) => column_tag(s),
        Value::Number(n) => column_tag(&n.to_string()),
        _ => return None,
    };
    let answer = match first_of(object, &ANSWER_KEYS) {
        None => None,
        Some(Value::Null) => Some(None),
        Some(other) => Some(Some(other.clone())),
    };
    Some(ParsedCell {
        column,
        raw: RawAnswer {
            answer,
            quote: first_of(object, &QUOTE_KEYS)
                .and_then(Value::as_str)
                .map(str::to_string),
            passage: passage_number(first_of(object, &PASSAGE_KEYS)),
        },
    })
}

/// The cells of a parsed value: the list under `cells`/`results`/`answers` of an object
/// (an empty list included), or a top-level array holding at least one cell.
fn cells_of_value(value: &Value) -> Option<Vec<ParsedCell>> {
    match value {
        Value::Object(object) => LIST_KEYS.iter().find_map(|key| match object.get(*key) {
            Some(Value::Array(items)) => Some(items.iter().filter_map(cell_of).collect()),
            _ => None,
        }),
        Value::Array(items) => {
            let cells: Vec<ParsedCell> = items.iter().filter_map(cell_of).collect();
            // A bare array is only a reply when something in it answers: a list of
            // columns (a model echoing the prompt) has a tag but no answer or quote.
            let answers = cells
                .iter()
                .any(|c| c.raw.answer.is_some() || c.raw.quote.is_some());
            answers.then_some(cells)
        }
        _ => None,
    }
}

/// Where the items of the reply's cell list begin (just after its `[`): under one of
/// the list keys, else a bare array that opens with an object.
fn list_start(text: &str) -> Option<usize> {
    for key in LIST_KEYS {
        let pattern = format!("\"{key}\"");
        let mut from = 0;
        while let Some(found) = text[from..].find(&pattern) {
            let after = from + found + pattern.len();
            if let Some(rest) = text[after..].trim_start().strip_prefix(':') {
                let rest = rest.trim_start();
                if rest.starts_with('[') {
                    return Some(text.len() - rest.len() + 1);
                }
            }
            from = after;
        }
    }
    let mut from = 0;
    while let Some(found) = text[from..].find('[') {
        let after = from + found + 1;
        if text[after..].trim_start().starts_with('{') {
            return Some(after);
        }
        from = after;
    }
    None
}

/// The complete cell objects of a cut-off reply, up to the first object that is not
/// complete.
fn salvage(text: &str) -> Option<ParsedReply> {
    let mut at = list_start(text)?;
    let mut cells = Vec::new();
    loop {
        let rest = &text[at..];
        let item = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        at += rest.len() - item.len();
        if !item.starts_with('{') {
            break;
        }
        let Some(len) = balanced_len(item) else {
            break;
        };
        if let Ok(value) = serde_json::from_str::<Value>(&strip_trailing_commas(&item[..len])) {
            cells.extend(cell_of(&value));
        }
        at += len;
    }
    (!cells.is_empty()).then_some(ParsedReply {
        cells,
        cut_off: true,
    })
}

/// The cells of a reply, or `None` when it holds no cells at all (no envelope, and not
/// even one complete cell object). That one case earns the format-correction retry.
pub fn parse_reply(reply: &str) -> Option<ParsedReply> {
    let text = after_thinking(reply);
    let mut from = 0;
    while let Some(offset) = text[from..].find(['{', '[']) {
        let start = from + offset;
        if let Some(len) = balanced_len(&text[start..]) {
            let candidate = strip_trailing_commas(&text[start..start + len]);
            if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
                if let Some(cells) = cells_of_value(&value) {
                    return Some(ParsedReply {
                        cells,
                        cut_off: false,
                    });
                }
            }
        }
        from = start + 1;
    }
    salvage(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(reply: &str) -> Vec<ParsedCell> {
        parse_reply(reply).unwrap().cells
    }

    #[test]
    fn fences_thinking_and_prose_around_the_envelope_are_skipped() {
        let reply = "<think>\nPassage 2 has {the number}.\n</think>\nSure! Here it is:\n```json\n\
            {\"cells\":[{\"column\":\" C1 \",\"answer\":\"0.912\",\"quote\":\"Recall@10 of 0.912\",\"passage\":2,\"confidence\":0.8}]}\n\
            ```\nHope this helps {really}.";
        let parsed = parse_reply(reply).unwrap();
        assert!(!parsed.cut_off);
        let cells = parsed.cells;
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].column, "c1");
        assert_eq!(cells[0].raw.answer, Some(Some(Value::from("0.912"))));
        assert_eq!(cells[0].raw.passage, Some(2));
    }

    #[test]
    fn braces_and_quotes_inside_strings_do_not_end_the_envelope() {
        let reply = r#"{"cells":[{"column":"c1","answer":"the set \\{x : x > 0\\}","quote":"We define \"positive\" as {x | x > 0}.","passage":"[3]"}]}"#;
        let cells = cells(reply);
        assert_eq!(
            cells[0].raw.quote.as_deref(),
            Some("We define \"positive\" as {x | x > 0}.")
        );
        assert_eq!(cells[0].raw.passage, Some(3));
    }

    #[test]
    fn an_earlier_object_without_cells_is_passed_over() {
        let reply = r#"Columns: {"c1": "dataset"} and [3]. Answer: {"cells":[{"column":"c1","answer":null,"quote":"","passage":null}]}"#;
        let cells = cells(reply);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].raw.passage, None);
    }

    #[test]
    fn a_null_answer_is_not_a_missing_answer() {
        let reply = r#"{"cells":[
            {"column":"c1","answer":null,"quote":"","passage":null},
            {"column":"c2","quote":"a quote of several words","passage":1},
            {"column":"c3","answer":"x"}]}"#;
        let cells = cells(reply);
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
        let cells = cells(reply);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].column, "c2");
        assert_eq!(cells[0].raw.answer, None);
        assert_eq!(cells[1].column, "c3");
        assert_eq!(cells[1].raw.answer, Some(Some(Value::Bool(true))));
    }

    #[test]
    fn a_bare_array_and_the_other_list_keys_are_cells_too() {
        let bare = r#"Here: [{"column":"c1","answer":"A","quote":"one two three"},{"column":"c2","answer":null}]"#;
        let parsed = parse_reply(bare).unwrap();
        assert!(!parsed.cut_off);
        assert_eq!(parsed.cells.len(), 2);
        for key in ["results", "answers"] {
            let reply = format!(r#"{{"{key}":[{{"column":"c1","answer":"A"}}]}}"#);
            assert_eq!(cells(&reply).len(), 1, "{key}");
        }
        // An array that is not cells is not a reply.
        assert_eq!(parse_reply("The list [1, 2, 3] and [\"a\"]"), None);
    }

    #[test]
    fn trailing_commas_are_tolerated_outside_strings_only() {
        let reply = r#"{"cells":[{"column":"c1","answer":"a,]","quote":"x, } y",},],}"#;
        let cells = cells(reply);
        assert_eq!(cells[0].raw.answer, Some(Some(Value::from("a,]"))));
        assert_eq!(cells[0].raw.quote.as_deref(), Some("x, } y"));
    }

    #[test]
    fn key_aliases_are_read_and_the_text_is_never_altered() {
        let reply = "{\"cells\":[{\"key\":\"C2\",\"value\":\" 0.912 \",\"evidence\":\"  \u{201c}Recall\u{201d} of 0.912  \",\"passage_number\":\"2\"},{\"key\":1,\"result\":\"yes\",\"source_quote\":\"it is\"}]}";
        let cells = cells(reply);
        assert_eq!(cells[0].column, "c2");
        assert_eq!(cells[0].raw.answer, Some(Some(Value::from(" 0.912 "))));
        assert_eq!(
            cells[0].raw.quote.as_deref(),
            Some("  \u{201c}Recall\u{201d} of 0.912  ")
        );
        assert_eq!(cells[0].raw.passage, Some(2));
        assert_eq!(cells[1].column, "c1");
        assert_eq!(cells[1].raw.answer, Some(Some(Value::from("yes"))));
        assert_eq!(cells[1].raw.quote.as_deref(), Some("it is"));
    }

    #[test]
    fn an_echoed_column_list_is_not_the_reply() {
        let reply = r#"The columns are:
[{"id":"c1","kind":"text","question":"Dataset?"},{"id":"c2","kind":"number","question":"Recall?"}]
Here are the answers:
{"cells":[{"column":"c1","answer":"A","quote":"one two three"},{"column":"c2","answer":null}]}"#;
        let parsed = parse_reply(reply).unwrap();
        assert!(!parsed.cut_off);
        assert_eq!(parsed.cells.len(), 2);
        assert_eq!(parsed.cells[0].raw.answer, Some(Some(Value::from("A"))));
        // The echo alone is nothing, and an echo before a cut-off envelope still lets the
        // envelope's complete cells through.
        assert_eq!(
            parse_reply(r#"[{"id":"c1","kind":"text","question":"Dataset?"}]"#),
            None
        );
        let cut = r#"[{"id":"c1","question":"Dataset?"}] {"cells":[{"column":"c1","answer":"A","quote":"one two three"},{"column":"c2","ans"#;
        let parsed = parse_reply(cut).unwrap();
        assert!(parsed.cut_off);
        assert_eq!(parsed.cells.len(), 1);
    }

    #[test]
    fn column_tags_are_read_as_c_numbers() {
        for (written, tag) in [
            ("c1", "c1"),
            ("C1.", "c1"),
            ("1", "c1"),
            (" Column 2 ", "c2"),
            ("column_3", "c3"),
            ("col-4", "c4"),
            ("c05", "c5"),
            ("dataset", "dataset"),
        ] {
            assert_eq!(column_tag(written), tag, "{written}");
        }
    }

    #[test]
    fn a_cut_off_reply_keeps_its_complete_cells() {
        let reply = "<think>hmm</think>\n```json\n{\"cells\":[\n\
            {\"column\":\"c1\",\"answer\":\"A\",\"quote\":\"We pre-train on A, {sic}\",\"passage\":1},\n\
            {\"column\":\"c2\",\"answer\":null,\"quote\":\"\",\"passage\":null},\n\
            {\"column\":\"c3\",\"answer\":\"yes\",\"quote\":\"Our code is avail";
        let parsed = parse_reply(reply).unwrap();
        assert!(parsed.cut_off);
        assert_eq!(parsed.cells.len(), 2);
        assert_eq!(parsed.cells[0].column, "c1");
        assert_eq!(
            parsed.cells[0].raw.quote.as_deref(),
            Some("We pre-train on A, {sic}")
        );
        assert_eq!(parsed.cells[1].raw.answer, Some(None));
        // Cut before any cell is complete: nothing to keep.
        assert_eq!(
            parse_reply("{\"cells\":[{\"column\":\"c1\",\"answer\":\"A\",\"quo"),
            None
        );
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
