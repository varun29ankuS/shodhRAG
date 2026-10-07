use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::document_model::{Block, BlockKind, StructuredDocument};
use super::pdf_layout::{TableMode, TableReport};
use super::table_model::{TableModel, TABLE_MODEL_ID};
use super::{pdf_forms, pdf_layout, tabular, text_structure};
use crate::types::{DocumentFormat, DocumentSection};

#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub content: String,
    pub title: String,
    pub metadata: HashMap<String, String>,
    pub format: DocumentFormat,
    /// Structured sections for PDFs with forms/tables. Empty for plain text formats.
    pub structured_sections: Vec<DocumentSection>,
    /// Semantic blocks (headings, paragraphs, tables, theorems, ...) with
    /// pages and bounding boxes, for formats the structure parsers handle
    /// (PDF, LaTeX, Markdown). `None` for other formats and for PDFs that
    /// only the unpaged fallback extractor could read.
    pub document: Option<StructuredDocument>,
}

/// Metadata key listing a PDF's table-candidate pages (comma-separated, 1-based).
pub const TABLE_CANDIDATES_KEY: &str = "table_candidate_pages";
/// Metadata key naming the table model whose structure a PDF's tables carry.
pub const TABLE_MODEL_KEY: &str = "table_model";
/// Metadata key counting the tables the model structured.
pub const MODEL_TABLES_KEY: &str = "model_tables";

/// Parses files into text, sections and structured documents.
///
/// PDF tables come from the layout heuristics; a parser built
/// [`with_table_model`](Self::with_table_model) structures the tables of the
/// table-candidate pages with the model instead. The fast path records the
/// candidate pages in the metadata ([`TABLE_CANDIDATES_KEY`]) so a caller can
/// refine those files later.
#[derive(Debug, Clone, Default)]
pub struct DocumentParser {
    table_model: Option<Arc<TableModel>>,
}

impl DocumentParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// A parser that structures PDF tables with `model` on candidate pages.
    pub fn with_table_model(model: Arc<TableModel>) -> Self {
        Self {
            table_model: Some(model),
        }
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

        let mut document: Option<StructuredDocument> = None;
        let content = match &tables {
            Some(tables) => tabular::tables_to_text(tables),
            None => match extension.as_str() {
                "pdf" => {
                    let (text, doc, tables) = self.parse_pdf(path)?;
                    document = doc;
                    if let Some(report) = tables {
                        record_table_report(&report, self.table_model.is_some(), &mut metadata);
                    }
                    text
                }
                "tex" | "latex" => {
                    let raw = tabular::read_text_file(path)?;
                    let doc = text_structure::parse_latex(&raw);
                    let text = doc.plain_text();
                    document = Some(doc);
                    text
                }
                "md" | "markdown" => {
                    let raw = tabular::read_text_file(path)?;
                    let doc = text_structure::parse_markdown(&raw);
                    document = Some(doc);
                    raw
                }
                "docx" => self.parse_docx(path)?,
                "pptx" => self.parse_pptx(path)?,
                "html" | "htm" => self.parse_html(path)?,
                "png" | "jpg" | "jpeg" | "bmp" | "tiff" | "tif" => self.parse_image(path)?,
                _ => tabular::read_text_file(path)?,
            },
        };
        if let Some(doc) = &document {
            if !doc.pages.is_empty() {
                metadata.insert("page_count".to_string(), doc.pages.len().to_string());
            }
        }

        // Extract structured sections for formats with tabular/form data. A PDF
        // with a structured document carries its form fields as blocks; only a PDF
        // the layout parser could not read gets sections (page text and fields).
        let structured_sections = match tables {
            Some(tables) => {
                record_table_metadata(&tables, &mut metadata);
                tabular::tables_to_sections(tables)
            }
            None if format == DocumentFormat::PDF && document.is_none() => {
                self.extract_pdf_structure(path, &content)
            }
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
            document,
        })
    }

    /// Parse a PDF into text plus, when the layout parser succeeds, its
    /// structured blocks.
    ///
    /// Order of attempts:
    /// 1. The layout parser ([`pdf_layout`]): paged, positioned blocks.
    /// 2. When the layout parser cannot open the file, `pdf_extract` text
    ///    (unpaged).
    /// 3. When the PDF has no text layer (a scan), Windows OCR page by page.
    ///
    /// A PDF is treated as scanned only when its text layer is essentially
    /// empty across all pages ([`is_effectively_scanned`]), never because one
    /// extractor failed.
    #[allow(clippy::type_complexity)]
    fn parse_pdf(
        &self,
        path: &Path,
    ) -> Result<(String, Option<StructuredDocument>, Option<TableReport>)> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read PDF: {}", path.display()))?;

        // Form fields and comments live outside the text layer; they are placed into
        // the structured document as blocks with their pages and boxes.
        let forms = match pdf_forms::extract_forms(&bytes) {
            Ok(forms) => forms,
            Err(e) => {
                tracing::debug!(error = %e, "PDF form fields not read: {}", path.display());
                pdf_forms::FormExtraction::default()
            }
        };
        if !forms.fields.is_empty() {
            tracing::info!(
                fields = forms.fields.len(),
                stats = ?forms.stats,
                "PDF form fields read: {}",
                path.display()
            );
        }
        let mode = match &self.table_model {
            Some(model) => TableMode::Model(model),
            None => TableMode::Candidates,
        };
        let mut layout_failed = false;
        match pdf_layout::parse_pdf_layout_with(&bytes, mode) {
            Ok(parsed) => {
                let mut doc = parsed.document;
                for error in &parsed.tables.model_errors {
                    tracing::warn!(error = %error, "table model page skipped: {}", path.display());
                }
                let chars = doc.text_chars();
                if !is_effectively_scanned(chars, doc.pages.len()) {
                    insert_form_fields(&mut doc, &forms.fields);
                    return Ok((doc.plain_text(), Some(doc), Some(parsed.tables)));
                }
                tracing::info!(
                    pages = doc.pages.len(),
                    text_chars = chars,
                    "PDF has no text layer; trying OCR: {}",
                    path.display()
                );
            }
            Err(e) => {
                layout_failed = true;
                tracing::warn!(error = %e, "PDF layout parsing failed: {}", path.display());
            }
        }

        if layout_failed {
            if let Some(text) = pdf_extract_text(&bytes) {
                return Ok((text, None, None));
            }
        }

        #[cfg(windows)]
        {
            match super::windows_ocr::ocr_pdf_pages(path) {
                Ok(pages) => {
                    let mut doc = ocr_pages_to_document(&pages);
                    if doc.text_chars() > 0 || !forms.fields.is_empty() {
                        insert_form_fields(&mut doc, &forms.fields);
                        tracing::info!(pages = pages.len(), "Using OCR text: {}", path.display());
                        return Ok((doc.plain_text(), Some(doc), None));
                    }
                }
                Err(e) => tracing::warn!("Windows OCR failed for {}: {:#}", path.display(), e),
            }
        }

        Err(anyhow::anyhow!(
            "PDF contains no extractable text (scanned/image-based and OCR found no text): {}",
            path.display()
        ))
    }

    /// Sections of a PDF the layout parser could not read: its form fields and
    /// comments grouped by page, then per-page text from the fallback reader (or the
    /// whole-document text, unpaged, when that reader finds none).
    fn extract_pdf_structure(&self, path: &Path, fallback_content: &str) -> Vec<DocumentSection> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::debug!("PDF re-read failed for {}: {}", path.display(), e);
                return Vec::new();
            }
        };
        let mut sections = Vec::new();
        if let Ok(forms) = pdf_forms::extract_forms(&bytes) {
            let mut by_page: std::collections::BTreeMap<usize, Vec<(String, String)>> =
                std::collections::BTreeMap::new();
            for field in forms.fields.iter().filter(|f| !f.value.trim().is_empty()) {
                by_page
                    .entry(field.page.map(|p| p as usize).unwrap_or(0))
                    .or_default()
                    .push((field.label.clone(), field.value.clone()));
            }
            for (page, fields) in by_page {
                sections.push(DocumentSection::FormFields { fields, page });
            }
        }
        match super::lopdf_parser::LoPdfParser::parse_bytes(&bytes) {
            Ok(doc) => {
                for page in &doc.pages {
                    let text = page.text.trim();
                    if !text.is_empty() {
                        sections.push(DocumentSection::Text {
                            content: text.to_string(),
                            page: page.page_number,
                            heading: None,
                        });
                    }
                }
            }
            Err(e) => tracing::debug!("lopdf extraction failed for {}: {}", path.display(), e),
        }
        // The fallback content is whole-document text with no page boundaries, so it
        // is recorded as unpaged (page 0) rather than attributed to page 1.
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
            document: None,
        }
    }
}

/// Whether a PDF's text layer is too thin to be its real content: under 200
/// characters in total and under 8 per page. Pages that are figures only are
/// normal in papers, so this looks at the whole document, not single pages.
pub fn is_effectively_scanned(text_chars: usize, pages: usize) -> bool {
    text_chars < 200 && text_chars < 8 * pages.max(1)
}

/// Whole-document text via `pdf_extract`, or `None` when it fails, panics
/// (it can on malformed CMaps) or yields nothing.
fn pdf_extract_text(bytes: &[u8]) -> Option<String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }));
    let text = match result {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            tracing::debug!("pdf_extract failed: {:?}", e);
            return None;
        }
        Err(_) => {
            tracing::warn!("pdf_extract panicked");
            return None;
        }
    };
    let cleaned = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Places form fields into a structured document as [`BlockKind::FormField`]
/// blocks: on their page, before the first block that lies wholly below the field
/// (else after the page's last block), so they read in page order and inherit the
/// section they sit in. Fields without a page go to the end. Empty text and choice
/// fields are left out; checkboxes and signatures always count (`No`, `Not signed`).
pub fn insert_form_fields(doc: &mut StructuredDocument, fields: &[pdf_forms::FormField]) {
    use pdf_forms::FieldKind;
    let mut inserted = 0usize;
    for field in fields {
        let keep = !field.value.trim().is_empty()
            || matches!(field.kind, FieldKind::Checkbox | FieldKind::Signature);
        if !keep {
            continue;
        }
        let mut block = Block::form_field(&field.label, &field.value, field.kind);
        block.page = field.page;
        block.bbox = field.bbox;
        let at = match field.page {
            Some(page) => {
                let centre = field.bbox.map(|b| b.center_y());
                let below = doc.blocks.iter().position(|b| {
                    b.page == Some(page)
                        && centre.is_some_and(|y| b.bbox.is_some_and(|bb| bb.y1 < y))
                });
                below.unwrap_or_else(|| {
                    doc.blocks
                        .iter()
                        .rposition(|b| b.page.is_some_and(|p| p <= page))
                        .map(|i| i + 1)
                        .unwrap_or(0)
                })
            }
            None => doc.blocks.len(),
        };
        doc.blocks.insert(at, block);
        inserted += 1;
    }
    if inserted > 0 {
        doc.finalize();
    }
}

/// One paragraph block per non-empty OCR'd page, numbered from 1.
#[cfg_attr(not(windows), allow(dead_code))]
fn ocr_pages_to_document(pages: &[String]) -> StructuredDocument {
    let blocks = pages
        .iter()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(i, text)| Block::new(BlockKind::Paragraph, text.trim()).on_page(i as u32 + 1, None))
        .collect();
    let mut doc = StructuredDocument {
        pages: Vec::new(),
        blocks,
    };
    doc.finalize();
    doc
}

/// Records the table detection of a PDF parse: its candidate pages, and with the
/// model, the model's id and how many tables it structured.
fn record_table_report(
    report: &TableReport,
    with_model: bool,
    metadata: &mut HashMap<String, String>,
) {
    if !report.candidates.is_empty() {
        let pages: Vec<String> = report
            .candidates
            .iter()
            .map(|c| c.page.to_string())
            .collect();
        metadata.insert(TABLE_CANDIDATES_KEY.to_string(), pages.join(","));
    }
    if with_model {
        metadata.insert(TABLE_MODEL_KEY.to_string(), TABLE_MODEL_ID.to_string());
        metadata.insert(
            MODEL_TABLES_KEY.to_string(),
            report.model_tables.to_string(),
        );
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
            if (tag_lower.starts_with("<td") || tag_lower.starts_with("<th"))
                && !result.is_empty()
                && !result.ends_with('\n')
                && !result.ends_with('\t')
            {
                result.push('\t');
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::pdf_fixtures::{build_pdf, column};

    /// A ToUnicode CMap with a 6-byte bfrange destination. Valid enough for
    /// PDF viewers, but pdf_extract's CMap parser panics on it ("bad length
    /// of hexstring") — the failure that made a text PDF look scanned.
    const ODD_CMAP: &str = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /Odd def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n1 beginbfrange\n<20> <7E> <0020>\nendbfrange\n1 beginbfrange\n<80> <81> <000000000041>\nendbfrange\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

    #[test]
    fn text_pdf_that_crashes_pdf_extract_is_not_treated_as_scanned() {
        let bytes = build_pdf(&[column(72.0, 700.0, "robot", 6)], Some(ODD_CMAP));
        let pdf_extract_result =
            std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes));
        assert!(
            pdf_extract_result.is_err(),
            "fixture must reproduce the pdf_extract panic"
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("robottt-like.pdf");
        std::fs::write(&path, &bytes).expect("write pdf");
        let parsed = DocumentParser::new().parse_file(&path).expect("parsed");
        assert!(
            parsed.content.contains("robot line 0 of body text"),
            "{}",
            parsed.content
        );
        let doc = parsed.document.expect("structured document");
        assert!(!doc.blocks.is_empty());
        assert!(doc.blocks.iter().all(|b| b.page == Some(1)));
        assert_eq!(
            parsed.metadata.get("page_count").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn acroform_fields_become_citable_blocks_on_their_pages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("return-form.pdf");
        std::fs::write(&path, crate::processing::pdf_forms::tests::form_pdf()).expect("write");
        let parsed = DocumentParser::new().parse_file(&path).expect("parsed");
        // Fields are blocks of the document, not a page-less section.
        assert!(parsed.structured_sections.is_empty());
        let doc = parsed.document.expect("structured document");
        let fields: Vec<&Block> = doc
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::FormField { .. }))
            .collect();
        let texts: Vec<&str> = fields.iter().map(|b| b.text.as_str()).collect();
        assert!(
            texts.contains(&"Full name of the applicant: Asha Verma"),
            "{texts:?}"
        );
        assert!(texts.contains(&"Resident of India: Yes (Resident)"));
        assert!(texts.contains(&"Declaration accepted: No"));
        assert!(texts.contains(&"Tax regime: New regime"));
        assert!(texts.contains(&"States of income: Maharashtra, Karnataka"));
        assert!(texts.contains(&"Comment: Check the totals"));
        // The empty field is left out.
        assert!(!texts.iter().any(|t| t.starts_with("notes")));
        let name = fields
            .iter()
            .find(|b| b.text.contains("Asha Verma"))
            .expect("name field");
        assert_eq!(name.page, Some(1));
        assert_eq!(
            name.bbox,
            Some(crate::processing::document_model::BBox::new(
                250.0, 696.0, 450.0, 712.0
            ))
        );
        let employer = fields
            .iter()
            .find(|b| b.text.contains("Acme Tools Ltd"))
            .expect("employer field");
        assert_eq!(employer.page, Some(2));
        // Placed in reading order on its page: after the page-1 heading text above it.
        let heading_at = doc
            .blocks
            .iter()
            .position(|b| b.text.contains("Applicant details"))
            .expect("heading text");
        let name_at = doc
            .blocks
            .iter()
            .position(|b| b.text.contains("Asha Verma"))
            .expect("name");
        assert!(heading_at < name_at);
        // A value drawn by its widget's appearance stream is not read twice.
        assert_eq!(
            parsed.content.matches("Asha Verma").count(),
            1,
            "{}",
            parsed.content
        );

        // The chunker packs the fields into form chunks citing each widget's box.
        let chunks = crate::processing::structure_chunker::StructureChunker::new(300).chunk(
            &doc,
            "Return",
            &|t: &str| t.split_whitespace().count(),
        );
        let form = chunks
            .iter()
            .find(|c| c.text.contains("Asha Verma"))
            .expect("form chunk");
        let layout = form.layout.as_ref().expect("layout");
        assert_eq!(layout.unit, "form");
        assert_eq!(layout.page_start, Some(1));
        assert!(layout
            .regions
            .iter()
            .any(|r| r.page == 1 && r.bbox.x0 == 250.0 && r.bbox.y1 == 712.0));
    }

    #[test]
    fn scanned_only_when_text_is_essentially_absent_across_pages() {
        assert!(is_effectively_scanned(0, 22));
        assert!(is_effectively_scanned(60, 22));
        assert!(!is_effectively_scanned(64_190, 22));
        // A one-page memo with a short text layer is real text.
        assert!(!is_effectively_scanned(40, 1));
        assert!(!is_effectively_scanned(250, 40));
    }

    #[test]
    fn latex_and_markdown_files_get_structured_documents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tex = dir.path().join("paper.tex");
        std::fs::write(
            &tex,
            "\\begin{document}\n\\section{Intro}\nPrimes spiral.\n\\begin{lemma}\nEvery prime is odd or two.\n\\end{lemma}\n\\end{document}\n",
        )
        .expect("write tex");
        let parsed = DocumentParser::new().parse_file(&tex).expect("parsed tex");
        let doc = parsed.document.expect("tex structure");
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(&b.kind, BlockKind::Theorem { label } if label == "Lemma")));
        assert!(parsed.content.contains("Primes spiral."));

        let md = dir.path().join("notes.md");
        std::fs::write(&md, "# Notes\n\nSome text.\n").expect("write md");
        let parsed = DocumentParser::new().parse_file(&md).expect("parsed md");
        assert_eq!(parsed.document.expect("md structure").blocks.len(), 2);
    }
}
