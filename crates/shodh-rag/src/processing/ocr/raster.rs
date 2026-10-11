//! Page images for OCR: PDF pages rendered at a fixed resolution, and image
//! files decoded.
//!
//! PDF pages are rendered by docling.rs's pure-Rust renderer on every
//! platform: it is already part of the build for the table model (MIT, no
//! native library), so this costs no added size, and Windows and macOS read
//! scans through one path. It decodes Flate, LZW, RunLength, DCT (JPEG) and
//! CCITT fax page images; JBIG2 and JPEG 2000 images, which some scanners
//! write, render as blank grey. [`uses_unrendered_codecs`] finds such files so
//! the parser can hand them to the system OCR on Windows (Windows.Data.Pdf
//! decodes both). Windows.Data.Pdf is not used to feed PP-OCRv6: a process
//! that rendered with it and ran the ONNX models under the async runtime did
//! not terminate on exit (a thread kept spinning in the exiting process),
//! measured with `shodh-eval ocr`.

use anyhow::{bail, Context, Result};
use image::RgbImage;

/// Resolution PDF pages are rendered at for OCR. Chosen by measurement on
/// the scanned evaluation set (`shodh-eval ocr`).
pub const RENDER_DPI: u32 = 300;

/// Largest page image OCR accepts, in pixels; larger images are scaled down.
/// A Letter page at 600 dpi is about 34 megapixels.
pub const MAX_PIXELS: u64 = 40_000_000;

/// Points per inch.
const POINTS_PER_INCH: f32 = 72.0;

/// Image codecs the pure-Rust renderer cannot decode.
const UNRENDERED_FILTERS: [&[u8]; 2] = [b"JBIG2Decode", b"JPXDecode"];

/// `img` scaled down (keeping its aspect ratio) so it has at most
/// [`MAX_PIXELS`] pixels.
pub fn limit_size(img: RgbImage) -> RgbImage {
    limit_to(img, MAX_PIXELS)
}

fn limit_to(img: RgbImage, max_pixels: u64) -> RgbImage {
    let pixels = img.width() as u64 * img.height() as u64;
    if pixels <= max_pixels {
        return img;
    }
    let scale = (max_pixels as f64 / pixels as f64).sqrt();
    let w = ((img.width() as f64 * scale) as u32).max(1);
    let h = ((img.height() as f64 * scale) as u32).max(1);
    image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
}

/// An image file (PNG, JPEG, BMP, TIFF; the first frame of a multi-page
/// TIFF) as RGB, limited to [`MAX_PIXELS`].
pub fn load_image(path: &std::path::Path) -> Result<RgbImage> {
    let img = image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("reading {}", path.display()))?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?;
    Ok(limit_size(img.to_rgb8()))
}

/// Whether the PDF in `bytes` has images in a codec the pure-Rust renderer
/// cannot decode (JBIG2, JPEG 2000). `false` when the file cannot be read.
pub fn uses_unrendered_codecs(bytes: &[u8]) -> bool {
    let Ok(doc) = lopdf::Document::load_mem(bytes) else {
        return false;
    };
    let named = |obj: &lopdf::Object| match obj {
        lopdf::Object::Name(name) => UNRENDERED_FILTERS.contains(&name.as_slice()),
        _ => false,
    };
    doc.objects.values().any(|obj| {
        let lopdf::Object::Stream(stream) = obj else {
            return false;
        };
        let is_image = matches!(
            stream.dict.get(b"Subtype"),
            Ok(lopdf::Object::Name(name)) if name.as_slice() == b"Image"
        );
        is_image
            && match stream.dict.get(b"Filter") {
                Ok(lopdf::Object::Array(filters)) => filters.iter().any(named),
                Ok(filter) => named(filter),
                Err(_) => false,
            }
    })
}

/// The pages of a PDF, rendered one at a time.
pub struct PdfPages<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> PdfPages<'a> {
    /// Opens the PDF in `bytes`.
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        let count = docling_pdf::pdfium_backend::page_count(bytes, None)
            .map_err(|e| anyhow::anyhow!("reading the PDF's pages: {e}"))?;
        Ok(Self { bytes, count })
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Page `index` (0-based) rendered at `dpi`, limited to [`MAX_PIXELS`].
    pub fn render(&self, index: usize, dpi: u32) -> Result<RgbImage> {
        if index >= self.count {
            bail!(
                "page {} is outside the document ({} pages)",
                index + 1,
                self.count
            );
        }
        let page = index + 1;
        let bytes = self.bytes;
        let rendered = std::panic::catch_unwind(|| {
            docling_pdf::render_pages(
                bytes,
                None,
                Some((page, page)),
                dpi as f32 / POINTS_PER_INCH,
            )
        })
        .map_err(|_| anyhow::anyhow!("rendering page {page} panicked"))?
        .map_err(|e| anyhow::anyhow!("rendering page {page}: {e}"))?;
        let first = rendered
            .into_iter()
            .next()
            .with_context(|| format!("page {page} was not rendered"))?;
        let img = image::load_from_memory_with_format(&first.png, image::ImageFormat::Png)
            .context("decoding a rendered page")?;
        Ok(limit_size(img.to_rgb8()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page Letter PDF with a black box from (72, 72) to (144, 144) pt
    /// and `extra` objects appended.
    fn box_pdf(extra: &[&str]) -> Vec<u8> {
        let content = "0 0 0 rg 72 72 72 72 re f\n";
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
        ];
        objects.extend(extra.iter().map(|s| s.to_string()));
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn pages_render_at_the_asked_resolution() {
        let bytes = box_pdf(&[]);
        let pages = PdfPages::open(&bytes).unwrap();
        assert_eq!(pages.count(), 1);
        let img = pages.render(0, 150).unwrap();
        assert_eq!((img.width(), img.height()), (1275, 1650));
        // The box spans 150..300 px from the left and from the bottom.
        let inside = img.get_pixel(225, 1650 - 225);
        let outside = img.get_pixel(600, 600);
        assert!(inside.0.iter().all(|&c| c < 64), "{inside:?}");
        assert!(outside.0.iter().all(|&c| c > 192), "{outside:?}");
        assert!(pages.render(1, 150).is_err());
    }

    #[test]
    fn jbig2_and_jpeg2000_images_are_found() {
        assert!(!uses_unrendered_codecs(&box_pdf(&[])));
        let image = |filter: &str| {
            format!(
                "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 1 \
                 /ColorSpace /DeviceGray /Filter {filter} /Length 1 >>\nstream\n\0\nendstream"
            )
        };
        assert!(uses_unrendered_codecs(&box_pdf(&[&image("/JBIG2Decode")])));
        assert!(uses_unrendered_codecs(&box_pdf(&[&image("[/JPXDecode]")])));
        assert!(!uses_unrendered_codecs(&box_pdf(&[&image("/FlateDecode")])));
        assert!(!uses_unrendered_codecs(b"not a pdf"));
    }

    #[test]
    fn oversized_images_are_scaled_down() {
        let limited = limit_to(RgbImage::new(400, 300), 30_000);
        assert_eq!(limited.dimensions(), (200, 150));
        assert_eq!(limit_size(RgbImage::new(10, 10)).dimensions(), (10, 10));
    }
}
