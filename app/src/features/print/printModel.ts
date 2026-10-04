/**
 * Print documents: what Export → PDF sends to the print view, and how the
 * print view lays it out. Pure, unit-tested with Node
 * (`app/tests/printModel.test.ts`). The Rust side (`pdf_export.rs`)
 * validates the same shape.
 */

import { citedNumbersIn } from '../agent/grounding.ts';
import { formatLocation, sourceLabel } from '../ask/searchResults.ts';
import type { SearchHit } from '../ask/types.ts';
import { KIND_FENCE } from '../visuals/extract.ts';
import type { VisualKind } from '../visuals/extract.ts';

/** One entry of the printed Sources section (`PrintSource` in pdf_export.rs). */
export interface PrintSource {
  n: number;
  /** File name, record title or web page title. */
  title: string;
  /** "p. 4", "pp. 4–5" or a section; null when unknown. */
  location: string | null;
  /** For web sources. */
  url: string | null;
}

/** What a print job renders (`PrintDocument` in pdf_export.rs). */
export interface PrintDocument {
  title: string;
  subtitle: string | null;
  /** RFC 3339. */
  createdAt: string;
  markdown: string;
  sources: PrintSource[];
}

/** What the print view receives from `print_job`. */
export interface PrintJob {
  document: PrintDocument;
  /** The app writes the PDF itself (Windows); otherwise the print dialog opens. */
  native: boolean;
}

export const MAX_PRINT_TITLE = 300;

function isWebUrl(value: string): boolean {
  return /^https?:\/\//i.test(value);
}

/** The file name of a path (last segment). */
function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** A Sources entry for a cited passage: file name with its page, or the web page and its URL. */
export function printSource(hit: SearchHit): PrintSource {
  if (hit.url && isWebUrl(hit.url)) {
    return { n: hit.number, title: sourceLabel(hit), location: null, url: hit.url };
  }
  const record = /^[a-z]+:\/\//i.test(hit.sourceFile);
  const title = record ? sourceLabel(hit) : hit.fileName?.trim() || fileName(hit.sourceFile);
  const location = formatLocation(hit) ?? (hit.section?.trim() || null);
  return { n: hit.number, title: title || `Source ${hit.number}`, location, url: null };
}

/**
 * The Sources section: every cited number that has a passage, once, in
 * number order. Passages the text never cites are left out.
 */
export function printSources(markdown: string, hits: readonly SearchHit[]): PrintSource[] {
  const cited = citedNumbersIn(markdown);
  const byNumber = new Map<number, SearchHit>();
  for (const hit of hits) if (cited.has(hit.number) && !byNumber.has(hit.number)) byNumber.set(hit.number, hit);
  return [...byNumber.values()].sort((a, b) => a.number - b.number).map(printSource);
}

/** Cited numbers no passage resolves (printed as plain "[n]", never as a link). */
export function unresolvedCitations(markdown: string, sources: readonly PrintSource[]): number[] {
  const known = new Set(sources.map(s => s.n));
  return [...citedNumbersIn(markdown)].filter(n => !known.has(n)).sort((a, b) => a - b);
}

/** A title for an answer: its first heading, else its first sentence, shortened. */
export function answerTitle(markdown: string, fallback = 'Shodh answer'): string {
  const heading = /^\s{0,3}#{1,3}\s+(.+?)\s*#*\s*$/m.exec(markdown);
  const raw = heading
    ? heading[1]
    : markdown
        .replace(/```[\s\S]*?```/g, ' ')
        .replace(/\$\$[\s\S]*?\$\$/g, ' ')
        .split(/\n\s*\n/)
        .map(p => p.trim())
        .find(p => p.length > 0 && !p.startsWith('|')) ?? '';
  const plain = raw
    .replace(/\s*\[(\d+(?:\s*[-–,]\s*\d+)*)\]/g, '')
    .replace(/[*_`#>]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  const sentence = /^(.+?[.!?])(\s|$)/.exec(plain)?.[1] ?? plain;
  const short = sentence.length > 90 ? `${sentence.slice(0, 89).trimEnd()}…` : sentence;
  return short || fallback;
}

/** The markdown without a first-line `# heading` that repeats the title (the page prints the title). */
export function stripLeadingTitle(markdown: string, title: string): string {
  const match = /^\s*#\s+(.+?)\s*\n/.exec(markdown);
  if (match && match[1].trim().toLowerCase() === title.trim().toLowerCase()) return markdown.slice(match[0].length);
  return markdown;
}

/** A file name for the save dialog: the title made safe, with `.pdf`. */
export function printFileName(title: string): string {
  const stem = title.replace(/[\\/:*?"<>|\u0000-\u001f]+/g, ' ').replace(/\s+/g, ' ').trim().replace(/[. ]+$/, '').slice(0, 80);
  return `${stem || 'Shodh export'}.pdf`;
}

/** Build the document Export → PDF sends. */
export function buildPrintDocument(input: {
  title: string;
  subtitle?: string | null;
  createdAt?: Date | string | null;
  markdown: string;
  hits?: readonly SearchHit[];
}): PrintDocument {
  const title = (input.title.trim() || 'Shodh answer').slice(0, MAX_PRINT_TITLE);
  const when = input.createdAt ? new Date(input.createdAt) : new Date();
  const createdAt = Number.isNaN(when.getTime()) ? new Date().toISOString() : when.toISOString();
  const markdown = stripLeadingTitle(input.markdown, title);
  return {
    title,
    subtitle: input.subtitle?.trim() ? input.subtitle.trim().slice(0, MAX_PRINT_TITLE) : null,
    createdAt,
    markdown,
    sources: printSources(markdown, input.hits ?? []),
  };
}

/**
 * A single gallery visual as markdown the answer renderer draws: fenced in
 * its language, an equation as display math, a table as itself, with the
 * user's note after it.
 */
export function visualPrintMarkdown(visual: { kind: VisualKind; source: string; note?: string | null }): string {
  const source = visual.source.trim();
  const fence = KIND_FENCE[visual.kind];
  const body = fence ? ['```' + fence, source, '```'].join('\n') : visual.kind === 'equation' ? ['$$', source, '$$'].join('\n') : source;
  const note = visual.note?.trim();
  return note ? `${body}\n\n${note}` : body;
}

/** Search hits standing in for the printed sources, so the renderer resolves `[n]`. */
export function hitsFromSources(sources: readonly PrintSource[]): SearchHit[] {
  return sources.map(s => ({
    number: s.n,
    sourceFile: s.url ?? s.title,
    fileName: s.title,
    title: s.title,
    text: '',
    snippet: '',
    score: 0,
    page: null,
    lineRange: null,
    url: s.url,
  }));
}

/** Element id of a source in the printed Sources list (citation links point here). */
export function sourceAnchor(n: number): string {
  return `print-source-${n}`;
}

/** One line of the printed Sources list. */
export function sourceLine(source: PrintSource): string {
  const parts = [source.title];
  if (source.location) parts.push(source.location);
  return parts.join(', ');
}

/** The date printed under the title, in the reader's locale. */
export function printDate(createdAt: string, locale?: string): string {
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleDateString(locale, { year: 'numeric', month: 'long', day: 'numeric' });
}

/**
 * Controls never printed: buttons (Expand & ask, copy, simulation and zoom
 * controls), form fields (plot and simulation sliders), toolbars and
 * anything marked `data-print-hide`.
 */
export const PRINT_HIDDEN_SELECTORS = [
  'button',
  'input',
  'select',
  'textarea',
  '[role="toolbar"]',
  '[role="slider"]',
  '[data-print-hide]',
] as const;

/** Blocks a page break must not cut through. */
export const PRINT_UNBREAKABLE_SELECTORS = [
  'figure',
  'table',
  'tr',
  'svg',
  'img',
  'pre',
  '.katex-display',
  '[data-print-block]',
] as const;

/** The print stylesheet of the print view. */
export function printStylesheet(): string {
  return [
    `${PRINT_HIDDEN_SELECTORS.map(s => `.print-view ${s}`).join(', ')} { display: none !important; }`,
    `${PRINT_UNBREAKABLE_SELECTORS.map(s => `.print-view ${s}`).join(', ')} { break-inside: avoid; page-break-inside: avoid; }`,
    '.print-view h1, .print-view h2, .print-view h3, .print-view h4 { break-after: avoid; page-break-after: avoid; }',
    '.print-view .overflow-x-auto, .print-view .overflow-hidden, .print-view .overflow-auto { overflow: visible !important; }',
    '.print-view pre, .print-view code { white-space: pre-wrap !important; word-break: break-word; }',
    '.print-view svg, .print-view img, .print-view canvas { max-width: 100% !important; height: auto; }',
    '.print-view a.print-cite { color: inherit; text-decoration: none; font-size: 0.8em; vertical-align: super; }',
    '@media print { html, body { background: #ffffff !important; } .print-screen-only { display: none !important; } }',
  ].join('\n');
}
