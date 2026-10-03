/**
 * DOM markers shared by the focus pop-out and the surfaces under it.
 */

/** Attribute on the pop-out's dialog element. */
export const FOCUS_OVERLAY_ATTR = 'data-focus-overlay';

/** Matches the pop-out (and anything inside it). */
export const FOCUS_OVERLAY_SELECTOR = `[${FOCUS_OVERLAY_ATTR}]`;

/** Whether an event target is inside the focus pop-out. */
export function isInFocusOverlay(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(FOCUS_OVERLAY_SELECTOR) !== null;
}

/** Rows of an HTML table as cell text (header row first). */
export function tableRows(table: HTMLTableElement): string[][] {
  return Array.from(table.rows).map(row => Array.from(row.cells).map(cell => (cell.textContent ?? '').trim()));
}
