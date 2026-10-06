/**
 * Builders of focus targets from what each surface has at hand, with a
 * short human label for each. Payloads are capped so a thread record stays
 * small. Pure module, unit-tested with Node (`app/tests/focusContext.test.ts`).
 */

import type { FocusBox, FocusDocumentRef, FocusParamValue, FocusSourceHit, FocusTarget, FocusTaskSnapshot } from './focusTypes.ts';
import { annotateTex } from '../ask/visual/symbols.ts';
import type { SymbolNote } from '../ask/visual/symbols.ts';
import { MAX_TARGET_CHARS, blockError, readParamValues } from './threadStore.ts';
import { SVG_MAX_CHARS } from '../ask/visual/svgSanitize.ts';
import { PLOT_MAX_CHARS } from '../ask/visual/plotSpec.ts';
import { SIMULATION_MAX_CHARS } from '../ask/visual/simulationSpec.ts';

const LABEL_CHARS = 60;
/** Rows and columns of a table kept with a thread. */
const MAX_ROWS = 400;
const MAX_COLUMNS = 40;
const MAX_CELL = 500;

function short(text: string, max = LABEL_CHARS): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  const chars = Array.from(flat);
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : flat;
}

function cap(text: string, max = MAX_TARGET_CHARS): string {
  const chars = Array.from(text);
  return chars.length > max ? chars.slice(0, max).join('') : text;
}

const DIAGRAM_NAMES: Record<string, string> = {
  flowchart: 'Flowchart',
  graph: 'Flowchart',
  sequencediagram: 'Sequence diagram',
  classdiagram: 'Class diagram',
  statediagram: 'State diagram',
  'statediagram-v2': 'State diagram',
  erdiagram: 'Entity relationship diagram',
  gantt: 'Gantt chart',
  pie: 'Pie chart',
  journey: 'User journey',
  gitgraph: 'Git graph',
  mindmap: 'Mind map',
  timeline: 'Timeline',
};

/** A diagram; `error` is the parser's message when the diagram did not draw. */
export function mermaidTarget(source: string, error?: string | null): FocusTarget {
  const lines = source.split('\n').map(l => l.trim()).filter(l => l && !l.startsWith('%%'));
  const title = /^\s*title\s+(.+)$/im.exec(source)?.[1];
  const keyword = (lines[0] ?? '').split(/\s+/)[0].toLowerCase();
  const name = DIAGRAM_NAMES[keyword] ?? 'Diagram';
  return { kind: 'mermaid', label: title ? short(title) : name, source: cap(source), ...blockError(error) };
}

/** A chart; `error` is why it did not draw (kept so the reader can ask about it). */
export function chartTarget(source: string, title?: string | null, error?: string | null): FocusTarget {
  let label = title?.trim() || '';
  if (!label) {
    try {
      const parsed = JSON.parse(source) as { title?: unknown };
      if (typeof parsed.title === 'string') label = parsed.title;
    } catch {
      // Not JSON: keep the generic label.
    }
  }
  return { kind: 'chart', label: label ? short(label) : 'Chart', source: cap(source), ...blockError(error) };
}

/** The <title> of an SVG, if it has one. */
function svgTitle(source: string): string {
  const m = /<title(?:\s[^>]*)?>([\s\S]*?)<\/title>/i.exec(source);
  return m ? m[1].replace(/<[^>]*>/g, '').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&').trim() : '';
}

function specTitle(source: string): string {
  try {
    const parsed = JSON.parse(source) as { title?: unknown };
    return typeof parsed.title === 'string' ? parsed.title.trim() : '';
  } catch {
    return '';
  }
}

/**
 * A ```svg sketch. Sources over the render cap are never drawn, so they never
 * become targets; null keeps a truncated (invalid) SVG out of a thread.
 */
export function svgTarget(source: string, title?: string | null, error?: string | null): FocusTarget | null {
  if (source.length > SVG_MAX_CHARS) return null;
  const label = title?.trim() || svgTitle(source);
  return { kind: 'svg', label: label ? short(label) : 'Sketch', source, ...blockError(error) };
}

/** A ```plot with the slider positions the reader had when opening it. */
export function plotTarget(source: string, title: string | null | undefined, values: readonly FocusParamValue[], error?: string | null): FocusTarget | null {
  if (source.length > PLOT_MAX_CHARS) return null;
  const label = title?.trim() || specTitle(source);
  return { kind: 'plot', label: label ? short(label) : 'Interactive plot', source, values: readParamValues(values), ...blockError(error) };
}

/** A ```simulation with the slider positions the reader had when opening it. */
export function simulationTarget(source: string, title: string | null | undefined, values: readonly FocusParamValue[], error?: string | null): FocusTarget | null {
  if (source.length > SIMULATION_MAX_CHARS) return null;
  const label = title?.trim() || specTitle(source);
  return { kind: 'simulation', label: label ? short(label) : 'Simulation', source, values: readParamValues(values), ...blockError(error) };
}

/** A display equation, with the meanings of the symbols it contains (if any were given). */
export function equationTarget(tex: string, symbols: readonly SymbolNote[] = []): FocusTarget {
  const inside = symbolsIn(tex, symbols);
  return { kind: 'equation', label: `Equation ${short(tex, 40)}`, tex: cap(tex), ...(inside.length > 0 ? { symbols: inside } : {}) };
}

/** The notes of the symbols that occur in `tex`. */
export function symbolsIn(tex: string, symbols: readonly SymbolNote[]): SymbolNote[] {
  if (symbols.length === 0) return [];
  return annotateTex(tex, symbols).matched.map(i => symbols[i]);
}

/** Characters of the text near a figure kept with a thread. */
export const MAX_FIGURE_NEARBY_CHARS = 2_000;

export interface FigureTargetInput {
  filePath: string;
  fileName?: string;
  page: number;
  bbox: FocusBox;
  figureId: string | null;
  caption: string;
  /** "Figure 3", when known. */
  label?: string;
  /** Paragraphs that refer to the figure. */
  mentions?: readonly string[];
}

/** A figure of a paper; null without a usable place. */
export function figureTarget(input: FigureTargetInput): FocusTarget | null {
  const { x0, y0, x1, y1 } = input.bbox;
  if (!input.filePath || !Number.isInteger(input.page) || input.page < 1 || ![x0, y0, x1, y1].every(Number.isFinite) || x1 <= x0 || y1 <= y0) return null;
  const caption = cap(input.caption.replace(/\s+/g, ' ').trim(), MAX_SELECTED_CHARS);
  const fileName = input.fileName || input.filePath.split(/[\\/]/).pop() || input.filePath;
  const number = /^(?:figure|fig\.?)\s*([A-Z]?\d+[a-z]?)/i.exec(caption)?.[1];
  const name = input.label?.trim() || (number ? `Figure ${number}` : 'Figure');
  return {
    kind: 'figure',
    label: short(`${name} · ${fileName}`, 80),
    filePath: input.filePath,
    fileName,
    page: input.page,
    bbox: { x0, y0, x1, y1 },
    figureId: input.figureId,
    caption,
    nearby: cap((input.mentions ?? []).map(m => m.replace(/\s+/g, ' ').trim()).filter(Boolean).join('\n\n'), MAX_FIGURE_NEARBY_CHARS),
  };
}

export interface DerivationStepInput {
  title: string;
  index: number;
  total: number;
  latex: string;
  justification: string;
  previous: string | null;
  next: string | null;
  symbols?: readonly SymbolNote[];
}

/** One step of a derivation with its neighbours. */
export function derivationStepTarget(input: DerivationStepInput): FocusTarget | null {
  if (!input.latex.trim() || !Number.isInteger(input.index) || input.index < 0 || input.index >= input.total) return null;
  const inside = symbolsIn(input.latex, input.symbols ?? []);
  const title = short(input.title, 80);
  return {
    kind: 'derivation_step',
    label: `Step ${input.index + 1} of ${input.total}${title ? ` · ${title}` : ''}`,
    title,
    index: input.index,
    total: input.total,
    latex: cap(input.latex),
    justification: cap(input.justification, MAX_SELECTED_CHARS),
    previous: input.previous ? cap(input.previous) : null,
    next: input.next ? cap(input.next) : null,
    ...(inside.length > 0 ? { symbols: inside } : {}),
  };
}

export function tableTarget(rows: readonly (readonly string[])[]): FocusTarget | null {
  const kept = rows
    .filter(r => r.length > 0)
    .slice(0, MAX_ROWS)
    .map(r => r.slice(0, MAX_COLUMNS).map(c => cap(c, MAX_CELL)));
  if (kept.length === 0) return null;
  const header = kept[0].filter(c => c.trim()).join(', ');
  return { kind: 'table', label: header ? `Table: ${short(header, 50)}` : 'Table', rows: kept };
}

/** Images kept by address only; inline data is too large to keep with a thread. */
export function imageTarget(src: string | null | undefined, alt: string | null | undefined): FocusTarget {
  const keep = src && !src.startsWith('data:') ? src : src && src.length <= 200_000 ? src : null;
  return { kind: 'image', label: alt?.trim() ? short(alt) : 'Image', src: keep, alt: alt?.trim() ?? '' };
}

export function sourceTarget(hit: FocusSourceHit, label: string): FocusTarget {
  return {
    kind: 'source',
    label: short(label, 80),
    hit: { ...hit, text: cap(hit.text), snippet: cap(hit.snippet, 1_000) },
  };
}

export function taskTarget(task: FocusTaskSnapshot): FocusTarget {
  return { kind: 'task', label: short(task.title) || 'Task', task: { ...task, notes: cap(task.notes) } };
}

/** Characters of a selection, and of the text around it, kept with a thread. */
export const MAX_SELECTED_CHARS = 4_000;
export const MAX_SURROUNDING_CHARS = 3_000;
/** Characters on each side of a selection taken from a long block (a PDF page). */
export const SURROUNDING_RADIUS = 600;

function flat(text: string): string {
  return text.replace(/\s+/g, ' ').trim();
}

/**
 * The text around `selected` inside `full`: the whole block when it is
 * short, else a window of `radius` characters on each side of the first
 * occurrence (whitespace-insensitive). The block's start when not found.
 */
export function surroundingText(full: string, selected: string, radius = SURROUNDING_RADIUS): string {
  const block = flat(full);
  const needle = flat(selected);
  if (Array.from(block).length <= radius * 2 + Array.from(needle).length) return cap(block, MAX_SURROUNDING_CHARS);
  const at = needle ? block.indexOf(needle) : -1;
  if (at < 0) return `${cap(block, radius * 2).trimEnd()}…`;
  const start = Math.max(0, at - radius);
  const end = Math.min(block.length, at + needle.length + radius);
  const window = block.slice(start, end);
  return cap(`${start > 0 ? '…' : ''}${window}${end < block.length ? '…' : ''}`, MAX_SURROUNDING_CHARS);
}

export interface SelectionInput {
  text: string;
  /** The paragraph (or page text) the selection was made in. */
  context: string;
  origin: 'answer' | 'document';
  document?: FocusDocumentRef | null;
}

/** A target for "Ask about this" on selected text; null for an empty selection. */
export function selectionTarget(input: SelectionInput): FocusTarget | null {
  const text = input.text.replace(/[ \t]+/g, ' ').replace(/\n{3,}/g, '\n\n').trim();
  if (!flat(text)) return null;
  const doc = input.document && input.document.sourceFile ? input.document : null;
  return {
    kind: 'selection',
    label: `“${short(text, 48)}”`,
    text: cap(text, MAX_SELECTED_CHARS),
    paragraph: surroundingText(input.context, text),
    origin: input.origin,
    document: doc
      ? { sourceFile: doc.sourceFile, fileName: doc.fileName, page: doc.page !== null && Number.isInteger(doc.page) && doc.page > 0 ? doc.page : null }
      : null,
  };
}

/** A place in a document to show next to the discussion ("Show in paper"). */
export interface PaperRef {
  sourceFile: string;
  fileName: string;
  page: number | null;
  /** Text to find and highlight on the page. */
  passage: string;
  /** Rectangles to outline instead of finding the passage (a snippet's region). */
  rects?: { page: number; rect: { x: number; y: number; width: number; height: number } }[];
  /** Indexer boxes to outline instead of finding the passage. */
  regions?: { page: number; x0: number; y0: number; x1: number; y1: number }[];
}

/** The document place of one target, if it has one. */
export function paperOf(target: FocusTarget): PaperRef | null {
  if (target.kind === 'selection' && target.document) {
    return { sourceFile: target.document.sourceFile, fileName: target.document.fileName, page: target.document.page, passage: target.text };
  }
  if (target.kind === 'source' && !target.hit.url) {
    return {
      sourceFile: target.hit.sourceFile,
      fileName: target.hit.fileName || target.hit.title,
      page: target.hit.page?.start ?? null,
      passage: target.hit.text || target.hit.snippet,
      ...(target.hit.regions && target.hit.regions.length > 0 ? { regions: target.hit.regions } : {}),
    };
  }
  if (target.kind === 'figure') {
    return {
      sourceFile: target.filePath,
      fileName: target.fileName,
      page: target.page,
      passage: target.caption,
      regions: [{ page: target.page, ...target.bbox }],
    };
  }
  if (target.kind === 'snippet') {
    return {
      sourceFile: target.filePath,
      fileName: target.fileName,
      page: target.page,
      passage: target.text,
      rects: [{ page: target.page, rect: target.rect }],
    };
  }
  return null;
}

/**
 * The nearest document place for a level: its own target first, then its
 * outer levels from the nearest outwards. `targets` runs outermost first.
 */
export function nearestPaper(targets: readonly FocusTarget[]): PaperRef | null {
  for (let i = targets.length - 1; i >= 0; i--) {
    const ref = paperOf(targets[i]);
    if (ref) return ref;
  }
  return null;
}
