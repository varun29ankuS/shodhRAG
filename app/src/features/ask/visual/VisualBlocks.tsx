import { useEffect, useId, useMemo, useState } from 'react';
import { AlertTriangle, Loader2 } from 'lucide-react';
import { ChartArtifact } from '../../../components/ChartArtifact';
import type { Artifact } from '../../../components/EnhancedArtifactPanel';
import { parseChartBlock } from './chartSpec';

type Mermaid = typeof import('mermaid').default;

let mermaidPromise: Promise<Mermaid> | null = null;

/** mermaid is large; load it on the first diagram only. */
function loadMermaid(): Promise<Mermaid> {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid')
      .then(mod => mod.default)
      .catch(error => {
        mermaidPromise = null;
        throw error;
      });
  }
  return mermaidPromise;
}

// mermaid.render is global-stateful; serialise renders so concurrent diagrams
// (several in one answer, or a streaming re-render) cannot interleave.
let renderQueue: Promise<unknown> = Promise.resolve();

function renderDiagram(id: string, source: string, dark: boolean): Promise<string> {
  const job = renderQueue.then(async () => {
    const mermaid = await loadMermaid();
    mermaid.initialize({
      startOnLoad: false,
      // Diagrams come from model output: no scripts, no click handlers, labels escaped.
      securityLevel: 'strict',
      theme: dark ? 'dark' : 'default',
      fontFamily: '"Geist Variable", system-ui, sans-serif',
    });
    await mermaid.parse(source);
    const { svg } = await mermaid.render(id, source);
    return svg;
  });
  renderQueue = job.catch(() => undefined);
  return job;
}

function BlockError({ title, message, source }: { title: string; message: string; source: string }) {
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

export function MermaidBlock({ source, dark }: { source: string; dark: boolean }) {
  const reactId = useId();
  const domId = `mmd-${reactId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const [state, setState] = useState<{ status: 'loading' } | { status: 'ready'; svg: string } | { status: 'error'; message: string }>({
    status: 'loading',
  });

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    renderDiagram(domId, source.trim(), dark)
      .then(svg => {
        if (!cancelled) setState({ status: 'ready', svg });
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: error instanceof Error ? error.message.split('\n')[0] : 'The diagram could not be drawn.' });
      });
    return () => {
      cancelled = true;
    };
  }, [domId, source, dark]);

  if (state.status === 'error') return <BlockError title="Diagram not drawn" message={state.message} source={source} />;
  return (
    <figure className="my-4 rounded-xl border border-shodh-border bg-shodh-surface p-4 overflow-x-auto scrollbar-thin" aria-busy={state.status === 'loading'}>
      {state.status === 'loading' ? (
        <div className="flex items-center gap-2 h-24 justify-center text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Drawing diagram…
        </div>
      ) : (
        // SVG produced by mermaid with securityLevel 'strict' (sanitised, no scripts).
        <div role="img" aria-label="Diagram" className="flex justify-center [&_svg]:max-w-full [&_svg]:h-auto" dangerouslySetInnerHTML={{ __html: state.svg }} />
      )}
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

  if ('error' in result) return <BlockError title="Chart not drawn" message={result.error} source={source} />;
  if (!artifact) return null;
  return (
    <figure className="my-4 rounded-xl border border-shodh-border overflow-hidden">
      <ChartArtifact artifact={artifact} theme={theme} />
    </figure>
  );
}
