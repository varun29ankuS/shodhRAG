//! Explicit memory requests: "Remember that I'm writing my thesis on ...", "From now on,
//! cite page numbers". The user's own words are saved as a note by the host when the
//! message arrives (a sensitive one waits for approval), before and without any model call
//! ([`super::Learner::file_user_note`]), so an unambiguous request never depends on the
//! answering model choosing to call `remember`.
//!
//! The matcher is anchored and deterministic: the message must *start* with the request
//! (after an optional "please"). A request inside a question ("Can you summarise it?
//! Remember to cite the pages."), a question about remembering ("Do you remember the
//! paper...?", "Remember when we...?"), a one-off reminder ("Remember to cite..."), a
//! pointer to something discussed ("Remember the paper we discussed"), quoted text, and a
//! note that also asks the assistant to do something ("Keep in mind the page limit and
//! summarise section 4", "Going forward, summarise section 5") are not requests to keep
//! something: when in doubt, the message is an ordinary one, answered and checked as usual.

use super::extract::{words, worth_learning};

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
    /// if anything.
    pub rest: Option<String>,
    /// A standing instruction ("From now on, ...") rather than something to keep about the
    /// user.
    pub standing: bool,
}

impl MemoryRequest {
    /// Whether the message is nothing but this request: no other sentence, not an
    /// instruction, and a statement about the user ([`worth_learning`]: in the first
    /// person, or asking to keep something). Only then is there no question to answer from
    /// the documents.
    pub fn is_all_of_the_message(&self) -> bool {
        self.rest.is_none() && !self.standing && worth_learning(&self.note)
    }
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

/// Openings of a standing instruction; the whole instruction is the note. `needs_comma`:
/// only an instruction when a comma follows ("In future work, ..." is not one).
const STANDING: [(&str, bool); 4] = [
    ("from now on", false),
    ("going forward", true),
    ("in future", true),
    ("in the future", true),
];

/// Words after "remember" that point at something discussed ("Remember the paper we
/// discussed"), which is a question about it, not something to keep.
const POINTERS: [&str; 7] = ["the", "this", "these", "those", "your", "our", "its"];

/// Phrases of a reference back to the conversation ("the paper we discussed").
const RECALLS: [&str; 7] = [
    "we discussed",
    "we talked about",
    "you said",
    "you mentioned",
    "i mentioned",
    "last time",
    "earlier",
];

/// Verbs of a request to the assistant ("summarise section 4", "explain HNSW").
const REQUEST_VERBS: [&str; 38] = [
    "summarise",
    "summarize",
    "explain",
    "compare",
    "list",
    "show",
    "find",
    "draw",
    "write",
    "describe",
    "give",
    "tell",
    "cite",
    "search",
    "look",
    "check",
    "read",
    "open",
    "translate",
    "plot",
    "chart",
    "analyse",
    "analyze",
    "review",
    "outline",
    "extract",
    "calculate",
    "compute",
    "make",
    "create",
    "generate",
    "answer",
    "help",
    "suggest",
    "recommend",
    "rewrite",
    "fix",
    "add",
];

/// Words that may come before a request at the start of a clause ("and then summarise").
const CLAUSE_LINKS: [&str; 7] = ["and", "then", "also", "please", "now", "just", "but"];

/// Words before a request verb that make it a request ("and explain", "when you summarise").
const REQUEST_LEADS: [&str; 4] = ["and", "then", "please", "you"];

/// Things in the user's documents ("the SPANN paper", "the appendix").
const DOCUMENT_NOUNS: [&str; 9] = [
    "paper",
    "papers",
    "pdf",
    "pdfs",
    "document",
    "documents",
    "appendix",
    "abstract",
    "article",
];

/// Parts of a document that are cited by number ("section 4", "page 12").
const NUMBERED_PARTS: [&str; 9] = [
    "section", "sections", "chapter", "page", "pages", "figure", "fig", "table", "equation",
];

/// Words before a document noun that make it a particular document ("the SPANN paper").
const DEFINITE: [&str; 6] = ["the", "this", "that", "these", "those", "its"];

/// Whether `text` asks the assistant to do something: a clause that starts with a request
/// verb ("compare SPANN and DiskANN"), or a request verb after "and", "then", "please" or
/// "you" ("... and explain HNSW", "... when you summarise the paper").
fn asks_the_assistant(text: &str) -> bool {
    text.split([',', ';', ':']).any(|clause| {
        let words = words(clause);
        let opens = words
            .iter()
            .find(|w| !CLAUSE_LINKS.contains(&w.as_str()))
            .is_some_and(|w| REQUEST_VERBS.contains(&w.as_str()));
        opens
            || words.windows(2).any(|pair| {
                REQUEST_LEADS.contains(&pair[0].as_str())
                    && REQUEST_VERBS.contains(&pair[1].as_str())
            })
    })
}

/// Whether `text` names a particular document or a numbered part of one ("the SPANN
/// paper", "section 4"): something the documents answer, not something to keep.
fn names_a_document(text: &str) -> bool {
    let words = words(text);
    words.iter().enumerate().any(|(i, word)| {
        let numbered = NUMBERED_PARTS.contains(&word.as_str())
            && words
                .get(i + 1)
                .is_some_and(|n| n.starts_with(|c: char| c.is_ascii_digit()));
        let definite = DOCUMENT_NOUNS.contains(&word.as_str())
            && words[i.saturating_sub(2)..i]
                .iter()
                .any(|w| DEFINITE.contains(&w.as_str()));
        numbered || definite
    })
}

/// Question words: a note with one asks something ("Remember that I care about latency and
/// how does DiskANN compare").
const WH_WORDS: [&str; 9] = [
    "what", "which", "how", "why", "where", "when", "who", "whom", "whose",
];

/// Whether `text` holds a question word.
fn has_question_word(text: &str) -> bool {
    // "what's new" holds "what".
    words(text)
        .iter()
        .filter_map(|w| w.split('\'').next())
        .any(|w| WH_WORDS.contains(&w))
}

/// Whether `text` goes on, after a `,` `;` `:` `—` `–` or a "so" or "but", with a clause
/// that is not about the user ("I'm a beginner, so explain HNSW simply", "I hate jargon —
/// keep it short"). "... vector search, and memory cost matters most to me" goes on about
/// the user and is kept.
fn has_other_clause(text: &str) -> bool {
    let lower = text.to_lowercase();
    let clauses: Vec<&str> = lower
        .split([',', ';', ':', '\u{2014}', '\u{2013}'])
        .flat_map(|part| part.split(" so "))
        .flat_map(|part| part.split(" but "))
        .collect();
    clauses
        .iter()
        .skip(1)
        .filter(|clause| !words(clause).is_empty())
        .any(|clause| !worth_learning(clause))
}

/// Whether `text` refers back to the conversation ("the paper we discussed").
fn recalls_the_conversation(text: &str) -> bool {
    let joined = format!(" {} ", words(text).join(" "));
    RECALLS
        .iter()
        .any(|phrase| joined.contains(&format!(" {phrase} ")))
}

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
    let (note, rest, standing) = match keep_request(start) {
        Some((note, rest)) => (note, rest, false),
        None => {
            let (note, rest) = standing_instruction(start)?;
            (note, rest, true)
        }
    };
    let note = note.trim_end();
    if note.chars().filter(|c| c.is_alphanumeric()).count() < 3
        || note.chars().count() > MAX_NOTE_CHARS
        || names_a_document(note)
        || recalls_the_conversation(note)
        || (!standing
            && (asks_the_assistant(note) || has_question_word(note) || has_other_clause(note)))
    {
        return None;
    }
    // `note` is a part of `start`: the evidence runs from the request to the note's end.
    let note_end = note.as_ptr() as usize - start.as_ptr() as usize + note.len();
    Some(MemoryRequest {
        note: note.to_string(),
        evidence: start[..note_end].trim().to_string(),
        rest: rest.map(str::to_string),
        standing,
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
        let first = first_word(body);
        if !that && NOT_KEPT_AFTER.contains(&first.as_str()) {
            return None;
        }
        if opening == "remember" && !that && POINTERS.contains(&first.as_str()) {
            return None;
        }
        return first_sentence(body);
    }
    None
}

/// "From now on, ..." or "Always/Never ... when you answer": the whole instruction.
fn standing_instruction(text: &str) -> Option<(&str, Option<&str>)> {
    let standing = STANDING.iter().any(|(opening, needs_comma)| {
        after(text, opening).is_some_and(|rest| !needs_comma || rest.trim_start().starts_with(','))
    });
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
    fn a_note_that_also_asks_for_something_is_not_a_request_to_remember() {
        for text in [
            "Keep in mind the page limit and summarise section 4 of the SPANN paper",
            "Don't forget the appendix when you summarise the paper",
            "Please note that I'm a beginner and explain HNSW",
            "Note: compare SPANN and DiskANN on recall",
            "Going forward, summarise section 5",
            "In future work section, list the open problems",
            "In future work, list the open problems",
            "Remember the paper we discussed. What were its findings?",
            "Remember that the SPANN paper keeps centroids in memory",
            "Remember that table 3 has the recall numbers",
            "Keep in mind what you said earlier about recall",
            "From now on, cite page numbers and summarise the SPANN paper",
        ] {
            assert_eq!(memory_request(text), None, "{text}");
        }
    }

    #[test]
    fn a_note_that_goes_on_with_a_task_or_a_question_files_no_card() {
        for text in [
            "Remember that I'm a beginner, so explain HNSW simply",
            "Remember that I'm a beginner so walk me through how HNSW search works",
            "Remember I work on DiskANN \u{2014} what's new in FreshDiskANN",
            "Remember that I care about latency and how does DiskANN compare on SSDs",
            "Note that I'm reviewing for NeurIPS, which papers in my library cover graph indexes",
            "Keep in mind that I only have 10 minutes, what are the key results",
            "Don't forget that I hate jargon \u{2014} keep the explanation of IVF short",
            "Remember that I want the answer in Hindi, what is product quantization",
        ] {
            assert_eq!(memory_request(text), None, "{text}");
        }
        // Going on about the user is kept.
        assert!(memory_request(
            "Remember that I'm writing my thesis on disk-based vector search, and memory cost matters most to me."
        )
        .is_some());
        assert!(
            memory_request("Remember that my advisor is Prof. R. K. Rao, and I use 3.5 GB.")
                .is_some()
        );
    }

    #[test]
    fn only_a_statement_about_the_user_is_all_of_a_message() {
        let all = |text: &str| memory_request(text).unwrap().is_all_of_the_message();
        assert!(all(
            "Remember that I'm writing my thesis on disk-based vector search, and memory cost matters most to me."
        ));
        assert!(all("Note: I review papers on Fridays"));
        // Anything else and the message is answered (and checked) as usual.
        assert!(!all(
            "Remember that I use DiskANN. Summarise the SPANN paper."
        ));
        assert!(!all("From now on, cite the page numbers."));
        assert!(!all("Always use British spelling when you answer"));
        assert!(!all("Remember that the deadline is Friday."));
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
