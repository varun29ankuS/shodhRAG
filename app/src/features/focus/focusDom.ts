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
 * Attribute on inline controls inside an answer whose text is not answer
 * text: citation pills, grounding flags, copy buttons. Quoted text leaves
 * them out.
 */
export const ANSWER_UI_ATTR = 'data-answer-ui';

/** Class of text that only screen readers get. */
const SR_ONLY_CLASS = 'sr-only';

const ELEMENT_NODE = 1;
const TEXT_NODE = 3;
const CDATA_SECTION_NODE = 4;

/**
 * The parts of a DOM node `readableText` reads: an element, a text node or a
 * fragment (`Range.cloneContents()`). Structural, so the walk is testable
 * without a DOM.
 */
export interface ReadableNode {
  readonly nodeType: number;
  readonly nodeValue: string | null;
  readonly childNodes: ArrayLike<ReadableNode>;
  hasAttribute?(name: string): boolean;
  readonly classList?: { contains(token: string): boolean };
}

/**
 * A node's text as a reader sees it: `textContent` without answer controls
 * (`[data-answer-ui]`, so "latency [1][2]" is not "latency12") and without
 * screen-reader-only text. Other buttons keep their text: a "go deeper" mark
 * is answer text.
 */
export function readableText(node: ReadableNode): string {
  if (node.nodeType === TEXT_NODE || node.nodeType === CDATA_SECTION_NODE) return node.nodeValue ?? '';
  if (node.nodeType === ELEMENT_NODE) {
    if (node.hasAttribute?.(ANSWER_UI_ATTR) || node.classList?.contains(SR_ONLY_CLASS)) return '';
  }
  let out = '';
  const children = node.childNodes;
  for (let i = 0; i < children.length; i++) out += readableText(children[i]);
  return out;
}

/**
 * Rows of an HTML table as cell text (header row first). Answer controls
 * inside cells (citation pills) are left out, so "$4.5M" with pill 2 does not
 * read as "$4.5M2".
 */
export function tableRows(table: HTMLTableElement): string[][] {
  return Array.from(table.rows).map(row =>
    Array.from(row.cells).map(cell => readableText(cell).replace(/\s+/g, ' ').trim()),
  );
}
