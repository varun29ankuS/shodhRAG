//! Reading order of recognised lines on a page: a recursive XY cut on the
//! deskewed line boxes.
//!
//! - The page's skew (median angle of the long lines) is removed from the
//!   line centres first, so a 2 degree scan still separates its lines.
//! - A region splits into columns at a vertical gutter only when both sides
//!   read as text columns: at least three rows each, and either their rows do
//!   not line up (typeset columns rarely share every baseline) or both sides
//!   carry prose (a median of five or more words per row). A table, whose
//!   cells line up row by row and hold a few words each, is never split, so
//!   its rows read across.
//! - Otherwise a region splits into bands at horizontal gaps of at least half
//!   a line height (blocks), and a region that cannot split is read row by
//!   row, left to right, the boxes of a row joined by spaces.

use super::geometry::Quad;

/// Least share of a line height between two blocks.
const BLOCK_GAP: f32 = 0.5;
/// Least gutter width, in line heights, for a column split.
const MIN_GUTTER: f32 = 1.0;
/// Rows each side of a gutter needs before it can be a column.
const MIN_COLUMN_ROWS: usize = 3;
/// Rows whose centres are within this share of a line height line up.
const ALIGNED: f32 = 0.3;
/// Median words per row from which both sides of a gutter count as prose.
const PROSE_WORDS: usize = 5;
/// Skew estimates beyond this are not trusted (rotated pages are not deskewed).
const MAX_SKEW_DEG: f32 = 10.0;

#[derive(Debug, Clone)]
struct Item<'a> {
    text: &'a str,
    cx: f32,
    cy: f32,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some(values[values.len() / 2])
}

/// The page's skew in radians: the median top-edge angle of lines at least
/// twice as wide as tall, or 0.
fn page_skew(lines: &[(Quad, &str)]) -> f32 {
    let angles: Vec<f32> = lines
        .iter()
        .filter(|(q, _)| q.width() >= 2.0 * q.height())
        .map(|(q, _)| q.angle())
        .collect();
    median(angles)
        .filter(|a| a.abs() <= MAX_SKEW_DEG.to_radians())
        .unwrap_or(0.0)
}

struct Layout<'a> {
    items: Vec<Item<'a>>,
    line_height: f32,
}

impl Layout<'_> {
    /// Rows of `ids`: sorted by centre, a box joins the row whose mean centre
    /// is within half a line height; each row sorted left to right.
    fn rows(&self, ids: &[usize]) -> Vec<Vec<usize>> {
        let mut sorted = ids.to_vec();
        sorted.sort_by(|&a, &b| self.items[a].cy.total_cmp(&self.items[b].cy));
        let mut rows: Vec<(f32, Vec<usize>)> = Vec::new();
        for id in sorted {
            let cy = self.items[id].cy;
            match rows.last_mut() {
                Some((mean, row)) if (cy - *mean).abs() <= 0.5 * self.line_height => {
                    row.push(id);
                    *mean += (cy - *mean) / row.len() as f32;
                }
                _ => rows.push((cy, vec![id])),
            }
        }
        rows.into_iter()
            .map(|(_, mut row)| {
                row.sort_by(|&a, &b| self.items[a].left.total_cmp(&self.items[b].left));
                row
            })
            .collect()
    }

    fn row_centre(&self, row: &[usize]) -> f32 {
        row.iter().map(|&i| self.items[i].cy).sum::<f32>() / row.len() as f32
    }

    fn row_words(&self, row: &[usize]) -> usize {
        row.iter()
            .map(|&i| self.items[i].text.split_whitespace().count())
            .sum()
    }

    /// Whether `left` and `right` of a gutter read as two text columns.
    fn columns(&self, left: &[usize], right: &[usize]) -> bool {
        let (rows_l, rows_r) = (self.rows(left), self.rows(right));
        if rows_l.len() < MIN_COLUMN_ROWS || rows_r.len() < MIN_COLUMN_ROWS {
            return false;
        }
        // Rows of the shorter side that line up with a row across the gutter:
        // a table column lines up with its neighbours however long the page
        // around it is.
        let (few, many) = if rows_l.len() <= rows_r.len() {
            (&rows_l, &rows_r)
        } else {
            (&rows_r, &rows_l)
        };
        let centres: Vec<f32> = many.iter().map(|r| self.row_centre(r)).collect();
        let aligned = few
            .iter()
            .filter(|r| {
                let c = self.row_centre(r);
                centres
                    .iter()
                    .any(|o| (o - c).abs() <= ALIGNED * self.line_height)
            })
            .count();
        if (aligned as f32) < 0.5 * few.len() as f32 {
            return true;
        }
        let words = |rows: &[Vec<usize>]| {
            let mut counts: Vec<usize> = rows.iter().map(|r| self.row_words(r)).collect();
            counts.sort_unstable();
            counts[counts.len() / 2]
        };
        words(&rows_l) >= PROSE_WORDS && words(&rows_r) >= PROSE_WORDS
    }

    /// The widest gutter that splits `ids` into two text columns.
    fn column_split(&self, ids: &[usize]) -> Option<(Vec<usize>, Vec<usize>)> {
        let mut spans: Vec<(f32, f32)> = ids
            .iter()
            .map(|&i| (self.items[i].left, self.items[i].right))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut gutters: Vec<(f32, f32)> = Vec::new();
        let mut reach = f32::NEG_INFINITY;
        for (l, r) in spans {
            if reach.is_finite() && l - reach >= MIN_GUTTER * self.line_height {
                gutters.push((reach, l));
            }
            reach = reach.max(r);
        }
        gutters.sort_by(|a, b| (b.1 - b.0).total_cmp(&(a.1 - a.0)));
        gutters.into_iter().find_map(|(start, end)| {
            let (left, right): (Vec<usize>, Vec<usize>) =
                ids.iter().partition(|&&i| self.items[i].right <= start);
            debug_assert!(right.iter().all(|&i| self.items[i].left >= end));
            self.columns(&left, &right).then_some((left, right))
        })
    }

    /// Horizontal bands of `ids` separated by gaps of at least
    /// [`BLOCK_GAP`] line heights, top to bottom.
    fn bands(&self, ids: &[usize]) -> Vec<Vec<usize>> {
        let mut sorted = ids.to_vec();
        sorted.sort_by(|&a, &b| self.items[a].top.total_cmp(&self.items[b].top));
        let mut bands: Vec<Vec<usize>> = Vec::new();
        let mut reach = f32::NEG_INFINITY;
        for id in sorted {
            let item = &self.items[id];
            if bands.is_empty() || item.top - reach >= BLOCK_GAP * self.line_height {
                bands.push(Vec::new());
            }
            if let Some(band) = bands.last_mut() {
                band.push(id);
            }
            reach = reach.max(item.bottom);
        }
        bands
    }

    fn read(&self, ids: &[usize], out: &mut Vec<Vec<String>>) {
        if ids.is_empty() {
            return;
        }
        if let Some((left, right)) = self.column_split(ids) {
            self.read(&left, out);
            self.read(&right, out);
            return;
        }
        let bands = self.bands(ids);
        if bands.len() > 1 {
            for band in bands {
                self.read(&band, out);
            }
            return;
        }
        let block = self
            .rows(ids)
            .into_iter()
            .map(|row| {
                row.iter()
                    .map(|&i| self.items[i].text.trim())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if !block.is_empty() {
            out.push(block);
        }
    }
}

/// The lines of a page (box in page pixels, text) in reading order: blocks
/// of text lines, a table row as one line.
pub fn reading_order(lines: &[(Quad, &str)]) -> Vec<Vec<String>> {
    if lines.is_empty() {
        return Vec::new();
    }
    let skew = page_skew(lines);
    let (sin, cos) = (-skew).sin_cos();
    let items: Vec<Item> = lines
        .iter()
        .map(|(quad, text)| {
            let c = quad.center();
            let (cx, cy) = (c.x * cos - c.y * sin, c.x * sin + c.y * cos);
            let (w, h) = (quad.width(), quad.height());
            Item {
                text,
                cx,
                cy,
                left: cx - w / 2.0,
                right: cx + w / 2.0,
                top: cy - h / 2.0,
                bottom: cy + h / 2.0,
            }
        })
        .collect();
    let line_height = median(lines.iter().map(|(q, _)| q.height()).collect())
        .unwrap_or(1.0)
        .max(1.0);
    let layout = Layout { items, line_height };
    let ids: Vec<usize> = (0..layout.items.len()).collect();
    let mut out = Vec::new();
    layout.read(&ids, &mut out);
    debug_assert!(layout.items.iter().all(|i| i.cx.is_finite()));
    out
}

/// A page's text: blocks separated by blank lines, lines by newlines.
pub fn page_text(blocks: &[Vec<String>]) -> String {
    blocks
        .iter()
        .map(|b| b.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::super::geometry::Point;
    use super::*;

    /// An upright box `x0..x1` x `y0..y1`, rotated by `deg` about the origin.
    fn bx(x0: f32, y0: f32, x1: f32, y1: f32, deg: f32) -> Quad {
        let (s, c) = deg.to_radians().sin_cos();
        let r = |x: f32, y: f32| Point::new(x * c - y * s, x * s + y * c);
        Quad([r(x0, y0), r(x1, y0), r(x1, y1), r(x0, y1)])
    }

    fn read(lines: &[(Quad, &str)]) -> Vec<Vec<String>> {
        reading_order(lines)
    }

    #[test]
    fn table_rows_read_across_and_are_never_split_into_columns() {
        // Detection order scrambled; cells of a row at slightly different heights.
        let lines = [
            (bx(600.0, 100.0, 700.0, 130.0, 0.0), "Qty"),
            (bx(100.0, 101.0, 300.0, 131.0, 0.0), "Item"),
            (bx(800.0, 99.0, 1000.0, 129.0, 0.0), "Amount"),
            (bx(800.0, 140.0, 1000.0, 170.0, 0.0), "3,60,000.00"),
            (
                bx(100.0, 142.0, 500.0, 172.0, 0.0),
                "Data warehouse design sprint",
            ),
            (bx(600.0, 141.0, 640.0, 171.0, 0.0), "1"),
            (
                bx(100.0, 180.0, 480.0, 210.0, 0.0),
                "Cloud cost optimisation review",
            ),
            (bx(600.0, 181.0, 640.0, 211.0, 0.0), "1"),
            (bx(800.0, 179.0, 1000.0, 209.0, 0.0), "40,000.00"),
            (bx(800.0, 220.0, 1000.0, 250.0, 0.0), "6,75,000.00"),
            (bx(600.0, 221.0, 760.0, 251.0, 0.0), "Subtotal"),
        ];
        assert_eq!(
            read(&lines),
            vec![vec![
                "Item Qty Amount",
                "Data warehouse design sprint 1 3,60,000.00",
                "Cloud cost optimisation review 1 40,000.00",
                "Subtotal 6,75,000.00",
            ]]
        );
    }

    #[test]
    fn a_table_column_clear_of_the_prose_still_reads_across() {
        // Prose lines above and below end left of the amount column, so a
        // gutter runs down the whole page; the amounts still belong to their rows.
        let mut lines = vec![
            (bx(100.0, 20.0, 700.0, 50.0, 0.0), "TAX INVOICE"),
            (
                bx(100.0, 60.0, 700.0, 90.0, 0.0),
                "Invoice No: INV-2025-0117",
            ),
            (
                bx(100.0, 100.0, 700.0, 130.0, 0.0),
                "From: Bluepeak Systems LLP",
            ),
            (
                bx(100.0, 140.0, 700.0, 170.0, 0.0),
                "To: Northwind Analytics Pvt. Ltd.",
            ),
        ];
        let rows = [
            ("Data warehouse design sprint", "3,60,000.00"),
            ("Operations retainer (May 2025)", "2,75,000.00"),
            ("Cloud cost optimisation review", "40,000.00"),
        ];
        for (i, (item, amount)) in rows.iter().enumerate() {
            let y = 200.0 + 40.0 * i as f32;
            lines.push((bx(100.0, y, 500.0, y + 30.0, 0.0), *item));
            lines.push((bx(800.0, y, 1000.0, y + 30.0, 0.0), *amount));
        }
        lines.push((
            bx(100.0, 340.0, 700.0, 370.0, 0.0),
            "Total amount due: INR 7,96,500.00",
        ));
        let text = read(&lines).concat();
        assert!(
            text.contains(&"Data warehouse design sprint 3,60,000.00".to_string()),
            "{text:?}"
        );
        assert!(
            text.contains(&"Cloud cost optimisation review 40,000.00".to_string()),
            "{text:?}"
        );
        assert_eq!(
            text.last().map(String::as_str),
            Some("Total amount due: INR 7,96,500.00")
        );
    }

    #[test]
    fn two_text_columns_read_column_by_column_under_a_full_width_title() {
        let mut lines = vec![(
            bx(100.0, 20.0, 1100.0, 60.0, 0.0),
            "A Title Across Both Columns",
        )];
        let left = [
            "the left column opens with this line of text",
            "and it continues on the second line here",
            "before ending on a third line of prose",
        ];
        let right = [
            "meanwhile the right column says other things",
            "in lines that share the same baselines too",
            "so only the prose test can separate them",
        ];
        for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            let y = 100.0 + 40.0 * i as f32;
            lines.push((bx(100.0, y, 560.0, y + 30.0, 0.0), *l));
            lines.push((bx(640.0, y, 1100.0, y + 30.0, 0.0), *r));
        }
        let blocks = read(&lines);
        let text: Vec<String> = blocks.concat();
        assert_eq!(text[0], "A Title Across Both Columns");
        assert_eq!(&text[1..4], &left.map(String::from));
        assert_eq!(&text[4..7], &right.map(String::from));
    }

    #[test]
    fn skewed_lines_stay_separate_and_in_order() {
        // 300 dpi page lines 58 px apart, 1900 px long, skewed by 2 degrees:
        // each line drifts 66 px across, more than the line pitch.
        let texts = ["first line", "second line", "third line", "fourth line"];
        let mut lines: Vec<(Quad, &str)> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let y = 300.0 + 58.0 * i as f32;
                (bx(300.0, y, 2200.0, y + 40.0, 2.0), *t)
            })
            .collect();
        lines.reverse();
        assert_eq!(read(&lines), vec![texts.map(String::from).to_vec()]);
    }

    #[test]
    fn blocks_split_at_gaps_and_page_text_joins_them() {
        let lines = [
            (bx(100.0, 100.0, 900.0, 130.0, 0.0), "Heading"),
            (bx(100.0, 200.0, 900.0, 230.0, 0.0), "Body one"),
            (bx(100.0, 235.0, 900.0, 265.0, 0.0), "Body two"),
        ];
        let blocks = read(&lines);
        assert_eq!(blocks.len(), 2);
        assert_eq!(page_text(&blocks), "Heading\n\nBody one\nBody two");
        assert!(read(&[]).is_empty());
    }
}
