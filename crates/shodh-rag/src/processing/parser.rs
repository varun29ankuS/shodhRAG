use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use super::tabular;
use crate::types::{DocumentFormat, DocumentSection};

#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub content: String,
    pub title: String,
    pub metadata: HashMap<String, String>,
    pub format: DocumentFormat,
    /// Structured sections for PDFs with forms/tables. Empty for plain text formats.
    pub structured_sections: Vec<DocumentSection>,
}

pub struct DocumentParser;

impl DocumentParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse_file(&self, path: &Path) -> Result<ParsedDocument> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt")
            .to_lowercase();

        let format = DocumentFormat::from_extension(&extension);
        // Use file stem (without extension) for a cleaner display title
        let title = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string();

        let mut metadata = HashMap::new();
        metadata.insert("file_path".to_string(), path.display().to_string());
        metadata.insert("file_extension".to_string(), extension.clone());

        if let Ok(meta) = std::fs::metadata(path) {
            metadata.insert("file_size".to_string(), meta.len().to_string());
        }

        // Tabular formats produce typed tables first; their flat text is derived
        // from the same tables so the file is read exactly once.
        let tables = if tabular::is_spreadsheet_extension(&extension) {
            Some(tabular::read_spreadsheet(path)?)
        } else if tabular::is_delimited_extension(&extension) {
            Some(vec![tabular::read_delimited_file(path, &extension)?])
        } else {
            None
        };

        let content = match &tables {
            Some(tables) => tabular::tables_to_text(tables),
            None => match extension.as_str() {
                "pdf" => self.parse_pdf(path)?,
                "docx" => self.parse_docx(path)?,
                "pptx" => self.parse_pptx(path)?,
                "html" | "htm" => self.parse_html(path)?,
                "png" | "jpg" | "jpeg" | "bmp" | "tiff" | "tif" => self.parse_image(path)?,
                _ => tabular::read_text_file(path)?,
            },
        };

        // Extract structured sections for formats with tabular/form data
        let structured_sections = match tables {
            Some(tables) => {
                record_table_metadata(&tables, &mut metadata);
                tabular::tables_to_sections(tables)
            }
            None if format == DocumentFormat::PDF => self.extract_pdf_structure(path, &content),
            None => Vec::new(),
        };

        if !structured_sections.is_empty() {
            let field_count = structured_sections
                .iter()
                .filter(|s| matches!(s, DocumentSection::FormFields { .. }))
                .count();
            let table_count = structured_sections
                .iter()
                .filter(|s| matches!(s, DocumentSection::Table { .. }))
                .count();
            tracing::info!(
                sections = structured_sections.len(),
                form_field_groups = field_count,
                tables = table_count,
                "Structured extraction complete: {}",
                path.display()
            );
        }

        Ok(ParsedDocument {
            content,
            title,
            metadata,
            format,
            structured_sections,
        })
    }

    fn parse_pdf(&self, path: &Path) -> Result<String> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read PDF: {}", path.display()))?;

        // Layer 1: pdf_extract for fast text extraction
        // Wrapped in catch_unwind because pdf_extract can panic on malformed
        // font tables, CIDFont encodings, or unusual PDF structures.
        let bytes_clone = bytes.clone();
        let text_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pdf_extract::extract_text_from_mem(&bytes_clone)
        }));

        let text_ok = match text_result {
            Ok(Ok(text)) => Some(text),
            Ok(Err(e)) => {
                tracing::debug!("pdf_extract failed on {}: {:?}", path.display(), e);
                None
            }
            Err(_) => {
                tracing::warn!(
                    "pdf_extract panicked on {}, falling through to lopdf/OCR",
                    path.display()
                );
                None
            }
        };

        if let Some(text) = text_ok {
            let cleaned = text
                .lines()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n");

            if !cleaned.is_empty() {
                // Check if extraction looks garbled (column merge artifacts)
                let garble_score = Self::column_garble_score(&cleaned);
                if garble_score < 0.25 {
                    // Good quality — use pdf_extract output
                    return Ok(cleaned);
                }

                // Likely garbled columns — try OCR for better spatial layout
                tracing::info!(
                    garble_score = format!("{:.2}", garble_score),
                    "PDF text extraction appears garbled, attempting OCR: {}",
                    path.display()
                );

                #[cfg(windows)]
                {
                    match super::windows_ocr::ocr_pdf(path) {
                        Ok(ocr_text) if !ocr_text.trim().is_empty() => {
                            tracing::info!("Using OCR output for garbled PDF: {}", path.display());
                            return Ok(ocr_text);
                        }
                        Ok(_) => {
                            tracing::warn!("OCR returned empty text, falling back to pdf_extract");
                        }
                        Err(e) => {
                            tracing::warn!("OCR failed ({}), falling back to pdf_extract", e);
                        }
                    }
                }

                // OCR unavailable or failed — return pdf_extract output as-is
                return Ok(cleaned);
            }
        }

        // pdf_extract failed — try lopdf's content stream parsing
        if let Ok(lopdf_doc) = super::lopdf_parser::LoPdfParser::parse(path) {
            let text = lopdf_doc.full_text();
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }

        // Both failed — try OCR as last resort
        #[cfg(windows)]
        {
            tracing::info!("No text in PDF, attempting Windows OCR: {}", path.display());
            match super::windows_ocr::ocr_pdf(path) {
                Ok(ocr_text) => return Ok(ocr_text),
                Err(e) => {
                    tracing::warn!("Windows OCR failed for {}: {}", path.display(), e);
                }
            }
        }

        Err(anyhow::anyhow!(
            "PDF contains no extractable text (scanned/image-based): {}",
            path.display()
        ))
    }

    /// Score how likely the extracted text is garbled from column merging.
    /// Returns 0.0 (clean) to 1.0 (heavily garbled).
    ///
    /// Heuristic: pdf_extract merges multi-column layouts into single lines,
    /// producing lines with large internal whitespace gaps (3+ spaces) where
    /// unrelated column content gets concatenated. Normal prose never has this.
    fn column_garble_score(text: &str) -> f64 {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() < 3 {
            return 0.0;
        }

        let mut garbled_lines = 0usize;
        let mut scored_lines = 0usize;

        for line in &lines {
            // Skip very short lines (headers, labels)
            if line.len() < 15 {
                continue;
            }
            scored_lines += 1;

            // Count internal whitespace gaps of 5+ spaces — hallmark of column merge.
            // Using 5 instead of 3 to avoid false positives on table-style PDFs
            // (invoices, receipts) where moderate spacing is intentional alignment.
            let gap_count = line
                .as_bytes()
                .windows(5)
                .filter(|w| w.iter().all(|&b| b == b' '))
                .count();

            // Also check for tab characters (another column separator artifact)
            let tab_count = line.chars().filter(|&c| c == '\t').count();

            if gap_count >= 2 || tab_count >= 3 {
                garbled_lines += 1;
            }
        }

        if scored_lines == 0 {
            return 0.0;
        }

        garbled_lines as f64 / scored_lines as f64
    }

    /// Extract structured sections from a PDF using lopdf.
    /// Returns form fields, relationships, and per-page text as typed sections.
    fn extract_pdf_structure(&self, path: &Path, fallback_content: &str) -> Vec<DocumentSection> {
        let lopdf_doc = match super::lopdf_parser::LoPdfParser::parse(path) {
            Ok(doc) => doc,
            Err(e) => {
                tracing::debug!("lopdf extraction failed for {}: {}", path.display(), e);
                return Vec::new();
            }
        };

        let mut sections = Vec::new();

        // 1. Form field pairs → single FormFields section
        let field_pairs = lopdf_doc.form_field_pairs();
        let annotation_pairs = lopdf_doc.annotation_pairs();

        // Merge form fields + named annotations into one set
        let mut all_pairs: Vec<(String, String)> = field_pairs;
        for (name, value) in annotation_pairs {
            if !name.is_empty() && !all_pairs.iter().any(|(n, _)| n == &name) {
                all_pairs.push((name, value));
            }
        }

        if !all_pairs.is_empty() {
            sections.push(DocumentSection::FormFields {
                fields: all_pairs,
                page: 0, // document-level
            });
        }

        // 2. Relationship text from all form data + annotations
        let relationship_text = lopdf_doc.build_relationship_text();
        if !relationship_text.trim().is_empty() {
            sections.push(DocumentSection::Relationships {
                content: relationship_text,
            });
        }

        // 3. Per-page text sections
        for page in &lopdf_doc.pages {
            let text = page.text.trim();
            if text.is_empty() {
                continue;
            }
            sections.push(DocumentSection::Text {
                content: text.to_string(),
                page: page.page_number,
                heading: None,
            });
        }

        // If lopdf produced no page text but we have fallback content,
        // add it as a single text section. The fallback is whole-document text
        // with no page boundaries, so it is recorded as unpaged (page 0)
        // rather than misattributing everything to page 1.
        let has_text_sections = sections
            .iter()
            .any(|s| matches!(s, DocumentSection::Text { .. }));
        if !has_text_sections && !fallback_content.trim().is_empty() {
            sections.push(DocumentSection::Text {
                content: fallback_content.to_string(),
                page: 0,
                heading: None,
            });
        }

        sections
    }

    fn parse_docx(&self, path: &Path) -> Result<String> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open DOCX: {}", path.display()))?;

        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("Failed to read DOCX as ZIP: {}", path.display()))?;

        let mut xml_content = String::new();
        {
            let mut document_xml = archive
                .by_name("word/document.xml")
                .with_context(|| format!("DOCX missing word/document.xml: {}", path.display()))?;
            use std::io::Read;
            document_xml
                .read_to_string(&mut xml_content)
                .with_context(|| "Failed to read document.xml from DOCX")?;
        }

        let text = extract_docx_text(&xml_content);

        if text.is_empty() {
            return Err(anyhow::anyhow!(
                "DOCX contains no extractable text: {}",
                path.display()
            ));
        }

        Ok(text)
    }

    fn parse_image(&self, path: &Path) -> Result<String> {
        #[cfg(windows)]
        {
            super::windows_ocr::ocr_image(path)
        }

        #[cfg(not(windows))]
        {
            Err(anyhow::anyhow!(
                "Image OCR not available on this platform: {}",
                path.display()
            ))
        }
    }

    /// Parse PPTX by extracting text from each slide's XML.
    fn parse_pptx(&self, path: &Path) -> Result<String> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open PPTX: {}", path.display()))?;

        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("Failed to read PPTX as ZIP: {}", path.display()))?;

        let mut slides: Vec<(usize, String)> = Vec::new();

        for i in 0..archive.len() {
            let mut entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.name().to_string();
            // Slide XML files: ppt/slides/slide1.xml, slide2.xml, ...
            if !name.starts_with("ppt/slides/slide") || !name.ends_with(".xml") {
                continue;
            }

            // Extract slide number from filename
            let slide_num = name
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse::<usize>()
                .unwrap_or(0);

            let mut xml = String::new();
            use std::io::Read;
            if entry.read_to_string(&mut xml).is_ok() {
                let text = extract_pptx_slide_text(&xml);
                if !text.is_empty() {
                    slides.push((slide_num, text));
                }
            }
        }

        if slides.is_empty() {
            return Err(anyhow::anyhow!(
                "PPTX contains no extractable text: {}",
                path.display()
            ));
        }

        slides.sort_by_key(|(num, _)| *num);

        let text = slides
            .into_iter()
            .map(|(num, text)| format!("--- Slide {} ---\n{}", num, text))
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(text)
    }

    /// Parse HTML by stripping tags and extracting visible text.
    fn parse_html(&self, path: &Path) -> Result<String> {
        let raw = tabular::read_text_file(path)
            .with_context(|| format!("Failed to read HTML: {}", path.display()))?;

        Ok(strip_html_tags(&raw))
    }

    pub fn parse_content(
        &self,
        content: &str,
        format: DocumentFormat,
        title: &str,
    ) -> ParsedDocument {
        ParsedDocument {
            content: content.to_string(),
            title: title.to_string(),
            metadata: HashMap::new(),
            format,
            structured_sections: Vec::new(),
        }
    }
}

/// Record per-table statistics (sheet count, row count, numeric columns) in
/// document metadata. Numeric column hints are consumed by chart generation.
fn record_table_metadata(
    tables: &[tabular::ExtractedTable],
    metadata: &mut HashMap<String, String>,
) {
    metadata.insert("sheet_count".to_string(), tables.len().to_string());
    let total_rows: usize = tables.iter().map(|t| t.rows.len()).sum();
    metadata.insert("total_data_rows".to_string(), total_rows.to_string());
    for (idx, table) in tables.iter().enumerate() {
        metadata.insert(format!("sheet_{}_name", idx), table.name.clone());
        let numeric = tabular::numeric_columns(table);
        if !numeric.is_empty() {
            metadata.insert(format!("sheet_{}_numeric_columns", idx), numeric.join(","));
        }
    }
}

/// Extract text from PPTX slide XML by parsing <a:t> elements within <a:p> paragraphs.
fn extract_pptx_slide_text(xml: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;

    while pos < xml.len() {
        if let Some(p_start) = xml[pos..].find("<a:p") {
            let abs_p_start = pos + p_start;
            let p_end = xml[abs_p_start..]
                .find("</a:p>")
                .map(|e| abs_p_start + e + 6)
                .unwrap_or(xml.len());

            let paragraph = &xml[abs_p_start..p_end];
            let mut para_text = String::new();
            let mut t_pos = 0;

            while t_pos < paragraph.len() {
                if let Some(t_start) = paragraph[t_pos..].find("<a:t") {
                    let abs_t_start = t_pos + t_start;
                    if let Some(tag_end) = paragraph[abs_t_start..].find('>') {
                        let content_start = abs_t_start + tag_end + 1;
                        if let Some(t_end) = paragraph[content_start..].find("</a:t>") {
                            para_text.push_str(&paragraph[content_start..content_start + t_end]);
                            t_pos = content_start + t_end + 6;
                        } else {
                            t_pos = content_start;
                        }
                    } else {
                        t_pos = abs_t_start + 4;
                    }
                } else {
                    break;
                }
            }

            if !para_text.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&para_text);
            }

            pos = p_end;
        } else {
            break;
        }
    }

    result
}

/// Strip HTML tags and decode common entities, returning visible text content.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_whitespace = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if in_script {
            // Skip until </script>
            if i + 9 <= len && &lower[i..i + 9] == "</script>" {
                in_script = false;
                i += 9;
            } else {
                i += 1;
            }
            continue;
        }
        if in_style {
            if i + 8 <= len && &lower[i..i + 8] == "</style>" {
                in_style = false;
                i += 8;
            } else {
                i += 1;
            }
            continue;
        }

        if chars[i] == '<' {
            // Check for <script or <style
            if i + 7 <= len && &lower[i..i + 7] == "<script" {
                in_script = true;
                i += 7;
                continue;
            }
            if i + 6 <= len && &lower[i..i + 6] == "<style" {
                in_style = true;
                i += 6;
                continue;
            }
            in_tag = true;

            // Block elements get a newline
            let tag_lower = &lower[i..];
            let is_block = tag_lower.starts_with("<p")
                || tag_lower.starts_with("<div")
                || tag_lower.starts_with("<br")
                || tag_lower.starts_with("<h1")
                || tag_lower.starts_with("<h2")
                || tag_lower.starts_with("<h3")
                || tag_lower.starts_with("<h4")
                || tag_lower.starts_with("<li")
                || tag_lower.starts_with("<tr")
                || tag_lower.starts_with("</p")
                || tag_lower.starts_with("</div")
                || tag_lower.starts_with("</tr");

            if is_block && !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
                last_was_whitespace = true;
            }

            // <td> / <th> get a tab separator
            if tag_lower.starts_with("<td") || tag_lower.starts_with("<th") {
                if !result.is_empty() && !result.ends_with('\n') && !result.ends_with('\t') {
                    result.push('\t');
                }
            }

            i += 1;
            continue;
        }

        if chars[i] == '>' && in_tag {
            in_tag = false;
            i += 1;
            continue;
        }

        if !in_tag {
            // Decode HTML entities
            if chars[i] == '&' {
                if i + 4 <= len && &html[i..i + 4] == "&lt;" {
                    result.push('<');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                }
                if i + 4 <= len && &html[i..i + 4] == "&gt;" {
                    result.push('>');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                }
                if i + 5 <= len && &html[i..i + 5] == "&amp;" {
                    result.push('&');
                    i += 5;
                    last_was_whitespace = false;
                    continue;
                }
                if i + 6 <= len && &html[i..i + 6] == "&nbsp;" {
                    result.push(' ');
                    i += 6;
                    last_was_whitespace = true;
                    continue;
                }
                if i + 6 <= len && &html[i..i + 6] == "&quot;" {
                    result.push('"');
                    i += 6;
                    last_was_whitespace = false;
                    continue;
                }
            }

            let ch = chars[i];
            if ch.is_whitespace() {
                if !last_was_whitespace && !result.is_empty() {
                    result.push(if ch == '\n' { '\n' } else { ' ' });
                    last_was_whitespace = true;
                }
            } else {
                result.push(ch);
                last_was_whitespace = false;
            }
        }
        i += 1;
    }

    // Clean up excessive blank lines
    let mut cleaned = String::with_capacity(result.len());
    let mut blank_lines = 0;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 {
                cleaned.push('\n');
            }
        } else {
            blank_lines = 0;
            if !cleaned.is_empty() && !cleaned.ends_with('\n') {
                cleaned.push('\n');
            }
            cleaned.push_str(trimmed);
        }
    }

    cleaned
}

/// Extract text from DOCX XML by parsing <w:t> elements within <w:p> paragraphs
fn extract_docx_text(xml: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;

    while pos < xml.len() {
        if let Some(p_start) = xml[pos..].find("<w:p") {
            let abs_p_start = pos + p_start;

            let p_end = if let Some(end) = xml[abs_p_start..].find("</w:p>") {
                abs_p_start + end + 6
            } else {
                xml.len()
            };

            let paragraph = &xml[abs_p_start..p_end];
            let mut para_text = String::new();
            let mut t_pos = 0;

            while t_pos < paragraph.len() {
                if let Some(t_start) = paragraph[t_pos..].find("<w:t") {
                    let abs_t_start = t_pos + t_start;
                    if let Some(tag_end) = paragraph[abs_t_start..].find('>') {
                        let content_start = abs_t_start + tag_end + 1;
                        if let Some(t_end) = paragraph[content_start..].find("</w:t>") {
                            para_text.push_str(&paragraph[content_start..content_start + t_end]);
                            t_pos = content_start + t_end + 6;
                        } else {
                            t_pos = content_start;
                        }
                    } else {
                        t_pos = abs_t_start + 4;
                    }
                } else {
                    break;
                }
            }

            if !para_text.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&para_text);
            }

            pos = p_end;
        } else {
            break;
        }
    }

    result
}
