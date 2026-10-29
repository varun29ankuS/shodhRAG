//! The research-paper corpus and its dataset: a synthetic library of 21
//! papers. Seven are hand-written ([`crate::papers_text`]): six paged PDFs
//! laid out like preprints (title, authors, numbered sections, display
//! equations, tables, figure captions, a bibliography; one long survey) and
//! one paper as LaTeX source. Fourteen are generated with a seeded generator
//! around hand-written anchor sentences ([`crate::papers_gen`]): twelve
//! method papers, a long thesis and a long survey, so that the library is
//! several times larger than the candidate pool a search reranks. Every
//! method, author, benchmark and number is invented; no third-party text is
//! included.
//!
//! The dataset gives each expected source as a file and a passage only; the
//! page is found by laying the paper out, so it always matches the PDF.
//!
//! `shodh-eval synth-papers --out <dir>` writes `<dir>/corpus/**` and
//! `<dir>/dataset.json`; unit tests check that the committed copy in
//! `crates/shodh-eval/data/papers` is byte-identical to this generator's
//! output and that every passage is in one block of one page of its source
//! and every fact in one of its case's sources.

use std::collections::BTreeSet;

use anyhow::{bail, Result};

use crate::dataset::{Dataset, EvalCase, ExpectedSource};
use crate::pdf::{write_pdf, PdfPage, TextRun, PAGE_WIDTH};
use crate::synth::{SynthCorpus, SynthFile};
use crate::text::contains_span;

pub const DATASET_ID: &str = "papers-v1";

const LEFT: f32 = 72.0;
const RIGHT: f32 = PAGE_WIDTH - 72.0;
const TOP: f32 = 720.0;
const BOTTOM: f32 = 72.0;
/// Characters per line at each size, so every line stays inside the margins
/// (Helvetica averages about half an em per character).
const TITLE_CHARS: usize = 56;
const BODY_CHARS: usize = 92;
const SMALL_CHARS: usize = 100;
const EQUATION_X: f32 = 150.0;
const EQUATION_CHARS: usize = 76;
const TABLE_SIZE: f32 = 9.0;
const RUNNING_HEADER: &str = "Preprint. A synthetic paper written for the shodh-eval corpus.";

/// One block of a paper, in reading order.
pub(crate) enum Block {
    Title(String),
    Authors(String),
    Heading(String),
    Paragraph(String),
    /// A display equation with its number, on one line.
    Equation(String),
    Table {
        caption: String,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        /// Characters per column, first column first.
        widths: Vec<usize>,
    },
    Figure(String),
    /// Bibliography entries (the "References" heading is its own block).
    References(Vec<String>),
}

pub(crate) fn h(text: &str) -> Block {
    Block::Heading(text.to_string())
}

pub(crate) fn p(text: &str) -> Block {
    Block::Paragraph(text.to_string())
}

pub(crate) fn eq(text: &str) -> Block {
    Block::Equation(text.to_string())
}

pub(crate) fn fig(text: &str) -> Block {
    Block::Figure(text.to_string())
}

pub(crate) fn table(caption: &str, header: &[&str], rows: &[&[&str]]) -> Block {
    let columns = header.len();
    let first = 30;
    let rest = if columns > 1 { 62 / (columns - 1) } else { 0 };
    let mut widths = vec![first];
    widths.extend(std::iter::repeat_n(rest, columns.saturating_sub(1)));
    Block::Table {
        caption: caption.to_string(),
        header: header.iter().map(|c| c.to_string()).collect(),
        rows: rows
            .iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect(),
        widths,
    }
}

pub(crate) fn refs(entries: &[&str]) -> Block {
    Block::References(entries.iter().map(|e| e.to_string()).collect())
}

/// A paper laid out as PDF pages.
pub(crate) struct PdfPaper {
    pub path: &'static str,
    pub title: &'static str,
    pub blocks: Vec<Block>,
}

/// Greedy word wrap at `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

struct Line {
    x: f32,
    size: f32,
    bold: bool,
    text: String,
    /// Space from this line's baseline to the next one's.
    advance: f32,
    /// Most characters the line may hold at its position and size.
    limit: usize,
}

/// Lines placed together on one page, and the texts a passage is matched in.
struct Unit {
    lines: Vec<Line>,
    texts: Vec<String>,
    gap_after: f32,
    keep_with_next: bool,
}

impl Unit {
    fn height(&self) -> f32 {
        self.lines.iter().map(|l| l.advance).sum()
    }
}

fn text_lines(text: &str, width: usize, size: f32, bold: bool, advance: f32) -> Vec<Line> {
    wrap(text, width)
        .into_iter()
        .map(|text| Line {
            x: LEFT,
            size,
            bold,
            text,
            advance,
            limit: width,
        })
        .collect()
}

fn units(blocks: &[Block]) -> Vec<Unit> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Title(text) => out.push(Unit {
                lines: text_lines(text, TITLE_CHARS, 16.0, true, 22.0),
                texts: vec![text.clone()],
                gap_after: 4.0,
                keep_with_next: true,
            }),
            Block::Authors(text) => out.push(Unit {
                lines: text_lines(text, BODY_CHARS, 10.0, false, 14.0),
                texts: vec![text.clone()],
                gap_after: 16.0,
                keep_with_next: true,
            }),
            Block::Heading(text) => out.push(Unit {
                lines: text_lines(text, BODY_CHARS, 12.0, true, 18.0),
                texts: vec![text.clone()],
                gap_after: 2.0,
                keep_with_next: true,
            }),
            Block::Paragraph(text) => out.push(Unit {
                lines: text_lines(text, BODY_CHARS, 10.0, false, 13.0),
                texts: vec![text.clone()],
                gap_after: 7.0,
                keep_with_next: false,
            }),
            Block::Equation(text) => out.push(Unit {
                lines: vec![Line {
                    x: EQUATION_X,
                    size: 10.0,
                    bold: false,
                    text: text.clone(),
                    advance: 16.0,
                    limit: EQUATION_CHARS,
                }],
                texts: vec![text.clone()],
                gap_after: 6.0,
                keep_with_next: false,
            }),
            Block::Figure(text) => out.push(Unit {
                lines: text_lines(text, SMALL_CHARS, 9.0, false, 11.0),
                texts: vec![text.clone()],
                gap_after: 10.0,
                keep_with_next: false,
            }),
            Block::Table {
                caption,
                header,
                rows,
                widths,
            } => {
                out.push(Unit {
                    lines: text_lines(caption, SMALL_CHARS, 9.0, false, 12.0),
                    texts: vec![caption.clone()],
                    gap_after: 2.0,
                    keep_with_next: true,
                });
                let mut lines = Vec::new();
                let mut texts = Vec::new();
                for (i, row) in std::iter::once(header).chain(rows.iter()).enumerate() {
                    let mut x = LEFT;
                    for (cell, &width) in row.iter().zip(widths.iter()) {
                        lines.push(Line {
                            x,
                            size: TABLE_SIZE,
                            bold: i == 0,
                            text: cell.clone(),
                            advance: 0.0,
                            limit: width.saturating_sub(2),
                        });
                        x += width as f32 * 5.0;
                    }
                    if let Some(last) = lines.last_mut() {
                        last.advance = 13.0;
                    }
                    texts.push(row.join(" "));
                }
                out.push(Unit {
                    lines,
                    texts,
                    gap_after: 10.0,
                    keep_with_next: false,
                });
            }
            Block::References(entries) => {
                for entry in entries {
                    let mut lines = text_lines(entry, SMALL_CHARS - 4, 9.0, false, 11.0);
                    for line in lines.iter_mut().skip(1) {
                        line.x = LEFT + 14.0;
                    }
                    out.push(Unit {
                        lines,
                        texts: vec![entry.clone()],
                        gap_after: 2.0,
                        keep_with_next: false,
                    });
                }
            }
        }
    }
    out
}

/// A laid-out paper: its PDF pages, the text of each page, the blocks of
/// each page (passages are matched within one block), and the lines that do
/// not fit their margins.
pub(crate) struct LaidOut {
    pub pages: Vec<PdfPage>,
    pub page_texts: Vec<String>,
    pub page_blocks: Vec<Vec<String>>,
    pub problems: Vec<String>,
}

pub(crate) fn lay_out(paper: &PdfPaper) -> LaidOut {
    let units = units(&paper.blocks);
    let mut out = LaidOut {
        pages: Vec::new(),
        page_texts: Vec::new(),
        page_blocks: Vec::new(),
        problems: Vec::new(),
    };
    let mut runs: Vec<TextRun> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut blocks: Vec<String> = Vec::new();
    let mut y = TOP;
    let finish = |out: &mut LaidOut,
                  runs: &mut Vec<TextRun>,
                  lines: &mut Vec<String>,
                  blocks: &mut Vec<String>| {
        let number = out.pages.len() + 1;
        runs.push(TextRun {
            x: LEFT,
            y: 760.0,
            size: 8.0,
            bold: false,
            text: RUNNING_HEADER.to_string(),
        });
        runs.push(TextRun {
            x: PAGE_WIDTH / 2.0,
            y: 40.0,
            size: 9.0,
            bold: false,
            text: number.to_string(),
        });
        out.pages.push(PdfPage {
            runs: std::mem::take(runs),
        });
        out.page_texts.push(std::mem::take(lines).join("\n"));
        out.page_blocks.push(std::mem::take(blocks));
    };
    for (i, unit) in units.iter().enumerate() {
        let mut needed = unit.height();
        if unit.keep_with_next {
            // A heading or caption is not left alone at the foot of a page.
            needed += units
                .get(i + 1)
                .map_or(0.0, |next| next.height().min(3.0 * 13.0));
        }
        if y - needed < BOTTOM && !lines.is_empty() {
            finish(&mut out, &mut runs, &mut lines, &mut blocks);
            y = TOP;
        }
        if TOP - unit.height() < BOTTOM {
            out.problems.push(format!(
                "{}: a block is taller than a page: {}",
                paper.path,
                unit.texts.join(" / ")
            ));
        }
        let mut row: Vec<&str> = Vec::new();
        for line in &unit.lines {
            if line.text.chars().count() > line.limit {
                out.problems.push(format!(
                    "{}: line too long for its column: {}",
                    paper.path, line.text
                ));
            }
            if line.x + line.text.chars().count() as f32 * line.size * 0.5 > RIGHT + 1.0 {
                out.problems.push(format!(
                    "{}: line runs past the margin: {}",
                    paper.path, line.text
                ));
            }
            runs.push(TextRun {
                x: line.x,
                y,
                size: line.size,
                bold: line.bold,
                text: line.text.clone(),
            });
            row.push(&line.text);
            if line.advance > 0.0 {
                lines.push(row.join(" "));
                row.clear();
                y -= line.advance;
            }
        }
        blocks.extend(unit.texts.iter().cloned());
        y -= unit.gap_after;
    }
    if !lines.is_empty() {
        finish(&mut out, &mut runs, &mut lines, &mut blocks);
    }
    out
}

/// One file of the corpus with the blocks of each page.
pub struct PaperFile {
    pub file: SynthFile,
    pub page_blocks: Vec<Vec<String>>,
    pub problems: Vec<String>,
}

fn pdf_paper(paper: PdfPaper) -> PaperFile {
    let laid = lay_out(&paper);
    PaperFile {
        file: SynthFile {
            path: paper.path.to_string(),
            bytes: write_pdf(paper.title, &laid.pages),
            pages: laid.page_texts,
            pdf_pages: laid.pages,
        },
        page_blocks: laid.page_blocks,
        problems: laid.problems,
    }
}

/// A text file (LaTeX source): one unpaged "page" whose blocks are its
/// paragraphs (separated by blank lines).
fn text_paper(path: &str, body: &str) -> PaperFile {
    let blocks = body
        .split("\n\n")
        .map(|b| b.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|b| !b.is_empty())
        .collect();
    PaperFile {
        file: SynthFile {
            path: path.to_string(),
            bytes: body.as_bytes().to_vec(),
            pages: vec![body.to_string()],
            pdf_pages: Vec::new(),
        },
        page_blocks: vec![blocks],
        problems: Vec::new(),
    }
}

/// A question, the passages that answer it (file, passage) and the facts a
/// correct answer states.
pub(crate) struct Question {
    pub id: &'static str,
    pub question: &'static str,
    pub sources: &'static [(&'static str, &'static str)],
    pub facts: &'static [&'static str],
}

/// The pages of `file` whose blocks hold `passage` (1-based).
fn pages_with(file: &PaperFile, passage: &str) -> Vec<u32> {
    file.page_blocks
        .iter()
        .enumerate()
        .filter(|(_, blocks)| blocks.iter().any(|b| contains_span(b, passage)))
        .map(|(i, _)| i as u32 + 1)
        .collect()
}

fn case(files: &[PaperFile], q: &Question) -> EvalCase {
    let sources = q
        .sources
        .iter()
        .map(|(path, passage)| {
            let paged = path.ends_with(".pdf");
            let pages = match files.iter().find(|f| f.file.path == *path) {
                Some(file) if paged => pages_with(file, passage),
                _ => Vec::new(),
            };
            ExpectedSource {
                file: path.to_string(),
                pages,
                passage: Some(passage.to_string()),
            }
        })
        .collect();
    EvalCase {
        id: q.id.to_string(),
        question: q.question.to_string(),
        answerable: true,
        sources,
        facts: q.facts.iter().map(|f| f.to_string()).collect(),
        keep: true,
        context: None,
    }
}

fn unanswerable(id: &str, question: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        question: question.to_string(),
        answerable: false,
        sources: Vec::new(),
        facts: Vec::new(),
        keep: true,
        context: None,
    }
}

/// The paper corpus: its files (with page blocks) and dataset.
pub struct PaperCorpus {
    pub files: Vec<PaperFile>,
    pub dataset: Dataset,
}

/// The paper corpus and its dataset.
pub fn build() -> PaperCorpus {
    use crate::papers_text as t;
    let mut files: Vec<PaperFile> = t::pdf_papers()
        .into_iter()
        .chain(crate::papers_gen::pdf_papers())
        .map(pdf_paper)
        .collect();
    files.extend(
        t::text_papers()
            .into_iter()
            .map(|(path, body)| text_paper(path, body)),
    );
    let mut cases: Vec<EvalCase> = t::questions().iter().map(|q| case(&files, q)).collect();
    cases.extend(
        t::unanswerable_questions()
            .iter()
            .map(|(id, question)| unanswerable(id, question)),
    );
    PaperCorpus {
        files,
        dataset: Dataset {
            id: DATASET_ID.to_string(),
            description: "A synthetic library of 21 research papers on linear attention, \
                          long-term memory, KV cache compression, Kolmogorov-Arnold networks, \
                          retrieval-augmented generation and mixture-of-experts, three of them long \
                          (two surveys and a thesis); fictional, generated by `shodh-eval synth-papers`"
                .to_string(),
            cases,
        },
    }
}

impl PaperCorpus {
    /// As a [`SynthCorpus`], for writing.
    pub fn to_synth(&self) -> SynthCorpus {
        SynthCorpus {
            files: self.files.iter().map(|f| f.file.clone()).collect(),
            dataset: self.dataset.clone(),
        }
    }

    /// Check the layout and the dataset against the generated text: every
    /// line inside its margins, ASCII only, every passage in one block of
    /// exactly one page of its file, every fact in one of its case's
    /// sources, ids unique, and each question's sources distinct files.
    pub fn check(&self) -> Result<()> {
        self.dataset.validate()?;
        let mut problems: Vec<String> = Vec::new();
        for file in &self.files {
            problems.extend(file.problems.iter().cloned());
            if !file.file.pages.iter().all(|p| p.is_ascii()) {
                problems.push(format!("{} is not ASCII", file.file.path));
            }
        }
        for case in self.dataset.active_cases() {
            let mut texts: Vec<&str> = Vec::new();
            let mut seen = BTreeSet::new();
            for src in &case.sources {
                if !seen.insert(src.file.as_str()) {
                    problems.push(format!("{}: {} is listed twice", case.id, src.file));
                }
                let Some(file) = self.files.iter().find(|f| f.file.path == src.file) else {
                    problems.push(format!("{}: no file {}", case.id, src.file));
                    continue;
                };
                let Some(passage) = &src.passage else {
                    problems.push(format!("{}: {} has no passage", case.id, src.file));
                    continue;
                };
                let found = pages_with(file, passage);
                if found.len() != 1 {
                    problems.push(format!(
                        "{}: passage on {} pages of {} (want 1): {passage}",
                        case.id,
                        found.len(),
                        src.file
                    ));
                }
                texts.extend(file.page_blocks.iter().flatten().map(String::as_str));
            }
            for fact in &case.facts {
                if !texts.iter().any(|t| contains_span(t, fact)) {
                    problems.push(format!("{}: fact not in its sources: {fact}", case.id));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            bail!("{}", problems.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn committed() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("papers")
    }

    #[test]
    fn dataset_matches_the_generated_text() {
        build().check().unwrap();
    }

    #[test]
    fn the_dataset_has_the_question_kinds_it_measures() {
        let corpus = build();
        let answerable: Vec<&EvalCase> = corpus
            .dataset
            .active_cases()
            .filter(|c| c.answerable)
            .collect();
        assert!(
            (60..=90).contains(&answerable.len()),
            "{}",
            answerable.len()
        );
        let multi = answerable.iter().filter(|c| c.sources.len() > 1).count();
        assert!(multi >= 12, "{multi} questions span several papers");
        assert!(answerable.iter().all(|c| c.sources.len() <= 4));
        let which = answerable
            .iter()
            .filter(|c| c.question.to_lowercase().starts_with("which paper"))
            .count();
        assert!(which >= 6, "{which} 'which paper' questions");
        assert!(corpus.files.len() >= 15, "{} papers", corpus.files.len());
        // The two surveys and the thesis are each at least twice as long as
        // the median paper, and share the other papers' vocabulary.
        let words = |file: &PaperFile| {
            file.page_blocks
                .iter()
                .flatten()
                .map(|b| b.split_whitespace().count())
                .sum::<usize>()
        };
        let long = [
            crate::papers_text::SURVEY,
            crate::papers_gen::KV_SURVEY,
            crate::papers_gen::THESIS,
        ];
        let mut others: Vec<usize> = corpus
            .files
            .iter()
            .filter(|f| !long.contains(&f.file.path.as_str()))
            .map(words)
            .collect();
        others.sort_unstable();
        let median = others[others.len() / 2];
        for path in long {
            let file = corpus.files.iter().find(|f| f.file.path == path).unwrap();
            assert!(
                words(file) >= 2 * median,
                "{path}: {} words, median paper {median}",
                words(file)
            );
        }
    }

    #[test]
    fn lines_wrap_at_word_boundaries() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("  ", 5), Vec::<String>::new());
        assert_eq!(wrap("toolongword x", 4), ["toolongword", "x"]);
    }

    #[test]
    fn generation_is_deterministic() {
        let (a, b) = (build(), build());
        assert_eq!(a.files.len(), b.files.len());
        for (x, y) in a.files.iter().zip(&b.files) {
            assert_eq!(x.file.bytes, y.file.bytes, "{}", x.file.path);
        }
        assert_eq!(a.dataset, b.dataset);
    }

    #[test]
    fn committed_corpus_is_the_generator_output() {
        let corpus = build();
        let root = committed();
        for file in &corpus.files {
            let path = file
                .file
                .path
                .split('/')
                .fold(root.join("corpus"), |p, s| p.join(s));
            let on_disk =
                std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(
                on_disk == file.file.bytes,
                "{} differs from the generator; run `shodh-eval synth-papers --out crates/shodh-eval/data/papers`",
                file.file.path
            );
        }
        let listed = crate::corpus::supported_files(&root.join("corpus")).unwrap();
        let mut produced: Vec<String> = corpus.files.iter().map(|f| f.file.path.clone()).collect();
        produced.sort();
        assert_eq!(
            listed, produced,
            "the committed corpus has extra or missing files"
        );
        let dataset = std::fs::read_to_string(root.join("dataset.json")).unwrap();
        assert_eq!(dataset, corpus.dataset.to_json().unwrap());
    }
}
