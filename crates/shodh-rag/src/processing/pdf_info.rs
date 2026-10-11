//! Cheap facts about a PDF for file lists: title, page count and the size of
//! the first page. Reads the object tree only; no page content is decoded
//! and no text is extracted.

use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::Serialize;

use super::lopdf_parser::decode_pdf_string;

/// What a file list shows about a PDF.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfInfo {
    /// The document title from the Info dictionary, if it has a usable one.
    pub title: Option<String>,
    pub page_count: u32,
    /// First page size in PDF points (1/72 inch), as displayed: width and
    /// height are swapped for a page rotated by 90 or 270 degrees.
    pub first_page_width: Option<f32>,
    pub first_page_height: Option<f32>,
}

#[derive(Debug, thiserror::Error)]
pub enum PdfInfoError {
    #[error("Could not read the PDF: {0}")]
    Read(#[from] lopdf::Error),
}

/// Page attributes a page may inherit from its ancestors in the page tree.
const MAX_TREE_DEPTH: usize = 64;

/// Facts about the PDF at `path`.
pub fn pdf_info(path: &Path) -> Result<PdfInfo, PdfInfoError> {
    Ok(info_of(&Document::load(path)?))
}

/// Facts about a PDF held in memory.
pub fn pdf_info_from_bytes(bytes: &[u8]) -> Result<PdfInfo, PdfInfoError> {
    Ok(info_of(&Document::load_mem(bytes)?))
}

fn info_of(doc: &Document) -> PdfInfo {
    let pages = doc.get_pages();
    let page_count = u32::try_from(pages.len()).unwrap_or(u32::MAX);
    let (first_page_width, first_page_height) = pages
        .values()
        .next()
        .and_then(|&id| page_size(doc, id))
        .map_or((None, None), |(w, h)| (Some(w), Some(h)));
    PdfInfo {
        title: title(doc),
        page_count,
        first_page_width,
        first_page_height,
    }
}

/// The Info dictionary's `Title`, resolved through the trailer (never by
/// assuming an object number), decoded from PDFDocEncoding or UTF-16.
fn title(doc: &Document) -> Option<String> {
    let info = doc.trailer.get(b"Info").ok()?;
    let (_, info) = doc.dereference(info).ok()?;
    let title = info.as_dict().ok()?.get(b"Title").ok()?;
    let (_, title) = doc.dereference(title).ok()?;
    let text = decode_pdf_string(title.as_str().ok()?);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn number(doc: &Document, object: &Object) -> Option<f32> {
    let (_, object) = doc.dereference(object).ok()?;
    match object {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// `key` on the page or, failing that, the nearest ancestor that has it.
fn inherited<'a>(doc: &'a Document, page: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut node = page;
    for _ in 0..MAX_TREE_DEPTH {
        if let Ok(value) = node.get(key) {
            return doc.dereference(value).ok().map(|(_, v)| v);
        }
        let parent = node.get(b"Parent").ok()?.as_reference().ok()?;
        node = doc.get_dictionary(parent).ok()?;
    }
    None
}

/// Displayed width and height of page `id`: its (inherited) CropBox, else
/// MediaBox, turned by its (inherited) Rotate.
fn page_size(doc: &Document, id: ObjectId) -> Option<(f32, f32)> {
    let page = doc.get_dictionary(id).ok()?;
    let rect = inherited(doc, page, b"CropBox")
        .or_else(|| inherited(doc, page, b"MediaBox"))?
        .as_array()
        .ok()?;
    let [x0, y0, x1, y1] = rect.as_slice() else {
        return None;
    };
    let (x0, y0, x1, y1) = (
        number(doc, x0)?,
        number(doc, y0)?,
        number(doc, x1)?,
        number(doc, y1)?,
    );
    let (width, height) = ((x1 - x0).abs(), (y1 - y0).abs());
    let rotate = inherited(doc, page, b"Rotate")
        .and_then(|r| number(doc, r))
        .unwrap_or(0.0) as i64;
    if rotate.rem_euclid(180) == 90 {
        Some((height, width))
    } else {
        Some((width, height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream, StringFormat};

    /// A PDF with `pages` empty pages: the tree carries a Letter MediaBox
    /// and the first page overrides it with A4 rotated by `rotate`.
    fn fixture(title: Option<Object>, pages: usize, rotate: i64) -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let content = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let mut kids = Vec::new();
        for i in 0..pages {
            let mut page = dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content,
            };
            if i == 0 {
                page.set(
                    "MediaBox",
                    vec![0.into(), 0.into(), 595.into(), Object::Real(842.0)],
                );
                page.set("Rotate", rotate);
            }
            kids.push(doc.add_object(page).into());
        }
        let count = i64::try_from(pages).unwrap();
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        if let Some(title) = title {
            let info = doc.add_object(dictionary! { "Title" => title });
            doc.trailer.set("Info", info);
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn reads_title_page_count_and_first_page_size() {
        let title = Object::String(b"Quarterly Report".to_vec(), StringFormat::Literal);
        let info = pdf_info_from_bytes(&fixture(Some(title), 3, 0)).unwrap();
        assert_eq!(info.title.as_deref(), Some("Quarterly Report"));
        assert_eq!(info.page_count, 3);
        assert_eq!(info.first_page_width, Some(595.0));
        assert_eq!(info.first_page_height, Some(842.0));
    }

    #[test]
    fn rotated_pages_swap_width_and_height() {
        let info = pdf_info_from_bytes(&fixture(None, 1, 90)).unwrap();
        assert_eq!(info.first_page_width, Some(842.0));
        assert_eq!(info.first_page_height, Some(595.0));
        assert_eq!(info.title, None);
        let turned = pdf_info_from_bytes(&fixture(None, 1, 270)).unwrap();
        assert_eq!(turned.first_page_width, Some(842.0));
    }

    #[test]
    fn utf16_titles_decode_and_blank_titles_are_none() {
        let mut utf16 = vec![0xFE, 0xFF];
        for unit in "रिपोर्ट 2026".encode_utf16() {
            utf16.extend_from_slice(&unit.to_be_bytes());
        }
        let info = pdf_info_from_bytes(&fixture(
            Some(Object::String(utf16, StringFormat::Hexadecimal)),
            1,
            0,
        ))
        .unwrap();
        assert_eq!(info.title.as_deref(), Some("रिपोर्ट 2026"));
        let blank = pdf_info_from_bytes(&fixture(
            Some(Object::String(b"   ".to_vec(), StringFormat::Literal)),
            1,
            0,
        ))
        .unwrap();
        assert_eq!(blank.title, None);
    }

    #[test]
    fn pages_inherit_the_media_box_from_the_tree() {
        // Every page but the first inherits Letter from the Pages node; the
        // reader only measures the first, so check inheritance directly.
        let bytes = fixture(None, 2, 0);
        let doc = Document::load_mem(&bytes).unwrap();
        let second = *doc.get_pages().get(&2).unwrap();
        assert_eq!(page_size(&doc, second), Some((612.0, 792.0)));
    }

    #[test]
    fn files_that_are_not_pdfs_are_errors() {
        assert!(pdf_info_from_bytes(b"not a pdf").is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        std::fs::write(&path, fixture(None, 2, 0)).unwrap();
        assert_eq!(pdf_info(&path).unwrap().page_count, 2);
    }
}
