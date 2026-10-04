import React, { useEffect, useId, useMemo, useState } from 'react';
import { AlertTriangle, ChartColumn, Info, Loader2 } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext';
import { cn } from '../../lib/utils';
import { ChartArtifact } from '../../components/ChartArtifact';
import type { Artifact } from '../../components/EnhancedArtifactPanel';
import { parseChartBlock } from '../ask/visual/chartSpec';
import { onResearchChanged, researchApi, toResearchError } from './api';
import { columnChart, columnLabel, comparisonRows, sortedFacets, valueLabel } from './comparison';
import { showSourceBox } from './snippetBus';
import type { Comparison, ComparisonCell, ResultFacets, ResultFilter } from './types';
import { INPUT, SECTION_TITLE } from './ui';
import { FOCUS_RING } from './ui';

type Load<T> = { status: 'loading' } | { status: 'ready'; value: T } | { status: 'error'; message: string };

function CellValue({ cell, method, column }: { cell: ComparisonCell; method: string; column: string }) {
  return (
    <button
      type="button"
      onClick={() =>
        showSourceBox({
          filePath: cell.filePath,
          fileName: cell.fileName,
          page: cell.page,
          regions: cell.region ? [cell.region] : null,
          label: `${method} · ${column}`,
        })
      }
      className={cn('text-left rounded hover:text-shodh-accent-text', FOCUS_RING)}
      title={`Show the cell in ${cell.fileName}, page ${cell.page}`}
    >
      <span className="tabular-nums text-shodh-text underline decoration-dotted underline-offset-2">{valueLabel(cell)}</span>
      <span className="block text-[11px] text-shodh-text-muted">{`${cell.fileName} p.${cell.page}${cell.setting ? ` · ${cell.setting}` : ''}`}</span>
    </button>
  );
}

/**
 * Cross-paper comparison builder: pick a metric and/or dataset (and
 * optionally a method or papers), get a methods × (dataset, metric) table
 * with a chart. Every cell opens its source box in the paper; coverage
 * notes say which papers report the metric and which do not.
 */
export function CompareResults({
  workspace = null,
  initialMethod = '',
  initialDataset = '',
}: {
  workspace?: string | null;
  /** Method id to start with (a method page compares its results). */
  initialMethod?: string;
  /** Dataset id to start with (a dataset page). */
  initialDataset?: string;
}) {
  const { theme } = useTheme();
  const metricId = useId();
  const datasetId = useId();
  const methodId = useId();
  const chartColumnId = useId();
  const [facets, setFacets] = useState<Load<ResultFacets>>({ status: 'loading' });
  const [metric, setMetric] = useState('');
  const [dataset, setDataset] = useState(initialDataset);
  const [method, setMethod] = useState(initialMethod);
  const [papers, setPapers] = useState<string[]>([]);
  const [result, setResult] = useState<Load<Comparison> | null>(null);
  const [chartColumn, setChartColumn] = useState('');
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    researchApi
      .resultFacets(workspace)
      .then(value => {
        if (!cancelled) setFacets({ status: 'ready', value: sortedFacets(value) });
      })
      .catch(error => {
        if (!cancelled) setFacets({ status: 'error', message: toResearchError(error).message });
      });
    return () => {
      cancelled = true;
    };
  }, [workspace, tick]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onResearchChanged(change => {
      if (change.kind !== 'snippet') setTick(t => t + 1);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const filter = useMemo<ResultFilter | null>(() => {
    if (!metric && !dataset && !method) return null;
    return { metric: metric || null, dataset: dataset || null, method: method || null, papers: papers.length > 0 ? papers : null, workspace };
  }, [metric, dataset, method, papers, workspace]);

  useEffect(() => {
    if (!filter) {
      setResult(null);
      return;
    }
    let cancelled = false;
    setResult({ status: 'loading' });
    researchApi
      .queryResults(filter)
      .then(value => {
        if (cancelled) return;
        setResult({ status: 'ready', value });
        setChartColumn(prev => (value.columns.some(c => c.key === prev) ? prev : value.columns[0]?.key ?? ''));
      })
      .catch(error => {
        if (!cancelled) setResult({ status: 'error', message: toResearchError(error).message });
      });
    return () => {
      cancelled = true;
    };
  }, [filter, tick]);

  const comparison = result?.status === 'ready' ? result.value : null;
  const rows = useMemo(() => (comparison ? comparisonRows(comparison) : []), [comparison]);
  const chart = useMemo(() => (comparison && chartColumn ? columnChart(comparison, chartColumn) : null), [comparison, chartColumn]);
  const artifact = useMemo<Artifact | null>(() => {
    if (!chart) return null;
    const parsed = parseChartBlock(JSON.stringify(chart.spec));
    if (!parsed.ok) return null;
    return { id: 'compare-chart', artifact_type: 'chart', title: chart.spec.title, content: JSON.stringify(parsed.chart), editable: false, version: 1, created_at: '' } as Artifact;
  }, [chart]);

  if (facets.status === 'loading') {
    return (
      <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        Loading results…
      </p>
    );
  }
  if (facets.status === 'error') {
    return (
      <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
        <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        {`Results could not be loaded: ${facets.message}`}
      </p>
    );
  }
  const f = facets.value;
  if (f.metrics.length === 0) {
    return (
      <p className="text-[12.5px] text-shodh-text-muted">
        No results extracted yet. Open a paper in a folder, choose Results and extract them; then compare them across papers here.
      </p>
    );
  }

  const select = (id: string, label: string, value: string, set: (v: string) => void, options: { id: string; label: string; count: number }[], any: string) => (
    <div className="flex flex-col gap-1 min-w-[180px] flex-1">
      <label htmlFor={id} className="text-[12px] font-medium text-shodh-text-secondary">{label}</label>
      <select id={id} className={INPUT} value={value} onChange={e => set(e.target.value)}>
        <option value="">{any}</option>
        {options.map(o => (
          <option key={o.id} value={o.id}>{`${o.label} (${o.count})`}</option>
        ))}
      </select>
    </div>
  );

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap gap-3">
        {select(metricId, 'Metric', metric, setMetric, f.metrics, 'Any metric')}
        {select(datasetId, 'Dataset', dataset, setDataset, f.datasets, 'Any dataset')}
        {select(methodId, 'Method (optional)', method, setMethod, f.methods, 'Every method')}
      </div>
      {f.papers.length > 1 && (
        <fieldset className="flex flex-col gap-1.5">
          <legend className="text-[12px] font-medium text-shodh-text-secondary">Papers (none selected means all)</legend>
          <div className="flex flex-wrap gap-1.5">
            {f.papers.map(p => {
              const on = papers.includes(p.filePath);
              return (
                <button
                  key={p.filePath}
                  type="button"
                  aria-pressed={on}
                  onClick={() => setPapers(list => (on ? list.filter(x => x !== p.filePath) : [...list, p.filePath]))}
                  className={cn(
                    'h-7 px-2.5 rounded-full text-[12px] border max-w-[260px] truncate transition-colors duration-micro',
                    on ? 'bg-shodh-accent-soft border-shodh-accent text-shodh-accent-text' : 'border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised',
                    FOCUS_RING,
                  )}
                  title={p.filePath}
                >
                  {`${p.fileName} (${p.results})`}
                </button>
              );
            })}
          </div>
        </fieldset>
      )}

      {!filter ? (
        <p className="text-[12.5px] text-shodh-text-muted">Choose a metric, a dataset or a method to compare papers.</p>
      ) : result?.status === 'loading' ? (
        <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Comparing…
        </p>
      ) : result?.status === 'error' ? (
        <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          {`The comparison failed: ${result.message}`}
        </p>
      ) : comparison ? (
        <>
          {(comparison.notes.length > 0 || comparison.pendingReview > 0 || comparison.truncated) && (
            <div role="note" className="rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
              <Info className="w-4 h-4 mt-0.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
              <ul className="flex flex-col gap-1">
                {comparison.notes.map((n, i) => <li key={i}>{n}</li>)}
                {comparison.pendingReview > 0 && (
                  <li className="text-shodh-text">{`${comparison.pendingReview} matching ${comparison.pendingReview === 1 ? 'result waits' : 'results wait'} for review and ${comparison.pendingReview === 1 ? 'is' : 'are'} not shown. Review them in each paper’s Results.`}</li>
                )}
                {comparison.truncated && <li>Only the first rows are shown; narrow the filters to see the rest.</li>}
              </ul>
            </div>
          )}
          {rows.length === 0 ? (
            <p className="text-[12.5px] text-shodh-text-muted">No accepted results match.</p>
          ) : (
            <div className="overflow-x-auto rounded-xl border border-shodh-border">
              <table className="min-w-full border-collapse text-[12.5px]">
                <caption className="sr-only">Results by method and dataset/metric; each value opens its cell in the paper.</caption>
                <thead className="bg-shodh-raised">
                  <tr>
                    <th scope="col" className="px-3 py-1.5 text-left font-semibold text-shodh-text border-b border-shodh-border">Method</th>
                    {comparison.columns.map(c => (
                      <th key={c.key} scope="col" className="px-3 py-1.5 text-left font-semibold text-shodh-text border-b border-shodh-border whitespace-nowrap">{columnLabel(c)}</th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {rows.map(row => (
                    <tr key={row.methodId} className="align-top">
                      <th scope="row" className="px-3 py-1.5 text-left font-medium text-shodh-text border-b border-shodh-border-subtle">{row.method}</th>
                      {row.cells.map((cell, i) => (
                        <td key={cell.key} className="px-3 py-1.5 border-b border-shodh-border-subtle">
                          {cell.values.length === 0 ? (
                            <span className="text-shodh-text-faint" aria-label="not reported">—</span>
                          ) : (
                            <ul className="flex flex-col gap-1">
                              {cell.values.map(v => (
                                <li key={v.resultId}>
                                  <CellValue cell={v} method={row.method} column={columnLabel(comparison.columns[i])} />
                                </li>
                              ))}
                            </ul>
                          )}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          {comparison.columns.length > 0 && rows.length > 0 && (
            <section aria-labelledby={`${chartColumnId}-title`} className="flex flex-col gap-2">
              <div className="flex flex-wrap items-center gap-2">
                <h3 id={`${chartColumnId}-title`} className={cn(SECTION_TITLE, 'inline-flex items-center gap-1.5')}>
                  <ChartColumn className="w-3.5 h-3.5" aria-hidden="true" />
                  Chart
                </h3>
                {comparison.columns.length > 1 && (
                  <>
                    <label htmlFor={chartColumnId} className="sr-only">Column to chart</label>
                    <select id={chartColumnId} className={cn(INPUT, 'w-auto')} value={chartColumn} onChange={e => setChartColumn(e.target.value)}>
                      {comparison.columns.map(c => (
                        <option key={c.key} value={c.key}>{columnLabel(c)}</option>
                      ))}
                    </select>
                  </>
                )}
              </div>
              {artifact ? (
                <div className="rounded-xl border border-shodh-border bg-shodh-surface p-2">
                  <ChartArtifact artifact={artifact} theme={theme} height={300} />
                </div>
              ) : (
                <p className="text-[12.5px] text-shodh-text-muted">The values in this column are not plain numbers, so they are not charted.</p>
              )}
              {chart && chart.multiple > 0 && (
                <p className="text-[11.5px] text-shodh-text-muted">{`${chart.multiple} further ${chart.multiple === 1 ? 'value' : 'values'} from the same paper (other settings) are in the table but not in the chart.`}</p>
              )}
              {chart && chart.skipped > 0 && (
                <p className="text-[11.5px] text-shodh-text-muted">{`${chart.skipped} ${chart.skipped === 1 ? 'value is' : 'values are'} not a plain number and not charted.`}</p>
              )}
            </section>
          )}
        </>
      ) : null}
    </div>
  );
}
