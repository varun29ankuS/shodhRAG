//! Reading a JSONL transcript line by line, defensively: each line is decoded on its
//! own, so one damaged line (a crash mid-write, an unknown shape) costs that line only.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde_json::Value;

use super::redact::Redactor;
use super::{LineError, ParseBudget, ERROR_PREVIEW_CHARS};

/// One line of a transcript, decoded.
pub(super) struct Line {
    /// 1-based.
    pub number: u64,
    pub value: Value,
}

/// Reads `path` and hands each decoded line to `each`, in order. A line that is not JSON,
/// or too long, is recorded in `errors` (with a redacted preview, never its raw text);
/// blank lines are passed over. Stops early when `each` returns `false`.
pub(super) fn for_each_line(
    path: &Path,
    redactor: &Redactor,
    budget: ParseBudget,
    errors: &mut Vec<LineError>,
    error_count: &mut usize,
    mut each: impl FnMut(Line) -> bool,
) -> std::io::Result<()> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buffer: Vec<u8> = Vec::new();
    let mut offset: u64 = 0;
    let mut number: u64 = 0;
    loop {
        buffer.clear();
        let read = read_line_bounded(&mut reader, &mut buffer, budget.max_line_bytes)?;
        let Some((consumed, complete)) = read else {
            break;
        };
        number += 1;
        let line_offset = offset;
        offset += consumed as u64;
        if !complete {
            *error_count += 1;
            if errors.len() < budget.max_errors {
                errors.push(LineError {
                    line: number,
                    offset: line_offset,
                    message: format!(
                        "the line is longer than {} MB and was not read",
                        budget.max_line_bytes / (1024 * 1024)
                    ),
                    preview: redactor.redact_raw_line(&buffer, ERROR_PREVIEW_CHARS),
                });
            }
            continue;
        }
        let text = trim_line(&buffer);
        if text.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Value>(text) {
            Ok(value) => {
                if !each(Line { number, value }) {
                    break;
                }
            }
            Err(e) => {
                *error_count += 1;
                if errors.len() < budget.max_errors {
                    errors.push(LineError {
                        line: number,
                        offset: line_offset,
                        message: format!("the line is not valid JSON ({})", json_error(&e)),
                        preview: redactor.redact_raw_line(text, ERROR_PREVIEW_CHARS),
                    });
                }
            }
        }
    }
    Ok(())
}

/// What serde_json says, without the text it quotes (which could hold a secret).
fn json_error(e: &serde_json::Error) -> String {
    let kind = match e.classify() {
        serde_json::error::Category::Io => "read error",
        serde_json::error::Category::Syntax => "syntax error",
        serde_json::error::Category::Data => "unexpected data",
        serde_json::error::Category::Eof => "the line ends early",
    };
    format!("{kind} at column {}", e.column())
}

fn trim_line(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r' | b' ' | b'\t') {
        end -= 1;
    }
    let mut start = 0;
    // A UTF-8 byte order mark on the first line.
    if line.starts_with(&[0xEF, 0xBB, 0xBF]) {
        start = 3;
    }
    while start < end && matches!(line[start], b' ' | b'\t') {
        start += 1;
    }
    &line[start..end]
}

/// Reads one line into `buffer` (with its line break), keeping at most `max` bytes of
/// it: a longer line is read through to its end and reported incomplete. Returns the
/// bytes consumed and whether the line was kept whole; `None` at the end of the file.
fn read_line_bounded<R: Read>(
    reader: &mut BufReader<R>,
    buffer: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<Option<(usize, bool)>> {
    let mut consumed = 0usize;
    let mut complete = true;
    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(if consumed == 0 {
                None
            } else {
                Some((consumed, complete))
            });
        }
        let (take, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if complete {
            if buffer.len() + take <= max {
                buffer.extend_from_slice(&available[..take]);
            } else {
                // Keep a little of it for the error's preview.
                let room = ERROR_PREVIEW_CHARS * 4;
                if buffer.len() < room {
                    let more = (room - buffer.len()).min(take);
                    buffer.extend_from_slice(&available[..more]);
                }
                complete = false;
            }
        }
        reader.consume(take);
        consumed += take;
        if done {
            return Ok(Some((consumed, complete)));
        }
    }
}
