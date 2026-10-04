import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import { AlertTriangle, CheckSquare, Loader2, Minus, Plus, Square } from 'lucide-react';
import { cn } from '../../lib/utils';
import { ChartArtifact } from '../../components/ChartArtifact';
import type { Artifact } from '../../components/EnhancedArtifactPanel';
import { parseChartBlock } from '../ask/visual/chartSpec';
import { tryParseChartSpec } from '../../utils/artifactExtractor';
import { renderDiagram } from '../ask/visual/VisualBlocks';
import { SketchSurface, useSvgSketch } from '../ask/visual/SvgSketch';
import { InteractivePlot } from '../ask/visual/PlotView';
import { SimulationPlayer } from '../ask/visual/SimulationView';
import { initialValues, parsePlotSpec } from '../ask/visual/plotSpec';
import { parseSimulationSpec } from '../ask/visual/simulationSpec';
import { useSourceDocument } from '../ask/useSourceDocument';
import type { SearchHit } from '../ask/types';
import { momentToDate, parseMoment } from '../tasks/dueDate';
import { PRIORITY_LABELS, STATUS_LABELS } from '../tasks/types';
import { SnippetStage } from '../research/SnippetStage';
import type { FocusCommand } from './focusKeys';
import { panDelta } from './focusKeys';
import type { FocusParamValue, FocusTarget, FocusTaskSnapshot } from './focusTypes';
import {
  MAX_ZOOM,
  MIN_ZOOM,
  PAN_STEP,
  ZOOM_STEP,
  canPan,
  centeredView,
  clampView,
  fitView,
  panBy,
  scaledFontSize,
  wheelZoomFactor,
  zoomAt,
  zoomBy,
} from './zoomMath';
import type { Size, View } from './zoomMath';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/** Lets the pop-out route keyboard commands to the stage. True when handled. */
export type StageCommandRef = React.MutableRefObject<((command: FocusCommand, large: boolean) => boolean) | null>;

/** Natural size of a chart in the focus view (before zoom). */
const CHART_SIZE: Size = { width: 760, height: 360 };
const CHART_TITLE_HEIGHT = 40;
const EQUATION_FONT = 24;
const TABLE_FONT = 14;
/** Smallest scale "Fit" uses for text (equations, tables). */
const MIN_TEXT_FIT = 0.75;

function useElementSize(ref: React.RefObject<HTMLElement>): Size {
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setSize(prev => {
      const next = { width: el.clientWidth, height: el.clientHeight };
      return prev.width === next.width && prev.height === next.height ? prev : next;
    });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return size;
}

function ZoomToolbar({
  scale,
  fitted,
  onZoomIn,
  onZoomOut,
  onFit,
  onActual,
}: {
  scale: number;
  fitted: boolean;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onFit: () => void;
  onActual: () => void;
}) {
  const button = cn(
    'h-8 min-w-8 px-1.5 inline-flex items-center justify-center rounded-lg text-[12px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-micro',
    FOCUS_RING,
  );
  const percent = Math.round(scale * 100);
  return (
    <div
      role="toolbar"
      aria-label="Zoom"
      className="absolute right-3 bottom-3 z-10 flex items-center gap-0.5 rounded-xl border border-shodh-border bg-shodh-surface/95 p-1 shadow-[0_6px_24px_rgba(0,0,0,0.25)]"
      onPointerDown={e => e.stopPropagation()}
    >
      <button type="button" className={button} onClick={onZoomOut} disabled={scale <= MIN_ZOOM + 1e-6} aria-label="Zoom out" title="Zoom out (−)">
        <Minus className="w-4 h-4" aria-hidden="true" />
      </button>
      <span className="w-12 text-center text-[12px] tabular-nums text-shodh-text-secondary" aria-hidden="true">
        {percent}%
      </span>
      <span className="sr-only" role="status" aria-live="polite">{`Zoom ${percent} percent`}</span>
      <button type="button" className={button} onClick={onZoomIn} disabled={scale >= MAX_ZOOM - 1e-6} aria-label="Zoom in" title="Zoom in (+)">
        <Plus className="w-4 h-4" aria-hidden="true" />
      </button>
      <button type="button" className={cn(button, fitted && 'bg-shodh-raised text-shodh-text')} onClick={onFit} aria-pressed={fitted} title="Fit (0)">
        Fit
      </button>
      <button type="button" className={button} onClick={onActual} title="Actual size (1)">
        100%
      </button>
    </div>
  );
}

/**
 * Zoom and pan of drawn content (diagrams, charts, images). Content is laid
 * out at natural size × scale, so SVG redraws crisply at every zoom level.
 * Ctrl/⌘ + wheel or pinch zooms around the cursor, plain wheel and drag
 * pan, keys come through `commandRef`.
 */
function CanvasStage({
  natural,
  label,
  commandRef,
  children,
}: {
  natural: Size | null;
  label: string;
  commandRef: StageCommandRef;
  children: (scale: number) => React.ReactNode;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const viewport = useElementSize(viewportRef);
  const [view, setView] = useState<View>({ scale: 1, x: 0, y: 0 });
  const [fitted, setFitted] = useState(true);
  const viewRef = useRef(view);
  viewRef.current = view;
  const ready = natural !== null && viewport.width > 0 && viewport.height > 0;

  // Fit on open and while fitted; otherwise keep the view reachable on resize.
  useLayoutEffect(() => {
    if (!ready || !natural) return;
    setView(prev => (fitted ? fitView(natural, viewport) : clampView(prev, natural, viewport)));
  }, [ready, natural, viewport, fitted]);

  const apply = useCallback((next: (v: View) => View, keepFit = false) => {
    if (!natural) return;
    if (!keepFit) setFitted(false);
    setView(v => next(v));
  }, [natural]);

  // Native wheel listener: React's onWheel is passive and cannot stop the
  // window from zooming or the page from scrolling.
  useEffect(() => {
    const el = viewportRef.current;
    if (!el || !natural) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      if (e.ctrlKey || e.metaKey) {
        const factor = wheelZoomFactor(e.deltaY, e.deltaMode);
        const point = { x: e.clientX - rect.left, y: e.clientY - rect.top };
        const size = { width: rect.width, height: rect.height };
        setFitted(false);
        setView(v => zoomAt(v, v.scale * factor, point, natural, size));
      } else {
        const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? rect.height : 1;
        const dx = (e.shiftKey && e.deltaX === 0 ? e.deltaY : e.deltaX) * unit;
        const dy = (e.shiftKey && e.deltaX === 0 ? 0 : e.deltaY) * unit;
        setView(v => panBy(v, -dx, -dy, natural, { width: rect.width, height: rect.height }));
      }
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [natural]);

  // Drag to pan.
  const drag = useRef<{ id: number; x: number; y: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !natural) return;
    drag.current = { id: e.pointerId, x: e.clientX, y: e.clientY };
    e.currentTarget.setPointerCapture(e.pointerId);
    setDragging(true);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId || !natural) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    d.x = e.clientX;
    d.y = e.clientY;
    setView(v => panBy(v, dx, dy, natural, viewport));
  };
  const endDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    if (drag.current?.id !== e.pointerId) return;
    drag.current = null;
    setDragging(false);
  };

  const zoomIn = () => natural && apply(v => zoomBy(v, ZOOM_STEP, natural, viewport));
  const zoomOut = () => natural && apply(v => zoomBy(v, 1 / ZOOM_STEP, natural, viewport));
  const fit = () => setFitted(true);
  const actual = () => natural && apply(() => centeredView(natural, viewport, 1));

  commandRef.current = (command, large) => {
    if (!natural) return false;
    switch (command) {
      case 'zoomIn':
        zoomIn();
        return true;
      case 'zoomOut':
        zoomOut();
        return true;
      case 'fit':
        fit();
        return true;
      case 'actualSize':
        actual();
        return true;
      default: {
        const delta = panDelta(command, large ? PAN_STEP * 5 : PAN_STEP);
        if (!delta || !canPan(viewRef.current, natural, viewport)) return false;
        setView(v => panBy(v, delta.dx, delta.dy, natural, viewport));
        return true;
      }
    }
  };

  const pannable = natural ? canPan(view, natural, viewport) : false;

  return (
    <div
      ref={viewportRef}
      tabIndex={0}
      role="group"
      aria-roledescription="zoomable view"
      aria-label={`${label}. Zoom with plus and minus, 0 to fit${pannable ? ', arrow keys or drag to pan' : ''}.`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      className={cn(
        'relative flex-1 min-h-0 min-w-0 overflow-hidden bg-shodh-raised-2 touch-none select-none',
        pannable ? (dragging ? 'cursor-grabbing' : 'cursor-grab') : 'cursor-default',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
      )}
    >
      {ready && natural ? (
        <div
          className="absolute"
          style={{ left: view.x, top: view.y, width: natural.width * view.scale, height: natural.height * view.scale }}
        >
          {children(view.scale)}
        </div>
      ) : (
        <div className="h-full flex items-center justify-center gap-2 text-[13px] text-shodh-text-muted" role="status">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Preparing view…
        </div>
      )}
      {ready && <ZoomToolbar scale={view.scale} fitted={fitted} onZoomIn={zoomIn} onZoomOut={zoomOut} onFit={fit} onActual={actual} />}
    </div>
  );
}

/**
 * Zoom of text-like content (equations, tables): re-laid out at a larger
 * font size, so KaTeX glyphs and table text stay sharp; overflow scrolls.
 */
function TextStage({
  baseFont,
  label,
  commandRef,
  children,
}: {
  baseFont: number;
  label: string;
  commandRef: StageCommandRef;
  children: React.ReactNode;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const viewport = useElementSize(viewportRef);
  const [scale, setScale] = useState(1);
  const [fitted, setFitted] = useState(true);
  const [natural, setNatural] = useState<Size | null>(null);

  // Natural size at the base font, measured once the content is laid out.
  useLayoutEffect(() => {
    const el = contentRef.current;
    if (!el || natural) return;
    const w = el.scrollWidth / scale;
    const h = el.scrollHeight / scale;
    if (w > 0 && h > 0) setNatural({ width: w, height: h });
  }, [natural, scale]);

  useLayoutEffect(() => {
    if (!fitted || !natural || viewport.width === 0) return;
    // Text below this size is not worth fitting: scroll instead.
    setScale(Math.max(MIN_TEXT_FIT, fitView(natural, viewport).scale));
  }, [fitted, natural, viewport]);

  const setManual = useCallback((next: (s: number) => number) => {
    setFitted(false);
    setScale(s => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, next(s))));
  }, []);

  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const factor = wheelZoomFactor(e.deltaY, e.deltaMode);
      setManual(s => s * factor);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [setManual]);

  commandRef.current = command => {
    switch (command) {
      case 'zoomIn':
        setManual(s => s * ZOOM_STEP);
        return true;
      case 'zoomOut':
        setManual(s => s / ZOOM_STEP);
        return true;
      case 'fit':
        setFitted(true);
        return true;
      case 'actualSize':
        setManual(() => 1);
        return true;
      default:
        // Arrow keys scroll the focused scroller natively.
        return false;
    }
  };

  return (
    <div
      ref={viewportRef}
      tabIndex={0}
      role="group"
      aria-roledescription="zoomable view"
      aria-label={`${label}. Zoom with plus and minus, 0 to fit, arrow keys to scroll.`}
      className="relative flex-1 min-h-0 min-w-0 overflow-auto scrollbar-thin bg-shodh-raised-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    >
      <div className="min-h-full min-w-full w-max flex items-center justify-center p-6">
        <div ref={contentRef} style={{ fontSize: scaledFontSize(baseFont, scale) }} className="text-shodh-text">
          {children}
        </div>
      </div>
      <ZoomToolbar
        scale={scale}
        fitted={fitted}
        onZoomIn={() => setManual(s => s * ZOOM_STEP)}
        onZoomOut={() => setManual(s => s / ZOOM_STEP)}
        onFit={() => setFitted(true)}
        onActual={() => setManual(() => 1)}
      />
    </div>
  );
}

function StageError({ message, detail }: { message: string; detail?: string }) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-3 p-6 text-center">
      <p role="alert" className="flex items-center gap-2 text-[13px] text-shodh-text-secondary">
        <AlertTriangle className="w-4 h-4 text-shodh-warning" aria-hidden="true" />
        {message}
      </p>
      {detail && (
        <pre className="max-w-full max-h-60 overflow-auto scrollbar-thin rounded-lg border border-shodh-border bg-shodh-surface-2 p-3 text-left text-[12px] font-mono text-shodh-text-muted whitespace-pre-wrap">
          {detail}
        </pre>
      )}
    </div>
  );
}

/** Natural size of an SVG from its viewBox (or width/height attributes). */
function svgSize(svg: string): Size | null {
  const box = /viewBox="\s*[-\d.]+[\s,]+[-\d.]+[\s,]+([\d.]+)[\s,]+([\d.]+)\s*"/.exec(svg);
  if (box) return { width: Number(box[1]), height: Number(box[2]) };
  const w = /\swidth="([\d.]+)(?:px)?"/.exec(svg);
  const h = /\sheight="([\d.]+)(?:px)?"/.exec(svg);
  return w && h ? { width: Number(w[1]), height: Number(h[1]) } : null;
}

function MermaidStage({ source, label, dark, commandRef }: { source: string; label: string; dark: boolean; commandRef: StageCommandRef }) {
  const reactId = useId();
  // A fresh id: the answer's own copy of this diagram is still in the document.
  const domId = `focus-mmd-${reactId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const [state, setState] = useState<{ status: 'loading' } | { status: 'ready'; svg: string; size: Size } | { status: 'error'; message: string }>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    renderDiagram(domId, source.trim(), dark)
      .then(svg => {
        if (cancelled) return;
        const size = svgSize(svg);
        if (!size || size.width <= 0 || size.height <= 0) setState({ status: 'error', message: 'The diagram has no drawable size.' });
        else setState({ status: 'ready', svg, size });
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: error instanceof Error ? error.message.split('\n')[0] : 'The diagram could not be drawn.' });
      });
    return () => {
      cancelled = true;
    };
  }, [domId, source, dark]);

  if (state.status === 'error') return <StageError message={`Diagram not drawn: ${state.message}`} detail={source} />;
  return (
    <CanvasStage natural={state.status === 'ready' ? state.size : null} label={label} commandRef={commandRef}>
      {() => state.status === 'ready' && (
        // SVG produced by mermaid with securityLevel 'strict' (sanitised, no scripts).
        <div
          role="img"
          aria-label={label}
          className="w-full h-full rounded-lg bg-shodh-surface [&>svg]:!w-full [&>svg]:!h-full [&>svg]:!max-w-none"
          dangerouslySetInnerHTML={{ __html: state.svg }}
        />
      )}
    </CanvasStage>
  );
}

function ChartStage({ source, label, theme, commandRef }: { source: string; label: string; theme: string; commandRef: StageCommandRef }) {
  // Fenced ```chart blocks use parseChartBlock; charts extracted from an
  // answer as artifacts use the artifact parser. Accept either.
  const parsed = useMemo(() => parseChartBlock(source), [source]);
  const legacy = useMemo(() => (parsed.ok ? null : tryParseChartSpec(source)), [parsed, source]);
  const title = parsed.ok ? parsed.chart.title ?? null : legacy?.title ?? null;
  const artifact = useMemo<Artifact | null>(() => {
    const content = parsed.ok ? JSON.stringify(parsed.chart) : legacy ? source : null;
    if (content === null) return null;
    return {
      id: 'focus-chart',
      artifact_type: 'chart',
      title: title ?? 'Chart',
      content,
      editable: false,
      version: 1,
      created_at: '',
    } as Artifact;
  }, [parsed, legacy, source, title]);
  if (!artifact) return <StageError message={`Chart not drawn: ${'error' in parsed ? parsed.error : 'unreadable chart data'}`} detail={source} />;
  const titled = Boolean(title);
  return (
    <CanvasStage natural={CHART_SIZE} label={label} commandRef={commandRef}>
      {scale => (
        <div className="w-full h-full rounded-lg bg-shodh-surface overflow-hidden">
          <ChartArtifact
            artifact={artifact}
            theme={theme}
            height={Math.max(120, CHART_SIZE.height * scale - (titled ? CHART_TITLE_HEIGHT : 0))}
          />
        </div>
      )}
    </CanvasStage>
  );
}

function SvgStage({ source, label, commandRef }: { source: string; label: string; commandRef: StageCommandRef }) {
  const { result, markup, seed } = useSvgSketch(source);
  if ('error' in result) return <StageError message={`Sketch not drawn: ${result.error}`} detail={source} />;
  return (
    <CanvasStage natural={{ width: result.width, height: result.height }} label={label} commandRef={commandRef}>
      {() => (
        <SketchSurface
          markup={markup}
          sketch={result.sketch}
          seed={seed}
          label={label}
          className="w-full h-full rounded-lg bg-shodh-surface [&>svg]:!w-full [&>svg]:!h-full [&>svg]:!max-w-none"
        />
      )}
    </CanvasStage>
  );
}

/** Interactive visuals are not zoomed: they are laid out large, sliders included. */
function InteractiveStage({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex-1 min-h-0 min-w-0 overflow-y-auto scrollbar-thin bg-shodh-raised-2 p-6 flex justify-center">
      <div className="w-full max-w-[960px] h-fit rounded-2xl border border-shodh-border bg-shodh-surface p-5">{children}</div>
    </div>
  );
}

function PlotStage({ source, values }: { source: string; values: FocusParamValue[] }) {
  const result = useMemo(() => parsePlotSpec(source), [source]);
  const spec = result.ok ? result.spec : null;
  const [current, setCurrent] = useState<number[]>(() => (spec ? initialValues(spec.params, values) : []));
  if ('error' in result) return <StageError message={`Plot not drawn: ${result.error}`} detail={source} />;
  if (!spec) return null;
  return (
    <InteractiveStage>
      <InteractivePlot spec={spec} values={current} onValues={setCurrent} maxHeight={560} />
    </InteractiveStage>
  );
}

function SimulationStage({ source, values }: { source: string; values: FocusParamValue[] }) {
  const result = useMemo(() => parseSimulationSpec(source), [source]);
  if ('error' in result) return <StageError message={`Simulation not run: ${result.error}`} detail={source} />;
  if (!result.ok) return null;
  return (
    <InteractiveStage>
      <SimulationPlayer model={result.model} initial={values} maxHeight={560} autoPlay preempt />
    </InteractiveStage>
  );
}

function ImageStage({ src, alt, label, commandRef }: { src: string | null; alt: string; label: string; commandRef: StageCommandRef }) {
  const [size, setSize] = useState<Size | null>(null);
  const [broken, setBroken] = useState(false);
  if (!src) return <StageError message="This image was not kept with the discussion, so it cannot be shown again." />;
  if (broken) return <StageError message="This image could not be displayed." />;
  return (
    <>
      {!size && (
        <img
          src={src}
          alt=""
          aria-hidden="true"
          className="hidden"
          onLoad={e => setSize({ width: e.currentTarget.naturalWidth || 1, height: e.currentTarget.naturalHeight || 1 })}
          onError={() => setBroken(true)}
        />
      )}
      <CanvasStage natural={size} label={label} commandRef={commandRef}>
        {() => <img src={src} alt={alt || label} draggable={false} className="block w-full h-full" />}
      </CanvasStage>
    </>
  );
}

function SourceStage({ hit, onPageChange }: { hit: SearchHit; onPageChange?: (page: number) => void }) {
  const doc = useSourceDocument(hit, { onPageChange });
  return <div className="flex-1 min-h-0 min-w-0 flex flex-col bg-shodh-surface">{doc.body}</div>;
}

function formatDue(value: string | null): string {
  const moment = parseMoment(value);
  if (!moment) return value ? value : 'No due date';
  const date = momentToDate(moment);
  return moment.kind === 'date'
    ? date.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' })
    : date.toLocaleString(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric', hour: 'numeric', minute: '2-digit' });
}

function TaskStage({ task }: { task: FocusTaskSnapshot }) {
  const rows: [string, string][] = [
    ['Status', STATUS_LABELS[task.status] ?? task.status],
    ['Priority', PRIORITY_LABELS[task.priority] ?? task.priority],
    ['Due', formatDue(task.dueDate)],
  ];
  if (task.project) rows.push(['Project', task.project]);
  return (
    <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin bg-shodh-raised-2 p-6 flex justify-center">
      <article className="w-full max-w-[560px] h-fit rounded-2xl border border-shodh-border bg-shodh-surface p-5 flex flex-col gap-4">
        <h3 className="text-[18px] font-semibold text-shodh-text break-words">{task.title}</h3>
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-[13.5px]">
          {rows.map(([k, v]) => (
            <React.Fragment key={k}>
              <dt className="text-shodh-text-muted">{k}</dt>
              <dd className="text-shodh-text">{v}</dd>
            </React.Fragment>
          ))}
        </dl>
        {task.tags.length > 0 && (
          <ul className="flex flex-wrap gap-1.5" aria-label="Tags">
            {task.tags.map(tag => (
              <li key={tag} className="px-2 h-6 inline-flex items-center rounded-full bg-shodh-raised-2 text-[12px] text-shodh-text-secondary">{tag}</li>
            ))}
          </ul>
        )}
        <section className="flex flex-col gap-1.5">
          <h4 className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">Notes</h4>
          <p className="text-[14px] leading-relaxed text-shodh-text-secondary whitespace-pre-wrap break-words">{task.notes.trim() || 'No notes.'}</p>
        </section>
        {task.subtasks.length > 0 && (
          <section className="flex flex-col gap-1.5">
            <h4 className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">Subtasks</h4>
            <ul className="flex flex-col gap-1">
              {task.subtasks.map((s, i) => (
                <li key={`${i}-${s.title}`} className="flex items-center gap-2 text-[13.5px] text-shodh-text-secondary">
                  {s.completed ? <CheckSquare className="w-4 h-4 text-shodh-success" aria-label="Done" /> : <Square className="w-4 h-4 text-shodh-text-faint" aria-label="Not done" />}
                  <span className={cn(s.completed && 'line-through text-shodh-text-muted')}>{s.title}</span>
                </li>
              ))}
            </ul>
          </section>
        )}
        <p className="text-[12px] text-shodh-text-muted">A snapshot from when this view opened. Edit the task in Tasks.</p>
      </article>
    </div>
  );
}

/** The selected text, shown within the passage it came from. */
function SelectionStage({ target }: { target: Extract<FocusTarget, { kind: 'selection' }> }) {
  const text = target.text.trim();
  const paragraph = target.paragraph.trim();
  const at = paragraph && text ? paragraph.indexOf(text) : -1;
  const where = target.document
    ? `${target.document.fileName || target.document.sourceFile}${target.document.page !== null ? `, page ${target.document.page}` : ''}`
    : 'From an answer';
  return (
    <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin bg-shodh-raised-2 p-6 flex justify-center">
      <article className="w-full max-w-[640px] h-fit rounded-2xl border border-shodh-border bg-shodh-surface p-5 flex flex-col gap-3">
        <p className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">{where}</p>
        {at >= 0 ? (
          <p className="text-[15px] leading-relaxed text-shodh-text-secondary whitespace-pre-wrap break-words">
            {paragraph.slice(0, at)}
            <mark className="rounded-[3px] bg-shodh-accent-soft px-0.5 text-shodh-text">{text}</mark>
            {paragraph.slice(at + text.length)}
          </p>
        ) : (
          <>
            <blockquote className="pl-4 border-l-2 border-shodh-accent text-[15px] leading-relaxed text-shodh-text whitespace-pre-wrap break-words">{text}</blockquote>
            {paragraph && (
              <p className="text-[13.5px] leading-relaxed text-shodh-text-muted whitespace-pre-wrap break-words">{paragraph}</p>
            )}
          </>
        )}
      </article>
    </div>
  );
}

function TableView({ rows }: { rows: string[][] }) {
  const [header, ...body] = rows;
  return (
    <table className="border-collapse rounded-xl border border-shodh-border bg-shodh-surface" style={{ fontSize: '1em' }}>
      {header && (
        <thead className="bg-shodh-raised">
          <tr>
            {header.map((cell, i) => (
              <th key={i} scope="col" className="px-[0.85em] py-[0.55em] text-left font-semibold text-shodh-text border-b border-shodh-border">{cell}</th>
            ))}
          </tr>
        </thead>
      )}
      <tbody>
        {body.map((row, r) => (
          <tr key={r}>
            {row.map((cell, c) => (
              <td key={c} className="px-[0.85em] py-[0.55em] align-top text-shodh-text-secondary border-b border-shodh-border-subtle">{cell}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export interface FocusStageProps {
  target: FocusTarget;
  theme: string;
  commandRef: StageCommandRef;
  /** A document's current page changed. */
  onPageChange?: (page: number) => void;
}

/** The focused object, drawn for close reading. */
export function FocusStage({ target, theme, commandRef, onPageChange }: FocusStageProps) {
  // Documents and tasks have no stage zoom (PDF pages zoom in their own viewer).
  // Interactive visuals have sliders and controls instead of zoom.
  if (target.kind === 'source' || target.kind === 'task' || target.kind === 'selection' || target.kind === 'snippet' || target.kind === 'plot' || target.kind === 'simulation') {
    commandRef.current = null;
  }
  switch (target.kind) {
    case 'mermaid':
      return <MermaidStage source={target.source} label={target.label} dark={theme === 'dark'} commandRef={commandRef} />;
    case 'chart':
      return <ChartStage source={target.source} label={target.label} theme={theme} commandRef={commandRef} />;
    case 'svg':
      return <SvgStage source={target.source} label={target.label} commandRef={commandRef} />;
    case 'plot':
      return <PlotStage source={target.source} values={target.values} />;
    case 'simulation':
      return <SimulationStage source={target.source} values={target.values} />;
    case 'image':
      return <ImageStage src={target.src} alt={target.alt} label={target.label} commandRef={commandRef} />;
    case 'equation':
      return (
        <TextStage baseFont={EQUATION_FONT} label={target.label} commandRef={commandRef}>
          <ReactMarkdown remarkPlugins={[remarkMath]} rehypePlugins={[rehypeKatex]}>
            {`$$\n${target.tex}\n$$`}
          </ReactMarkdown>
        </TextStage>
      );
    case 'table':
      return (
        <TextStage baseFont={TABLE_FONT} label={target.label} commandRef={commandRef}>
          <TableView rows={target.rows} />
        </TextStage>
      );
    case 'source':
      return <SourceStage hit={target.hit} onPageChange={onPageChange} />;
    case 'task':
      return <TaskStage task={target.task} />;
    case 'selection':
      return <SelectionStage target={target} />;
    case 'snippet':
      return <SnippetStage target={target} />;
  }
}
