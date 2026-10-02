use crate::types::DocumentSection;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ChunkResult {
    pub id: Uuid,
    pub text: String,
    pub index: usize,
    pub heading: Option<String>,
    pub start_offset: usize,
    pub end_offset: usize,
}

pub struct TextChunker {
    chunk_size: usize,
    chunk_overlap: usize,
    min_chunk_size: usize,
}

impl TextChunker {
    pub fn new(chunk_size: usize, chunk_overlap: usize, min_chunk_size: usize) -> Self {
        Self {
            chunk_size,
            chunk_overlap,
            min_chunk_size,
        }
    }

    pub fn chunk(&self, text: &str) -> Vec<ChunkResult> {
        if text.len() <= self.chunk_size {
            if text.len() < self.min_chunk_size {
                return Vec::new();
            }
            return vec![ChunkResult {
                id: Uuid::new_v4(),
                text: text.to_string(),
                index: 0,
                heading: None,
                start_offset: 0,
                end_offset: text.len(),
            }];
        }

        let mut chunks = Vec::new();
        let mut start = 0;
        let mut index = 0;

        while start < text.len() {
            let raw_end = (start + self.chunk_size).min(text.len());
            let end = snap_to_char_boundary(text, raw_end);

            // Try to find a sentence boundary near the end
            let actual_end = if end < text.len() {
                self.find_break_point(text, start, end)
            } else {
                end
            };

            let chunk_text = &text[start..actual_end];

            if chunk_text.len() >= self.min_chunk_size {
                let heading = self.extract_heading(chunk_text);

                chunks.push(ChunkResult {
                    id: Uuid::new_v4(),
                    text: chunk_text.to_string(),
                    index,
                    heading,
                    start_offset: start,
                    end_offset: actual_end,
                });
                index += 1;
            }

            // Move forward with overlap
            let step = if actual_end - start > self.chunk_overlap {
                actual_end - start - self.chunk_overlap
            } else {
                actual_end - start
            };

            let raw_next = start + step;
            start = snap_to_char_boundary(text, raw_next);
            if start >= text.len() {
                break;
            }
        }

        chunks
    }

    fn find_break_point(&self, text: &str, start: usize, preferred_end: usize) -> usize {
        let raw_search_start = if preferred_end > 200 {
            preferred_end - 200
        } else {
            start
        };
        let search_start = snap_to_char_boundary(text, raw_search_start);
        let safe_end = snap_to_char_boundary(text, preferred_end);

        if search_start >= safe_end {
            return safe_end;
        }

        let search_region = &text[search_start..safe_end];

        // Priority: paragraph break > sentence end > line break > word break
        if let Some(pos) = search_region.rfind("\n\n") {
            return search_start + pos + 2;
        }
        if let Some(pos) = search_region.rfind(". ") {
            return search_start + pos + 2;
        }
        if let Some(pos) = search_region.rfind(".\n") {
            return search_start + pos + 2;
        }
        if let Some(pos) = search_region.rfind('\n') {
            return search_start + pos + 1;
        }
        if let Some(pos) = search_region.rfind(' ') {
            return search_start + pos + 1;
        }

        safe_end
    }

    fn extract_heading(&self, text: &str) -> Option<String> {
        let first_line = text.lines().next()?;
        if first_line.starts_with('#') {
            Some(first_line.trim_start_matches('#').trim().to_string())
        } else {
            None
        }
    }
}

/// Snap a byte offset to the nearest valid UTF-8 char boundary (rounding down).
/// If `pos` is already on a boundary, returns `pos` unchanged.
/// If `pos` is beyond text length, returns `text.len()`.
fn snap_to_char_boundary(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    // Walk backwards until we hit a char boundary
    let mut p = pos;
    while p > 0 && !text.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// A chunk with document-level context prepended for embedding.
/// The original text is preserved for display; the contextualized form is used
/// for embedding and full-text indexing to improve retrieval recall.
#[derive(Debug, Clone)]
pub struct ContextualChunkResult {
    pub id: Uuid,
    /// Original chunk text (stored in DB and shown to user)
    pub text: String,
    /// Context-prefixed text (embedded and FTS-indexed for better retrieval)
    pub contextualized_text: String,
    pub index: usize,
    pub heading: Option<String>,
    pub start_offset: usize,
    pub end_offset: usize,
    /// 1-based source page for chunks produced from a paged section (PDF page).
    /// `None` for unpaged content (plain text, spreadsheets, CSV, document-level data).
    pub page: Option<usize>,
}

/// Map a section's page number to an optional page: 0 means "not paged".
fn page_of(page: usize) -> Option<usize> {
    (page > 0).then_some(page)
}

impl TextChunker {
    /// Chunk with document-level context prepended (Anthropic's contextual retrieval approach).
    /// Prepending "Document: X. Section: Y." to each chunk before embedding
    /// improves retrieval by giving the embedding model document-level awareness.
    pub fn chunk_with_context(
        &self,
        text: &str,
        doc_title: &str,
        doc_source: &str,
    ) -> Vec<ContextualChunkResult> {
        let base_chunks = self.chunk(text);

        // Extract first paragraph as document summary (for chunks without headings)
        let doc_summary: String = text
            .split("\n\n")
            .next()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();

        base_chunks
            .into_iter()
            .map(|chunk| {
                let section = chunk
                    .heading
                    .as_deref()
                    .filter(|h| !h.is_empty())
                    .unwrap_or(&doc_summary);

                let context_prefix = format!(
                    "Document: \"{}\". Source: {}. Section: {}. ",
                    doc_title, doc_source, section
                );

                ContextualChunkResult {
                    contextualized_text: format!("{}{}", context_prefix, chunk.text),
                    id: chunk.id,
                    text: chunk.text,
                    index: chunk.index,
                    heading: chunk.heading,
                    start_offset: chunk.start_offset,
                    end_offset: chunk.end_offset,
                    page: None,
                }
            })
            .collect()
    }

    /// Structure-aware chunking for documents with typed sections (PDFs with forms, tables, etc.).
    /// Keeps related data together: all form fields in one chunk, tables as atomic units,
    /// relationship text as a single chunk. Falls back to sliding-window for narrative text.
    pub fn chunk_structured(
        &self,
        sections: &[DocumentSection],
        doc_title: &str,
        doc_source: &str,
    ) -> Vec<ContextualChunkResult> {
        let mut results = Vec::new();
        let mut global_index = 0usize;

        for section in sections {
            match section {
                DocumentSection::FormFields { fields, page } => {
                    let mut body = String::new();
                    for (key, value) in fields {
                        if !key.is_empty() && !value.is_empty() {
                            body.push_str(key);
                            body.push_str(": ");
                            body.push_str(value);
                            body.push('\n');
                        }
                    }
                    let body = body.trim().to_string();
                    if body.is_empty() {
                        continue;
                    }

                    let form_page = page_of(*page);
                    let page_label = match form_page {
                        Some(p) => format!(" (Page {})", p),
                        None => String::new(),
                    };

                    // If form fields fit in one chunk, keep them atomic
                    if body.len() <= self.chunk_size * 2 {
                        let context_prefix = format!(
                            "Document: \"{}\". Source: {}. Form Data{}. ",
                            doc_title, doc_source, page_label
                        );
                        results.push(ContextualChunkResult {
                            id: Uuid::new_v4(),
                            text: body.clone(),
                            contextualized_text: format!("{}{}", context_prefix, body),
                            index: global_index,
                            heading: Some("Form Fields".to_string()),
                            start_offset: 0,
                            end_offset: body.len(),
                            page: form_page,
                        });
                        global_index += 1;
                    } else {
                        // Very large form — split by groups of lines, keeping all fields visible
                        let lines: Vec<&str> = body.lines().collect();
                        let mut chunk_start = 0;
                        while chunk_start < lines.len() {
                            let mut char_count = 0;
                            let mut chunk_end = chunk_start;
                            while chunk_end < lines.len()
                                && char_count + lines[chunk_end].len() < self.chunk_size
                            {
                                char_count += lines[chunk_end].len() + 1;
                                chunk_end += 1;
                            }
                            if chunk_end == chunk_start {
                                chunk_end = chunk_start + 1;
                            }
                            let chunk_text = lines[chunk_start..chunk_end].join("\n");
                            let context_prefix = format!(
                                "Document: \"{}\". Source: {}. Form Data{} (part {}). ",
                                doc_title,
                                doc_source,
                                page_label,
                                results.len() + 1
                            );
                            results.push(ContextualChunkResult {
                                id: Uuid::new_v4(),
                                text: chunk_text.clone(),
                                contextualized_text: format!("{}{}", context_prefix, chunk_text),
                                index: global_index,
                                heading: Some("Form Fields".to_string()),
                                start_offset: 0,
                                end_offset: chunk_text.len(),
                                page: form_page,
                            });
                            global_index += 1;
                            chunk_start = chunk_end;
                        }
                    }
                }

                DocumentSection::Table {
                    headers,
                    rows,
                    page,
                    caption,
                } => {
                    for chunk in self.chunk_table(
                        headers,
                        rows,
                        *page,
                        caption.as_deref(),
                        doc_title,
                        doc_source,
                    ) {
                        results.push(ContextualChunkResult {
                            index: global_index,
                            ..chunk
                        });
                        global_index += 1;
                    }
                }

                DocumentSection::Relationships { content } => {
                    let content = content.trim();
                    if content.is_empty() {
                        continue;
                    }

                    let context_prefix = format!(
                        "Document: \"{}\". Source: {}. Key Relationships. ",
                        doc_title, doc_source
                    );

                    if content.len() <= self.chunk_size * 2 {
                        results.push(ContextualChunkResult {
                            id: Uuid::new_v4(),
                            text: content.to_string(),
                            contextualized_text: format!("{}{}", context_prefix, content),
                            index: global_index,
                            heading: Some("Relationships".to_string()),
                            start_offset: 0,
                            end_offset: content.len(),
                            page: None,
                        });
                        global_index += 1;
                    } else {
                        // Large relationship block — use sliding window
                        let sub_chunks = self.chunk_with_context(content, doc_title, doc_source);
                        for mut sc in sub_chunks {
                            sc.index = global_index;
                            sc.heading = Some("Relationships".to_string());
                            results.push(sc);
                            global_index += 1;
                        }
                    }
                }

                DocumentSection::Text {
                    content,
                    page,
                    heading,
                } => {
                    let content = content.trim();
                    if content.len() < self.min_chunk_size {
                        continue;
                    }

                    let text_page = page_of(*page);
                    let section_label = heading
                        .clone()
                        .or_else(|| text_page.map(|p| format!("Page {}", p)));
                    let page_source = match text_page {
                        Some(p) => format!("{} (Page {})", doc_source, p),
                        None => doc_source.to_string(),
                    };

                    let sub_chunks = self.chunk_with_context(content, doc_title, &page_source);
                    for mut sc in sub_chunks {
                        sc.index = global_index;
                        sc.page = text_page;
                        if sc.heading.is_none() {
                            sc.heading = section_label.clone();
                        }
                        results.push(sc);
                        global_index += 1;
                    }
                }
            }
        }

        results
    }
}

impl TextChunker {
    /// Row-wise table chunking.
    ///
    /// Each chunk starts with the table label (sheet / caption, plus page for
    /// paged sources) and a `Columns: ...` header line, followed by whole rows
    /// rendered as `Header: value | Header: value` so every row is
    /// self-describing for retrieval. Rows are never split. Chunk length is
    /// bounded by `chunk_size` measured in characters; the only exception is a
    /// single row that alone exceeds the budget, which is emitted as its own
    /// (oversized) chunk rather than being cut. The returned `index` values are
    /// local; callers renumber them.
    fn chunk_table(
        &self,
        headers: &[String],
        rows: &[Vec<String>],
        page: usize,
        caption: Option<&str>,
        doc_title: &str,
        doc_source: &str,
    ) -> Vec<ContextualChunkResult> {
        let table_page = page_of(page);
        let base = caption
            .map(single_line)
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "Table".to_string());
        let label = match table_page {
            Some(p) => format!("{} (Page {})", base, p),
            None => base,
        };

        let header_line = format!(
            "Columns: {}",
            headers
                .iter()
                .map(|h| single_line(h))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let row_lines: Vec<String> = rows
            .iter()
            .map(|row| format_table_row(headers, row))
            .filter(|line| !line.is_empty())
            .collect();

        if headers.is_empty() && row_lines.is_empty() {
            return Vec::new();
        }

        let context_prefix = format!(
            "Document: \"{}\". Source: {}. {}. ",
            doc_title, doc_source, label
        );
        let make_chunk = |text: String| ContextualChunkResult {
            id: Uuid::new_v4(),
            contextualized_text: format!("{}{}", context_prefix, text),
            index: 0,
            heading: Some(label.clone()),
            start_offset: 0,
            end_offset: text.len(),
            text,
            page: table_page,
        };

        let row_chars: Vec<usize> = row_lines.iter().map(|l| l.chars().count()).collect();
        let header_chars = header_line.chars().count();
        let whole_table_chars = label.chars().count()
            + 1
            + header_chars
            + row_chars.iter().map(|c| c + 1).sum::<usize>();

        if whole_table_chars <= self.chunk_size {
            let mut text = format!("{}\n{}", label, header_line);
            for line in &row_lines {
                text.push('\n');
                text.push_str(line);
            }
            return vec![make_chunk(text)];
        }

        // Budget the per-chunk prefix with the widest possible row-range label so
        // every chunk respects the bound regardless of which rows it holds.
        let total = row_lines.len();
        let widest_label = format!("{} (rows {}-{} of {})", label, total, total, total);
        let fixed_chars = widest_label.chars().count() + 1 + header_chars;

        pack_rows(&row_chars, fixed_chars, self.chunk_size)
            .into_iter()
            .map(|(start, end)| {
                let mut text = format!(
                    "{} (rows {}-{} of {})\n{}",
                    label,
                    start + 1,
                    end,
                    total,
                    header_line
                );
                for line in &row_lines[start..end] {
                    text.push('\n');
                    text.push_str(line);
                }
                make_chunk(text)
            })
            .collect()
    }
}

/// Collapse embedded line breaks (multi-line CSV cells, Alt+Enter in Excel)
/// so one table row always renders as exactly one line.
fn single_line(value: &str) -> String {
    value
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render a row as `Header: value | Header: value`, skipping empty cells.
/// Cells beyond the header row are labelled `Column N`.
fn format_table_row(headers: &[String], row: &[String]) -> String {
    row.iter()
        .enumerate()
        .filter_map(|(idx, cell)| {
            let value = single_line(cell);
            if value.is_empty() {
                return None;
            }
            let header = headers
                .get(idx)
                .map(|h| single_line(h))
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| format!("Column {}", idx + 1));
            Some(format!("{}: {}", header, value))
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Greedily pack rows into `[start, end)` ranges so that
/// `fixed_chars + sum(row_chars + 1)` stays within `budget`. Every range holds
/// at least one row, so a single oversized row still makes progress.
fn pack_rows(row_chars: &[usize], fixed_chars: usize, budget: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < row_chars.len() {
        let mut used = fixed_chars;
        let mut end = start;
        while end < row_chars.len() {
            let add = row_chars[end] + 1;
            if end > start && used + add > budget {
                break;
            }
            used += add;
            end += 1;
        }
        ranges.push((start, end));
        start = end;
    }
    ranges
}

impl Default for TextChunker {
    fn default() -> Self {
        Self::new(1750, 200, 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(
        headers: &[&str],
        rows: &[Vec<String>],
        page: usize,
        caption: &str,
    ) -> DocumentSection {
        DocumentSection::Table {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            rows: rows.to_vec(),
            page,
            caption: Some(caption.to_string()),
        }
    }

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn small_table_is_single_self_describing_chunk() {
        let chunker = TextChunker::new(500, 50, 10);
        let sections = vec![table(
            &["Region", "Sales"],
            &[row(&["North", "1200"]), row(&["South", ""])],
            0,
            "Q1 Sheet",
        )];
        let chunks = chunker.chunk_structured(&sections, "Report", "report.xlsx");
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].text,
            "Q1 Sheet\nColumns: Region | Sales\nRegion: North | Sales: 1200\nRegion: South"
        );
        assert_eq!(chunks[0].heading.as_deref(), Some("Q1 Sheet"));
        assert_eq!(chunks[0].page, None);
        assert!(chunks[0]
            .contextualized_text
            .starts_with("Document: \"Report\""));
    }

    #[test]
    fn large_table_splits_on_whole_rows_and_repeats_header() {
        let chunk_size = 260;
        let chunker = TextChunker::new(chunk_size, 0, 10);
        let rows: Vec<Vec<String>> = (1..=40)
            .map(|i| {
                vec![
                    format!("Item {}", i),
                    format!("Größe-{}-äöü-日本語", i),
                    format!("{}", i * 100),
                ]
            })
            .collect();
        let sections = vec![table(&["Name", "Größe", "Price"], &rows, 3, "Inventory")];
        let chunks = chunker.chunk_structured(&sections, "Doc", "doc.pdf");
        assert!(chunks.len() > 1, "expected multiple chunks");

        let mut seen_rows = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i);
            assert_eq!(chunk.page, Some(3));
            assert!(
                chunk.text.chars().count() <= chunk_size,
                "chunk {} has {} chars",
                i,
                chunk.text.chars().count()
            );
            let mut lines = chunk.text.lines();
            let label = lines.next().expect("label line");
            assert!(label.starts_with("Inventory (Page 3) (rows "), "{}", label);
            assert_eq!(lines.next(), Some("Columns: Name | Größe | Price"));
            for line in lines {
                // Every row line is complete: all three fields present.
                assert!(line.starts_with("Name: Item "), "{}", line);
                assert!(line.contains(" | Größe: Größe-"), "{}", line);
                assert!(line.contains("-äöü-日本語 | Price: "), "{}", line);
                seen_rows.push(line.to_string());
            }
        }
        assert_eq!(seen_rows.len(), 40);
        assert_eq!(
            seen_rows[0],
            "Name: Item 1 | Größe: Größe-1-äöü-日本語 | Price: 100"
        );
        assert_eq!(
            seen_rows[39],
            "Name: Item 40 | Größe: Größe-40-äöü-日本語 | Price: 4000"
        );
    }

    #[test]
    fn oversized_single_row_is_emitted_whole() {
        let chunker = TextChunker::new(40, 0, 1);
        let long_value = "é".repeat(100);
        let rows = vec![row(&["a", long_value.as_str()]), row(&["b", "short"])];
        let sections = vec![table(&["K", "V"], &rows, 0, "T")];
        let chunks = chunker.chunk_structured(&sections, "Doc", "doc.csv");
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0]
            .text
            .ends_with(&format!("K: a | V: {}", long_value)));
        assert!(chunks[1].text.ends_with("K: b | V: short"));
    }

    #[test]
    fn multiline_cells_never_break_a_row_line() {
        let line = format_table_row(
            &["Note".to_string(), "Qty".to_string()],
            &[
                "first line\r\nsecond line".to_string(),
                "2".to_string(),
                "extra".to_string(),
            ],
        );
        assert_eq!(
            line,
            "Note: first line second line | Qty: 2 | Column 3: extra"
        );
    }

    #[test]
    fn pack_rows_always_progresses_and_respects_budget() {
        assert_eq!(pack_rows(&[5, 5, 5], 10, 22), vec![(0, 2), (2, 3)]);
        assert_eq!(pack_rows(&[50, 1], 10, 20), vec![(0, 1), (1, 2)]);
        assert!(pack_rows(&[], 10, 20).is_empty());
    }

    #[test]
    fn pdf_text_sections_carry_page_and_unpaged_text_does_not() {
        let chunker = TextChunker::new(500, 50, 10);
        let sections = vec![
            DocumentSection::Text {
                content: "This is the body text of the second page of the contract.".to_string(),
                page: 2,
                heading: None,
            },
            DocumentSection::Text {
                content: "Whole-document fallback text without page boundaries.".to_string(),
                page: 0,
                heading: None,
            },
        ];
        let chunks = chunker.chunk_structured(&sections, "Contract", "contract.pdf");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].page, Some(2));
        assert_eq!(chunks[0].heading.as_deref(), Some("Page 2"));
        assert!(chunks[0]
            .contextualized_text
            .contains("contract.pdf (Page 2)"));
        assert_eq!(chunks[1].page, None);
        assert_eq!(chunks[1].heading, None);
        assert!(!chunks[1].contextualized_text.contains("(Page"));
    }

    #[test]
    fn form_fields_page_zero_is_document_level() {
        let chunker = TextChunker::new(500, 50, 10);
        let sections = vec![DocumentSection::FormFields {
            fields: vec![("Name".to_string(), "Asha".to_string())],
            page: 0,
        }];
        let chunks = chunker.chunk_structured(&sections, "Form", "form.pdf");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].page, None);
        assert_eq!(chunks[0].text, "Name: Asha");
    }
}
