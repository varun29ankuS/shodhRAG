/**
 * Locate the rows of a spreadsheet that a retrieved table chunk came from.
 *
 * The chunker (crates/shodh-rag/src/processing/chunker.rs, `chunk_table`)
 * writes table chunks as:
 *
 *   <sheet name>[ (Page N)][ (rows A-B of N)]
 *   Columns: H1 | H2 | …
 *   H1: v1 | H3: v3          ← one line per row, empty cells skipped
 *
 * The `(rows A-B of N)` range is the primary signal; each row line is then
 * confirmed (or found elsewhere) by comparing its `Header: value` cells with
 * the sheet, using the same single-line whitespace collapsing the chunker
 * applies to cell values.
 */

export interface SheetData {
  name: string;
  headers: string[];
  rows: string[][];
}

export interface TableLocation {
  sheetIndex: number;
  /** 0-based data row indices to highlight, ascending. */
  rows: number[];
  /** True when every cited row line was matched by its cell values. */
  confirmed: boolean;
}

interface ParsedChunk {
  sheetName: string | null;
  /** 0-based inclusive data-row range from `(rows A-B of N)`. */
  range: [number, number] | null;
  rowLines: string[];
}

const ROWS_SUFFIX = /\s*\(rows (\d+)-(\d+) of (\d+)\)\s*$/;
const PAGE_SUFFIX = /\s*\(Page \d+\)\s*$/;

/** Mirror of the chunker's `single_line`: collapse embedded line breaks. */
export function singleLine(value: string): string {
  return value
    .split(/[\r\n]+/)
    .map(part => part.trim())
    .filter(part => part.length > 0)
    .join(' ');
}

function parseChunk(text: string): ParsedChunk {
  const lines = text.split(/\r?\n/).map(l => l.trim()).filter(l => l.length > 0);
  if (lines.length === 0) return { sheetName: null, range: null, rowLines: [] };

  let sheetName: string | null = null;
  let range: [number, number] | null = null;
  let body = lines;
  const hasColumnsLine = lines.length > 1 && lines[1].startsWith('Columns: ');
  if (hasColumnsLine) {
    let label = lines[0];
    const rows = ROWS_SUFFIX.exec(label);
    if (rows) {
      const start = Number(rows[1]);
      const end = Number(rows[2]);
      if (Number.isInteger(start) && Number.isInteger(end) && start >= 1 && end >= start) {
        range = [start - 1, end - 1];
      }
      label = label.slice(0, rows.index);
    }
    label = label.replace(PAGE_SUFFIX, '');
    sheetName = label.trim() || null;
    body = lines.slice(2);
  } else if (lines[0].startsWith('Columns: ')) {
    body = lines.slice(1);
  }
  return { sheetName, range, rowLines: body };
}

interface Cell {
  column: number | null;
  value: string;
}

/**
 * Split a row line into cells. Segments are separated by " | "; a segment
 * that does not start with a known "Header: " prefix is joined back onto the
 * previous cell, since cell values may themselves contain " | ".
 */
function parseRowLine(line: string, headers: readonly string[]): Cell[] {
  const segments = line.split(' | ');
  const cells: Cell[] = [];
  for (const segment of segments) {
    let column: number | null = null;
    let value = segment;
    for (let idx = 0; idx < headers.length; idx += 1) {
      const prefix = `${headers[idx]}: `;
      if (headers[idx] && segment.startsWith(prefix)) {
        column = idx;
        value = segment.slice(prefix.length);
        break;
      }
    }
    if (column === null) {
      const generic = /^Column (\d+): /.exec(segment);
      if (generic) {
        column = Number(generic[1]) - 1;
        value = segment.slice(generic[0].length);
      }
    }
    if (column === null && cells.length > 0 && cells[cells.length - 1].column !== null) {
      cells[cells.length - 1].value += ` | ${segment}`;
      continue;
    }
    cells.push({ column, value: value.trim() });
  }
  return cells.filter(c => c.value.length > 0);
}

function cellScore(row: readonly string[], cells: readonly Cell[]): number {
  if (cells.length === 0) return 0;
  let hits = 0;
  for (const cell of cells) {
    if (cell.column !== null) {
      const v = row[cell.column];
      if (v !== undefined && singleLine(v) === cell.value) hits += 1;
    } else if (row.some(v => singleLine(v) === cell.value)) {
      hits += 1;
    }
  }
  return hits / cells.length;
}

function locateInSheet(sheet: SheetData, chunk: ParsedChunk): { rows: number[]; confirmed: boolean; score: number } {
  const headers = sheet.headers.map(singleLine);
  const total = sheet.rows.length;
  const used = new Set<number>();
  const rows: number[] = [];
  let matchedLines = 0;

  const inRange = (idx: number) => chunk.range !== null && idx >= chunk.range[0] && idx <= chunk.range[1];

  for (const line of chunk.rowLines) {
    const cells = parseRowLine(line, headers);
    if (cells.length === 0) continue;
    let best = -1;
    let bestScore = 0;
    const consider = (idx: number) => {
      if (used.has(idx)) return;
      const score = cellScore(sheet.rows[idx], cells);
      if (score > bestScore) {
        best = idx;
        bestScore = score;
      }
    };
    if (chunk.range) {
      const end = Math.min(chunk.range[1], total - 1);
      for (let idx = chunk.range[0]; idx <= end && bestScore < 1; idx += 1) consider(idx);
    }
    if (bestScore < 1) {
      for (let idx = 0; idx < total && bestScore < 1; idx += 1) {
        if (!inRange(idx)) consider(idx);
      }
    }
    if (best >= 0 && bestScore >= 0.6) {
      used.add(best);
      rows.push(best);
      if (bestScore === 1) matchedLines += 1;
    }
  }

  // A range with no confirmable row lines still pinpoints the rows.
  if (rows.length === 0 && chunk.range) {
    const end = Math.min(chunk.range[1], total - 1);
    for (let idx = chunk.range[0]; idx <= end; idx += 1) rows.push(idx);
  }

  rows.sort((a, b) => a - b);
  const confirmed = chunk.rowLines.length > 0 && matchedLines === chunk.rowLines.length;
  return { rows, confirmed, score: matchedLines * 2 + rows.length };
}

/** Find the sheet and rows a table chunk was cut from, or null. */
export function locateTableRows(sheets: readonly SheetData[], passage: string): TableLocation | null {
  if (sheets.length === 0) return null;
  const chunk = parseChunk(passage);
  if (chunk.rowLines.length === 0 && !chunk.range) return null;

  const wanted = chunk.sheetName ? singleLine(chunk.sheetName) : null;
  const named = wanted === null ? [] : sheets.map((s, i) => (singleLine(s.name) === wanted ? i : -1)).filter(i => i >= 0);
  const candidates = named.length > 0 ? named : sheets.map((_, i) => i);

  let best: TableLocation | null = null;
  let bestScore = 0;
  for (const sheetIndex of candidates) {
    const found = locateInSheet(sheets[sheetIndex], chunk);
    if (found.rows.length > 0 && found.score > bestScore) {
      best = { sheetIndex, rows: found.rows, confirmed: found.confirmed };
      bestScore = found.score;
    }
  }
  return best;
}
