/**
 * Figures and equations of a paper as the `paper_objects` command returns
 * them, a ```figure block written by the agent, and how the block finds its
 * figure. Boxes are PDF points with a bottom-left origin (the indexer's
 * convention, like citation regions).
 *
 * Pure module, unit-tested with Node (`app/tests/paperObjects.test.ts`).
 */

import type { ResultRegion } from './types.ts';

export interface PdfBoxCorners {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export interface PaperFigure {
  id: string;
  number: string | null;
  label: string;
  caption: string;
  page: number;
  bbox: PdfBoxCorners;
  captionBbox: PdfBoxCorners;
  regionFound: boolean;
  mentions: string[];
}

export type EquationOrigin = 'reconstructed' | 'source';

export interface PaperEquation {
  id: string;
  number: string | null;
  text: string;
  latex: string;
  origin: EquationOrigin;
  sourceFile: string | null;
  page: number | null;
  bbox: PdfBoxCorners | null;
}

export interface PaperParts {
  filePath: string;
  fileName: string;
  figures: PaperFigure[];
  equations: PaperEquation[];
  texSource: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown): value is string {
  return typeof value === 'string';
}

function finite(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function page(value: unknown): number | null {
  return typeof value === 'number' && Number.isInteger(value) && value >= 1 ? value : null;
}

/** A box from `{x0, y0, x1, y1}` or `[x0, y0, x1, y1]`, corners ordered; null when empty. */
export function readBox(value: unknown): PdfBoxCorners | null {
  let raw: unknown[] | null = null;
  if (Array.isArray(value) && value.length === 4) raw = value;
  else if (isRecord(value)) raw = [value.x0, value.y0, value.x1, value.y1];
  if (!raw || !raw.every(finite)) return null;
  const [a, b, c, d] = raw as number[];
  const box = { x0: Math.min(a, c), y0: Math.min(b, d), x1: Math.max(a, c), y1: Math.max(b, d) };
  return box.x1 - box.x0 >= 1 && box.y1 - box.y0 >= 1 && box.x0 >= -1 && box.y0 >= -1 ? box : null;
}

function readFigure(value: unknown): PaperFigure | null {
  if (!isRecord(value) || !str(value.id) || !value.id) return null;
  const p = page(value.page);
  const bbox = readBox(value.bbox);
  if (p === null || !bbox) return null;
  return {
    id: value.id,
    number: str(value.number) ? value.number : null,
    label: str(value.label) && value.label ? value.label : 'Figure',
    caption: str(value.caption) ? value.caption : '',
    page: p,
    bbox,
    captionBbox: readBox(value.captionBbox) ?? bbox,
    regionFound: value.regionFound === true,
    mentions: Array.isArray(value.mentions) ? value.mentions.filter(str) : [],
  };
}

function readEquation(value: unknown): PaperEquation | null {
  if (!isRecord(value) || !str(value.id) || !value.id || !str(value.latex)) return null;
  return {
    id: value.id,
    number: str(value.number) ? value.number : null,
    text: str(value.text) ? value.text : '',
    latex: value.latex,
    origin: value.origin === 'source' ? 'source' : 'reconstructed',
    sourceFile: str(value.sourceFile) ? value.sourceFile : null,
    page: page(value.page),
    bbox: readBox(value.bbox),
  };
}

/** The command's answer, or null when it is unreadable. Bad entries are dropped. */
export function readPaperParts(value: unknown): PaperParts | null {
  if (!isRecord(value) || !str(value.filePath)) return null;
  return {
    filePath: value.filePath,
    fileName: str(value.fileName) ? value.fileName : value.filePath.split(/[\\/]/).pop() ?? value.filePath,
    figures: Array.isArray(value.figures) ? value.figures.map(readFigure).filter((f): f is PaperFigure => f !== null) : [],
    equations: Array.isArray(value.equations) ? value.equations.map(readEquation).filter((e): e is PaperEquation => e !== null) : [],
    texSource: str(value.texSource) ? value.texSource : null,
  };
}

// ------------------------------------------------------------- ```figure

export interface FigureSpec {
  /** The indexed PDF (path as the tool returned it). */
  paper: string;
  figureId: string | null;
  page: number | null;
  bbox: PdfBoxCorners | null;
  caption: string;
}

export type FigureParseResult = { ok: true; spec: FigureSpec } | { ok: false; error: string };

const MAX_PAPER_CHARS = 1_000;
const MAX_CAPTION_CHARS = 2_000;

export function parseFigureBlock(source: string): FigureParseResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { ok: false, error: 'The figure reference is not valid JSON.' };
  }
  if (!isRecord(parsed)) return { ok: false, error: 'The figure reference must be a JSON object.' };
  const paper = str(parsed.paper) ? parsed.paper.trim() : str(parsed.file) ? parsed.file.trim() : '';
  if (!paper || paper.length > MAX_PAPER_CHARS) return { ok: false, error: 'The figure reference needs "paper" (the PDF it is in).' };
  const rawId = parsed.figureId ?? parsed.figure_id ?? parsed.id;
  const figureId = str(rawId) && /^fig-[a-z0-9.-]{1,40}$/i.test(rawId.trim()) ? rawId.trim().toLowerCase() : null;
  const p = page(parsed.page);
  const bbox = readBox(parsed.bbox);
  if (!figureId && (p === null || !bbox)) return { ok: false, error: 'The figure reference needs "figureId" (from show_figure) or "page" and "bbox".' };
  const caption = str(parsed.caption) ? Array.from(parsed.caption.replace(/\s+/g, ' ').trim()).slice(0, MAX_CAPTION_CHARS).join('') : '';
  return { ok: true, spec: { paper, figureId, page: p, bbox: bbox && p !== null ? bbox : null, caption } };
}

export interface ResolvedFigure {
  figureId: string | null;
  page: number;
  bbox: PdfBoxCorners;
  caption: string;
  label: string;
  mentions: string[];
}

/**
 * The figure a block shows: the paper's figure with that id when the paper
 * still has it (its region may have improved since the answer), else the
 * page and box the block carries, so an old answer still draws. Null when
 * neither is available.
 */
export function resolveFigure(spec: FigureSpec, figures: readonly PaperFigure[] | null): ResolvedFigure | null {
  const found = spec.figureId && figures ? figures.find(f => f.id === spec.figureId) ?? null : null;
  if (found) {
    return { figureId: found.id, page: found.page, bbox: found.bbox, caption: found.caption || spec.caption, label: found.label, mentions: found.mentions };
  }
  if (spec.page !== null && spec.bbox) {
    const number = /^(?:figure|fig\.?)\s*([A-Z]?\d+[a-z]?)/i.exec(spec.caption)?.[1];
    return { figureId: spec.figureId, page: spec.page, bbox: spec.bbox, caption: spec.caption, label: number ? `Figure ${number}` : 'Figure', mentions: [] };
  }
  return null;
}

/** The figure's box as a citation region (for "Show in PDF"). */
export function figureRegion(page: number, bbox: PdfBoxCorners): ResultRegion {
  return { page, x0: bbox.x0, y0: bbox.y0, x1: bbox.x1, y1: bbox.y1 };
}
