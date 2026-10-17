/**
 * DOM markers shared by the focus pop-out and the surfaces under it.
 */

import { stripSymbolWrappers } from '../ask/visual/symbols.ts';

/** Attribute on the pop-out's dialog element. */
export const FOCUS_OVERLAY_ATTR = 'data-focus-overlay';

/** Matches the pop-out (and anything inside it). */
export const FOCUS_OVERLAY_SELECTOR = `[${FOCUS_OVERLAY_ATTR}]`;

/** Whether an event target is inside the focus pop-out. */
export function isInFocusOverlay(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(FOCUS_OVERLAY_SELECTOR) !== null;
}

/** Attribute marking where selected text can be asked about; its value is the scope's kind. */
export const ASK_SCOPE_ATTR = 'data-ask-scope';

/** Whether an ask scope's kind is an answer: one in the conversation, or one in a side thread. */
export function isAnswerScope(kind: string | null | undefined): boolean {
  return kind === 'answer' || kind === 'thread';
}

/**
 * Attribute on inline controls inside an answer whose text is not answer
 * text: citation pills, grounding flags, copy buttons. Quoted text leaves
 * them out.
 */
export const ANSWER_UI_ATTR = 'data-answer-ui';

/**
 * Classes of text that only screen readers get: `sr-only`, and KaTeX's
 * MathML copy of each formula (its `<annotation>` holds the TeX source),
 * which sits beside the visible `katex-html` rendering.
 */
const SCREEN_READER_CLASSES = ['sr-only', 'katex-mathml'];

/** A rendered formula, and the wrapper KaTeX puts around display math. */
const KATEX_CLASS = 'katex';
const KATEX_DISPLAY_CLASS = 'katex-display';
/** Encoding of the MathML annotation that holds a formula's TeX source. */
const TEX_ENCODING = 'application/x-tex';

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
  getAttribute?(name: string): string | null;
  readonly classList?: { contains(token: string): boolean };
}

function hasClass(node: ReadableNode, name: string): boolean {
  return node.nodeType === ELEMENT_NODE && (node.classList?.contains(name) ?? false);
}

/** Every text node under `node`, as `textContent` reads it. */
function plainText(node: ReadableNode): string {
  if (node.nodeType === TEXT_NODE || node.nodeType === CDATA_SECTION_NODE) return node.nodeValue ?? '';
  let out = '';
  for (let i = 0; i < node.childNodes.length; i++) out += plainText(node.childNodes[i]);
  return out;
}

/**
 * The TeX source of a rendered formula, from KaTeX's MathML annotation, with
 * the app's symbol wrappers (`symbols.ts`) removed; null when the node holds
 * no annotation (a selection that covers only part of a formula).
 */
function annotationTex(node: ReadableNode): string | null {
  if (node.nodeType === ELEMENT_NODE && node.getAttribute?.('encoding') === TEX_ENCODING) {
    const tex = stripSymbolWrappers(plainText(node).trim());
    return tex || null;
  }
  for (let i = 0; i < node.childNodes.length; i++) {
    const found = annotationTex(node.childNodes[i]);
    if (found !== null) return found;
  }
  return null;
}

function readable(node: ReadableNode, display: boolean): string {
  if (node.nodeType === TEXT_NODE || node.nodeType === CDATA_SECTION_NODE) return node.nodeValue ?? '';
  if (node.nodeType === ELEMENT_NODE) {
    if (node.hasAttribute?.(ANSWER_UI_ATTR)) return '';
    if (hasClass(node, KATEX_CLASS)) {
      const tex = annotationTex(node);
      if (tex !== null) return display ? `$$${tex}$$` : `$${tex}$`;
    }
    if (SCREEN_READER_CLASSES.some(name => hasClass(node, name))) return '';
  }
  const inDisplay = display || hasClass(node, KATEX_DISPLAY_CLASS);
  let out = '';
  const children = node.childNodes;
  for (let i = 0; i < children.length; i++) out += readable(children[i], inDisplay);
  return out;
}

/**
 * A node's text as a reader sees it: `textContent` without answer controls
 * (`[data-answer-ui]`, so "latency [1][2]" is not "latency12") and without
 * screen-reader-only text. A formula reads as its TeX source (`$x^2$`, or
 * `$$…$$` for display math), not as KaTeX's glyphs ("x2"); a part of a
 * formula without its source reads as the glyphs shown. Other buttons keep
 * their text: a "go deeper" mark is answer text.
 */
export function readableText(node: ReadableNode): string {
  return readable(node, false);
}

/**
 * Selected text as a reader sees it: `readableText` of the range's contents,
 * so pills and flags the selection crosses are never quoted.
 * `Selection.toString()` would include them.
 */
export function readableSelection(range: { cloneContents(): ReadableNode }): string {
  return readableText(range.cloneContents());
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
