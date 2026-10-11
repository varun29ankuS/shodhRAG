/**
 * A laid-out diagram (`diagramLayout.ts`) as SVG markup.
 *
 * Two forms from one drawing:
 * - `interactive`: themed with the app's tokens (Tailwind classes), each node
 *   a focusable button (`data-node` carries its index) for the answer view;
 * - export: plain presentation attributes in `currentColor` with opacity, so
 *   the ```svg sanitizer keeps it (the "Expand & ask" pop-out, saving).
 *
 * The accent marks only emphasised nodes. A delta is told by a badge and a
 * line style as well as colour: + added (solid), − removed (dashed), Δ
 * changed. Every string is escaped.
 *
 * Pure module, unit-tested with Node (`app/tests/visualDiagram.test.ts`).
 */

import type { DiagramDrawing } from './diagramLayout.ts';
import type { DiagramSpec } from './diagramSpec.ts';

export interface SvgOptions {
  interactive: boolean;
  /** Prefix of element ids (markers), unique per rendering. */
  idPrefix: string;
}

export function escapeXml(text: string): string {
  return text.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c);
}

type Paint = { cls: string; attrs: string };

const p = (cls: string, attrs: string): Paint => ({ cls, attrs });

/** Every themed part, as classes (interactive) and as attributes (export). */
const PAINT = {
  node: p('fill-shodh-surface-2 stroke-shodh-border-strong', 'fill="currentColor" fill-opacity="0.04" stroke="currentColor" stroke-opacity="0.45"'),
  nodeEmphasis: p('fill-shodh-accent-soft stroke-shodh-accent', 'fill="currentColor" fill-opacity="0.12" stroke="currentColor"'),
  nodeAdded: p('fill-shodh-success-soft stroke-shodh-success', 'fill="currentColor" fill-opacity="0.06" stroke="currentColor"'),
  nodeRemoved: p('fill-shodh-surface stroke-shodh-text-faint', 'fill="none" stroke="currentColor" stroke-opacity="0.5"'),
  nodeChanged: p('fill-shodh-warning-soft stroke-shodh-warning', 'fill="currentColor" fill-opacity="0.06" stroke="currentColor"'),
  label: p('fill-shodh-text', 'fill="currentColor"'),
  labelRemoved: p('fill-shodh-text-muted', 'fill="currentColor" fill-opacity="0.6"'),
  sub: p('fill-shodh-text-muted', 'fill="currentColor" fill-opacity="0.65"'),
  wire: p('stroke-shodh-text-faint', 'stroke="currentColor" stroke-opacity="0.6"'),
  wireAdded: p('stroke-shodh-success', 'stroke="currentColor"'),
  wireChanged: p('stroke-shodh-warning', 'stroke="currentColor"'),
  arrow: p('fill-shodh-text-faint', 'fill="currentColor" fill-opacity="0.6"'),
  mask: p('fill-shodh-surface', 'fill="white"'),
  wireLabel: p('fill-shodh-text-muted', 'fill="currentColor" fill-opacity="0.75"'),
  frame: p('fill-none stroke-shodh-border-strong', 'fill="none" stroke="currentColor" stroke-opacity="0.3"'),
  frameLabel: p('fill-shodh-text-muted', 'fill="currentColor" fill-opacity="0.6"'),
  rule: p('stroke-shodh-border-strong', 'stroke="currentColor" stroke-opacity="0.35"'),
  caption: p('fill-shodh-text-muted', 'fill="currentColor" fill-opacity="0.7"'),
  badge: p('fill-shodh-surface stroke-shodh-border-strong', 'fill="white" stroke="currentColor" stroke-opacity="0.45"'),
  badgeText: p('fill-shodh-text-secondary', 'fill="currentColor"'),
  sign: p('fill-shodh-text', 'fill="currentColor"'),
} as const;

const SANS = 'font-family="ui-sans-serif, system-ui, sans-serif"';
const MONO = 'font-family="ui-monospace, SFMono-Regular, Menlo, monospace"';

function paint(part: Paint, o: SvgOptions, extraClass = ''): string {
  if (o.interactive) return `class="${[part.cls, extraClass].filter(Boolean).join(' ')}"`;
  return part.attrs;
}

/**
 * A node's focus and selection ring: its shape's outline turns to text colour
 * and thickens (selection is `aria-pressed`, set without redrawing).
 */
const NODE_CLASS = [
  'cursor-pointer outline-none',
  '[&:focus-visible>:is(rect,circle):first-of-type]:stroke-shodh-text',
  '[&:focus-visible>:is(rect,circle):first-of-type]:[stroke-width:2.5]',
  '[&[aria-pressed=true]>:is(rect,circle):first-of-type]:stroke-shodh-text',
  '[&[aria-pressed=true]>:is(rect,circle):first-of-type]:[stroke-width:2.5]',
].join(' ');

const BADGES = { added: '+', removed: '−', changed: 'Δ' } as const;

/** The SVG markup of `drawing`. */
export function diagramSvg(spec: DiagramSpec, drawing: DiagramDrawing, o: SvgOptions): string {
  const id = (name: string) => `${o.idPrefix}-${name}`;
  const out: string[] = [];
  const title = spec.title || 'Diagram';
  out.push(
    // Interactive: a group, so its node buttons stay exposed to assistive
    // technology (the children of an image are not).
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${drawing.width} ${drawing.height}" width="${drawing.width}" height="${drawing.height}" role="${o.interactive ? 'group' : 'img'}"${o.interactive ? ` aria-label="${escapeXml(title)}"` : ''}>`,
  );
  out.push(`<title>${escapeXml(title)}</title>`);
  out.push(
    `<defs><marker id="${id('arrow')}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 1 L9 5 L0 9 z" ${paint(PAINT.arrow, o)}/></marker></defs>`,
  );

  for (const f of drawing.frames) {
    out.push(`<rect x="${f.x}" y="${f.y}" width="${f.w}" height="${f.h}" rx="8" stroke-dasharray="4 4" ${paint(PAINT.frame, o)}/>`);
    out.push(`<text x="${f.x + 10}" y="${f.y + 16}" font-size="10.5" letter-spacing="0.06em" ${MONO} ${paint(PAINT.frameLabel, o)}>${escapeXml(f.label.toUpperCase())}</text>`);
  }
  for (const r of drawing.rules) {
    out.push(`<line x1="${r.x1}" y1="${r.y1}" x2="${r.x2}" y2="${r.y2}"${r.dashed ? ' stroke-dasharray="4 4"' : ''} ${paint(PAINT.rule, o)}/>`);
  }

  // Lines before boxes, so they pass behind them.
  for (const w of drawing.wires) {
    const edge = spec.edges[w.edge];
    const part = edge.change === 'added' ? PAINT.wireAdded : edge.change === 'changed' ? PAINT.wireChanged : PAINT.wire;
    const dash = w.dashed ? ' stroke-dasharray="5 4"' : '';
    const width = edge.change === 'added' || edge.change === 'changed' ? 2 : 1.5;
    out.push(`<path d="${w.d}" fill="none" stroke-width="${width}"${dash}${w.arrow ? ` marker-end="url(#${id('arrow')})"` : ''} ${paint(part, o)}/>`);
  }
  for (const w of drawing.wires) {
    if (w.label) {
      const l = w.label;
      out.push(`<rect x="${l.x - l.w / 2}" y="${l.y - 9}" width="${l.w}" height="16" rx="3" ${paint(PAINT.mask, o)}/>`);
      out.push(`<text x="${l.x}" y="${l.y + 3}" text-anchor="middle" font-size="10.5" ${MONO} ${paint(PAINT.wireLabel, o)}>${escapeXml(l.text)}</text>`);
    }
    if (w.sign) {
      out.push(`<text x="${w.sign.x}" y="${w.sign.y + 5}" text-anchor="middle" font-size="15" font-weight="700" ${SANS} ${paint(PAINT.sign, o)}>${escapeXml(w.sign.text)}</text>`);
    }
  }

  drawing.boxes.forEach(b => {
    const node = spec.nodes[b.node];
    const part = node.emphasis
      ? PAINT.nodeEmphasis
      : node.change === 'added'
        ? PAINT.nodeAdded
        : node.change === 'removed'
          ? PAINT.nodeRemoved
          : node.change === 'changed'
            ? PAINT.nodeChanged
            : PAINT.node;
    const name = [node.label, node.kind, node.change, node.path, node.cite !== null ? `source ${node.cite}` : null].filter(Boolean).join(', ');
    const group = o.interactive
      ? `<g data-node="${b.node}" role="button" tabindex="0" aria-label="${escapeXml(name)}" class="${NODE_CLASS}">`
      : '<g>';
    out.push(group);
    out.push(`<title>${escapeXml(node.detail ? `${node.label}: ${node.detail}` : node.label)}</title>`);
    const removedDash = node.change === 'removed' ? ' stroke-dasharray="5 4"' : '';
    const strokeWidth = node.emphasis || node.change === 'added' || node.change === 'changed' ? 2 : 1;
    if (b.dot) {
      const cx = b.x + b.w / 2;
      const cy = b.y + b.h / 2;
      out.push(`<circle cx="${cx}" cy="${cy}" r="6" stroke-width="${strokeWidth + 0.5}"${removedDash} ${paint(part, o)}/>`);
      out.push(`<text x="${cx + 12}" y="${cy + 4}" font-size="12.5" font-weight="500" ${SANS} ${paint(PAINT.label, o)}>${escapeXml(b.lines[0])}</text>`);
    } else {
      out.push(`<rect x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" rx="6" stroke-width="${strokeWidth}"${removedDash} ${paint(part, o)}/>`);
      const textPart = node.change === 'removed' ? PAINT.labelRemoved : PAINT.label;
      const cx = b.x + b.w / 2;
      const block = b.lines.length * 16 + (b.sub ? 14 : 0);
      let y = b.y + (b.h - block) / 2 + 12;
      for (const line of b.lines) {
        const strike = node.change === 'removed' ? ' text-decoration="line-through"' : '';
        out.push(`<text x="${cx}" y="${y}" text-anchor="middle" font-size="13" font-weight="600"${strike} ${SANS} ${paint(textPart, o)}>${escapeXml(line)}</text>`);
        y += 16;
      }
      if (b.sub) {
        out.push(`<text x="${cx}" y="${y - 2}" text-anchor="middle" font-size="10.5" ${MONO} ${paint(PAINT.sub, o)}>${escapeXml(b.sub)}</text>`);
      }
      const marks: string[] = [];
      if (node.change) marks.push(BADGES[node.change]);
      if (node.cite !== null) marks.push(String(node.cite));
      else if (node.path !== null) marks.push('↗');
      marks.forEach((mark, i) => {
        const bx = b.x + b.w - 8 - i * 20;
        const by = b.y;
        out.push(`<rect x="${bx - 8}" y="${by - 8}" width="16" height="16" rx="3" ${paint(PAINT.badge, o)}/>`);
        out.push(`<text x="${bx}" y="${by + 4}" text-anchor="middle" font-size="10.5" font-weight="600" ${MONO} ${paint(PAINT.badgeText, o)}>${escapeXml(mark)}</text>`);
      });
    }
    out.push('</g>');
  });

  for (const c of drawing.captions) {
    out.push(`<text x="${c.x}" y="${c.y}" text-anchor="${c.anchor}" font-size="${c.mono ? 11 : 22}"${c.mono ? '' : ' font-weight="700"'} ${c.mono ? MONO : SANS} ${paint(PAINT.caption, o)}>${escapeXml(c.text)}</text>`);
  }
  out.push('</svg>');
  return out.join('');
}
