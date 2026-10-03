import { useCallback, useId, useMemo, useRef, useState } from 'react';
import type React from 'react';
import { FocusFrame } from '../../focus/FocusFrame';
import { plotTarget } from '../../focus/targets';
import type { FocusParamValue } from '../../focus/focusTypes';
import { BlockError } from './VisualBlocks';
import { FrameAxes, HALO_STYLE, ParamSliders, useElementWidth } from './VisualControls';
import { colorVar } from './palette';
import {
  type FrameLayout,
  type PlotItem,
  type PlotSpec,
  type Point,
  arrowHead,
  frameLayout,
  initialValues,
  parsePlotSpec,
  sampleItem,
  segmentsPath,
  snapParam,
} from './plotSpec';

function evalPair(pair: [(s: ArrayLike<number>) => number, (s: ArrayLike<number>) => number], values: readonly number[]): Point {
  return [pair[0](values), pair[1](values)];
}

function finitePoint(p: Point): boolean {
  return Number.isFinite(p[0]) && Number.isFinite(p[1]);
}

/** One plot item drawn in pixel space. */
function PlotMark({
  item,
  values,
  spec,
  layout,
  onDragStart,
}: {
  item: PlotItem;
  values: readonly number[];
  spec: PlotSpec;
  layout: FrameLayout;
  onDragStart: (item: Extract<PlotItem, { type: 'point' }>, e: React.PointerEvent<SVGCircleElement>) => void;
}) {
  const color = colorVar(item.color);
  switch (item.type) {
    case 'function':
    case 'parametric': {
      const d = segmentsPath(sampleItem(item, values, spec), layout);
      return d ? <path d={d} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" /> : null;
    }
    case 'point': {
      const p = evalPair(item.at, values);
      if (!finitePoint(p)) return null;
      const cx = layout.sx(p[0]);
      const cy = layout.sy(p[1]);
      return (
        <g>
          <circle cx={cx} cy={cy} r={5} fill={color} stroke="var(--c-surface)" strokeWidth={1.5} />
          {item.drag && (
            <circle
              cx={cx}
              cy={cy}
              r={14}
              fill="transparent"
              className="cursor-grab active:cursor-grabbing"
              onPointerDown={e => onDragStart(item, e)}
            >
              <title>{`Drag to change ${[item.drag.x, item.drag.y].filter(Boolean).join(' and ')} (or use the sliders)`}</title>
            </circle>
          )}
          {item.label && (
            <text x={cx + 8} y={cy - 8} fontSize={12} fill={color} style={HALO_STYLE}>
              {item.label}
            </text>
          )}
        </g>
      );
    }
    case 'vector':
    case 'segment': {
      const from = evalPair(item.from, values);
      const to = evalPair(item.to, values);
      if (!finitePoint(from) || !finitePoint(to)) return null;
      const a: Point = [layout.sx(from[0]), layout.sy(from[1])];
      const b: Point = [layout.sx(to[0]), layout.sy(to[1])];
      const head = item.type === 'vector' ? arrowHead(a, b) : null;
      return (
        <g>
          <line x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} stroke={color} strokeWidth={2} strokeLinecap="round" />
          {head && <polygon points={head.map(p => p.join(',')).join(' ')} fill={color} />}
          {item.label && (
            <text x={(a[0] + b[0]) / 2 + 6} y={(a[1] + b[1]) / 2 - 6} fontSize={12} fill={color} style={HALO_STYLE}>
              {item.label}
            </text>
          )}
        </g>
      );
    }
    case 'label': {
      const at = evalPair(item.at, values);
      if (!finitePoint(at)) return null;
      return (
        <text x={layout.sx(at[0])} y={layout.sy(at[1])} fontSize={12} fill={colorVar(item.color)} style={HALO_STYLE}>
          {item.text}
        </text>
      );
    }
  }
}

/** A plot with live sliders; draggable points move their parameters. */
export function InteractivePlot({
  spec,
  values,
  onValues,
  maxHeight,
}: {
  spec: PlotSpec;
  values: number[];
  onValues: (next: number[]) => void;
  maxHeight: number;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const clipId = `plot-clip-${useId().replace(/[^a-zA-Z0-9]/g, '')}`;
  const width = useElementWidth(hostRef);
  const x: [number, number] = [spec.x.min, spec.x.max];
  const y: [number, number] = [spec.y.min, spec.y.max];
  const layout = useMemo(
    () => (width > 0 ? frameLayout(width, [spec.x.min, spec.x.max], [spec.y.min, spec.y.max], { equal: spec.equal, maxHeight }) : null),
    [width, spec, maxHeight],
  );

  const setParam = useCallback((index: number, value: number) => {
    const next = values.slice();
    next[index] = value;
    onValues(next);
  }, [values, onValues]);

  const drag = useRef<{ pointerId: number; item: Extract<PlotItem, { type: 'point' }> } | null>(null);
  const onDragStart = useCallback((item: Extract<PlotItem, { type: 'point' }>, e: React.PointerEvent<SVGCircleElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    drag.current = { pointerId: e.pointerId, item };
    svgRef.current?.setPointerCapture(e.pointerId);
  }, []);
  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    const svg = svgRef.current;
    if (!d || d.pointerId !== e.pointerId || !svg || !layout || !d.item.drag) return;
    const rect = svg.getBoundingClientRect();
    const [dx, dy] = layout.invert(((e.clientX - rect.left) / rect.width) * layout.width, ((e.clientY - rect.top) / rect.height) * layout.height);
    const next = values.slice();
    const names = spec.params.map(p => p.name);
    for (const [axis, v] of [[d.item.drag.x, dx], [d.item.drag.y, dy]] as const) {
      if (!axis) continue;
      const i = names.indexOf(axis);
      if (i >= 0) next[i] = snapParam(spec.params[i], v);
    }
    onValues(next);
  };
  const endDrag = (e: React.PointerEvent<SVGSVGElement>) => {
    if (drag.current?.pointerId === e.pointerId) drag.current = null;
  };

  const legend = spec.items.filter(i => (i.type === 'function' || i.type === 'parametric') && i.label);
  const description = [
    spec.title || 'Plot',
    `x from ${spec.x.min} to ${spec.x.max}${spec.x.label ? ` (${spec.x.label})` : ''}`,
    `y from ${spec.y.min} to ${spec.y.max}${spec.y.label ? ` (${spec.y.label})` : ''}`,
    spec.params.length > 0 ? `${spec.params.length} adjustable parameter${spec.params.length === 1 ? '' : 's'} below` : '',
  ].filter(Boolean).join('; ');

  return (
    <div className="flex flex-col gap-3">
      {/* Always present: keeps the plot clear of the "Expand & ask" button. */}
      <div className="min-h-[28px] flex flex-wrap items-baseline gap-x-4 gap-y-1 pr-28">
        {spec.title && <h4 className="m-0 text-[14px] font-semibold text-shodh-text">{spec.title}</h4>}
        {legend.map((item, i) => (
          <span key={i} className="inline-flex items-center gap-1.5 text-[12px] text-shodh-text-secondary">
            <span aria-hidden="true" className="inline-block w-3 h-0.5 rounded" style={{ background: colorVar(item.color) }} />
            {'label' in item ? item.label : ''}
          </span>
        ))}
      </div>
      <div ref={hostRef} className="w-full flex justify-center">
        {layout && (
          <svg
            ref={svgRef}
            role="img"
            aria-label={description}
            width={layout.width}
            height={layout.height}
            viewBox={`0 0 ${layout.width} ${layout.height}`}
            className="max-w-full h-auto touch-none select-none"
            onPointerMove={onPointerMove}
            onPointerUp={endDrag}
            onPointerCancel={endDrag}
          >
            <defs>
              <clipPath id={clipId}>
                <rect x={layout.left} y={layout.top} width={layout.innerWidth} height={layout.innerHeight} />
              </clipPath>
            </defs>
            <FrameAxes layout={layout} x={x} y={y} xLabel={spec.x.label} yLabel={spec.y.label} />
            <g clipPath={`url(#${clipId})`}>
              {spec.items.map((item, i) => (
                <PlotMark key={i} item={item} values={values} spec={spec} layout={layout} onDragStart={onDragStart} />
              ))}
            </g>
          </svg>
        )}
      </div>
      <ParamSliders params={spec.params} values={values} onChange={setParam} />
    </div>
  );
}

/** Current parameter positions as name/value pairs. */
export function paramValues(spec: PlotSpec | null, values: readonly number[]): FocusParamValue[] {
  return spec ? spec.params.map((p, i) => ({ name: p.name, value: values[i] ?? p.value })) : [];
}

/** A ```plot block in an answer. */
export function PlotBlock({ source }: { source: string }) {
  const result = useMemo(() => parsePlotSpec(source), [source]);
  const spec = result.ok ? result.spec : null;
  const [values, setValues] = useState<number[]>(() => (spec ? initialValues(spec.params) : []));
  // A new spec (the answer is still streaming or was edited) restarts from its own values.
  const [specSeen, setSpecSeen] = useState(spec);
  if (spec !== specSeen) {
    setSpecSeen(spec);
    setValues(spec ? initialValues(spec.params) : []);
  }
  const valuesRef = useRef(values);
  valuesRef.current = values;
  const getTarget = useCallback(
    () => plotTarget(source.trim(), spec?.title ?? null, paramValues(spec, valuesRef.current)),
    [source, spec],
  );

  if ('error' in result) return <BlockError title="Plot not drawn" message={result.error} source={source} />;
  if (!spec) return null;
  return (
    <FocusFrame noun="plot" getTarget={getTarget} doubleClick={false} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface p-4">
        <InteractivePlot spec={spec} values={values} onValues={setValues} maxHeight={380} />
      </figure>
    </FocusFrame>
  );
}
