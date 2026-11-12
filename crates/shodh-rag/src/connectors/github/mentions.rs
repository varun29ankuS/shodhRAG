//! @mentions in text Shodh posts to GitHub (comments, pull request bodies).
//!
//! A mention notifies its person or team, so a post written by the agent could
//! ping strangers, or be steered by planted text into pinging them. Every
//! `@login` and `@org/team` outside code is therefore shown as code
//! (`` `@login` ``), which GitHub does not notify, unless the user wrote that
//! same mention in their own message. The approval card names whoever will
//! still be notified.
//!
//! What counts as a mention follows GitHub's rules closely enough to err on
//! the side of neutralising: an `@` not glued to a word, URL or e-mail address
//! before it, then a login (letters, digits and single hyphens, at most 39
//! characters), optionally `/team`. Mentions in fenced code blocks and in
//! inline code are left alone: GitHub does not notify there.

/// Longest GitHub login.
const MAX_LOGIN_CHARS: usize = 39;

/// One mention found in a line: where it starts (the `@`), where it ends, and
/// the mention as written (`@login` or `@org/team`).
struct Found {
    start: usize,
    end: usize,
    text: String,
}

/// Whether `c`, right before an `@`, makes it part of a word, a URL, an e-mail
/// address or an escape rather than a mention. A backtick is not on the list:
/// right after a code span an `@` still notifies.
fn glued_before(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '/' | '.' | '-' | '+' | '=' | '\\' | '@')
}

fn login_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

fn team_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

/// The mention starting at byte `at` of `line` (an `@`), if it is one.
fn mention_at(line: &str, at: usize) -> Option<Found> {
    let rest = &line[at + 1..];
    let login_len: usize = rest
        .chars()
        .take_while(|c| login_char(*c))
        .map(char::len_utf8)
        .sum();
    let login = rest[..login_len].trim_end_matches('-');
    if login.is_empty()
        || login.starts_with('-')
        || login.contains("--")
        || login.chars().count() > MAX_LOGIN_CHARS
    {
        return None;
    }
    let mut end = at + 1 + login.len();
    // A login glued to an underscore (`@user_name`) is not a login.
    if line[end..].starts_with('_') {
        return None;
    }
    // `@org/team`.
    if let Some(after) = line[end..].strip_prefix('/') {
        let team_len: usize = after
            .chars()
            .take_while(|c| team_char(*c))
            .map(char::len_utf8)
            .sum();
        let team = after[..team_len].trim_end_matches(['-', '_']);
        if !team.is_empty() {
            end += 1 + team.len();
        }
    }
    Some(Found {
        start: at,
        end,
        text: line[at..end].to_string(),
    })
}

/// The mentions of one line outside its inline code spans.
fn line_mentions(line: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '`' {
            // An inline code span: a run of n backticks up to the next run of
            // exactly n. Without a closing run the backticks are plain text.
            let run = line[i..].chars().take_while(|&b| b == '`').count();
            let after = i + run;
            let close = find_run(&line[after..], run).map(|at| after + at);
            let skip_to = close.map_or(after, |at| at + run);
            while chars.peek().is_some_and(|(j, _)| *j < skip_to) {
                chars.next();
            }
            previous = Some('`');
            continue;
        }
        if c == '@' && !previous.is_some_and(glued_before) {
            if let Some(mention) = mention_at(line, i) {
                let end = mention.end;
                found.push(mention);
                while chars.peek().is_some_and(|(j, _)| *j < end) {
                    chars.next();
                }
                previous = line[..end].chars().next_back();
                continue;
            }
        }
        previous = Some(c);
    }
    found
}

/// Byte offset in `text` of the first run of exactly `n` backticks.
fn find_run(text: &str, n: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if run == n {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// Whether `line` opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = line.trim_start_matches(' ');
    indent <= 3 && (rest.starts_with("```") || rest.starts_with("~~~"))
}

/// Every mention in `text` that GitHub would notify (outside code), as
/// written, in order.
pub fn mentions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.split('\n') {
        if is_fence(line) {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            out.extend(line_mentions(line).into_iter().map(|m| m.text));
        }
    }
    out
}

/// The mentions the user asked for: those in `user_words` (their own message),
/// lower-cased.
fn wanted(user_words: Option<&str>) -> Vec<String> {
    user_words
        .map(|words| {
            mentions(words)
                .into_iter()
                .map(|m| m.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// `text` as Shodh posts it: every mention shown as code unless the user wrote
/// it in `user_words`, with the mentions that still notify (each once, as
/// first written).
pub fn neutralise(text: &str, user_words: Option<&str>) -> (String, Vec<String>) {
    let keep = wanted(user_words);
    let mut kept: Vec<String> = Vec::new();
    let mut out = String::with_capacity(text.len() + 8);
    let mut fenced = false;
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if is_fence(line) {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced {
            out.push_str(line);
            continue;
        }
        let mut at = 0;
        for mention in line_mentions(line) {
            out.push_str(&line[at..mention.start]);
            if keep.contains(&mention.text.to_lowercase()) {
                if !kept.iter().any(|k| k.eq_ignore_ascii_case(&mention.text)) {
                    kept.push(mention.text.clone());
                }
                out.push_str(&mention.text);
            } else {
                out.push('`');
                out.push_str(&mention.text);
                out.push('`');
            }
            at = mention.end;
        }
        out.push_str(&line[at..]);
    }
    (out, kept)
}

/// What the approval card says about the mentions that notify, if any:
/// "This will notify @a and @org/team."
pub fn notify_words(kept: &[String]) -> Option<String> {
    match kept {
        [] => None,
        [one] => Some(format!("This will notify {one}.")),
        [rest @ .., last] => Some(format!("This will notify {} and {last}.", rest.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_the_user_did_not_write_are_shown_as_code() {
        let (text, kept) = neutralise(
            "Thanks @alice and @acme/reviewers, cc @bob.",
            Some("comment that the fix is in"),
        );
        assert_eq!(text, "Thanks `@alice` and `@acme/reviewers`, cc `@bob`.");
        assert!(kept.is_empty());
        assert_eq!(mentions(&text), Vec::<String>::new());
        assert_eq!(notify_words(&kept), None);
    }

    #[test]
    fn a_mention_the_user_wrote_stays_and_is_named_on_the_card() {
        let (text, kept) = neutralise(
            "@Alice can you take a look? Also @bob and @ACME/Reviewers. @alice again.",
            Some("comment and ask @alice and @acme/reviewers to review"),
        );
        assert_eq!(
            text,
            "@Alice can you take a look? Also `@bob` and @ACME/Reviewers. @alice again."
        );
        assert_eq!(kept, ["@Alice", "@ACME/Reviewers"]);
        assert_eq!(
            notify_words(&kept).as_deref(),
            Some("This will notify @Alice and @ACME/Reviewers.")
        );
        assert_eq!(
            notify_words(&["@a".into(), "@b".into(), "@c/d".into()]).as_deref(),
            Some("This will notify @a, @b and @c/d.")
        );
    }

    #[test]
    fn code_emails_urls_and_non_logins_are_left_alone() {
        let body = "Mail me@example.com or see https://medium.com/@writer.\n\
                    Inline `@inline` and ``code with ` and @two`` stay.\n\
                    ```\n\
                    @fenced in a block\n\
                    ```\n\
                    ~~~rust\n\
                    let x = \"@tilde\";\n\
                    ~~~\n\
                    An @ alone, @-dash, @a--b, @x_y and \\@escaped.";
        let (text, kept) = neutralise(body, None);
        assert_eq!(text, body);
        assert!(kept.is_empty());
    }

    #[test]
    fn mentions_at_line_starts_after_punctuation_and_after_code_are_caught() {
        let (text, _) = neutralise(
            "@lead\n(@one) \"@two\" - `x` @three\n* @four-five: done",
            None,
        );
        assert_eq!(
            text,
            "`@lead`\n(`@one`) \"`@two`\" - `x` `@three`\n* `@four-five`: done"
        );
    }

    #[test]
    fn an_unclosed_backtick_does_not_hide_a_mention() {
        let (text, _) = neutralise("a ` stray tick then @dev", None);
        assert_eq!(text, "a ` stray tick then `@dev`");
        // A trailing hyphen is not part of a login.
        let (text, _) = neutralise("ping @dev-", None);
        assert_eq!(text, "ping `@dev`-");
    }

    #[test]
    fn the_users_message_counts_only_its_own_mentions_outside_code() {
        // The user quoting a mention in code is not asking to notify it.
        let (text, kept) = neutralise("cc @bob", Some("post `@bob` as text"));
        assert_eq!(text, "cc `@bob`");
        assert!(kept.is_empty());
    }
}
