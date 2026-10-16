/**
 * "Go deeper" marks: questions the model prepares on parts of its answer. A
 * mark is a `deeper` string on a visual's element (a diagram node, a chart
 * series, a plot or simulation parameter, a derivation step, a symbol), or
 * `[[shown text|question]]` in prose. Clicking one opens a nested window that
 * asks the question; nothing is sent before the click.
 *
 * An answer shows at most MAX_MARKS marks, the first ones in document order
 * (prose and visuals together); the rest render as plain text or no button.
 * Marks in inline or fenced code are code, not marks.
 *
 * Pure module, unit-tested with Node (`app/tests/visualMarks.test.ts`).
 */

export const MAX_MARKS = 4;
export const MAX_QUESTION_CHARS = 160;
export const MAX_SHOWN_CHARS = 80;
const MAX_LABEL_CHARS = 60;

/** Fenced languages whose elements may carry `deeper`. */
export const MARK_LANGUAGES: ReadonlySet<string> = new Set(['diagram', 'chart', 'plot', 'simulation', 'derivation', 'symbols']);

export interface Mark {
  /** What the mark is on (a node's label, a series, "Step 2"). */
  label: string;
  question: string;
}

export interface TextMark {
  start: number;
  end: number;
  shown: string;
  /** Null when the question is unusable; the shown text is then plain. */
  question: string | null;
}

/**
 * `[[shown|question]]`: shown 1–80 characters, no brackets or line breaks. The
 * question part matches up to 400 characters so an overlong one is still a
 * mark (shown as plain text, never as syntax); readDeeper drops it past 160.
 */
const TEXT_MARK = /\[\[([^[\]|\n]{1,80})\|([^[\]\n]{0,400})\]\]/g;
const FENCE = /^```([\w-]*)[^\n]*\n([\s\S]*?)^```[ \t]*$/gm;
const INLINE_CODE = /`[^`\n]*`/g;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function flat(value: unknown, max: number): string | null {
  if (typeof value !== 'string') return null;
  const text = value.replace(/\s+/g, ' ').trim();
  if (!text) return null;
  const chars = Array.from(text);
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : text;
}

/** A prepared question: flattened, non-empty, at most MAX_QUESTION_CHARS (longer ones are dropped). */
export function readDeeper(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const text = value.replace(/\s+/g, ' ').trim();
  if (!text || Array.from(text).length > MAX_QUESTION_CHARS) return null;
  return text;
}

/** Prose marks of `text`, in order. Code is not excluded here: callers pass prose. */
export function findTextMarks(text: string): TextMark[] {
  const out: TextMark[] = [];
  for (const m of text.matchAll(TEXT_MARK)) {
    const start = m.index ?? 0;
    const shown = m[1].trim();
    if (!shown) continue;
    out.push({ start, end: start + m[0].length, shown, question: readDeeper(m[2]) });
  }
  return out;
}

/** Ranges of inline code spans in `text`. */
function codeRanges(text: string): [number, number][] {
  return Array.from(text.matchAll(INLINE_CODE), m => [m.index ?? 0, (m.index ?? 0) + m[0].length] as [number, number]);
}

/** `text` with every complete prose mark (outside inline code) replaced by its shown text. */
export function stripTextMarks(text: string): string {
  const code = codeRanges(text);
  let out = '';
  let last = 0;
  for (const mark of findTextMarks(text)) {
    if (code.some(([s, e]) => mark.start >= s && mark.start < e)) continue;
    out += text.slice(last, mark.start) + mark.shown;
    last = mark.end;
  }
  return out + text.slice(last);
}

/**
 * `text` with its first `allowed` usable prose marks (outside inline code)
 * replaced by `placeholder(i)` and the other marks by their shown text;
 * marks inside inline code stay as written.
 */
export function placeTextMarks(
  text: string,
  allowed: number,
  placeholder: (index: number) => string,
): { text: string; marks: { shown: string; question: string }[] } {
  const code = codeRanges(text);
  const marks: { shown: string; question: string }[] = [];
  let out = '';
  let last = 0;
  for (const mark of findTextMarks(text)) {
    if (code.some(([s, e]) => mark.start >= s && mark.start < e)) continue;
    out += text.slice(last, mark.start);
    if (mark.question !== null && marks.length < allowed) {
      out += placeholder(marks.length);
      marks.push({ shown: mark.shown, question: mark.question });
    } else {
      out += mark.shown;
    }
    last = mark.end;
  }
  return { text: out + text.slice(last), marks };
}

function list(value: unknown, key: string): unknown[] {
  if (Array.isArray(value)) return value;
  if (isRecord(value) && Array.isArray(value[key])) return value[key] as unknown[];
  return [];
}

/** The marks of one fenced block, in element order. Unknown languages and invalid JSON have none. */
export function blockMarks(lang: string, source: string): Mark[] {
  if (!MARK_LANGUAGES.has(lang)) return [];
  let raw: unknown;
  try {
    raw = JSON.parse(source);
  } catch {
    return [];
  }
  const entries: { label: string | null; deeper: unknown }[] = [];
  if (lang === 'diagram') {
    for (const n of list(raw, 'nodes')) if (isRecord(n)) entries.push({ label: flat(n.label, MAX_LABEL_CHARS) ?? flat(n.id, MAX_LABEL_CHARS), deeper: n.deeper });
  } else if (lang === 'chart') {
    const series = isRecord(raw) && Array.isArray(raw.series)
      ? raw.series
      : isRecord(raw) && isRecord(raw.data) && Array.isArray(raw.data.datasets) ? raw.data.datasets : [];
    for (const s of series) if (isRecord(s)) entries.push({ label: flat(s.label, MAX_LABEL_CHARS) ?? flat(s.key, MAX_LABEL_CHARS), deeper: s.deeper });
  } else if (lang === 'plot' || lang === 'simulation') {
    for (const p of isRecord(raw) && Array.isArray(raw.params) ? raw.params : []) {
      if (isRecord(p)) entries.push({ label: flat(p.label, MAX_LABEL_CHARS) ?? flat(p.name, MAX_LABEL_CHARS), deeper: p.deeper });
    }
  } else if (lang === 'derivation') {
    list(raw, 'steps').forEach((s, i) => {
      if (isRecord(s)) entries.push({ label: `Step ${i + 1}`, deeper: s.deeper });
    });
  } else if (lang === 'symbols') {
    for (const s of list(raw, 'symbols')) if (isRecord(s)) entries.push({ label: flat(s.symbol ?? s.latex, MAX_LABEL_CHARS), deeper: s.deeper });
  }
  const out: Mark[] = [];
  for (const e of entries) {
    const question = readDeeper(e.deeper);
    if (question && e.label) out.push({ label: e.label, question });
  }
  return out;
}

export interface MarkBudget {
  /** How many prose marks (in order, outside code) render as buttons. */
  text: number;
  /** Fence body (trimmed) to how many of its marks render; repeated identical blocks get 0. */
  blocks: Map<string, number>;
}

/** Which marks of an answer render: the first MAX_MARKS in document order. */
export function markBudget(content: string): MarkBudget {
  // First pass: count occurrences of each trimmed fence body.
  const bodyOccurrences = new Map<string, number>();
  for (const m of content.matchAll(FENCE)) {
    const body = m[2].replace(/\n$/, '').trim();
    bodyOccurrences.set(body, (bodyOccurrences.get(body) ?? 0) + 1);
  }

  const blocks = new Map<string, number>();
  let left = MAX_MARKS;
  let text = 0;
  const prose = (part: string) => {
    const withoutCode = part.replace(INLINE_CODE, m => ' '.repeat(m.length));
    const usable = findTextMarks(withoutCode).filter(m => m.question !== null).length;
    const allowed = Math.min(usable, left);
    text += allowed;
    left -= allowed;
  };
  let last = 0;
  for (const m of content.matchAll(FENCE)) {
    const start = m.index ?? 0;
    prose(content.slice(last, start));
    last = start + m[0].length;
    const body = m[2].replace(/\n$/, '').trim();
    const count = blockMarks(m[1].toLowerCase(), body).length;
    if (count > 0 && !blocks.has(body)) {
      // Repeated identical blocks get no marks so the 4-per-answer cap holds.
      if ((bodyOccurrences.get(body) ?? 0) > 1) {
        blocks.set(body, 0);
      } else {
        const allowed = Math.min(count, left);
        blocks.set(body, allowed);
        left -= allowed;
      }
    }
  }
  prose(content.slice(last));
  return { text, blocks };
}
