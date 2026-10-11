//! Text detection pre- and post-processing for the PP-OCRv6 DB detector, as
//! PaddleOCR specifies it in the model's `inference.yml` (`DetResizeForTest`,
//! `NormalizeImage`, `DBPostProcess`).
//!
//! PaddleOCR feeds the detector the whole page at its native size (rounded to
//! multiples of 32, the longer side capped at 4000 px). The detector's memory
//! grows with its input's pixel count (over a gigabyte for a 300 dpi page), so
//! here a tall page is detected in overlapping full-width bands of at most
//! [`DetParams::band_pixels`] pixels ([`bands`]) and each line is kept from
//! the band whose core holds its centre: the same native resolution, with
//! memory bounded by the band. Scaling the page down instead was measured to
//! lose lines on speckled scans (`shodh-eval ocr`).

use image::RgbImage;

use super::geometry::{min_area_rect, Point, Quad};

/// `NormalizeImage` mean and standard deviation, applied to the channels in
/// the image's (BGR) order.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// `DBPostProcess.min_size`: the shortest side a candidate's rectangle may have.
const MIN_SIZE: f32 = 3.0;

/// Detector settings: the model's `PostProcess` values and the input cap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetParams {
    /// Probability above which a pixel is text (`thresh`).
    pub thresh: f32,
    /// Minimum mean probability inside a candidate box (`box_thresh`).
    pub box_thresh: f32,
    /// How far a box is grown, relative to its area over its perimeter
    /// (`unclip_ratio`).
    pub unclip_ratio: f32,
    /// At most this many candidates are scored (`max_candidates`).
    pub max_candidates: usize,
    /// Most pixels the detector sees at once (see [`bands`]).
    pub band_pixels: u32,
}

/// Widest detector input (PaddleOCR's `max_side_limit`); wider pages are
/// scaled down to it.
pub const MAX_WIDTH: u32 = 4000;

/// Rows two neighbouring bands share. A line up to this tall (about 37 pt
/// at 300 dpi) lies wholly inside the band whose core holds its centre.
pub const BAND_OVERLAP: u32 = 160;

/// A horizontal band of the page the detector reads at once: rows
/// `top..top + height`, owning the lines whose centre lies in
/// `core_top..core_bottom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub top: u32,
    pub height: u32,
    pub core_top: f32,
    pub core_bottom: f32,
}

impl Band {
    /// Whether a line centred at page row `y` belongs to this band.
    pub fn owns(&self, y: f32) -> bool {
        y >= self.core_top && y < self.core_bottom
    }
}

/// The bands of a `width` x `height` page with at most `band_pixels` pixels
/// each (but at least three overlaps tall), overlapping by [`BAND_OVERLAP`]
/// rows, their cores tiling the page without gaps. A page that fits is one band.
pub fn bands(width: u32, height: u32, band_pixels: u32) -> Vec<Band> {
    let o = BAND_OVERLAP;
    let band_h = ((band_pixels / width.max(1)) / 32 * 32).max(3 * o);
    if height <= band_h {
        return vec![Band {
            top: 0,
            height,
            core_top: f32::NEG_INFINITY,
            core_bottom: f32::INFINITY,
        }];
    }
    let step = band_h - o;
    let mut out: Vec<Band> = Vec::new();
    let mut top = 0u32;
    loop {
        let last = top + band_h >= height;
        let top_here = if last { height - band_h } else { top };
        out.push(Band {
            top: top_here,
            height: band_h,
            core_top: out
                .last()
                .map_or(f32::NEG_INFINITY, |prev: &Band| prev.core_bottom),
            core_bottom: if last {
                f32::INFINITY
            } else {
                (top + band_h - o / 2) as f32
            },
        });
        if last {
            break;
        }
        top += step;
    }
    out
}

/// One detected text line in page pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetBox {
    pub quad: Quad,
    pub score: f32,
}

/// The detector input for `img`: both sides rounded to a multiple of 32
/// (`DetResizeForTest`), as NCHW floats in BGR order normalised with
/// [`MEAN`] and [`STD`]. Returns the tensor data with its width and height.
pub fn prepare(img: &RgbImage) -> (Vec<f32>, u32, u32) {
    let (w, h) = img.dimensions();
    let (rw, rh) = input_size(w, h);
    let resized;
    let src = if (rw, rh) == (w, h) {
        img
    } else {
        resized = image::imageops::resize(img, rw, rh, image::imageops::FilterType::Triangle);
        &resized
    };
    let n = (rw * rh) as usize;
    let mut data = vec![0f32; 3 * n];
    for (i, px) in src.pixels().enumerate() {
        // BGR planes: plane 0 is blue.
        for (plane, channel) in [2usize, 1, 0].into_iter().enumerate() {
            data[plane * n + i] = (px.0[channel] as f32 / 255.0 - MEAN[plane]) / STD[plane];
        }
    }
    (data, rw, rh)
}

/// The detector input size for a `w` x `h` image (see [`prepare`]).
pub fn input_size(w: u32, h: u32) -> (u32, u32) {
    let round32 = |v: u32| (((v as f32 / 32.0).round() as u32) * 32).max(32);
    (round32(w), round32(h))
}

/// The pixels of each 8-connected blob of `mask` (`width` x `height`, row
/// major) that touch the background or the border: their hull is the blob's.
fn blob_outlines(mask: &[bool], width: usize, height: usize) -> Vec<Vec<Point>> {
    let mut seen = vec![false; mask.len()];
    let mut blobs = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut outline = Vec::new();
        while let Some(i) = stack.pop() {
            let (x, y) = (i % width, i / width);
            let mut edge = x == 0 || y == 0 || x + 1 == width || y + 1 == height;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                        continue;
                    }
                    let j = ny as usize * width + nx as usize;
                    if !mask[j] {
                        if dx == 0 || dy == 0 {
                            edge = true;
                        }
                        continue;
                    }
                    if !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            if edge {
                outline.push(Point::new(x as f32, y as f32));
            }
        }
        blobs.push(outline);
    }
    blobs
}

/// `box_score_fast`: the mean probability over the pixels of the quad's
/// bounding box whose centres lie inside the quad.
fn box_score(prob: &[f32], width: usize, height: usize, quad: &Quad) -> f32 {
    let (l, t, r, b) = quad.bounds();
    let clamp = |v: f32, max: usize| (v.max(0.0) as usize).min(max - 1);
    let (x0, x1) = (clamp(l.floor(), width), clamp(r.ceil(), width));
    let (y0, y1) = (clamp(t.floor(), height), clamp(b.ceil(), height));
    let (mut sum, mut n) = (0.0f64, 0usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if quad.contains(Point::new(x as f32, y as f32)) {
                sum += prob[y * width + x] as f64;
                n += 1;
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        (sum / n as f64) as f32
    }
}

/// `DBPostProcess` on the probability map `prob` (`width` x `height`, the
/// detector input's size): text pixels, one minimum-area rectangle per blob,
/// blobs scored and the weak ones dropped, rectangles grown by
/// `area * unclip_ratio / perimeter` on every side (what PaddleOCR's polygon
/// offset with round joins followed by `minAreaRect` amounts to for a
/// rectangle), scaled to the page's `dest` size and clipped to it. Boxes
/// narrower or shorter than 4 px are dropped (`filter_tag_det_res`).
pub fn postprocess(
    prob: &[f32],
    width: usize,
    height: usize,
    dest: (u32, u32),
    params: &DetParams,
) -> Vec<DetBox> {
    if width == 0 || height == 0 || prob.len() != width * height {
        return Vec::new();
    }
    let mask: Vec<bool> = prob.iter().map(|&p| p > params.thresh).collect();
    let (sx, sy) = (dest.0 as f32 / width as f32, dest.1 as f32 / height as f32);
    let (dw, dh) = (dest.0 as f32, dest.1 as f32);
    let mut boxes = Vec::new();
    for outline in blob_outlines(&mask, width, height)
        .into_iter()
        .take(params.max_candidates)
    {
        let Some(rect) = min_area_rect(&outline) else {
            continue;
        };
        if rect.short_side() < MIN_SIZE {
            continue;
        }
        let score = box_score(prob, width, height, &rect.quad());
        if score < params.box_thresh {
            continue;
        }
        let quad = rect.quad();
        let perimeter = quad.perimeter();
        if perimeter <= f32::EPSILON {
            continue;
        }
        let grown = rect.expanded(quad.area() * params.unclip_ratio / perimeter);
        if grown.short_side() < MIN_SIZE + 2.0 {
            continue;
        }
        let scaled = Quad(grown.quad().0.map(|p| {
            Point::new(
                (p.x * sx).round().clamp(0.0, dw),
                (p.y * sy).round().clamp(0.0, dh),
            )
        }));
        let [p0, p1, _, p3] = scaled.0;
        let edge = |a: Point, b: Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
        if edge(p0, p1) as u32 <= 3 || edge(p0, p3) as u32 <= 3 {
            continue;
        }
        boxes.push(DetBox {
            quad: scaled,
            score,
        });
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: DetParams = DetParams {
        thresh: 0.2,
        box_thresh: 0.45,
        unclip_ratio: 1.4,
        max_candidates: 3000,
        band_pixels: 2_000_000,
    };

    #[test]
    fn input_is_rounded_to_multiples_of_32() {
        assert_eq!(input_size(2550, 770), (2560, 768));
        assert_eq!(input_size(1000, 700), (992, 704));
        assert_eq!(input_size(10, 10), (32, 32));
    }

    #[test]
    fn bands_overlap_and_their_cores_tile_the_page() {
        // A Letter page at 300 dpi in 2-megapixel bands: 768 rows each.
        let bands = bands(2550, 3300, 2_000_000);
        assert!(bands.len() > 1);
        assert!(bands.iter().all(|b| b.height == 768));
        assert_eq!(bands[0].top, 0);
        assert_eq!(bands.last().map(|b| b.top + b.height), Some(3300));
        assert_eq!(bands[0].core_top, f32::NEG_INFINITY);
        assert_eq!(bands.last().map(|b| b.core_bottom), Some(f32::INFINITY));
        for pair in bands.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert_eq!(a.core_bottom, b.core_top, "cores meet");
            assert!(a.top + a.height >= b.top + BAND_OVERLAP, "bands overlap");
        }
        // Every row is owned by exactly one band, and a line up to the overlap
        // tall centred anywhere lies inside the band that owns it.
        let half = BAND_OVERLAP as f32 / 2.0;
        for y in (0..3300).map(|y| y as f32 + 0.5) {
            let owners: Vec<&Band> = bands.iter().filter(|b| b.owns(y)).collect();
            assert_eq!(owners.len(), 1, "row {y}");
            let b = owners[0];
            let (top, bottom) = ((y - half).max(0.0), (y + half).min(3300.0));
            assert!(
                top >= b.top as f32 && bottom <= (b.top + b.height) as f32,
                "row {y}"
            );
        }
        // A page that fits is one band owning everything.
        let one = super::bands(1700, 1100, 2_000_000);
        assert_eq!(one.len(), 1);
        assert!(one[0].owns(-1.0) && one[0].owns(1e9));
    }

    #[test]
    fn input_is_bgr_and_normalised() {
        let img = RgbImage::from_pixel(32, 32, image::Rgb([255, 0, 0]));
        let (data, w, h) = prepare(&img);
        assert_eq!((w, h), (32, 32));
        let n = 32 * 32;
        // Blue plane first: blue is 0.
        assert!((data[0] - (0.0 - 0.485) / 0.229).abs() < 1e-5);
        assert!((data[n] - (0.0 - 0.456) / 0.224).abs() < 1e-5);
        // Red plane last: red is 255.
        assert!((data[2 * n] - (1.0 - 0.406) / 0.225).abs() < 1e-5);
    }

    fn map_with(
        width: usize,
        height: usize,
        rects: &[(usize, usize, usize, usize, f32)],
    ) -> Vec<f32> {
        let mut prob = vec![0.0f32; width * height];
        for &(x0, y0, x1, y1, p) in rects {
            for y in y0..y1 {
                for x in x0..x1 {
                    prob[y * width + x] = p;
                }
            }
        }
        prob
    }

    #[test]
    fn boxes_are_grown_scaled_and_weak_or_tiny_blobs_dropped() {
        // A strong 60 x 10 line, a weak line, and a 2-pixel speck.
        let prob = map_with(
            100,
            50,
            &[
                (10, 10, 70, 20, 0.9),
                (10, 30, 70, 40, 0.3),
                (90, 45, 92, 47, 0.95),
            ],
        );
        let boxes = postprocess(&prob, 100, 50, (200, 100), &PARAMS);
        assert_eq!(boxes.len(), 1, "{boxes:?}");
        let b = boxes[0];
        assert!((b.score - 0.9).abs() < 1e-4);
        // The blob spans pixel centres 10..=69 x 10..=19 (59 x 9); grown by
        // 59 * 9 * 1.4 / 136 = 5.47 px per side, then doubled.
        let (l, t, r, bottom) = b.quad.bounds();
        assert_eq!((l, t), (9.0, 9.0));
        assert_eq!((r, bottom), (149.0, 49.0));
    }

    #[test]
    fn a_tilted_line_keeps_its_angle() {
        let (w, h) = (300usize, 100usize);
        let mut prob = vec![0.0f32; w * h];
        let slope = 2f32.to_radians().tan();
        for x in 20..280 {
            let y0 = 40.0 + (x as f32 - 20.0) * slope;
            for y in y0 as usize..y0 as usize + 8 {
                prob[y * w + x] = 0.8;
            }
        }
        let boxes = postprocess(&prob, w, h, (w as u32, h as u32), &PARAMS);
        assert_eq!(boxes.len(), 1);
        let angle = boxes[0].quad.angle().to_degrees();
        assert!((angle - 2.0).abs() < 0.3, "angle {angle}");
        assert!(boxes[0].quad.height() < 20.0, "{:?}", boxes[0].quad);
    }

    #[test]
    fn separate_blobs_give_separate_boxes() {
        let prob = map_with(
            100,
            60,
            &[
                (5, 5, 40, 15, 0.9),
                (50, 5, 95, 15, 0.9),
                (5, 35, 95, 45, 0.9),
            ],
        );
        let boxes = postprocess(&prob, 100, 60, (100, 60), &PARAMS);
        assert_eq!(boxes.len(), 3);
    }
}
