//! A minimal, deterministic PDF writer for the synthetic corpora: US Letter
//! pages of positioned text in the standard Helvetica fonts, or of one
//! full-page image (a scan). No dates or ids, and text pages are not
//! compressed, so the same input always gives the same bytes.

/// One piece of text at a position (points from the bottom-left corner).
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub bold: bool,
    pub text: String,
}

/// The text runs of one page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfPage {
    pub runs: Vec<TextRun>,
}

pub const PAGE_WIDTH: f32 = 612.0;
pub const PAGE_HEIGHT: f32 = 792.0;

/// A PDF string literal body: `\`, `(` and `)` escaped. Text must be ASCII
/// (WinAnsi); the corpus generator checks this.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '(' | ')' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn content_stream(page: &PdfPage) -> String {
    let mut out = String::new();
    for run in &page.runs {
        let font = if run.bold { "F2" } else { "F1" };
        out.push_str(&format!(
            "BT /{font} {:.1} Tf {:.2} {:.2} Td ({}) Tj ET\n",
            run.size,
            run.x,
            run.y,
            escape(&run.text)
        ));
    }
    out
}

/// Serialise `pages` as a PDF 1.4 document titled `title`.
pub fn write_pdf(title: &str, pages: &[PdfPage]) -> Vec<u8> {
    // Objects 1..=5 are fixed; page i (0-based) is object 6 + 2i and its
    // content stream 7 + 2i.
    let page_ids: Vec<usize> = (0..pages.len()).map(|i| 6 + 2 * i).collect();
    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_string(),
        format!(
            "<< /Title ({}) /Producer (shodh-eval synthetic corpus) >>",
            escape(title)
        ),
    ];
    for (i, page) in pages.iter().enumerate() {
        let content_id = 7 + 2 * i;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH:.0} {PAGE_HEIGHT:.0}] \
             /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {content_id} 0 R >>"
        ));
        let stream = content_stream(page);
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}endstream",
            stream.len()
        ));
    }

    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A full-page grey image (a scan): `data` holds `height` rows, one byte per
/// pixel, or one bit per pixel (most significant first, rows padded to a
/// byte, 1 = white) when `bilevel` is set.
#[derive(Debug, Clone, PartialEq)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub bilevel: bool,
    pub data: Vec<u8>,
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    // Writing to a Vec cannot fail.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// Serialise `pages` as a PDF 1.4 document of image-only US Letter pages
/// (no text layer), each image Flate-compressed and drawn over the whole page.
pub fn write_image_pdf(title: &str, pages: &[PageImage]) -> Vec<u8> {
    // Objects 1..=3 are fixed; page i (0-based) is object 4 + 3i, its content
    // stream 5 + 3i and its image 6 + 3i.
    let kids = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + 3 * i))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes(),
        format!(
            "<< /Title ({}) /Producer (shodh-eval scanned corpus) >>",
            escape(title)
        )
        .into_bytes(),
    ];
    for (i, page) in pages.iter().enumerate() {
        let (content_id, image_id) = (5 + 3 * i, 6 + 3 * i);
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH:.0} {PAGE_HEIGHT:.0}] \
                 /Resources << /XObject << /Im0 {image_id} 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        let content = format!("q {PAGE_WIDTH:.0} 0 0 {PAGE_HEIGHT:.0} 0 0 cm /Im0 Do Q\n");
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            )
            .into_bytes(),
        );
        let compressed = deflate(&page.data);
        let mut image = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray \
             /BitsPerComponent {} /Filter /FlateDecode /Length {} >>\nstream\n",
            page.width,
            page.height,
            if page.bilevel { 1 } else { 8 },
            compressed.len()
        )
        .into_bytes();
        image.extend_from_slice(&compressed);
        image.extend_from_slice(b"\nendstream");
        objects.push(image);
    }

    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str) -> PdfPage {
        PdfPage {
            runs: vec![TextRun {
                x: 72.0,
                y: 700.0,
                size: 10.0,
                bold: false,
                text: text.into(),
            }],
        }
    }

    #[test]
    fn output_is_deterministic_and_well_formed() {
        let pages = [page("Total (net): 1,000"), page("Second \\ page")];
        let a = write_pdf("T", &pages);
        assert_eq!(a, write_pdf("T", &pages));
        let text = String::from_utf8_lossy(&a);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("(Total \\(net\\): 1,000) Tj"));
        assert!(text.contains("/Count 2"));
        assert!(text.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn image_pdfs_have_no_text_and_one_image_per_page() {
        let page = PageImage {
            width: 16,
            height: 2,
            bilevel: true,
            data: vec![0xff, 0x0f, 0x00, 0xff],
        };
        let pages = [page.clone(), page];
        let bytes = write_image_pdf("Scan", &pages);
        assert_eq!(bytes, write_image_pdf("Scan", &pages));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 2"));
        assert_eq!(text.matches("/Subtype /Image").count(), 2);
        assert!(text.contains("/BitsPerComponent 1 /Filter /FlateDecode"));
        assert!(!text.contains(" Tj"));
        assert!(text.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        let bytes = write_pdf("T", &[page("x")]);
        // One char per byte, so string offsets are byte offsets.
        let text: String = bytes
            .iter()
            .map(|&b| if b.is_ascii() { b as char } else { '?' })
            .collect();
        let start: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|t| t.lines().next())
            .unwrap()
            .parse()
            .unwrap();
        assert!(text[start..].starts_with("xref\n"));
        let entries: Vec<&str> = text[start..].lines().skip(3).take(7).collect();
        for (i, entry) in entries.iter().enumerate() {
            let offset: usize = entry[..10].parse().unwrap();
            assert!(
                text[offset..].starts_with(&format!("{} 0 obj", i + 1)),
                "object {} at {offset}",
                i + 1
            );
        }
    }
}
