//! LoPDF-based PDF parser for structured form field and annotation extraction.
//! Extracts AcroForm fields, per-page annotations, and content stream text.
//! Designed for structured documents: tax returns, payslips, bank statements, forms.

use anyhow::{anyhow, Context, Result};
use lopdf::{Document, Object};
use std::path::Path;

/// Parsed PDF with structured content: pages, form fields, metadata.
#[derive(Debug, Clone)]
pub struct ParsedPdfDocument {
    pub title: Option<String>,
    pub author: Option<String>,
    pub pages: Vec<ParsedPage>,
    pub form_fields: Vec<FormField>,
}

/// Single page with text and annotations.
#[derive(Debug, Clone)]
pub struct ParsedPage {
    pub page_number: usize,
    pub text: String,
    pub annotations: Vec<AnnotationEntry>,
}

/// A single annotation entry: field name (if available) + value.
#[derive(Debug, Clone)]
pub struct AnnotationEntry {
    pub field_name: Option<String>,
    pub value: String,
}

/// AcroForm field: name, value, type, page.
#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub value: Option<String>,
    pub field_type: String,
    pub page: Option<usize>,
}

pub struct LoPdfParser;

impl LoPdfParser {
    pub fn parse(path: &Path) -> Result<ParsedPdfDocument> {
        let doc = Document::load(path)
            .with_context(|| format!("lopdf: failed to load {}", path.display()))?;
        Self::extract_document(&doc)
    }

    pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedPdfDocument> {
        let doc = Document::load_mem(bytes).context("lopdf: failed to load PDF from memory")?;
        Self::extract_document(&doc)
    }

    fn extract_document(doc: &Document) -> Result<ParsedPdfDocument> {
        let (title, author) = Self::extract_metadata(doc);

        let page_ids: Vec<(u32, u16)> = doc.get_pages().values().cloned().collect();
        let mut pages = Vec::with_capacity(page_ids.len());

        for (i, &page_id) in page_ids.iter().enumerate() {
            let text = Self::extract_page_text(doc, page_id).unwrap_or_default();
            let annotations = Self::extract_page_annotations(doc, page_id).unwrap_or_default();

            pages.push(ParsedPage {
                page_number: i + 1,
                text,
                annotations,
            });
        }

        let form_fields = Self::extract_form_fields(doc).unwrap_or_default();

        Ok(ParsedPdfDocument {
            title,
            author,
            pages,
            form_fields,
        })
    }

    // ── Metadata ──────────────────────────────────────────────────────

    fn extract_metadata(doc: &Document) -> (Option<String>, Option<String>) {
        let mut title = None;
        let mut author = None;

        // Resolve Info dict from the PDF trailer rather than assuming a fixed object ID.
        let info_obj = doc
            .trailer
            .get(b"Info")
            .ok()
            .and_then(|info_ref| match info_ref {
                Object::Reference(ref_id) => doc.get_object(*ref_id).ok(),
                other => Some(other),
            });

        if let Some(info) = info_obj {
            if let Ok(dict) = info.as_dict() {
                if let Ok(obj) = dict.get(b"Title") {
                    if let Ok(bytes) = obj.as_str() {
                        let t = decode_pdf_string(bytes);
                        if !t.is_empty() {
                            title = Some(t);
                        }
                    }
                }
                if let Ok(obj) = dict.get(b"Author") {
                    if let Ok(bytes) = obj.as_str() {
                        let a = decode_pdf_string(bytes);
                        if !a.is_empty() {
                            author = Some(a);
                        }
                    }
                }
            }
        }

        (title, author)
    }

    // ── Page text ─────────────────────────────────────────────────────

    fn extract_page_text(doc: &Document, page_id: (u32, u16)) -> Result<String> {
        let page = doc.get_object(page_id)?;
        let page_dict = page.as_dict().map_err(|_| anyhow!("Page is not a dict"))?;

        if let Ok(contents) = page_dict.get(b"Contents") {
            Self::extract_content_text(doc, contents)
        } else {
            Ok(String::new())
        }
    }

    fn extract_content_text(doc: &Document, contents: &Object) -> Result<String> {
        match contents {
            Object::Reference(ref_id) => {
                let obj = doc.get_object(*ref_id)?;
                Self::extract_content_text(doc, &obj)
            }
            Object::Array(arr) => {
                let mut text = String::new();
                for item in arr {
                    if let Ok(t) = Self::extract_content_text(doc, item) {
                        text.push_str(&t);
                    }
                }
                Ok(text)
            }
            Object::Stream(stream) => {
                if let Ok(data) = stream.decode_content() {
                    if let Ok(bytes) = data.encode() {
                        let content = String::from_utf8_lossy(&bytes);
                        Ok(Self::parse_content_stream(&content))
                    } else {
                        Ok(String::new())
                    }
                } else {
                    Ok(String::new())
                }
            }
            _ => Ok(String::new()),
        }
    }

    /// Parse PDF content stream operators (Tj, TJ, ET) to extract text.
    fn parse_content_stream(content: &str) -> String {
        let mut result = String::new();
        let mut current = String::new();

        for line in content.lines() {
            let line = line.trim();

            if line.ends_with("Tj") {
                if let (Some(start), Some(end)) = (line.find('('), line.rfind(')')) {
                    if end > start {
                        current.push_str(&unescape_pdf_string(&line[start + 1..end]));
                        current.push(' ');
                    }
                }
            } else if line.ends_with("TJ") {
                if let (Some(start), Some(end)) = (line.find('['), line.rfind(']')) {
                    if end > start {
                        let arr = &line[start + 1..end];
                        for part in arr.split(')').filter(|s| !s.is_empty()) {
                            if let Some(ts) = part.rfind('(') {
                                current.push_str(&unescape_pdf_string(&part[ts + 1..]));
                            }
                        }
                        current.push(' ');
                    }
                }
            } else if line == "ET" {
                if !current.is_empty() {
                    result.push_str(current.trim());
                    result.push('\n');
                    current.clear();
                }
            }
        }
        if !current.is_empty() {
            result.push_str(current.trim());
        }
        result
    }

    // ── Annotations ───────────────────────────────────────────────────

    fn extract_page_annotations(
        doc: &Document,
        page_id: (u32, u16),
    ) -> Result<Vec<AnnotationEntry>> {
        let mut entries = Vec::new();

        let page = doc.get_object(page_id)?;
        let page_dict = match page.as_dict() {
            Ok(d) => d,
            Err(_) => return Ok(entries),
        };

        let annots_obj = match page_dict.get(b"Annots") {
            Ok(obj) => obj,
            Err(_) => return Ok(entries),
        };

        let annots_resolved = match annots_obj {
            Object::Reference(ref_id) => doc.get_object(*ref_id)?,
            obj => obj,
        };

        let arr = match annots_resolved.as_array() {
            Ok(a) => a,
            Err(_) => return Ok(entries),
        };

        for annot_ref in arr {
            if let Object::Reference(annot_id) = annot_ref {
                if let Ok(annot_obj) = doc.get_object(*annot_id) {
                    if let Ok(dict) = annot_obj.as_dict() {
                        // Get field name from T or TU
                        let field_name = Self::get_dict_string(dict, b"T")
                            .or_else(|| Self::get_dict_string(dict, b"TU"));

                        // Get value from V (form widget) or Contents (standard annotation)
                        let value = Self::get_dict_string(dict, b"V")
                            .or_else(|| Self::get_dict_string(dict, b"Contents"));

                        if let Some(val) = value {
                            if !val.is_empty() {
                                entries.push(AnnotationEntry {
                                    field_name,
                                    value: val,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(entries)
    }

    // ── Form Fields (AcroForm) ────────────────────────────────────────

    fn extract_form_fields(doc: &Document) -> Result<Vec<FormField>> {
        let mut fields = Vec::new();

        let catalog = doc.catalog().map_err(|e| anyhow!("Catalog: {:?}", e))?;

        let acroform_ref = match catalog.get(b"AcroForm") {
            Ok(r) => r,
            Err(_) => return Ok(fields), // No form
        };

        let acroform_obj = match acroform_ref {
            Object::Reference(ref_id) => doc.get_object(*ref_id)?,
            obj => obj,
        };

        let acroform_dict = match acroform_obj.as_dict() {
            Ok(d) => d,
            Err(_) => return Ok(fields),
        };

        let fields_ref = match acroform_dict.get(b"Fields") {
            Ok(r) => r,
            Err(_) => return Ok(fields),
        };

        let fields_obj = match fields_ref {
            Object::Reference(ref_id) => doc.get_object(*ref_id)?,
            obj => obj,
        };

        let fields_arr = match fields_obj.as_array() {
            Ok(a) => a,
            Err(_) => return Ok(fields),
        };

        for field_ref in fields_arr {
            if let Object::Reference(field_id) = field_ref {
                Self::collect_fields(doc, *field_id, &mut fields);
            }
        }

        Ok(fields)
    }

    /// Recursively collect form fields, traversing Kids arrays.
    fn collect_fields(doc: &Document, field_id: (u32, u16), out: &mut Vec<FormField>) {
        let field_obj = match doc.get_object(field_id) {
            Ok(o) => o,
            Err(_) => return,
        };
        let dict = match field_obj.as_dict() {
            Ok(d) => d,
            Err(_) => return,
        };

        let name = Self::get_dict_string(dict, b"T").unwrap_or_default();

        // Check for value in current field, then parent
        let value = Self::get_dict_string(dict, b"V").or_else(|| {
            dict.get(b"Parent").ok().and_then(|p| {
                if let Object::Reference(pid) = p {
                    doc.get_object(*pid).ok().and_then(|pobj| {
                        pobj.as_dict()
                            .ok()
                            .and_then(|pd| Self::get_dict_string(pd, b"V"))
                    })
                } else {
                    None
                }
            })
        });

        let field_type = Self::get_dict_name(dict, b"FT").unwrap_or_else(|| "Unknown".to_string());

        // If this field has a name or value, record it
        if !name.is_empty() || value.is_some() {
            out.push(FormField {
                name,
                value,
                field_type,
                page: None,
            });
        }

        // Recurse into Kids
        if let Ok(kids) = dict.get(b"Kids") {
            if let Ok(kids_arr) = kids.as_array() {
                for kid in kids_arr {
                    if let Object::Reference(kid_id) = kid {
                        Self::collect_fields(doc, *kid_id, out);
                    }
                }
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────

    fn get_dict_string(dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
        dict.get(key).ok().and_then(|obj| match obj {
            Object::String(bytes, _) => {
                let s = decode_pdf_string(bytes);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
            Object::Name(bytes) => {
                let s = decode_pdf_string(bytes);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
            _ => None,
        })
    }

    fn get_dict_name(dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
        dict.get(key).ok().and_then(|obj| {
            if let Object::Name(bytes) = obj {
                Some(String::from_utf8_lossy(bytes).into_owned())
            } else {
                None
            }
        })
    }
}

// ── PDF string decoding ──────────────────────────────────────────────

/// Robust PDF string decoder: handles UTF-8, UTF-16BE, UTF-16LE, PDFDocEncoding.
pub fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    // UTF-16 BOM detection
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16be(&bytes[2..]);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_utf16le(&bytes[2..]);
    }

    // Heuristic: detect UTF-16 without BOM by null-byte pattern
    if bytes.len() >= 4 && bytes.len() % 2 == 0 {
        let odd_nulls = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
        let even_nulls = bytes.iter().step_by(2).filter(|&&b| b == 0).count();
        if odd_nulls > bytes.len() / 4 && odd_nulls > even_nulls {
            return decode_utf16be(bytes);
        }
        if even_nulls > bytes.len() / 4 && even_nulls > odd_nulls {
            return decode_utf16le(bytes);
        }
    }

    String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
}

fn decode_utf16be(bytes: &[u8]) -> String {
    let values: Vec<u16> = bytes
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    clean_decoded(&String::from_utf16_lossy(&values))
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let values: Vec<u16> = bytes
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    clean_decoded(&String::from_utf16_lossy(&values))
}

fn clean_decoded(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '\0' && (c >= ' ' || c == '\t' || c == '\n'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Unescape PDF string escapes (\n, \r, \t, \\, \(, \)).
fn unescape_pdf_string(s: &str) -> String {
    s.replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
        .replace("\\(", "(")
        .replace("\\)", ")")
        .replace("\\\\", "\\")
}

// ── Public helpers ───────────────────────────────────────────────────

impl ParsedPdfDocument {
    /// Build a relationship summary from all form fields and annotations.
    /// Groups related data together without assuming document structure.
    pub fn build_relationship_text(&self) -> String {
        let mut lines = Vec::new();

        // Collect form field key-value pairs
        let filled_fields: Vec<_> = self
            .form_fields
            .iter()
            .filter(|f| f.value.is_some() && !f.name.is_empty())
            .collect();

        if !filled_fields.is_empty() {
            lines.push("=== Form Data ===".to_string());
            for field in &filled_fields {
                lines.push(format!(
                    "{}: {}",
                    field.name,
                    field.value.as_deref().unwrap_or("")
                ));
            }
            lines.push(String::new());
        }

        // Collect annotation key-value pairs per page
        for page in &self.pages {
            let named: Vec<_> = page
                .annotations
                .iter()
                .filter(|a| a.field_name.is_some())
                .collect();
            let unnamed: Vec<_> = page
                .annotations
                .iter()
                .filter(|a| a.field_name.is_none() && !a.value.trim().is_empty())
                .collect();

            if !named.is_empty() {
                lines.push(format!("=== Page {} Fields ===", page.page_number));
                for entry in &named {
                    lines.push(format!(
                        "{}: {}",
                        entry.field_name.as_deref().unwrap_or(""),
                        entry.value
                    ));
                }
                lines.push(String::new());
            }

            if !unnamed.is_empty() {
                lines.push(format!("=== Page {} Data ===", page.page_number));
                for entry in &unnamed {
                    lines.push(entry.value.clone());
                }
                lines.push(String::new());
            }
        }

        lines.join("\n")
    }

    /// Get all form fields as (name, value) pairs.
    pub fn form_field_pairs(&self) -> Vec<(String, String)> {
        self.form_fields
            .iter()
            .filter_map(|f| {
                f.value
                    .as_ref()
                    .filter(|v| !v.is_empty())
                    .map(|v| (f.name.clone(), v.clone()))
            })
            .collect()
    }

    /// Get all annotation entries as (name_or_empty, value) pairs.
    pub fn annotation_pairs(&self) -> Vec<(String, String)> {
        self.pages
            .iter()
            .flat_map(|p| {
                p.annotations
                    .iter()
                    .map(|a| (a.field_name.clone().unwrap_or_default(), a.value.clone()))
            })
            .collect()
    }

    /// Total page count.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Combined text from all pages.
    pub fn full_text(&self) -> String {
        self.pages
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
