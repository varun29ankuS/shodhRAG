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

/// The Windows-1252 character of byte `b` (RTF's `\'hh` escapes and 8-bit text).
fn cp1252(b: u8) -> char {
    let byte = [b];
    let (text, _, _) = encoding_rs::WINDOWS_1252.decode(&byte);
    text.chars().next().unwrap_or('\u{FFFD}')
}

/// The text of an RTF file: paragraphs as lines, `\tab` as a tab, Unicode
/// (`\uN`) and 8-bit (`\'hh`) characters decoded, and the content of every
/// destination that is not text (fonts, styles, pictures, objects, properties,
/// headers and footers, ignorable `\*` destinations) left out.
pub fn rtf_text(bytes: &[u8]) -> Result<String, RtfError> {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    if !bytes[start..].starts_with(b"{\\rtf") {
        return Err(RtfError::NotRtf);
    }

    /// The state a group starts with and restores when it ends.
    #[derive(Clone, Copy)]
    struct Group {
        skip: bool,
        /// Fallback characters after a `\uN` (`\ucN`).
        uc: usize,
    }

    let mut out = String::new();
    let mut stack: Vec<Group> = Vec::new();
    let mut state = Group { skip: false, uc: 1 };
    // Fallback characters still to drop after a `\uN`.
    let mut pending_fallback = 0usize;
    // The next control word opens an ignorable destination (`{\*\name`).
    let mut ignorable = false;
    // At the start of a group: its first control word may name a destination.
    let mut group_start = false;
    let mut i = start;
    let push = |out: &mut String, c: char, state: &Group, pending: &mut usize| {
        if *pending > 0 {
            *pending -= 1;
        } else if !state.skip {
            out.push(c);
        }
    };
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'{' => {
                if stack.len() >= MAX_RTF_DEPTH {
                    return Err(RtfError::TooDeep);
                }
                stack.push(state);
                group_start = true;
                ignorable = false;
                i += 1;
            }
            b'}' => {
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
                    let opens = group_start;
                    group_start = false;
                    if (opens && RTF_SKIPPED.contains(&word))
                        || (ignorable && opens_after_star(word))
                    {
                        state.skip = true;
                    }
                    ignorable = false;
                    match word {
                        "par" | "line" | "sect" | "page" | "row" => {
                            push(&mut out, '\n', &state, &mut 0)
                        }
                        "tab" | "cell" => push(&mut out, '\t', &state, &mut 0),
                        "emdash" => push(&mut out, '—', &state, &mut 0),
                        "endash" => push(&mut out, '–', &state, &mut 0),
                        "lquote" => push(&mut out, '‘', &state, &mut 0),
                        "rquote" => push(&mut out, '’', &state, &mut 0),
                        "ldblquote" => push(&mut out, '“', &state, &mut 0),
                        "rdblquote" => push(&mut out, '”', &state, &mut 0),
                        "bullet" => push(&mut out, '•', &state, &mut 0),
                        "uc" => state.uc = param.unwrap_or(1).clamp(0, 16) as usize,
                        "u" => {
                            if let Some(n) = param {
                                // A signed 16-bit value: negatives are code units above 32767.
                                let unit = if n < 0 { n + 65_536 } else { n };
                                let c = u32::try_from(unit)
                                    .ok()
                                    .and_then(char::from_u32)
                                    .unwrap_or('\u{FFFD}');
                                push(&mut out, c, &state, &mut 0);
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
                                push(&mut out, cp1252(byte), &state, &mut pending_fallback);
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
                        b'~' => push(&mut out, '\u{a0}', &state, &mut pending_fallback),
                        b'_' => push(&mut out, '-', &state, &mut pending_fallback),
                        b'-' => {}
                        b'\\' | b'{' | b'}' => {
                            push(&mut out, char::from(next), &state, &mut pending_fallback)
                        }
                        b'\n' | b'\r' => push(&mut out, '\n', &state, &mut 0),
                        _ => {}
                    }
                    group_start = false;
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                group_start = false;
                let c = if b.is_ascii() {
                    char::from(b)
                } else {
                    cp1252(b)
                };
                push(&mut out, c, &state, &mut pending_fallback);
                i += 1;
            }
        }
    }
    Ok(tidy_lines(&out))
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
    if mime.starts_with("multipart/") {
        let Some(boundary) = header_param(content_type, "boundary") else {
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
    let attachment = disposition
        .trim()
        .to_ascii_lowercase()
        .starts_with("attachment")
        || (name.is_some() && !mime.starts_with("text/"));
    if attachment {
        out.attachments
            .push(name.unwrap_or_else(|| format!("an attachment ({mime})")));
        return;
    }
    let charset = header_param(content_type, "charset");
    let bytes = transfer_decode(body, header(headers, "content-transfer-encoding"));
    let text = decode_charset(&bytes, charset.as_deref());
    match mime.as_str() {
        "text/plain" | "" => out.plain.push(text),
        "text/html" => out.html.push(text),
        other if other.starts_with("text/") => out.plain.push(text),
        _ => out
            .attachments
            .push(name.unwrap_or_else(|| format!("an attachment ({mime})"))),
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

/// A cell's `source` (a string, or lines).
fn notebook_source(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(lines)) => lines
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<String>(),
        _ => String::new(),
    }
}

/// A notebook's cells in order, each under a marker (`--- Cell 3 (code) ---`):
/// Markdown as written, code as written, and a code cell's text outputs (stream
/// text, `text/plain` results and error names) after it, at most
/// [`MAX_NOTEBOOK_OUTPUT_CHARS`] a cell. Images and HTML outputs are left out.
pub fn notebook_text(json: &str) -> Result<String, NotebookError> {
    let notebook: serde_json::Value = serde_json::from_str(json)?;
    let cells = notebook
        .get("cells")
        .and_then(serde_json::Value::as_array)
        .ok_or(NotebookError::NoCells)?;
    let mut out = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        let kind = cell
            .get("cell_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("raw");
        let source = notebook_source(cell.get("source"));
        let mut text = source.trim_end().to_string();
        if kind == "code" {
            let mut outputs = String::new();
            for output in cell
                .get("outputs")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let piece = match output
                    .get("output_type")
                    .and_then(serde_json::Value::as_str)
                {
                    Some("stream") => notebook_source(output.get("text")),
                    Some("execute_result") | Some("display_data") => {
                        notebook_source(output.get("data").and_then(|d| d.get("text/plain")))
                    }
                    Some("error") => format!(
                        "{}: {}",
                        output
                            .get("ename")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("Error"),
                        output
                            .get("evalue")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
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
    #[error(
        "this {0} file is from before {0} 97, a format that is not supported; save it again as {1}"
    )]
    TooOld(&'static str, &'static str),
    #[error("this {0} file contains no extractable text")]
    NoText(&'static str),
}

/// Whether the directory of the compound file in `bytes` is a tree office_oxide
/// can search: no entry reached twice (a loop would recurse until the stack runs
/// out) and no path deeper than [`MAX_CFB_TREE_DEPTH`]. `Err` names what is wrong.
fn check_cfb_tree(bytes: &[u8]) -> Result<Vec<String>, String> {
    use office_oxide::cfb::CfbReader;
    /// No sibling or child ([MS-CFB] 2.6.1 NOSTREAM).
    const NO_ENTRY: u32 = 0xFFFF_FFFF;
    let reader = CfbReader::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let entries = reader.entries();
    let Some(root) = entries.first() else {
        return Err("it has no directory".into());
    };
    let mut seen = vec![false; entries.len()];
    seen[0] = true;
    let mut names = Vec::new();
    let mut stack = vec![(root.child, 1usize)];
    while let Some((id, depth)) = stack.pop() {
        if id == NO_ENTRY {
            continue;
        }
        let index = id as usize;
        let Some(entry) = entries.get(index) else {
            return Err(format!(
                "its directory names an entry ({id}) that is not there"
            ));
        };
        if std::mem::replace(&mut seen[index], true) {
            return Err("its directory loops".into());
        }
        if depth > MAX_CFB_TREE_DEPTH {
            return Err("its directory nests too deep".into());
        }
        names.push(entry.name.clone());
        stack.push((entry.left_sibling, depth + 1));
        stack.push((entry.right_sibling, depth + 1));
        stack.push((entry.child, depth + 1));
    }
    Ok(names)
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
    check_cfb_tree(bytes).map_err(|e| LegacyOfficeError::Damaged(KIND, e))?;
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
    let names = check_cfb_tree(bytes).map_err(|e| LegacyOfficeError::Damaged(KIND, e))?;
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
