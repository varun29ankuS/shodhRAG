//! Explicit memory requests: "Remember that I'm writing my thesis on ...", "From now on,
//! cite page numbers". The user's own words are filed as a note suggestion by the host
//! when the message arrives, before and without any model call
//! ([`super::Learner::file_user_note`]), so an unambiguous request never depends on the
//! answering model choosing to call `remember`.
//!
//! The matcher is anchored and deterministic: the message must *start* with the request
//! (after an optional "please"). A request inside a question ("Can you summarise it?
//! Remember to cite the pages."), a question about remembering ("Do you remember the
//! paper...?", "Remember when we...?"), a one-off reminder ("Remember to cite...") and
//! quoted text are not requests to keep something.

/// Most characters of a note (the `remember` tool's limit).
pub const MAX_NOTE_CHARS: usize = 2_000;

/// An explicit request to remember something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRequest {
    /// What to keep, verbatim from the user's message ("I'm writing my thesis on ...").
    pub note: String,
    /// The user's words it was taken from, request included ("Remember that I'm ...").
    pub evidence: String,
    /// What the message says after the note's sentence (a separate question or request),
    /// if anything: it is what the answer is about.
    pub rest: Option<String>,
}

/// Openings that ask to keep what follows; the opening itself is not part of the note.
/// `needs_that`: only a request when followed by "that" or a colon ("Note the
/// differences between ..." asks about the documents).
const KEEP: [(&str, bool); 9] = [
    ("remember", false),
    ("keep in mind", false),
    ("don't forget", false),
    ("do not forget", false),
    ("dont forget", false),
    ("make a note", true),
    ("take note", true),
    ("note down", false),
    ("note", true),
];

/// Words after "remember" or "don't forget" that make it a question or a one-off
/// reminder, not something to keep.
const NOT_KEPT_AFTER: [&str; 11] = [
    "to", "when", "what", "how", "which", "who", "whom", "where", "why", "if", "whether",
];

/// Openings of a standing instruction; the whole instruction is the note.
const STANDING: [&str; 4] = ["from now on", "going forward", "in future", "in the future"];

/// "Always ..." / "Never ..." is a standing instruction when it is about answers.
const ABOUT_ANSWERS: [&str; 10] = [
    "when you answer",
    "when answering",
    "whenever you answer",
    "in your answers",
    "in every answer",
    "in all answers",
    "in answers",
    "when you reply",
    "when you respond",
    "in your replies",
];

/// Characters that open quoted text.
const QUOTES: [char; 7] = ['"', '\'', '\u{201c}', '\u{2018}', '\u{ab}', '`', '>'];

fn same(a: char, b: char) -> bool {
    let apostrophe = |c: char| matches!(c, '\'' | '\u{2019}' | '\u{2018}');
    (apostrophe(a) && apostrophe(b)) || a.to_lowercase().eq(b.to_lowercase())
}

/// `text` after `phrase` (compared ignoring case, typographic apostrophes as plain ones),
/// when `phrase` ends at a word boundary.
fn after<'t>(text: &'t str, phrase: &str) -> Option<&'t str> {
    let mut chars = text.char_indices();
    for expected in phrase.chars() {
        let (_, c) = chars.next()?;
        if !same(c, expected) {
            return None;
        }
    }
    let rest = match chars.next() {
        Some((at, c)) if !c.is_alphanumeric() => &text[at..],
        Some(_) => return None,
        None => "",
    };
    Some(rest)
}

/// `text` without leading spaces and `,:;-–—` separators.
fn skip_separators(text: &str) -> &str {
    text.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ':' | ';' | '-' | '\u{2013}' | '\u{2014}')
    })
}

/// The first word of `text`, lower-cased.
fn first_word(text: &str) -> String {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .find(|w| !w.is_empty())
        .unwrap_or("")
        .to_lowercase()
}

/// Words before a full stop that do not end a sentence ("Dr. Rao", "e.g. DiskANN").
const ABBREVIATIONS: [&str; 14] = [
    "dr", "mr", "mrs", "ms", "prof", "st", "vs", "etc", "e.g", "i.e", "fig", "eq", "no", "al",
];

/// Whether the `.` at byte `at` of `text` ends a sentence: it is followed by a space or the
/// end, and does not close an abbreviation or an initial.
fn ends_sentence(text: &str, at: usize) -> bool {
    if !text[at + 1..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
    {
        return false;
    }
    let word = text[..at]
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or("")
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    let initial = word.chars().count() == 1 && word.chars().all(char::is_alphabetic);
    !(initial || ABBREVIATIONS.contains(&word.as_str()))
}

/// The first sentence of `text` (the note) and what follows it (`None` when nothing
/// does). `None` when the first sentence is a question: the message asks something about
/// remembering, it does not ask to keep anything.
fn first_sentence(text: &str) -> Option<(&str, Option<&str>)> {
    for (at, c) in text.char_indices() {
        let end = match c {
            '?' => return None,
            '!' | ';' | '\n' => true,
            '.' => ends_sentence(text, at),
            _ => false,
        };
        if end {
            let rest = text[at + c.len_utf8()..].trim();
            return Some((text[..=at].trim(), (!rest.is_empty()).then_some(rest)));
        }
    }
    Some((text.trim(), None))
}

/// The request in `text`, if the message is one.
pub fn memory_request(text: &str) -> Option<MemoryRequest> {
    let text = text.trim();
    if text.starts_with(QUOTES) {
        return None;
    }
    let start = after(text, "please").map(skip_separators).unwrap_or(text);
    let (note, rest) = keep_request(start).or_else(|| standing_instruction(start))?;
    let note = note.trim_end();
    if note.chars().filter(|c| c.is_alphanumeric()).count() < 3
        || note.chars().count() > MAX_NOTE_CHARS
    {
        return None;
    }
    // `note` is a part of `start`: the evidence runs from the request to the note's end.
    let note_end = note.as_ptr() as usize - start.as_ptr() as usize + note.len();
    Some(MemoryRequest {
        note: note.to_string(),
        evidence: start[..note_end].trim().to_string(),
        rest: rest.map(str::to_string),
    })
}

/// "Remember (that) ...", "Keep in mind ...", "Note: ...": what follows the opening.
fn keep_request(text: &str) -> Option<(&str, Option<&str>)> {
    for (opening, needs_that) in KEEP {
        let Some(rest) = after(text, opening) else {
            continue;
        };
        let separated = skip_separators(rest);
        let colon = rest.trim_start().starts_with(':');
        let (body, that) = match after(separated, "that") {
            Some(body) => (skip_separators(body), true),
            None => (separated, false),
        };
        if needs_that && !that && !colon {
            return None;
        }
        if !that && NOT_KEPT_AFTER.contains(&first_word(body).as_str()) {
            return None;
        }
        return first_sentence(body);
    }
    None
}

/// "From now on, ..." or "Always/Never ... when you answer": the whole instruction.
fn standing_instruction(text: &str) -> Option<(&str, Option<&str>)> {
    let standing = STANDING
        .iter()
        .any(|opening| after(text, opening).is_some());
    let about_answers = (after(text, "always").is_some() || after(text, "never").is_some()) && {
        let lower = text.to_lowercase();
        ABOUT_ANSWERS.iter().any(|marker| lower.contains(marker))
    };
    if !standing && !about_answers {
        return None;
    }
    first_sentence(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(text: &str) -> Option<String> {
        memory_request(text).map(|r| r.note)
    }

    #[test]
    fn requests_to_remember_keep_the_users_words() {
        let thesis = memory_request(
            "Remember that I'm writing my thesis on disk-based vector search, and memory cost matters most to me.",
        )
        .unwrap();
        assert_eq!(
            thesis.note,
            "I'm writing my thesis on disk-based vector search, and memory cost matters most to me."
        );
        assert_eq!(
            thesis.evidence,
            "Remember that I'm writing my thesis on disk-based vector search, and memory cost matters most to me."
        );
        assert_eq!(thesis.rest, None);
        assert_eq!(
            note("please remember: my advisor is Dr. Rao").as_deref(),
            Some("my advisor is Dr. Rao")
        );
        assert_eq!(
            note("Keep in mind that I use metric units").as_deref(),
            Some("I use metric units")
        );
        assert_eq!(
            note("Don\u{2019}t forget that the deadline is Friday.").as_deref(),
            Some("the deadline is Friday.")
        );
        assert_eq!(
            note("Note: I review papers on Fridays").as_deref(),
            Some("I review papers on Fridays")
        );
        assert_eq!(note("REMEMBER I am vegan").as_deref(), Some("I am vegan"));
    }

    #[test]
    fn standing_instructions_are_kept_whole() {
        assert_eq!(
            note("From now on, cite the page numbers.").as_deref(),
            Some("From now on, cite the page numbers.")
        );
        assert_eq!(
            note("Always use British spelling when you answer").as_deref(),
            Some("Always use British spelling when you answer")
        );
        assert_eq!(
            note("Never use bullet points in your answers.").as_deref(),
            Some("Never use bullet points in your answers.")
        );
        // "Always" about something else is not an instruction to Shodh.
        assert_eq!(note("Always the same result with SPANN"), None);
    }

    #[test]
    fn a_separate_question_is_left_for_the_answer() {
        let request = memory_request(
            "Remember that I care about memory cost. How does SPANN compare with DiskANN?",
        )
        .unwrap();
        assert_eq!(request.note, "I care about memory cost.");
        assert_eq!(
            request.rest.as_deref(),
            Some("How does SPANN compare with DiskANN?")
        );
        assert_eq!(request.evidence, "Remember that I care about memory cost.");
    }

    #[test]
    fn the_note_is_the_first_sentence_and_the_rest_is_the_question() {
        let request =
            memory_request("Remember that I use DiskANN. Summarise the SPANN paper.").unwrap();
        assert_eq!(request.note, "I use DiskANN.");
        assert_eq!(request.rest.as_deref(), Some("Summarise the SPANN paper."));
        assert_eq!(request.evidence, "Remember that I use DiskANN.");
        let request = memory_request(
            "From now on, cite page numbers! Then compare SPANN with DiskANN, e.g. on recall.",
        )
        .unwrap();
        assert_eq!(request.note, "From now on, cite page numbers!");
        assert_eq!(
            request.rest.as_deref(),
            Some("Then compare SPANN with DiskANN, e.g. on recall.")
        );
        // Abbreviations, initials and decimals do not end the note.
        let request =
            memory_request("Remember that my advisor is Prof. R. K. Rao, and I use 3.5 GB.")
                .unwrap();
        assert_eq!(
            request.note,
            "my advisor is Prof. R. K. Rao, and I use 3.5 GB."
        );
        assert_eq!(request.rest, None);
    }

    #[test]
    fn questions_reminders_and_quotes_are_not_requests() {
        for text in [
            "Do you remember the paper about SPANN?",
            "Can you summarise section 4? Remember to cite the pages.",
            "Remember to cite the pages when you summarise this.",
            "Remember when we discussed FreshDiskANN?",
            "Remember what I said about recall?",
            "Remember that paper on graph indexes?",
            "\"Remember that the index is immutable\" - is this claim in the paper?",
            "\u{201c}Keep in mind the cost\u{201d}, the paper says. What does it mean?",
            "Note the differences between DiskANN and SPANN",
            "Notes on chapter 3, please",
            "Remembering the past is hard",
            "What should I remember about SPANN?",
            "Don't forget to add a chart",
            "Remember",
            "Remember that ok",
        ] {
            assert_eq!(memory_request(text), None, "{text}");
        }
    }
}
