import { useCallback, useMemo } from 'react';
import { AlertTriangle } from 'lucide-react';
import { ChartArtifact } from '../../../components/ChartArtifact';
import type { Artifact } from '../../../components/EnhancedArtifactPanel';
import { parseChartBlock } from './chartSpec';
import { FocusFrame } from '../../focus/FocusFrame';
import { chartTarget } from '../../focus/targets';
import type { FocusTarget } from '../../focus/focusTypes';
import { cn } from '../../../lib/utils';

export { renderDiagram } from './mermaidRender';
export { MermaidBlock } from './MermaidBlock';

interface BlockErrorProps {
  title: string;
  message: string;
  source: string;
  /**
   * The block as a focus target carrying `message` as its error, so "Expand &
   * ask" opens it with the source and the reason it did not draw. Omitted for
   * blocks that have no focus target of their own.
   */
  ask?: { noun: string; getTarget: () => FocusTarget | null };
}

/** A visual block that could not be drawn: the reason and the source as written. */
export function BlockError({ title, message, source, ask }: BlockErrorProps) {
  const figure = (
    <figure className={cn('rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden', ask ? 'm-0' : 'my-4')}>
      {/* With "Expand & ask", the right padding keeps the text clear of its button. */}
      <figcaption className={cn('flex items-start gap-2 pl-3 py-2 text-[12.5px] text-shodh-text-secondary border-b border-shodh-border-subtle', ask ? 'pr-32' : 'pr-3')}>
        <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        <span>
          {title}: {message}
        </span>
      </figcaption>
      <pre className="m-0 p-3 max-h-60 overflow-auto scrollbar-thin text-[12px] leading-relaxed font-mono text-shodh-text-muted whitespace-pre-wrap">{source}</pre>
    </figure>
  );
  if (!ask) return figure;
  return (
    <FocusFrame noun={ask.noun} getTarget={ask.getTarget} doubleClick={false} className="my-4">
      {figure}
    </FocusFrame>
  );
}

export function ChartBlock({ source, theme }: { source: string; theme: string }) {
  const result = useMemo(() => parseChartBlock(source), [source]);
  const artifact = useMemo<Artifact | null>(() => {
    if (!result.ok) return null;
    return {
      id: `chart-${source.length}-${result.chart.data.labels.length}`,
      artifact_type: 'chart',
      title: result.chart.title ?? 'Chart',
      content: JSON.stringify(result.chart),
      editable: false,
      version: 1,
      created_at: '',
    } as Artifact;
  }, [result, source.length]);
  const title = result.ok ? result.chart.title ?? null : null;
  const error = 'error' in result ? result.error : null;
  const getTarget = useCallback(() => chartTarget(source, title, error), [source, title, error]);

  if (error !== null) return <BlockError title="Chart not drawn" message={error} source={source} ask={{ noun: 'chart', getTarget }} />;
  if (!artifact) return null;
  return (
    <FocusFrame noun="chart" getTarget={getTarget} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border overflow-hidden">
        <ChartArtifact artifact={artifact} theme={theme} />
      </figure>
    </FocusFrame>
  );
}
