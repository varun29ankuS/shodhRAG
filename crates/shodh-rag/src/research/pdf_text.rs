//! Text inside a rectangle of a PDF page.
//!
//! The rule is shared with the viewer (`app/src/features/research/snippetGeometry.ts`,
//! `textInBox`), so a snippet the agent creates and one the user drags read the same text:
//! 1. a text box is inside when at least half of its area lies within the rectangle (a box
//!    without area when its centre does);
//! 2. boxes are visited top to bottom (highest top edge first), then left to right; each
//!    joins the first line whose box overlaps it vertically by more than half the smaller
//!    height, else starts a new line;
//! 3. lines are ordered by top edge, then left edge; boxes in a line left to right; each
//!    box's text has whitespace collapsed and is trimmed (empty ones are dropped); boxes
//!    are joined by one space, lines by `\n`.
//!
//! Rectangles are given in PDF points from the top-left corner of the page box, y growing
//! downwards (the research ontology's `snippetRect`); text boxes are in PDF user space
//! (bottom-left origin).

use serde::{Deserialize, Serialize};

use super::{ResearchError, ResearchResult};
use crate::processing::document_model::BBox;

/// A rectangle in PDF points from the top-left corner of the page box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl PageRect {
    /// Checks the rectangle is finite, non-negative and not degenerate.
    pub fn validate(&self) -> ResearchResult<()> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(ResearchError::Invalid(
                "The rectangle must have finite, non-negative coordinates.".to_string(),
            ));
        }
        if self.width < 1.0 || self.height < 1.0 {
            return Err(ResearchError::Invalid(
                "The rectangle is too small (at least 1 × 1 point).".to_string(),
            ));
        }
        Ok(())
    }

    /// The same rectangle in PDF user space (bottom-left origin) for a page box
    /// `(x0, y0, x1, y1)`.
    pub fn to_user_space(&self, page_box: (f32, f32, f32, f32)) -> BBox {
        let (bx0, _by0, _bx1, by1) = page_box;
        BBox::new(
            bx0 + self.x,
            by1 - self.y - self.height,
            bx0 + self.x + self.width,
            by1 - self.y,
        )
    }

    /// The ontology's `snippetRect` text: `x,y,width,height` with up to two decimals.
    pub fn to_property(&self) -> String {
        [self.x, self.y, self.width, self.height]
            .iter()
            .map(|v| format_points(*v))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Parses `x,y,width,height`.
    pub fn from_property(text: &str) -> Option<Self> {
        let parts: Vec<f32> = text
            .split(',')
            .map(|p| p.trim().parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        match parts.as_slice() {
            [x, y, width, height] => Some(Self {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
            }),
            _ => None,
        }
    }
}

/// A number of points with at most two decimals and no trailing zeros.
pub fn format_points(value: f32) -> String {
    let rounded = (f64::from(value) * 100.0).round() / 100.0;
    let text = format!("{rounded:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// A piece of text with its box in PDF user space.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
    pub text: String,
    pub bbox: BBox,
}

/// The text of a region and the page size it was read from.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionText {
    pub text: String,
    pub page_width: f32,
    pub page_height: f32,
}

fn area(b: &BBox) -> f32 {
    b.width().max(0.0) * b.height().max(0.0)
}

fn intersection(a: &BBox, b: &BBox) -> f32 {
    let w = a.x1.min(b.x1) - a.x0.max(b.x0);
    let h = a.y1.min(b.y1) - a.y0.max(b.y0);
    if w <= 0.0 || h <= 0.0 {
        0.0
    } else {
        w * h
    }
}

/// Whether at least half of `item` lies inside `rect` (both in user space); a box
/// without area counts when its centre is inside.
pub fn is_inside(item: &BBox, rect: &BBox) -> bool {
    let total = area(item);
    if total <= 0.0 {
        return rect.contains_point(item.center_x(), item.center_y());
    }
    intersection(item, rect) >= 0.5 * total
}

/// The text of the boxes inside `rect` (user space), in reading order (see the module
/// documentation).
pub fn collect_text(boxes: &[TextBox], rect: &BBox) -> String {
    let mut inside: Vec<(String, BBox)> = boxes
        .iter()
        .map(|b| {
            (
                b.text.split_whitespace().collect::<Vec<_>>().join(" "),
                b.bbox,
            )
        })
        .filter(|(text, bbox)| !text.is_empty() && is_inside(bbox, rect))
        .collect();
    // Top to bottom (PDF y grows upwards), then left to right.
    inside.sort_by(|a, b| b.1.y1.total_cmp(&a.1.y1).then(a.1.x0.total_cmp(&b.1.x0)));
    let mut lines: Vec<(BBox, Vec<(String, BBox)>)> = Vec::new();
    for item in inside {
        let height = item.1.height();
        let line = lines.iter_mut().find(|(line, _)| {
            let overlap = line.y1.min(item.1.y1) - line.y0.max(item.1.y0);
            overlap > 0.5 * height.min(line.height())
        });
        match line {
            Some((line, items)) => {
                *line = line.union(&item.1);
                items.push(item);
            }
            None => lines.push((item.1, vec![item])),
        }
    }
    lines.sort_by(|a, b| b.0.y1.total_cmp(&a.0.y1).then(a.0.x0.total_cmp(&b.0.x0)));
    lines
        .into_iter()
        .map(|(_, mut items)| {
            items.sort_by(|a, b| a.1.x0.total_cmp(&b.1.x0));
            items
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text inside `rect` on the 1-based `page` of the PDF `bytes`, read from the PDF's text
/// spans. A panic inside the PDF library is reported as an error.
pub fn text_in_rect(bytes: Vec<u8>, page: u32, rect: PageRect) -> ResearchResult<RegionText> {
    rect.validate()?;
    let index = usize::try_from(page.max(1) - 1).unwrap_or(0);
    let run = move || -> ResearchResult<RegionText> {
        let doc = pdf_oxide::PdfDocument::from_bytes(bytes)
            .map_err(|e| ResearchError::Pdf(format!("The PDF could not be opened: {e}")))?;
        let pages = doc
            .page_count()
            .map_err(|e| ResearchError::Pdf(format!("The PDF could not be read: {e}")))?;
        if page == 0 || index >= pages {
            return Err(ResearchError::Invalid(format!(
                "Page {page} does not exist (the PDF has {pages} pages)."
            )));
        }
        let page_box = doc
            .get_page_media_box(index)
            .map_err(|e| ResearchError::Pdf(format!("Page {page} has no page box: {e}")))?;
        let (x0, y0, x1, y1) = page_box;
        let page_box = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
        let spans = doc
            .extract_spans(index)
            .map_err(|e| ResearchError::Pdf(format!("Page {page} text could not be read: {e}")))?;
        let boxes: Vec<TextBox> = spans
            .iter()
            .map(|s| TextBox {
                text: s.text.clone(),
                bbox: BBox::new(
                    s.bbox.x,
                    s.bbox.y,
                    s.bbox.x + s.bbox.width,
                    s.bbox.y + s.bbox.height,
                ),
            })
            .collect();
        let user = rect.to_user_space(page_box);
        Ok(RegionText {
            text: collect_text(&boxes, &user),
            page_width: page_box.2 - page_box.0,
            page_height: page_box.3 - page_box.1,
        })
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| {
        Err(ResearchError::Pdf(
            "The PDF library failed on this page.".to_string(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::pdf_fixtures::{build_pdf, text};

    fn tb(text: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> TextBox {
        TextBox {
            text: text.to_string(),
            bbox: BBox::new(x0, y0, x1, y1),
        }
    }

    #[test]
    fn half_the_area_decides_and_lines_read_in_order() {
        let boxes = vec![
            tb("world", 160.0, 700.0, 200.0, 710.0),
            tb("Hello", 100.0, 700.5, 150.0, 710.5),
            tb("second", 100.0, 686.0, 150.0, 696.0),
            // 40% inside: excluded.
            tb("edge", 190.0, 670.0, 240.0, 680.0),
            tb("outside", 300.0, 700.0, 350.0, 710.0),
        ];
        let rect = BBox::new(95.0, 660.0, 210.0, 720.0);
        assert_eq!(collect_text(&boxes, &rect), "Hello world\nsecond");
        assert!(is_inside(
            &BBox::new(0.0, 0.0, 10.0, 10.0),
            &BBox::new(5.0, 0.0, 20.0, 10.0)
        ));
        // A box without area counts by its centre; whitespace inside a box collapses.
        assert!(is_inside(&BBox::new(100.0, 700.0, 100.0, 710.0), &rect));
        let spaced = vec![
            tb("  two   words ", 100.0, 700.0, 150.0, 710.0),
            tb("   ", 160.0, 700.0, 170.0, 710.0),
        ];
        assert_eq!(collect_text(&spaced, &rect), "two words");
        assert!(!is_inside(
            &BBox::new(0.0, 0.0, 10.0, 10.0),
            &BBox::new(5.1, 0.0, 20.0, 10.0)
        ));
    }

    #[test]
    fn rectangles_convert_between_top_left_and_user_space() {
        let rect = PageRect {
            x: 72.0,
            y: 100.0,
            width: 200.0,
            height: 50.0,
        };
        let user = rect.to_user_space((0.0, 0.0, 612.0, 792.0));
        assert_eq!(user, BBox::new(72.0, 642.0, 272.0, 692.0));
        // A page box with an origin offset (crop box).
        let shifted = rect.to_user_space((10.0, 20.0, 622.0, 812.0));
        assert_eq!(shifted, BBox::new(82.0, 662.0, 282.0, 712.0));
        assert_eq!(rect.to_property(), "72,100,200,50");
        assert_eq!(
            PageRect::from_property("72.5,100.25,200,50.1").map(|r| r.to_property()),
            Some("72.5,100.25,200,50.1".to_string())
        );
        assert_eq!(PageRect::from_property("1,2,3"), None);
        assert!(PageRect { x: -1.0, ..rect }.validate().is_err());
        assert!(PageRect { width: 0.5, ..rect }.validate().is_err());
        assert!(PageRect {
            x: f32::NAN,
            ..rect
        }
        .validate()
        .is_err());
        assert_eq!(format_points(1.005), "1");
        assert_eq!(format_points(12.345), "12.35");
    }

    #[test]
    fn reads_text_inside_a_rectangle_of_a_real_pdf() {
        let pdf = build_pdf(
            &[vec![
                text(72.0, 700.0, 12.0, "Inside the box"),
                text(72.0, 680.0, 12.0, "also inside"),
                text(72.0, 400.0, 12.0, "far below"),
            ]],
            None,
        );
        // Top-left rectangle covering y (user) 670..720 on a 792 pt page.
        let rect = PageRect {
            x: 60.0,
            y: 72.0,
            width: 300.0,
            height: 50.0,
        };
        let region = text_in_rect(pdf.clone(), 1, rect).unwrap();
        assert_eq!(region.text, "Inside the box\nalso inside");
        assert_eq!(region.page_height, 792.0);
        assert!(matches!(
            text_in_rect(pdf, 2, rect),
            Err(ResearchError::Invalid(_))
        ));
        assert!(text_in_rect(b"not a pdf".to_vec(), 1, rect).is_err());
    }
}
