import type { PageSpan, PdfRegion } from '../types.ts';

/**
 * Layout boxes of an indexed passage, as stored by the indexer: PDF user
 * space points with the origin at the bottom-left of the page (the PDF
 * specification's convention, which is also pdf.js' PDF coordinate space).
 * The viewer maps them to CSS pixels with the page viewport's transform.
 */

function finite(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/**
 * Parse regions from a step detail (array of objects) or chunk metadata
 * (JSON string). Invalid entries are dropped; null when nothing is valid.
 */
export function parseRegions(value: unknown): PdfRegion[] | null {
  let raw: unknown = value;
  if (typeof raw === 'string') {
    try {
      raw = JSON.parse(raw);
    } catch {
      return null;
    }
  }
  if (!Array.isArray(raw)) return null;
  const out: PdfRegion[] = [];
  for (const entry of raw) {
    if (typeof entry !== 'object' || entry === null) continue;
    const r = entry as Record<string, unknown>;
    const { page, x0, y0, x1, y1 } = r;
    if (!finite(page) || !Number.isInteger(page) || page < 1) continue;
    if (!finite(x0) || !finite(y0) || !finite(x1) || !finite(y1)) continue;
    if (x1 <= x0 || y1 <= y0) continue;
    out.push({ page, x0, y0, x1, y1 });
  }
  return out.length > 0 ? out : null;
}

/** Regions on the cited pages (all regions when no pages are cited). */
export function regionsOnPages(regions: readonly PdfRegion[], pages: PageSpan | null): PdfRegion[] {
  if (!pages) return [...regions];
  return regions.filter(r => r.page >= pages.start && r.page <= pages.end);
}

/** The page to show first: the lowest page that has a region. */
export function firstRegionPage(regions: readonly PdfRegion[]): number | null {
  let best: number | null = null;
  for (const r of regions) if (best === null || r.page < best) best = r.page;
  return best;
}

export interface CssRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * Map a region to CSS pixels on the rendered page. `transform` is pdf.js'
 * `PageViewport.transform` ([a, b, c, d, e, f]: PDF point → viewport pixel),
 * which already accounts for the view box origin, the scale and the page
 * rotation; both corners are transformed and the result normalised.
 */
export function regionToCssRect(region: PdfRegion, transform: readonly number[]): CssRect {
  const [a, b, c, d, e, f] = transform;
  const apply = (x: number, y: number): [number, number] => [a * x + c * y + e, b * x + d * y + f];
  const [ax, ay] = apply(region.x0, region.y0);
  const [bx, by] = apply(region.x1, region.y1);
  const left = Math.min(ax, bx);
  const top = Math.min(ay, by);
  return { left, top, width: Math.abs(bx - ax), height: Math.abs(by - ay) };
}

/** Backing-store and display size of a rendered page canvas. */
export interface PageCanvasSize {
  /** Backing-store pixels: the viewport scaled by `ratio`, at least 1. */
  width: number;
  height: number;
  /** Display size: always the viewport's CSS size, independent of `ratio`. */
  cssWidth: string;
  cssHeight: string;
}

/**
 * A page is rasterized at `ratio` device pixels per CSS pixel for sharpness,
 * but must be displayed at the viewport's CSS size: highlights are placed in
 * CSS pixels from the viewport transform, so a canvas shown at its backing
 * size drifts away from them on any display scaled above 100%.
 */
export function pageCanvasSize(viewportWidth: number, viewportHeight: number, ratio: number): PageCanvasSize {
  return {
    width: Math.max(1, Math.floor(viewportWidth * ratio)),
    height: Math.max(1, Math.floor(viewportHeight * ratio)),
    cssWidth: `${viewportWidth}px`,
    cssHeight: `${viewportHeight}px`,
  };
}

/** The vertical span of a highlighted passage on its page, in CSS pixels from the page's top. */
export interface VerticalExtent {
  top: number;
  bottom: number;
}

/** The span the highlight `rects` cover (top of the highest to bottom of the lowest); null when there are none. */
export function verticalExtent(rects: readonly { top: number; height: number }[]): VerticalExtent | null {
  if (rects.length === 0) return null;
  let top = Infinity;
  let bottom = -Infinity;
  for (const r of rects) {
    top = Math.min(top, r.top);
    bottom = Math.max(bottom, r.top + r.height);
  }
  return Number.isFinite(top) && Number.isFinite(bottom) ? { top, bottom } : null;
}

/** Room kept above a passage too tall to centre. */
export const PASSAGE_SCROLL_MARGIN = 24;

/**
 * The scroll position that shows a highlighted passage: centred in the
 * viewport when it fits (with a margin above and below), else its top a
 * margin below the viewport's top, so it is read from its start.
 * `pageOffset` is the page's offset in the scrolled content.
 */
export function passageScrollTop(
  pageOffset: number,
  extent: VerticalExtent,
  viewportHeight: number,
  margin = PASSAGE_SCROLL_MARGIN,
): number {
  const height = extent.bottom - extent.top;
  const target = height + 2 * margin <= viewportHeight
    ? pageOffset + (extent.top + extent.bottom) / 2 - viewportHeight / 2
    : pageOffset + extent.top - margin;
  return Math.max(0, target);
}
