//! Small PDFs generated in tests: text placed at exact positions with the
//! standard Helvetica fonts, so parser behaviour can be asserted on layouts
//! (columns, heading sizes, tables, equations, bibliographies) without
//! checking binary fixtures into the repository.

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};

/// One line of text at a position (PDF points, bottom-left origin).
#[derive(Debug, Clone)]
pub struct Text {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub bold: bool,
    pub text: String,
}

pub fn text(x: f32, y: f32, size: f32, s: &str) -> Text {
    Text {
        x,
        y,
        size,
        bold: false,
        text: s.to_string(),
    }
}

pub fn bold(x: f32, y: f32, size: f32, s: &str) -> Text {
    Text {
        x,
        y,
        size,
        bold: true,
        text: s.to_string(),
    }
}

/// Build a US-Letter PDF with one content stream per page, drawn in the
/// order given (so tests control the content-stream order).
///
/// `regular_to_unicode` attaches a ToUnicode CMap to the regular font.
pub fn build_pdf(pages: &[Vec<Text>], regular_to_unicode: Option<&str>) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut regular = dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    };
    if let Some(cmap) = regular_to_unicode {
        let cmap_id = doc.add_object(Stream::new(dictionary! {}, cmap.as_bytes().to_vec()));
        regular.set("ToUnicode", cmap_id);
    }
    let regular_id = doc.add_object(regular);
    let bold_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica-Bold",
        "Encoding" => "WinAnsiEncoding",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => regular_id, "F2" => bold_id },
    });
    let mut kids: Vec<Object> = Vec::new();
    for page in pages {
        let mut operations = Vec::new();
        for item in page {
            operations.push(Operation::new("BT", vec![]));
            operations.push(Operation::new(
                "Tf",
                vec![
                    Object::Name(if item.bold {
                        b"F2".to_vec()
                    } else {
                        b"F1".to_vec()
                    }),
                    item.size.into(),
                ],
            ));
            operations.push(Operation::new(
                "Tm",
                vec![
                    1.into(),
                    0.into(),
                    0.into(),
                    1.into(),
                    item.x.into(),
                    item.y.into(),
                ],
            ));
            operations.push(Operation::new(
                "Tj",
                vec![Object::string_literal(item.text.as_bytes().to_vec())],
            ));
            operations.push(Operation::new("ET", vec![]));
        }
        let content = Content { operations };
        let encoded = content.encode().expect("encode content stream");
        let content_id = doc.add_object(Stream::new(dictionary! {}, encoded));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("serialize pdf");
    out
}

/// Body-text lines of a column: `n` lines of prose starting at `top`, 12 pt
/// apart, each tagged so tests can find them.
pub fn column(x: f32, top: f32, tag: &str, n: usize) -> Vec<Text> {
    (0..n)
        .map(|i| {
            text(
                x,
                top - 12.0 * i as f32,
                10.0,
                &format!("{tag} line {i} of body text set in this column."),
            )
        })
        .collect()
}
