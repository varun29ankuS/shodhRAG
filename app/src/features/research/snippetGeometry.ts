/**
 * Geometry of PDF snippets: which page text lies inside a rectangle, how a
 * selection or a dragged marquee becomes a `SnippetRect`, how to render a
 * rectangle sharply with pdf.js, and conversions between the snippet's
 * top-left rectangle and the indexer's bottom-left regions.
 *
 * Pure module (types only) so it is unit-tested with Node
 * (`app/tests/snippetGeometry.test.ts`).
 *
 * Text-in-rect rule (the backend's `create_snippet` uses the same rule on
 * the parser's text runs, so a snippet's text does not depend on who made it):
 * 1. an item is inside when at least half of its box area lies in the
 *    rectangle (an item with no area: when its centre does);
 * 2. inside items are grouped into lines: an item joins the first line
 *    (lines in creation order) whose box overlaps it vertically by more than
 *    half of the smaller of the two heights; items are visited top to bottom,
 *    then left to right; a line's box grows to include its items;
 * 3. lines are ordered top to bottom, items in a line left to right; each
 *    item's text has whitespace runs collapsed and is trimmed, empty items
 *    are dropped, items are joined with one space and lines with "\n".
 */

import type { ResultRegion, SnippetRect } from './types.ts';

/** A box in PDF points, bottom-left origin (y grows upwards). */
export interface PdfBox {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** A piece of page text with its box in PDF points. */
export interface TextBox {
  text: string;
  box: PdfBox;
}

/** A pdf.js text item (`TextItem`), the fields used here. */
export interface PdfTextItemLike {
  str: string;
  /** [a, b, c, d, e, f]: text space → PDF user space; (e, f) is the baseline origin. */
  transform: readonly number[];
  width: number;
  height: number;
}

/** pdf.js `page.view`: the page's view box [x0, y0, x1, y1] in PDF points. */
export type ViewBox = readonly [number, number, number, number] | readonly number[];

/** A rectangle in CSS pixels relative to a rendered page's top-left corner. */
export interface CssBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Smallest rectangle side, in PDF points, a snippet may have. */
export const MIN_SNIPPET_SIDE = 4;

function finite(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function normalized(box: PdfBox): PdfBox {
  return {
    x0: Math.min(box.x0, box.x1),
    y0: Math.min(box.y0, box.y1),
    x1: Math.max(box.x0, box.x1),
    y1: Math.max(box.y0, box.y1),
  };
}

/**
 * The box of a pdf.js text item in PDF points. The item's text space is
 * mapped through its transform: the run spans `width` along the text
 * direction and `height` across it from the baseline.
 */
export function textItemBox(item: PdfTextItemLike): PdfBox | null {
  const [a, b, c, d, e, f] = item.transform;
  if (![a, b, c, d, e, f].every(finite) || !finite(item.width) || !finite(item.height)) return null;
  // Unit vectors along and across the run (rotated text included).
  const along = Math.hypot(a, b) || 1;
  const across = Math.hypot(c, d) || 1;
  const ux = a / along;
  const uy = b / along;
  const vx = c / across;
  const vy = d / across;
  const corners: [number, number][] = [
    [e, f],
    [e + ux * item.width, f + uy * item.width],
    [e + vx * item.height, f + vy * item.height],
    [e + ux * item.width + vx * item.height, f + uy * item.width + vy * item.height],
  ];
  const xs = corners.map(p => p[0]);
  const ys = corners.map(p => p[1]);
  return { x0: Math.min(...xs), y0: Math.min(...ys), x1: Math.max(...xs), y1: Math.max(...ys) };
}

/** Text boxes of pdf.js text items (marked-content markers and empty runs are skipped). */
export function textBoxes(items: readonly unknown[]): TextBox[] {
  const out: TextBox[] = [];
  for (const raw of items) {
    if (typeof raw !== 'object' || raw === null) continue;
    const item = raw as Partial<PdfTextItemLike>;
    if (typeof item.str !== 'string' || !Array.isArray(item.transform)) continue;
    const box = textItemBox(item as PdfTextItemLike);
    if (box) out.push({ text: item.str, box });
  }
  return out;
}

/** The snippet rectangle as a PDF box (bottom-left origin). */
export function rectToBox(rect: SnippetRect, view: ViewBox): PdfBox {
  const [vx0, , , vy1] = view;
  return {
    x0: vx0 + rect.x,
    x1: vx0 + rect.x + rect.width,
    y1: vy1 - rect.y,
    y0: vy1 - rect.y - rect.height,
  };
}

/** A PDF box (bottom-left origin) as a snippet rectangle relative to the view box. */
export function boxToRect(box: PdfBox, view: ViewBox): SnippetRect {
  const [vx0, , , vy1] = view;
  const b = normalized(box);
  return { x: b.x0 - vx0, y: vy1 - b.y1, width: b.x1 - b.x0, height: b.y1 - b.y0 };
}

/** The snippet rectangle as an indexer region on its page. */
export function rectToRegion(rect: SnippetRect, page: number, view: ViewBox): ResultRegion {
  const b = rectToBox(rect, view);
  return { page, x0: b.x0, y0: b.y0, x1: b.x1, y1: b.y1 };
}

/** An indexer region as a snippet rectangle. */
export function regionToRect(region: ResultRegion, view: ViewBox): SnippetRect {
  return boxToRect(region, view);
}

function area(box: PdfBox): number {
  return Math.max(0, box.x1 - box.x0) * Math.max(0, box.y1 - box.y0);
}

function intersection(a: PdfBox, b: PdfBox): number {
  const w = Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0);
  const h = Math.min(a.y1, b.y1) - Math.max(a.y0, b.y0);
  return w > 0 && h > 0 ? w * h : 0;
}

/** Whether a text box counts as inside `rect` (rule 1). */
export function insideBox(box: PdfBox, rect: PdfBox): boolean {
  const own = area(box);
  if (own <= 0) {
    const cx = (box.x0 + box.x1) / 2;
    const cy = (box.y0 + box.y1) / 2;
    return cx >= rect.x0 && cx <= rect.x1 && cy >= rect.y0 && cy <= rect.y1;
  }
  return intersection(box, rect) / own >= 0.5;
}

function cleanText(text: string): string {
  return text.replace(/\s+/g, ' ').trim();
}

/** The text inside `rect` (a PDF box), by the rule in the module comment. */
export function textInBox(items: readonly TextBox[], rect: PdfBox): string {
  const r = normalized(rect);
  const inside = items
    .map(item => ({ text: cleanText(item.text), box: normalized(item.box) }))
    .filter(item => item.text.length > 0 && insideBox(item.box, r));
  inside.sort((a, b) => b.box.y1 - a.box.y1 || a.box.x0 - b.box.x0);
  const lines: { box: PdfBox; items: { text: string; box: PdfBox }[] }[] = [];
  for (const item of inside) {
    const h = item.box.y1 - item.box.y0;
    const line = lines.find(l => {
      const overlap = Math.min(l.box.y1, item.box.y1) - Math.max(l.box.y0, item.box.y0);
      return overlap > 0.5 * Math.min(h, l.box.y1 - l.box.y0);
    });
    if (line) {
      line.items.push(item);
      line.box = {
        x0: Math.min(line.box.x0, item.box.x0),
        y0: Math.min(line.box.y0, item.box.y0),
        x1: Math.max(line.box.x1, item.box.x1),
        y1: Math.max(line.box.y1, item.box.y1),
      };
    } else {
      lines.push({ box: { ...item.box }, items: [item] });
    }
  }
  lines.sort((a, b) => b.box.y1 - a.box.y1 || a.box.x0 - b.box.x0);
  return lines
    .map(l => [...l.items].sort((a, b) => a.box.x0 - b.box.x0).map(i => i.text).join(' '))
    .join('\n');
}

/** The text of a page inside a snippet rectangle. */
export function textInRect(items: readonly TextBox[], rect: SnippetRect, view: ViewBox): string {
  return textInBox(items, rectToBox(rect, view));
}

/** Inverse of an affine transform [a, b, c, d, e, f]; null when singular. */
export function invertTransform(t: readonly number[]): number[] | null {
  const [a, b, c, d, e, f] = t;
  const det = a * d - b * c;
  if (!finite(det) || Math.abs(det) < 1e-12) return null;
  return [d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det];
}

function apply(t: readonly number[], x: number, y: number): [number, number] {
  return [t[0] * x + t[2] * y + t[4], t[1] * x + t[3] * y + t[5]];
}

/**
 * CSS rectangles on a rendered page (selection rects, a dragged marquee)
 * as one snippet rectangle: each corner goes through the inverse of the
 * page viewport's transform (PDF point → CSS pixel), the union is taken in
 * PDF space and expressed relative to the view box. Null when nothing has
 * at least `MIN_SNIPPET_SIDE` points on each side.
 */
export function cssToSnippetRect(boxes: readonly CssBox[], transform: readonly number[], view: ViewBox): SnippetRect | null {
  const inverse = invertTransform(transform);
  if (!inverse) return null;
  let union: PdfBox | null = null;
  for (const b of boxes) {
    if (!finite(b.left) || !finite(b.top) || !finite(b.width) || !finite(b.height) || b.width <= 0 || b.height <= 0) continue;
    const corners = [
      apply(inverse, b.left, b.top),
      apply(inverse, b.left + b.width, b.top),
      apply(inverse, b.left, b.top + b.height),
      apply(inverse, b.left + b.width, b.top + b.height),
    ];
    const box: PdfBox = {
      x0: Math.min(...corners.map(p => p[0])),
      y0: Math.min(...corners.map(p => p[1])),
      x1: Math.max(...corners.map(p => p[0])),
      y1: Math.max(...corners.map(p => p[1])),
    };
    union = union
      ? { x0: Math.min(union.x0, box.x0), y0: Math.min(union.y0, box.y0), x1: Math.max(union.x1, box.x1), y1: Math.max(union.y1, box.y1) }
      : box;
  }
  if (!union) return null;
  // Keep the rectangle on the page.
  const [vx0, vy0, vx1, vy1] = view;
  const clipped: PdfBox = {
    x0: Math.max(vx0, union.x0),
    y0: Math.max(vy0, union.y0),
    x1: Math.min(vx1, union.x1),
    y1: Math.min(vy1, union.y1),
  };
  if (clipped.x1 - clipped.x0 < MIN_SNIPPET_SIDE || clipped.y1 - clipped.y0 < MIN_SNIPPET_SIDE) return null;
  return roundRect(boxToRect(clipped, view));
}

/** A rectangle rounded to 0.01 pt (what is stored). */
export function roundRect(rect: SnippetRect): SnippetRect {
  const r = (v: number) => Math.round(v * 100) / 100;
  return { x: r(rect.x), y: r(rect.y), width: r(rect.width), height: r(rect.height) };
}

/** Whether a value is a usable snippet rectangle. */
export function isSnippetRect(value: unknown): value is SnippetRect {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return finite(r.x) && finite(r.y) && finite(r.width) && finite(r.height) && (r.width as number) > 0 && (r.height as number) > 0 && (r.x as number) >= 0 && (r.y as number) >= 0;
}

/** Limits of a snippet render. */
export interface CropLimits {
  /** Preferred pixels per PDF point. */
  targetScale: number;
  /** Longest canvas side in pixels. */
  maxSide: number;
  /** Most canvas pixels. */
  maxPixels: number;
}

export const DEFAULT_CROP_LIMITS: CropLimits = { targetScale: 4, maxSide: 4096, maxPixels: 16_000_000 };

/** How to render a snippet: pdf.js viewport options and the canvas size. */
export interface CropPlan {
  scale: number;
  /** Pass to `page.getViewport({ scale, rotation, offsetX, offsetY })`. */
  offsetX: number;
  offsetY: number;
  width: number;
  height: number;
}

/**
 * The render of a snippet rectangle: the largest scale up to
 * `targetScale` whose canvas fits `maxSide` and `maxPixels`, and the
 * viewport offset that puts the rectangle's top-left corner at the canvas
 * origin. `unitTransform` is the page viewport's transform at scale 1 with
 * the page's rotation and no offset (a viewport's transform at scale s is s
 * times it), so rotated pages crop correctly.
 */
export function cropPlan(rect: SnippetRect, view: ViewBox, unitTransform: readonly number[], limits: CropLimits = DEFAULT_CROP_LIMITS): CropPlan | null {
  const box = rectToBox(rect, view);
  const corners = [apply(unitTransform, box.x0, box.y0), apply(unitTransform, box.x1, box.y0), apply(unitTransform, box.x0, box.y1), apply(unitTransform, box.x1, box.y1)];
  const left = Math.min(...corners.map(p => p[0]));
  const top = Math.min(...corners.map(p => p[1]));
  const w = Math.max(...corners.map(p => p[0])) - left;
  const h = Math.max(...corners.map(p => p[1])) - top;
  if (!(w > 0) || !(h > 0) || !finite(w) || !finite(h)) return null;
  const bySide = limits.maxSide / Math.max(w, h);
  const byArea = Math.sqrt(limits.maxPixels / (w * h));
  const scale = Math.max(0.01, Math.min(limits.targetScale, bySide, byArea));
  const width = Math.min(limits.maxSide, Math.max(1, Math.ceil(w * scale)));
  const height = Math.min(limits.maxSide, Math.max(1, Math.ceil(h * scale)));
  return { scale, offsetX: -left * scale, offsetY: -top * scale, width, height };
}
