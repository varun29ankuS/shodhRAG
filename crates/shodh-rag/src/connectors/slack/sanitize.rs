//! The last guard before text leaves for Slack: local paths, the computer's home folder,
//! the user's names and internal ids are replaced, whatever produced the text (the model,
//! a passage, a document title).
//!
//! The answer path already keeps paths away from the model (it sees titles and pages
//! only); this catches what a document's own text or a model's guess could still carry.
//! It errs towards removing too much: a sentence that runs on after a path may lose a few
//! words, a path may never survive.
//!
//! - **Paths**: Windows drive paths (`C:\…`, `c:/…`), root-relative Windows paths
//!   (`\Users\…`), UNC paths with either slash (`\\server\share\…`, `//server/share/…`),
//!   `file:` URLs, home-relative paths (`~/…`, `%USERPROFILE%\…`, `$HOME/…`), and any
//!   absolute Unix path of two or more parts (`/Users/…`, `/home/…`, `/Volumes/…`,
//!   `/private/…`, `/opt/…`, `/srv/…`). Percent-encoded text (`C%3A%5CUsers%5C…`) is
//!   decoded and checked too. The last part of a path runs to its extension, so
//!   `D:\Clients\Acme\Board plan.docx` goes whole, or, without one, to the end of the line.
//! - **Names**: each name (the OS account name, the account's display name, the home
//!   folder's name) as a whole word, case-insensitive, also joined by `_`, `-`, `.`,
//!   spaces or nothing (`Ada_Lovelace_CV.pdf`, `ada-lovelace`, `adalovelace`), and each
//!   part of 3 or more characters on its own unless it is a common word. Account names that
//!   name a role (`user`, `admin`, `owner`, …) are ordinary words and never replaced.
//! - **Ids**: UUIDs.

use std::sync::OnceLock;

use regex::{Captures, Regex};

/// What replaces a local path.
pub const PATH_MARK: &str = "[local path]";
/// What replaces the user's name.
pub const NAME_MARK: &str = "[name]";
/// What replaces an internal id.
pub const ID_MARK: &str = "[id]";

/// Shortest name (or name part) replaced as a word: shorter ones ("al", "jo") are ordinary
/// words too often.
pub const MIN_NAME_CHARS: usize = 3;

/// Account names that name a role or a machine, not a person. An account called this is
/// not a name to hide: replacing every "user" or "admin" in an answer would only garble it.
pub const GENERIC_ACCOUNT_NAMES: [&str; 32] = [
    "admin",
    "administrator",
    "default",
    "demo",
    "desktop",
    "dev",
    "developer",
    "guest",
    "home",
    "laptop",
    "local",
    "mac",
    "macbook",
    "me",
    "office",
    "owner",
    "pc",
    "public",
    "root",
    "runner",
    "shared",
    "staff",
    "student",
    "system",
    "test",
    "tester",
    "ubuntu",
    "user",
    "users",
    "windows",
    "work",
    "workstation",
];

/// Common words that are also given names or surnames. A name part equal to one is not
/// replaced on its own (the whole name still is): "we will see" keeps its "will".
pub const COMMON_NAME_WORDS: [&str; 60] = [
    "the", "and", "for", "van", "von", "der", "den", "des", "del", "bin", "ben", "abu", "will",
    "may", "mark", "bill", "rose", "grace", "hope", "joy", "art", "dawn", "june", "april",
    "august", "faith", "sky", "summer", "young", "king", "brown", "green", "white", "black",
    "gray", "grey", "long", "rich", "page", "rice", "bell", "hall", "wood", "ward", "stone",
    "price", "bond", "miles", "case", "love", "cook", "baker", "fisher", "hunter", "mason",
    "carter", "turner", "walker", "new", "little",
];

/// The private strings of this computer: its home folder(s) and the user's name(s).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivateTerms {
    pub home_dirs: Vec<String>,
    /// The OS account name, the account's display name and the home folder's name.
    pub user_names: Vec<String>,
}

impl PrivateTerms {
    /// This computer's home folder and account name (`USERNAME` / `USER`). The app adds
    /// the account's display name where the OS has one ([`Self::add_name`]).
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
        terms.tidy();
        terms
    }

    /// Adds a name (the account's display name); blank ones are ignored.
    pub fn add_name(&mut self, name: &str) {
        self.user_names.push(name.trim().to_string());
        self.tidy();
    }

    fn tidy(&mut self) {
        self.user_names.retain(|n| !n.trim().is_empty());
        self.user_names.sort();
        self.user_names.dedup();
    }
}

/// Characters a path part never contains here: separators, line ends and the characters
/// Windows forbids in names (quotes and `<>|*?` also end a path in running text).
const NOT_IN_PART: &str = r#"\\/\r\n"'<>|*?`"#;

/// The tail of a path after its root: folders (which may contain spaces, as they end at
/// a separator), then the last part: up to its extension (`Board plan.docx`,
/// `notes.v2.pdf`), else to the end of the line.
fn path_tail() -> String {
    format!(
        r#"(?:[^{n}]*[\\/])*(?:[^{n}]{{0,120}}?\.[A-Za-z0-9]{{1,8}}\b(?:\.[A-Za-z0-9_-]+)*|[^{n}]*)"#,
        n = NOT_IN_PART
    )
}

/// A path pattern. `keeps_prefix`: the pattern captures the character before the path
/// (`pre`), which stays.
struct PathPattern {
    re: Regex,
    keeps_prefix: bool,
}

/// How many path patterns there are (each must compile).
pub const PATH_PATTERNS: usize = 9;

fn path_patterns() -> &'static [PathPattern] {
    static PATTERNS: OnceLock<Vec<PathPattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let tail = path_tail();
        // A first path part starts with a character that is not a space, so "3 / 4" or
        // "\ n" in prose is not a path.
        let first = format!(r#"[^\s{NOT_IN_PART}]"#);
        let specs: Vec<(String, bool)> = vec![
            // file: URLs, whatever follows.
            (r"(?i)file:/+\S*".to_string(), false),
            // %USERPROFILE%\… and $HOME/…
            (
                format!(r"(?i)(?:%userprofile%|%homepath%|\$home|\$\{{home\}})[\\/]?{tail}"),
                false,
            ),
            // Home-relative paths: ~/…, ~\…
            (format!(r"~[\\/]{tail}"), false),
            // UNC paths with backslashes: \\server\share\…
            (format!(r"\\\\{first}[^\s\\/]*[\\/]{tail}"), false),
            // UNC paths with forward slashes: //server/share/… (not the // of a URL).
            (
                format!(r#"(?P<pre>^|[^:\w/\\])//{first}[^\s\\/]*/{tail}"#),
                true,
            ),
            // Windows drive paths, either separator: C:\Users\…, c:/data/…
            (format!(r"(?i)\b[a-z]:[\\/]{tail}"), false),
            // Root-relative Windows paths of two parts or more: \Users\…, \Clients\acme\…
            (
                // The first part has no spaces and no TeX characters, so `\alpha \beta` or
                // `\frac{a}{b}\cdot` quoted from a paper stays.
                format!(r#"(?P<pre>^|[^\\\w:])\\{first}[^\s{NOT_IN_PART}{{}}$^]*\\{tail}"#),
                true,
            ),
            // Home folders on macOS, Linux and WSL, even alone: /Users, /home/x, /mnt/c/Users/…
            (
                format!(r"(?i)(?:/mnt/[a-z])?/(?:users|home|root)\b(?:/{tail})?"),
                false,
            ),
            // Any other absolute Unix path of two parts or more: /Volumes/Client Drive/…,
            // /private/var/…, /opt/…, /srv/… (not the path of a URL, not "and/or").
            (
                format!(r#"(?P<pre>^|[\s(\[{{"'`=,;])/{first}[^{NOT_IN_PART}]*/{tail}"#),
                true,
            ),
        ];
        specs
            .into_iter()
            .filter_map(|(p, keeps_prefix)| match Regex::new(&p) {
                Ok(re) => Some(PathPattern { re, keeps_prefix }),
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

/// Runs of text with a percent-encoded byte (`C%3A%5CUsers%5C…`).
fn encoded_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| Regex::new(r"[^\s<>]*%[0-9A-Fa-f]{2}[^\s<>]*").ok())
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
    pattern.push_str(&format!(r"(?:[\\/]{})?", path_tail()));
    Regex::new(&pattern).ok()
}

/// `%XX` sequences decoded (invalid UTF-8 replaced); `None` when nothing was encoded.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut decoded = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let byte = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(byte) = byte {
                out.push(byte);
                decoded = true;
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    decoded.then(|| String::from_utf8_lossy(&out).into_owned())
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

fn is_listed(word: &str, list: &[&str]) -> bool {
    list.iter().any(|w| w.eq_ignore_ascii_case(word))
}

/// The patterns that find `name`: the whole name with its parts joined by spaces, `_`,
/// `-`, `.` or nothing, in either order, then each part of [`MIN_NAME_CHARS`] or more
/// that is not a common word. None for a generic account name.
fn name_patterns(name: &str) -> Vec<Regex> {
    let name = name.trim();
    if name.chars().count() < MIN_NAME_CHARS || is_listed(name, &GENERIC_ACCOUNT_NAMES) {
        return Vec::new();
    }
    let parts: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect();
    const JOIN: &str = r"[\s_.\-]*";
    let mut sources: Vec<String> = Vec::new();
    if parts.len() >= 2 {
        let escaped: Vec<String> = parts.iter().map(|p| regex::escape(p)).collect();
        sources.push(escaped.join(JOIN));
        let mut reversed = escaped;
        reversed.reverse();
        sources.push(reversed.join(JOIN));
        for part in &parts {
            if part.chars().count() >= MIN_NAME_CHARS
                && !is_listed(part, &COMMON_NAME_WORDS)
                && !is_listed(part, &GENERIC_ACCOUNT_NAMES)
            {
                sources.push(regex::escape(part));
            }
        }
    } else {
        sources.push(regex::escape(name));
    }
    sources
        .iter()
        .filter_map(|s| Regex::new(&format!("(?i){s}")).ok())
        .collect()
}

/// Replaces every match of `re` that stands as a word of its own (not inside a longer
/// word: `_`, `.`, `-` and spaces separate words, so `ada_notes.pdf` loses the name).
fn replace_words(text: &str, re: &Regex) -> String {
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

/// `text` with every local path and the home folders replaced.
fn replace_paths(text: &str, private: &PrivateTerms) -> String {
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
    for pattern in path_patterns() {
        out = if pattern.keeps_prefix {
            pattern
                .re
                .replace_all(&out, |c: &Captures| {
                    format!("{}{PATH_MARK}", c.name("pre").map_or("", |m| m.as_str()))
                })
                .into_owned()
        } else {
            pattern.re.replace_all(&out, PATH_MARK).into_owned()
        };
    }
    out
}

/// `text` with the user's names replaced.
fn replace_names(text: &str, private: &PrivateTerms) -> String {
    let mut patterns: Vec<Regex> = private
        .user_names
        .iter()
        .flat_map(|n| name_patterns(n))
        .collect();
    // Whole names before their parts, so "Ada Lovelace" becomes one mark, not two.
    patterns.sort_by_key(|re| std::cmp::Reverse(re.as_str().len()));
    let mut out = text.to_string();
    for re in &patterns {
        out = replace_words(&out, re);
    }
    out
}

/// `text` with every local path, the home folders, the user's names and every UUID
/// replaced.
pub fn sanitize(text: &str, private: &PrivateTerms) -> String {
    // Percent-encoded runs are decoded and checked: one that hides a path or a name goes
    // in its cleaned, decoded form.
    let mut out = match encoded_pattern() {
        Some(re) => re
            .replace_all(text, |c: &Captures| {
                let raw = &c[0];
                match percent_decode(raw) {
                    Some(decoded) => {
                        let cleaned = replace_names(&replace_paths(&decoded, private), private);
                        if cleaned == decoded {
                            raw.to_string()
                        } else {
                            cleaned
                        }
                    }
                    None => raw.to_string(),
                }
            })
            .into_owned(),
        None => text.to_string(),
    };
    out = replace_paths(&out, private);
    out = replace_names(&out, private);
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

    /// Every path shape the review listed, with nothing of the path left.
    #[test]
    fn every_path_shape_is_removed() {
        let cases: [(&str, &[&str]); 9] = [
            (
                r"see \Users\Quill Marrow\Clients\acme\plan.pdf today",
                &["users", "quill", "marrow", "clients", "acme", "plan"],
            ),
            (
                r"see \Clients\acme\plan.pdf today",
                &["clients", "acme", "plan"],
            ),
            (
                "see //fileserver/clients/acme/plan.pdf today",
                &["fileserver", "clients", "acme", "plan"],
            ),
            (
                "see /Volumes/Client Drive/acme/plan.pdf today",
                &["volumes", "client", "drive", "acme", "plan"],
            ),
            (
                "see /private/var/folders/xy/plan.pdf today",
                &["private", "folders", "plan"],
            ),
            (
                "see /opt/data/clients/plan.pdf today",
                &["opt", "clients", "plan"],
            ),
            (
                "see /srv/share/acme/plan.pdf today",
                &["srv", "share", "acme", "plan"],
            ),
            (
                "see C%3A%5CUsers%5CQuill%20Marrow%5CClients%5Cplan.pdf today",
                &["users", "quill", "marrow", "clients", "plan", "%5c"],
            ),
            (
                r"see D:\Clients\Acme\Board plan.docx today",
                &["clients", "acme", "board", "plan"],
            ),
        ];
        for (text, leaks) in cases {
            let out = sanitize(text, &PrivateTerms::default());
            assert!(out.starts_with("see [local path]"), "{text} -> {out}");
            assert!(out.ends_with(" today"), "{text} -> {out}");
            for leak in leaks {
                assert!(
                    !out.to_lowercase().contains(leak),
                    "{leak} in {text} -> {out}"
                );
            }
        }
    }

    #[test]
    fn a_last_part_without_an_extension_goes_to_the_end_of_the_line() {
        let out = sanitize(
            "Kept in D:\\Clients\\Acme Board minutes\nNext line stays.",
            &PrivateTerms::default(),
        );
        assert_eq!(out, "Kept in [local path]\nNext line stays.");
    }

    #[test]
    fn urls_fractions_percentages_and_prose_slashes_are_not_paths() {
        for text in [
            "See https://example.com/docs/guide for the guide.",
            "The ratio is 3 / 4 and 5 / 6, and/or 1/2.",
            "Rates went up 10%. A 50% share and 20%-30% growth.",
            "Dates 12/03/2024 and km/h stay.",
        ] {
            assert_eq!(sanitize(text, &terms()), text);
        }
    }

    #[test]
    fn tex_quoted_from_a_paper_is_not_a_path() {
        for text in [
            r"The loss is \alpha \beta over the batch.",
            r"Use \frac{a}{b}\cdot x here.",
        ] {
            assert_eq!(sanitize(text, &PrivateTerms::default()), text);
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

    /// The forms a two-part name takes in file names and handles. Synthetic names only.
    #[test]
    fn joined_forms_and_parts_of_a_name_are_removed() {
        let private = PrivateTerms {
            home_dirs: vec![],
            user_names: vec!["Quill Marrow".into()],
        };
        for (text, expected) in [
            ("Quill_Marrow_CV.pdf", "[name]_CV.pdf"),
            ("quill-marrow", "[name]"),
            ("QuillMarrow wrote it", "[name] wrote it"),
            ("quill.marrow@example", "[name]@example"),
            ("Marrow Quill", "[name]"),
            ("Marrow, Quill", "[name], [name]"),
            ("Marrow wrote it", "[name] wrote it"),
            ("ask QUILL", "ask [name]"),
            ("Quillon and Marrowbone stay", "Quillon and Marrowbone stay"),
        ] {
            assert_eq!(sanitize(text, &private), expected, "{text}");
        }
    }

    #[test]
    fn common_words_in_a_name_and_generic_account_names_are_kept() {
        let private = PrivateTerms {
            home_dirs: vec![],
            user_names: vec![
                "Will Brennerton".into(),
                "user".into(),
                "Admin".into(),
                "owner".into(),
                "default".into(),
            ],
        };
        let prose = "We will see what the user, the admin and the owner chose by default.";
        assert_eq!(sanitize(prose, &private), prose);
        assert_eq!(
            sanitize("Will Brennerton and Brennerton.", &private),
            "[name] and [name]."
        );
    }

    #[test]
    fn a_display_name_is_added_once() {
        let mut private = PrivateTerms::default();
        private.add_name("Quill Marrow");
        private.add_name(" Quill Marrow ");
        private.add_name("  ");
        assert_eq!(private.user_names, vec!["Quill Marrow".to_string()]);
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
        assert_eq!(path_patterns().len(), PATH_PATTERNS);
        assert!(uuid_pattern().is_some());
        assert!(encoded_pattern().is_some());
        assert!(home_pattern(r"C:\Users\Ada Lovelace").is_some());
        assert!(home_pattern("/").is_none(), "a root is not a home folder");
    }
}
