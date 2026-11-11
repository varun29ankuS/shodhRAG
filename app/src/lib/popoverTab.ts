/**
 * What Tab does in a small pop-up that lives in a body-level portal, outside the
 * page's order: it moves through the pop-up's own controls (its close button,
 * then any link or button in its text), and past the last one, or back from the
 * first, it closes the pop-up (focus then returns to the button that opened it).
 *
 * Pure module, unit-tested with Node (`app/tests/popoverTab.test.ts`).
 */

/** What a Tab press does: let the browser move focus, or close the pop-up. */
export type PopoverTab = 'move' | 'close';

/**
 * `at`: the index of the focused control among the pop-up's `count` controls in
 * document order, or -1 when the pop-up itself has focus (as it does on opening).
 */
export function popoverTab(at: number, count: number, shift: boolean): PopoverTab {
  if (at < 0) return !shift && count > 0 ? 'move' : 'close';
  if (shift) return at === 0 ? 'close' : 'move';
  return at >= count - 1 ? 'close' : 'move';
}

/** What a pop-up's Tab cycle reaches: its links, buttons and fields. */
export const POPOVER_FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
