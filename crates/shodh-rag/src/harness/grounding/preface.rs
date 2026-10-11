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

/// Held text that is released even without a paragraph break, so a turn
/// written as one long paragraph streams after its opening sentences.
pub const MAX_HELD_CHARS: usize = 600;
/// Paragraphs of preface dropped at most before the rest is released as is.
pub const MAX_PREFACE_PARAGRAPHS: usize = 3;

/// Holds the opening of one message of a follow-up turn until its preface
/// can be told apart from its answer.
#[derive(Debug, Clone, Default)]
pub struct OpeningGate {
    held: String,
    open: bool,
    dropped: usize,
}

impl OpeningGate {
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
            let opening = preface_end(&self.held[..end], partial);
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
    let mut offset = 0;
    for (start, end) in sentences(paragraph, partial) {
        if !is_preface(&paragraph[start..end]) {
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
