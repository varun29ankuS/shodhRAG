/**
 * Pure logic of the Tables tab: reading a cell's value under its column's kind, sorting
 * and filtering rows, cell marks and staleness, arrow-key movement in the grid, the
 * quote's matched words, where a cell opens in its paper, and the preset columns.
 *
 * Type-only imports apart from the pure region parser, so `node --test` loads it directly
 * (`app/tests/tablesModel.test.ts`).
 */

import { firstRegionPage, parseRegions } from '../ask/viewer/regionGeometry.ts';
import type { PdfRegion } from '../ask/types.ts';
import type { Cell, CellValue, Column, ColumnKind, ColumnSpec, RowView, TableRow } from './types.ts';

/** Longest `text` value, in characters (mirrors `MAX_TEXT_CHARS`). */
export const MAX_TEXT_CHARS = 500;
/** Options of a pick-list column (mirrors `MIN_OPTIONS`/`MAX_OPTIONS`). */
export const MIN_OPTIONS = 2;
export const MAX_OPTIONS = 12;
/** Most columns of a table (mirrors `MAX_COLUMNS`). */
export const MAX_COLUMNS = 22;
/** Longest question, instructions and option (mirror `research::tables`). */
export const MAX_QUESTION_CHARS = 300;
export const MAX_INSTRUCTIONS_CHARS = 1000;
export const MAX_OPTION_CHARS = 80;
/** Longest table name (mirrors `MAX_NAME_CHARS`). */
export const MAX_NAME_CHARS = 120;
/** Longest value typed into a cell (mirrors `MAX_EDIT_CHARS`). */
export const MAX_EDIT_CHARS = 2000;

export const KIND_LABELS: Record<ColumnKind, string> = {
  text: 'Text',
  number: 'Number',
  yes_no: 'Yes / no',
  choice: 'Pick-list',
};

/** Whitespace runs as one space, trimmed (`collapse`). */
export function collapse(text: string): string {
  return text.split(/\s+/u).filter(part => part.length > 0).join(' ');
}

function chars(text: string): number {
  return Array.from(text).length;
}

/** The value as one line of text (`CellValue::display`). */
export function valueText(value: CellValue | null): string {
  if (!value) return '';
  switch (value.kind) {
    case 'text':
      return value.text;
    case 'number':
      return value.unit ? `${value.lexeme} ${value.unit}` : value.lexeme;
    case 'yes_no':
      return value.yes ? 'yes' : 'no';
    case 'choice':
      return value.option;
    default:
      return '';
  }
}

/** A number as an answer states it: value, then an optional unit without digits (`ANSWER_NUMBER`). */
const ANSWER_NUMBER =
  /^(?<v>[+\-−–]?(?:\p{Nd}{1,3}(?:,\p{Nd}{3})+|\p{Nd}+)(?:\.\p{Nd}+)?|[+\-−–]?\.\p{Nd}+)\s*(?<u>[^\p{Nd}\s][^\p{Nd}]{0,23})?$/u;

/** The digits of a number without sign or thousands separators (`unsigned_decimal`). */
function unsignedDecimal(lexeme: string): string {
  const digits = lexeme.replace(/^[+\-−–]+/u, '').replace(/,/g, '');
  return digits.startsWith('.') ? `0${digits}` : digits;
}

function readNumber(text: string): CellValue | null {
  const match = ANSWER_NUMBER.exec(text.trim());
  if (!match || !match.groups) return null;
  const lexeme = match.groups.v;
  if (typeof lexeme !== 'string') return null;
  const negative = /^[-−–]/u.test(lexeme);
  const unsigned = unsignedDecimal(lexeme);
  const unitText = typeof match.groups.u === 'string' ? collapse(match.groups.u) : '';
  return { kind: 'number', lexeme, decimal: negative ? `-${unsigned}` : unsigned, unit: unitText || null };
}

/** The value `text` gives under `spec`'s type rule, or null (`validate::typed_value` on a string). */
export function typedValue(spec: Pick<ColumnSpec, 'kind' | 'options'>, text: string): CellValue | null {
  switch (spec.kind) {
    case 'text': {
      const value = collapse(text);
      if (!value || chars(value) > MAX_TEXT_CHARS) return null;
      return { kind: 'text', text: value };
    }
    case 'yes_no': {
      const word = text.trim().toLowerCase();
      if (word === 'yes') return { kind: 'yes_no', yes: true };
      if (word === 'no') return { kind: 'yes_no', yes: false };
      return null;
    }
    case 'choice': {
      const folded = collapse(text).toLowerCase();
      const option = (spec.options ?? []).find(o => o.toLowerCase() === folded);
      return option !== undefined ? { kind: 'choice', option } : null;
    }
    case 'number':
      return readNumber(text);
    default:
      return null;
  }
}

/**
 * A stored value read under its column's current kind, or null when it does not fit
 * (`CellValue::in_kind`: `typed_value(spec, display())`). A cell keeps the variant it was
 * filled or typed with when its column changes, so every typed sort, filter and aggregate
 * reads values through this, never `value` directly.
 */
export function inKind(value: CellValue | null, column: Pick<ColumnSpec, 'kind' | 'options'>): CellValue | null {
  if (!value) return null;
  return typedValue(column, valueText(value));
}

/** A cell's value under its column's kind; null for no value or one flagged as no longer fitting. */
export function cellValue(cell: Cell | undefined, column: Pick<ColumnSpec, 'kind' | 'options'>): CellValue | null {
  if (!cell || !cell.value) return null;
  if (cell.error === 'value_no_longer_fits') return null;
  return inKind(cell.value, column);
}

export function cellKey(rowId: string, columnId: string): string {
  return `${rowId}\u0000${columnId}`;
}

/** The cells by row and column. */
export function cellIndex(cells: readonly Cell[]): Map<string, Cell> {
  const index = new Map<string, Cell>();
  for (const cell of cells) index.set(cellKey(cell.rowId, cell.columnId), cell);
  return index;
}

// ── Sorting and filtering ───────────────────────────────────────────

export type SortDir = 'asc' | 'desc';

/** What a sort compares, typed: numbers numerically (units ignored), text case-blind. */
type SortKey = { kind: 'number'; n: number } | { kind: 'text'; s: string } | { kind: 'rank'; n: number };

function sortKey(value: CellValue, column: Pick<ColumnSpec, 'kind' | 'options'>): SortKey | null {
  switch (value.kind) {
    case 'number': {
      const n = Number(value.decimal);
      return Number.isFinite(n) ? { kind: 'number', n } : null;
    }
    case 'yes_no':
      return { kind: 'rank', n: value.yes ? 1 : 0 };
    case 'choice':
      return { kind: 'rank', n: (column.options ?? []).indexOf(value.option) };
    case 'text':
      return { kind: 'text', s: value.text };
    default:
      return null;
  }
}

const TEXT_ORDER = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });

function compareKeys(a: SortKey, b: SortKey): number {
  if (a.kind === 'text' && b.kind === 'text') return TEXT_ORDER.compare(a.s, b.s);
  if (a.kind !== 'text' && b.kind !== 'text') return a.n - b.n;
  return 0;
}

/**
 * The rows sorted by a column (`null`: by the paper's title). Values are read through
 * `inKind`; a row without a value under the column's kind goes last in both directions.
 * Ties keep the table's order.
 */
export function sortRows<R extends TableRow & { title?: string }>(
  rows: readonly R[],
  cells: ReadonlyMap<string, Cell>,
  column: Column | null,
  dir: SortDir,
): R[] {
  const sign = dir === 'asc' ? 1 : -1;
  const keyed = rows.map(row => {
    let key: SortKey | null;
    if (column === null) {
      key = { kind: 'text', s: row.title ?? '' };
    } else {
      const value = cellValue(cells.get(cellKey(row.id, column.id)), column);
      key = value ? sortKey(value, column) : null;
    }
    return { row, key };
  });
  keyed.sort((a, b) => {
    if (a.key === null || b.key === null) {
      if (a.key === null && b.key === null) return a.row.position - b.row.position;
      return a.key === null ? 1 : -1;
    }
    return sign * compareKeys(a.key, b.key) || a.row.position - b.row.position;
  });
  return keyed.map(k => k.row);
}

/** The marks a cell can carry in the grid. */
export type CellMark = 'unverified' | 'not_found' | 'edited' | 'failed';

/** A cell's mark, or null for a verified, empty or queued cell. */
export function cellMark(cell: Pick<Cell, 'status'> | undefined): CellMark | null {
  if (!cell) return null;
  switch (cell.status) {
    case 'unverified':
    case 'not_found':
    case 'edited':
    case 'failed':
      return cell.status;
    default:
      return null;
  }
}

/** A row filter: by `yes_no`/`choice` values (read through `inKind`), or by cell status. */
export type RowFilter =
  | { kind: 'value'; columnId: string; values: readonly string[] }
  | { kind: 'status'; statuses: readonly Cell['status'][] };

/** The text a `yes_no` or `choice` value is filtered by ("yes", "no", or the option). */
export function filterValue(value: CellValue | null): string | null {
  if (!value) return null;
  if (value.kind === 'yes_no') return value.yes ? 'yes' : 'no';
  if (value.kind === 'choice') return value.option;
  return null;
}

/**
 * The rows every filter keeps. A value filter keeps rows whose cell, read under the
 * column's kind, is one of its values (a value that no longer fits never matches); a
 * status filter keeps rows with at least one cell in one of its statuses. An empty filter
 * keeps everything.
 */
export function filterRows<R extends TableRow>(
  rows: readonly R[],
  cells: ReadonlyMap<string, Cell>,
  columns: readonly Column[],
  filters: readonly RowFilter[],
): R[] {
  return rows.filter(row =>
    filters.every(filter => {
      if (filter.kind === 'value') {
        if (filter.values.length === 0) return true;
        const column = columns.find(c => c.id === filter.columnId);
        if (!column) return true;
        const value = filterValue(cellValue(cells.get(cellKey(row.id, column.id)), column));
        return value !== null && filter.values.includes(value);
      }
      if (filter.statuses.length === 0) return true;
      return columns.some(column => {
        const status = cells.get(cellKey(row.id, column.id))?.status ?? 'empty';
        return filter.statuses.includes(status);
      });
    }),
  );
}

/**
 * Whether a filled cell was answered for an older version of its column's question (its
 * `columnRevision` is behind): still shown, dimmed, and filled again on the next run.
 * Edited cells are the user's and never stale.
 */
export function isStale(cell: Pick<Cell, 'status' | 'columnRevision'> | undefined, column: Pick<Column, 'revision'>): boolean {
  if (!cell) return false;
  if (cell.status !== 'verified' && cell.status !== 'unverified' && cell.status !== 'not_found') return false;
  return typeof cell.columnRevision === 'number' && cell.columnRevision < column.revision;
}

// ── Keyboard movement ───────────────────────────────────────────────

/** A body cell of the grid: `col` 0 is the paper column, then the table's columns. */
export interface GridPos {
  row: number;
  col: number;
}

/** How far PageUp/PageDown move. */
export const PAGE_ROWS = 10;

/**
 * Where a key moves the focus (arrows, Home/End, Ctrl+Home/End, PageUp/PageDown), clamped
 * to the grid's edges; null for any other key or an empty grid.
 */
export function nextCell(pos: GridPos, key: string, dims: { rows: number; cols: number }, ctrl = false): GridPos | null {
  if (dims.rows <= 0 || dims.cols <= 0) return null;
  const clamp = (row: number, col: number): GridPos => ({
    row: Math.min(Math.max(row, 0), dims.rows - 1),
    col: Math.min(Math.max(col, 0), dims.cols - 1),
  });
  switch (key) {
    case 'ArrowUp':
      return clamp(pos.row - 1, pos.col);
    case 'ArrowDown':
      return clamp(pos.row + 1, pos.col);
    case 'ArrowLeft':
      return clamp(pos.row, pos.col - 1);
    case 'ArrowRight':
      return clamp(pos.row, pos.col + 1);
    case 'Home':
      return ctrl ? clamp(0, 0) : clamp(pos.row, 0);
    case 'End':
      return ctrl ? clamp(dims.rows - 1, dims.cols - 1) : clamp(pos.row, dims.cols - 1);
    case 'PageUp':
      return clamp(pos.row - PAGE_ROWS, pos.col);
    case 'PageDown':
      return clamp(pos.row + PAGE_ROWS, pos.col);
    default:
      return null;
  }
}

// ── The quote ───────────────────────────────────────────────────────

export interface QuoteSegment {
  text: string;
  /** Part of the answer found in the quote. */
  marked: boolean;
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** The answer's words worth marking: a number's lexeme and unit, else words of 3+ characters. */
function answerTerms(value: CellValue): string[] {
  switch (value.kind) {
    case 'number':
      return [value.lexeme, ...(value.unit ? [value.unit] : [])];
    case 'yes_no':
      return [];
    case 'text':
    case 'choice': {
      const words = valueText(value).split(/[^\p{L}\p{N}]+/u).filter(w => chars(w) >= 3 || /\p{N}/u.test(w));
      return Array.from(new Set(words));
    }
    default:
      return [];
  }
}

/**
 * The quote cut into runs, the answer's words marked where they occur in it as whole
 * words (letter case ignored), so the reader sees what backs the value.
 */
export function quoteSegments(quote: string, value: CellValue | null): QuoteSegment[] {
  if (!quote) return [];
  const terms = value ? answerTerms(value).filter(t => t.length > 0) : [];
  if (terms.length === 0) return [{ text: quote, marked: false }];
  // A term that starts or ends with a letter or digit must not continue a longer word or
  // number there ("94.1" is not in "94.10"); a symbol ("%") has no such edge.
  const alnum = /[\p{L}\p{N}]/u;
  const alternatives = [...terms]
    .sort((a, b) => b.length - a.length)
    .map(term => {
      const first = Array.from(term)[0] ?? '';
      const last = Array.from(term).pop() ?? '';
      const before = alnum.test(first) ? '(?<![\\p{L}\\p{N}])' : '';
      const after = alnum.test(last) ? '(?![\\p{L}\\p{N}])' : '';
      return `${before}${escapeRegExp(term)}${after}`;
    });
  const pattern = new RegExp(alternatives.join('|'), 'giu');
  const segments: QuoteSegment[] = [];
  let last = 0;
  for (const match of quote.matchAll(pattern)) {
    const start = match.index ?? 0;
    if (match[0].length === 0) continue;
    if (start > last) segments.push({ text: quote.slice(last, start), marked: false });
    segments.push({ text: match[0], marked: true });
    last = start + match[0].length;
  }
  if (last < quote.length) segments.push({ text: quote.slice(last), marked: false });
  return segments;
}

// ── Opening a cell's passage ────────────────────────────────────────

/** Where a cell's quote is in its paper: the PDF page with the passage's boxes. */
export interface CellSource {
  filePath: string;
  fileName: string;
  page: number;
  regions: PdfRegion[] | null;
}

/**
 * The place to open for a cell: its page (or the first page its boxes are on) with the
 * boxes, or null when the cell has neither. `documentId` is the library path the viewer
 * resolves (lower-cased on Windows, where the file system ignores case).
 */
export function cellSource(cell: Pick<Cell, 'page' | 'bboxes'>, row: Pick<RowView, 'documentId' | 'title'>): CellSource | null {
  const regions = parseRegions(cell.bboxes);
  const page = typeof cell.page === 'number' && cell.page >= 1 ? cell.page : regions ? firstRegionPage(regions) : null;
  if (page === null) return null;
  return { filePath: row.documentId, fileName: row.title, page, regions };
}

// ── Presets ─────────────────────────────────────────────────────────

export interface PresetColumn {
  id: string;
  label: string;
  spec: ColumnSpec;
}

/** Columns a new table can start with. */
export const PRESET_COLUMNS: readonly PresetColumn[] = [
  { id: 'title', label: 'Title', spec: { question: 'What is the title of the paper?', kind: 'text', options: [], instructions: '' } },
  { id: 'year', label: 'Year', spec: { question: 'In what year was the paper published?', kind: 'number', options: [], instructions: 'The four-digit year only.' } },
  { id: 'method', label: 'Method', spec: { question: 'What method does the paper propose or use?', kind: 'text', options: [], instructions: 'Name the method in a few words.' } },
  { id: 'dataset', label: 'Dataset', spec: { question: 'Which datasets are used in the experiments?', kind: 'text', options: [], instructions: '' } },
  { id: 'main-result', label: 'Main result', spec: { question: 'What is the main quantitative result?', kind: 'text', options: [], instructions: 'Include the metric and the number as the paper states them.' } },
  { id: 'limitations', label: 'Limitations', spec: { question: 'What limitations do the authors state?', kind: 'text', options: [], instructions: '' } },
];

/** A column spec as the user typed it, checked as the backend checks it; an error message or null. */
export function specProblem(spec: ColumnSpec): string | null {
  const question = collapse(spec.question);
  if (!question) return 'A column needs a question.';
  if (chars(question) > MAX_QUESTION_CHARS) return `A column question is at most ${MAX_QUESTION_CHARS} characters.`;
  if (chars(collapse(spec.instructions)) > MAX_INSTRUCTIONS_CHARS) return `Column instructions are at most ${MAX_INSTRUCTIONS_CHARS} characters.`;
  const options = spec.options.map(collapse).filter(o => o.length > 0);
  if (spec.kind === 'choice') {
    if (options.length < MIN_OPTIONS || options.length > MAX_OPTIONS) return `A pick-list column needs ${MIN_OPTIONS} to ${MAX_OPTIONS} options.`;
    const seen = new Set<string>();
    for (const option of options) {
      if (chars(option) > MAX_OPTION_CHARS) return `An option is at most ${MAX_OPTION_CHARS} characters.`;
      const folded = option.toLowerCase();
      if (seen.has(folded)) return `The option "${option}" is listed twice.`;
      seen.add(folded);
    }
  }
  return null;
}

/** The spec to send: options only for a pick-list (the backend refuses them on other kinds). */
export function cleanSpec(spec: ColumnSpec): ColumnSpec {
  return {
    question: collapse(spec.question),
    kind: spec.kind,
    options: spec.kind === 'choice' ? spec.options.map(collapse).filter(o => o.length > 0) : [],
    instructions: collapse(spec.instructions),
  };
}
