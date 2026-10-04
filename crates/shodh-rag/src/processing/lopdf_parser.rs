//! LoPDF-based fallback reader: document metadata and per-page content-stream text,
//! used when the layout parser cannot open a PDF. Form fields and annotations are
//! read by [`super::pdf_forms`].

use anyhow::{anyhow, Context, Result};
use lopdf::{Document, Object};
use std::path::Path;

/// A PDF read by the fallback reader: metadata and per-page text.
#[derive(Debug, Clone)]
pub struct ParsedPdfDocument {
    pub title: Option<String>,
    pub author: Option<String>,
    pub pages: Vec<ParsedPage>,
}

/// Single page with its text.
#[derive(Debug, Clone)]
pub struct ParsedPage {
    pub page_number: usize,
    pub text: String,
}

pub struct LoPdfParser;

impl LoPdfParser {
    pub fn parse(path: &Path) -> Result<ParsedPdfDocument> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("lopdf: failed to read {}", path.display()))?;
        Self::parse_bytes(&bytes)
            .with_context(|| format!("lopdf: failed to load {}", path.display()))
    }

    pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedPdfDocument> {
        let mut doc = Document::load_mem(super::pdf_forms::strip_leading_junk(bytes))
            .context("lopdf: failed to load PDF from memory")?;
        if doc.is_encrypted() {
            doc.decrypt("")
                .map_err(|_| anyhow!("lopdf: PDF is encrypted with a password"))?;
        }
        Self::extract_document(&doc)
    }

    fn extract_document(doc: &Document) -> Result<ParsedPdfDocument> {
        let (title, author) = Self::extract_metadata(doc);

        let page_ids: Vec<(u32, u16)> = doc.get_pages().values().cloned().collect();
        let mut pages = Vec::with_capacity(page_ids.len());

        for (i, &page_id) in page_ids.iter().enumerate() {
            let text = Self::extract_page_text(doc, page_id).unwrap_or_default();
            pages.push(ParsedPage {
                page_number: i + 1,
                text,
            });
        }

        Ok(ParsedPdfDocument {
            title,
            author,
            pages,
        })
    }

    // ── Metadata ──────────────────────────────────────────────────────

    fn extract_metadata(doc: &Document) -> (Option<String>, Option<String>) {
        let mut title = None;
        let mut author = None;

        // Resolve Info dict from the trailer — never assume object (1,0)
        let info_obj = doc.trailer.get(b"Info").ok().and_then(|obj| match obj {
            Object::Reference(id) => doc.get_object(*id).ok(),
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
