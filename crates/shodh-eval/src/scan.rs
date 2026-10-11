//! The scanned corpus: pages of the synthetic contracts and invoices drawn
//! as images, the way a scanner delivers them (no text layer), with the
//! questions whose answers sit on those pages. It measures OCR through the
//! real indexing path: a page that is misread loses its passage hits.
//!
//! Pages are drawn from the synthetic corpus's page layouts with Liberation
//! Sans (committed under `data/fonts`, SIL Open Font License; metric-compatible
//! with the Helvetica the text PDFs name), so the images do not depend on the
//! fonts of the machine. Each page gets its own scan settings: resolution
//! (150, 200 or 300 dpi), skew (up to 2 degrees), grey or black-and-white,
//! and speckle noise from a seeded generator. Three pages hold tables.
//!
//! `shodh-eval synth-scanned --out <dir>` writes `<dir>/corpus/**`,
//! `<dir>/dataset.json` and `<dir>/ground-truth.json` (each scanned file's
//! text by page, for the OCR text measure in `shodh-eval ocr`). A unit test
//! checks the committed copy in `crates/shodh-eval/data/scanned` is
//! byte-identical to this generator's output. Some research notes and
//! policies of the synthetic corpus are copied in as text distractors.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::dataset::{Dataset, EvalCase};
use crate::pdf::{write_image_pdf, PageImage, PdfPage, PAGE_HEIGHT, PAGE_WIDTH};
use crate::synth::{SynthCorpus, SynthFile};

pub const DATASET_ID: &str = "scanned-v1";

const REGULAR: &[u8] = include_bytes!("../data/fonts/LiberationSans-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../data/fonts/LiberationSans-Bold.ttf");

/// How a page was scanned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scan {
    pub dpi: u32,
    /// Clockwise rotation of the page content, in degrees.
    pub skew_deg: f32,
    /// Black and white (1 bit per pixel) rather than grey.
    pub bilevel: bool,
    /// Share of pixels hit by a dark speck.
    pub speckle: f32,
}

const fn scan(dpi: u32, skew_deg: f32, bilevel: bool, speckle: f32) -> Scan {
    Scan {
        dpi,
        skew_deg,
        bilevel,
        speckle,
    }
}

/// One scanned file: the synthetic PDF it was scanned from and how each of
/// its pages (1-based) was scanned. A `.png` holds one page.
struct ScanDoc {
    path: &'static str,
    title: &'static str,
    source: &'static str,
    pages: Vec<(usize, Scan)>,
}

fn scan_docs() -> Vec<ScanDoc> {
    vec![
        ScanDoc {
            path: "contracts/msa-northwind-bluepeak-scan.pdf",
            title: "Master Services Agreement (scanned)",
            source: "contracts/msa-northwind-bluepeak.pdf",
            pages: vec![
                (1, scan(300, 0.0, true, 0.0)),
                (2, scan(300, 1.5, false, 0.0)),
                (3, scan(150, 0.0, false, 0.0)),
            ],
        },
        ScanDoc {
            path: "contracts/lease-saffron-tidewater-scan.pdf",
            title: "Commercial Lease Deed (scanned)",
            source: "contracts/lease-saffron-tidewater.pdf",
            pages: vec![
                (1, scan(200, 0.0, false, 0.002)),
                (2, scan(300, -1.0, true, 0.001)),
                (3, scan(150, 0.7, false, 0.0)),
            ],
        },
        ScanDoc {
            path: "invoices/inv-2025-0117-scan.pdf",
            title: "Tax Invoice INV-2025-0117 (scanned)",
            source: "invoices/inv-2025-0117.pdf",
            pages: vec![(1, scan(300, 0.0, true, 0.0))],
        },
        ScanDoc {
            path: "invoices/inv-2025-0164-scan.pdf",
            title: "Tax Invoice INV-2025-0164 (scanned)",
            source: "invoices/inv-2025-0164.pdf",
            pages: vec![
                (1, scan(200, 1.0, false, 0.0)),
                (2, scan(150, 0.0, false, 0.0)),
            ],
        },
        ScanDoc {
            path: "invoices/inv-ks-8842-scan.pdf",
            title: "Invoice KS-8842 (scanned)",
            source: "invoices/inv-ks-8842.pdf",
            pages: vec![(1, scan(300, -2.0, false, 0.001))],
        },
        ScanDoc {
            path: "contracts/nda-orchid-kestrel-p2-scan.png",
            title: "Mutual Non-Disclosure Agreement, page 2 (scanned)",
            source: "contracts/nda-orchid-kestrel.pdf",
            pages: vec![(2, scan(200, 0.0, true, 0.0005))],
        },
    ]
}

/// Text files of the synthetic corpus copied in as distractors.
const DISTRACTORS: [&str; 5] = [
    "research/chunking-ablation.md",
    "research/reranker-latency.md",
    "research/ocr-error-study.md",
    "policies/travel-policy.txt",
    "policies/hybrid-work-policy.txt",
];

/// Passages for the cases whose answer is a table cell (the synthetic
/// dataset gives them none): the cell's row as the text corpus writes it.
const ROW_PASSAGES: [(&str, &str); 4] = [
    (
        "inv117-design",
        "Data warehouse design sprint 1 3,60,000.00 3,60,000.00",
    ),
    (
        "inv164-hours",
        "Dashboard migration (hours) 40 2,500.00 1,00,000.00",
    ),
    (
        "inv164-incident",
        "Incident response, on-call (hours) 6 3,000.00 18,000.00",
    ),
    (
        "ks-unit-price",
        "Pipetting robot arm PR-200 2 7,200.00 14,400.00",
    ),
];

/// A grey page image, one byte per pixel (0 black, 255 white).
struct Gray {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

struct Fonts<'a> {
    regular: ttf_parser::Face<'a>,
    bold: ttf_parser::Face<'a>,
}

impl Fonts<'static> {
    fn load() -> Result<Self> {
        Ok(Self {
            regular: ttf_parser::Face::parse(REGULAR, 0).context("parsing Liberation Sans")?,
            bold: ttf_parser::Face::parse(BOLD, 0).context("parsing Liberation Sans Bold")?,
        })
    }
}

/// Glyph outlines into a tiny-skia path.
struct Outline(PathBuilder);

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// xorshift64*: a small seeded generator, so noise is the same on every run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Draws `page` as `scan` sees it: text at the page's positions, rotated
/// about the page centre by the skew, then speckled.
fn draw(page: &PdfPage, scan: &Scan, fonts: &Fonts, seed: u64) -> Result<Gray> {
    let px_per_pt = scan.dpi as f32 / 72.0;
    let width = (PAGE_WIDTH * px_per_pt).round() as u32;
    let height = (PAGE_HEIGHT * px_per_pt).round() as u32;
    let mut pixmap = Pixmap::new(width, height).context("allocating a page image")?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    // Page points (origin bottom left, y up) to pixels, then the skew.
    let page_to_px =
        Transform::from_rotate_at(scan.skew_deg, width as f32 / 2.0, height as f32 / 2.0)
            .pre_concat(Transform::from_row(
                px_per_pt,
                0.0,
                0.0,
                -px_per_pt,
                0.0,
                height as f32,
            ));
    for run in &page.runs {
        let face = if run.bold {
            &fonts.bold
        } else {
            &fonts.regular
        };
        let em = run.size / face.units_per_em() as f32;
        let mut pen = run.x;
        for c in run.text.chars() {
            let Some(glyph) = face.glyph_index(c) else {
                bail!("Liberation Sans has no glyph for {c:?}");
            };
            let mut outline = Outline(PathBuilder::new());
            if face.outline_glyph(glyph, &mut outline).is_some() {
                if let Some(path) = outline.0.finish() {
                    // Glyph units (y up) to page points at the pen position.
                    let glyph_to_page = Transform::from_row(em, 0.0, 0.0, em, pen, run.y);
                    pixmap.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        page_to_px.pre_concat(glyph_to_page),
                        None,
                    );
                }
            }
            pen += face.glyph_hor_advance(glyph).unwrap_or(0) as f32 * em;
        }
    }
    // Opaque black on white: every channel holds the grey level.
    let mut pixels: Vec<u8> = pixmap.data().chunks_exact(4).map(|p| p[0]).collect();
    let specks = (width as f32 * height as f32 * scan.speckle) as u64;
    let mut rng = Rng::new(seed);
    for _ in 0..specks {
        let x = rng.below(width as u64 - 1) as usize;
        let y = rng.below(height as u64 - 1) as usize;
        let level = rng.below(96) as u8;
        let i = y * width as usize + x;
        pixels[i] = pixels[i].min(level);
        // About half the specks are two pixels wide, a quarter two tall.
        if rng.below(2) == 0 {
            pixels[i + 1] = pixels[i + 1].min(level);
        }
        if rng.below(4) == 0 {
            pixels[i + width as usize] = pixels[i + width as usize].min(level);
        }
    }
    Ok(Gray {
        width,
        height,
        pixels,
    })
}

/// A page image for the PDF writer: grey bytes, or one bit per pixel
/// (darker than mid-grey is black) when bilevel.
fn page_image(gray: Gray, bilevel: bool) -> PageImage {
    if !bilevel {
        return PageImage {
            width: gray.width,
            height: gray.height,
            bilevel: false,
            data: gray.pixels,
        };
    }
    let row_bytes = (gray.width as usize).div_ceil(8);
    let mut data = vec![0u8; row_bytes * gray.height as usize];
    for (y, row) in gray.pixels.chunks_exact(gray.width as usize).enumerate() {
        let out = &mut data[y * row_bytes..(y + 1) * row_bytes];
        for (x, &v) in row.iter().enumerate() {
            if v >= 128 {
                out[x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    PageImage {
        width: gray.width,
        height: gray.height,
        bilevel: true,
        data,
    }
}

/// A PNG of one page: grey, or black and white stored as grey levels 0 and 255.
fn page_png(gray: Gray, bilevel: bool) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let pixels: Vec<u8> = if bilevel {
        gray.pixels
            .iter()
            .map(|&v| if v >= 128 { 255 } else { 0 })
            .collect()
    } else {
        gray.pixels
    };
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        &pixels,
        gray.width,
        gray.height,
        image::ExtendedColorType::L8,
    )
    .context("encoding a scanned page as PNG")?;
    Ok(png)
}

/// The scanned corpus (files with their text by page, and the dataset).
pub fn build() -> Result<SynthCorpus> {
    let synth = crate::synth::documents();
    let source = |path: &str| -> Result<&SynthFile> {
        synth
            .iter()
            .find(|f| f.path == path)
            .with_context(|| format!("the synthetic corpus has no {path}"))
    };
    let fonts = Fonts::load()?;
    let mut files = Vec::new();
    // Synthetic (file, page) -> (scanned file, its page; None for an image).
    let mut moved: BTreeMap<(String, u32), (String, Option<u32>)> = BTreeMap::new();
    for (doc_index, doc) in scan_docs().iter().enumerate() {
        let src = source(doc.source)?;
        let mut grays = Vec::new();
        let mut texts = Vec::new();
        for (page_index, (page, scan)) in doc.pages.iter().enumerate() {
            let layout = src
                .pdf_pages
                .get(page - 1)
                .with_context(|| format!("{} has no page {page}", doc.source))?;
            let seed = ((doc_index as u64) << 16) | page_index as u64;
            grays.push((draw(layout, scan, &fonts, seed)?, scan.bilevel));
            texts.push(src.pages[page - 1].clone());
            let target_page = (!doc.path.ends_with(".png")).then_some(page_index as u32 + 1);
            moved.insert(
                (doc.source.to_string(), *page as u32),
                (doc.path.to_string(), target_page),
            );
        }
        let bytes = if doc.path.ends_with(".png") {
            let [(gray, bilevel)]: [(Gray, bool); 1] = grays
                .try_into()
                .map_err(|_| anyhow::anyhow!("{} must have exactly one page", doc.path))?;
            page_png(gray, bilevel)?
        } else {
            let images: Vec<PageImage> = grays.into_iter().map(|(g, b)| page_image(g, b)).collect();
            write_image_pdf(doc.title, &images)
        };
        files.push(SynthFile {
            path: doc.path.to_string(),
            bytes,
            pages: texts,
            pdf_pages: Vec::new(),
        });
    }
    for path in DISTRACTORS {
        files.push(source(path)?.clone());
    }
    Ok(SynthCorpus {
        files,
        dataset: Dataset {
            id: DATASET_ID.to_string(),
            description: "Synthetic contracts and invoices as image-only scans (150-300 dpi, \
                          skewed, speckled, three table pages) and text distractors (fictional; \
                          generated by `shodh-eval synth-scanned`)"
                .to_string(),
            cases: cases(&moved),
        },
    })
}

/// The synthetic dataset's cases whose every source page was scanned,
/// pointed at the scanned files; every answerable case has a passage. The
/// unanswerable cases are kept.
fn cases(moved: &BTreeMap<(String, u32), (String, Option<u32>)>) -> Vec<EvalCase> {
    let rows: BTreeMap<&str, &str> = ROW_PASSAGES.into_iter().collect();
    crate::synth::cases()
        .into_iter()
        .filter_map(|mut case| {
            if !case.answerable {
                return Some(case);
            }
            for source in &mut case.sources {
                let [page] = source.pages[..] else {
                    return None;
                };
                let (file, target) = moved.get(&(source.file.clone(), page))?;
                source.file = file.clone();
                source.pages = target.iter().copied().collect();
                if source.passage.is_none() {
                    source.passage = rows.get(case.id.as_str()).map(|p| p.to_string());
                }
            }
            Some(case)
        })
        .collect()
}

/// Each scanned file's text by page.
pub fn ground_truth(corpus: &SynthCorpus) -> BTreeMap<String, Vec<String>> {
    corpus
        .files
        .iter()
        .filter(|f| f.path.ends_with("-scan.pdf") || f.path.ends_with("-scan.png"))
        .map(|f| (f.path.clone(), f.pages.clone()))
        .collect()
}

/// Write the corpus, dataset and ground truth to `dir`.
pub fn write(corpus: &SynthCorpus, dir: &Path) -> Result<()> {
    corpus.write(dir)?;
    let truth = serde_json::to_string_pretty(&ground_truth(corpus))? + "\n";
    let path = dir.join("ground-truth.json");
    std::fs::write(&path, truth).with_context(|| format!("writing {}", path.display()))
}

/// Read a `ground-truth.json`.
pub fn read_ground_truth(path: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn corpus() -> &'static SynthCorpus {
        static CORPUS: OnceLock<SynthCorpus> = OnceLock::new();
        CORPUS.get_or_init(|| build().unwrap())
    }

    fn committed() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("scanned")
    }

    #[test]
    fn dataset_matches_the_page_text_and_every_case_has_a_passage() {
        let corpus = corpus();
        corpus.check().unwrap();
        let answerable: Vec<&EvalCase> = corpus
            .dataset
            .active_cases()
            .filter(|c| c.answerable)
            .collect();
        assert_eq!(answerable.len(), 29);
        for case in &answerable {
            for source in &case.sources {
                assert!(source.passage.is_some(), "{} has no passage", case.id);
                assert!(
                    source.file.contains("-scan."),
                    "{}: {}",
                    case.id,
                    source.file
                );
            }
        }
        // The image's case has no page: images are not paged.
        let nda = answerable.iter().find(|c| c.id == "nda-duration").unwrap();
        assert!(nda.sources[0].pages.is_empty());
    }

    #[test]
    fn scans_are_image_only_and_sized_by_their_resolution() {
        let corpus = corpus();
        for file in corpus.files.iter().filter(|f| f.path.ends_with(".pdf")) {
            let text = String::from_utf8_lossy(&file.bytes);
            assert!(!text.contains(" Tj"), "{} has a text layer", file.path);
            assert!(!text.contains("/Font"), "{} names a font", file.path);
            assert_eq!(text.matches("/Subtype /Image").count(), file.pages.len());
        }
        let ks = corpus
            .files
            .iter()
            .find(|f| f.path == "invoices/inv-ks-8842-scan.pdf")
            .unwrap();
        assert!(String::from_utf8_lossy(&ks.bytes).contains("/Width 2550 /Height 3300"));
        let png = corpus
            .files
            .iter()
            .find(|f| f.path.ends_with(".png"))
            .unwrap();
        let img = image::load_from_memory(&png.bytes).unwrap();
        assert_eq!((img.width(), img.height()), (1700, 2200));
    }

    #[test]
    fn generation_is_deterministic() {
        let again = build().unwrap();
        let first = corpus();
        assert_eq!(first.files.len(), again.files.len());
        for (x, y) in first.files.iter().zip(&again.files) {
            assert!(x.bytes == y.bytes, "{}", x.path);
        }
        assert_eq!(first.dataset, again.dataset);
    }

    #[test]
    fn committed_corpus_is_the_generator_output() {
        let corpus = corpus();
        let root = committed();
        for file in &corpus.files {
            let path = file
                .path
                .split('/')
                .fold(root.join("corpus"), |p, s| p.join(s));
            let on_disk =
                std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(
                on_disk == file.bytes,
                "{} differs from the generator; run `shodh-eval synth-scanned --out crates/shodh-eval/data/scanned`",
                file.path
            );
        }
        let listed = crate::corpus::supported_files(&root.join("corpus")).unwrap();
        let mut produced: Vec<String> = corpus.files.iter().map(|f| f.path.clone()).collect();
        produced.sort();
        assert_eq!(
            listed, produced,
            "the committed corpus has extra or missing files"
        );
        let dataset = std::fs::read_to_string(root.join("dataset.json")).unwrap();
        assert_eq!(dataset, corpus.dataset.to_json().unwrap());
        assert_eq!(
            read_ground_truth(&root.join("ground-truth.json")).unwrap(),
            ground_truth(corpus)
        );
    }

    #[test]
    fn noise_is_seeded() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        let xs: Vec<u64> = (0..5).map(|_| a.below(1000)).collect();
        let ys: Vec<u64> = (0..5).map(|_| b.below(1000)).collect();
        assert_eq!(xs, ys);
        assert_ne!(Rng::new(8).below(1_000_000), Rng::new(7).below(1_000_000));
    }
}
