//! Readers for formats with no structure the layout parser knows: RTF, email
//! (`.eml`), Jupyter notebooks (`.ipynb`), EPUB books, and legacy Word and
//! PowerPoint (`.doc`, `.ppt`, through `office_oxide`'s binary readers).
//!
//! Each reader is bounded: inputs are already under their size limit
//! ([`super::limits`]), nesting is followed with explicit stacks or caps, never
//! unbounded recursion, and a file a reader cannot make sense of fails with a
//! plain reason ("damaged or incomplete", "protected with a password", "contains
//! no extractable text") that the Library shows for the file
//! (`app/src/features/library/indexingAdvice.ts`). The binary readers run behind
//! [`std::panic::catch_unwind`], and a compound file whose directory loops or nests
//! too deep is refused before they read it.

use std::io::Cursor;

use base64::Engine as _;

// ── RTF ───────────────────────────────────────────────────────────────────

/// Groups of an RTF file nested deeper than this are refused (Word writes a few
/// dozen levels; nesting costs two bytes a level).
pub const MAX_RTF_DEPTH: usize = 1_000;

/// Destinations whose content is never text of the document: tables of fonts,
/// colours and styles, document properties, pictures, embedded objects, field
/// instructions (their result is read), revision tables and theme data.
const RTF_SKIPPED: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "objdata",
    "fldinst",
    "listtable",
    "listoverridetable",
    "revtbl",
    "rsidtbl",
    "generator",
    "xmlnstbl",
    "themedata",
    "colorschememapping",
    "datastore",
    "latentstyles",
    "filetbl",
    "pgdsctbl",
    "mmathPr",
    "wgrffmtfilter",
    "header",
    "footer",
    "headerl",
    "headerr",
    "headerf",
    "footerl",
    "footerr",
    "footerf",
    "bkmkstart",
    "bkmkend",
    "xe",
    "tc",
];

/// Why an RTF file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum RtfError {
    #[error("this is not an RTF file (it does not start with {{\\rtf), so it may be damaged")]
    NotRtf,
    #[error("its groups nest more than {MAX_RTF_DEPTH} deep, which no RTF file does, so it may be damaged")]
    TooDeep,
}

/// The encoding of RTF's 8-bit text and `\'hh` escapes for the code page an
/// `\ansicpgN` names; Windows-1252 (RTF's default) for one this does not know.
fn rtf_code_page(page: i64) -> &'static encoding_rs::Encoding {
    let label = match page {
        874 => "windows-874".to_string(),
        932 => "shift_jis".to_string(),
        936 => "gbk".to_string(),
        949 => "euc-kr".to_string(),
        950 => "big5".to_string(),
        1250..=1258 => format!("windows-{page}"),
        10000 => "macintosh".to_string(),
        20866 => "koi8-r".to_string(),
        21866 => "koi8-u".to_string(),
        28592..=28599 | 28603..=28606 => format!("iso-8859-{}", page - 28590),
        65001 => "utf-8".to_string(),
        _ => "windows-1252".to_string(),
    };
    encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252)
}

/// The state an RTF group starts with and restores when it ends.
#[derive(Clone, Copy)]
struct RtfGroup {
    skip: bool,
    /// Hidden text (`\v`): formatting, not a destination, so `\v0` and `\plain`
    /// end it within the group.
    hidden: bool,
    /// Fallback characters after a `\uN` (`\ucN`).
    uc: usize,
}

impl RtfGroup {
    fn shows_text(&self) -> bool {
        !self.skip && !self.hidden
    }
}

/// An RTF file's text as it is read. 8-bit text is decoded in the document's
/// code page: a multibyte character (Shift-JIS `\'82\'a0`) is two escapes, so
/// bytes are gathered and decoded together when anything else comes.
struct RtfText {
    out: String,
    bytes: Vec<u8>,
    encoding: &'static encoding_rs::Encoding,
    /// A `\uN` high surrogate waiting for its low half.
    high: Option<u32>,
}

impl RtfText {
    fn flush(&mut self) {
        if !self.bytes.is_empty() {
            let (text, _) = self.encoding.decode_without_bom_handling(&self.bytes);
            self.out.push_str(&text);
            self.bytes.clear();
        }
        if self.high.take().is_some() {
            self.out.push('\u{FFFD}');
        }
    }

    fn push(&mut self, c: char, state: &RtfGroup, pending: &mut usize) {
        if *pending > 0 {
            *pending -= 1;
        } else if state.shows_text() {
            self.flush();
            self.out.push(c);
        }
    }

    fn push_byte(&mut self, b: u8, state: &RtfGroup, pending: &mut usize) {
        if *pending > 0 {
            *pending -= 1;
        } else if state.shows_text() {
            if self.high.take().is_some() {
                self.out.push('\u{FFFD}');
            }
            self.bytes.push(b);
        }
    }

    /// A `\uN` code unit: a surrogate pair's halves make one character.
    fn push_unit(&mut self, unit: u32, state: &RtfGroup) {
        if !state.shows_text() {
            return;
        }
        if (0xD800..=0xDBFF).contains(&unit) {
            self.flush();
            self.high = Some(unit);
            return;
        }
        if (0xDC00..=0xDFFF).contains(&unit) {
            let pair = self
                .high
                .take()
                .map(|high| 0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00));
            self.flush();
            self.out
                .push(pair.and_then(char::from_u32).unwrap_or('\u{FFFD}'));
            return;
        }
        self.flush();
        self.out.push(char::from_u32(unit).unwrap_or('\u{FFFD}'));
    }
}

/// The text of an RTF file: paragraphs as lines, `\tab` as a tab, Unicode
/// (`\uN`, surrogate pairs joined) and 8-bit (`\'hh`, in the code page
/// `\ansicpgN` names: Cyrillic, Greek, Shift-JIS) characters decoded, and the
/// content of every destination that is not text (fonts, styles, pictures,
/// objects, properties, headers and footers, ignorable `\*` destinations) and
/// hidden text (`\v`) left out.
pub fn rtf_text(bytes: &[u8]) -> Result<String, RtfError> {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    if !bytes[start..].starts_with(b"{\\rtf") {
        return Err(RtfError::NotRtf);
    }

    let mut text = RtfText {
        out: String::new(),
        bytes: Vec::new(),
        encoding: encoding_rs::WINDOWS_1252,
        high: None,
    };
    let mut stack: Vec<RtfGroup> = Vec::new();
    let mut state = RtfGroup {
        skip: false,
        hidden: false,
        uc: 1,
    };
    // Fallback characters still to drop after a `\uN`.
    let mut pending_fallback = 0usize;
    // The next control word opens an ignorable destination (`{\*\name`).
    let mut ignorable = false;
    // At the start of a group: its first control word may name a destination.
    let mut group_start = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'{' => {
                if stack.len() >= MAX_RTF_DEPTH {
                    return Err(RtfError::TooDeep);
                }
                text.flush();
                stack.push(state);
                group_start = true;
                ignorable = false;
                i += 1;
            }
            b'}' => {
                text.flush();
                state = stack.pop().unwrap_or(state);
                pending_fallback = 0;
                group_start = false;
                ignorable = false;
                i += 1;
            }
            b'\\' => {
                let Some(&next) = bytes.get(i + 1) else {
                    break;
                };
                if next.is_ascii_alphabetic() {
                    let word_start = i + 1;
                    let mut j = word_start;
                    while j < bytes.len() && bytes[j].is_ascii_alphabetic() && j - word_start < 32 {
                        j += 1;
                    }
                    let word = std::str::from_utf8(&bytes[word_start..j]).unwrap_or("");
                    let param_start = j;
                    if j < bytes.len() && bytes[j] == b'-' {
                        j += 1;
                    }
                    while j < bytes.len() && bytes[j].is_ascii_digit() && j - param_start < 11 {
                        j += 1;
                    }
                    let param: Option<i64> = std::str::from_utf8(&bytes[param_start..j])
                        .ok()
                        .and_then(|p| p.parse().ok());
                    // One space ends a control word and is not text.
                    if j < bytes.len() && bytes[j] == b' ' {
                        j += 1;
                    }
                    i = j;
                    // A `\u` is text; any other control word ends gathered bytes
                    // before it can change what is shown.
                    if word != "u" {
                        text.flush();
                    }
                    let opens = group_start;
                    group_start = false;
                    if (opens && RTF_SKIPPED.contains(&word))
                        || (ignorable && opens_after_star(word))
                    {
                        state.skip = true;
                    }
                    ignorable = false;
                    let mut none = 0;
                    match word {
                        "par" | "line" | "sect" | "page" | "row" => {
                            text.push('\n', &state, &mut none)
                        }
                        "tab" | "cell" => text.push('\t', &state, &mut none),
                        "emdash" => text.push('—', &state, &mut none),
                        "endash" => text.push('–', &state, &mut none),
                        "lquote" => text.push('‘', &state, &mut none),
                        "rquote" => text.push('’', &state, &mut none),
                        "ldblquote" => text.push('“', &state, &mut none),
                        "rdblquote" => text.push('”', &state, &mut none),
                        "bullet" => text.push('•', &state, &mut none),
                        "uc" => state.uc = param.unwrap_or(1).clamp(0, 16) as usize,
                        "ansicpg" => {
                            if let Some(page) = param {
                                text.encoding = rtf_code_page(page);
                            }
                        }
                        "v" => state.hidden = param != Some(0),
                        "plain" => state.hidden = false,
                        "u" => {
                            if let Some(n) = param {
                                // A signed 16-bit value: negatives are code units above 32767.
                                let unit = if n < 0 { n + 65_536 } else { n };
                                text.push_unit(u32::try_from(unit).unwrap_or(0xFFFD), &state);
                                pending_fallback = state.uc;
                            }
                        }
                        "bin" => {
                            // Binary data: skipped whole.
                            let len = usize::try_from(param.unwrap_or(0).max(0)).unwrap_or(0);
                            i = i.saturating_add(len).min(bytes.len());
                        }
                        _ => {}
                    }
                } else {
                    i += 2;
                    match next {
                        b'\'' => {
                            let hex = bytes
                                .get(i..i + 2)
                                .and_then(|h| std::str::from_utf8(h).ok());
                            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                                i += 2;
                                text.push_byte(byte, &state, &mut pending_fallback);
                            } else {
                                // A cut-short escape: its hex digits are not text.
                                let mut skipped = 0;
                                while skipped < 2 && bytes.get(i).is_some_and(u8::is_ascii_hexdigit)
                                {
                                    i += 1;
                                    skipped += 1;
                                }
                            }
                        }
                        b'*' => ignorable = true,
                        b'~' => text.push('\u{a0}', &state, &mut pending_fallback),
                        b'_' => text.push('-', &state, &mut pending_fallback),
                        b'-' => {}
                        b'\\' | b'{' | b'}' => {
                            text.push(char::from(next), &state, &mut pending_fallback)
                        }
                        b'\n' | b'\r' => text.push('\n', &state, &mut 0),
                        _ => {}
                    }
                    group_start = false;
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                group_start = false;
                if b.is_ascii() {
                    text.push(char::from(b), &state, &mut pending_fallback);
                } else {
                    text.push_byte(b, &state, &mut pending_fallback);
                }
                i += 1;
            }
        }
    }
    text.flush();
    Ok(tidy_lines(&text.out))
}

/// Whether a control word after `\*` opens a destination to skip: every ignorable
/// destination is, except the few whose text is the document's.
fn opens_after_star(word: &str) -> bool {
    !matches!(word, "shppict" | "nonshppict" | "ud" | "upr")
}

/// `text` with trailing spaces cut from each line and runs of blank lines kept to one.
fn tidy_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

// ── Email (.eml) ──────────────────────────────────────────────────────────

/// MIME parts nested deeper than this are not read.
pub const MAX_MIME_DEPTH: usize = 8;
/// The most MIME parts read from one message.
pub const MAX_MIME_PARTS: usize = 500;

/// A message's text: its headers, its body (the plain-text part, else the HTML
/// part as text) and the names of its attachments, which are not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmailText {
    pub subject: Option<String>,
    pub text: String,
    pub attachments: Vec<String>,
}

/// Header lines unfolded (a line that starts with a space continues the last),
/// and the body after the blank line.
fn split_headers(raw: &str) -> (Vec<(String, String)>, &str) {
    let (head, body) = match raw.find("\r\n\r\n") {
        Some(at) => (&raw[..at], &raw[at + 4..]),
        None => match raw.find("\n\n") {
            Some(at) => (&raw[..at], &raw[at + 2..]),
            None => (raw, ""),
        },
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = headers.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    (headers, body)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// The value of parameter `name` of a structured header (`charset`, `boundary`,
/// `filename`), unquoted.
fn header_param(value: &str, name: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|part| {
        let (key, val) = part.split_once('=')?;
        let key = key.trim();
        // RFC 2231: `name*=charset'lang'percent-encoded`.
        let extended = key.ends_with('*');
        if !key.trim_end_matches('*').eq_ignore_ascii_case(name) {
            return None;
        }
        let val = val.trim();
        if extended {
            if let Some(encoded) = val.splitn(3, '\'').nth(2) {
                return Some(percent_decode(encoded));
            }
        }
        Some(val.trim_matches('"').to_string())
    })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(byte) = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `bytes` in the charset `charset` names (UTF-8 when it names none Shodh knows).
fn decode_charset(bytes: &[u8], charset: Option<&str>) -> String {
    let encoding = charset
        .and_then(|c| encoding_rs::Encoding::for_label(c.trim().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// Quoted-printable decoded (soft line breaks joined).
fn quoted_printable(body: &str) -> Vec<u8> {
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            match bytes.get(i + 1..i + 3) {
                Some([b'\r', b'\n']) => {
                    i += 3;
                    continue;
                }
                Some([b'\n', _]) => {
                    i += 2;
                    continue;
                }
                Some(hex) => {
                    if let Some(byte) = std::str::from_utf8(hex)
                        .ok()
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                    {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                }
                None if bytes.get(i + 1) == Some(&b'\n') => {
                    i += 2;
                    continue;
                }
                None => {}
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Base64 with its line breaks and padding ignored; a body that is not base64
/// decodes to nothing.
fn decode_base64(text: &str) -> Vec<u8> {
    let compact: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .collect();
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(compact.as_bytes())
        .unwrap_or_default()
}

/// A body in its transfer encoding decoded to bytes.
fn transfer_decode(body: &str, encoding: Option<&str>) -> Vec<u8> {
    match encoding.map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        Some("base64") => decode_base64(body),
        Some("quoted-printable") => quoted_printable(body),
        _ => body.as_bytes().to_vec(),
    }
}

/// A header value with its RFC 2047 encoded words (`=?utf-8?B?...?=`,
/// `=?iso-8859-1?Q?...?=`) decoded.
fn decode_words(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        // Whitespace between two encoded words is not text.
        if !(last_was_word && before.trim().is_empty()) {
            out.push_str(before);
        }
        let word = &rest[start + 2..];
        let parts: Vec<&str> = word.splitn(3, '?').collect();
        let decoded = match parts.as_slice() {
            [charset, enc, tail] => tail.find("?=").map(|end| {
                let data = &tail[..end];
                let bytes = if enc.eq_ignore_ascii_case("b") {
                    decode_base64(data)
                } else {
                    quoted_printable(&data.replace('_', " "))
                };
                let consumed = charset.len() + enc.len() + 2 + end + 2;
                (decode_charset(&bytes, Some(charset)), consumed)
            }),
            _ => None,
        };
        match decoded {
            Some((text, consumed)) => {
                out.push_str(&text);
                rest = &word[consumed..];
                last_was_word = true;
            }
            None => {
                out.push_str("=?");
                rest = word;
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What a MIME walk collected.
#[derive(Default)]
struct MimeParts {
    plain: Vec<String>,
    html: Vec<String>,
    attachments: Vec<String>,
    parts: usize,
}

/// The largest index at most `max` that is a character boundary of `text`.
fn floor_char_boundary(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }
    (0..=max)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0)
}

fn walk_mime(headers: &[(String, String)], body: &str, depth: usize, out: &mut MimeParts) {
    out.parts += 1;
    if out.parts > MAX_MIME_PARTS || depth > MAX_MIME_DEPTH {
        return;
    }
    let content_type = header(headers, "content-type").unwrap_or("text/plain");
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let disposition = header(headers, "content-disposition").unwrap_or("");
    let name = header_param(disposition, "filename")
        .or_else(|| header_param(content_type, "name"))
        .map(|n| decode_words(&n));
    // An attachment is decided first, for every part (a forwarded message and a
    // group of parts too): one marked `attachment`, or one with a file name, is
    // listed by name and never read. Only the message's body is indexed.
    let attachment = disposition
        .trim()
        .to_ascii_lowercase()
        .starts_with("attachment")
        || name.as_deref().is_some_and(|n| !n.trim().is_empty());
    if attachment {
        out.attachments
            .push(name.unwrap_or_else(|| format!("an attachment ({mime})")));
        return;
    }
    if mime.starts_with("multipart/") {
        let Some(boundary) = header_param(content_type, "boundary").filter(|b| !b.is_empty())
        else {
            return;
        };
        let delimiter = format!("--{boundary}");
        let mut sections = body.split(delimiter.as_str());
        // Before the first delimiter: the preamble.
        sections.next();
        for section in sections {
            if section.starts_with("--") {
                break;
            }
            let section = section.trim_start_matches(['\r', '\n']);
            let (part_headers, part_body) = split_headers(section);
            walk_mime(&part_headers, part_body, depth + 1, out);
        }
        return;
    }
    if mime == "message/rfc822" {
        let (inner_headers, inner_body) = split_headers(body);
        walk_mime(&inner_headers, inner_body, depth + 1, out);
        return;
    }
    // The body: unnamed plain text and HTML. Anything else (a calendar invite, a
    // CSV, an image sent inline) is a file, listed and not read.
    if !matches!(mime.as_str(), "text/plain" | "text/html" | "") {
        out.attachments.push(format!("an attachment ({mime})"));
        return;
    }
    // A body larger than a text file may be is cut there.
    let body = &body[..floor_char_boundary(body, super::limits::MAX_TEXT_BYTES as usize)];
    let charset = header_param(content_type, "charset");
    let bytes = transfer_decode(body, header(headers, "content-transfer-encoding"));
    let text = decode_charset(&bytes, charset.as_deref());
    if mime == "text/html" {
        out.html.push(text);
    } else {
        out.plain.push(text);
    }
}

/// An email message read: From, To, Cc, Date and Subject, then the body (its
/// plain-text parts, else its HTML parts as text, through `html_to_text`), then
/// the names of its attachments, which are listed, not read.
pub fn eml_text(bytes: &[u8], html_to_text: impl Fn(&str) -> String) -> EmailText {
    let raw = String::from_utf8_lossy(bytes);
    let (headers, body) = split_headers(&raw);
    let mut parts = MimeParts::default();
    walk_mime(&headers, body, 0, &mut parts);
    let mut text = String::new();
    for name in ["from", "to", "cc", "date", "subject"] {
        if let Some(value) = header(&headers, name) {
            let label = match name {
                "from" => "From",
                "to" => "To",
                "cc" => "Cc",
                "date" => "Date",
                _ => "Subject",
            };
            text.push_str(&format!("{label}: {}\n", decode_words(value)));
        }
    }
    let body_text = if parts.plain.iter().any(|p| !p.trim().is_empty()) {
        parts.plain.join("\n\n")
    } else {
        parts
            .html
            .iter()
            .map(|h| html_to_text(h))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    if !body_text.trim().is_empty() {
        text.push('\n');
        text.push_str(body_text.trim());
        text.push('\n');
    }
    if !parts.attachments.is_empty() {
        text.push_str(&format!(
            "\nAttachments (not indexed): {}\n",
            parts.attachments.join(", ")
        ));
    }
    EmailText {
        subject: header(&headers, "subject")
            .map(decode_words)
            .filter(|s| !s.trim().is_empty()),
        text: text.trim().to_string(),
        attachments: parts.attachments,
    }
}

// ── Jupyter notebooks (.ipynb) ────────────────────────────────────────────

/// The most characters of one cell's outputs that are kept.
pub const MAX_NOTEBOOK_OUTPUT_CHARS: usize = 2_000;

/// Why a notebook could not be read.
#[derive(Debug, thiserror::Error)]
pub enum NotebookError {
    #[error("this notebook is not valid JSON, so it may be damaged or incomplete ({0})")]
    Json(#[from] serde_json::Error),
    #[error("this file has no notebook cells, so it may be damaged")]
    NoCells,
}

/// A notebook as read: only what is indexed is held. Everything else (metadata,
/// images and HTML in outputs, attachments) is skipped while it is parsed,
/// without being built in memory and without a nesting limit.
#[derive(serde::Deserialize)]
struct Notebook {
    cells: Option<Vec<NotebookCell>>,
}

#[derive(serde::Deserialize)]
struct NotebookCell {
    #[serde(default)]
    cell_type: Option<String>,
    #[serde(default)]
    source: NotebookSource,
    #[serde(default)]
    outputs: Vec<NotebookOutput>,
}

#[derive(serde::Deserialize)]
struct NotebookOutput {
    #[serde(default)]
    output_type: Option<String>,
    #[serde(default)]
    text: NotebookSource,
    #[serde(default)]
    data: NotebookData,
    #[serde(default)]
    ename: Option<String>,
    #[serde(default)]
    evalue: Option<String>,
}

#[derive(Default, serde::Deserialize)]
struct NotebookData {
    #[serde(default, rename = "text/plain")]
    plain: NotebookSource,
}

/// A cell's `source` or an output's text: a string, or lines. Anything else
/// (a number, an object, null) is no text.
#[derive(Default)]
struct NotebookSource(String);

/// Reads a [`NotebookSource`] (`lines`: a list of strings is read) or one of its
/// lines (a list or an object there is no line, skipped whole).
struct SourceVisitor {
    lines: bool,
}

impl<'de> serde::de::Visitor<'de> for SourceVisitor {
    type Value = String;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a string or a list of strings")
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
        Ok(v.to_string())
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<String, E> {
        Ok(v)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<String, A::Error> {
        let mut out = String::new();
        if self.lines {
            while let Some(line) = seq.next_element::<SourceLine>()? {
                out.push_str(&line.0);
            }
        } else {
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
        }
        Ok(out)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<String, A::Error> {
        while map
            .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
            .is_some()
        {}
        Ok(String::new())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<String, E> {
        Ok(String::new())
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<String, E> {
        Ok(String::new())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<String, E> {
        Ok(String::new())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<String, E> {
        Ok(String::new())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<String, E> {
        Ok(String::new())
    }
}

impl<'de> serde::Deserialize<'de> for NotebookSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(SourceVisitor { lines: true })
            .map(NotebookSource)
    }
}

/// One line of a `source` list.
struct SourceLine(String);

impl<'de> serde::Deserialize<'de> for SourceLine {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(SourceVisitor { lines: false })
            .map(SourceLine)
    }
}

/// A notebook's cells in order, each under a marker (`--- Cell 3 (code) ---`):
/// Markdown as written, code as written, and a code cell's text outputs (stream
/// text, `text/plain` results and error names) after it, at most
/// [`MAX_NOTEBOOK_OUTPUT_CHARS`] a cell. Images and HTML outputs are left out.
pub fn notebook_text(json: &str) -> Result<String, NotebookError> {
    let notebook: Notebook = serde_json::from_str(json)?;
    let cells = notebook.cells.ok_or(NotebookError::NoCells)?;
    let mut out = Vec::new();
    for (index, cell) in cells.into_iter().enumerate() {
        let kind = cell.cell_type.as_deref().unwrap_or("raw").to_string();
        let mut text = cell.source.0.trim_end().to_string();
        if kind == "code" {
            let mut outputs = String::new();
            for output in cell.outputs {
                let piece = match output.output_type.as_deref() {
                    Some("stream") => output.text.0,
                    Some("execute_result") | Some("display_data") => output.data.plain.0,
                    Some("error") => format!(
                        "{}: {}",
                        output.ename.as_deref().unwrap_or("Error"),
                        output.evalue.as_deref().unwrap_or("")
                    ),
                    _ => String::new(),
                };
                outputs.push_str(&piece);
                if outputs.chars().count() > MAX_NOTEBOOK_OUTPUT_CHARS {
                    break;
                }
            }
            let outputs: String = outputs.chars().take(MAX_NOTEBOOK_OUTPUT_CHARS).collect();
            if !outputs.trim().is_empty() {
                text.push_str("\n\nOutput:\n");
                text.push_str(outputs.trim_end());
            }
        }
        if text.trim().is_empty() {
            continue;
        }
        out.push(format!("--- Cell {} ({kind}) ---\n{text}", index + 1));
    }
    Ok(out.join("\n\n"))
}

// ── Legacy Word and PowerPoint (.doc, .ppt) ───────────────────────────────

/// A compound file's directory tree deeper than this is refused: office_oxide
/// searches it recursively.
pub const MAX_CFB_TREE_DEPTH: usize = 512;

/// Why a legacy Word or PowerPoint file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum LegacyOfficeError {
    #[error("this {0} file is protected with a password, so its text cannot be read")]
    Encrypted(&'static str),
    #[error("this {0} file is damaged or incomplete ({1})")]
    Damaged(&'static str, String),
    #[error("this {0} file was saved in {0} 95 or older; save it again as {1}")]
    TooOld(&'static str, &'static str),
    #[error("this {0} file contains no extractable text")]
    NoText(&'static str),
    #[error(
        "this {0} file links more than 512 parts in one chain, a layout that is not supported"
    )]
    TooComplex(&'static str),
    #[error("this {0} file took too long to read, so it may be damaged or incomplete")]
    TimedOut(&'static str),
    #[error("reading this {0} file stopped the reader (it ran out of memory or crashed), so the file may be damaged or incomplete")]
    TooMuchMemory(&'static str),
    /// What the reader process said was wrong, in its words.
    #[error("{0}")]
    Reported(String),
}

/// What [`validate_compound_file`] found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfbIssue {
    /// Broken or crafted: a chain that loops, a sector past the end, a size the
    /// file cannot hold.
    Damaged(String),
    /// Well formed but linked deeper than office_oxide's recursive search can
    /// safely follow.
    TooDeep,
}

/// [MS-CFB] sector markers.
const MAX_REG_SECT: u32 = 0xFFFF_FFFA;
const NO_STREAM: u32 = 0xFFFF_FFFF;
/// The mini-stream cutoff every compound file uses ([MS-CFB] 2.2).
const MINI_STREAM_CUTOFF: u64 = 4096;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..at + 4)
        .map_or(NO_STREAM, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// One directory entry, as office_oxide reads it.
struct CfbEntry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u64,
}

/// Validates the compound file (OLE2) in `bytes` before office_oxide reads it, in
/// time and memory linear in the file's size, and returns its entries' names.
///
/// office_oxide 0.1.10 follows the DIFAT with no cycle check and bounds a chain
/// only by the FAT's length, which the file decides: a few kilobytes can make it
/// loop for ever or read gigabytes. So every chain it will follow is walked here
/// first, each sector at most once: the DIFAT, the FAT sectors it lists (each
/// once, inside the file), the directory, the mini FAT, the mini stream and every
/// stream, each no longer than its size needs. The directory tree must not loop
/// or nest deeper than [`MAX_CFB_TREE_DEPTH`] (office_oxide searches it
/// recursively).
pub fn validate_compound_file(bytes: &[u8]) -> Result<Vec<String>, CfbIssue> {
    let damaged = |what: &str| CfbIssue::Damaged(what.to_string());
    const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    if bytes.len() < 512 || bytes[..8] != SIGNATURE {
        return Err(damaged("it is not a compound file"));
    }
    let major = u16::from_le_bytes([bytes[0x1A], bytes[0x1B]]);
    let shift = u16::from_le_bytes([bytes[0x1E], bytes[0x1F]]);
    let mini_shift = u16::from_le_bytes([bytes[0x20], bytes[0x21]]);
    let sector_size: usize = match (major, shift) {
        (3, 9) => 512,
        (4, 12) => 4096,
        _ => return Err(damaged("its header names an unknown sector size")),
    };
    if mini_shift != 6 || u64::from(u32_at(bytes, 0x38)) != MINI_STREAM_CUTOFF {
        return Err(damaged("its header names an unknown mini sector size"));
    }
    // Whole sectors after the header (sector i starts at (i + 1) × size).
    let sectors = (bytes.len() / sector_size).saturating_sub(1);
    let sector = |id: u32| -> Result<&[u8], CfbIssue> {
        let index = id as usize;
        if index >= sectors {
            return Err(damaged("a sector it names is past its end"));
        }
        let at = (index + 1) * sector_size;
        Ok(&bytes[at..at + sector_size])
    };

    // Each sector belongs to one structure: the DIFAT, the FAT or one chain. A
    // sector reached twice is a loop or two structures sharing it.
    let mut owned = vec![false; sectors];
    let header_difat: Vec<u32> = (0..109).map(|i| u32_at(bytes, 0x4C + i * 4)).collect();
    let per_sector = sector_size / 4;
    let words = |id: u32| -> Option<Vec<u32>> {
        let data = sector(id).ok()?;
        Some((0..per_sector).map(|i| u32_at(data, i * 4)).collect())
    };
    let fat_ids = fat_sector_ids(
        &header_difat,
        u32_at(bytes, 0x44),
        &words,
        sectors,
        &mut owned,
    )?;
    // The FAT: its sectors' entries, so it is no longer than the file has sectors.
    let mut fat: Vec<u32> = Vec::new();
    for id in &fat_ids {
        fat.extend(words(*id).ok_or_else(|| damaged("a sector it names is past its end"))?);
    }
    drop(fat_ids);

    // The directory.
    let mut entries = Vec::new();
    for id in walk_chain(&fat, u32_at(bytes, 0x30), sectors, sectors, &mut owned)? {
        let data = sector(id)?;
        for raw in data.chunks_exact(128) {
            let len = (u16::from_le_bytes([raw[0x40], raw[0x41]]) as usize).min(64);
            let units: Vec<u16> = raw[..len]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|u| *u != 0)
                .collect();
            let low = u64::from(u32_at(raw, 0x78));
            let size = if major == 4 {
                (u64::from(u32_at(raw, 0x7C)) << 32) | low
            } else {
                low
            };
            entries.push(CfbEntry {
                name: String::from_utf16_lossy(&units),
                kind: raw[0x42],
                left: u32_at(raw, 0x44),
                right: u32_at(raw, 0x48),
                child: u32_at(raw, 0x4C),
                start: u32_at(raw, 0x74),
                size,
            });
        }
    }
    let Some(root) = entries.first() else {
        return Err(damaged("it has no directory"));
    };

    // The directory tree: no entry twice, no path deeper than office_oxide can follow.
    let links: Vec<(u32, u32, u32)> = entries.iter().map(|e| (e.left, e.right, e.child)).collect();
    let names: Vec<String> = walk_directory_tree(&links)?
        .into_iter()
        .map(|i| entries[i].name.clone())
        .collect();

    // The mini stream (the root's chain) and the mini FAT.
    let mini_stream = if root.kind == 5 && root.start <= MAX_REG_SECT {
        check_stream_size(root.size, sectors, sector_size)?;
        walk_chain(
            &fat,
            root.start,
            sectors,
            sectors_for(root.size, sector_size),
            &mut owned,
        )?
        .len()
            * sector_size
    } else {
        0
    };
    let mut mini_fat: Vec<u32> = Vec::new();
    let first_mini_fat = u32_at(bytes, 0x3C);
    if first_mini_fat <= MAX_REG_SECT {
        for id in walk_chain(&fat, first_mini_fat, sectors, sectors, &mut owned)? {
            mini_fat.extend(words(id).ok_or_else(|| damaged("a sector it names is past its end"))?);
        }
    }
    let mini_sectors = mini_stream / 64;
    let mut mini_owned = vec![false; mini_sectors];

    // Every stream, as office_oxide reads it: from the mini stream when it is
    // smaller than the cutoff and there is a mini stream, else from the FAT.
    for entry in entries.iter().filter(|e| e.kind == 2 && e.size > 0) {
        check_stream_size(entry.size, sectors, sector_size)?;
        if entry.size < MINI_STREAM_CUTOFF && mini_stream > 0 {
            walk_chain(
                &mini_fat,
                entry.start,
                mini_sectors,
                sectors_for(entry.size, 64),
                &mut mini_owned,
            )?;
        } else {
            walk_chain(
                &fat,
                entry.start,
                sectors,
                sectors_for(entry.size, sector_size),
                &mut owned,
            )?;
        }
    }
    Ok(names)
}

/// The most sectors a stream of `size` bytes can span in `unit`-byte sectors.
pub(crate) fn sectors_for(size: u64, unit: usize) -> usize {
    usize::try_from(size.div_ceil(unit as u64)).unwrap_or(usize::MAX)
}

/// Refuses a stated stream size larger than the file's `sectors` of `sector_size`
/// bytes can hold.
pub(crate) fn check_stream_size(
    size: u64,
    sectors: usize,
    sector_size: usize,
) -> Result<(), CfbIssue> {
    if size > (sectors as u64).saturating_mul(sector_size as u64) {
        return Err(CfbIssue::Damaged("a stream is larger than the file".into()));
    }
    Ok(())
}

/// The sectors of the chain from `start` in `table` (a FAT or mini FAT), ended by
/// any marker above [`MAX_REG_SECT`]. Each sector must be one of `sectors`, owned by
/// no other structure (`owned`, which the walk marks), and the chain no longer
/// than `limit` sectors.
pub(crate) fn walk_chain(
    table: &[u32],
    start: u32,
    sectors: usize,
    limit: usize,
    owned: &mut [bool],
) -> Result<Vec<u32>, CfbIssue> {
    let mut out = Vec::new();
    let mut at = start;
    while at <= MAX_REG_SECT {
        let index = at as usize;
        if index >= sectors || index >= table.len() || index >= owned.len() {
            return Err(CfbIssue::Damaged("a sector chain runs past its end".into()));
        }
        if std::mem::replace(&mut owned[index], true) {
            return Err(CfbIssue::Damaged("a sector chain loops".into()));
        }
        out.push(at);
        if out.len() > limit {
            return Err(CfbIssue::Damaged(
                "a stream is longer than its size says".into(),
            ));
        }
        at = table[index];
    }
    Ok(out)
}

/// The FAT's sectors: the header's DIFAT entries, then those of each DIFAT sector
/// from `first_difat` (`words` gives a sector's entries, its last the next DIFAT
/// sector; `None` past the file's end). DIFAT and FAT sectors must each be one of
/// `sectors` and appear once (`owned` is marked), so the walk ends and the FAT is
/// no longer than the file.
pub(crate) fn fat_sector_ids(
    header_difat: &[u32],
    first_difat: u32,
    words: &dyn Fn(u32) -> Option<Vec<u32>>,
    sectors: usize,
    owned: &mut [bool],
) -> Result<Vec<u32>, CfbIssue> {
    let damaged = |what: &str| CfbIssue::Damaged(what.to_string());
    let mut ids: Vec<u32> = header_difat
        .iter()
        .copied()
        .filter(|s| *s <= MAX_REG_SECT)
        .collect();
    let mut difat = first_difat;
    while difat <= MAX_REG_SECT {
        let index = difat as usize;
        if index >= sectors || index >= owned.len() {
            return Err(damaged("its sector list (DIFAT) runs past its end"));
        }
        if std::mem::replace(&mut owned[index], true) {
            return Err(damaged("its sector list (DIFAT) loops"));
        }
        let entries =
            words(difat).ok_or_else(|| damaged("its sector list (DIFAT) runs past its end"))?;
        let Some((next, listed)) = entries.split_last() else {
            return Err(damaged("its sector list (DIFAT) is empty"));
        };
        ids.extend(listed.iter().copied().filter(|s| *s <= MAX_REG_SECT));
        difat = *next;
    }
    for id in &ids {
        let index = *id as usize;
        if index >= sectors || index >= owned.len() {
            return Err(damaged(
                "its sector table (FAT) names a sector past its end",
            ));
        }
        if std::mem::replace(&mut owned[index], true) {
            return Err(damaged("its sector table (FAT) lists a sector twice"));
        }
    }
    Ok(ids)
}

/// The entries reached from the root's child through `links` (left, right and
/// child of each entry), in the order found; refused when one is reached twice
/// (office_oxide's recursive search would never end), when a link names no entry,
/// or when a path is deeper than [`MAX_CFB_TREE_DEPTH`].
pub(crate) fn walk_directory_tree(links: &[(u32, u32, u32)]) -> Result<Vec<usize>, CfbIssue> {
    let Some(&(_, _, root_child)) = links.first() else {
        return Err(CfbIssue::Damaged("it has no directory".into()));
    };
    let mut seen = vec![false; links.len()];
    seen[0] = true;
    let mut reached = Vec::new();
    let mut stack = vec![(root_child, 1usize)];
    while let Some((id, depth)) = stack.pop() {
        if id == NO_STREAM {
            continue;
        }
        let index = id as usize;
        let Some(&(left, right, child)) = links.get(index) else {
            return Err(CfbIssue::Damaged(
                "its directory names an entry that is not there".into(),
            ));
        };
        if std::mem::replace(&mut seen[index], true) {
            return Err(CfbIssue::Damaged("its directory loops".into()));
        }
        if depth > MAX_CFB_TREE_DEPTH {
            return Err(CfbIssue::TooDeep);
        }
        reached.push(index);
        stack.push((left, depth + 1));
        stack.push((right, depth + 1));
        stack.push((child, depth + 1));
    }
    Ok(reached)
}

fn cfb_error(kind: &'static str, issue: CfbIssue) -> LegacyOfficeError {
    match issue {
        CfbIssue::Damaged(what) => LegacyOfficeError::Damaged(kind, what),
        CfbIssue::TooDeep => LegacyOfficeError::TooComplex(kind),
    }
}

/// `read` run so that a panic in it is a damaged file, not a crash.
fn guarded<T>(
    kind: &'static str,
    read: impl FnOnce() -> Result<T, LegacyOfficeError>,
) -> Result<T, LegacyOfficeError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).unwrap_or_else(|panic| {
        let what = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "the reader stopped".into());
        Err(LegacyOfficeError::Damaged(kind, what))
    })
}

/// The text of a legacy Word (`.doc`, Word 97 and later) file: its main text,
/// then its footnotes, endnotes, comments, text boxes and headers and footers,
/// each under its name.
pub fn doc_text(bytes: &[u8]) -> Result<String, LegacyOfficeError> {
    use office_oxide::doc::{DocDocument, DocError, SubDocumentKind};
    const KIND: &str = "Word";
    validate_compound_file(bytes).map_err(|e| cfb_error(KIND, e))?;
    let doc = guarded(KIND, || {
        DocDocument::from_reader(Cursor::new(bytes)).map_err(|e| match e {
            DocError::Encrypted => LegacyOfficeError::Encrypted(KIND),
            DocError::UnsupportedVersion(_) => LegacyOfficeError::TooOld(KIND, ".docx"),
            other => LegacyOfficeError::Damaged(KIND, other.to_string()),
        })
    })?;
    let mut text = doc.plain_text_ref().trim().to_string();
    for sub in doc.subdocuments() {
        let name = match sub.kind {
            SubDocumentKind::Footnotes => "Footnotes",
            SubDocumentKind::Endnotes => "Endnotes",
            SubDocumentKind::Comments => "Comments",
            SubDocumentKind::TextBoxes | SubDocumentKind::HeaderTextBoxes => "Text boxes",
            SubDocumentKind::HeadersFooters => "Headers and footers",
        };
        let body = sub.text.trim();
        if !body.is_empty() {
            text.push_str(&format!("\n\n{name}:\n{body}"));
        }
    }
    if text.trim().is_empty() {
        return Err(LegacyOfficeError::NoText(KIND));
    }
    Ok(text)
}

/// The text of each slide of a legacy PowerPoint (`.ppt`) file, in order, without
/// speaker notes (as for `.pptx`).
pub fn ppt_slides(bytes: &[u8]) -> Result<Vec<String>, LegacyOfficeError> {
    use office_oxide::ppt::{PptDocument, PptError, TextType};
    const KIND: &str = "PowerPoint";
    let names = validate_compound_file(bytes).map_err(|e| cfb_error(KIND, e))?;
    // An encrypted presentation keeps its properties in "EncryptedSummary"
    // ([MS-PPT] 2.3.7); its slides are unreadable.
    if names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("EncryptedSummary"))
    {
        return Err(LegacyOfficeError::Encrypted(KIND));
    }
    if !names.iter().any(|n| {
        n.eq_ignore_ascii_case("PowerPoint Document") || n.eq_ignore_ascii_case("PP97_DUALSTORAGE")
    }) {
        return Err(LegacyOfficeError::Damaged(
            KIND,
            "it has no PowerPoint Document stream".into(),
        ));
    }
    let doc = guarded(KIND, || {
        PptDocument::from_reader(Cursor::new(bytes)).map_err(|e| match e {
            PptError::Encrypted => LegacyOfficeError::Encrypted(KIND),
            other => LegacyOfficeError::Damaged(KIND, other.to_string()),
        })
    })?;
    let slides: Vec<String> = doc
        .slides
        .iter()
        .map(|slide| {
            slide
                .text_runs
                .iter()
                .filter(|run| run.text_type != TextType::Notes)
                .map(|run| run.text.replace('\r', "\n").trim().to_string())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    if slides.iter().all(|s| s.trim().is_empty()) {
        return Err(LegacyOfficeError::NoText(KIND));
    }
    Ok(slides)
}

// ── EPUB ──────────────────────────────────────────────────────────────────

/// The most chapters (spine items) of one book that are read.
pub const MAX_EPUB_CHAPTERS: usize = 5_000;

/// One chapter of a book: its title (its first heading, else its `<title>`) and
/// its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub title: Option<String>,
    pub text: String,
}

/// `href` (relative to the folder of `base`, URL-encoded, maybe with a fragment)
/// as a path inside the archive; `None` when it leaves the archive.
pub fn resolve_href(base: &str, href: &str) -> Option<String> {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_decode(href);
    let mut parts: Vec<&str> = match base.rfind('/') {
        Some(at) => base[..at].split('/').filter(|p| !p.is_empty()).collect(),
        None => Vec::new(),
    };
    if href.starts_with('/') {
        parts.clear();
    }
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The first heading (`h1`–`h3`) of an XHTML chapter, else its `<title>`, as text.
pub fn chapter_title(xhtml: &str, html_to_text: &dyn Fn(&str) -> String) -> Option<String> {
    let lower = xhtml.to_ascii_lowercase();
    let mut found = None;
    for tag in ["h1", "h2", "h3", "title"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let Some(start) = lower.find(&open) else {
            continue;
        };
        let Some(gt) = lower[start..].find('>') else {
            continue;
        };
        let from = start + gt + 1;
        let Some(end) = lower[from..].find(&close) else {
            continue;
        };
        let text = html_to_text(&xhtml[from..from + end]);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            found = Some(text.chars().take(200).collect());
            break;
        }
    }
    found
}
