import React, { useEffect, useId, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import { AlertTriangle, Play } from 'lucide-react';
import { ChartArtifact } from '../../components/ChartArtifact';
import type { Artifact } from '../../components/EnhancedArtifactPanel';
import { cn } from '../../lib/utils';
import { parseChartBlock } from '../ask/visual/chartSpec';
import { initialValues, parsePlotSpec } from '../ask/visual/plotSpec';
import { InteractivePlot } from '../ask/visual/PlotView';
import { parseSimulationSpec } from '../ask/visual/simulationSpec';
import { SimulationPlayer } from '../ask/visual/SimulationView';
import { SketchSurface, useSvgSketch } from '../ask/visual/SvgSketch';
import { renderDiagram } from '../ask/visual/VisualBlocks';
import { markdownTableRows } from './extract';
import type { VisualRecord } from './model';
import { paramValuesOf } from './model';

/** Height of a thumbnail, in px. */
export const THUMB_HEIGHT = 150;
const THUMB_ROWS = 6;
const THUMB_COLUMNS = 4;

const NOOP = () => undefined;

/** True once the element has come near the viewport (thumbnails render lazily). */
function useNearViewport(ref: React.RefObject<HTMLElement | null>): boolean {
  const [near, setNear] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || near) return;
    if (typeof IntersectionObserver === 'undefined') {
      setNear(true);
      return;
    }
    const observer = new IntersectionObserver(entries => {
      if (entries.some(e => e.isIntersecting)) {
        setNear(true);
        observer.disconnect();
      }
    }, { rootMargin: '200px' });
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref, near]);
  return near;
}

function ThumbProblem({ text }: { text: string }) {
  return (
    <div className="h-full flex items-center justify-center gap-1.5 px-3 text-[12px] text-shodh-text-muted">
      <AlertTriangle className="w-3.5 h-3.5 shrink-0 text-shodh-warning" aria-hidden="true" />
      {text}
    </div>
  );
}

function MermaidThumb({ source, dark }: { source: string; dark: boolean }) {
  const reactId = useId();
  const domId = `thumb-mmd-${reactId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const [svg, setSvg] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setSvg(null);
    setFailed(false);
    renderDiagram(domId, source, dark)
      .then(markup => { if (!cancelled) setSvg(markup); })
      .catch(() => { if (!cancelled) setFailed(true); });
    return () => { cancelled = true; };
  }, [domId, source, dark]);
  if (failed) return <ThumbProblem text="Diagram not drawn" />;
  if (!svg) return <div className="h-full animate-pulse motion-reduce:animate-none bg-shodh-raised" />;
  // mermaid renders with securityLevel 'strict' (sanitised, no scripts).
  return <div className="h-full flex items-center justify-center p-2 [&_svg]:max-w-full [&_svg]:max-h-full [&_svg]:h-auto" dangerouslySetInnerHTML={{ __html: svg }} />;
}

function ChartThumb({ source, theme }: { source: string; theme: string }) {
  const result = useMemo(() => parseChartBlock(source), [source]);
  const artifact = useMemo<Artifact | null>(() => {
    if (!result.ok) return null;
    return {
      id: `thumb-chart-${source.length}`,
      artifact_type: 'chart',
      title: '',
      content: JSON.stringify({ ...result.chart, title: undefined }),
      editable: false,
      version: 1,
      created_at: '',
    } as Artifact;
  }, [result, source.length]);
  if (!artifact) return <ThumbProblem text="Chart not drawn" />;
  return (
    <div className="h-full overflow-hidden [&_.recharts-legend-wrapper]:hidden">
      <ChartArtifact artifact={artifact} theme={theme} />
    </div>
  );
}

function SvgThumb({ source, label }: { source: string; label: string }) {
  const { result, markup, seed } = useSvgSketch(source);
  if (!result.ok) return <ThumbProblem text="Sketch not drawn" />;
  return (
    <SketchSurface
      markup={markup}
      sketch={result.sketch}
      seed={seed}
      label={label}
      className="h-full flex items-center justify-center p-2 [&_svg]:max-w-full [&_svg]:max-h-full [&_svg]:h-auto"
    />
  );
}

function PlotThumb({ source, record }: { source: string; record: Pick<VisualRecord, 'params'> }) {
  const result = useMemo(() => parsePlotSpec(source), [source]);
  const values = useMemo(
    () => (result.ok ? initialValues(result.spec.params, paramValuesOf(record.params)) : []),
    [result, record.params],
  );
  if (!result.ok) return <ThumbProblem text="Plot not drawn" />;
  return (
    <div className="h-full overflow-hidden px-2 pt-2">
      <InteractivePlot spec={result.spec} values={values} onValues={NOOP} maxHeight={THUMB_HEIGHT - 12} />
    </div>
  );
}

/** First frame of a simulation (never started), with a play mark. */
function SimulationThumb({ source, record }: { source: string; record: Pick<VisualRecord, 'params'> }) {
  const result = useMemo(() => parseSimulationSpec(source), [source]);
  if (!result.ok) return <ThumbProblem text="Simulation not run" />;
  return (
    <div className="relative h-full overflow-hidden px-2 pt-2">
      <SimulationPlayer model={result.model} initial={paramValuesOf(record.params)} maxHeight={THUMB_HEIGHT - 12} autoPlay={false} />
      <span className="absolute inset-0 flex items-center justify-center" aria-hidden="true">
        <span className="w-10 h-10 rounded-full bg-shodh-surface/90 border border-shodh-border flex items-center justify-center shadow-sm">
          <Play className="w-4 h-4 ml-0.5 text-shodh-text" />
        </span>
      </span>
    </div>
  );
}

function EquationThumb({ tex }: { tex: string }) {
  return (
    <div className="h-full flex items-center justify-center px-3 overflow-hidden text-[13px] text-shodh-text [&_.katex-display]:m-0">
      <ReactMarkdown remarkPlugins={[remarkMath]} rehypePlugins={[rehypeKatex]}>{`$$\n${tex}\n$$`}</ReactMarkdown>
    </div>
  );
}

function TableThumb({ source }: { source: string }) {
  const rows = useMemo(() => markdownTableRows(source).slice(0, THUMB_ROWS).map(r => r.slice(0, THUMB_COLUMNS)), [source]);
  if (rows.length === 0) return <ThumbProblem text="Table not readable" />;
  const [header, ...body] = rows;
  return (
    <div className="h-full overflow-hidden p-2">
      <table className="w-full text-[11px] border-collapse">
        <thead>
          <tr>{header.map((c, i) => <th key={i} className="px-1.5 py-1 text-left font-semibold text-shodh-text border-b border-shodh-border truncate max-w-[120px]">{c}</th>)}</tr>
        </thead>
        <tbody>
          {body.map((row, r) => (
            <tr key={r}>{row.map((c, i) => <td key={i} className="px-1.5 py-0.5 text-shodh-text-secondary border-b border-shodh-border-subtle truncate max-w-[120px]">{c}</td>)}</tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * A small, static rendering of a stored visual: the visual itself, drawn
 * when the card nears the viewport. Not interactive (`inert`): the card
 * around it is the control.
 */
export function VisualThumb({
  record,
  theme,
  className,
}: {
  record: Pick<VisualRecord, 'kind' | 'title' | 'source' | 'params'>;
  theme: string;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const near = useNearViewport(ref);
  const dark = theme === 'dark';
  let body: React.ReactNode = null;
  if (near) {
    switch (record.kind) {
      case 'mermaid':
        body = <MermaidThumb source={record.source} dark={dark} />;
        break;
      case 'chart':
        body = <ChartThumb source={record.source} theme={theme} />;
        break;
      case 'svg':
        body = <SvgThumb source={record.source} label={record.title} />;
        break;
      case 'plot':
        body = <PlotThumb source={record.source} record={record} />;
        break;
      case 'simulation':
        body = <SimulationThumb source={record.source} record={record} />;
        break;
      case 'equation':
        body = <EquationThumb tex={record.source} />;
        break;
      case 'table':
        body = <TableThumb source={record.source} />;
        break;
    }
  }
  return (
    <div
      ref={ref}
      inert
      aria-hidden="true"
      data-visual-thumb=""
      className={cn('relative overflow-hidden bg-shodh-surface pointer-events-none select-none', className)}
      style={{ height: THUMB_HEIGHT }}
    >
      {body}
    </div>
  );
}
