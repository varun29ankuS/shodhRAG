import { useCallback, useId, useLayoutEffect, useMemo, useRef } from 'react';
import DOMPurify from 'dompurify';
import { cn } from '../../../lib/utils';
import { FocusFrame } from '../../focus/FocusFrame';
import { svgTarget } from '../../focus/targets';
import { BlockError } from './VisualBlocks';
import { type SvgSanitizeResult, sanitizeSvg } from './svgSanitize';

const LOCAL_HREF = /^#[A-Za-z_][\w.-]*$/;

/**
 * A DOMPurify instance of our own: its hook must not affect other users of
 * the shared default instance (mermaid).
 */
let sketchPurifier: ReturnType<typeof DOMPurify> | null = null;

function purifier(): ReturnType<typeof DOMPurify> {
  if (!sketchPurifier) {
    const instance = DOMPurify(window);
    // <use> and gradients may only point inside the drawing.
    instance.addHook('uponSanitizeAttribute', (_node, data) => {
      if ((data.attrName === 'href' || data.attrName === 'xlink:href') && !LOCAL_HREF.test(data.attrValue.trim())) data.keepAttr = false;
    });
    sketchPurifier = instance;
  }
  return sketchPurifier;
}

/**
 * Second, independent sanitizing layer (the first is the allowlist rebuild
 * in svgSanitize.ts): DOMPurify's SVG profile with everything that can
 * script, style, animate, link or embed forbidden outright, and links
 * limited to local `#id` references.
 */
function purify(svg: string): string {
  return purifier().sanitize(svg, {
    USE_PROFILES: { svg: true },
    ADD_TAGS: ['use'],
    FORBID_TAGS: ['script', 'style', 'foreignObject', 'set', 'animate', 'animateMotion', 'animateTransform', 'a', 'image', 'feImage', 'iframe'],
    FORBID_ATTR: ['class'],
    ALLOW_DATA_ATTR: false,
  });
}

/** Sanitized markup of a model-written SVG, with ids unique to this rendering. */
export function useSvgSketch(source: string): { result: SvgSanitizeResult; markup: string; seed: number } {
  const reactId = useId();
  const prefix = `sk${reactId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const result = useMemo(() => sanitizeSvg(source.trim(), { idPrefix: prefix }), [source, prefix]);
  const markup = useMemo(() => (result.ok ? purify(result.svg) : ''), [result]);
  // A stable seed: the hand-drawn wobble is the same on every render of one source.
  const seed = useMemo(() => {
    let h = 2166136261;
    for (let i = 0; i < source.length; i++) h = Math.imul(h ^ source.charCodeAt(i), 16777619);
    return (h >>> 0) % 2147483646 + 1;
  }, [source]);
  return { result, markup, seed };
}

const SKETCH_SHAPES = 'path, line, rect, circle, ellipse, polygon, polyline';
const NON_DRAWN = 'defs, marker, clipPath, mask, pattern, symbol';

/** A presentation value of a shape as written, inherited from its ancestors when unset. */
function paintOf(el: Element, prop: string, root: Element): string | null {
  for (let node: Element | null = el; node; node = node === root ? null : node.parentElement) {
    const styled = (node as SVGElement).style?.getPropertyValue(prop);
    if (styled) return styled.trim();
    const attr = node.getAttribute(prop);
    if (attr) return attr.trim();
  }
  return null;
}

function hasMarkers(el: Element): boolean {
  const style = (el as SVGElement).style;
  return ['marker-start', 'marker-mid', 'marker-end'].some(m => el.hasAttribute(m) || Boolean(style?.getPropertyValue(m)));
}

/**
 * Redraw plain shapes hand-drawn with roughjs. Shapes roughjs cannot redraw
 * faithfully keep their original form: ones with markers (arrowheads),
 * rounded rectangles, shapes referenced by id, gradient or pattern paint,
 * and anything inside definitions.
 */
async function applySketch(svg: SVGSVGElement, seed: number): Promise<void> {
  const { default: rough } = await import('roughjs');
  if (!svg.isConnected) return;
  const rc = rough.svg(svg);
  const shapes = Array.from(svg.querySelectorAll<SVGGraphicsElement>(SKETCH_SHAPES));
  for (const el of shapes) {
    if (el.closest(NON_DRAWN) || el.id || hasMarkers(el)) continue;
    const fill = paintOf(el, 'fill', svg) ?? 'currentColor';
    const stroke = paintOf(el, 'stroke', svg) ?? 'none';
    if (/url\(/i.test(fill) || /url\(/i.test(stroke)) continue;
    const width = Number.parseFloat(paintOf(el, 'stroke-width', svg) ?? '1');
    const options = {
      seed,
      roughness: 1.1,
      bowing: 0.8,
      stroke: stroke === 'none' ? 'none' : stroke,
      strokeWidth: Number.isFinite(width) && width > 0 ? width : 1,
      fill: fill === 'none' || fill === 'transparent' ? undefined : fill,
      fillStyle: fill.includes('--c-surface') ? 'solid' : 'hachure',
      hachureGap: 5,
    };
    let drawn: SVGGElement | null = null;
    const tag = el.tagName.toLowerCase();
    try {
      if (tag === 'line') {
        const l = el as SVGLineElement;
        drawn = rc.line(l.x1.baseVal.value, l.y1.baseVal.value, l.x2.baseVal.value, l.y2.baseVal.value, { ...options, fill: undefined });
      } else if (tag === 'rect') {
        const r = el as SVGRectElement;
        if (r.rx.baseVal.value > 0 || r.ry.baseVal.value > 0) continue;
        drawn = rc.rectangle(r.x.baseVal.value, r.y.baseVal.value, r.width.baseVal.value, r.height.baseVal.value, options);
      } else if (tag === 'circle') {
        const c = el as SVGCircleElement;
        drawn = rc.circle(c.cx.baseVal.value, c.cy.baseVal.value, c.r.baseVal.value * 2, options);
      } else if (tag === 'ellipse') {
        const e = el as SVGEllipseElement;
        drawn = rc.ellipse(e.cx.baseVal.value, e.cy.baseVal.value, e.rx.baseVal.value * 2, e.ry.baseVal.value * 2, options);
      } else if (tag === 'polygon' || tag === 'polyline') {
        const list = (el as SVGPolygonElement).points;
        const points: [number, number][] = [];
        for (let i = 0; i < list.numberOfItems; i++) points.push([list.getItem(i).x, list.getItem(i).y]);
        if (points.length < 2) continue;
        drawn = tag === 'polygon' ? rc.polygon(points, options) : rc.linearPath(points, { ...options, fill: undefined });
      } else if (tag === 'path') {
        const d = el.getAttribute('d');
        if (!d) continue;
        drawn = rc.path(d, options);
      }
    } catch {
      // Geometry roughjs cannot read: keep the original shape.
      drawn = null;
    }
    if (!drawn) continue;
    for (const attr of ['transform', 'opacity']) {
      const v = el.getAttribute(attr);
      if (v) drawn.setAttribute(attr, v);
    }
    // Theme colours written as CSS variables only resolve in style, not in presentation attributes.
    for (const part of Array.from(drawn.querySelectorAll('path'))) {
      for (const prop of ['stroke', 'fill']) {
        const v = part.getAttribute(prop);
        if (v && v.includes('var(')) {
          part.removeAttribute(prop);
          part.style.setProperty(prop, v);
        }
      }
    }
    el.replaceWith(drawn);
  }
}

/** Draws sanitized SVG markup; optionally redrawn hand-drawn. */
export function SketchSurface({
  markup,
  sketch,
  seed,
  label,
  className,
}: {
  markup: string;
  sketch: boolean;
  seed: number;
  label: string;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const host = ref.current;
    if (!host) return;
    // Markup passed both the allowlist rebuild and DOMPurify (see useSvgSketch).
    host.innerHTML = markup;
    const svg = host.querySelector('svg');
    if (!svg) return;
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('focusable', 'false');
    if (sketch) {
      applySketch(svg, seed).catch(() => {
        // roughjs failed to load: the exact drawing stays.
      });
    }
  }, [markup, sketch, seed]);
  return <div ref={ref} role="img" aria-label={label} className={cn('text-shodh-text', className)} />;
}

/** A ```svg block in an answer. */
export function SvgBlock({ source }: { source: string }) {
  const { result, markup, seed } = useSvgSketch(source);
  const title = result.ok ? result.title : '';
  const getTarget = useCallback(() => svgTarget(source.trim(), title), [source, title]);
  if ('error' in result) {
    const error = result.error;
    return (
      <BlockError
        title="Sketch not drawn"
        message={error}
        source={source}
        ask={{ noun: 'sketch', getTarget: () => svgTarget(source.trim(), null, error) }}
      />
    );
  }
  return (
    <FocusFrame noun="sketch" getTarget={getTarget} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface p-4 overflow-x-auto scrollbar-thin">
        <SketchSurface
          markup={markup}
          sketch={result.sketch}
          seed={seed}
          label={title || 'Sketch'}
          className="flex justify-center [&_svg]:max-w-full [&_svg]:h-auto [&_svg]:max-h-[560px]"
        />
        {result.removed.length > 0 && (
          <figcaption className="mt-2 text-[11.5px] text-shodh-text-muted">
            {`Not drawn for safety: ${result.removed.slice(0, 6).join(', ')}${result.removed.length > 6 ? '…' : ''}.`}
          </figcaption>
        )}
      </figure>
    </FocusFrame>
  );
}
