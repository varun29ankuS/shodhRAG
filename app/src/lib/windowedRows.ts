/**
 * Pure parts of the app's windowed lists (`useWindowedList`): which rows stay drawn
 * outside the visible range, and how a grid's cards are laid out in rows. Tested in
 * `app/tests/windowedRows.test.ts`.
 *
 * No imports: the Node tests load it.
 */

/**
 * Indexes to draw: the visible range plus the rows that must stay drawn wherever the
 * list is scrolled (the one holding focus, the one the keyboard's tab stop is on).
 * Sorted, unique, in range.
 */
export function drawnIndexes(visible: readonly number[], keep: readonly (number | null | undefined)[], count: number): number[] {
  const set = new Set<number>();
  for (const i of visible) if (i >= 0 && i < count) set.add(i);
  for (const i of keep) if (typeof i === 'number' && i >= 0 && i < count) set.add(i);
  return [...set].sort((a, b) => a - b);
}

/**
 * The columns of a grid `width` px wide whose cards are at least `min` px wide with `gap`
 * px between them (CSS `repeat(auto-fill, minmax(min, 1fr))`). At least one.
 */
export function gridColumns(width: number, min: number, gap: number): number {
  if (!(width > 0) || !(min > 0)) return 1;
  return Math.max(1, Math.floor((width + gap) / (min + gap)));
}

/** The rows of `count` cards in `columns` columns: each row's first and last card index. */
export function gridRows(count: number, columns: number): Array<{ first: number; last: number }> {
  const per = Math.max(1, Math.floor(columns));
  const rows: Array<{ first: number; last: number }> = [];
  for (let first = 0; first < count; first += per) rows.push({ first, last: Math.min(count, first + per) - 1 });
  return rows;
}

/** The row of a grid in `columns` columns that card `index` is in. */
export function rowOfCard(index: number, columns: number): number {
  return Math.floor(Math.max(0, index) / Math.max(1, Math.floor(columns)));
}
