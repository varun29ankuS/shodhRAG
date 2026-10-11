/**
 * Sketches written by the agent as ```svg fences.
 *
 * The source is parsed by a small strict XML reader and rebuilt from an
 * allowlist: only drawing elements and presentation attributes survive.
 * Removed, whatever their spelling: scripts, styles, foreignObject,
 * animation (set/animate can rewrite attributes at runtime), links, event
 * handlers, `class` (it would pull in the app's own CSS), external or `javascript:` references (only `#id` and
 * `url(#id)` are kept), CSS escapes, DOCTYPE/entities. Element ids are
 * prefixed per rendering so two sketches (or the answer copy and the
 * enlarged copy) never resolve each other's markers. Black and white ink
 * are mapped to theme colours.
 *
 * The browser renderer passes the result through DOMPurify as a second,
 * independent layer.
 *
 * Pure module, unit-tested with Node (`app/tests/visualSvg.test.ts`).
 */

/** Largest SVG source rendered, in characters. Also the cap kept with a side thread. */
export const SVG_MAX_CHARS = 100_000;
/** Most elements kept from one SVG. */
export const SVG_MAX_ELEMENTS = 4_000;
const MAX_DEPTH = 64;
/** Largest width/height of the drawing, in px. */
const MAX_SIZE = 4_000;

const ALLOWED_ELEMENTS = new Set([
  'svg', 'g', 'defs', 'title', 'desc', 'symbol', 'use',
  'path', 'line', 'polyline', 'polygon', 'rect', 'circle', 'ellipse',
  'text', 'tspan', 'textPath',
  'marker', 'linearGradient', 'radialGradient', 'stop', 'clipPath', 'mask', 'pattern',
]);

/** Elements removed with everything inside them. Anything not allowed is removed the same way. */
const ALWAYS_DROPPED = ['script', 'style', 'foreignObject', 'set', 'animate', 'animateTransform', 'animateMotion', 'a', 'iframe', 'image', 'feImage'];

const ALLOWED_ATTRIBUTES = new Set([
  'id', 'transform', 'style',
  'x', 'y', 'x1', 'y1', 'x2', 'y2', 'cx', 'cy', 'r', 'rx', 'ry', 'width', 'height', 'd', 'points', 'pathLength',
  'viewBox', 'preserveAspectRatio',
  'fill', 'fill-opacity', 'fill-rule', 'stroke', 'stroke-width', 'stroke-opacity', 'stroke-linecap', 'stroke-linejoin',
  'stroke-dasharray', 'stroke-dashoffset', 'stroke-miterlimit', 'opacity', 'color', 'visibility', 'display',
  'vector-effect', 'paint-order', 'shape-rendering',
  'marker-start', 'marker-mid', 'marker-end', 'markerWidth', 'markerHeight', 'markerUnits', 'refX', 'refY', 'orient',
  'clip-path', 'clip-rule', 'clipPathUnits', 'mask', 'maskUnits', 'maskContentUnits',
  'patternUnits', 'patternContentUnits', 'patternTransform',
  'gradientUnits', 'gradientTransform', 'spreadMethod', 'fx', 'fy', 'fr', 'offset', 'stop-color', 'stop-opacity',
  'font-family', 'font-size', 'font-style', 'font-weight', 'text-anchor', 'dominant-baseline', 'alignment-baseline',
  'baseline-shift', 'letter-spacing', 'word-spacing', 'text-decoration', 'dx', 'dy', 'rotate', 'textLength',
  'lengthAdjust', 'startOffset', 'href', 'xlink:href', 'xml:space', 'role', 'aria-label', 'aria-hidden',
]);

/** CSS properties kept in `style` attributes. */
const ALLOWED_STYLE = new Set([
  'fill', 'fill-opacity', 'fill-rule', 'stroke', 'stroke-width', 'stroke-opacity', 'stroke-linecap', 'stroke-linejoin',
  'stroke-dasharray', 'stroke-dashoffset', 'stroke-miterlimit', 'opacity', 'color', 'visibility', 'display',
  'font-family', 'font-size', 'font-style', 'font-weight', 'text-anchor', 'dominant-baseline', 'letter-spacing',
  'marker-start', 'marker-mid', 'marker-end', 'paint-order', 'vector-effect', 'stop-color', 'stop-opacity',
]);

const PAINT_ATTRIBUTES = new Set(['fill', 'stroke', 'color', 'stop-color']);

interface XmlElement {
  name: string;
  attrs: [string, string][];
  children: XmlNode[];
}

type XmlNode = XmlElement | { text: string };

export type SvgSanitizeResult =
  | { ok: true; svg: string; sketch: boolean; title: string; width: number; height: number; removed: string[] }
  | { ok: false; error: string };

class SvgError extends Error {}

function decodeEntities(value: string): string {
  return value.replace(/&(#x[0-9a-fA-F]+|#\d+|lt|gt|amp|quot|apos);/g, (whole, body: string) => {
    switch (body) {
      case 'lt': return '<';
      case 'gt': return '>';
      case 'amp': return '&';
      case 'quot': return '"';
      case 'apos': return "'";
      default: {
        const code = body[1] === 'x' ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
        return Number.isFinite(code) && code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff) ? String.fromCodePoint(code) : '';
      }
    }
  });
}

/** Strict reader for the XML subset an SVG drawing needs. */
function parseXml(source: string): XmlElement {
  let i = 0;
  const root: XmlElement = { name: '#root', attrs: [], children: [] };
  const stack: XmlElement[] = [root];
  const nameRe = /^[A-Za-z_][\w.:-]*/;
  let count = 0;

  while (i < source.length) {
    const lt = source.indexOf('<', i);
    const textEnd = lt < 0 ? source.length : lt;
    if (textEnd > i) {
      const raw = source.slice(i, textEnd);
      if (raw.trim()) stack[stack.length - 1].children.push({ text: decodeEntities(raw) });
      i = textEnd;
      continue;
    }
    if (source.startsWith('<!--', i)) {
      const end = source.indexOf('-->', i + 4);
      if (end < 0) throw new SvgError('A comment is not closed.');
      i = end + 3;
      continue;
    }
    if (source.startsWith('<![CDATA[', i)) {
      const end = source.indexOf(']]>', i + 9);
      if (end < 0) throw new SvgError('A CDATA section is not closed.');
      const body = source.slice(i + 9, end);
      if (body.trim()) stack[stack.length - 1].children.push({ text: body });
      i = end + 3;
      continue;
    }
    if (source.startsWith('<!', i)) throw new SvgError('DOCTYPE and entity declarations are not allowed.');
    if (source.startsWith('<?', i)) {
      const end = source.indexOf('?>', i + 2);
      if (end < 0) throw new SvgError('A processing instruction is not closed.');
      i = end + 2;
      continue;
    }
    if (source.startsWith('</', i)) {
      const m = nameRe.exec(source.slice(i + 2));
      if (!m) throw new SvgError('A closing tag has no name.');
      const close = source.indexOf('>', i + 2 + m[0].length);
      if (close < 0 || source.slice(i + 2 + m[0].length, close).trim()) throw new SvgError(`The closing tag </${m[0]}> is malformed.`);
      const open = stack.pop();
      if (!open || open === root || open.name !== m[0]) throw new SvgError(`</${m[0]}> does not match the open element${open && open !== root ? ` <${open.name}>` : ''}.`);
      i = close + 1;
      continue;
    }
    // Start tag.
    const m = nameRe.exec(source.slice(i + 1));
    if (!m) throw new SvgError('A "<" is not followed by an element name.');
    const el: XmlElement = { name: m[0], attrs: [], children: [] };
    if (++count > SVG_MAX_ELEMENTS) throw new SvgError(`The SVG has more than ${SVG_MAX_ELEMENTS} elements.`);
    i += 1 + m[0].length;
    let selfClosing = false;
    for (;;) {
      while (i < source.length && /\s/.test(source[i])) i++;
      if (i >= source.length) throw new SvgError(`The tag <${el.name}> is not closed.`);
      if (source[i] === '>') {
        i++;
        break;
      }
      if (source.startsWith('/>', i)) {
        selfClosing = true;
        i += 2;
        break;
      }
      const a = nameRe.exec(source.slice(i));
      if (!a) throw new SvgError(`Unreadable attribute in <${el.name}>.`);
      i += a[0].length;
      while (i < source.length && /\s/.test(source[i])) i++;
      if (source[i] !== '=') throw new SvgError(`Attribute ${a[0]} in <${el.name}> has no value.`);
      i++;
      while (i < source.length && /\s/.test(source[i])) i++;
      const quote = source[i];
      if (quote !== '"' && quote !== "'") throw new SvgError(`Attribute ${a[0]} in <${el.name}> must be quoted.`);
      const end = source.indexOf(quote, i + 1);
      if (end < 0) throw new SvgError(`Attribute ${a[0]} in <${el.name}> is not closed.`);
      const value = source.slice(i + 1, end);
      if (value.includes('<')) throw new SvgError(`Attribute ${a[0]} in <${el.name}> contains "<".`);
      if (el.attrs.some(([k]) => k === a[0])) throw new SvgError(`Attribute ${a[0]} appears twice in <${el.name}>.`);
      el.attrs.push([a[0], decodeEntities(value)]);
      i = end + 1;
    }
    stack[stack.length - 1].children.push(el);
    if (!selfClosing) {
      if (stack.length > MAX_DEPTH) throw new SvgError('The SVG is nested too deeply.');
      stack.push(el);
    }
  }
  if (stack.length > 1) throw new SvgError(`<${stack[stack.length - 1].name}> is not closed.`);
  return root;
}

const LOCAL_REF = /^#([A-Za-z_][\w.-]*)$/;
const URL_FN = /url\(\s*(['"]?)(.*?)\1\s*\)/gi;

/** Rewrite every `url(...)` to point at a prefixed local id; null when any is not local. */
function rewriteUrls(value: string, prefix: string): string | null {
  let safe = true;
  const out = value.replace(URL_FN, (_whole, _q: string, target: string) => {
    const m = LOCAL_REF.exec(target.trim());
    if (!m) {
      safe = false;
      return '';
    }
    return `url(#${prefix}${m[1]})`;
  });
  // A url( the pattern did not match (unbalanced, nested quotes) is not safe either.
  if (!safe || /url\s*\(/i.test(out.replace(/url\(#[\w.-]+\)/g, ''))) return null;
  return out;
}

/** Values that must never pass, in any attribute. */
function hostileValue(value: string): boolean {
  const flat = value.toLowerCase().replace(/[\s\u0000-\u001f]+/g, '');
  return value.includes('\\') || flat.includes('javascript:') || flat.includes('vbscript:') || flat.includes('data:') || flat.includes('expression(') || flat.includes('@import') || flat.includes('<');
}

const BLACK = /^(?:black|#000|#000000|rgb\(\s*0\s*,\s*0\s*,\s*0\s*\))$/i;
const WHITE = /^(?:white|#fff|#ffffff|rgb\(\s*255\s*,\s*255\s*,\s*255\s*\))$/i;

/** Theme mapping of ink colours: black follows the text colour, white the surface. */
function themePaint(value: string): { attr: string } | { style: string } {
  const v = value.trim();
  if (BLACK.test(v)) return { attr: 'currentColor' };
  if (WHITE.test(v)) return { style: 'var(--c-surface)' };
  return { attr: v };
}

function sanitizeStyle(value: string, prefix: string): string {
  const kept: string[] = [];
  for (const decl of value.split(';')) {
    const colon = decl.indexOf(':');
    if (colon < 0) continue;
    const prop = decl.slice(0, colon).trim().toLowerCase();
    let val = decl.slice(colon + 1).trim();
    if (!ALLOWED_STYLE.has(prop) || !val || hostileValue(val) || /!important/i.test(val)) continue;
    if (/url\s*\(/i.test(val)) {
      const rewritten = rewriteUrls(val, prefix);
      if (rewritten === null) continue;
      val = rewritten;
    }
    if (PAINT_ATTRIBUTES.has(prop)) {
      const paint = themePaint(val);
      val = 'attr' in paint ? paint.attr : paint.style;
    }
    kept.push(`${prop}: ${val}`);
  }
  return kept.join('; ');
}

function escapeAttr(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function escapeText(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function num(value: string | undefined): number | null {
  if (value === undefined) return null;
  const m = /^\s*([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)\s*(?:px)?\s*$/i.exec(value);
  return m ? Number(m[1]) : null;
}

function parseViewBox(value: string | undefined): [number, number, number, number] | null {
  if (!value) return null;
  const parts = value.trim().split(/[\s,]+/).map(Number);
  if (parts.length !== 4 || parts.some(n => !Number.isFinite(n)) || parts[2] <= 0 || parts[3] <= 0) return null;
  return [parts[0], parts[1], parts[2], parts[3]];
}

function textOf(node: XmlNode): string {
  if ('text' in node) return node.text;
  return node.children.map(textOf).join(' ');
}

/** A first-line `<!-- sketch -->` asks for the hand-drawn style. */
export function wantsSketch(source: string): boolean {
  return /^\s*(?:<\?xml[^>]*\?>\s*)?<!--\s*sketch\s*-->/i.test(source);
}

export interface SvgSanitizeOptions {
  /** Prefix for every element id, unique per rendering (letters, digits, -). */
  idPrefix?: string;
}

/** Parse, filter and rebuild a model-written SVG. Never throws. */
export function sanitizeSvg(source: string, options: SvgSanitizeOptions = {}): SvgSanitizeResult {
  if (source.length > SVG_MAX_CHARS) return { ok: false, error: `The SVG is larger than ${SVG_MAX_CHARS / 1000} KB.` };
  const prefix = `${(options.idPrefix ?? 'svg').replace(/[^A-Za-z0-9-]/g, '') || 'svg'}-`;
  let doc: XmlElement;
  try {
    doc = parseXml(source);
  } catch (error) {
    return { ok: false, error: error instanceof SvgError ? `The SVG is not well-formed: ${error.message}` : 'The SVG could not be read.' };
  }
  const elements = doc.children.filter((n): n is XmlElement => !('text' in n));
  if (elements.length !== 1 || elements[0].name !== 'svg') return { ok: false, error: 'The block must hold exactly one <svg> element.' };
  const svgEl = elements[0];

  const removed = new Set<string>();
  const rootAttrs = new Map(svgEl.attrs);
  let box = parseViewBox(rootAttrs.get('viewBox'));
  const w = num(rootAttrs.get('width'));
  const h = num(rootAttrs.get('height'));
  if (!box) {
    if (w && h && w > 0 && h > 0) box = [0, 0, w, h];
    else return { ok: false, error: 'The <svg> needs a viewBox (or numeric width and height).' };
  }
  // Natural size: the given size, else the viewBox, kept within bounds and aspect.
  let width = w && w > 0 ? w : box[2];
  let height = h && h > 0 ? h : box[3];
  if (!(w && h)) height = (width * box[3]) / box[2];
  const scale = Math.min(1, MAX_SIZE / Math.max(width, height));
  width = Math.max(1, Math.round(width * scale));
  height = Math.max(1, Math.round(height * scale));

  let title = '';
  const out: string[] = [];

  const emit = (el: XmlElement, depth: number) => {
    if (!ALLOWED_ELEMENTS.has(el.name)) {
      removed.add(ALWAYS_DROPPED.includes(el.name) ? `<${el.name}>` : `<${el.name}> (not supported)`);
      return;
    }
    if (el.name === 'svg' && depth > 0) {
      removed.add('nested <svg>');
      return;
    }
    if (el.name === 'title' && !title) title = textOf(el).replace(/\s+/g, ' ').trim().slice(0, 120);
    const attrs: string[] = [];
    const styles: string[] = [];
    for (const [name, rawValue] of el.attrs) {
      if (/^on/i.test(name)) {
        removed.add('event handlers');
        continue;
      }
      if (name === 'xmlns' || name.startsWith('xmlns:')) continue;
      if (!ALLOWED_ATTRIBUTES.has(name)) {
        removed.add(`${name} attribute`);
        continue;
      }
      if (depth === 0 && (name === 'width' || name === 'height' || name === 'viewBox')) continue;
      let value = rawValue;
      if (name === 'href' || name === 'xlink:href') {
        const m = LOCAL_REF.exec(value.trim());
        if (!m) {
          removed.add('external links');
          continue;
        }
        attrs.push(`href="${escapeAttr(`#${prefix}${m[1]}`)}"`);
        continue;
      }
      if (name === 'id') {
        if (!/^[A-Za-z_][\w.-]*$/.test(value)) continue;
        attrs.push(`id="${escapeAttr(prefix + value)}"`);
        continue;
      }
      if (name === 'style') {
        const style = sanitizeStyle(value, prefix);
        if (style) styles.push(style);
        else if (value.trim()) removed.add('unsafe styles');
        continue;
      }
      if (hostileValue(value)) {
        removed.add(`unsafe ${name} value`);
        continue;
      }
      if (/url\s*\(/i.test(value)) {
        const rewritten = rewriteUrls(value, prefix);
        if (rewritten === null) {
          removed.add('external references');
          continue;
        }
        value = rewritten;
      }
      if (PAINT_ATTRIBUTES.has(name)) {
        const paint = themePaint(value);
        if ('style' in paint) {
          styles.push(`${name}: ${paint.style}`);
          continue;
        }
        value = paint.attr;
      }
      attrs.push(`${name}="${escapeAttr(value)}"`);
    }
    if (depth === 0) {
      attrs.unshift(`xmlns="http://www.w3.org/2000/svg"`, `viewBox="${box!.join(' ')}"`, `width="${width}"`, `height="${height}"`);
      // Unset fill is black in SVG: make it follow the theme's text colour.
      if (!el.attrs.some(([k]) => k === 'fill')) attrs.push('fill="currentColor"');
    }
    if (styles.length > 0) attrs.push(`style="${escapeAttr(styles.join('; '))}"`);
    out.push(`<${el.name}${attrs.length ? ` ${attrs.join(' ')}` : ''}>`);
    for (const child of el.children) {
      if ('text' in child) out.push(escapeText(child.text));
      else emit(child, depth + 1);
    }
    out.push(`</${el.name}>`);
  };

  emit(svgEl, 0);
  return { ok: true, svg: out.join(''), sketch: wantsSketch(source), title, width, height, removed: Array.from(removed) };
}

/**
 * A shorter SVG for an agent's context: long path data and point lists are
 * cut (they say little in words), structure and text are kept.
 */
export function compactSvg(source: string, maxValue = 120): string {
  return source
    .replace(/\s(d|points)\s*=\s*(["'])([\s\S]*?)\2/g, (whole, name: string, q: string, value: string) =>
      value.length > maxValue ? ` ${name}=${q}${value.slice(0, maxValue)}…${q}` : whole)
    .replace(/[ \t]+\n/g, '\n');
}
