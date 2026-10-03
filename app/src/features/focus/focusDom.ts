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

/**
 * Rows of an HTML table as cell text (header row first). Controls inside
 * cells (citation pills) are left out, so "$4.5M" with pill 2 does not
 * read as "$4.5M2".
 */
export function tableRows(table: HTMLTableElement): string[][] {
  return Array.from(table.rows).map(row =>
    Array.from(row.cells).map(cell => {
      const copy = cell.cloneNode(true) as HTMLElement;
      copy.querySelectorAll('button').forEach(el => el.remove());
      return (copy.textContent ?? '').replace(/\s+/g, ' ').trim();
    }),
  );
}
