import React, { useCallback, useEffect, useId, useState } from 'react';
import { AlertTriangle, Check, Loader2, RotateCcw, Sparkles, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { relativeTime } from '../../utils/time';
import { onResearchChanged, researchApi, toResearchError } from './api';
import { groupByTable, provenanceLabel, valueLabel } from './comparison';
import { showSourceBox } from './snippetBus';
import { fileNameOf } from './snippetModel';
import type { ExtractionReport, PaperResults, ResultRecord } from './types';
import { BUTTON, FOCUS_RING, PRIMARY_BUTTON, SECTION_TITLE } from './ui';

type State = { status: 'loading' } | { status: 'ready'; data: PaperResults } | { status: 'error'; message: string };

/** Opens the paper at a result's cell with its box outlined. */
export function showResultCell(r: Pick<ResultRecord, 'filePath' | 'fileName' | 'page' | 'region' | 'method' | 'metric'>): void {
  showSourceBox({
    filePath: r.filePath,
    fileName: r.fileName || fileNameOf(r.filePath),
    page: r.page,
    regions: r.region ? [r.region] : null,
    label: [r.method, r.metric].filter(Boolean).join(' · '),
  });
}

/** A value that opens its cell in the paper. */
export function ValueButton({ result, className }: { result: ResultRecord; className?: string }) {
  return (
    <button
      type="button"
      onClick={() => showResultCell(result)}
      className={cn('tabular-nums underline decoration-dotted underline-offset-2 hover:text-shodh-accent-text rounded', FOCUS_RING, className)}
      title={`Show the cell on page ${result.page}`}
      aria-label={`${valueLabel(result)}: show the cell on page ${result.page}`}
    >
      {valueLabel(result)}
    </button>
  );
}

function ReportSummary({ report }: { report: ExtractionReport }) {
  return (
    <div className="rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 flex flex-col gap-2 text-[12.5px] text-shodh-text-secondary">
      <p>
        {`${report.tables} ${report.tables === 1 ? 'table' : 'tables'} read · ${report.added} added · ${report.unchanged} unchanged · ${report.review} to review`}
        {report.conflicts > 0 ? ` · ${report.conflicts} differ from results you confirmed (kept yours)` : ''}
        {' · '}
        <time dateTime={report.extractedAt}>{relativeTime(report.extractedAt)}</time>
        {report.model ? ` · headers interpreted with ${report.model}` : ' · rules only'}
      </p>
      {!report.tableModel && (
        <p className="text-shodh-text-muted">
          Tables were read with the layout heuristics only, so table extraction quality is reduced: merged headers, spanning cells and some tables can be missed. Install the table model in Settings to structure tables with it.
        </p>
      )}
      {report.skipped.length > 0 && (
        <details>
          <summary className={cn('cursor-pointer w-fit rounded text-shodh-text', FOCUS_RING)}>
            {`${report.skipped.length} ${report.skipped.length === 1 ? 'table' : 'tables'} gave no results`}
          </summary>
          <ul className="mt-2 flex flex-col gap-1 pl-4 list-disc">
            {report.skipped.map((t, i) => (
              <li key={i}>
                <span className="text-shodh-text">{`Page ${t.page}${t.caption ? ` — ${t.caption.slice(0, 120)}` : ''}`}</span>
                {`: ${t.reason}`}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

function ResultTable({ results, review, onReview, busyId }: { results: ResultRecord[]; review: boolean; onReview?: (r: ResultRecord, accept: boolean) => void; busyId: string | null }) {
  return (
    <div className="overflow-x-auto rounded-xl border border-shodh-border">
      <table className="min-w-full border-collapse text-[12.5px]">
        <thead className="bg-shodh-raised">
          <tr>
            {['Method', 'Dataset', 'Metric', 'Value', 'Setting', 'Source'].map(h => (
              <th key={h} scope="col" className="px-3 py-1.5 text-left font-semibold text-shodh-text border-b border-shodh-border whitespace-nowrap">{h}</th>
            ))}
            {review && <th scope="col" className="px-3 py-1.5 text-left font-semibold text-shodh-text border-b border-shodh-border">Decision</th>}
          </tr>
        </thead>
        <tbody>
          {results.map(r => (
            <tr key={r.id} className="align-top">
              <th scope="row" className="px-3 py-1.5 text-left font-medium text-shodh-text border-b border-shodh-border-subtle">{r.method}</th>
              <td className="px-3 py-1.5 text-shodh-text-secondary border-b border-shodh-border-subtle">{r.dataset}</td>
              <td className="px-3 py-1.5 text-shodh-text-secondary border-b border-shodh-border-subtle">{r.metric}</td>
              <td className="px-3 py-1.5 text-shodh-text border-b border-shodh-border-subtle whitespace-nowrap">
                <ValueButton result={r} />
              </td>
              <td className="px-3 py-1.5 text-shodh-text-muted border-b border-shodh-border-subtle">{r.setting ?? ''}</td>
              <td className="px-3 py-1.5 text-shodh-text-muted border-b border-shodh-border-subtle whitespace-nowrap">{`p.${r.page} · ${provenanceLabel(r)}`}</td>
              {review && onReview && (
                <td className="px-3 py-1.5 border-b border-shodh-border-subtle whitespace-nowrap">
                  <span className="inline-flex gap-1">
                    <button type="button" className={BUTTON} disabled={busyId === r.id} onClick={() => onReview(r, true)} aria-label={`Accept ${r.method} ${r.metric} ${r.valueText}`}>
                      <Check className="w-3.5 h-3.5" aria-hidden="true" />
                      Accept
                    </button>
                    <button type="button" className={BUTTON} disabled={busyId === r.id} onClick={() => onReview(r, false)} aria-label={`Reject ${r.method} ${r.metric} ${r.valueText}`}>
                      <X className="w-3.5 h-3.5" aria-hidden="true" />
                      Reject
                    </button>
                  </span>
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * A paper's Result statements: extract them from its tables (rules first;
 * optionally the configured model interprets headers, never values), the
 * extraction report, the results by table and the review list. Every value
 * opens its cell in the paper.
 */
export function ResultsView({ filePath, workspace = null }: { filePath: string; workspace?: string | null }) {
  const useModelId = useId();
  const [state, setState] = useState<State>({ status: 'loading' });
  const [tick, setTick] = useState(0);
  const [useModel, setUseModel] = useState(false);
  const [extracting, setExtracting] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setState(s => (s.status === 'ready' ? s : { status: 'loading' }));
    researchApi
      .listResults(filePath)
      .then(data => {
        if (!cancelled) setState({ status: 'ready', data });
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: toResearchError(error).message });
      });
    return () => {
      cancelled = true;
    };
  }, [filePath, tick]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onResearchChanged(change => {
      if (change.kind !== 'snippet' && (change.filePath === null || change.filePath === filePath)) setTick(t => t + 1);
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
  }, [filePath]);

  const extract = async () => {
    setExtracting(true);
    try {
      const report = await researchApi.extractResults(filePath, workspace, useModel);
      notify.success(`${report.added} results added, ${report.review} to review`);
      setTick(t => t + 1);
    } catch (error) {
      notify.error('Results could not be extracted', { description: toResearchError(error).message });
    } finally {
      setExtracting(false);
    }
  };

  const review = useCallback(async (r: ResultRecord, accept: boolean) => {
    setBusyId(r.id);
    try {
      await researchApi.reviewResult(r.id, accept);
      setTick(t => t + 1);
    } catch (error) {
      notify.error(accept ? 'The result could not be accepted' : 'The result could not be rejected', { description: toResearchError(error).message });
    } finally {
      setBusyId(null);
    }
  }, []);

  const data = state.status === 'ready' ? state.data : null;
  const groups = data ? groupByTable(data.results) : [];

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <button type="button" className={PRIMARY_BUTTON} onClick={() => void extract()} disabled={extracting}>
          {extracting ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : data?.report ? <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" /> : <Sparkles className="w-3.5 h-3.5" aria-hidden="true" />}
          {data?.report ? 'Extract again' : 'Extract results'}
        </button>
        <label htmlFor={useModelId} className="inline-flex items-center gap-2 text-[12.5px] text-shodh-text-secondary">
          <input id={useModelId} type="checkbox" checked={useModel} onChange={e => setUseModel(e.target.checked)} className="accent-shodh-accent" />
          Let the configured model interpret unclear table headers (it never supplies values)
        </label>
      </div>

      {state.status === 'loading' ? (
        <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Loading results…
        </p>
      ) : state.status === 'error' ? (
        <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          {`Results could not be loaded: ${state.message}`}
        </p>
      ) : (
        <>
          {data?.report ? (
            <ReportSummary report={data.report} />
          ) : (
            <p className="text-[12.5px] text-shodh-text-muted">
              No results have been extracted from this paper yet. Extraction reads the parser’s table blocks: numbers under metric headers, row labels as methods, datasets from headers and captions. Every value is taken verbatim from its cell.
            </p>
          )}
          {data && data.review.length > 0 && (
            <section aria-labelledby={`${useModelId}-review`} className="flex flex-col gap-2">
              <h3 id={`${useModelId}-review`} className={SECTION_TITLE}>{`To review (${data.review.length}) · not used until you accept them`}</h3>
              <ResultTable results={data.review} review onReview={(r, accept) => void review(r, accept)} busyId={busyId} />
            </section>
          )}
          {groups.map(group => (
            <section key={`${group.page}:${group.caption}`} aria-label={group.caption} className="flex flex-col gap-2">
              <h3 className={SECTION_TITLE}>{`${group.caption.slice(0, 140)} · page ${group.page}`}</h3>
              <ResultTable results={group.results} review={false} busyId={busyId} />
            </section>
          ))}
          {data?.report && data.results.length === 0 && data.review.length === 0 && (
            <p className="text-[12.5px] text-shodh-text-muted">No results were found in this paper’s tables (see the report above for why).</p>
          )}
        </>
      )}
    </div>
  );
}
