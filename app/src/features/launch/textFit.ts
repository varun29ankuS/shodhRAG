/**
 * Whether the text on screen fits: the launch test's measure of a view (`launch_probe.rs`,
 * the views tour). Every element that holds text of its own and is drawn is measured the
 * way the browser lays it out: text wider than its box (`scrollWidth > clientWidth`) is cut
 * off or spills, and so is text taller than a box that hides what does not fit. A box that
 * hides its overflow is measured for all the text inside it. A field's
 * placeholder or value is measured in its own font against the room the field gives it.
 *
 * The rule (`clipped`) takes numbers only, so the Node tests hold it; `clippedText` reads
 * those numbers from a page.
 */

/** What is measured of one element. */
export interface BoxMeasure {
  scrollWidth: number;
  clientWidth: number;
  scrollHeight: number;
  clientHeight: number;
  /** Computed `overflow-x` and `overflow-y`. */
  overflowX: string;
  overflowY: string;
}

/** A box that scrolls shows all of its text, a part at a time. */
const scrolls = (overflow: string) => overflow === 'auto' || overflow === 'scroll';
/** A box that hides what does not fit. */
const hides = (overflow: string) => overflow === 'hidden' || overflow === 'clip';

/** Why the text of a box does not fit, or null when it does. */
export function clipped(box: BoxMeasure): 'width' | 'height' | null {
  if (!scrolls(box.overflowX) && box.scrollWidth > box.clientWidth) return 'width';
  if (hides(box.overflowY) && box.scrollHeight > box.clientHeight) return 'height';
  return null;
}

/** Whether text `textWidth` wide fits a field with `room` for it. */
export function fieldTextFits(textWidth: number, room: number): boolean {
  return textWidth <= room;
}

/** The most that is reported of one view (`launch_probe.rs` takes no more). */
export const MAX_REPORTED = 60;
/** The most that is quoted of one element's text. */
const MAX_QUOTED = 48;

const quote = (text: string) => {
  const flat = text.replace(/\s+/g, ' ').trim();
  return flat.length > MAX_QUOTED ? `${flat.slice(0, MAX_QUOTED)}…` : flat;
};

/** Where an element is, for whoever reads the report: its tag, a name it carries, its first classes. */
function nameOf(el: Element): string {
  const named = el.closest('[data-testid], [id], [aria-label]');
  const anchor = named
    ? named.getAttribute('data-testid') ?? named.getAttribute('id') ?? named.getAttribute('aria-label') ?? ''
    : '';
  const classes = typeof el.className === 'string' ? el.className.split(/\s+/).filter(Boolean).slice(0, 4).join('.') : '';
  return `${el.tagName.toLowerCase()}${classes ? `.${classes}` : ''}${anchor ? ` in [${quote(anchor)}]` : ''}`;
}

/** Whether the element is drawn: it has a box of more than a pixel and is not hidden. */
function drawn(el: HTMLElement, style: CSSStyleDeclaration): boolean {
  if (style.visibility !== 'visible' || style.display === 'none') return false;
  const rect = el.getBoundingClientRect();
  // A box of one pixel is text kept for screen readers only.
  return rect.width > 1 && rect.height > 1;
}

/** Whether the element has text of its own (not only its children's). */
function ownText(el: Element): string {
  let text = '';
  for (const node of Array.from(el.childNodes)) {
    if (node.nodeType === Node.TEXT_NODE) text += node.textContent ?? '';
  }
  return text.trim();
}

/** The width of `text` in the element's font. */
function widthOf(text: string, style: CSSStyleDeclaration, canvas: CanvasRenderingContext2D): number {
  canvas.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  const spacing = Number.parseFloat(style.letterSpacing);
  return canvas.measureText(text).width + (Number.isFinite(spacing) ? spacing * text.length : 0);
}

/**
 * Every drawn element of `doc` whose text does not fit, as one line each for the launch
 * test's log (at most `MAX_REPORTED`; the count is of all of them).
 */
export function clippedText(doc: Document): { count: number; lines: string[] } {
  const view = doc.defaultView;
  const lines: string[] = [];
  let count = 0;
  if (!view) return { count, lines };
  const canvas = doc.createElement('canvas').getContext('2d');
  const report = (line: string) => {
    count += 1;
    if (lines.length < MAX_REPORTED) lines.push(line);
  };
  for (const el of Array.from(doc.body.querySelectorAll<HTMLElement>('*'))) {
    const style = view.getComputedStyle(el);
    if (!drawn(el, style)) continue;

    if (el instanceof HTMLInputElement) {
      const type = el.type;
      if (type !== 'text' && type !== 'search' && type !== 'email' && type !== 'url' && type !== 'tel' && type !== 'number') continue;
      const shown = el.value || el.placeholder;
      if (!shown || !canvas) continue;
      const room = el.clientWidth - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight);
      const width = widthOf(shown, style, canvas);
      if (!fieldTextFits(width, room)) report(`field ${nameOf(el)} "${quote(shown)}" text=${Math.ceil(width)} room=${Math.floor(room)}`);
      continue;
    }

    // An inline box takes the width of its text; its block is what can cut it.
    if (style.display === 'inline' || style.display === 'contents') continue;
    // A box that hides what does not fit cuts whatever text is inside it, however deep
    // (a title in a span in a truncating row); any other box only its own text.
    const own = ownText(el);
    const inside = (el.textContent ?? '').trim();
    const cutsX = hides(style.overflowX) ? inside : own;
    const cutsY = hides(style.overflowY) ? inside : '';
    const text = cutsX || cutsY;
    if (!text) continue;
    const why = clipped({
      scrollWidth: cutsX ? el.scrollWidth : 0,
      clientWidth: el.clientWidth,
      scrollHeight: cutsY ? el.scrollHeight : 0,
      clientHeight: el.clientHeight,
      overflowX: style.overflowX,
      overflowY: style.overflowY,
    });
    if (why === 'width') report(`width ${nameOf(el)} "${quote(text)}" text=${el.scrollWidth} room=${el.clientWidth}`);
    else if (why === 'height') report(`height ${nameOf(el)} "${quote(text)}" text=${el.scrollHeight} room=${el.clientHeight}`);
  }
  return { count, lines };
}
