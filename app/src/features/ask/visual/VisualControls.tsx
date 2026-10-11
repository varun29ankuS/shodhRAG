import { useId, useLayoutEffect, useState } from 'react';
import type React from 'react';
import { cn } from '../../../lib/utils';
import { type FrameLayout, type PlotParam, formatNumber, niceTicks, snapParam } from './plotSpec';

/** Width of an element, tracked as it resizes. */
export function useElementWidth(ref: React.RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setWidth(prev => (prev === el.clientWidth ? prev : el.clientWidth));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}

/** Labelled range inputs for the parameters of a plot or simulation. */
export function ParamSliders({
  params,
  values,
  onChange,
  className,
}: {
  params: readonly PlotParam[];
  values: readonly number[];
  onChange: (index: number, value: number) => void;
  className?: string;
}) {
  const baseId = useId();
  if (params.length === 0) return null;
  return (
    <div className={cn('grid gap-x-6 gap-y-2.5 sm:grid-cols-2', className)} role="group" aria-label="Parameters">
      {params.map((p, i) => {
        const id = `${baseId}-p${i}`;
        const value = values[i] ?? p.value;
        const shown = formatNumber(value, 4);
        return (
          <div key={p.name} className="flex flex-col gap-1 min-w-0">
            <div className="flex items-baseline justify-between gap-3 text-[12.5px]">
              <label htmlFor={id} className="text-shodh-text-secondary truncate">
                {p.label}
              </label>
              <output htmlFor={id} className="font-mono tabular-nums text-shodh-text">
                {shown}
              </output>
            </div>
            <input
              id={id}
              type="range"
              min={p.min}
              max={p.max}
              step={p.step}
              value={value}
              aria-valuetext={`${p.label}: ${shown}`}
              onChange={e => onChange(i, snapParam(p, Number(e.currentTarget.value)))}
              className="w-full h-5 cursor-pointer accent-[var(--c-accent)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface rounded"
            />
          </div>
        );
      })}
    </div>
  );
}

/** Text with a halo in the surface colour, legible over grid lines and curves. */
export const HALO_STYLE: React.CSSProperties = {
  paintOrder: 'stroke',
  stroke: 'var(--c-surface)',
  strokeWidth: 3,
  strokeLinejoin: 'round',
};

/** Grid, zero axes, tick labels and axis titles of a plotting frame. */
export function FrameAxes({
  layout,
  x,
  y,
  xLabel,
  yLabel,
  grid = true,
}: {
  layout: FrameLayout;
  x: readonly [number, number];
  y: readonly [number, number];
  xLabel?: string;
  yLabel?: string;
  grid?: boolean;
}) {
  const xTicks = niceTicks(x[0], x[1], Math.max(2, Math.round(layout.innerWidth / 80)));
  const yTicks = niceTicks(y[0], y[1], Math.max(2, Math.round(layout.innerHeight / 56)));
  const { left, top, innerWidth: w, innerHeight: h } = layout;
  const bottom = top + h;
  const zeroX = x[0] <= 0 && x[1] >= 0 ? layout.sx(0) : null;
  const zeroY = y[0] <= 0 && y[1] >= 0 ? layout.sy(0) : null;
  return (
    <g aria-hidden="true" fontSize={11} fill="var(--c-text-muted)">
      <rect x={left} y={top} width={w} height={h} fill="none" stroke="var(--c-border)" />
      {grid && xTicks.map(t => <line key={`gx${t}`} x1={layout.sx(t)} x2={layout.sx(t)} y1={top} y2={bottom} stroke="var(--c-border-subtle)" />)}
      {grid && yTicks.map(t => <line key={`gy${t}`} y1={layout.sy(t)} y2={layout.sy(t)} x1={left} x2={left + w} stroke="var(--c-border-subtle)" />)}
      {zeroX !== null && <line x1={zeroX} x2={zeroX} y1={top} y2={bottom} stroke="var(--c-border-strong)" />}
      {zeroY !== null && <line y1={zeroY} y2={zeroY} x1={left} x2={left + w} stroke="var(--c-border-strong)" />}
      {xTicks.map(t => (
        <text key={`tx${t}`} x={layout.sx(t)} y={bottom + 14} textAnchor="middle" className="tabular-nums">
          {formatNumber(t)}
        </text>
      ))}
      {yTicks.map(t => (
        <text key={`ty${t}`} x={left - 6} y={layout.sy(t) + 4} textAnchor="end" className="tabular-nums">
          {formatNumber(t)}
        </text>
      ))}
      {xLabel && (
        <text x={left + w / 2} y={bottom + 31} textAnchor="middle" fill="var(--c-text-secondary)">
          {xLabel}
        </text>
      )}
      {yLabel && (
        <text x={left + 4} y={top + 12} textAnchor="start" fill="var(--c-text-secondary)" style={HALO_STYLE}>
          {yLabel}
        </text>
      )}
    </g>
  );
}
