//! Structure-aware chunking: index semantic units instead of token windows.
//!
//! Rules (each enforced by a test below):
//! - A chunk never crosses a heading: units are packed only within one section.
//! - Paragraphs and list items of a section are packed together up to the
//!   token budget; a paragraph longer than the budget is split at sentence
//!   boundaries.
//! - A table is emitted as its header plus groups of whole rows; the header
//!   (and caption) is repeated in every chunk of the table.
//! - A theorem, lemma or definition is kept with the display equations that
//!   follow it and the start of its proof.
//! - A display equation stays with the sentence that introduces it and, when
//!   the next paragraph continues the sentence ("where ..."), with that too.
//! - A figure is its caption, alone.
//! - Every bibliography entry is its own chunk (for the citation graph).
//! - Code blocks are kept whole when they fit, else split at line breaks.
//! - Form fields of one section and page are packed together, one `label: value`
//!   per line, so a label is never separated from its value.
//! - The raw text of a table region whose cells missed part of it is its own chunk,
//!   flagged (with the table's chunks) as possibly incomplete.
//!
//! Token counts come from the embedding model's tokenizer, so the budget is
//! exact: the context prefix plus the chunk text never exceeds what the
//! embedder reads (512 tokens for E5).

use uuid::Uuid;

use super::chunker::ContextualChunkResult;
use super::document_model::{render_table, BBox, Block, BlockKind, StructuredDocument};

/// Tokens the embedder reads per passage, minus special tokens and the
/// `passage: ` instruction prefix.
pub const EMBEDDER_TOKEN_LIMIT: usize = 500;

/// Default token budget for a chunk's text (prefix excluded).
pub const DEFAULT_CHUNK_TOKENS: usize = 380;

/// Version tag stored with every chunk this chunker produces.
pub const STRUCTURE_CHUNKER_VERSION: &str = "structure-v1";

/// Most regions kept per chunk; further boxes on a page are merged.
const MAX_REGIONS: usize = 16;

/// Where a chunk came from: pages, boxes, section, block kinds.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkLayout {
    pub page_start: Option<u32>,
    pub page_end: Option<u32>,
    pub regions: Vec<ChunkRegion>,
    pub section_path: Vec<String>,
    /// Kinds of the blocks in the chunk, in first-appearance order.
    pub block_kinds: Vec<&'static str>,
    /// The kind of semantic unit the chunk is (see [`UnitKind::name`]).
    pub unit: &'static str,
    /// The chunk holds a table whose cells missed part of its region's text, or that
    /// region's raw text ([`BlockKind::TableText`]).
    pub incomplete: bool,
}

/// One bounding box on one page.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChunkRegion {
    pub page: u32,
    #[serde(flatten)]
    pub bbox: BBox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitKind {
    /// Paragraphs, list items, footnotes, proofs: packable prose.
    Flow,
    /// Theorem-like statement or definition with its equations and proof start.
    Statement,
    Table,
    Figure,
    Reference,
    Code,
    /// Consecutive form fields of one section and page.
    Form,
    /// Raw text of a table region the cells did not fully capture.
    TableText,
}

impl UnitKind {
    fn name(self) -> &'static str {
        match self {
            UnitKind::Flow => "text",
            UnitKind::Statement => "statement",
            UnitKind::Table => "table",
            UnitKind::Figure => "figure",
            UnitKind::Reference => "reference_entry",
            UnitKind::Code => "code",
            UnitKind::Form => "form",
            UnitKind::TableText => "table_text",
        }
    }
}

#[derive(Debug, Clone)]
struct Unit {
    kind: UnitKind,
    blocks: Vec<usize>,
    section_path: Vec<String>,
    /// Heading text to show at the start of the unit (first unit of a section).
    heading: Option<String>,
    /// Block index from which text may be dropped to fit (proof start).
    trim_from: Option<usize>,
}

/// Packs a [`StructuredDocument`] into chunks of semantic units.
pub struct StructureChunker {
    max_tokens: usize,
}

impl Default for StructureChunker {
    fn default() -> Self {
        Self::new(DEFAULT_CHUNK_TOKENS)
    }
}

impl StructureChunker {
    /// `max_tokens` is the budget for a chunk's text; it is clamped so that
    /// text plus context prefix stays within [`EMBEDDER_TOKEN_LIMIT`].
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens: max_tokens.clamp(64, EMBEDDER_TOKEN_LIMIT - 60),
        }
    }

    /// Chunk `doc`. `count_tokens` must count tokens the way the embedder
    /// tokenizes (see `EmbeddingModel::count_tokens`).
    pub fn chunk(
        &self,
        doc: &StructuredDocument,
        doc_title: &str,
        count_tokens: &dyn Fn(&str) -> usize,
    ) -> Vec<ContextualChunkResult> {
        let units = build_units(&doc.blocks);
        let mut out: Vec<ContextualChunkResult> = Vec::new();
        let mut pack: Vec<(Unit, String, usize)> = Vec::new();

        let mut i = 0;
        while i < units.len() {
            let unit = &units[i];
            let prefix = context_prefix(doc_title, &unit.section_path, unit.kind);
            let budget = self
                .max_tokens
                .min(EMBEDDER_TOKEN_LIMIT.saturating_sub(count_tokens(&prefix)))
                .max(32);
            match unit.kind {
                UnitKind::Flow => {
                    let text = unit_text(unit, &doc.blocks);
                    let tokens = count_tokens(&text);
                    let same_section = pack
                        .last()
                        .is_none_or(|(u, _, _)| u.section_path == unit.section_path);
                    let packed_tokens: usize = pack.iter().map(|(_, _, t)| *t).sum();
                    if !same_section || packed_tokens + tokens > budget {
                        self.flush_pack(&mut pack, doc, doc_title, count_tokens, &mut out);
                    }
                    if tokens > budget {
                        for piece in split_text(&text, budget, count_tokens) {
                            let piece_tokens = count_tokens(&piece);
                            pack.push((unit.clone(), piece, piece_tokens));
                            self.flush_pack(&mut pack, doc, doc_title, count_tokens, &mut out);
                        }
                    } else {
                        pack.push((unit.clone(), text, tokens));
                    }
                }
                UnitKind::Table => {
                    self.flush_pack(&mut pack, doc, doc_title, count_tokens, &mut out);
                    for text in table_pieces(unit, &doc.blocks, budget, count_tokens) {
                        push_chunk(&mut out, doc_title, &[unit], text, &doc.blocks);
                    }
                }
                UnitKind::Statement => {
                    self.flush_pack(&mut pack, doc, doc_title, count_tokens, &mut out);
                    for text in statement_pieces(unit, &doc.blocks, budget, count_tokens) {
                        push_chunk(&mut out, doc_title, &[unit], text, &doc.blocks);
                    }
                }
                UnitKind::Form => {
                    self.flush_pack(&mut pack, doc, doc_title, count_tokens, &mut out);
                    for (text, blocks) in form_pieces(unit, &doc.blocks, budget, count_tokens) {
                        let piece = Unit {
                            blocks,
                            ..unit.clone()
                        };
                        push_chunk(&mut out, doc_title, &[&piece], text, &doc.blocks);
                    }
                }
                UnitKind::Figure | UnitKind::Reference | UnitKind::Code | UnitKind::TableText => {
                    self.flush_pack(&mut pack, doc, doc_title, count_tokens, &mut out);
                    let text = unit_text(unit, &doc.blocks);
                    let pieces = if count_tokens(&text) > budget {
                        if matches!(unit.kind, UnitKind::Code | UnitKind::TableText) {
                            split_lines(&text, budget, count_tokens)
                        } else {
                            split_text(&text, budget, count_tokens)
                        }
                    } else {
                        vec![text]
                    };
                    for text in pieces {
                        push_chunk(&mut out, doc_title, &[unit], text, &doc.blocks);
                    }
                }
            }
            i += 1;
        }
        self.flush_pack(&mut pack, doc, doc_title, count_tokens, &mut out);

        for (index, chunk) in out.iter_mut().enumerate() {
            chunk.index = index;
        }
        out
    }

    fn flush_pack(
        &self,
        pack: &mut Vec<(Unit, String, usize)>,
        doc: &StructuredDocument,
        doc_title: &str,
        count_tokens: &dyn Fn(&str) -> usize,
        out: &mut Vec<ContextualChunkResult>,
    ) {
        if pack.is_empty() {
            return;
        }
        let units: Vec<&Unit> = pack.iter().map(|(u, _, _)| u).collect();
        let text = pack
            .iter()
            .map(|(_, t, _)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        // Sums of per-unit counts can undercount at joins; verify once.
        let prefix = context_prefix(doc_title, &units[0].section_path, UnitKind::Flow);
        if count_tokens(&format!("{prefix}{text}")) > EMBEDDER_TOKEN_LIMIT && pack.len() > 1 {
            let budget = EMBEDDER_TOKEN_LIMIT.saturating_sub(count_tokens(&prefix));
            let pieces = split_text(&text, budget, count_tokens);
            for piece in pieces {
                push_chunk(out, doc_title, &units, piece, &doc.blocks);
            }
        } else {
            push_chunk(out, doc_title, &units, text, &doc.blocks);
        }
        pack.clear();
    }
}

/// Group blocks into semantic units (see the module documentation).
fn build_units(blocks: &[Block]) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    let mut pending_heading: Option<String> = None;
    // An equation just closed a unit that the next continuation joins.
    let mut glue_next = false;
    // The last unit is a statement still absorbing equations/proof start.
    let mut absorbing = false;

    let new_unit =
        |kind: UnitKind, index: usize, block: &Block, heading: &mut Option<String>| Unit {
            kind,
            blocks: vec![index],
            section_path: block.section_path.clone(),
            heading: heading.take(),
            trim_from: None,
        };

    for (index, block) in blocks.iter().enumerate() {
        let same_section = units
            .last()
            .is_some_and(|u| u.section_path == block.section_path);
        match &block.kind {
            BlockKind::Title => {}
            BlockKind::Heading { .. } => {
                pending_heading = Some(block.text.trim().to_string()).filter(|t| !t.is_empty());
                glue_next = false;
                absorbing = false;
            }
            BlockKind::Equation => {
                let attach = same_section
                    && units
                        .last()
                        .is_some_and(|u| matches!(u.kind, UnitKind::Flow | UnitKind::Statement));
                if attach {
                    if let Some(last) = units.last_mut() {
                        last.blocks.push(index);
                    }
                } else {
                    units.push(new_unit(UnitKind::Flow, index, block, &mut pending_heading));
                }
                glue_next = true;
            }
            BlockKind::Theorem { .. } | BlockKind::Definition { .. } => {
                units.push(new_unit(
                    UnitKind::Statement,
                    index,
                    block,
                    &mut pending_heading,
                ));
                absorbing = true;
                glue_next = false;
            }
            BlockKind::Proof => {
                let into_statement = absorbing
                    && same_section
                    && units
                        .last()
                        .is_some_and(|u| u.kind == UnitKind::Statement && u.trim_from.is_none());
                if into_statement {
                    if let Some(last) = units.last_mut() {
                        last.trim_from = Some(last.blocks.len());
                        last.blocks.push(index);
                    }
                } else {
                    units.push(new_unit(UnitKind::Flow, index, block, &mut pending_heading));
                }
                absorbing = false;
                glue_next = false;
            }
            BlockKind::Paragraph | BlockKind::ListItem | BlockKind::Footnote => {
                let continues = glue_next
                    && same_section
                    && starts_as_continuation(&block.text)
                    && units
                        .last()
                        .is_some_and(|u| u.kind == UnitKind::Flow || u.kind == UnitKind::Statement);
                if continues {
                    if let Some(last) = units.last_mut() {
                        last.blocks.push(index);
                    }
                } else {
                    units.push(new_unit(UnitKind::Flow, index, block, &mut pending_heading));
                }
                absorbing = false;
                glue_next = false;
            }
            BlockKind::Table { .. } => {
                units.push(new_unit(
                    UnitKind::Table,
                    index,
                    block,
                    &mut pending_heading,
                ));
                absorbing = false;
                glue_next = false;
            }
            BlockKind::FormField { .. } => {
                let continues = same_section
                    && units.last().is_some_and(|u| {
                        u.kind == UnitKind::Form
                            && u.blocks
                                .last()
                                .is_some_and(|&b| blocks[b].page == block.page)
                    });
                match units.last_mut() {
                    Some(last) if continues => last.blocks.push(index),
                    _ => units.push(new_unit(UnitKind::Form, index, block, &mut pending_heading)),
                }
                absorbing = false;
                glue_next = false;
            }
            BlockKind::TableText => {
                units.push(new_unit(
                    UnitKind::TableText,
                    index,
                    block,
                    &mut pending_heading,
                ));
                absorbing = false;
                glue_next = false;
            }
            BlockKind::Figure { .. } => {
                units.push(new_unit(
                    UnitKind::Figure,
                    index,
                    block,
                    &mut pending_heading,
                ));
                glue_next = false;
            }
            BlockKind::ReferenceEntry => {
                units.push(new_unit(
                    UnitKind::Reference,
                    index,
                    block,
                    &mut pending_heading,
                ));
                absorbing = false;
                glue_next = false;
            }
            BlockKind::Code => {
                units.push(new_unit(UnitKind::Code, index, block, &mut pending_heading));
                absorbing = false;
                glue_next = false;
            }
        }
    }
    units.retain(|u| !unit_is_empty(u, blocks));
    units
}

fn unit_is_empty(unit: &Unit, blocks: &[Block]) -> bool {
    unit.blocks
        .iter()
        .all(|&i| blocks[i].render().split_whitespace().count() < 2)
}

/// Text that continues the sentence of a preceding display equation.
fn starts_as_continuation(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with(|c: char| c.is_lowercase())
        || ["where ", "Where ", "with ", "With ", "Here ", "here "]
            .iter()
            .any(|p| t.starts_with(p))
}

fn unit_text(unit: &Unit, blocks: &[Block]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(h) = &unit.heading {
        parts.push(h.clone());
    }
    for &i in &unit.blocks {
        let text = blocks[i].render();
        let text = text.trim();
        if !text.is_empty() {
            parts.push(text.to_string());
        }
    }
    parts.join("\n\n")
}

fn context_prefix(title: &str, section_path: &[String], kind: UnitKind) -> String {
    let mut prefix = format!("Document: \"{}\".", title.trim());
    if !section_path.is_empty() {
        prefix.push_str(" Section: ");
        prefix.push_str(&section_path.join(" > "));
        prefix.push('.');
    }
    match kind {
        UnitKind::Table => prefix.push_str(" Table."),
        UnitKind::Figure => prefix.push_str(" Figure."),
        UnitKind::Reference => prefix.push_str(" Bibliography entry."),
        UnitKind::Code => prefix.push_str(" Code."),
        UnitKind::Form => prefix.push_str(" Form fields."),
        UnitKind::TableText => prefix.push_str(" Table text as printed."),
        UnitKind::Statement | UnitKind::Flow => {}
    }
    prefix.push(' ');
    prefix
}

/// Table chunks: caption and header repeated, followed by as many whole rows
/// as fit. A single row that alone exceeds the budget is emitted on its own.
fn table_pieces(
    unit: &Unit,
    blocks: &[Block],
    budget: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> Vec<String> {
    let Some(&index) = unit.blocks.first() else {
        return Vec::new();
    };
    let BlockKind::Table {
        header,
        rows,
        caption,
        ..
    } = &blocks[index].kind
    else {
        return vec![blocks[index].render()];
    };
    let caption = match (&unit.heading, caption) {
        (Some(h), Some(c)) => Some(format!("{h}\n{c}")),
        (Some(h), None) => Some(h.clone()),
        (None, c) => c.clone(),
    };
    let base_tokens = count_tokens(&render_table(caption.as_deref(), header, &[]));
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < rows.len() || (rows.is_empty() && pieces.is_empty()) {
        let mut end = start;
        let mut used = base_tokens;
        while end < rows.len() {
            let row_tokens = count_tokens(&render_table(None, &[], &rows[end..end + 1]));
            if used + row_tokens > budget && end > start {
                break;
            }
            used += row_tokens;
            end += 1;
        }
        pieces.push(render_table(caption.as_deref(), header, &rows[start..end]));
        if end == start {
            break;
        }
        start = end;
    }
    pieces
}

/// Form chunks: whole fields (`label: value` lines) packed up to the budget, with
/// the section heading in front of the first. Returns each piece's text and blocks.
fn form_pieces(
    unit: &Unit,
    blocks: &[Block],
    budget: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> Vec<(String, Vec<usize>)> {
    let overhead = count_tokens("");
    let cost = |s: &str| count_tokens(s).saturating_sub(overhead) + 1;
    let room = budget.saturating_sub(overhead).max(1);
    let mut pieces: Vec<(String, Vec<usize>)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut members: Vec<usize> = Vec::new();
    let mut used = 0usize;
    if let Some(h) = &unit.heading {
        used += cost(h);
        lines.push(h.clone());
    }
    for &i in &unit.blocks {
        let text = blocks[i].render();
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let tokens = cost(text);
        if used + tokens > room && !members.is_empty() {
            pieces.push((lines.join("\n"), std::mem::take(&mut members)));
            lines.clear();
            used = 0;
        }
        lines.push(text.to_string());
        members.push(i);
        used += tokens;
    }
    if !members.is_empty() {
        pieces.push((lines.join("\n"), members));
    }
    pieces
}

/// A statement with its equations and proof start. When it does not fit, the
/// proof start is shortened first; a statement that alone exceeds the budget
/// is split at sentence boundaries.
fn statement_pieces(
    unit: &Unit,
    blocks: &[Block],
    budget: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> Vec<String> {
    let full = unit_text(unit, blocks);
    if count_tokens(&full) <= budget {
        return vec![full];
    }
    let Some(trim_from) = unit.trim_from else {
        return split_text(&full, budget, count_tokens);
    };
    let head_unit = Unit {
        blocks: unit.blocks[..trim_from].to_vec(),
        ..unit.clone()
    };
    let head = unit_text(&head_unit, blocks);
    let head_tokens = count_tokens(&head);
    if head_tokens >= budget {
        return split_text(&head, budget, count_tokens);
    }
    let proof: String = unit.blocks[trim_from..]
        .iter()
        .map(|&i| blocks[i].render())
        .collect::<Vec<_>>()
        .join("\n\n");
    let remaining = budget - head_tokens;
    let proof_start = split_text(&proof, remaining.max(16), count_tokens)
        .into_iter()
        .next()
        .unwrap_or_default();
    if proof_start.is_empty() || count_tokens(&proof_start) > remaining {
        return vec![head];
    }
    vec![format!("{head}\n\n{proof_start}")]
}

/// Split prose at sentence boundaries into pieces of at most `budget`
/// tokens. A sentence longer than the budget is split at word boundaries.
///
/// Each sentence (and, for over-long sentences, each word) is counted once
/// and pieces are sized by summing: tokenizing every growing candidate
/// would be quadratic in the paragraph length.
pub fn split_text(text: &str, budget: usize, count_tokens: &dyn Fn(&str) -> usize) -> Vec<String> {
    let overhead = count_tokens("");
    let cost = |s: &str| count_tokens(s).saturating_sub(overhead);
    let room = budget.saturating_sub(overhead).max(1);
    let mut pieces = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for sentence in split_sentences(text) {
        let tokens = cost(sentence);
        if tokens > room {
            if !current.is_empty() {
                pieces.push(current.join(" "));
                current.clear();
                used = 0;
            }
            pieces.extend(pack_by_cost(sentence.split_whitespace(), " ", room, &cost));
            continue;
        }
        if used + tokens > room && !current.is_empty() {
            pieces.push(current.join(" "));
            current.clear();
            used = 0;
        }
        current.push(sentence);
        used += tokens;
    }
    if !current.is_empty() {
        pieces.push(current.join(" "));
    }
    pieces.retain(|p| !p.trim().is_empty());
    pieces
}

/// Greedily pack `parts` (joined by `separator`) into pieces whose summed
/// cost stays within `room`; a single part over `room` becomes its own piece.
fn pack_by_cost<'a>(
    parts: impl Iterator<Item = &'a str>,
    separator: &str,
    room: usize,
    cost: &dyn Fn(&str) -> usize,
) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for part in parts {
        let tokens = cost(part);
        if used + tokens > room && !current.is_empty() {
            pieces.push(current.join(separator));
            current.clear();
            used = 0;
        }
        current.push(part);
        used += tokens;
    }
    if !current.is_empty() {
        pieces.push(current.join(separator));
    }
    pieces
}

/// Split code at line boundaries into pieces of at most `budget` tokens.
fn split_lines(text: &str, budget: usize, count_tokens: &dyn Fn(&str) -> usize) -> Vec<String> {
    let overhead = count_tokens("");
    let cost = |s: &str| count_tokens(s).saturating_sub(overhead) + 1;
    let room = budget.saturating_sub(overhead).max(1);
    let mut pieces = pack_by_cost(text.lines(), "\n", room, &cost);
    pieces.retain(|p| !p.trim().is_empty());
    pieces
}

const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "et al.", "al.", "Fig.", "Figs.", "Eq.", "Eqs.", "Sec.", "Ref.", "vs.", "cf.",
    "resp.", "approx.", "Thm.", "Def.", "Prop.", "No.", "Dr.", "Mr.", "Ms.", "Prof.", "Tab.",
];

/// Sentence spans: a boundary is `.`, `!` or `?` followed by whitespace and an
/// uppercase letter, digit or opening bracket, unless the word before is a
/// common abbreviation.
fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for k in 0..chars.len() {
        let (pos, c) = chars[k];
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let Some(&(_, next)) = chars.get(k + 1) else {
            continue;
        };
        if !next.is_whitespace() {
            continue;
        }
        let after = chars[k + 1..]
            .iter()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|&(_, ch)| ch);
        if !after
            .is_some_and(|ch| ch.is_uppercase() || ch.is_ascii_digit() || ch == '(' || ch == '[')
        {
            continue;
        }
        let end = pos + c.len_utf8();
        let word_start = text[start..end]
            .rfind(char::is_whitespace)
            .map(|w| start + w + 1)
            .unwrap_or(start);
        let word = &text[word_start..end];
        if ABBREVIATIONS.contains(&word)
            || (word.len() == 2 && word.starts_with(|ch: char| ch.is_uppercase()))
        {
            continue;
        }
        let sentence = text[start..end].trim();
        if !sentence.is_empty() {
            out.push(sentence);
        }
        start = end;
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn push_chunk(
    out: &mut Vec<ContextualChunkResult>,
    doc_title: &str,
    units: &[&Unit],
    text: String,
    blocks: &[Block],
) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    let first = units[0];
    let block_indices: Vec<usize> = units
        .iter()
        .flat_map(|u| u.blocks.iter().copied())
        .collect();
    let layout = layout_of(first, &block_indices, blocks);
    let kind = if units.iter().all(|u| u.kind == first.kind) {
        first.kind
    } else {
        UnitKind::Flow
    };
    let prefix = context_prefix(doc_title, &first.section_path, kind);
    out.push(ContextualChunkResult {
        id: Uuid::new_v4(),
        contextualized_text: format!("{prefix}{text}"),
        index: 0,
        heading: first.section_path.last().cloned(),
        start_offset: 0,
        end_offset: text.len(),
        page: layout.page_start.map(|p| p as usize),
        text,
        layout: Some(ChunkLayout {
            unit: kind.name(),
            ..layout
        }),
    });
}

fn layout_of(first: &Unit, block_indices: &[usize], blocks: &[Block]) -> ChunkLayout {
    let pages: Vec<u32> = block_indices
        .iter()
        .filter_map(|&i| blocks[i].page)
        .collect();
    let mut regions: Vec<ChunkRegion> = Vec::new();
    for &i in block_indices {
        if let (Some(page), Some(bbox)) = (blocks[i].page, blocks[i].bbox) {
            regions.push(ChunkRegion { page, bbox });
        }
    }
    if regions.len() > MAX_REGIONS {
        // Keep one box per page: the union of that page's boxes.
        let mut merged: Vec<ChunkRegion> = Vec::new();
        for region in regions {
            match merged.iter_mut().find(|r| r.page == region.page) {
                Some(existing) => existing.bbox = existing.bbox.union(&region.bbox),
                None => merged.push(region),
            }
        }
        regions = merged;
    }
    let mut kinds: Vec<&'static str> = Vec::new();
    for &i in block_indices {
        let name = blocks[i].kind.name();
        if !kinds.contains(&name) {
            kinds.push(name);
        }
    }
    let incomplete = block_indices
        .iter()
        .any(|&i| blocks[i].table_incomplete() || matches!(blocks[i].kind, BlockKind::TableText));
    ChunkLayout {
        page_start: pages.iter().copied().min(),
        page_end: pages.iter().copied().max(),
        regions,
        section_path: first.section_path.clone(),
        block_kinds: kinds,
        unit: first.kind.name(),
        incomplete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whitespace token count: a stand-in tokenizer for the rules.
    fn words(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn block(kind: BlockKind, text: &str, page: u32) -> Block {
        Block::new(kind, text).on_page(page, Some(BBox::new(72.0, 100.0, 300.0, 200.0)))
    }

    fn doc(blocks: Vec<Block>) -> StructuredDocument {
        let mut d = StructuredDocument {
            pages: Vec::new(),
            blocks,
        };
        d.finalize();
        d
    }

    /// A capitalised sentence of `n` words ending in `tag.`.
    fn sentence(n: usize, tag: &str) -> String {
        let body: Vec<&str> = std::iter::once("Word")
            .chain(std::iter::repeat("word").take(n.saturating_sub(2)))
            .collect();
        format!("{} {tag}.", body.join(" "))
    }

    #[test]
    fn never_crosses_a_heading() {
        let d = doc(vec![
            block(BlockKind::Heading { level: 1 }, "1 Intro", 1),
            block(BlockKind::Paragraph, "Alpha paragraph text here.", 1),
            block(BlockKind::Heading { level: 1 }, "2 Method", 2),
            block(BlockKind::Paragraph, "Beta paragraph text here.", 2),
        ]);
        let chunks = StructureChunker::new(300).chunk(&d, "Doc", &words);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].text.contains("Alpha") && !chunks[0].text.contains("Beta"));
        assert!(chunks[0].text.starts_with("1 Intro"));
        assert!(chunks[1].text.starts_with("2 Method"));
        let layout = chunks[1].layout.as_ref().unwrap();
        assert_eq!(layout.section_path, vec!["2 Method"]);
        assert_eq!(layout.page_start, Some(2));
        assert!(chunks[1].contextualized_text.contains("Section: 2 Method."));
    }

    #[test]
    fn packs_paragraphs_within_budget_and_splits_long_ones() {
        let d = doc(vec![
            block(BlockKind::Heading { level: 1 }, "Intro", 1),
            block(BlockKind::Paragraph, &sentence(40, "a"), 1),
            block(BlockKind::Paragraph, &sentence(40, "b"), 2),
            block(BlockKind::Paragraph, &sentence(40, "c"), 2),
            block(
                BlockKind::Paragraph,
                &format!(
                    "{} {} {}",
                    sentence(60, "d"),
                    sentence(60, "Ee"),
                    sentence(60, "Ff")
                ),
                3,
            ),
        ]);
        let chunker = StructureChunker::new(100);
        let chunks = chunker.chunk(&d, "Doc", &words);
        for c in &chunks {
            assert!(
                words(&c.text) <= 100,
                "chunk over budget: {}",
                words(&c.text)
            );
            assert!(words(&c.contextualized_text) <= EMBEDDER_TOKEN_LIMIT);
        }
        // a+b packed (2 x 40 + heading), c alone, d/e/f split at sentences.
        assert!(chunks[0].text.contains("a.") && chunks[0].text.contains("b."));
        let l0 = chunks[0].layout.as_ref().unwrap();
        assert_eq!((l0.page_start, l0.page_end), (Some(1), Some(2)));
        assert_eq!(l0.regions.len(), 2);
        assert!(chunks.iter().any(|c| c.text.ends_with("Ee.")));
    }

    #[test]
    fn table_header_repeats_in_every_chunk() {
        let rows: Vec<Vec<String>> = (0..30)
            .map(|i| vec![format!("model{i}"), format!("{i}.0"), "x y z w".to_string()])
            .collect();
        let d = doc(vec![block(
            BlockKind::Table {
                header: vec!["Model".into(), "PPL".into(), "Notes".into()],
                rows,
                caption: Some("Table 2: Results.".into()),
                cell_boxes: Vec::new(),
                cell_coverage: None,
            },
            "",
            4,
        )]);
        let chunks = StructureChunker::new(80).chunk(&d, "Doc", &words);
        assert!(chunks.len() > 1);
        let mut seen = 0;
        for c in &chunks {
            assert!(c
                .text
                .starts_with("Table 2: Results.\n| Model | PPL | Notes |"));
            assert_eq!(c.layout.as_ref().unwrap().unit, "table");
            seen += c.text.matches("| model").count();
        }
        assert_eq!(seen, 30, "every row appears exactly once");
    }

    #[test]
    fn theorem_keeps_equations_and_proof_start() {
        let d = doc(vec![
            block(
                BlockKind::Paragraph,
                "Theorem 2 (Bound). Let x be positive. Then",
                5,
            ),
            block(BlockKind::Equation, "f(x) ≤ g(x) (4)", 5),
            block(
                BlockKind::Paragraph,
                "Proof. Apply Lemma 1 twice. The rest follows.",
                5,
            ),
            block(
                BlockKind::Paragraph,
                "Unrelated discussion follows here.",
                6,
            ),
        ]);
        let chunks = StructureChunker::new(200).chunk(&d, "Doc", &words);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].text.contains("Theorem 2"));
        assert!(chunks[0].text.contains("f(x) ≤ g(x)"));
        assert!(chunks[0].text.contains("Proof. Apply Lemma 1"));
        let l = chunks[0].layout.as_ref().unwrap();
        assert_eq!(l.unit, "statement");
        assert_eq!(l.block_kinds, vec!["theorem", "equation", "proof"]);
        assert!(!chunks[1].text.contains("Theorem"));
    }

    #[test]
    fn long_proof_is_shortened_not_the_statement() {
        let proof = format!(
            "Proof. {}",
            (0..20)
                .map(|i| sentence(10, &format!("P{i}")))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let d = doc(vec![
            block(
                BlockKind::Paragraph,
                "Lemma 3. Every bounded sequence has a convergent subsequence.",
                1,
            ),
            block(BlockKind::Paragraph, &proof, 1),
        ]);
        let chunks = StructureChunker::new(64).chunk(&d, "Doc", &words);
        assert!(chunks[0].text.starts_with("Lemma 3."));
        assert!(chunks[0].text.contains("Proof."));
        assert!(words(&chunks[0].text) <= 64);
    }

    #[test]
    fn equation_stays_with_its_introduction_and_where_clause() {
        let d = doc(vec![
            block(BlockKind::Paragraph, "The state is updated as", 2),
            block(BlockKind::Equation, "S_t = S_{t-1} + v k (1)", 2),
            block(BlockKind::Paragraph, "where k is the key vector.", 2),
            block(BlockKind::Paragraph, "Figure 1: Overview of the model.", 2),
        ]);
        let chunks = StructureChunker::new(30).chunk(&d, "Doc", &words);
        assert!(
            chunks[0].text.contains("updated as")
                && chunks[0].text.contains("S_t")
                && chunks[0].text.contains("where k")
        );
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].layout.as_ref().unwrap().unit, "figure");
    }

    #[test]
    fn form_fields_pack_by_section_and_page_and_keep_label_with_value() {
        let field = |label: &str, value: &str, page: u32, y: f32| {
            Block::form_field(
                label,
                value,
                crate::processing::pdf_forms::FieldKind::Layout,
            )
            .on_page(page, Some(BBox::new(40.0, y, 560.0, y + 9.0)))
        };
        let d = doc(vec![
            block(BlockKind::Heading { level: 1 }, "Income details", 1),
            field("Gross salary", "9,10,000", 1, 700.0),
            field("Standard deduction", "75,000", 1, 680.0),
            field("Net salary", "8,35,000", 2, 700.0),
            block(BlockKind::Heading { level: 1 }, "Taxes paid", 2),
            field("TDS", "12,000", 2, 600.0),
        ]);
        let chunks = StructureChunker::new(300).chunk(&d, "Return", &words);
        assert_eq!(chunks.len(), 3, "{chunks:#?}");
        assert_eq!(
            chunks[0].text,
            "Income details\nGross salary: 9,10,000\nStandard deduction: 75,000"
        );
        assert!(chunks[0]
            .contextualized_text
            .starts_with("Document: \"Return\". Section: Income details. Form fields. "));
        let layout = chunks[0].layout.as_ref().unwrap();
        assert_eq!(layout.unit, "form");
        assert_eq!(layout.regions.len(), 2, "one box per field");
        assert_eq!((layout.page_start, layout.page_end), (Some(1), Some(1)));
        assert_eq!(chunks[1].text, "Net salary: 8,35,000");
        assert_eq!(chunks[1].layout.as_ref().unwrap().page_start, Some(2));
        assert!(chunks[2].text.ends_with("TDS: 12,000"));
        assert_eq!(
            chunks[2].layout.as_ref().unwrap().section_path,
            vec!["Taxes paid"]
        );

        // A long form splits between fields, never inside one.
        let many: Vec<Block> = (0..40)
            .map(|i| {
                field(
                    &format!("Field number {i}"),
                    &format!("{i},000"),
                    3,
                    700.0 - i as f32,
                )
            })
            .collect();
        let chunks = StructureChunker::new(64).chunk(&doc(many), "Return", &words);
        assert!(chunks.len() > 1);
        let mut seen = 0;
        for c in &chunks {
            assert!(words(&c.text) <= 64);
            for line in c.text.lines() {
                assert!(
                    line.starts_with("Field number ") && line.contains(": "),
                    "{line}"
                );
                seen += 1;
            }
        }
        assert_eq!(seen, 40);
    }

    #[test]
    fn an_incomplete_table_and_its_region_text_are_flagged() {
        let table = Block::new(
            BlockKind::Table {
                header: vec!["Item".into(), "Amount".into()],
                rows: vec![vec!["Rent".into(), "1,000".into()]],
                caption: None,
                cell_boxes: Vec::new(),
                cell_coverage: Some(0.6),
            },
            "",
        )
        .on_page(1, Some(BBox::new(40.0, 500.0, 560.0, 600.0)));
        let raw = Block::new(BlockKind::TableText, "Item Amount\nRent 1,000\nWater 250")
            .on_page(1, Some(BBox::new(40.0, 500.0, 560.0, 600.0)));
        let d = doc(vec![
            table,
            raw,
            block(BlockKind::Paragraph, "Unrelated closing words here.", 1),
        ]);
        let chunks = StructureChunker::new(300).chunk(&d, "Bill", &words);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].layout.as_ref().unwrap().incomplete);
        let raw = chunks[1].layout.as_ref().unwrap();
        assert!(raw.incomplete);
        assert_eq!(raw.unit, "table_text");
        assert!(chunks[1].text.contains("Water 250"));
        assert!(chunks[1]
            .contextualized_text
            .contains("Table text as printed."));
        assert!(!chunks[2].layout.as_ref().unwrap().incomplete);
    }

    #[test]
    fn reference_entries_are_individual_chunks() {
        let d = doc(vec![
            block(BlockKind::Heading { level: 1 }, "References", 9),
            block(BlockKind::Paragraph, "[1] A. Author. Paper one. 2020.", 9),
            block(BlockKind::Paragraph, "[2] B. Author. Paper two. 2021.", 9),
        ]);
        let chunks = StructureChunker::new(300).chunk(&d, "Doc", &words);
        assert_eq!(chunks.len(), 2);
        for c in &chunks {
            assert_eq!(c.layout.as_ref().unwrap().unit, "reference_entry");
            assert!(c.contextualized_text.contains("Bibliography entry."));
        }
        assert!(chunks[0].text.ends_with("Paper one. 2020."));
    }

    #[test]
    fn split_text_counts_each_sentence_once_and_respects_the_budget() {
        let calls = std::cell::Cell::new(0usize);
        // Counter with a fixed 5-token overhead, like the embedder's prefix.
        let counter = |t: &str| {
            calls.set(calls.get() + 1);
            words(t) + 5
        };
        let text: String = (0..40)
            .map(|i| sentence(12, &format!("S{i}")))
            .collect::<Vec<_>>()
            .join(" ");
        let pieces = split_text(&text, 50, &counter);
        assert!(pieces.len() > 1);
        for piece in &pieces {
            assert!(
                words(piece) + 5 <= 50,
                "piece over budget: {}",
                words(piece)
            );
        }
        assert_eq!(pieces.join(" "), text);
        // One count for the overhead plus one per sentence: linear.
        assert!(calls.get() <= 41, "tokenizer called {} times", calls.get());

        let code: String = (0..30)
            .map(|i| format!("let x{i} = compute({i});"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        let lines = split_lines(&code, 30, &|t: &str| words(t) + 5);
        assert!(lines.len() > 1);
        assert_eq!(
            lines.join(
                "
"
            ),
            code
        );
    }

    #[test]
    fn sentence_split_respects_abbreviations() {
        let s = split_sentences("See Fig. 3 for details. Results e.g. Table 2 hold. Next one.");
        assert_eq!(
            s,
            vec![
                "See Fig. 3 for details.",
                "Results e.g. Table 2 hold.",
                "Next one."
            ]
        );
    }
}
