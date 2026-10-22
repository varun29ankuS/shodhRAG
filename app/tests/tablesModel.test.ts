/**
 * Evidence table logic tests (sorting, filtering, inKind, marks, staleness, keyboard
 * movement, quotes, opening a cell's passage). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/tablesModel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  addBatches,
  cellIndex,
  cellMark,
  cellSource,
  cleanSpec,
  filterRows,
  inKind,
  isStale,
  fileNameOf,
  nextCell,
  paperKey,
  PRESET_COLUMNS,
  quoteSegments,
  sortRows,
  specProblem,
  typedValue,
  valueText,
} from '../src/features/tables/model.ts';
import type { Cell, CellStatus, CellValue, Column, ColumnKind, RowView } from '../src/features/tables/types.ts';

function column(id: string, kind: ColumnKind, extra: Partial<Column> = {}): Column {
  return {
    id,
    tableId: 't',
    position: 0,
    question: `Question ${id}?`,
    kind,
    options: [],
    instructions: '',
    key: `key-${id}`,
    revision: 1,
    createdAt: '2026-10-20T00:00:00Z',
    updatedAt: '2026-10-20T00:00:00Z',
    ...extra,
  };
}

function row(id: string, position: number, title = id): RowView {
  return { id, tableId: 't', position, documentId: `c:/papers/${id}.pdf`, addedAt: '2026-10-20T00:00:00Z', title };
}

function cell(rowId: string, columnId: string, status: CellStatus, value: CellValue | null, extra: Partial<Cell> = {}): Cell {
  return {
    tableId: 't',
    rowId,
    columnId,
    status,
    value,
    quote: null,
    chunkId: null,
    page: null,
    bboxes: null,
    model: null,
    columnRevision: 1,
    paperHash: null,
    filledAt: null,
    editedAt: null,
    error: null,
    ...extra,
  };
}

const num = (lexeme: string, decimal: string, unit: string | null = null): CellValue => ({ kind: 'number', lexeme, decimal, unit });
const text = (t: string): CellValue => ({ kind: 'text', text: t });
const ids = (rows: readonly RowView[]) => rows.map(r => r.id);

// ── inKind ──────────────────────────────────────────────────────────

test('inKind reads an edited text "2021" in a number column as the number 2021', () => {
  assert.deepEqual(inKind(text('2021'), column('c', 'number')), num('2021', '2021'));
});

test('inKind reads a number in a text column as its printed text, unit included', () => {
  assert.deepEqual(inKind(num('94.1', '94.1', '%'), column('c', 'text')), text('94.1 %'));
});

test('inKind follows the number rule: separators, Unicode minus, leading point, units', () => {
  const c = column('c', 'number');
  assert.deepEqual(inKind(text('1,234.5 ms'), c), num('1,234.5', '1234.5', 'ms'));
  assert.deepEqual(inKind(text('\u22123.2'), c), num('\u22123.2', '-3.2'));
  assert.deepEqual(inKind(text('.5'), c), num('.5', '0.5'));
  assert.deepEqual(inKind(text('+7 GB'), c), num('+7', '7', 'GB'));
  assert.equal(inKind(text('about 12'), c), null);
  assert.equal(inKind(text('12 to 14'), c), null, 'a unit holds no digits');
  assert.equal(inKind(text('Top-1 error'), c), null);
});

test('inKind matches yes/no and pick-list values case-blind, and nothing else', () => {
  assert.deepEqual(inKind(text(' YES '), column('c', 'yes_no')), { kind: 'yes_no', yes: true });
  assert.equal(inKind(text('maybe'), column('c', 'yes_no')), null);
  const choice = column('c', 'choice', { options: ['HNSW', 'IVF-PQ'] });
  assert.deepEqual(inKind(text('hnsw'), choice), { kind: 'choice', option: 'HNSW' });
  assert.deepEqual(inKind({ kind: 'choice', option: 'ivf-pq' }, choice), { kind: 'choice', option: 'IVF-PQ' });
  assert.equal(inKind({ kind: 'choice', option: 'Flat' }, choice), null);
  assert.equal(inKind({ kind: 'yes_no', yes: true }, column('c', 'number')), null);
});

test('inKind holds text to 1–500 characters, counted as characters', () => {
  const c = column('c', 'text');
  assert.equal(inKind(text('   '), c), null);
  assert.deepEqual(inKind(text('a  b\n c'), c), text('a b c'));
  assert.deepEqual(inKind(text('\u{1F600}'.repeat(500)), c), text('\u{1F600}'.repeat(500)));
  assert.equal(inKind(text('x'.repeat(501)), c), null);
  assert.equal(inKind(null, c), null);
});

test('typedValue is what an edit would store', () => {
  assert.deepEqual(typedValue({ kind: 'number', options: [] }, '3.2x'), num('3.2', '3.2', 'x'));
  assert.equal(valueText(num('3.2', '3.2', 'x')), '3.2 x');
  assert.equal(valueText({ kind: 'yes_no', yes: false }), 'no');
  assert.equal(valueText(null), '');
});

// ── Sorting ─────────────────────────────────────────────────────────

test('numbers sort numerically, units ignored, and empty cells go last both ways', () => {
  const c = column('n', 'number');
  const rows = [row('a', 0), row('b', 1), row('c', 2), row('d', 3), row('e', 4)];
  const cells = cellIndex([
    cell('a', 'n', 'verified', num('10', '10', 'ms')),
    cell('b', 'n', 'verified', num('9.5', '9.5', 's')),
    cell('c', 'n', 'not_found', null),
    cell('d', 'n', 'verified', num('1,000', '1000')),
  ]);
  assert.deepEqual(ids(sortRows(rows, cells, c, 'asc')), ['b', 'a', 'd', 'c', 'e']);
  assert.deepEqual(ids(sortRows(rows, cells, c, 'desc')), ['d', 'a', 'b', 'c', 'e']);
});

test('a Text "2021" in a number column sorts as 2021', () => {
  const c = column('n', 'number');
  const rows = [row('a', 0), row('b', 1), row('c', 2)];
  const cells = cellIndex([
    cell('a', 'n', 'verified', num('2023', '2023')),
    cell('b', 'n', 'edited', text('2021')),
    cell('c', 'n', 'verified', num('2022', '2022')),
  ]);
  assert.deepEqual(ids(sortRows(rows, cells, c, 'asc')), ['b', 'c', 'a']);
});

test('a cell flagged value_no_longer_fits sorts last, as does a value that does not fit', () => {
  const c = column('n', 'number');
  const rows = [row('a', 0), row('b', 1), row('c', 2)];
  const cells = cellIndex([
    cell('a', 'n', 'edited', num('1', '1'), { error: 'value_no_longer_fits' }),
    cell('b', 'n', 'verified', num('5', '5')),
    cell('c', 'n', 'edited', text('soon')),
  ]);
  assert.deepEqual(ids(sortRows(rows, cells, c, 'asc')), ['b', 'a', 'c']);
  assert.deepEqual(ids(sortRows(rows, cells, c, 'desc')), ['b', 'a', 'c']);
});

test('text sorts case-blind with numbers in order, pick-lists by option order, titles when no column', () => {
  const t = column('t', 'text');
  const rows = [row('a', 0, 'b paper'), row('b', 1, 'A paper'), row('c', 2, 'C paper')];
  const cells = cellIndex([cell('a', 't', 'verified', text('item 10')), cell('b', 't', 'verified', text('Item 9')), cell('c', 't', 'verified', text('alpha'))]);
  assert.deepEqual(ids(sortRows(rows, cells, t, 'asc')), ['c', 'b', 'a']);
  assert.deepEqual(ids(sortRows(rows, cells, null, 'asc')), ['b', 'a', 'c']);
  const choice = column('k', 'choice', { options: ['low', 'high'] });
  const picked = cellIndex([cell('a', 'k', 'verified', { kind: 'choice', option: 'high' }), cell('b', 'k', 'verified', { kind: 'choice', option: 'low' })]);
  assert.deepEqual(ids(sortRows(rows, picked, choice, 'asc')), ['b', 'a', 'c']);
});

// ── Filtering ───────────────────────────────────────────────────────

test('value filters read through inKind: a Number in a text column never matches a typed filter', () => {
  const yn = column('y', 'yes_no');
  const rows = [row('a', 0), row('b', 1), row('c', 2), row('d', 3)];
  const cells = cellIndex([
    cell('a', 'y', 'verified', { kind: 'yes_no', yes: true }),
    cell('b', 'y', 'edited', text('No')),
    cell('c', 'y', 'verified', num('1', '1')),
  ]);
  assert.deepEqual(ids(filterRows(rows, cells, [yn], [{ kind: 'value', columnId: 'y', values: ['yes'] }])), ['a']);
  assert.deepEqual(ids(filterRows(rows, cells, [yn], [{ kind: 'value', columnId: 'y', values: ['no'] }])), ['b']);
  assert.deepEqual(ids(filterRows(rows, cells, [yn], [{ kind: 'value', columnId: 'y', values: [] }])), ['a', 'b', 'c', 'd']);
});

test('a Number in a text column filters as text', () => {
  const t = column('t', 'text');
  const choice = column('k', 'choice', { options: ['94.1 %', 'other'] });
  const rows = [row('a', 0)];
  const cells = cellIndex([cell('a', 't', 'verified', num('94.1', '94.1', '%')), cell('a', 'k', 'verified', num('94.1', '94.1', '%'))]);
  assert.deepEqual(inKind(cells.get('a\u0000t')?.value ?? null, t), text('94.1 %'));
  assert.deepEqual(ids(filterRows(rows, cells, [t, choice], [{ kind: 'value', columnId: 'k', values: ['94.1 %'] }])), ['a']);
});

test('a status filter keeps rows with a cell in one of its statuses, empty cells counting as empty', () => {
  const a = column('a', 'text');
  const b = column('b', 'text');
  const rows = [row('r1', 0), row('r2', 1), row('r3', 2)];
  const cells = cellIndex([
    cell('r1', 'a', 'verified', text('x')),
    cell('r1', 'b', 'unverified', text('y')),
    cell('r2', 'a', 'verified', text('x')),
    cell('r2', 'b', 'verified', text('y')),
    cell('r3', 'a', 'failed', null),
  ]);
  assert.deepEqual(ids(filterRows(rows, cells, [a, b], [{ kind: 'status', statuses: ['unverified', 'failed'] }])), ['r1', 'r3']);
  assert.deepEqual(ids(filterRows(rows, cells, [a, b], [{ kind: 'status', statuses: ['empty'] }])), ['r3']);
  assert.deepEqual(
    ids(filterRows(rows, cells, [a, b], [{ kind: 'status', statuses: ['verified'] }, { kind: 'status', statuses: ['unverified'] }])),
    ['r1'],
  );
});

// ── Marks and staleness ─────────────────────────────────────────────

test('marks are shown for unverified, not found, edited and failed cells only', () => {
  assert.equal(cellMark(cell('r', 'c', 'unverified', text('x'))), 'unverified');
  assert.equal(cellMark(cell('r', 'c', 'not_found', null)), 'not_found');
  assert.equal(cellMark(cell('r', 'c', 'edited', text('x'))), 'edited');
  assert.equal(cellMark(cell('r', 'c', 'failed', null)), 'failed');
  for (const status of ['verified', 'empty', 'queued'] as const) assert.equal(cellMark(cell('r', 'c', status, null)), null);
  assert.equal(cellMark(undefined), null);
});

test('a filled cell of an older column revision is stale; edits, empties and current cells are not', () => {
  const c = column('c', 'text', { revision: 3 });
  assert.equal(isStale(cell('r', 'c', 'verified', text('x'), { columnRevision: 2 }), c), true);
  assert.equal(isStale(cell('r', 'c', 'not_found', null, { columnRevision: 1 }), c), true);
  assert.equal(isStale(cell('r', 'c', 'verified', text('x'), { columnRevision: 3 }), c), false);
  assert.equal(isStale(cell('r', 'c', 'edited', text('x'), { columnRevision: 1 }), c), false);
  assert.equal(isStale(cell('r', 'c', 'empty', null, { columnRevision: null }), c), false);
  assert.equal(isStale(cell('r', 'c', 'verified', text('x'), { columnRevision: null }), c), false);
  assert.equal(isStale(undefined, c), false);
});

// ── Keyboard ────────────────────────────────────────────────────────

test('arrow keys move one cell and stop at the edges', () => {
  const dims = { rows: 3, cols: 4 };
  assert.deepEqual(nextCell({ row: 0, col: 0 }, 'ArrowUp', dims), { row: 0, col: 0 });
  assert.deepEqual(nextCell({ row: 0, col: 0 }, 'ArrowLeft', dims), { row: 0, col: 0 });
  assert.deepEqual(nextCell({ row: 0, col: 0 }, 'ArrowDown', dims), { row: 1, col: 0 });
  assert.deepEqual(nextCell({ row: 2, col: 3 }, 'ArrowDown', dims), { row: 2, col: 3 });
  assert.deepEqual(nextCell({ row: 2, col: 3 }, 'ArrowRight', dims), { row: 2, col: 3 });
  assert.deepEqual(nextCell({ row: 1, col: 1 }, 'ArrowRight', dims), { row: 1, col: 2 });
});

test('Home, End, their Ctrl forms and page keys jump within the grid', () => {
  const dims = { rows: 25, cols: 4 };
  assert.deepEqual(nextCell({ row: 5, col: 2 }, 'Home', dims), { row: 5, col: 0 });
  assert.deepEqual(nextCell({ row: 5, col: 2 }, 'End', dims), { row: 5, col: 3 });
  assert.deepEqual(nextCell({ row: 5, col: 2 }, 'Home', dims, true), { row: 0, col: 0 });
  assert.deepEqual(nextCell({ row: 5, col: 2 }, 'End', dims, true), { row: 24, col: 3 });
  assert.deepEqual(nextCell({ row: 5, col: 2 }, 'PageUp', dims), { row: 0, col: 2 });
  assert.deepEqual(nextCell({ row: 20, col: 2 }, 'PageDown', dims), { row: 24, col: 2 });
  assert.equal(nextCell({ row: 0, col: 0 }, 'Enter', dims), null);
  assert.equal(nextCell({ row: 0, col: 0 }, 'ArrowDown', { rows: 0, cols: 4 }), null);
});

// ── Quotes and passages ─────────────────────────────────────────────

test('the answer is marked in its quote as whole words, case-blind', () => {
  const segments = quoteSegments('Recall@10 reaches 94.1% on SIFT1M, beating 94.10 elsewhere.', num('94.1', '94.1', '%'));
  assert.deepEqual(segments.filter(s => s.marked).map(s => s.text), ['94.1', '%']);
  assert.equal(segments.map(s => s.text).join(''), 'Recall@10 reaches 94.1% on SIFT1M, beating 94.10 elsewhere.');
  const words = quoteSegments('We evaluate on the sift1m and GIST datasets.', text('SIFT1M and GIST'));
  assert.deepEqual(words.filter(s => s.marked).map(s => s.text), ['sift1m', 'and', 'GIST']);
  assert.deepEqual(quoteSegments('Some quote here.', { kind: 'yes_no', yes: true }), [{ text: 'Some quote here.', marked: false }]);
  assert.deepEqual(quoteSegments('', text('x')), []);
});

test('a cell opens at its page with its boxes, or at the first boxed page when the page is missing', () => {
  const r = row('a', 0, 'A paper');
  const bboxes = JSON.stringify([{ page: 4, x0: 10, y0: 20, x1: 200, y1: 60 }, { page: 3, x0: 1, y0: 2, x1: 3, y1: 4 }]);
  assert.deepEqual(cellSource({ page: 4, bboxes }, r), {
    filePath: 'c:/papers/a.pdf',
    fileName: 'A paper',
    page: 4,
    regions: [{ page: 4, x0: 10, y0: 20, x1: 200, y1: 60 }, { page: 3, x0: 1, y0: 2, x1: 3, y1: 4 }],
  });
  assert.equal(cellSource({ page: null, bboxes }, r)?.page, 3);
  assert.deepEqual(cellSource({ page: 2, bboxes: 'not json' }, r), { filePath: 'c:/papers/a.pdf', fileName: 'A paper', page: 2, regions: null });
  assert.equal(cellSource({ page: null, bboxes: null }, r), null);
});

// ── Columns ─────────────────────────────────────────────────────────

test('presets are valid columns with distinct ids', () => {
  assert.deepEqual(PRESET_COLUMNS.map(p => p.label), ['Title', 'Year', 'Method', 'Dataset', 'Main result', 'Limitations']);
  assert.equal(new Set(PRESET_COLUMNS.map(p => p.id)).size, PRESET_COLUMNS.length);
  for (const preset of PRESET_COLUMNS) assert.equal(specProblem(preset.spec), null, preset.label);
});

test('a column spec is checked as the backend checks it', () => {
  const spec = (extra: Partial<Parameters<typeof specProblem>[0]>) => ({ question: 'What?', kind: 'text' as ColumnKind, options: [], instructions: '', ...extra });
  assert.equal(specProblem(spec({ question: '   ' })), 'A column needs a question.');
  assert.match(specProblem(spec({ question: 'q'.repeat(301) })) ?? '', /at most 300/);
  assert.match(specProblem(spec({ kind: 'choice', options: ['one'] })) ?? '', /2 to 12 options/);
  assert.match(specProblem(spec({ kind: 'choice', options: ['A', 'a '] })) ?? '', /listed twice/);
  assert.equal(specProblem(spec({ kind: 'choice', options: ['A', 'B', ' '] })), null);
  assert.deepEqual(cleanSpec(spec({ kind: 'text', options: ['x'], question: ' What   now? ' })), { question: 'What now?', kind: 'text', options: [], instructions: '' });
  assert.deepEqual(cleanSpec(spec({ kind: 'choice', options: [' A ', '', 'B'] })).options, ['A', 'B']);
});

// ── Papers ──────────────────────────────────────────────────────────

test('a path is compared with a row the way path_key spells it', () => {
  assert.equal(paperKey('C:\\Papers\\SPANN.pdf', true), 'c:/papers/spann.pdf');
  assert.equal(paperKey('/home/me/Papers/SPANN.pdf', false), '/home/me/Papers/SPANN.pdf');
  assert.equal(fileNameOf('C:\\Papers\\SPANN.pdf'), 'SPANN.pdf');
  assert.equal(fileNameOf('/a/b/'), 'b');
});

test('papers are added in batches the command accepts', () => {
  const paths = Array.from({ length: 1201 }, (_, i) => `p${i}`);
  const batches = addBatches(paths);
  assert.deepEqual(batches.map(b => b.length), [500, 500, 201]);
  assert.deepEqual(batches.flat(), paths);
  assert.deepEqual(addBatches([]), []);
});
