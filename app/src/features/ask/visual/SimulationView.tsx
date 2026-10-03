import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { Pause, Play, RotateCcw, Square } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { FocusFrame } from '../../focus/FocusFrame';
import { simulationTarget } from '../../focus/targets';
import type { FocusParamValue } from '../../focus/focusTypes';
import { BlockError } from './VisualBlocks';
import { FrameAxes, HALO_STYLE, ParamSliders, useElementWidth } from './VisualControls';
import { colorVar } from './palette';
import { type FrameLayout, type Point, arrowHead, formatNumber, frameLayout, initialValues } from './plotSpec';
import {
  type DrawItem,
  type SimModel,
  type SimState,
  advance,
  createSimState,
  finishMessage,
  parseSimulationSpec,
  simTime,
} from './simulationSpec';
import { claimPlayback, otherIsPlaying, releasePlayback } from './playback';

type Status = 'ready' | 'running' | 'paused' | 'finished' | 'stopped';

/** Longest wall-clock gap integrated in one frame (a background tab must not leap ahead). */
const MAX_FRAME_SECONDS = 0.1;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

function at(pair: [(s: ArrayLike<number>) => number, (s: ArrayLike<number>) => number], slots: Float64Array): Point {
  return [pair[0](slots), pair[1](slots)];
}

function ok(p: Point): boolean {
  return Number.isFinite(p[0]) && Number.isFinite(p[1]);
}

/** Zig-zag points of a spring between two pixel positions. */
function springPoints(a: Point, b: Point, coils: number): Point[] {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len = Math.hypot(dx, dy);
  if (!(len > 1)) return [a, b];
  const ux = dx / len;
  const uy = dy / len;
  const lead = len * 0.1;
  const amp = Math.min(9, Math.max(3, len * 0.12));
  const zig = coils * 2;
  const pts: Point[] = [a, [a[0] + ux * lead, a[1] + uy * lead]];
  for (let i = 1; i < zig; i++) {
    const s = lead + ((len - 2 * lead) * i) / zig;
    const side = i % 2 === 0 ? -amp : amp;
    pts.push([a[0] + ux * s - uy * side, a[1] + uy * s + ux * side]);
  }
  pts.push([b[0] - ux * lead, b[1] - uy * lead], b);
  return pts;
}

function points(list: Point[]): string {
  return list.map(p => `${p[0].toFixed(1)},${p[1].toFixed(1)}`).join(' ');
}

function DrawMark({ item, slots, layout, trail, unit }: { item: DrawItem; slots: Float64Array; layout: FrameLayout; trail: Point[] | undefined; unit: number }) {
  const color = colorVar(item.color);
  const px = (p: Point): Point => [layout.sx(p[0]), layout.sy(p[1])];
  switch (item.type) {
    case 'circle': {
      const c = at(item.at, slots);
      const r = item.r(slots);
      if (!ok(c) || !(r > 0) || !Number.isFinite(r)) return null;
      const [cx, cy] = px(c);
      return (
        <g>
          <circle cx={cx} cy={cy} r={Math.max(1.5, r * unit)} fill={item.fill ? color : 'none'} stroke={color} strokeWidth={1.5} />
          {item.label && <text x={cx + r * unit + 4} y={cy - 4} fontSize={12} fill={color} style={HALO_STYLE}>{item.label}</text>}
        </g>
      );
    }
    case 'rect': {
      const c = at(item.at, slots);
      const size = at(item.size, slots);
      const angle = item.angle ? item.angle(slots) : 0;
      if (!ok(c) || !ok(size) || !Number.isFinite(angle)) return null;
      const cos = Math.cos(angle);
      const sin = Math.sin(angle);
      const corners = ([[-1, -1], [1, -1], [1, 1], [-1, 1]] as const).map(([sx, sy]): Point => {
        const lx = (sx * size[0]) / 2;
        const ly = (sy * size[1]) / 2;
        return px([c[0] + lx * cos - ly * sin, c[1] + lx * sin + ly * cos]);
      });
      return (
        <g>
          <polygon points={points(corners)} fill={item.fill ? color : 'none'} fillOpacity={item.fill ? 0.85 : undefined} stroke={color} strokeWidth={1.5} />
          {item.label && <text x={px(c)[0]} y={px(c)[1] + 4} textAnchor="middle" fontSize={12} fill="var(--c-text)" style={HALO_STYLE}>{item.label}</text>}
        </g>
      );
    }
    case 'line':
    case 'vector':
    case 'spring': {
      const a = at(item.from, slots);
      const b = at(item.to, slots);
      if (!ok(a) || !ok(b)) return null;
      const pa = px(a);
      const pb = px(b);
      const mid: Point = [(pa[0] + pb[0]) / 2, (pa[1] + pb[1]) / 2];
      const label = item.label ? <text x={mid[0] + 6} y={mid[1] - 6} fontSize={12} fill={color} style={HALO_STYLE}>{item.label}</text> : null;
      if (item.type === 'spring') {
        return (
          <g>
            <polyline points={points(springPoints(pa, pb, item.coils))} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" />
            {label}
          </g>
        );
      }
      if (item.type === 'vector') {
        const head = arrowHead(pa, pb);
        return (
          <g>
            <line x1={pa[0]} y1={pa[1]} x2={pb[0]} y2={pb[1]} stroke={color} strokeWidth={2} strokeLinecap="round" />
            {head && <polygon points={points(head)} fill={color} />}
            {label}
          </g>
        );
      }
      return (
        <g>
          <line x1={pa[0]} y1={pa[1]} x2={pb[0]} y2={pb[1]} stroke={color} strokeWidth={item.width} strokeLinecap="round" />
          {label}
        </g>
      );
    }
    case 'trail':
      return trail && trail.length > 1 ? <polyline points={points(trail.map(px))} fill="none" stroke={color} strokeOpacity={0.55} strokeWidth={1.5} strokeLinejoin="round" /> : null;
    case 'label': {
      const p = at(item.at, slots);
      if (!ok(p)) return null;
      const [x, y] = px(p);
      return <text x={x} y={y} fontSize={12} fill={color} style={HALO_STYLE}>{item.text}</text>;
    }
  }
}

/**
 * A declarative simulation with Run / Pause / Reset / Stop. It plays only
 * while on screen, only one plays at a time, and it never starts by itself
 * when the reader prefers reduced motion.
 */
export function SimulationPlayer({
  model,
  initial,
  maxHeight,
  autoPlay,
  preempt = false,
  onValues,
}: {
  model: SimModel;
  initial?: readonly FocusParamValue[];
  maxHeight: number;
  autoPlay: boolean;
  /** Its first automatic start takes over from a running simulation (the enlarged copy of it). */
  preempt?: boolean;
  onValues?: (values: FocusParamValue[]) => void;
}) {
  const playerId = useId();
  const clipId = `sim-clip-${playerId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const hostRef = useRef<HTMLDivElement>(null);
  const width = useElementWidth(hostRef);
  const [values, setValues] = useState<number[]>(() => initialValues(model.params, initial));
  const simRef = useRef<SimState | null>(null);
  if (simRef.current === null) simRef.current = createSimState(model, values);
  const trailsRef = useRef(new Map<number, Point[]>());
  const [, setFrame] = useState(0);
  const [status, setStatus] = useState<Status>(() => (simRef.current?.finished ? 'finished' : 'ready'));
  const statusRef = useRef(status);
  statusRef.current = status;
  const autoPausedRef = useRef(false);
  const startedRef = useRef(false);
  const visibleRef = useRef(false);

  const recordTrails = useCallback(() => {
    const sim = simRef.current;
    if (!sim) return;
    model.draw.forEach((item, i) => {
      if (item.type !== 'trail') return;
      const p = at(item.at, sim.slots);
      if (!ok(p)) return;
      const list = trailsRef.current.get(i) ?? [];
      list.push(p);
      if (list.length > item.max) list.splice(0, list.length - item.max);
      trailsRef.current.set(i, list);
    });
  }, [model]);

  const restart = useCallback((next: readonly number[]) => {
    simRef.current = createSimState(model, next);
    trailsRef.current = new Map();
    recordTrails();
    setFrame(f => f + 1);
    return simRef.current;
  }, [model, recordTrails]);

  // A new model (edited or still streaming) starts over from its own values.
  const modelRef = useRef(model);
  useEffect(() => {
    if (modelRef.current === model) return;
    modelRef.current = model;
    const next = initialValues(model.params, initial);
    setValues(next);
    restart(next);
    setStatus(s => (s === 'running' ? 'running' : 'ready'));
  }, [model, initial, restart]);

  useEffect(() => {
    onValues?.(model.params.map((p, i) => ({ name: p.name, value: values[i] ?? p.value })));
  }, [model, values, onValues]);

  // Playback ownership: only one simulation runs at a time.
  useEffect(() => {
    if (status !== 'running') {
      releasePlayback(playerId);
      return;
    }
    claimPlayback(playerId, () => setStatus(s => (s === 'running' ? 'paused' : s)));
  }, [status, playerId]);
  useEffect(() => () => releasePlayback(playerId), [playerId]);

  // The animation loop. Work per frame is bounded by `advance`.
  useEffect(() => {
    if (status !== 'running') return;
    let raf = 0;
    let last = performance.now();
    const frame = (now: number) => {
      const seconds = Math.min(MAX_FRAME_SECONDS, Math.max(0, (now - last) / 1000));
      last = now;
      const sim = simRef.current;
      if (!sim) return;
      advance(model, sim, seconds);
      recordTrails();
      setFrame(f => f + 1);
      if (sim.finished) {
        setStatus('finished');
        return;
      }
      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [status, model, recordTrails]);

  const run = useCallback(() => {
    autoPausedRef.current = false;
    startedRef.current = true;
    if (statusRef.current === 'finished' || statusRef.current === 'stopped' || simRef.current?.finished) restart(values);
    setStatus('running');
  }, [restart, values]);

  // Plays only while on screen; may start by itself once, unless motion is reduced.
  useEffect(() => {
    const el = hostRef.current;
    if (!el || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(entries => {
      const visible = entries.some(e => e.isIntersecting && e.intersectionRatio >= 0.4);
      visibleRef.current = visible;
      if (!visible) {
        if (statusRef.current === 'running') {
          autoPausedRef.current = true;
          setStatus('paused');
        }
        return;
      }
      if (prefersReducedMotion()) return;
      const takeOver = preempt && autoPlay && !startedRef.current;
      if (otherIsPlaying(playerId) && !takeOver) return;
      if (autoPausedRef.current && statusRef.current === 'paused') {
        autoPausedRef.current = false;
        setStatus('running');
      } else if (autoPlay && !startedRef.current && statusRef.current === 'ready') {
        startedRef.current = true;
        setStatus('running');
      }
    }, { threshold: [0, 0.4] });
    observer.observe(el);
    return () => observer.disconnect();
  }, [autoPlay, preempt, playerId]);

  // Reduced motion switched on mid-run: pause.
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;
    const query = window.matchMedia('(prefers-reduced-motion: reduce)');
    const onChange = () => {
      if (query.matches && statusRef.current === 'running') setStatus('paused');
    };
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);

  const pause = () => {
    autoPausedRef.current = false;
    setStatus('paused');
  };
  const reset = () => {
    restart(values);
    setStatus(s => (s === 'running' ? 'running' : 'ready'));
  };
  const stop = () => {
    autoPausedRef.current = false;
    restart(values);
    setStatus('stopped');
  };
  const setParam = (index: number, value: number) => {
    const next = values.slice();
    next[index] = value;
    setValues(next);
    restart(next);
    setStatus(s => (s === 'running' ? 'running' : s === 'paused' ? 'paused' : 'ready'));
  };

  // First frame's trail point.
  useEffect(() => {
    if (trailsRef.current.size === 0) recordTrails();
  }, [recordTrails]);

  const layout = useMemo(
    () => (width > 0 ? frameLayout(width, model.view.x, model.view.y, { equal: model.view.equal, maxHeight }) : null),
    [width, model, maxHeight],
  );
  // Created during the first render (above) and only ever replaced, never cleared.
  const sim = simRef.current as SimState;
  const t = simTime(model, sim);
  const unit = layout ? layout.innerWidth / (model.view.x[1] - model.view.x[0]) : 1;
  const statusText =
    status === 'running' ? 'Running' :
    status === 'paused' ? 'Paused' :
    status === 'stopped' ? 'Stopped' :
    status === 'finished' && sim.finished ? finishMessage(sim.finished) :
    prefersReducedMotion() ? 'Ready. Press Run to start (motion is reduced, so it does not start by itself).' : 'Ready';

  const button = cn(
    'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] font-medium text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-micro',
    FOCUS_RING,
  );

  return (
    <div className="flex flex-col gap-3">
      <div className="min-h-[28px] flex items-baseline pr-28">
        {model.title && <h4 className="m-0 text-[14px] font-semibold text-shodh-text">{model.title}</h4>}
      </div>
      <div ref={hostRef} className="w-full flex justify-center">
        {layout && (
          <svg
            role="img"
            aria-label={`${model.title || 'Simulation'}: animation of the model below. ${statusText}.`}
            width={layout.width}
            height={layout.height}
            viewBox={`0 0 ${layout.width} ${layout.height}`}
            className={cn('max-w-full h-auto select-none', status === 'stopped' && 'opacity-60')}
          >
            <defs>
              <clipPath id={clipId}>
                <rect x={layout.left} y={layout.top} width={layout.innerWidth} height={layout.innerHeight} />
              </clipPath>
            </defs>
            <FrameAxes layout={layout} x={model.view.x} y={model.view.y} xLabel={model.view.xLabel} yLabel={model.view.yLabel} grid={model.view.grid} />
            <g clipPath={`url(#${clipId})`}>
              {model.draw.map((item, i) => (
                <DrawMark key={i} item={item} slots={sim.slots} layout={layout} trail={trailsRef.current.get(i)} unit={unit} />
              ))}
            </g>
          </svg>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Simulation controls">
        {status === 'running' ? (
          <button type="button" className={button} onClick={pause}>
            <Pause className="w-3.5 h-3.5" aria-hidden="true" />
            Pause
          </button>
        ) : (
          <button type="button" className={cn(button, 'text-shodh-text')} onClick={run}>
            <Play className="w-3.5 h-3.5" aria-hidden="true" />
            {status === 'paused' ? 'Resume' : 'Run'}
          </button>
        )}
        <button type="button" className={button} onClick={reset} disabled={t === 0 && status !== 'finished'}>
          <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
          Reset
        </button>
        <button type="button" className={button} onClick={stop} disabled={status === 'stopped' || (status === 'ready' && t === 0)}>
          <Square className="w-3.5 h-3.5" aria-hidden="true" />
          Stop
        </button>
        <span className="ml-auto font-mono text-[12px] tabular-nums text-shodh-text-muted" aria-hidden="true">
          {`t = ${formatNumber(t, 3)} s`}
        </span>
      </div>
      <p className={cn('m-0 text-[12px]', sim.finished === 'diverged' ? 'text-shodh-warning' : 'text-shodh-text-muted')} role="status" aria-live="polite">
        {statusText}
      </p>
      {model.readouts.length > 0 && (
        <dl className="m-0 grid grid-cols-2 sm:grid-cols-4 gap-x-4 gap-y-1.5 text-[12.5px]">
          {model.readouts.map((r, i) => (
            <div key={i} className="flex flex-col min-w-0">
              <dt className="text-shodh-text-muted truncate">{r.label}</dt>
              <dd className="m-0 font-mono tabular-nums text-shodh-text">{`${formatNumber(r.expr(sim.slots), r.digits)}${r.unit ? ` ${r.unit}` : ''}`}</dd>
            </div>
          ))}
        </dl>
      )}
      <ParamSliders params={model.params} values={values} onChange={setParam} />
    </div>
  );
}

/** A ```simulation block in an answer. */
export function SimulationBlock({ source }: { source: string }) {
  const result = useMemo(() => parseSimulationSpec(source), [source]);
  const model = result.ok ? result.model : null;
  const valuesRef = useRef<FocusParamValue[]>([]);
  const onValues = useCallback((v: FocusParamValue[]) => {
    valuesRef.current = v;
  }, []);
  const getTarget = useCallback(() => simulationTarget(source.trim(), model?.title ?? null, valuesRef.current), [source, model]);

  if ('error' in result) return <BlockError title="Simulation not run" message={result.error} source={source} />;
  if (!model) return null;
  return (
    <FocusFrame noun="simulation" getTarget={getTarget} doubleClick={false} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface p-4">
        <SimulationPlayer model={model} maxHeight={380} autoPlay onValues={onValues} />
      </figure>
    </FocusFrame>
  );
}
