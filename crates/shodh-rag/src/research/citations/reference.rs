//! Parsing one bibliography entry into authors, title, year, venue, DOI and arXiv id.
//!
//! Deterministic rules only (regular expressions and the shape of the common styles); no
//! model is involved. Each field carries its own confidence, so a caller can trust the
//! DOI of an entry whose title it could not find.
//!
//! The styles recognised:
//! - **IEEE**: `[1] I. Schlag, K. Irie, and J. Schmidhuber, “Linear transformers …,” in
//!   Proc. ICML, 2021, pp. 9355–9366.` (the title is quoted);
//! - **APA**: `Schlag, I., Irie, K., & Schmidhuber, J. (2021). Linear transformers ….
//!   In Proceedings of ICML.` (the year follows the authors in parentheses);
//! - **ACL**: `Imanol Schlag, Kazuki Irie, and Jürgen Schmidhuber. 2021. Linear
//!   transformers …. In Proceedings of ICML.` (the year follows the authors);
//! - **author–title–year** (NeurIPS, ICML, ICLR and most numbered styles):
//!   `Schlag, I., Irie, K., and Schmidhuber, J. Linear transformers …. In ICML, 2021.`
//!
//! Identifiers are read first and removed before the year is looked for, so the digits of
//! an arXiv id (`2010.11929`) or a DOI are never taken for a year.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::text::{clean_block_text, normalize_surname};
use crate::harness::web::papers::{normalize_arxiv_id, normalize_doi};

/// One author as printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorName {
    /// Given names or initials as printed (`Kazuki`, `K.`, `J.-P.`), possibly empty.
    pub given: String,
    /// Family name as printed (`Irie`, `van den Oord`).
    pub family: String,
}

impl AuthorName {
    /// `Given Family`, or the family name alone.
    pub fn display(&self) -> String {
        if self.given.is_empty() {
            self.family.clone()
        } else {
            format!("{} {}", self.given, self.family)
        }
    }

    /// Comparable surname (diacritics folded, letters only, lower case). For a name with
    /// particles (`van den Oord`) this is the last word (`oord`).
    pub fn surname_key(&self) -> String {
        let last = self
            .family
            .split_whitespace()
            .last()
            .unwrap_or(&self.family);
        normalize_surname(last)
    }
}

/// The citation style an entry was read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationStyle {
    /// Quoted title.
    Ieee,
    /// `Authors (Year). Title.`
    Apa,
    /// `Authors. Year. Title.`
    Acl,
    /// `Authors. Title. Venue, Year.`
    AuthorTitleYear,
    /// Only identifiers or fragments could be read.
    Unknown,
}

/// Confidence in `[0, 1]` of each parsed field (`0` when the field is absent).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConfidence {
    pub authors: f64,
    pub title: f64,
    pub year: f64,
    pub venue: f64,
    pub doi: f64,
    pub arxiv_id: f64,
}

/// One parsed bibliography entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedReference {
    /// The entry as one line (ligatures expanded, line-break hyphens joined), with its
    /// leading marker (`[12]`, `12.`) removed.
    pub text: String,
    /// The marker as printed (`12`, `Vas+17`), when the entry has one.
    pub marker: Option<String>,
    pub authors: Vec<AuthorName>,
    /// The list ended with `et al.` (more authors than printed).
    pub et_al: bool,
    pub title: Option<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    /// Lower-cased DOI (`10.1234/abc`).
    pub doi: Option<String>,
    /// arXiv id without version (`2102.11174`).
    pub arxiv_id: Option<String>,
    pub style: CitationStyle,
    pub confidence: FieldConfidence,
}

impl ParsedReference {
    /// Comparable surname of the first author.
    pub fn first_surname(&self) -> Option<String> {
        self.authors
            .first()
            .map(AuthorName::surname_key)
            .filter(|s| !s.is_empty())
    }

    /// Whether the entry identifies a work well enough to become a graph node: an
    /// identifier, or a title with a year or an author.
    pub fn is_identifiable(&self) -> bool {
        if self.doi.is_some() || self.arxiv_id.is_some() {
            return true;
        }
        let Some(title) = &self.title else {
            return false;
        };
        // Without authors, only a title-shaped text counts (not `pages 1–10`).
        let titled = !self.authors.is_empty()
            || (title.split_whitespace().count() >= 3
                && title.chars().next().is_some_and(char::is_uppercase));
        titled && self.confidence.title >= 0.5 && (self.year.is_some() || !self.authors.is_empty())
    }

    /// Overall confidence: the weakest of the fields that identify the work.
    pub fn confidence_overall(&self) -> f64 {
        if self.doi.is_some() || self.arxiv_id.is_some() {
            return self.confidence.doi.max(self.confidence.arxiv_id);
        }
        let mut c = self.confidence.title;
        if self.year.is_some() {
            c = c.min(self.confidence.year.max(0.5));
        }
        if !self.authors.is_empty() {
            c = c.min(self.confidence.authors.max(0.5));
        }
        c
    }
}

static RE_MARKER: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:\[(?P<a>[A-Za-z0-9+.\- ]{1,12})\]|\((?P<b>\d{1,4})\)|(?P<c>\d{1,4})\.(?:\s|$))\s*",
    )
    .ok()
}));
static RE_DOI: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(r"(?i)(?:https?://(?:dx\.)?doi\.org/|doi:\s*)?(?P<doi>10\.\d{4,9}/[^\s,;]+)").ok()
}));
static RE_ARXIV: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:arxiv\s*(?:preprint)?\s*[: ]\s*(?:arxiv:)?\s*|arxiv\.org/(?:abs|pdf)/|corr,?\s*abs/|\babs/)(?P<id>\d{4}\.\d{4,5}(?:v\d+)?|[a-z\-]+(?:\.[a-z]{2})?/\d{7}(?:v\d+)?)",
    )
    .ok()
}));
static RE_URL: Pattern = Pattern(LazyLock::new(|| Regex::new(r"(?i)\bhttps?://\S+").ok()));
static RE_YEAR: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(r"(?:^|[^0-9A-Za-z])(?P<y>(?:19[5-9]\d|20[0-4]\d))[a-z]?(?:[^0-9]|$)").ok()
}));
static RE_LEADING_YEAR: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(r"^\(?(?P<y>(?:19[5-9]\d|20[0-4]\d))[a-z]?\)?(?:\s*[.,:]|\s|$)\s*").ok()
}));
static RE_QUOTED: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(r#"[“"‘'']{1,2}(?P<t>[^“”"]{8,400}?)[,.]?\s*[”"’'']{1,2}"#).ok()
}));
/// A given-name token: a capitalised word, an initial with its period (`J.`, `J.-P.`) or
/// a bare initial.
const GIVEN: &str = r"(?:\p{Lu}[\p{Ll}\p{Lo}'’\-]+(?:\p{Lu}[\p{Ll}]+)*(?:-\p{Lu}[\p{Ll}]+)?|\p{Lu}\.(?:\s?-?\p{Lu}\.)*|\p{Lu}(?:\s|$))";
/// A family name, with lower-case particles (`van den Oord`, `de Freitas`).
const FAMILY: &str = r"(?:(?:van|von|der|den|de|del|della|da|di|du|dos|das|la|le|ten|ter|al|el|bin|ibn)\s+)*(?:\p{Lu}[\p{L}'’\-]*\p{L}|\p{Lu}\p{Ll}?)";
static RE_SURNAME_FIRST: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?P<family>{FAMILY}),\s*(?P<given>\p{{Lu}}\.(?:\s?-?\p{{Lu}}\.)*|(?:\p{{Lu}}[\p{{Ll}}\-]+)(?:\s+\p{{Lu}}\.)*)"
    ))
    .ok()
}));
static RE_GIVEN_FIRST: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?P<given>(?:{GIVEN}\s*){{1,4}}?)\s*(?P<family>{FAMILY})(?:\s+(?:Jr\.|Sr\.|II|III|IV))?"
    ))
    .ok()
}));
/// A further family-name word right after a given-first name (`Florencia Leoni Aleman`,
/// `Alexei A Efros`): the name continues when a separator or the list's end follows it.
static RE_NAME_CONTINUES: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(&format!(r"^\s+(?P<family>{FAMILY})")).ok()
}));
static RE_SEPARATOR: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(r"^(?:\s*,\s*(?:and\s+|&\s*)?|\s+(?:and|&)\s+|\s*;\s*)").ok()
}));
static RE_ET_AL: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(r"^\s*,?\s*(?:et\s*al\.?|and\s+others\.?)").ok()
}));
static RE_VENUE_TAIL: Pattern = Pattern(LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:,|\.)?\s*(?:pp?\.\s*\d|pages?\s+\d|vol(?:ume)?\.?\s*\d|no\.\s*\d|\(\s*\d{1,4}\s*\)|\d+\s*\(\d+\)|\d+\s*[–-]\s*\d+|(?:19[5-9]\d|20[0-4]\d)\b|url\b|issn\b|isbn\b|publisher\b|$)",
    )
    .ok()
}));

/// A compiled pattern. Every pattern is a literal checked by the tests; should one fail to
/// compile, it matches nothing and the parser degrades instead of panicking.
struct Pattern(LazyLock<Option<Regex>>);

impl Pattern {
    fn get(&self) -> Option<&Regex> {
        self.0.as_ref()
    }

    fn is_match(&self, text: &str) -> bool {
        self.get().is_some_and(|r| r.is_match(text))
    }

    fn find<'t>(&self, text: &'t str) -> Option<regex::Match<'t>> {
        self.get().and_then(|r| r.find(text))
    }

    fn captures<'t>(&self, text: &'t str) -> Option<regex::Captures<'t>> {
        self.get().and_then(|r| r.captures(text))
    }

    fn captures_iter<'t>(&self, text: &'t str) -> Vec<regex::Captures<'t>> {
        self.get()
            .map(|r| r.captures_iter(text).collect())
            .unwrap_or_default()
    }

    fn replace_all(&self, text: &str, with: &str) -> String {
        match self.get() {
            Some(r) => r.replace_all(text, with).to_string(),
            None => text.to_string(),
        }
    }
}

/// Words that end with a period without ending a sentence.
const ABBREVIATIONS: &[&str] = &[
    "al", "vs", "e.g", "i.e", "proc", "conf", "int", "intl", "vol", "no", "pp", "adv", "trans",
    "j", "jr", "sr", "st", "dept", "univ", "eds", "ed", "inc", "ltd", "co", "fig", "eq", "sec",
    "assoc", "comput", "ling", "lett", "rev", "phys", "natl", "acad", "sci", "symp", "workshop",
    "mach", "learn", "res", "neural", "inf", "process", "syst", "math", "stat", "ann", "appl",
];

/// Venues recognised by name or abbreviation, with their canonical short name.
const VENUES: &[(&str, &str)] = &[
    ("neural information processing systems", "NeurIPS"),
    ("neurips", "NeurIPS"),
    ("nips", "NeurIPS"),
    ("international conference on machine learning", "ICML"),
    ("icml", "ICML"),
    (
        "international conference on learning representations",
        "ICLR",
    ),
    ("iclr", "ICLR"),
    (
        "annual meeting of the association for computational linguistics",
        "ACL",
    ),
    ("association for computational linguistics", "ACL"),
    ("empirical methods in natural language processing", "EMNLP"),
    ("emnlp", "EMNLP"),
    ("naacl", "NAACL"),
    ("north american chapter", "NAACL"),
    (
        "transactions of the association for computational linguistics",
        "TACL",
    ),
    ("tacl", "TACL"),
    ("computer vision and pattern recognition", "CVPR"),
    ("cvpr", "CVPR"),
    ("iccv", "ICCV"),
    ("international conference on computer vision", "ICCV"),
    ("eccv", "ECCV"),
    ("european conference on computer vision", "ECCV"),
    ("aaai", "AAAI"),
    ("ijcai", "IJCAI"),
    ("journal of machine learning research", "JMLR"),
    ("jmlr", "JMLR"),
    ("transactions on machine learning research", "TMLR"),
    ("tmlr", "TMLR"),
    ("conference on language modeling", "COLM"),
    ("colm", "COLM"),
    ("sigir", "SIGIR"),
    ("kdd", "KDD"),
    ("knowledge discovery and data mining", "KDD"),
    ("www", "WWW"),
    ("the web conference", "WWW"),
    ("uai", "UAI"),
    ("uncertainty in artificial intelligence", "UAI"),
    ("aistats", "AISTATS"),
    ("artificial intelligence and statistics", "AISTATS"),
    ("colt", "COLT"),
    ("conference on learning theory", "COLT"),
    ("interspeech", "Interspeech"),
    ("icassp", "ICASSP"),
    ("neural computation", "Neural Computation"),
    ("nature", "Nature"),
    ("science", "Science"),
    (
        "ieee transactions on pattern analysis and machine intelligence",
        "TPAMI",
    ),
    ("tpami", "TPAMI"),
    ("arxiv", "arXiv"),
    ("corr", "arXiv"),
];

fn strip_marker(text: &str) -> (Option<String>, &str) {
    match RE_MARKER.captures(text) {
        Some(c) => {
            let marker = c
                .name("a")
                .or_else(|| c.name("b"))
                .or_else(|| c.name("c"))
                .map(|m| m.as_str().trim().to_string());
            let end = c.get(0).map(|m| m.end()).unwrap_or(0);
            (marker, &text[end..])
        }
        None => (None, text),
    }
}

fn trim_identifier(raw: &str) -> &str {
    raw.trim_end_matches(['.', ',', ';', ')', ']', '}', '>', '"', '\''])
}

/// DOI, arXiv id and the text with identifiers and URLs blanked out.
fn take_identifiers(text: &str) -> (Option<String>, Option<String>, String) {
    let mut rest = text.to_string();
    let arxiv = RE_ARXIV
        .captures(text)
        .and_then(|c| c.name("id"))
        .and_then(|m| normalize_arxiv_id(trim_identifier(m.as_str())));
    let doi = RE_DOI
        .captures(text)
        .and_then(|c| c.name("doi"))
        .and_then(|m| normalize_doi(trim_identifier(m.as_str())));
    for pattern in [&RE_URL, &RE_DOI, &RE_ARXIV] {
        rest = pattern.replace_all(&rest, " ");
    }
    // An arXiv DOI names the preprint: keep it as the arXiv id too.
    let arxiv = arxiv.or_else(|| doi.as_deref().and_then(normalize_arxiv_id));
    (doi, arxiv, rest)
}

/// The author list at the start of `text`: the authors, whether it ended with `et al.`,
/// and where the list ends (byte offset, after its closing period if any).
pub(crate) fn take_authors(text: &str) -> (Vec<AuthorName>, bool, usize) {
    let surname_first = RE_SURNAME_FIRST.is_match(text)
        && !text
            .split(',')
            .next()
            .is_some_and(|first| first.split_whitespace().count() > 3);
    let mut authors = Vec::new();
    let mut pos = 0usize;
    let mut et_al = false;
    loop {
        let rest = &text[pos..];
        if let Some(m) = RE_ET_AL.find(rest) {
            if !authors.is_empty() {
                et_al = true;
                pos += m.end();
                break;
            }
        }
        let unit = if surname_first {
            RE_SURNAME_FIRST.captures(rest).map(|c| {
                (
                    c.name("given").map(|m| m.as_str().trim()).unwrap_or(""),
                    c.name("family").map(|m| m.as_str().trim()).unwrap_or(""),
                    c.get(0).map(|m| m.end()).unwrap_or(0),
                )
            })
        } else {
            RE_GIVEN_FIRST.captures(rest).map(|c| {
                (
                    c.name("given").map(|m| m.as_str().trim()).unwrap_or(""),
                    c.name("family").map(|m| m.as_str().trim()).unwrap_or(""),
                    c.get(0).map(|m| m.end()).unwrap_or(0),
                )
            })
        };
        let Some((given, family, end)) = unit.filter(|(_, f, end)| !f.is_empty() && *end > 0)
        else {
            break;
        };
        // A given-first "name" must have a given part; a lone capitalised word is the
        // start of a title, not an author.
        if !surname_first && given.is_empty() {
            break;
        }
        let mut given = given.trim_end_matches(',').trim().to_string();
        let mut family = family.trim_end_matches(['.', ',']).to_string();
        pos += end;
        if !surname_first {
            for _ in 0..2 {
                let rest = &text[pos..];
                let Some(c) = RE_NAME_CONTINUES.captures(rest) else {
                    break;
                };
                let (Some(whole), Some(next)) = (c.get(0), c.name("family")) else {
                    break;
                };
                let tail = &rest[whole.end()..];
                let ends_name = tail.starts_with(',')
                    || tail.starts_with(" and ")
                    || tail.starts_with(" &")
                    || tail.starts_with('.');
                if !ends_name {
                    break;
                }
                given = format!("{given} {family}");
                family = next.as_str().to_string();
                pos += whole.end();
            }
        }
        authors.push(AuthorName { given, family });
        let after = &text[pos..];
        if let Some(m) = RE_ET_AL.find(after) {
            et_al = true;
            pos += m.end();
            break;
        }
        match RE_SEPARATOR.find(after) {
            Some(m) if m.end() > 0 => {
                let next = &after[m.end()..];
                let continues = RE_ET_AL.is_match(next)
                    || if surname_first {
                        RE_SURNAME_FIRST.is_match(next)
                    } else {
                        RE_GIVEN_FIRST.captures(next).is_some_and(|c| {
                            c.name("given")
                                .is_some_and(|g| !g.as_str().trim().is_empty())
                        })
                    };
                if !continues {
                    break;
                }
                pos += m.end();
            }
            _ => break,
        }
    }
    if authors.is_empty() {
        return (authors, false, 0);
    }
    // Close the list: a period (unless the last unit already ended with an initial's
    // period), then spaces.
    let rest = &text[pos..];
    let trimmed = rest.trim_start_matches([' ', ',']);
    let mut end = pos + (rest.len() - trimmed.len());
    if trimmed.starts_with('.') || trimmed.starts_with(':') {
        end += 1;
    }
    let tail = &text[end..];
    end += tail.len() - tail.trim_start().len();
    (authors, et_al, end)
}

/// Byte offset just past the first sentence end in `text` (`.`, `?` or `!` followed by a
/// space and an upper-case letter, digit or bracket), skipping abbreviations and initials.
fn sentence_end(text: &str) -> Option<usize> {
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, c)) in bytes.iter().enumerate() {
        if !matches!(c, '.' | '?' | '!') {
            continue;
        }
        let next = bytes.get(k + 1).map(|&(_, n)| n);
        let after = bytes.get(k + 2).map(|&(_, n)| n);
        let ends_text = next.is_none();
        let next_word: String = text[i + c.len_utf8()..]
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();
        let venue_follows =
            next_word.starts_with("arxiv") || next_word == "in" || next_word == "in:";
        let boundary = ends_text
            || (next.is_some_and(char::is_whitespace)
                && (venue_follows
                    || after.is_some_and(|a| {
                        a.is_uppercase() || a.is_ascii_digit() || a == '(' || a == '['
                    })));
        if !boundary {
            continue;
        }
        if c == '.' {
            let word: String = text[..i]
                .rsplit(|ch: char| ch.is_whitespace() || ch == '(')
                .next()
                .unwrap_or("")
                .to_string();
            let lower = word.to_lowercase();
            let initial = word.chars().count() == 1 && word.chars().all(char::is_uppercase);
            if initial || ABBREVIATIONS.contains(&lower.as_str()) {
                continue;
            }
        }
        return Some(i + c.len_utf8());
    }
    None
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

static TRAILING_YEAR: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r",\s*(?:19[5-9]\d|20[0-4]\d)[a-z]?\s*[.,]?$").ok());

fn clean_title(raw: &str) -> Option<String> {
    // `Title, 2023.`: the year printed after an unpublished work's title.
    let raw = match TRAILING_YEAR.as_ref() {
        Some(re) => re.replace(raw.trim(), "").to_string(),
        None => raw.to_string(),
    };
    let t = raw
        .trim()
        .trim_start_matches(['“', '"', '‘', '\''])
        .trim_end_matches(|c: char| {
            c.is_whitespace() || matches!(c, '”' | '"' | '’' | '\'' | ',' | '.' | ';')
        });
    (word_count(t) >= 1 && t.chars().any(char::is_alphabetic)).then(|| t.to_string())
}

fn title_confidence(title: &str, base: f64) -> f64 {
    let words = word_count(title);
    if words < 2 {
        base * 0.6
    } else if words > 40 {
        base * 0.5
    } else {
        base
    }
}

/// The venue named in `rest` (the text after the title), cut before pages, volume and year,
/// and its confidence: higher when it names a known venue.
fn venue_of(rest: &str) -> (Option<String>, f64) {
    let rest = rest
        .trim()
        .trim_start_matches(['.', ',', ' '])
        .trim_start_matches("In: ")
        .trim_start_matches("In ")
        .trim_start_matches("in ")
        .trim();
    if rest.is_empty() {
        return (None, 0.0);
    }
    let lower = rest.to_lowercase();
    // The longest known venue named anywhere in the remainder.
    let known = VENUES
        .iter()
        .filter(|(needle, _)| contains_word(&lower, needle))
        .max_by_key(|(needle, _)| needle.len())
        .map(|(_, name)| (*name).to_string());
    if let Some(name) = known {
        return (Some(name), 0.85);
    }
    let cut = RE_VENUE_TAIL
        .find(rest)
        .map(|m| m.start())
        .unwrap_or(rest.len());
    let venue = rest[..cut]
        .trim()
        .trim_end_matches([',', '.', ';', ':'])
        .trim()
        .to_string();
    if venue.chars().filter(|c| c.is_alphabetic()).count() < 3 || word_count(&venue) > 20 {
        return (None, 0.0);
    }
    (Some(venue), 0.6)
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut start = 0;
    while let Some(found) = haystack[start..].find(needle) {
        let at = start + found;
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + needle.len()..].chars().next();
        if before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
        {
            return true;
        }
        start = at + needle.len().max(1);
        if start >= haystack.len() {
            break;
        }
    }
    false
}

fn last_year(text: &str) -> (Option<i32>, usize) {
    let years: Vec<i32> = RE_YEAR
        .captures_iter(text)
        .into_iter()
        .filter_map(|c| c.name("y").and_then(|m| m.as_str().parse().ok()))
        .collect();
    let distinct = {
        let mut d = years.clone();
        d.sort_unstable();
        d.dedup();
        d.len()
    };
    (years.last().copied(), distinct)
}

/// Parses one bibliography entry. Never fails: fields that cannot be read are `None` with
/// confidence `0`.
pub fn parse_reference(raw: &str) -> ParsedReference {
    let cleaned = clean_block_text(raw)
        // An identifier broken across lines: `arxiv.org/abs/ 2401.12973`.
        .replace("abs/ ", "abs/")
        .replace("pdf/ ", "pdf/")
        .replace("doi.org/ ", "doi.org/");
    let (marker, body) = strip_marker(&cleaned);
    let body = body.trim().to_string();
    let (doi, arxiv_id, plain) = take_identifiers(&body);
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut confidence = FieldConfidence {
        doi: if doi.is_some() { 0.98 } else { 0.0 },
        arxiv_id: if arxiv_id.is_some() { 0.97 } else { 0.0 },
        ..FieldConfidence::default()
    };

    let (authors, et_al, after_authors) = take_authors(&plain);
    let rest = plain[after_authors..].trim();
    let mut style = CitationStyle::Unknown;
    let mut title: Option<String> = None;
    let mut year: Option<i32> = None;
    let mut venue_text = String::new();

    if let Some(q) = RE_QUOTED
        .captures(rest)
        .filter(|c| c.name("t").is_some_and(|t| word_count(t.as_str()) >= 2))
    {
        // IEEE: the title is the quoted part.
        style = CitationStyle::Ieee;
        title = q.name("t").and_then(|t| clean_title(t.as_str()));
        if let Some(t) = &title {
            confidence.title = title_confidence(t, 0.95);
        }
        venue_text = rest[q.get(0).map(|m| m.end()).unwrap_or(0)..].to_string();
    } else if let Some(c) = RE_LEADING_YEAR
        .captures(rest)
        .filter(|_| !authors.is_empty())
    {
        // APA `(2021).` or ACL `2021.` right after the authors.
        style = if rest.starts_with('(') {
            CitationStyle::Apa
        } else {
            CitationStyle::Acl
        };
        year = c.name("y").and_then(|m| m.as_str().parse().ok());
        confidence.year = 0.95;
        let after = &rest[c.get(0).map(|m| m.end()).unwrap_or(0)..];
        let end = sentence_end(after).unwrap_or(after.len());
        title = clean_title(&after[..end]);
        if let Some(t) = &title {
            confidence.title = title_confidence(t, 0.85);
        }
        venue_text = after[end..].to_string();
    } else if !rest.is_empty() {
        let end = sentence_end(rest).unwrap_or(rest.len());
        let candidate = &rest[..end];
        title = clean_title(candidate);
        if let Some(t) = &title {
            style = CitationStyle::AuthorTitleYear;
            let base = if authors.is_empty() { 0.55 } else { 0.8 };
            confidence.title = title_confidence(t, base);
        }
        venue_text = rest[end..].to_string();
    }

    if year.is_none() {
        let (found, distinct) = last_year(&venue_text);
        let (found, distinct) = match found {
            Some(y) => (Some(y), distinct),
            None => last_year(rest),
        };
        year = found;
        confidence.year = match (found, distinct) {
            (None, _) => 0.0,
            (Some(_), 0 | 1) => 0.85,
            (Some(_), _) => 0.6,
        };
    }

    let (venue, venue_confidence) = if arxiv_id.is_some() && venue_text.trim().is_empty() {
        (Some("arXiv".to_string()), 0.8)
    } else {
        venue_of(&venue_text)
    };
    confidence.venue = venue_confidence;
    confidence.authors = if authors.is_empty() {
        0.0
    } else if style == CitationStyle::Unknown {
        0.5
    } else {
        0.9
    };
    if title.is_none() && (doi.is_some() || arxiv_id.is_some()) {
        style = CitationStyle::Unknown;
    }

    ParsedReference {
        text: body,
        marker,
        authors,
        et_al,
        title,
        year,
        venue,
        doi,
        arxiv_id,
        style,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surnames(r: &ParsedReference) -> Vec<String> {
        r.authors.iter().map(|a| a.family.clone()).collect()
    }

    #[test]
    fn patterns_compile() {
        for pattern in [
            &RE_MARKER,
            &RE_DOI,
            &RE_ARXIV,
            &RE_URL,
            &RE_YEAR,
            &RE_LEADING_YEAR,
            &RE_QUOTED,
            &RE_SURNAME_FIRST,
            &RE_GIVEN_FIRST,
            &RE_SEPARATOR,
            &RE_NAME_CONTINUES,
            &RE_ET_AL,
            &RE_VENUE_TAIL,
        ] {
            assert!(pattern.get().is_some());
        }
    }

    #[test]
    fn acl_style_reads_the_year_after_the_authors() {
        let r = parse_reference(
            "Imanol Schlag, Kazuki Irie, and Jürgen Schmidhuber. 2021. Linear transformers are secretly fast weight programmers. In International Conference on Machine Learning, pages 9355–9366. PMLR.",
        );
        assert_eq!(r.style, CitationStyle::Acl);
        assert_eq!(surnames(&r), ["Schlag", "Irie", "Schmidhuber"]);
        assert_eq!(r.authors[1].given, "Kazuki");
        assert_eq!(r.year, Some(2021));
        assert_eq!(
            r.title.as_deref(),
            Some("Linear transformers are secretly fast weight programmers")
        );
        assert_eq!(r.venue.as_deref(), Some("ICML"));
        assert_eq!(r.first_surname().as_deref(), Some("schlag"));
    }

    #[test]
    fn icml_style_reads_surname_first_authors_and_a_trailing_year() {
        let r = parse_reference(
            "Schlag, I., Irie, K., and Schmidhuber, J. Linear transformers are secretly fast weight programmers. In International Conference on Machine Learning, pp. 9355–9366. PMLR, 2021.",
        );
        assert_eq!(r.style, CitationStyle::AuthorTitleYear);
        assert_eq!(surnames(&r), ["Schlag", "Irie", "Schmidhuber"]);
        assert_eq!(r.authors[0].given, "I.");
        assert_eq!(r.year, Some(2021));
        assert_eq!(
            r.title.as_deref(),
            Some("Linear transformers are secretly fast weight programmers")
        );
    }

    #[test]
    fn neurips_style_with_initials_and_arxiv_preprint() {
        let r = parse_reference(
            "[12] S. Yang, B. Wang, Y. Zhang, Y. Shen, and Y. Kim. Parallelizing linear transformers with the delta rule over sequence length. arXiv preprint arXiv:2406.06484, 2024.",
        );
        assert_eq!(r.marker.as_deref(), Some("12"));
        assert_eq!(surnames(&r), ["Yang", "Wang", "Zhang", "Shen", "Kim"]);
        assert_eq!(r.arxiv_id.as_deref(), Some("2406.06484"));
        assert_eq!(r.year, Some(2024));
        assert_eq!(
            r.title.as_deref(),
            Some("Parallelizing linear transformers with the delta rule over sequence length")
        );
        assert_eq!(r.venue.as_deref(), Some("arXiv"));
    }

    #[test]
    fn ieee_style_takes_the_quoted_title() {
        let r = parse_reference(
            "[3] A. Vaswani, N. Shazeer, N. Parmar, J. Uszkoreit, L. Jones, A. N. Gomez, Ł. Kaiser, and I. Polosukhin, “Attention is all you need,” in Advances in Neural Information Processing Systems, 2017, pp. 5998–6008.",
        );
        assert_eq!(r.style, CitationStyle::Ieee);
        assert_eq!(r.title.as_deref(), Some("Attention is all you need"));
        assert_eq!(r.authors.len(), 8);
        assert_eq!(r.authors[0].family, "Vaswani");
        assert_eq!(r.year, Some(2017));
        assert_eq!(r.venue.as_deref(), Some("NeurIPS"));
        assert!(r.confidence.title >= 0.9);
    }

    #[test]
    fn apa_style_reads_the_parenthesised_year_and_a_doi() {
        let r = parse_reference(
            "Hochreiter, S., & Schmidhuber, J. (1997). Long short-term memory. Neural Computation, 9(8), 1735–1780. https://doi.org/10.1162/neco.1997.9.8.1735",
        );
        assert_eq!(r.style, CitationStyle::Apa);
        assert_eq!(surnames(&r), ["Hochreiter", "Schmidhuber"]);
        assert_eq!(r.year, Some(1997));
        assert_eq!(r.title.as_deref(), Some("Long short-term memory"));
        assert_eq!(r.doi.as_deref(), Some("10.1162/neco.1997.9.8.1735"));
        assert_eq!(r.venue.as_deref(), Some("Neural Computation"));
    }

    #[test]
    fn arxiv_digits_are_never_taken_for_a_year_and_et_al_is_kept() {
        let r = parse_reference(
            "Dosovitskiy, A. et al. An image is worth 16x16 words: Transformers for image recognition at scale. CoRR, abs/2010.11929, 2020.",
        );
        assert!(r.et_al);
        assert_eq!(r.arxiv_id.as_deref(), Some("2010.11929"));
        assert_eq!(r.year, Some(2020));
        assert_eq!(
            r.title.as_deref(),
            Some("An image is worth 16x16 words: Transformers for image recognition at scale")
        );
    }

    #[test]
    fn particles_stay_in_the_family_name() {
        let r = parse_reference(
            "Aaron van den Oord, Sander Dieleman, and Heiga Zen. 2016. WaveNet: A generative model for raw audio. arXiv preprint arXiv:1609.03499.",
        );
        assert_eq!(r.authors[0].family, "van den Oord");
        assert_eq!(r.authors[0].surname_key(), "oord");
        assert_eq!(
            r.title.as_deref(),
            Some("WaveNet: A generative model for raw audio")
        );
        assert_eq!(r.arxiv_id.as_deref(), Some("1609.03499"));
    }

    #[test]
    fn fragments_without_identifying_fields_are_not_identifiable() {
        let r = parse_reference("pages 1–10. Springer, 2019.");
        assert!(!r.is_identifiable());
        let r = parse_reference("arXiv:2102.11174");
        assert!(r.is_identifiable());
        assert_eq!(r.arxiv_id.as_deref(), Some("2102.11174"));
    }
}
