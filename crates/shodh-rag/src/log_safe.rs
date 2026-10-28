//! What the log may say about the user's own content.
//!
//! The log is a plain file kept for weeks and attached to bug reports, so it never holds
//! a question's text, a document's name or path, or a message from Slack or Telegram.
//! A file is named by [`file`]: its kind and a short fingerprint of its path, enough to
//! follow one file through the log (every line about it carries the same fingerprint)
//! without saying which file it is. A question is described by its length ([`text`]).

use std::fmt;
use std::path::Path;

/// A file (or an indexed source: a path, or a `scheme://…` key) as the log shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct FileRef {
    kind: String,
    fingerprint: u32,
}

impl fmt::Display for FileRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{:08x}", self.kind, self.fingerprint)
    }
}

impl fmt::Debug for FileRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// FNV-1a: stable across runs and platforms (the log of one run must match the next).
fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// `path` as the log shows it: `pdf#1a2b3c4d`, `calendar#…`, `file#…` (no extension).
/// The fingerprint ignores case and the kind of slash, as Windows paths do.
pub fn file(path: impl AsRef<Path>) -> FileRef {
    let text = path.as_ref().to_string_lossy();
    let kind = match text.split_once("://") {
        Some((scheme, _))
            if !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            scheme.to_ascii_lowercase()
        }
        _ => Path::new(text.as_ref())
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .filter(|e| {
                !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .unwrap_or_else(|| "file".to_string()),
    };
    let normalized = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == '\\' { '/' } else { c })
        .collect::<String>();
    FileRef {
        kind,
        fingerprint: fnv1a(normalized.into_bytes()),
    }
}

/// Several files as the log shows them, in their order.
pub fn files<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) -> Vec<FileRef> {
    paths.into_iter().map(file).collect()
}

/// `message` with `path` in it (as a whole, or its file name) shown as [`file`] does: for
/// an error that names the file it is about. Case and the kind of slash are ignored.
pub fn scrub(message: &str, path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let shown = file(path).to_string();
    let whole = path.to_string_lossy().into_owned();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let once = replace_loosely(message, &whole, &shown);
    if name.chars().count() >= 3 {
        replace_loosely(&once, &name, &shown)
    } else {
        once
    }
}

/// Replaces `needle` in `message`, ignoring ASCII case and `\` versus `/`.
fn replace_loosely(message: &str, needle: &str, with: &str) -> String {
    let fold = |s: &str| -> Vec<u8> {
        s.bytes()
            .map(|b| {
                if b == b'\\' {
                    b'/'
                } else {
                    b.to_ascii_lowercase()
                }
            })
            .collect()
    };
    let hay = fold(message);
    let needle = fold(needle);
    if needle.is_empty() {
        return message.to_string();
    }
    let mut out = String::with_capacity(message.len());
    let (mut i, mut last) = (0, 0);
    while i + needle.len() <= hay.len() {
        let end = i + needle.len();
        if hay[i..end] == needle[..] && message.is_char_boundary(i) && message.is_char_boundary(end)
        {
            out.push_str(&message[last..i]);
            out.push_str(with);
            i = end;
            last = end;
        } else {
            i += 1;
        }
    }
    out.push_str(&message[last..]);
    out
}

/// A text the user wrote (a question, a search), as the log shows it: its length only.
pub fn text(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_shown_by_kind_and_fingerprint_never_by_name() {
        let shown = file(r"C:\Users\Ann\Documents\Lease agreement 2026.pdf").to_string();
        assert!(shown.starts_with("pdf#"), "{shown}");
        assert_eq!(shown.len(), "pdf#".len() + 8);
        for part in ["Users", "Ann", "Documents", "Lease", "agreement", "2026"] {
            assert!(!shown.contains(part), "{shown} names {part}");
        }
        assert_eq!(format!("{:?}", file("a/b.md")), file("a/b.md").to_string());
    }

    #[test]
    fn the_same_file_has_the_same_fingerprint_however_it_is_written() {
        let a = file(r"C:\Users\Ann\Notes\plan.md");
        assert_eq!(a, file("c:/users/ann/notes/plan.md"));
        assert_eq!(
            a,
            file(std::path::PathBuf::from(r"C:\Users\Ann\Notes\plan.md"))
        );
        assert_ne!(a, file(r"C:\Users\Ann\Notes\plan2.md"));
        // Stable across runs: a fixed value, not a per-process hash.
        assert_eq!(
            file("a.txt").to_string(),
            format!("txt#{:08x}", fnv1a(*b"a.txt"))
        );
    }

    #[test]
    fn sources_without_an_extension_or_with_a_scheme() {
        assert!(file("calendar://task/93308c4b")
            .to_string()
            .starts_with("calendar#"));
        assert!(file("note://abc").to_string().starts_with("note#"));
        assert!(file("/home/ann/README").to_string().starts_with("file#"));
        // An "extension" that is really part of a name is not shown.
        assert!(file("/home/ann/Report v2.final draft")
            .to_string()
            .starts_with("file#"));
    }

    #[test]
    fn an_error_naming_the_file_is_scrubbed() {
        let path = std::path::Path::new(r"C:\Users\Ann\Docs\Tax return.pdf");
        let shown = file(path).to_string();
        let message =
            "Failed to read PDF: c:/users/ann/docs/tax return.pdf: damaged (Tax return.pdf)";
        assert_eq!(
            scrub(message, path),
            format!("Failed to read PDF: {shown}: damaged ({shown})")
        );
        assert_eq!(scrub("no file here", path), "no file here");
    }

    #[test]
    fn a_question_is_shown_by_its_length() {
        assert_eq!(text("Was kostet die Miete?"), 21);
        assert_eq!(files(["a.pdf", "b.md"]).len(), 2);
    }
}
