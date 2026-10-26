//! Answers as Telegram shows them: HTML parse mode
//! (<https://core.telegram.org/bots/api#html-style>), where only `&`, `<` and `>` need
//! escaping, and at most 4 096 characters per message.
//!
//! The order matters: the plain text is cut into parts first (at paragraphs, then lines,
//! then words), each part is escaped, then the few Markdown marks the model writes become
//! tags. So no tag is ever cut in two, and the model's text can never open a tag or a
//! link of its own. Lengths are counted in UTF-16 units, as Telegram counts them, with a
//! margin for the tags.
//!
//! Citations name the document's title and page, never a path; diagram code is replaced
//! by a note; everything is sanitized ([`sanitize`]) before it leaves.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::connectors::slack::answer::{
    cited_numbers, strip_diagrams_with, truncate_chars, AnswerPassage, QUOTE_CHARS,
};
use crate::connectors::slack::sanitize::{sanitize, PrivateTerms};

/// Telegram's limit for one message.
pub const TELEGRAM_LIMIT: usize = 4_096;
/// The size a part is cut to, in UTF-16 units: room for the tags and escapes added after.
pub const PART_UNITS: usize = 3_500;
/// The most parts one answer is sent in; the rest is left out, with a note.
pub const MAX_PARTS: usize = 4;
/// What replaces a diagram.
pub const DIAGRAM_NOTE: &str = "(A diagram was left out: Telegram can't show diagrams.)";
/// The note at the end of an answer cut to [`MAX_PARTS`].
pub const CUT_NOTE: &str =
    "(The answer was longer; the rest was left out. Ask for the part you need.)";

/// Length as Telegram counts it.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Escapes the three characters HTML parse mode reads.
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn regex(cell: &'static OnceLock<Option<Regex>>, pattern: &str) -> Option<&'static Regex> {
    cell.get_or_init(|| Regex::new(pattern).ok()).as_ref()
}

/// Escaped `text` with the model's Markdown as Telegram tags: fenced code → `<pre>`,
/// `code` → `<code>`, `**bold**` and headings → `<b>`. Nothing else becomes a tag.
pub fn markdown_to_html(text: &str) -> String {
    static FENCE: OnceLock<Option<Regex>> = OnceLock::new();
    static CODE: OnceLock<Option<Regex>> = OnceLock::new();
    static BOLD: OnceLock<Option<Regex>> = OnceLock::new();
    static HEADING: OnceLock<Option<Regex>> = OnceLock::new();
    let mut out = escape_html(text);
    // Code first, so what is inside it is never read as bold.
    let mut blocks: Vec<String> = Vec::new();
    if let Some(re) = regex(&FENCE, r"(?s)```[A-Za-z0-9_+-]*\n?(.*?)```") {
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                blocks.push(format!("<pre>{}</pre>", c[1].trim_end_matches('\n')));
                format!("\u{0}{}\u{0}", blocks.len() - 1)
            })
            .into_owned();
    }
    if let Some(re) = regex(&CODE, r"`([^`\n]+)`") {
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                blocks.push(format!("<code>{}</code>", &c[1]));
                format!("\u{0}{}\u{0}", blocks.len() - 1)
            })
            .into_owned();
    }
    if let Some(re) = regex(&HEADING, r"(?m)^#{1,6}\s+(.+?)\s*#*\s*$") {
        out = re.replace_all(&out, "**$1**").into_owned();
    }
    if let Some(re) = regex(&BOLD, r"\*\*([^*\n]+)\*\*") {
        out = re.replace_all(&out, "<b>$1</b>").into_owned();
    }
    for (i, block) in blocks.iter().enumerate() {
        out = out.replace(&format!("\u{0}{i}\u{0}"), block);
    }
    out
}

/// `text` cut into parts of at most `max` UTF-16 units: at a blank line when one is in
/// the second half of a part, else at a line end, else at a space, else anywhere (at a
/// character boundary).
pub fn split_text(text: &str, max: usize) -> Vec<String> {
    let max = max.max(16);
    let mut parts = Vec::new();
    let mut rest = text.trim();
    while utf16_len(rest) > max {
        // The longest prefix within `max` units.
        let mut units = 0;
        let mut end = 0;
        for (i, c) in rest.char_indices() {
            if units + c.len_utf16() > max {
                break;
            }
            units += c.len_utf16();
            end = i + c.len_utf8();
        }
        let window = &rest[..end];
        let half = window.len() / 2;
        let cut = window
            .rfind("\n\n")
            .filter(|&at| at >= half)
            .or_else(|| window.rfind('\n').filter(|&at| at >= half))
            .or_else(|| window.rfind(' ').filter(|&at| at >= half))
            .unwrap_or(end);
        let cut = if cut == 0 { end } else { cut };
        parts.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        parts.push(rest.to_string());
    }
    parts
}

/// The start of a passage, for the sources.
fn quote(text: &str) -> String {
    truncate_chars(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        QUOTE_CHARS,
    )
}

/// The sources an answer cites: `[n] Title, p. 3 — "quote…"`, plain text.
pub fn sources_block(answer: &str, passages: &[AnswerPassage]) -> Option<String> {
    let cited: BTreeSet<u32> = cited_numbers(answer);
    let lines: Vec<String> = passages
        .iter()
        .filter(|p| cited.contains(&p.n))
        .map(|p| {
            let page = p
                .page
                .as_deref()
                .map(|pg| format!(", p. {pg}"))
                .unwrap_or_default();
            format!("[{}] {}{page} — “{}”", p.n, p.title.trim(), quote(&p.text))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// One message of an answer, ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Telegram HTML.
    pub html: String,
    /// The same without tags, sent when Telegram cannot read the HTML.
    pub plain: String,
}

/// An answer as the messages Telegram gets: diagrams replaced, sanitized, cut into
/// parts, the cited sources after the text (in bold "Sources"), at most [`MAX_PARTS`].
pub fn answer_parts(answer: &str, passages: &[AnswerPassage], private: &PrivateTerms) -> Vec<Part> {
    let body = sanitize(&strip_diagrams_with(answer.trim(), DIAGRAM_NOTE), private);
    let sources = sources_block(answer, passages).map(|s| sanitize(&s, private));
    let mut texts = split_text(&body, PART_UNITS);
    let mut parts: Vec<Part> = Vec::new();
    if texts.len() > MAX_PARTS {
        texts.truncate(MAX_PARTS);
        if let Some(last) = texts.last_mut() {
            last.push_str("\n\n");
            last.push_str(CUT_NOTE);
        }
    }
    for text in texts {
        parts.push(Part {
            html: markdown_to_html(&text),
            plain: text,
        });
    }
    if let Some(sources) = sources {
        for (i, chunk) in split_text(&sources, PART_UNITS).into_iter().enumerate() {
            let (html_head, plain_head) = if i == 0 {
                ("<b>Sources</b>\n", "Sources\n")
            } else {
                ("", "")
            };
            let html = format!("{html_head}{}", escape_html(&chunk));
            let plain = format!("{plain_head}{chunk}");
            // Short sources go with the last part of the text when they fit.
            match parts.last_mut() {
                Some(last)
                    if i == 0 && utf16_len(&last.html) + utf16_len(&html) + 2 <= TELEGRAM_LIMIT =>
                {
                    last.html = format!("{}\n\n{html}", last.html);
                    last.plain = format!("{}\n\n{plain}", last.plain);
                }
                _ => parts.push(Part { html, plain }),
            }
        }
    }
    parts
}

/// A short message (a refusal, a confirmation) as one part, sanitized and escaped.
pub fn note_part(text: &str, private: &PrivateTerms) -> Part {
    let plain = sanitize(text.trim(), private);
    Part {
        html: escape_html(&plain),
        plain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passage(n: u32, title: &str, page: Option<&str>, text: &str) -> AnswerPassage {
        AnswerPassage {
            n,
            title: title.into(),
            page: page.map(str::to_string),
            text: text.into(),
        }
    }

    #[test]
    fn html_is_escaped_before_marks_become_tags() {
        assert_eq!(
            escape_html("a < b && c > d"),
            "a &lt; b &amp;&amp; c &gt; d"
        );
        assert_eq!(
            markdown_to_html("**Rent** is <b>due</b> & `x<y` [1]"),
            "<b>Rent</b> is &lt;b&gt;due&lt;/b&gt; &amp; <code>x&lt;y</code> [1]"
        );
        assert_eq!(markdown_to_html("## Terms\nok"), "<b>Terms</b>\nok");
        assert_eq!(
            markdown_to_html("```rust\nlet a = **b**;\n```"),
            "<pre>let a = **b**;</pre>",
            "nothing inside code becomes bold"
        );
        // The model cannot make a link or any tag of its own.
        let html = markdown_to_html(r#"<a href="https://evil.example">click</a>"#);
        assert!(!html.contains("<a"), "{html}");
    }

    #[test]
    fn long_text_is_cut_at_paragraphs_within_telegrams_count() {
        let para = "word ".repeat(300); // 1 500 units
        let text = format!("{para}\n\n{para}\n\n{para}");
        let parts = split_text(&text, 3_500);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| utf16_len(p) <= 3_500));
        assert!(parts[1].starts_with("word"));
        // Emoji are two units each: counted so.
        let emoji = "😀".repeat(3_000);
        let parts = split_text(&emoji, 3_500);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| utf16_len(p) <= 3_500));
        assert_eq!(parts.concat(), emoji, "nothing lost, no character split");
        assert_eq!(split_text("  short  ", 3_500), vec!["short".to_string()]);
        assert!(split_text("", 3_500).is_empty());
    }

    #[test]
    fn an_answer_cites_titles_and_pages_never_paths_and_fits() {
        let private = PrivateTerms {
            home_dirs: vec![r"C:\Users\Ada Lovelace".into()],
            user_names: vec!["adal".into()],
        };
        let passages = vec![
            passage(
                1,
                "Office lease 2026",
                Some("3"),
                "The notice period is sixty days.",
            ),
            passage(2, "Unused", None, "x"),
        ];
        let parts = answer_parts(
            "Sixty days [1], see C:\\Users\\Ada Lovelace\\lease.pdf\n```mermaid\ngraph TD\nA-->B\n```",
            &passages,
            &private,
        );
        assert_eq!(parts.len(), 1);
        let html = &parts[0].html;
        assert!(
            html.contains(
                "<b>Sources</b>\n[1] Office lease 2026, p. 3 — “The notice period is sixty days.”"
            ),
            "{html}"
        );
        assert!(!html.contains("Unused"), "only cited passages");
        assert!(!html.contains("Lovelace"), "{html}");
        assert!(html.contains("Telegram can't show diagrams"));
        assert!(!html.contains("graph TD"));
        assert!(!parts[0].plain.contains('<'));
    }

    #[test]
    fn a_very_long_answer_is_split_and_then_cut_with_a_note() {
        let long = "sentence. ".repeat(2_000); // 20 000 units
        let parts = answer_parts(&long, &[], &PrivateTerms::default());
        assert_eq!(parts.len(), MAX_PARTS);
        assert!(parts.iter().all(|p| utf16_len(&p.html) <= TELEGRAM_LIMIT));
        assert!(parts.last().unwrap().plain.ends_with(CUT_NOTE));
    }
}
