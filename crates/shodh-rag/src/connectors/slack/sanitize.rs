//! The last guard before text leaves for Slack: local paths, the computer's home folder,
//! the OS user name and internal ids are replaced, whatever produced the text (the model,
//! a passage, a file name).
//!
//! The answer path already keeps paths away from the model (it sees titles and pages
//! only); this catches what a document's own text or a model's guess could still carry.
//! It errs towards removing too much: a sentence that runs on after a path may lose a few
//! words, a path may never survive.

use std::sync::OnceLock;

use regex::Regex;

/// What replaces a local path.
pub const PATH_MARK: &str = "[local path]";
/// What replaces the OS user name.
pub const NAME_MARK: &str = "[name]";
/// What replaces an internal id.
pub const ID_MARK: &str = "[id]";

/// Shortest user name replaced as a word: shorter ones ("al", "jo") are ordinary words
/// too often.
pub const MIN_NAME_CHARS: usize = 3;

/// The private strings of this computer: its home folder(s) and user name(s).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivateTerms {
    pub home_dirs: Vec<String>,
    pub user_names: Vec<String>,
}

impl PrivateTerms {
    /// This computer's home folder and user name (`USERNAME` / `USER`).
    pub fn of_this_computer() -> Self {
        let mut terms = Self::default();
        if let Some(home) = dirs::home_dir() {
            terms.home_dirs.push(home.to_string_lossy().into_owned());
            if let Some(name) = home.file_name() {
                terms.user_names.push(name.to_string_lossy().into_owned());
            }
        }
        for var in ["USERNAME", "USER"] {
            if let Ok(name) = std::env::var(var) {
                terms.user_names.push(name);
            }
        }
        terms.user_names.retain(|n| !n.trim().is_empty());
        terms.user_names.sort();
        terms.user_names.dedup();
        terms
    }
}

/// The tail of a path after its root: folders (which may contain spaces, as they end at a
/// separator), then a last part without spaces.
const PATH_TAIL: &str = r#"(?:[^\\/\r\n"'<>|*?`]*[\\/])*[^\s\\/"'<>|*?`]*"#;

fn path_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            // file:// URLs, whatever follows.
            r"(?i)file:/+\S*".to_string(),
            // Windows drive paths, either separator: C:\Users\…, c:/data/…
            format!(r"(?i)\b[a-z]:[\\/]{PATH_TAIL}"),
            // UNC paths: \\server\share\…
            format!(r"\\\\[^\s\\/]+[\\/]{PATH_TAIL}"),
            // Home folders on macOS, Linux and WSL: /Users/x/…, /home/x/…, /mnt/c/Users/…
            format!(r"(?i)(?:/mnt/[a-z])?/(?:users|home|root)(?:/{PATH_TAIL})?"),
            // Home-relative paths: ~/…, ~\…
            format!(r"~[\\/]{PATH_TAIL}"),
            // %USERPROFILE%\… and $HOME/…
            format!(r"(?i)(?:%userprofile%|%homepath%|\$home|\$\{{home\}})[\\/]?{PATH_TAIL}"),
        ]
        .iter()
        .filter_map(|p| match Regex::new(p) {
            Ok(re) => Some(re),
            Err(e) => {
                tracing::error!(pattern = %p, error = %e, "path pattern does not compile");
                None
            }
        })
        .collect()
    })
}

fn uuid_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").ok()
        })
        .as_ref()
}

/// A pattern for `home` itself and the path after it: case-insensitive (Windows paths
/// are stored lower-cased by the index), with either separator.
fn home_pattern(home: &str) -> Option<Regex> {
    let trimmed = home.trim().trim_end_matches(['\\', '/']);
    if trimmed.chars().count() < 3 {
        return None;
    }
    let mut pattern = String::from("(?i)");
    for c in trimmed.chars() {
        if c == '\\' || c == '/' {
            pattern.push_str(r"[\\/]");
        } else {
            pattern.push_str(&regex::escape(&c.to_string()));
        }
    }
    pattern.push_str(&format!(r"(?:[\\/]{PATH_TAIL})?"));
    Regex::new(&pattern).ok()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// Replaces `name` where it stands as a word of its own (case-insensitive; `_`, `.`, `-`
/// and spaces separate words, so `varun_notes.pdf` loses the name too).
fn replace_name(text: &str, name: &str) -> String {
    let name = name.trim();
    if name.chars().count() < MIN_NAME_CHARS {
        return text.to_string();
    }
    let Ok(re) = Regex::new(&format!("(?i){}", regex::escape(name))) else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in re.find_iter(text) {
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
            continue;
        }
        out.push_str(&text[last..m.start()]);
        out.push_str(NAME_MARK);
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

/// `text` with every local path, the home folder, the user name and every UUID replaced.
pub fn sanitize(text: &str, private: &PrivateTerms) -> String {
    let mut out = text.to_string();
    // The exact home folder first: it may contain spaces ("C:\Users\Ada Lovelace"), which
    // the generic patterns cannot tell from the end of a path.
    let mut homes: Vec<&String> = private.home_dirs.iter().collect();
    homes.sort_by_key(|h| std::cmp::Reverse(h.len()));
    for home in homes {
        if let Some(re) = home_pattern(home) {
            out = re.replace_all(&out, PATH_MARK).into_owned();
        }
    }
    for re in path_patterns() {
        out = re.replace_all(&out, PATH_MARK).into_owned();
    }
    for name in &private.user_names {
        out = replace_name(&out, name);
    }
    if let Some(re) = uuid_pattern() {
        out = re.replace_all(&out, ID_MARK).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms() -> PrivateTerms {
        PrivateTerms {
            home_dirs: vec![r"C:\Users\Ada Lovelace".into(), "/home/ada".into()],
            user_names: vec!["Ada Lovelace".into(), "adal".into(), "jo".into()],
        }
    }

    fn assert_clean(out: &str) {
        let lower = out.to_lowercase();
        for leak in [
            "users", "lovelace", "onedrive", "c:\\", "c:/", "/home", "adal", "\\\\", "file:",
        ] {
            assert!(!lower.contains(leak), "{leak:?} leaked in {out:?}");
        }
    }

    #[test]
    fn a_home_folder_with_a_space_is_removed_whole() {
        for text in [
            r"See C:\Users\Ada Lovelace\OneDrive\Papers\lease.pdf for details.",
            "See c:/users/ada lovelace/onedrive/papers/lease.pdf for details.",
            r"See C:\USERS\ADA LOVELACE for details.",
        ] {
            let out = sanitize(text, &terms());
            assert_clean(&out);
            assert!(out.starts_with("See [local path]"), "{out}");
            assert!(out.ends_with("for details."), "{out}");
        }
    }

    #[test]
    fn generic_paths_are_removed() {
        for text in [
            r"from D:\Research\Project X\notes.docx today",
            r"from \\fileserver\share\team folder\q3.xlsx today",
            "from file:///C:/x/y.pdf today",
            "from /Users/grace/Documents/thesis.pdf today",
            "from /home/grace/thesis.pdf today",
            "from /mnt/c/Users/grace/thesis.pdf today",
            r"from ~/papers/a.pdf today",
            r"from %USERPROFILE%\Desktop\a.pdf today",
            "from $HOME/a.pdf today",
        ] {
            let out = sanitize(text, &PrivateTerms::default());
            assert!(out.starts_with("from [local path]"), "{text} -> {out}");
            assert!(out.ends_with(" today"), "{text} -> {out}");
            for leak in [
                "research",
                "fileserver",
                "grace",
                "papers",
                "desktop",
                "a.pdf",
            ] {
                assert!(!out.to_lowercase().contains(leak), "{leak} in {out}");
            }
        }
    }

    #[test]
    fn the_user_name_is_removed_as_a_word_only() {
        let out = sanitize(
            "Prepared by Ada Lovelace (ADAL) in adal_notes.pdf; Adalbert and jo stay.",
            &terms(),
        );
        assert_eq!(
            out,
            "Prepared by [name] ([name]) in [name]_notes.pdf; Adalbert and jo stay."
        );
    }

    #[test]
    fn ids_are_removed_and_ordinary_text_is_kept() {
        let out = sanitize(
            "Chunk 3f2b8c1e-9a4d-4e5f-8b6a-1c2d3e4f5a6b says the notice period is 60 days (p. 3/4), and/or 1:2 ratios.",
            &PrivateTerms::default(),
        );
        assert_eq!(
            out,
            "Chunk [id] says the notice period is 60 days (p. 3/4), and/or 1:2 ratios."
        );
        let plain = "Rent is reviewed every spring. See [1] and [2].";
        assert_eq!(sanitize(plain, &terms()), plain);
    }

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(path_patterns().len(), 6);
        assert!(uuid_pattern().is_some());
        assert!(home_pattern(r"C:\Users\Ada Lovelace").is_some());
        assert!(home_pattern("/").is_none(), "a root is not a home folder");
    }
}
