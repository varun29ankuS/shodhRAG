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

                    let page_label = if *page > 0 {
                        format!(" (Page {})", page)
                    } else {
                        String::new()
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
                    if rows.is_empty() {
                        continue;
                    }

                    let cap = caption.as_deref().unwrap_or("Table");
                    let header_line = format!("| {} |", headers.join(" | "));
                    let separator = format!(
                        "| {} |",
                        headers
                            .iter()
                            .map(|_| "---")
                            .collect::<Vec<_>>()
                            .join(" | ")
                    );

                    // Build full table as markdown
                    let mut table_body = format!("{}\n{}\n", header_line, separator);
                    for row in rows {
                        table_body.push_str(&format!("| {} |\n", row.join(" | ")));
                    }
                    let table_body = table_body.trim().to_string();

                    let context_prefix = format!(
                        "Document: \"{}\". Source: {}. {} (Page {}). ",
                        doc_title, doc_source, cap, page
                    );

                    // If table fits in one chunk, keep it atomic
                    if table_body.len() <= self.chunk_size * 2 {
                        results.push(ContextualChunkResult {
                            id: Uuid::new_v4(),
                            text: table_body.clone(),
                            contextualized_text: format!("{}{}", context_prefix, table_body),
                            index: global_index,
                            heading: Some(format!("Table (Page {})", page)),
                            start_offset: 0,
                            end_offset: table_body.len(),
                        });
                        global_index += 1;
                    } else {
                        // Large table — split by row groups, repeat headers in each chunk
                        let row_lines: Vec<String> = rows
                            .iter()
                            .map(|row| format!("| {} |", row.join(" | ")))
                            .collect();
                        let header_block = format!("{}\n{}", header_line, separator);
                        let header_len = header_block.len() + 1;

                        let mut row_start = 0;
                        let mut part = 1;
                        while row_start < row_lines.len() {
                            let mut char_count = header_len;
                            let mut row_end = row_start;
                            while row_end < row_lines.len()
                                && char_count + row_lines[row_end].len() + 1 < self.chunk_size
                            {
                                char_count += row_lines[row_end].len() + 1;
                                row_end += 1;
                            }
                            if row_end == row_start {
                                row_end = row_start + 1;
                            }
                            let chunk_text = format!(
                                "{}\n{}",
                                header_block,
                                row_lines[row_start..row_end].join("\n")
                            );
                            let ctx = format!(
                                "Document: \"{}\". Source: {}. {} (Page {}, part {}). ",
                                doc_title, doc_source, cap, page, part
                            );
                            results.push(ContextualChunkResult {
                                id: Uuid::new_v4(),
                                text: chunk_text.clone(),
                                contextualized_text: format!("{}{}", ctx, chunk_text),
                                index: global_index,
                                heading: Some(format!("Table (Page {})", page)),
                                start_offset: 0,
                                end_offset: chunk_text.len(),
                            });
                            global_index += 1;
                            row_start = row_end;
                            part += 1;
                        }
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

                    let page_label = format!("Page {}", page);
                    let section_label = heading.as_deref().unwrap_or(&page_label);
                    let page_source = format!("{} (Page {})", doc_source, page);

                    let sub_chunks = self.chunk_with_context(content, doc_title, &page_source);
                    for mut sc in sub_chunks {
                        sc.index = global_index;
                        if sc.heading.is_none() {
                            sc.heading = Some(section_label.to_string());
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

impl Default for TextChunker {
    fn default() -> Self {
        Self::new(1750, 200, 100)
    }
}
