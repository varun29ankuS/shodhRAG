/**
 * Visual blocks of an answer, as the gallery records them: ```mermaid (and
 * the other diagram fences), ```chart, ```svg, ```plot, ```simulation,
 * display equations ($$…$$, \[…\]) and Markdown tables.
 *
 * Detection mirrors the answer renderer (MessageContentRenderer): the same
 * fence languages, `mermaidSource` for bare diagram fences, display math
 * found after `normalizeMathDelimiters`, and the spec parsers each visual
 * draws with. A block the renderer would show as an error carries a
 * `problem` and is never recorded.
 *
 * Titles come from the block itself (its title / <title>), else a caption
 * line next to it, else the nearest heading above it, else a first label
 * (a diagram's first node, a table's header), else the kind's noun.
 *
 * Pure module, unit-tested with Node (`app/tests/visualGallery.test.ts`).
 */

import { isMermaidLanguage, mermaidSource, normalizeMathDelimiters } from '../ask/visual/mathText.ts';
import { parseChartBlock } from '../ask/visual/chartSpec.ts';
import { parsePlotSpec, PLOT_MAX_CHARS } from '../ask/visual/plotSpec.ts';
import { parseSimulationSpec, SIMULATION_MAX_CHARS } from '../ask/visual/simulationSpec.ts';
import { sanitizeSvg, SVG_MAX_CHARS } from '../ask/visual/svgSanitize.ts';
import { mermaidTarget } from '../focus/targets.ts';

export const VISUAL_KINDS = ['mermaid', 'chart', 'svg', 'plot', 'simulation', 'equation', 'table'] as const;
export type VisualKind = typeof VISUAL_KINDS[number];

/** Longest source the gallery keeps (the backend's cap). */
export const MAX_VISUAL_SOURCE_CHARS = 100_000;
/** Longest title, in characters (the backend's cap). */
export const MAX_VISUAL_TITLE_CHARS = 120;
/** Most blocks recorded from one answer (the backend's cap). */
export const MAX_BLOCKS_PER_ANSWER = 64;

/** Characters of a caption or heading used as a title. */
const TITLE_CHARS = 80;

export const KIND_NOUN: Record<VisualKind, string> = {
  mermaid: 'Diagram',
  chart: 'Chart',
  svg: 'Sketch',
  plot: 'Plot',
  simulation: 'Simulation',
  equation: 'Equation',
  table: 'Table',
};

/** The fence language a kind is written with (equations and tables are not fenced). */
export const KIND_FENCE: Record<VisualKind, string | null> = {
  mermaid: 'mermaid',
  chart: 'chart',
  svg: 'svg',
  plot: 'plot',
  simulation: 'simulation',
  equation: null,
  table: null,
};

export function isVisualKind(value: unknown): value is VisualKind {
  return typeof value === 'string' && (VISUAL_KINDS as readonly string[]).includes(value);
}

export interface ExtractedVisual {
  kind: VisualKind;
  title: string;
  /** As stored: diagram header included, TeX without delimiters, the table's Markdown. */
  source: string;
  /** Why the renderer would not draw it (never recorded); absent when it draws. */
  problem?: string;
}

/**
 * Source with unified line endings, trailing spaces of each line and blank
 * lines at both ends removed. Same rule as the backend's content hash
 * (`shodh_rag::visuals::normalize_source`).
 */
export function normalizeSource(source: string): string {
  const lines = source.replace(/\r\n?/g, '\n').split('\n').map(l => l.replace(/\s+$/, ''));
  let start = 0;
  while (start < lines.length && lines[start] === '') start += 1;
  let end = lines.length;
  while (end > start && lines[end - 1] === '') end -= 1;
  return lines.slice(start, end).join('\n');
}

/** Identity of a block for deduplication: kind and normalised source. */
export function visualKey(kind: VisualKind, source: string): string {
  return `${kind}\n${normalizeSource(source)}`;
}

/** Blocks without problems, each (kind, normalised source) once, at most `max`. */
export function capturable(blocks: readonly ExtractedVisual[], max = MAX_BLOCKS_PER_ANSWER): ExtractedVisual[] {
  const seen = new Set<string>();
  const out: ExtractedVisual[] = [];
  for (const block of blocks) {
    if (block.problem) continue;
    const key = visualKey(block.kind, block.source);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({ kind: block.kind, title: block.title, source: normalizeSource(block.source) });
    if (out.length >= max) break;
  }
  return out;
}

function flat(text: string): string {
  return text.replace(/\s+/g, ' ').trim();
}

function short(text: string, max = TITLE_CHARS): string {
  const chars = Array.from(flat(text));
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : chars.join('');
}

/** Inline Markdown removed from a caption or heading. */
function plainInline(text: string): string {
  return flat(
    text
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
      .replace(/\[(?:\d+(?:\s*,\s*\d+)*)\]/g, '')
      .replace(/`([^`]*)`/g, '$1')
      .replace(/(\*\*|__)(.+?)\1/g, '$2')
      .replace(/(^|[^\w*])[*_]([^*_]+)[*_](?=$|[^\w*])/g, '$1$2'),
  );
}

/** "Figure 2: Forces" → "Forces". */
function stripCaptionPrefix(text: string): string {
  const stripped = text.replace(/^(?:figure|fig\.|table|diagram|chart|plot|sketch|equation|simulation)\s*\d*(?:\.\d+)*\s*[:.\-–—]\s*/i, '');
  return stripped.trim() ? stripped.trim() : text.trim();
}

const CAPTION_WORD = /^(?:figure|fig\.|table|diagram|chart|plot|sketch|equation|simulation)\b/i;
const HEADING = /^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$/;

/**
 * A caption written next to a block: a short line just above that is all
 * bold/italic, ends with ":", or starts with "Figure", "Table", …; or a
 * "Figure…"/"Table…" line (possibly emphasised) just below.
 */
function captionAround(lines: readonly string[], start: number, end: number): string | null {
  const isEmphasised = (l: string) => /^\s*(\*\*|__|\*|_)[^*_].*\1\s*:?\s*$/.test(l);
  for (let i = start - 1, seen = 0; i >= 0 && seen < 2; i--) {
    const line = lines[i].trim();
    if (!line) {
      seen += 1;
      continue;
    }
    if (HEADING.test(line) || /^[-*+]\s|^\d+\.\s|^\|/.test(line)) break;
    const text = plainInline(line);
    if (text && Array.from(text).length <= 100 && (isEmphasised(line) || /:\s*$/.test(text) || CAPTION_WORD.test(text))) {
      const caption = stripCaptionPrefix(text.replace(/:\s*$/, ''));
      if (caption) return caption;
    }
    break;
  }
  for (let i = end + 1, seen = 0; i < lines.length && seen < 2; i++) {
    const line = lines[i].trim();
    if (!line) {
      seen += 1;
      continue;
    }
    const text = plainInline(line);
    if (text && Array.from(text).length <= 100 && CAPTION_WORD.test(text)) {
      const caption = stripCaptionPrefix(text);
      if (caption) return caption;
    }
    break;
  }
  return null;
}

/** The nearest Markdown heading above line `index`. */
function headingAbove(lines: readonly string[], index: number): string | null {
  for (let i = index - 1; i >= 0; i--) {
    const m = HEADING.exec(lines[i]);
    if (m) {
      const text = plainInline(m[1]);
      if (text) return text;
    }
  }
  return null;
}

function jsonTitle(source: string): string {
  try {
    const parsed = JSON.parse(source) as { title?: unknown };
    return typeof parsed.title === 'string' ? flat(parsed.title) : '';
  } catch {
    return '';
  }
}

function svgTitle(source: string): string {
  const m = /<title(?:\s[^>]*)?>([\s\S]*?)<\/title>/i.exec(source);
  return m ? flat(m[1].replace(/<[^>]*>/g, '').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&amp;/g, '&')) : '';
}

/** A diagram's own title (`title …`), else its first node label ("A[Start]" → "Start"). */
function mermaidLabels(source: string): { own: string; first: string } {
  const own = /^\s*title\s+(.+)$/im.exec(source)?.[1] ?? '';
  const node = /[A-Za-z0-9_]+\s*(?:\[\[?|\(\(?|\{\{?|>)\s*"?([^"\]\)\}\n]{2,60}?)"?\s*(?:\]\]?|\)\)?|\}\}?)/.exec(source);
  return { own: flat(own), first: node ? flat(node[1]) : '' };
}

/** Cells of one Markdown table row; escaped pipes stay in the cell. */
export function tableCells(line: string): string[] {
  let row = line.trim();
  if (row.startsWith('|')) row = row.slice(1);
  if (row.endsWith('|') && !row.endsWith('\\|')) row = row.slice(0, -1);
  const cells: string[] = [];
  let cell = '';
  for (let i = 0; i < row.length; i++) {
    const ch = row[i];
    if (ch === '\\' && row[i + 1] === '|') {
      cell += '|';
      i += 1;
    } else if (ch === '|') {
      cells.push(cell);
      cell = '';
    } else {
      cell += ch;
    }
  }
  cells.push(cell);
  return cells.map(c => plainInline(c));
}

const DELIMITER_ROW = /^\s*\|?\s*:?-{1,}:?\s*(?:\|\s*:?-{1,}:?\s*)*\|?\s*$/;

/** Rows of a Markdown table (header first, delimiter row left out). */
export function markdownTableRows(source: string): string[][] {
  const lines = normalizeSource(source).split('\n').filter(l => l.trim());
  if (lines.length < 2 || !DELIMITER_ROW.test(lines[1])) return [];
  return [lines[0], ...lines.slice(2)].map(tableCells);
}

function equationLabel(tex: string): string {
  return `Equation ${short(tex, 40)}`;
}

interface Located {
  kind: VisualKind;
  source: string;
  /** First and last line of the block in the answer. */
  start: number;
  end: number;
  own: string;
  firstLabel: string;
  problem?: string;
}

const FENCE_OPEN = /^\s{0,3}(`{3,}|~{3,})\s*([^\s`]*)[^`]*$/;

/** A fenced block's kind and stored source, or null when it is not a visual. */
function fencedVisual(lang: string, body: string): Omit<Located, 'start' | 'end'> | null {
  const language = lang.toLowerCase();
  if (isMermaidLanguage(language)) {
    const source = mermaidSource(language, body.trim());
    const labels = mermaidLabels(source);
    const generic = mermaidTarget(source).label;
    return { kind: 'mermaid', source, own: labels.own, firstLabel: labels.first || generic };
  }
  if (language === 'chart') {
    const parsed = parseChartBlock(body);
    return {
      kind: 'chart',
      source: body,
      own: 'chart' in parsed ? flat(parsed.chart.title ?? '') : jsonTitle(body),
      firstLabel: '',
      ...('error' in parsed ? { problem: parsed.error } : {}),
    };
  }
  if (language === 'svg') {
    const problem = body.length > SVG_MAX_CHARS ? 'The sketch is larger than the drawing limit.' : (() => {
      const r = sanitizeSvg(body);
      return 'error' in r ? r.error : undefined;
    })();
    return { kind: 'svg', source: body, own: svgTitle(body), firstLabel: '', ...(problem ? { problem } : {}) };
  }
  if (language === 'plot') {
    const problem = body.length > PLOT_MAX_CHARS ? 'The plot spec is larger than the drawing limit.' : (() => {
      const r = parsePlotSpec(body);
      return 'error' in r ? r.error : undefined;
    })();
    return { kind: 'plot', source: body, own: jsonTitle(body), firstLabel: '', ...(problem ? { problem } : {}) };
  }
  if (language === 'simulation') {
    const problem = body.length > SIMULATION_MAX_CHARS ? 'The simulation spec is larger than the drawing limit.' : (() => {
      const r = parseSimulationSpec(body);
      return 'error' in r ? r.error : undefined;
    })();
    return { kind: 'simulation', source: body, own: jsonTitle(body), firstLabel: '', ...(problem ? { problem } : {}) };
  }
  return null;
}

/** Display equations and tables in prose lines `[from, to)` of `lines`. */
function proseVisuals(lines: readonly string[], from: number, to: number, out: Located[]): void {
  let i = from;
  while (i < to) {
    const line = lines[i];
    // Display math: $$ opening a line, closed by $$ ending a line.
    const open = /^\s*\$\$(.*)$/.exec(line);
    if (open) {
      const rest = open[1];
      const sameLine = /^(.*?)\$\$\s*$/.exec(rest);
      if (sameLine && sameLine[1].trim()) {
        out.push({ kind: 'equation', source: sameLine[1].trim(), start: i, end: i, own: '', firstLabel: equationLabel(sameLine[1]) });
        i += 1;
        continue;
      }
      const body: string[] = rest.trim() ? [rest] : [];
      let j = i + 1;
      let closed = false;
      while (j < to) {
        const close = /^(.*?)\$\$\s*$/.exec(lines[j]);
        if (close) {
          if (close[1].trim()) body.push(close[1]);
          closed = true;
          break;
        }
        body.push(lines[j]);
        j += 1;
      }
      if (closed) {
        const tex = body.join('\n').trim();
        if (tex) out.push({ kind: 'equation', source: tex, start: i, end: j, own: '', firstLabel: equationLabel(tex) });
        i = j + 1;
        continue;
      }
    }
    // GFM table: header row, delimiter row, body rows.
    if (line.includes('|') && i + 1 < to && DELIMITER_ROW.test(lines[i + 1]) && lines[i + 1].includes('-')) {
      const header = tableCells(line);
      const columns = tableCells(lines[i + 1]).length;
      if (header.length === columns || header.length > 1) {
        let j = i + 2;
        while (j < to && lines[j].trim() && lines[j].includes('|')) j += 1;
        const source = lines.slice(i, j).map(l => l.trim()).join('\n');
        const named = header.filter(c => c.trim()).join(', ');
        out.push({ kind: 'table', source, start: i, end: j - 1, own: '', firstLabel: named ? `Table: ${short(named, 50)}` : '' });
        i = j;
        continue;
      }
    }
    i += 1;
  }
}

/**
 * Every visual block of `markdown`, in order. Unclosed fences (an answer cut
 * off mid-block) are ignored.
 */
export function extractVisualBlocks(markdown: string): ExtractedVisual[] {
  const raw = markdown.replace(/\r\n?/g, '\n').split('\n');
  // The answer as lines, with math delimiters normalised in prose only (as
  // the renderer does: code is protected first). Block positions index it.
  const lines: string[] = [];
  const located: Located[] = [];
  const addProse = (from: number, to: number) => {
    if (to <= from) return;
    const start = lines.length;
    lines.push(...normalizeMathDelimiters(raw.slice(from, to).join('\n')).split('\n'));
    proseVisuals(lines, start, lines.length, located);
  };
  let prose = 0;
  let i = 0;
  while (i < raw.length) {
    const open = FENCE_OPEN.exec(raw[i]);
    if (!open) {
      i += 1;
      continue;
    }
    const marker = open[1];
    let closeAt = -1;
    for (let j = i + 1; j < raw.length; j++) {
      const m = /^\s{0,3}(`{3,}|~{3,})\s*$/.exec(raw[j]);
      if (m && m[1][0] === marker[0] && m[1].length >= marker.length) {
        closeAt = j;
        break;
      }
    }
    // An unclosed fence (an answer cut off mid-block) and everything after it are not drawn.
    if (closeAt < 0) break;
    addProse(prose, i);
    const start = lines.length;
    lines.push(...raw.slice(i, closeAt + 1));
    const visual = fencedVisual(open[2], raw.slice(i + 1, closeAt).join('\n'));
    if (visual) located.push({ ...visual, start, end: lines.length - 1 });
    i = closeAt + 1;
    prose = i;
  }
  addProse(prose, i);

  return located.map(block => {
    const title = block.own
      || captionAround(lines, block.start, block.end)
      || headingAbove(lines, block.start)
      || block.firstLabel
      || KIND_NOUN[block.kind];
    const source = normalizeSource(block.source);
    const tooLong = Array.from(source).length > MAX_VISUAL_SOURCE_CHARS;
    const problem = block.problem ?? (tooLong ? 'The block is larger than the gallery keeps.' : !source ? 'The block is empty.' : undefined);
    return { kind: block.kind, title: short(title, MAX_VISUAL_TITLE_CHARS), source, ...(problem ? { problem } : {}) };
  });
}
