import { useCallback, useMemo } from 'react';
import { AlertTriangle } from 'lucide-react';
import { ChartArtifact } from '../../../components/ChartArtifact';
import type { Artifact } from '../../../components/EnhancedArtifactPanel';
import { parseChartBlock } from './chartSpec';
import { FocusFrame } from '../../focus/FocusFrame';
import { chartTarget } from '../../focus/targets';

export { renderDiagram } from './mermaidRender';
export { MermaidBlock } from './MermaidBlock';

export function BlockError({ title, message, source }: { title: string; message: string; source: string }) {
  return (
    <figure className="my-4 rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
      <figcaption className="flex items-start gap-2 px-3 py-2 text-[12.5px] text-shodh-text-secondary border-b border-shodh-border-subtle">
        <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        <span>
          {title}: {message}
        </span>
      </figcaption>
      <pre className="m-0 p-3 max-h-60 overflow-auto scrollbar-thin text-[12px] leading-relaxed font-mono text-shodh-text-muted whitespace-pre-wrap">{source}</pre>
    </figure>
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
  const getTarget = useCallback(() => chartTarget(source, title), [source, title]);

  if ('error' in result) return <BlockError title="Chart not drawn" message={result.error} source={source} />;
  if (!artifact) return null;
  return (
    <FocusFrame noun="chart" getTarget={getTarget} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border overflow-hidden">
        <ChartArtifact artifact={artifact} theme={theme} />
      </figure>
    </FocusFrame>
  );
}
