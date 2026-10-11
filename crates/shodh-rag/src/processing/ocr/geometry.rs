//! Plane geometry for the text detector's post-processing: convex hulls,
//! minimum-area rectangles (what OpenCV's `minAreaRect` computes), point-in-quad
//! tests and the crop of a rotated text line out of a page image.

use image::RgbImage;

/// A point in image pixels (`x` right, `y` down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }

    fn dist(self, o: Point) -> f32 {
        let d = self.sub(o);
        (d.x * d.x + d.y * d.y).sqrt()
    }
}

fn cross(o: Point, a: Point, b: Point) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// A text line's box: four corners in the order top-left, top-right,
/// bottom-right, bottom-left (as PaddleOCR's `get_mini_boxes` orders them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad(pub [Point; 4]);

impl Quad {
    /// Length of the top edge, or the bottom one if longer.
    pub fn width(&self) -> f32 {
        let p = &self.0;
        p[0].dist(p[1]).max(p[3].dist(p[2]))
    }

    /// Length of the left edge, or the right one if longer.
    pub fn height(&self) -> f32 {
        let p = &self.0;
        p[0].dist(p[3]).max(p[1].dist(p[2]))
    }

    pub fn center(&self) -> Point {
        let p = &self.0;
        Point::new(
            (p[0].x + p[1].x + p[2].x + p[3].x) / 4.0,
            (p[0].y + p[1].y + p[2].y + p[3].y) / 4.0,
        )
    }

    /// Angle of the top edge in radians (positive: descending to the right).
    pub fn angle(&self) -> f32 {
        let d = self.0[1].sub(self.0[0]);
        d.y.atan2(d.x)
    }

    /// Axis-aligned bounds `(left, top, right, bottom)`.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let xs = self.0.map(|p| p.x);
        let ys = self.0.map(|p| p.y);
        (
            xs.iter().copied().fold(f32::INFINITY, f32::min),
            ys.iter().copied().fold(f32::INFINITY, f32::min),
            xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        )
    }

    /// Area by the shoelace formula.
    pub fn area(&self) -> f32 {
        let p = &self.0;
        let mut twice = 0.0;
        for i in 0..4 {
            let (a, b) = (p[i], p[(i + 1) % 4]);
            twice += a.x * b.y - b.x * a.y;
        }
        twice.abs() / 2.0
    }

    pub fn perimeter(&self) -> f32 {
        let p = &self.0;
        (0..4).map(|i| p[i].dist(p[(i + 1) % 4])).sum()
    }

    /// Whether `pt` lies inside the (convex) quad or on its edge.
    pub fn contains(&self, pt: Point) -> bool {
        let p = &self.0;
        let mut sign = 0.0f32;
        for i in 0..4 {
            let c = cross(p[i], p[(i + 1) % 4], pt);
            if c.abs() <= f32::EPSILON {
                continue;
            }
            if sign == 0.0 {
                sign = c.signum();
            } else if c.signum() != sign {
                return false;
            }
        }
        true
    }

    /// The quad with every coordinate multiplied by `sx`, `sy`.
    pub fn scaled(&self, sx: f32, sy: f32) -> Quad {
        Quad(self.0.map(|p| Point::new(p.x * sx, p.y * sy)))
    }
}

/// Convex hull (Andrew's monotone chain), counter-clockwise in image
/// coordinates, without collinear points.
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut pts: Vec<Point> = points.to_vec();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<Point> = Vec::with_capacity(pts.len());
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::with_capacity(pts.len());
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A rotated rectangle: centre, side lengths along `axis` (unit vector) and
/// its normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotatedRect {
    pub center: Point,
    /// Unit vector of the first side.
    pub axis: Point,
    /// Length along `axis`.
    pub len_a: f32,
    /// Length along the normal of `axis`.
    pub len_b: f32,
}

impl RotatedRect {
    pub fn short_side(&self) -> f32 {
        self.len_a.min(self.len_b)
    }

    /// The rectangle grown by `d` on every side.
    pub fn expanded(&self, d: f32) -> RotatedRect {
        RotatedRect {
            len_a: self.len_a + 2.0 * d,
            len_b: self.len_b + 2.0 * d,
            ..*self
        }
    }

    /// The corners ordered as PaddleOCR's `get_mini_boxes` orders
    /// `cv2.boxPoints`: sorted by `x`, the left pair split into top-left and
    /// bottom-left by `y`, the right pair into top-right and bottom-right.
    pub fn quad(&self) -> Quad {
        let (u, n) = (self.axis, Point::new(-self.axis.y, self.axis.x));
        let (ha, hb) = (self.len_a / 2.0, self.len_b / 2.0);
        let c = self.center;
        let corner = |sa: f32, sb: f32| {
            Point::new(
                c.x + u.x * ha * sa + n.x * hb * sb,
                c.y + u.y * ha * sa + n.y * hb * sb,
            )
        };
        let mut pts = [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ];
        pts.sort_by(|a, b| a.x.total_cmp(&b.x));
        let (tl, bl) = if pts[1].y > pts[0].y {
            (pts[0], pts[1])
        } else {
            (pts[1], pts[0])
        };
        let (tr, br) = if pts[3].y > pts[2].y {
            (pts[2], pts[3])
        } else {
            (pts[3], pts[2])
        };
        Quad([tl, tr, br, bl])
    }
}

/// The minimum-area enclosing rectangle of `points` (rotating calipers over
/// the convex hull). `None` for an empty set.
pub fn min_area_rect(points: &[Point]) -> Option<RotatedRect> {
    let hull = convex_hull(points);
    match hull.len() {
        0 => return None,
        1 => {
            return Some(RotatedRect {
                center: hull[0],
                axis: Point::new(1.0, 0.0),
                len_a: 0.0,
                len_b: 0.0,
            })
        }
        _ => {}
    }
    let mut best: Option<(f32, RotatedRect)> = None;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let edge = b.sub(a);
        let len = (edge.x * edge.x + edge.y * edge.y).sqrt();
        if len <= f32::EPSILON {
            continue;
        }
        let u = Point::new(edge.x / len, edge.y / len);
        let n = Point::new(-u.y, u.x);
        let (mut min_u, mut max_u, mut min_n, mut max_n) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for p in &hull {
            let d = p.sub(a);
            let pu = d.x * u.x + d.y * u.y;
            let pn = d.x * n.x + d.y * n.y;
            min_u = min_u.min(pu);
            max_u = max_u.max(pu);
            min_n = min_n.min(pn);
            max_n = max_n.max(pn);
        }
        let area = (max_u - min_u) * (max_n - min_n);
        if best.as_ref().is_none_or(|(a0, _)| area < *a0) {
            let cu = (min_u + max_u) / 2.0;
            let cn = (min_n + max_n) / 2.0;
            best = Some((
                area,
                RotatedRect {
                    center: Point::new(a.x + u.x * cu + n.x * cn, a.y + u.y * cu + n.y * cn),
                    axis: u,
                    len_a: max_u - min_u,
                    len_b: max_n - min_n,
                },
            ));
        }
    }
    best.map(|(_, r)| r)
}

fn sample_bilinear(img: &RgbImage, x: f32, y: f32) -> [u8; 3] {
    // Border replicate, as `cv2.BORDER_REPLICATE`.
    let (w, h) = (img.width() as i64, img.height() as i64);
    let xf = x.floor();
    let yf = y.floor();
    let (fx, fy) = (x - xf, y - yf);
    let clamp = |v: i64, max: i64| v.clamp(0, max - 1) as u32;
    let (x0, y0) = (xf as i64, yf as i64);
    let p = |xx: i64, yy: i64| img.get_pixel(clamp(xx, w), clamp(yy, h)).0;
    let (a, b, c, d) = (p(x0, y0), p(x0 + 1, y0), p(x0, y0 + 1), p(x0 + 1, y0 + 1));
    let mut out = [0u8; 3];
    for k in 0..3 {
        let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
        let bottom = c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
        out[k] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// The text line inside `quad`, straightened: PaddleOCR's
/// `get_rotate_crop_image`. The crop is `max(top, bottom)` wide and
/// `max(left, right)` tall (rounded to whole pixels, where the original
/// truncates: a 39.9999 px edge is 40 px); a crop at least 1.5
/// times taller than wide is turned 90 degrees counter-clockwise (vertical
/// text). A detected quad is a rectangle, so the perspective transform of
/// the original reduces to the affine map used here. `None` for a degenerate
/// quad.
pub fn crop_quad(img: &RgbImage, quad: &Quad) -> Option<RgbImage> {
    let w = quad.width().round() as u32;
    let h = quad.height().round() as u32;
    if w == 0 || h == 0 {
        return None;
    }
    let [p0, p1, _, p3] = quad.0;
    let ux = Point::new((p1.x - p0.x) / w as f32, (p1.y - p0.y) / w as f32);
    let vy = Point::new((p3.x - p0.x) / h as f32, (p3.y - p0.y) / h as f32);
    let mut out = RgbImage::new(w, h);
    for v in 0..h {
        for u in 0..w {
            // Pixel centres map to pixel centres, as cv2's warp does.
            let (fu, fv) = (u as f32 + 0.5, v as f32 + 0.5);
            let x = p0.x + ux.x * fu + vy.x * fv - 0.5;
            let y = p0.y + ux.y * fu + vy.y * fv - 0.5;
            out.put_pixel(u, v, image::Rgb(sample_bilinear(img, x, y)));
        }
    }
    if h as f32 / w as f32 >= 1.5 {
        Some(image::imageops::rotate270(&out))
    } else {
        Some(out)
    }
}

/// Bilinear resize (OpenCV's `INTER_LINEAR` sampling: pixel centres aligned,
/// no area filtering), as PaddleOCR resizes recognition crops.
pub fn resize_bilinear(img: &RgbImage, w: u32, h: u32) -> RgbImage {
    let (sw, sh) = (img.width() as f32, img.height() as f32);
    let (sx, sy) = (sw / w as f32, sh / h as f32);
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            out.put_pixel(x, y, image::Rgb(sample_bilinear(img, fx, fy)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn hull_drops_interior_and_collinear_points() {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(4.0, 0.0),
            Point::new(4.0, 3.0),
            Point::new(0.0, 3.0),
            Point::new(1.0, 1.0),
        ];
        let hull = convex_hull(&pts);
        assert_eq!(hull.len(), 4);
        assert!(!hull.contains(&Point::new(2.0, 0.0)));
        assert!(!hull.contains(&Point::new(1.0, 1.0)));
    }

    #[test]
    fn min_area_rect_of_an_axis_aligned_block() {
        let pts: Vec<Point> = (0..=10)
            .flat_map(|x| (0..=2).map(move |y| Point::new(x as f32, y as f32)))
            .collect();
        let r = min_area_rect(&pts).unwrap();
        assert!(close(r.len_a.max(r.len_b), 10.0));
        assert!(close(r.short_side(), 2.0));
        let q = r.quad();
        assert_eq!(q.0[0], Point::new(0.0, 0.0));
        assert_eq!(q.0[1], Point::new(10.0, 0.0));
        assert_eq!(q.0[2], Point::new(10.0, 2.0));
        assert_eq!(q.0[3], Point::new(0.0, 2.0));
    }

    #[test]
    fn min_area_rect_follows_a_tilted_line() {
        // A 100 x 10 bar rotated by 2 degrees.
        let (s, c) = 2f32.to_radians().sin_cos();
        let pts: Vec<Point> = (0..=100)
            .flat_map(|x| (0..=10).map(move |y| (x as f32, y as f32)))
            .map(|(x, y)| Point::new(x * c - y * s, x * s + y * c))
            .collect();
        let r = min_area_rect(&pts).unwrap();
        assert!(close(r.len_a.max(r.len_b), 100.0), "{r:?}");
        assert!(close(r.short_side(), 10.0), "{r:?}");
        let q = r.quad();
        assert!((q.angle().to_degrees() - 2.0).abs() < 0.01, "{}", q.angle());
        let expanded = r.expanded(3.0);
        assert!(close(expanded.short_side(), 16.0));
    }

    #[test]
    fn quad_measures_and_contains() {
        let q = Quad([
            Point::new(0.0, 0.0),
            Point::new(4.0, 0.0),
            Point::new(4.0, 2.0),
            Point::new(0.0, 2.0),
        ]);
        assert!(close(q.area(), 8.0));
        assert!(close(q.perimeter(), 12.0));
        assert!(q.contains(Point::new(2.0, 1.0)));
        assert!(q.contains(Point::new(0.0, 0.0)));
        assert!(!q.contains(Point::new(5.0, 1.0)));
        assert_eq!(q.center(), Point::new(2.0, 1.0));
    }

    #[test]
    fn crop_straightens_a_rotated_line_and_turns_vertical_ones() {
        // A dark 40 x 8 bar on white, rotated by 10 degrees about (50, 50).
        let mut img = RgbImage::from_pixel(100, 100, image::Rgb([255, 255, 255]));
        let (s, c) = 10f32.to_radians().sin_cos();
        let rot = |x: f32, y: f32| Point::new(50.0 + x * c - y * s, 50.0 + x * s + y * c);
        for py in 0..100 {
            for px in 0..100 {
                // Inverse rotation of the pixel centre.
                let (dx, dy) = (px as f32 + 0.5 - 50.0, py as f32 + 0.5 - 50.0);
                let (x, y) = (dx * c + dy * s, -dx * s + dy * c);
                if x.abs() <= 20.0 && y.abs() <= 4.0 {
                    img.put_pixel(px, py, image::Rgb([0, 0, 0]));
                }
            }
        }
        let quad = Quad([
            rot(-20.0, -4.0),
            rot(20.0, -4.0),
            rot(20.0, 4.0),
            rot(-20.0, 4.0),
        ]);
        let crop = crop_quad(&img, &quad).unwrap();
        assert_eq!((crop.width(), crop.height()), (40, 8));
        let dark = crop.pixels().filter(|p| p.0[0] < 128).count();
        assert!(dark as f32 > 0.9 * 320.0, "dark pixels {dark}");

        let tall = Quad([
            Point::new(10.0, 10.0),
            Point::new(14.0, 10.0),
            Point::new(14.0, 30.0),
            Point::new(10.0, 30.0),
        ]);
        let turned = crop_quad(&img, &tall).unwrap();
        assert_eq!((turned.width(), turned.height()), (20, 4));
    }

    #[test]
    fn resize_keeps_flat_colour() {
        let img = RgbImage::from_pixel(30, 10, image::Rgb([7, 8, 9]));
        let out = resize_bilinear(&img, 144, 48);
        assert_eq!((out.width(), out.height()), (144, 48));
        assert!(out.pixels().all(|p| p.0 == [7, 8, 9]));
    }
}
