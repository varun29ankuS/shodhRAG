//! What a PDF in the library is: its title, DOI or arXiv id, authors and abstract, read
//! from the document itself.
//!
//! Sources, strongest first:
//! - the arXiv stamp in the first page's margin (`arXiv:2406.06484v1 [cs.LG] 9 Jun 2024`);
//! - a DOI printed on the first page with its label (`DOI: 10.…`, `https://doi.org/10.…`);
//! - the PDF's Info dictionary (`Subject`, `Keywords` may carry a DOI; `Title` the title);
//! - the Title block of the layout parser (the chunker drops it, so the index lacks it).
//!
//! An arXiv id in the file name (`2406.06484 - Title.pdf`) is a hint only: it is kept in
//! [`LocalIdentity::filename_arxiv_id`] and used once a lookup shows that the work it
//! names has this paper's title.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::reference::{take_authors, AuthorName};
use super::text::clean_block_text;
use crate::harness::web::papers::{normalize_arxiv_id, normalize_doi};
use crate::processing::document_model::{BlockKind, StructuredDocument};

/// Where an identifier came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierSource {
    /// Printed on the first page.
    FirstPage,
    /// The PDF's Info dictionary.
    PdfInfo,
}

/// Where the title came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    /// The layout parser's Title block.
    TitleBlock,
    /// The PDF's Info dictionary.
    PdfInfo,
}

/// The identity of one library PDF.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalIdentity {
    pub title: Option<String>,
    pub title_source: Option<TitleSource>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub identifier_source: Option<IdentifierSource>,
    /// An arXiv id in the file name: unverified until a lookup confirms the title.
    pub filename_arxiv_id: Option<String>,
    /// Authors read from the title page (low confidence: affiliations interleave).
    pub authors: Vec<AuthorName>,
    /// Year from the arXiv stamp or id.
    pub year: Option<i32>,
    /// The abstract, when a block under an `Abstract` heading or starting with it exists.
    pub abstract_text: Option<String>,
}

/// Text fields of the PDF's Info dictionary.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PdfInfo {
    pub title: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
}

static ARXIV_STAMP: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)arXiv:\s*(?P<id>\d{4}\.\d{4,5})(?:v\d+)?\s*\[[A-Za-z.\-]+\]").ok()
});
static LABELLED_DOI: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\bdoi\s*[:=]?\s*|https?://(?:dx\.)?doi\.org/)(?P<doi>10\.\d{4,9}/[^\s,;]+)")
        .ok()
});
static ANY_DOI: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?P<doi>10\.\d{4,9}/[^\s,;]+)").ok());
static FILENAME_ARXIV: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(?P<id>\d{4}\.\d{4,5})(?:v\d+)?(?:[\s_\-.]|$)").ok());
/// Affiliation marks right after a name (`Yang1`, `Kim∗†`, `Wang1,2`): they separate the
/// names of a title page, which are often printed without commas.
static NAME_MARKS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(\p{L})[\d∗*†‡§¶⋆♠♣♦♥♡♢⋄]+(?:,[\d∗*†‡§¶⋆♠♣♦♥♡♢⋄]+)*").ok());
static TITLE_PAGE_NOISE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"[\w.+\-]+@[\w\-]+(?:\.[\w\-]+)+|[∗*†‡§¶⋆♠♣♦♥♡♢⋄]|\{[^}]*\}|\b\d+\b").ok()
});

fn trim_identifier(raw: &str) -> &str {
    raw.trim_end_matches(['.', ',', ';', ')', ']', '}'])
}

/// The year of a new-style arXiv id (`2406.06484` → 2024).
pub fn arxiv_year(id: &str) -> Option<i32> {
    let yy: i32 = id.get(..2)?.parse().ok()?;
    let mm: i32 = id.get(2..4)?.parse().ok()?;
    (id.as_bytes().get(4) == Some(&b'.') && (1..=12).contains(&mm)).then_some(2000 + yy)
}

/// The arXiv id a file name starts with (`2406.06484 - Title.pdf`).
pub fn filename_arxiv_id(file_name: &str) -> Option<String> {
    FILENAME_ARXIV
        .as_ref()?
        .captures(file_name.trim())
        .and_then(|c| c.name("id"))
        .and_then(|m| normalize_arxiv_id(m.as_str()))
}

/// Whether an Info-dictionary title looks like a real title rather than a producer's
/// placeholder (`Microsoft Word - draft.docx`, `untitled`, a file name).
fn plausible_info_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    let words = title.split_whitespace().count();
    (3..=40).contains(&words)
        && ![
            "microsoft word",
            "untitled",
            ".doc",
            ".tex",
            ".pdf",
            "latex",
            "powerpoint",
        ]
        .iter()
        .any(|bad| lower.contains(bad))
}

/// Whether text reads as a section heading rather than a title (`1 Introduction`,
/// `Abstract`).
fn looks_like_heading(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let unnumbered = lower.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
    unnumbered.len() < lower.len()
        || [
            "abstract",
            "introduction",
            "contents",
            "preface",
            "acknowledgements",
            "acknowledgments",
        ]
        .contains(&unnumbered)
}

fn plausible_title(text: &str) -> bool {
    let words = text.split_whitespace().count();
    (2..=40).contains(&words)
        && text.chars().any(char::is_alphabetic)
        && !text.contains('@')
        && !looks_like_heading(text)
}

/// The title: the Title block, unless it reads as a heading; then the first paragraph
/// of the first page that reads as a title. A title line ending with `,` or `:` takes the
/// next line too (titles set on two lines).
fn title_from_block(doc: &StructuredDocument) -> Option<String> {
    let block = doc
        .blocks
        .iter()
        .find(|b| matches!(b.kind, BlockKind::Title))?;
    let title = clean_block_text(&block.text);
    if plausible_title(&title) {
        return Some(title);
    }
    // The parser took a heading for the title: the real one is among the first lines.
    let first_page: Vec<&crate::processing::document_model::Block> = doc
        .blocks
        .iter()
        .filter(|b| b.page.is_none_or(|p| p == 1))
        .take(6)
        .collect();
    for (i, block) in first_page.iter().enumerate() {
        if !matches!(block.kind, BlockKind::Paragraph | BlockKind::Title) {
            continue;
        }
        let mut title = clean_block_text(&block.text);
        if title.ends_with(',') || title.ends_with(':') {
            if let Some(next) = first_page.get(i + 1) {
                title = format!("{title} {}", clean_block_text(&next.text));
            }
        }
        if plausible_title(&title) && title.split_whitespace().count() >= 3 {
            return Some(title);
        }
        // Only the first plausible block is tried: later ones are authors or body text.
        break;
    }
    None
}

fn first_page_text(doc: &StructuredDocument) -> String {
    doc.blocks
        .iter()
        .filter(|b| b.page.is_none_or(|p| p == 1))
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Authors from the block right after the Title block on the first page, when it reads as
/// an author list once footnote marks, e-mails and affiliation numbers are removed.
fn title_page_authors(doc: &StructuredDocument) -> Vec<AuthorName> {
    let Some(title_at) = doc
        .blocks
        .iter()
        .position(|b| matches!(b.kind, BlockKind::Title))
    else {
        return Vec::new();
    };
    for block in doc.blocks.iter().skip(title_at + 1).take(3) {
        if block.page.is_some_and(|p| p != 1) || matches!(block.kind, BlockKind::Heading { .. }) {
            break;
        }
        let text = clean_block_text(&block.text);
        let text = match NAME_MARKS.as_ref() {
            Some(re) => re.replace_all(&text, "$1, ").to_string(),
            None => text,
        };
        let text = match TITLE_PAGE_NOISE.as_ref() {
            Some(re) => re.replace_all(&text, " ").to_string(),
            None => text,
        };
        let text = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(" ,", ",")
            .replace(",,", ",");
        let text = text.trim().trim_end_matches(',').to_string();
        if text.split_whitespace().count() > 60 {
            continue;
        }
        let (authors, _, _) = take_authors(&text);
        // Affiliations follow the names on many title pages: stop at the first "name"
        // that is an institution.
        let authors: Vec<AuthorName> = authors
            .into_iter()
            .take_while(|a| !is_institution(&a.display()))
            .collect();
        if !authors.is_empty() {
            return authors;
        }
    }
    Vec::new()
}

fn is_institution(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        "university",
        "institute",
        "college",
        "school",
        "laboratory",
        "lab",
        "research",
        "google",
        "microsoft",
        "meta",
        "deepmind",
        "openai",
        "technology",
        "department",
        "center",
        "centre",
        "inc",
        "ltd",
        "corporation",
        "academy",
        "foundation",
    ]
    .iter()
    .any(|w| lower.split(|c: char| !c.is_alphanumeric()).any(|t| t == *w))
}

fn abstract_of(doc: &StructuredDocument) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for block in &doc.blocks {
        let under_heading = block
            .section_path
            .last()
            .is_some_and(|h| h.trim().eq_ignore_ascii_case("abstract"));
        let text = clean_block_text(&block.text);
        let starts = text
            .get(..8)
            .is_some_and(|head| head.eq_ignore_ascii_case("abstract"));
        if matches!(block.kind, BlockKind::Paragraph) && (under_heading || starts) {
            let body = if starts {
                text[8..]
                    .trim_start_matches([' ', '.', ':', '—', '-'])
                    .to_string()
            } else {
                text
            };
            parts.push(body);
        } else if !parts.is_empty() {
            break;
        }
        if parts.iter().map(String::len).sum::<usize>() > 3_000 {
            break;
        }
    }
    let text = parts.join(" ");
    (text.split_whitespace().count() >= 20).then_some(text)
}

/// Reads the identity of a parsed PDF. `info` is the PDF's Info dictionary and `file_name`
/// the file's name (its arXiv id, if any, is kept as an unverified hint).
pub fn identify(doc: &StructuredDocument, info: &PdfInfo, file_name: &str) -> LocalIdentity {
    let page_one = clean_block_text(&first_page_text(doc));
    let mut identity = LocalIdentity {
        filename_arxiv_id: filename_arxiv_id(file_name),
        abstract_text: abstract_of(doc),
        authors: title_page_authors(doc),
        ..LocalIdentity::default()
    };

    if let Some(id) = ARXIV_STAMP
        .as_ref()
        .and_then(|re| re.captures(&page_one))
        .and_then(|c| c.name("id"))
        .and_then(|m| normalize_arxiv_id(m.as_str()))
    {
        identity.year = arxiv_year(&id);
        identity.arxiv_id = Some(id);
        identity.identifier_source = Some(IdentifierSource::FirstPage);
    }
    if let Some(doi) = LABELLED_DOI
        .as_ref()
        .and_then(|re| re.captures(&page_one))
        .and_then(|c| c.name("doi"))
        .and_then(|m| normalize_doi(trim_identifier(m.as_str())))
    {
        if identity.arxiv_id.is_none() {
            identity.arxiv_id = normalize_arxiv_id(&doi);
        }
        identity.doi = Some(doi);
        identity.identifier_source = Some(IdentifierSource::FirstPage);
    }
    if identity.doi.is_none() {
        let info_text = [info.subject.as_deref(), info.keywords.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(doi) = ANY_DOI
            .as_ref()
            .and_then(|re| re.captures(&info_text))
            .and_then(|c| c.name("doi"))
            .and_then(|m| normalize_doi(trim_identifier(m.as_str())))
        {
            identity.doi = Some(doi);
            identity
                .identifier_source
                .get_or_insert(IdentifierSource::PdfInfo);
        }
    }

    if let Some(title) = title_from_block(doc) {
        identity.title = Some(title);
        identity.title_source = Some(TitleSource::TitleBlock);
    } else if let Some(title) = info
        .title
        .as_deref()
        .map(clean_block_text)
        .filter(|t| plausible_info_title(t))
    {
        identity.title = Some(title);
        identity.title_source = Some(TitleSource::PdfInfo);
    }
    identity
}

/// The Info dictionary of the PDF in `bytes` (empty when it has none or cannot be read).
pub fn read_pdf_info(bytes: &[u8]) -> PdfInfo {
    use lopdf::{Document, Object};
    let Ok(doc) = Document::load_mem(bytes) else {
        return PdfInfo::default();
    };
    let info = doc.trailer.get(b"Info").ok().and_then(|obj| match obj {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    });
    let Some(dict) = info.and_then(|o| o.as_dict().ok()) else {
        return PdfInfo::default();
    };
    let field = |key: &[u8]| {
        dict.get(key)
            .ok()
            .and_then(|o| o.as_str().ok())
            .map(crate::processing::lopdf_parser::decode_pdf_string)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    PdfInfo {
        title: field(b"Title"),
        subject: field(b"Subject"),
        keywords: field(b"Keywords"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::document_model::{Block, PageInfo};

    fn doc(blocks: Vec<Block>) -> StructuredDocument {
        StructuredDocument {
            pages: vec![PageInfo {
                number: 1,
                width: 612.0,
                height: 792.0,
            }],
            blocks,
        }
    }

    #[test]
    fn the_arxiv_stamp_and_title_block_identify_a_preprint() {
        let d = doc(vec![
            Block::new(
                BlockKind::Paragraph,
                "arXiv:2406.06484v6 [cs.LG] 3 Jan 2025",
            )
            .on_page(1, None),
            Block::new(
                BlockKind::Title,
                "Parallelizing Linear Transformers with the Delta Rule\nover Sequence Length",
            )
            .on_page(1, None),
            Block::new(
                BlockKind::Paragraph,
                "Songlin Yang1 Bailin Wang1 Yu Zhang2 Yikang Shen3 Yoon Kim1",
            )
            .on_page(1, None),
            Block::new(BlockKind::Heading { level: 1 }, "Abstract").on_page(1, None),
        ]);
        let id = identify(&d, &PdfInfo::default(), "2406.06484 - Parallelizing.pdf");
        assert_eq!(id.arxiv_id.as_deref(), Some("2406.06484"));
        assert_eq!(id.identifier_source, Some(IdentifierSource::FirstPage));
        assert_eq!(id.year, Some(2024));
        assert_eq!(
            id.title.as_deref(),
            Some("Parallelizing Linear Transformers with the Delta Rule over Sequence Length")
        );
        assert_eq!(id.filename_arxiv_id.as_deref(), Some("2406.06484"));
        let surnames: Vec<&str> = id.authors.iter().map(|a| a.family.as_str()).collect();
        assert_eq!(surnames, ["Yang", "Wang", "Zhang", "Shen", "Kim"]);
    }

    #[test]
    fn info_titles_are_used_only_when_plausible_and_info_dois_are_read() {
        let d = doc(vec![
            Block::new(BlockKind::Paragraph, "Some body text.").on_page(1, None)
        ]);
        let info = PdfInfo {
            title: Some("Microsoft Word - final.docx".into()),
            subject: Some("Neural Computation 9(8), doi:10.1162/neco.1997.9.8.1735".into()),
            keywords: None,
        };
        let id = identify(&d, &info, "lstm.pdf");
        assert_eq!(id.title, None);
        assert_eq!(id.doi.as_deref(), Some("10.1162/neco.1997.9.8.1735"));
        assert_eq!(id.identifier_source, Some(IdentifierSource::PdfInfo));
        assert_eq!(id.filename_arxiv_id, None);
        let info = PdfInfo {
            title: Some("Long Short-Term Memory".into()),
            ..PdfInfo::default()
        };
        assert_eq!(
            identify(&d, &info, "x.pdf").title_source,
            Some(TitleSource::PdfInfo)
        );
    }

    #[test]
    fn a_heading_taken_for_the_title_falls_back_to_the_first_page_lines() {
        let d = doc(vec![
            Block::new(
                BlockKind::Paragraph,
                "It’s All Connected: A Journey Through Test-Time Memorization,",
            )
            .on_page(1, None),
            Block::new(
                BlockKind::Paragraph,
                "Attentional Bias, Retention, and Online Optimization",
            )
            .on_page(1, None),
            Block::new(BlockKind::Title, "1 Introduction").on_page(1, None),
        ]);
        let id = identify(&d, &PdfInfo::default(), "paper.pdf");
        assert_eq!(
            id.title.as_deref(),
            Some("It’s All Connected: A Journey Through Test-Time Memorization, Attentional Bias, Retention, and Online Optimization")
        );
        let d = doc(vec![
            Block::new(BlockKind::Title, "A Title").on_page(1, None),
            Block::new(
                BlockKind::Paragraph,
                "Songlin Yang⋄ Yoon Kim⋄ ⋄Massachusetts Institute of Technology",
            )
            .on_page(1, None),
        ]);
        let surnames: Vec<String> = identify(&d, &PdfInfo::default(), "x.pdf")
            .authors
            .iter()
            .map(|a| a.family.clone())
            .collect();
        assert_eq!(surnames, ["Yang", "Kim"]);
    }

    #[test]
    fn arxiv_years_and_file_name_hints() {
        assert_eq!(arxiv_year("2102.11174"), Some(2021));
        assert_eq!(arxiv_year("2113.11174"), None);
        assert_eq!(
            filename_arxiv_id("2411.12537 - Unlocking State-Tracking.pdf").as_deref(),
            Some("2411.12537")
        );
        assert_eq!(
            filename_arxiv_id("KAN - Kolmogorov-Arnold Networks.pdf"),
            None
        );
    }
}
