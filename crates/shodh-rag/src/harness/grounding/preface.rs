//! The opening of a follow-up turn.
//!
//! A follow-up turn replies to the app's check prompt ([`super::followup`]),
//! and models open it by talking to the app rather than the user: "I
//! apologize for the error in my previous response. Let me provide a
//! corrected answer with proper citations." The prompt asks for no such
//! preface, and models write one anyway (Nemotron did on the answers eval), so
//! the text of a follow-up turn is held until its first paragraph is complete,
//! the sentences that open it addressing the app are dropped, and the rest
//! streams as the model wrote it ([`OpeningGate`]). Nothing is shown and then
//! taken back: what is dropped never reaches the transcript.
//!
//! A sentence counts as preface only when it carries no citation and no
//! number and reads as an apology, an acknowledgement, a reference to the
//! earlier answer or the check, or an announcement of the revision. Only the
//! sentences before the first sentence of the answer itself are dropped, in at
//! most [`MAX_PREFACE_PARAGRAPHS`] paragraphs.
//!
//! The answer that follows a model's searches opens the same way, addressed
//! to nobody: "Based on the documents I've found, I can now provide a
//! comprehensive answer." The earlier messages' working notes are set apart
//! (`TextInterim`), but this sentence opens the answer itself. The system
//! prompt already asks for the answer first, and free models write it anyway,
//! so a message written after a tool call passes a gate that drops such
//! narration ([`is_narration`]): a sentence with no citation, no number and
//! no word of the subject that announces the answer. A sentence that says
//! anything about the subject ("Based on the documents I found, here's how
//! SPANN keeps search fast:") is kept as written.

/// Held text that is released even without a paragraph break, so a turn
/// written as one long paragraph streams after its opening sentences.
pub const MAX_HELD_CHARS: usize = 600;
/// Paragraphs of preface dropped at most before the rest is released as is.
pub const MAX_PREFACE_PARAGRAPHS: usize = 3;

/// Holds the opening of one message until the sentences that open it
/// without being part of the answer (a follow-up turn's preface, an
/// answer's narration) can be told apart from the answer.
#[derive(Debug, Clone)]
pub struct OpeningGate {
    held: String,
    open: bool,
    dropped: usize,
    /// Whether a sentence is dropped.
    judge: fn(&str) -> bool,
}

impl Default for OpeningGate {
    fn default() -> Self {
        Self::preface()
    }
}

impl OpeningGate {
    fn with_judge(judge: fn(&str) -> bool) -> Self {
        Self {
            held: String::new(),
            open: false,
            dropped: 0,
            judge,
        }
    }

    /// For a message of a follow-up turn: drops its preface to the app and
    /// any narration.
    pub fn preface() -> Self {
        Self::with_judge(|s| is_preface(s) || is_narration(s))
    }

    /// For a message written after a tool call: drops narration only.
    pub fn narration() -> Self {
        Self::with_judge(is_narration)
    }

    /// Takes a streamed delta; returns the text to show now (empty while the
    /// opening is held).
    pub fn push(&mut self, delta: &str) -> String {
        if self.open {
            return delta.to_string();
        }
        self.held.push_str(delta);
        self.release(false)
    }

    /// The message's text ended (a tool call, the end of the message or of
    /// the turn): returns what is still held, without its preface.
    pub fn finish(&mut self) -> String {
        if self.open {
            return String::new();
        }
        self.release(true)
    }

    fn release(&mut self, done: bool) -> String {
        loop {
            if self.dropped > 0 {
                // The break after a dropped paragraph is not part of the answer.
                let start = self.held.len() - self.held.trim_start().len();
                self.held.drain(..start);
            }
            let boundary = self.held.find("\n\n");
            let end = boundary.unwrap_or(self.held.len());
            // Without a break and before the end, the last sentence may still
            // be streaming: only finished sentences are judged.
            let partial = boundary.is_none() && !done;
            let opening = opening_end(&self.held[..end], partial, self.judge);
            if !opening.answer_found {
                if partial && self.held.chars().count() < MAX_HELD_CHARS {
                    return String::new();
                }
                if boundary.is_some() && self.dropped < MAX_PREFACE_PARAGRAPHS && end > 0 {
                    self.held.drain(..end);
                    self.dropped += 1;
                    continue;
                }
            }
            let kept_from = opening.offset;
            self.open = true;
            let rest = self.held.split_off(kept_from);
            let had_preface = !self.held.trim().is_empty() || self.dropped > 0;
            self.held.clear();
            return if had_preface {
                rest.trim_start().to_string()
            } else {
                rest
            };
        }
    }
}

/// Where the answer begins in a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opening {
    /// Byte offset after the leading preface sentences.
    pub offset: usize,
    /// A sentence of the answer itself was found at `offset`; otherwise every
    /// sentence judged so far is preface.
    pub answer_found: bool,
}

/// Where the answer begins in `paragraph`: after its leading preface
/// sentences. With `partial`, a last sentence without its closing
/// punctuation is not judged yet.
pub fn preface_end(paragraph: &str, partial: bool) -> Opening {
    opening_end(paragraph, partial, is_preface)
}

/// Where the answer begins in `paragraph`: after its leading sentences that
/// `judge` drops.
fn opening_end(paragraph: &str, partial: bool, judge: fn(&str) -> bool) -> Opening {
    let mut offset = 0;
    for (start, end) in sentences(paragraph, partial) {
        if !judge(&paragraph[start..end]) {
            return Opening {
                offset: start,
                answer_found: true,
            };
        }
        offset = end;
    }
    Opening {
        offset,
        answer_found: false,
    }
}

/// Sentence spans of `text`: each ends after `.`, `!`, `?` or `:` followed by
/// whitespace, or at the end of the text (unless `partial`).
fn sentences(text: &str, partial: bool) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '.' | '!' | '?' | ':') {
            let next_is_space = chars.peek().is_some_and(|(_, n)| n.is_whitespace());
            if next_is_space {
                let end = i + c.len_utf8();
                spans.push((start, end));
                start = end;
            }
        }
    }
    if start < text.len() {
        let tail_closed = text[start..].trim_end().ends_with(['.', '!', '?', ':']);
        if !partial || tail_closed {
            spans.push((start, text.len()));
        }
    }
    spans
}

/// Apologies are preface only when they are about the earlier output or are
/// nothing but an apology: "I apologize, but the documents do not cover the
/// fee" is how a model says a part is missing, which is the answer.
const APOLOGIES: &[&str] = &[
    "i apologize",
    "i apologise",
    "apologies",
    "my apologies",
    "sorry",
    "i'm sorry",
    "i am sorry",
];
/// What an apology for the earlier output is for.
const APOLOGY_FOR: &[&str] = &[
    "error",
    "errors",
    "mistake",
    "mistakes",
    "confusion",
    "oversight",
    "omission",
    "inaccuracy",
    "inaccuracies",
    "misunderstanding",
    "incorrect",
    "previous",
    "earlier",
    "prior",
    "above",
];
/// Acknowledgements of the check.
const ACKNOWLEDGEMENTS: &[&str] = &[
    "thank you for",
    "thanks for",
    "you're right",
    "you are right",
    "you're correct",
    "you are correct",
    "good catch",
];
/// Acknowledgements that are preface only as a sentence of their own.
const SHORT_ACKNOWLEDGEMENTS: &[&str] = &["understood", "i understand", "certainly", "of course"];
/// Where an apology turns to what follows it.
const CONTRASTS: &[&str] = &[
    ", but ",
    " but ",
    "; however",
    ", however",
    " however,",
    "; ",
];

const EARLIER: &[&str] = &[
    "previous", "earlier", "prior", "above", "original", "initial", "last",
];
/// What the model calls its own earlier output ("version" and "draft" are
/// left out: "the previous version of the contract" is content).
const OUTPUT: &[&str] = &["response", "answer", "reply", "attempt"];
const META_PHRASES: &[&str] = &[
    "my mistake",
    "the error in my",
    "grounding check",
    "this check",
    "the app's check",
    "flagged statement",
    "flagged claim",
    "correct that",
    "correct this",
    "fix that",
    "fix this",
];
const ANNOUNCERS: &[&str] = &[
    "here is",
    "here's",
    "below is",
    "following is",
    "let me",
    "i will",
    "i'll",
    "i have",
    "i've",
];
const REVISED: &[&str] = &[
    "revised",
    "corrected",
    "updated",
    "amended",
    "fixed",
    "accurate",
];
const ANSWER: &[&str] = &["answer", "response", "version"];

/// Whether one sentence is preface: addressed to the app, carrying nothing
/// of the answer.
pub fn is_preface(sentence: &str) -> bool {
    let text = sentence
        .trim()
        .trim_start_matches(['#', '*', '_', '>', '-', ' '])
        .replace(['\u{2019}', '\u{2018}'], "'")
        .to_lowercase();
    let text = text.trim_end_matches(['*', '_', ' ']).trim();
    if text.is_empty() {
        return true;
    }
    if text.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .collect();
    let has = |set: &[&str]| words.iter().any(|w| set.contains(w));
    if APOLOGIES.iter().any(|a| starts_with_words(text, a)) {
        // "I apologize for the error, but here is the corrected answer" is
        // preface; "I apologize, but the documents do not cover it" is not.
        return match CONTRASTS.iter().find_map(|c| text.split_once(c)) {
            Some((_, rest)) => is_preface(rest),
            None => words.len() <= 3 || has(APOLOGY_FOR),
        };
    }
    if ACKNOWLEDGEMENTS.iter().any(|a| starts_with_words(text, a)) {
        return true;
    }
    if words.len() <= 3
        && SHORT_ACKNOWLEDGEMENTS
            .iter()
            .any(|a| starts_with_words(text, a))
    {
        return true;
    }
    if words
        .windows(2)
        .any(|pair| EARLIER.contains(&pair[0]) && OUTPUT.contains(&pair[1]))
    {
        return true;
    }
    if META_PHRASES.iter().any(|p| text.contains(p)) {
        return true;
    }
    if has(REVISED) && has(ANSWER) {
        let announced = ANNOUNCERS.iter().any(|a| text.contains(a));
        return announced || words.len() <= 4;
    }
    false
}

/// How a model announces the answer it is about to write.
const NARRATION_ANNOUNCERS: &[&str] = &[
    "i can now provide",
    "i can now give",
    "i can now answer",
    "i can now explain",
    "i can now offer",
    "i can now present",
    "i can now summarize",
    "i can now summarise",
    "i can now compile",
    "i can provide",
    "i can give you",
    "i will now provide",
    "i'll now provide",
    "i will provide",
    "i'll provide",
    "let me provide",
    "let me now provide",
    "let me compile",
    "let me now compile",
    "let me put together",
    "let me summarize",
    "let me summarise",
    "let me explain",
    "let me now explain",
    "now i can provide",
    "now i can answer",
    "now i can explain",
    "i now have",
    "now i have",
    "i have enough",
    "i've enough",
    "i have gathered",
    "i've gathered",
    "i have found enough",
    "i've found enough",
    "i have sufficient",
    "here is a comprehensive",
    "here's a comprehensive",
    "here is a detailed",
    "here's a detailed",
    "here is a complete",
    "here's a complete",
    "here is a thorough",
    "here's a thorough",
];

/// Words narration is made of besides function words: the search, the
/// documents and the answer it announces. A word outside these and the
/// stopwords is about the subject, and the sentence is kept.
const NARRATION_WORDS: &[&str] = &[
    "able",
    "after",
    "answer",
    "answers",
    "available",
    "based",
    "can",
    "clear",
    "complete",
    "compile",
    "comprehensive",
    "detailed",
    "details",
    "document",
    "documents",
    "enough",
    "explain",
    "explanation",
    "file",
    "files",
    "find",
    "findings",
    "found",
    "gather",
    "gathered",
    "give",
    "good",
    "have",
    "having",
    "here",
    "i",
    "i'll",
    "i'm",
    "i've",
    "information",
    "let",
    "look",
    "looked",
    "looking",
    "me",
    "now",
    "offer",
    "overview",
    "paper",
    "papers",
    "passage",
    "passages",
    "picture",
    "present",
    "provide",
    "put",
    "query",
    "question",
    "questions",
    "relevant",
    "research",
    "result",
    "results",
    "retrieved",
    "review",
    "reviewed",
    "reviewing",
    "search",
    "searched",
    "searches",
    "searching",
    "source",
    "sources",
    "sufficient",
    "summarise",
    "summarize",
    "summary",
    "thorough",
    "together",
    "understanding",
    "will",
    "your",
];

/// Whether one sentence is narration: it announces the answer ("Based on the
/// documents I've found, I can now provide a comprehensive answer.") and
/// says nothing about the subject. Never a sentence with a citation or a
/// number.
pub fn is_narration(sentence: &str) -> bool {
    let text = sentence
        .trim()
        .trim_start_matches(['#', '*', '_', '>', '-', ' '])
        .replace(['\u{2019}', '\u{2018}'], "'")
        .to_lowercase();
    let text = text.trim_end_matches(['*', '_', ' ']).trim();
    if text.is_empty() || text.chars().any(|c| c.is_ascii_digit() || c == '[') {
        return false;
    }
    if !NARRATION_ANNOUNCERS.iter().any(|a| contains_words(text, a)) {
        return false;
    }
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .all(|w| {
            NARRATION_WORDS.contains(&w)
                || crate::harness::web::relevance::is_stopword(w)
                || w.strip_suffix("'s")
                    .is_some_and(|base| NARRATION_WORDS.contains(&base))
        })
}

/// Whether `text` contains the words `phrase` (not inside a longer word).
fn contains_words(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + phrase.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric())
            && after.is_none_or(|c| !(c.is_alphanumeric() || c == '\''))
    })
}

/// Whether `text` starts with the words `prefix` ("sorry," yes, "sorry-free" no).
fn starts_with_words(text: &str, prefix: &str) -> bool {
    text.strip_prefix(prefix).is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || matches!(c, ',' | '.' | '!' | ':' | ';'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Streams `text` through a gate in chunks of `step` characters.
    fn stream(text: &str, step: usize) -> String {
        let mut gate = OpeningGate::default();
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::new();
        for chunk in chars.chunks(step) {
            out.push_str(&gate.push(&chunk.iter().collect::<String>()));
        }
        out.push_str(&gate.finish());
        out
    }

    #[test]
    fn the_eval_apology_is_dropped_and_the_answer_kept_exactly() {
        // The opening of the msa-interest answer on the Nemotron eval run.
        let answer =
            "Late payment interest is charged at 1.5% per month on overdue amounts [2].\n\n\
                      Therefore the interest is **1.5% per month** [2].";
        let turn = format!(
            "I apologize for the error in my previous response. Let me provide a corrected \
             answer with proper citations.\n\n{answer}"
        );
        for step in [1, 3, 7, 50, 10_000] {
            assert_eq!(stream(&turn, step), answer, "chunks of {step}");
        }
    }

    #[test]
    fn preface_inside_the_first_paragraph_is_cut_at_the_answer() {
        let turn = "I apologize for the confusion. The notice period is sixty days [1]. It \
                    starts on receipt [1].";
        assert_eq!(
            stream(turn, 4),
            "The notice period is sixty days [1]. It starts on receipt [1]."
        );
        let headed = "**Revised answer:**\n\nThe fee is due within forty-five days [3].";
        assert_eq!(
            stream(headed, 5),
            "The fee is due within forty-five days [3]."
        );
    }

    #[test]
    fn an_answer_without_preface_streams_unchanged() {
        for turn in [
            "The monthly retainer is INR 2,75,000 plus GST [11].",
            "Sorry-free text: the lease runs five years [2].\n\nMore [3].",
            "  Indented first line keeps its spaces [1].",
            "Germany's law governs the agreement [4]. Its courts in Munich decide disputes.",
            "",
        ] {
            assert_eq!(stream(turn, 2), turn, "{turn:?}");
        }
    }

    #[test]
    fn content_with_a_citation_or_number_is_never_preface() {
        assert!(!is_preface("Here is the revised answer [2]."));
        assert!(!is_preface("The previous answer period was 30 days."));
        assert!(!is_preface("The updated schedule applies from March."));
        assert!(!is_preface("The last version of the policy is binding."));
        assert!(is_preface(
            "I apologize for the error in my previous response."
        ));
        assert!(is_preface(
            "Let me provide a corrected answer with proper citations."
        ));
        assert!(is_preface("Here's the corrected response:"));
        assert!(is_preface("You\u{2019}re right."));
        assert!(is_preface("Thank you for checking."));
        assert!(is_preface(
            "Based on the grounding check, I have revised the claims."
        ));
        // An apology that says what the documents lack is the answer.
        assert!(!is_preface(
            "I apologize, but the documents do not cover the termination fee."
        ));
        assert!(!is_preface(
            "I'm sorry, the agreement says nothing about renewal."
        ));
        assert!(!is_preface("I understand the lease ends in March."));
        assert!(is_preface(
            "I apologize for the error in my previous response, but here is the corrected answer:"
        ));
        assert!(is_preface("My apologies."));
        assert!(is_preface("Understood."));
    }

    #[test]
    fn a_long_paragraph_is_released_after_its_opening_sentences() {
        let body = "The tenant pays rent monthly [1]. ".repeat(40);
        let turn = format!("I apologize for the oversight. {body}");
        let mut gate = OpeningGate::default();
        let first = gate.push(&turn);
        assert!(
            first.starts_with("The tenant pays rent monthly [1]."),
            "released before the turn ends: {first:?}"
        );
        assert_eq!(first, body);
        assert_eq!(gate.push(" More."), " More.");
        assert_eq!(gate.finish(), "");
    }

    #[test]
    fn a_missing_part_said_with_an_apology_is_shown() {
        let missing = "I apologize, but the documents do not cover the termination fee.";
        assert_eq!(stream(missing, 4), missing);
        let after_error = "I apologize for the error. The documents do not cover the termination \
                           fee.\n\nThe notice period is sixty days [1].";
        assert_eq!(
            stream(after_error, 4),
            "The documents do not cover the termination fee.\n\nThe notice period is sixty days [1]."
        );
    }

    /// Streams `text` through a narration gate (a message after a tool call).
    fn stream_answer(text: &str, step: usize) -> String {
        let mut gate = OpeningGate::narration();
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::new();
        for chunk in chars.chunks(step) {
            out.push_str(&gate.push(&chunk.iter().collect::<String>()));
        }
        out.push_str(&gate.finish());
        out
    }

    #[test]
    fn the_recorded_narration_opening_an_answer_is_dropped() {
        // The opening of an answer recorded on the release build (Nemotron).
        let answer = "HNSW keeps at most M neighbours per element and prunes them with a \
                      heuristic [28].\n\nNSG approximates the MRNG [21].";
        for opening in [
            "Based on the documents I've found, I can now provide a comprehensive answer.",
            "Based on the documents I\u{2019}ve found, I can now provide a comprehensive answer \
             to your question.",
            "Now I have enough information to answer your question.",
            "I now have sufficient information. Let me provide a detailed explanation:",
        ] {
            for sep in [" ", "\n\n"] {
                let turn = format!("{opening}{sep}{answer}");
                for step in [1, 5, 40, 10_000] {
                    assert_eq!(stream_answer(&turn, step), answer, "{turn:?} in {step}s");
                }
            }
        }
        // A heading of its own.
        let turn = format!("**Here is a comprehensive overview:**\n\n{answer}");
        assert_eq!(stream_answer(&turn, 7), answer);
    }

    #[test]
    fn narration_that_says_anything_of_the_subject_is_kept() {
        for sentence in [
            "Based on the documents I found, here's how FreshDiskANN and SPANN maintain fast search performance when indexes exceed RAM capacity:",
            "I can now provide a comprehensive answer about how HNSW and NSG prune neighbours.",
            "Based on the documents I've found, the notice period is sixty days [1].",
            "I now have the 2021 results.",
            "Let me explain the pruning rule.",
            "The documents I found describe a comprehensive answer key.",
        ] {
            assert!(!is_narration(sentence), "{sentence:?}");
        }
        for turn in [
            "The monthly retainer is INR 2,75,000 plus GST [11].",
            "Based on the documents I found, here's how SPANN works:\n\n- It clusters [1].",
            "",
        ] {
            assert_eq!(stream_answer(turn, 3), turn, "{turn:?}");
        }
    }

    #[test]
    fn a_turn_that_is_only_preface_shows_nothing() {
        assert_eq!(
            stream(
                "I apologize for the error. Here is the corrected answer:",
                3
            ),
            ""
        );
        assert_eq!(
            stream(
                "I apologize.\n\nHere is the revised answer:\n\nIt is due on receipt [1].",
                6
            ),
            "It is due on receipt [1]."
        );
    }
}
