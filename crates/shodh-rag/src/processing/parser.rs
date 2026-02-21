use anyhow::{Context, Result};
use calamine::{open_workbook_auto, Data, Reader};
use std::collections::HashMap;
use std::path::Path;

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

        let content = match extension.as_str() {
            "pdf" => self.parse_pdf(path)?,
            "docx" => self.parse_docx(path)?,
            "xlsx" | "xls" | "ods" | "xlsm" | "xlsb" => self.parse_spreadsheet(path)?,
            "pptx" => self.parse_pptx(path)?,
            "html" | "htm" => self.parse_html(path)?,
            "png" | "jpg" | "jpeg" | "bmp" | "tiff" | "tif" => self.parse_image(path)?,
            _ => std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read text file: {}", path.display()))?,
        };

        let mut metadata = HashMap::new();
        metadata.insert("file_path".to_string(), path.display().to_string());
        metadata.insert("file_extension".to_string(), extension.clone());

        if let Ok(meta) = std::fs::metadata(path) {
            metadata.insert("file_size".to_string(), meta.len().to_string());
        }

        // Extract structured sections for formats with tabular/form data
        let structured_sections = match format {
            DocumentFormat::PDF => self.extract_pdf_structure(path, &content),
            DocumentFormat::Spreadsheet => self.extract_spreadsheet_structure(path, &mut metadata),
            _ => Vec::new(),
        };

        if !structured_sections.is_empty() {
            let field_count = structured_sections.iter().filter(|s| matches!(s, DocumentSection::FormFields { .. })).count();
            tracing::info!(sections = structured_sections.len(), form_field_groups = field_count, "PDF structured extraction complete");
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
        let text_result = pdf_extract::extract_text_from_mem(&bytes);

        if let Ok(text) = text_result {
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

            // Count internal whitespace gaps of 3+ spaces — hallmark of column merge
            let gap_count = line
                .as_bytes()
                .windows(3)
                .filter(|w| w.iter().all(|&b| b == b' '))
                .count();

            // Also check for tab characters (another column separator artifact)
            let tab_count = line.chars().filter(|&c| c == '\t').count();

            if gap_count >= 1 || tab_count >= 2 {
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
        // add it as a single text section
        let has_text_sections = sections.iter().any(|s| matches!(s, DocumentSection::Text { .. }));
        if !has_text_sections && !fallback_content.trim().is_empty() {
            sections.push(DocumentSection::Text {
                content: fallback_content.to_string(),
                page: 1,
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

    /// Parse Excel/ODS spreadsheet into flat text (one row per line, pipe-separated).
    fn parse_spreadsheet(&self, path: &Path) -> Result<String> {
        let mut workbook = open_workbook_auto(path)
            .with_context(|| format!("Failed to open spreadsheet: {}", path.display()))?;

        let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
        if sheet_names.is_empty() {
            return Err(anyhow::anyhow!("Spreadsheet has no sheets: {}", path.display()));
        }

        let mut all_text = String::new();

        for sheet_name in &sheet_names {
            let range = match workbook.worksheet_range(sheet_name) {
                Ok(r) => r,
                Err(_) => continue,
            };

            if range.is_empty() {
                continue;
            }

            if sheet_names.len() > 1 {
                all_text.push_str(&format!("\n--- Sheet: {} ---\n", sheet_name));
            }

            for row in range.rows() {
                let cells: Vec<String> = row.iter().map(cell_to_string).collect();
                // Skip fully empty rows
                if cells.iter().all(|c| c.is_empty()) {
                    continue;
                }
                all_text.push_str(&cells.join(" | "));
                all_text.push('\n');
            }
        }

        if all_text.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "Spreadsheet contains no data: {}",
                path.display()
            ));
        }

        Ok(all_text)
    }

    /// Extract per-sheet `DocumentSection::Table` sections from a spreadsheet.
    /// First non-empty row of each sheet is treated as headers; remaining rows are data.
    /// Also populates metadata with sheet count and total row count.
    fn extract_spreadsheet_structure(
        &self,
        path: &Path,
        metadata: &mut HashMap<String, String>,
    ) -> Vec<DocumentSection> {
        let mut workbook = match open_workbook_auto(path) {
            Ok(wb) => wb,
            Err(e) => {
                tracing::warn!("Spreadsheet re-open failed for structure extraction: {}", e);
                return Vec::new();
            }
        };

        let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
        metadata.insert("sheet_count".to_string(), sheet_names.len().to_string());

        let mut sections = Vec::new();
        let mut total_rows: usize = 0;

        for (sheet_idx, sheet_name) in sheet_names.iter().enumerate() {
            let range = match workbook.worksheet_range(sheet_name) {
                Ok(r) => r,
                Err(_) => continue,
            };

            if range.is_empty() {
                continue;
            }

            let all_rows: Vec<Vec<String>> = range
                .rows()
                .map(|row| row.iter().map(cell_to_string).collect())
                .filter(|row: &Vec<String>| !row.iter().all(|c| c.is_empty()))
                .collect();

            if all_rows.is_empty() {
                continue;
            }

            // First row = headers, rest = data
            let headers = all_rows[0].clone();
            let data_rows: Vec<Vec<String>> = all_rows.into_iter().skip(1).collect();
            total_rows += data_rows.len();

            // Detect numeric columns for downstream chart generation
            let numeric_cols: Vec<usize> = (0..headers.len())
                .filter(|&col_idx| {
                    let numeric_count = data_rows
                        .iter()
                        .filter(|row| {
                            row.get(col_idx)
                                .map(|v| !v.is_empty() && v.parse::<f64>().is_ok())
                                .unwrap_or(false)
                        })
                        .count();
                    // Column is numeric if >50% of non-empty values parse as numbers
                    numeric_count > 0 && numeric_count * 2 >= data_rows.len()
                })
                .collect();

            if !numeric_cols.is_empty() {
                let numeric_headers: Vec<&str> = numeric_cols
                    .iter()
                    .filter_map(|&i| headers.get(i).map(|h| h.as_str()))
                    .collect();
                metadata.insert(
                    format!("sheet_{}_numeric_columns", sheet_idx),
                    numeric_headers.join(","),
                );
            }

            let caption = if sheet_names.len() > 1 {
                Some(sheet_name.clone())
            } else {
                None
            };

            sections.push(DocumentSection::Table {
                headers,
                rows: data_rows,
                page: sheet_idx + 1,
                caption,
            });
        }

        metadata.insert("total_data_rows".to_string(), total_rows.to_string());
        if !sections.is_empty() {
            tracing::info!(
                sheets = sections.len(),
                total_rows = total_rows,
                "Spreadsheet structured extraction complete"
            );
        }

        sections
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
        let raw = std::fs::read_to_string(path)
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

/// Convert a calamine cell to a clean string representation.
fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            // Use integer display when the float is a whole number (e.g. 1500.0 → "1500")
            if f.fract() == 0.0 && f.abs() < i64::MAX as f64 {
                (*f as i64).to_string()
            } else {
                format!("{:.4}", f).trim_end_matches('0').trim_end_matches('.').to_string()
            }
        }
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::Error(e) => format!("#ERR:{:?}", e),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
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
