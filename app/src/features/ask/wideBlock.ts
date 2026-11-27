/**
 * The conversation's two widths. Text reads in a column; a wide block of an answer (a
 * table, code, a chart or diagram, a previewed page) breaks out of it, centred on it, up
 * to its own maximum, and shrinks with the window. The rules are `.ask-log` and
 * `.ask-wide` in `index.css`, which must agree with the numbers here
 * (`app/tests/wideBlock.test.ts` reads both).
 *
 * Pure, so the Node tests load it.
 */

/** The text column's maximum width, padding included. */
export const TEXT_COLUMN_PX = 800;

/** A wide block's maximum width. */
export const WIDE_BLOCK_MAX_PX = 1100;

/** The column's side padding (Tailwind `px-7`), kept clear at both edges of the conversation. */
export const COLUMN_GUTTER_PX = 28;

/** On the conversation's scroller: wide blocks measure themselves against it. */
export const WIDE_BLOCK_HOST_CLASS = 'ask-log';

/** On a block that breaks out of the text column. */
export const WIDE_BLOCK_CLASS = 'ask-wide';

/** The text's width in a conversation `host` px wide. */
export function textWidth(host: number): number {
  return Math.max(0, Math.min(TEXT_COLUMN_PX, host) - COLUMN_GUTTER_PX * 2);
}

/** A wide block's width in a conversation `host` px wide: never narrower than the text. */
export function wideBlockWidth(host: number): number {
  return Math.max(0, Math.min(WIDE_BLOCK_MAX_PX, host - COLUMN_GUTTER_PX * 2));
}

/** How far a wide block reaches past the text on each side. */
export function wideBlockOverhang(host: number): number {
  return (wideBlockWidth(host) - textWidth(host)) / 2;
}

/** The docked plan's width (`PlanPanel`, variant `docked`). */
export const PLAN_DOCK_PX = 260;

/** The docked plan's distance from the conversation's right edge: clear of the question rail. */
export const PLAN_DOCK_INSET_PX = COLUMN_GUTTER_PX + 8;

/** The least space between the docked plan and the widest block. */
const PLAN_DOCK_CLEARANCE_PX = 8;

/**
 * Whether the newest answer's plan docks right of a conversation `host` px wide: only
 * where it covers none of it, the widest block included. Elsewhere the plan stays in the
 * answer, above its text.
 */
export function planDocks(host: number): boolean {
  return (host - WIDE_BLOCK_MAX_PX) / 2 >= PLAN_DOCK_PX + PLAN_DOCK_INSET_PX + PLAN_DOCK_CLEARANCE_PX;
}
