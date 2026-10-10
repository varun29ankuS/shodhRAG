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
