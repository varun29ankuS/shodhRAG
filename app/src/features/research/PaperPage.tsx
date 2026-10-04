import React, { useEffect, useId, useMemo, useState } from 'react';
import { AlertTriangle, ArrowLeft, ExternalLink, FileText, Loader2, MessageSquare } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { cn } from '../../lib/utils';
import { onResearchChanged, toResearchError } from './api';
import { graphApi } from './graphApi';
import type { ConceptNode, LinkedPaper, PaperDetail, PaperNode } from './graphTypes';
import { SnippetCard } from './SnippetCard';
import { readSnippet } from './snippetModel';
import { insertIntoComposer, showSourceBox } from './snippetBus';
import { valueLabel } from './comparison';
import type { Snippet } from './types';
import { BUTTON, FOCUS_RING, ICON_BUTTON, SECTION_TITLE } from './ui';

type Load<T> = { status: 'loading' } | { status: 'ready'; value: T } | { status: 'error'; message: string };

/** Rows shown per citation list before "Show all". */
const FOLDED = 12;

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

function paperTitle(p: PaperNode): string {
  return p.title ?? (p.filePath ? fileName(p.filePath) : p.arxivId ? `arXiv:${p.arxivId}` : p.doi ?? p.id);
}

function Meta({ paper }: { paper: PaperNode }) {
  const parts = [paper.authors, paper.venue, paper.year?.toString()].filter(Boolean);
  return <span className="block text-[11.5px] text-shodh-text-muted truncate">{parts.join(' · ')}</span>;
}

function CitationList({
  title,
  items,
  emptyText,
  onOpenPaper,
}: {
  title: string;
  items: LinkedPaper[];
  emptyText: string;
  onOpenPaper: (id: string) => void;
}) {
  const id = useId();
  const [all, setAll] = useState(false);
  const shown = all ? items : items.slice(0, FOLDED);
  return (
    <section aria-labelledby={id} className="flex flex-col gap-2">
      <h3 id={id} className={SECTION_TITLE}>{`${title} (${items.length})`}</h3>
      {items.length === 0 ? (
        <p className="text-[12.5px] text-shodh-text-muted">{emptyText}</p>
      ) : (
        <ul className="flex flex-col divide-y divide-shodh-border-subtle rounded-xl border border-shodh-border">
          {shown.map(item => (
            <li key={item.paper.id} className="flex items-start gap-2 px-3 py-2">
              <button type="button" onClick={() => onOpenPaper(item.paper.id)} className={cn('flex-1 min-w-0 text-left rounded', FOCUS_RING)}>
                <span className={cn('block text-[13px] truncate', item.paper.inLibrary ? 'text-shodh-text font-medium' : 'text-shodh-text-secondary')}>{paperTitle(item.paper)}</span>
                <Meta paper={item.paper} />
              </button>
              {item.evidence && item.citingFile && (
                <button
                  type="button"
                  className={ICON_BUTTON}
                  aria-label={`Show the reference entry in ${fileName(item.citingFile)}${item.evidence.page ? `, page ${item.evidence.page}` : ''}`}
                  title={item.evidence.text}
                  onClick={() =>
                    showSourceBox({
                      filePath: item.citingFile ?? '',
                      fileName: fileName(item.citingFile ?? ''),
                      page: item.evidence?.page ?? 1,
                      regions: item.evidence?.regions.length ? item.evidence.regions : null,
                      label: `Reference: ${paperTitle(item.paper)}`,
                    })
                  }
                >
                  <FileText className="w-3.5 h-3.5" aria-hidden="true" />
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {items.length > FOLDED && (
        <button type="button" onClick={() => setAll(v => !v)} className={cn(BUTTON, 'self-start')} aria-expanded={all}>
          {all ? 'Show fewer' : `Show all ${items.length}`}
        </button>
      )}
    </section>
  );
}

function Concepts({ title, items, kind, onOpenConcept }: { title: string; items: ConceptNode[]; kind: 'method' | 'dataset'; onOpenConcept: (kind: 'method' | 'dataset', id: string) => void }) {
  if (items.length === 0) return null;
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className="text-[12px] font-medium text-shodh-text-secondary mr-1">{title}</span>
      {items.map(c => (
        <button key={c.id} type="button" onClick={() => onOpenConcept(kind, c.id)} className={cn('h-7 px-2.5 rounded-full text-[12px] border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised', FOCUS_RING)}>
          {c.label}
        </button>
      ))}
    </div>
  );
}

/**
 * A paper page (Library → paper): metadata and links, what it cites in the
 * library and elsewhere, which library papers cite it, its results (each
 * value opens its cell), its snippets, related papers by shared references,
 * and Ask about this paper.
 */
export function PaperPage({
  paperId,
  onBack,
  onOpenPaper,
  onOpenConcept,
}: {
  paperId: string;
  onBack: () => void;
  onOpenPaper: (id: string) => void;
  onOpenConcept: (kind: 'method' | 'dataset', id: string) => void;
}) {
  const headingId = useId();
  const [detail, setDetail] = useState<Load<PaperDetail>>({ status: 'loading' });
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setDetail({ status: 'loading' });
    graphApi.paper(paperId).then(
      value => !cancelled && setDetail({ status: 'ready', value }),
      error => !cancelled && setDetail({ status: 'error', message: toResearchError(error).message }),
    );
    return () => {
      cancelled = true;
    };
  }, [paperId, tick]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onResearchChanged(() => setTick(t => t + 1))
      .then(fn => (disposed ? fn() : (unlisten = fn)))
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const snippets = useMemo<Snippet[]>(
    () => (detail.status === 'ready' ? detail.value.snippets.map(readSnippet).filter((s): s is Snippet => s !== null) : []),
    [detail],
  );

  const back = (
    <button type="button" onClick={onBack} className={cn(BUTTON, 'self-start')}>
      <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
      Graph
    </button>
  );

  if (detail.status === 'loading') {
    return (
      <div className="flex flex-col gap-3">
        {back}
        <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Loading the paper…
        </p>
      </div>
    );
  }
  if (detail.status === 'error') {
    return (
      <div className="flex flex-col gap-3">
        {back}
        <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          {detail.message}
        </p>
      </div>
    );
  }
  const d = detail.value;
  const p = d.paper;
  const links = [
    d.links.doi && { label: 'DOI', url: d.links.doi },
    d.links.arxiv && { label: 'arXiv', url: d.links.arxiv },
    d.links.openalex && { label: 'OpenAlex', url: d.links.openalex },
  ].filter((l): l is { label: string; url: string } => Boolean(l));

  return (
    <article aria-labelledby={headingId} className="flex flex-col gap-5">
      {back}
      <header className="flex flex-col gap-1.5">
        <h2 id={headingId} className="m-0 text-[19px] font-semibold text-shodh-text leading-snug">{paperTitle(p)}</h2>
        {d.authors.length > 0 && <p className="text-[13px] text-shodh-text-secondary">{d.authors.map(a => a.name).join(', ')}{p.authors?.endsWith('et al.') ? ' et al.' : ''}</p>}
        <p className="text-[12.5px] text-shodh-text-muted">
          {[p.venue, p.year, p.inLibrary ? 'In your library' : 'Not in your library', p.citedByCount !== null ? `cited by ${p.citedByCount.toLocaleString()} works (OpenAlex)` : null].filter(Boolean).join(' · ')}
        </p>
        <div className="flex flex-wrap items-center gap-2 pt-1">
          {links.map(l => (
            <button key={l.label} type="button" onClick={() => void openUrl(l.url)} className={BUTTON} aria-label={`Open on ${l.label} (${l.url})`}>
              <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
              {l.label}
            </button>
          ))}
          {p.filePath && (
            <button
              type="button"
              className={BUTTON}
              onClick={() => showSourceBox({ filePath: p.filePath ?? '', fileName: fileName(p.filePath ?? ''), page: 1, label: paperTitle(p) })}
            >
              <FileText className="w-3.5 h-3.5" aria-hidden="true" />
              Open PDF
            </button>
          )}
          <button type="button" className={BUTTON} onClick={() => insertIntoComposer(`About "${paperTitle(p)}"${p.filePath ? ` (${p.filePath})` : ''}: `)}>
            <MessageSquare className="w-3.5 h-3.5" aria-hidden="true" />
            Ask about this paper
          </button>
        </div>
      </header>

      <Concepts title="Proposes" items={d.proposes} kind="method" onOpenConcept={onOpenConcept} />
      <Concepts title="Methods" items={d.methods} kind="method" onOpenConcept={onOpenConcept} />
      <Concepts title="Datasets" items={d.datasets} kind="dataset" onOpenConcept={onOpenConcept} />

      <div className="grid gap-5 md:grid-cols-2">
        <CitationList title="Cites — in your library" items={d.citesInLibrary} emptyText="It cites none of your other papers." onOpenPaper={onOpenPaper} />
        <CitationList title="Cited by — in your library" items={d.citedByInLibrary} emptyText="None of your papers cite it." onOpenPaper={onOpenPaper} />
      </div>
      <CitationList title="Cites — elsewhere" items={d.citesElsewhere} emptyText={p.inLibrary ? 'No references were read from this paper.' : 'Only library papers have their references read.'} onOpenPaper={onOpenPaper} />

      {p.inLibrary && (
        <section aria-labelledby={`${headingId}-results`} className="flex flex-col gap-2">
          <h3 id={`${headingId}-results`} className={SECTION_TITLE}>{`Results (${d.results.length})`}</h3>
          {d.results.length === 0 ? (
            <p className="text-[12.5px] text-shodh-text-muted">No accepted results. Extract them from the paper’s tables in its Results view.</p>
          ) : (
            <div className="overflow-x-auto rounded-xl border border-shodh-border">
              <table className="min-w-full border-collapse text-[12.5px]">
                <caption className="sr-only">Results read from the paper’s tables; each value opens its cell.</caption>
                <thead className="bg-shodh-raised">
                  <tr>
                    {['Method', 'Dataset', 'Metric', 'Value'].map(h => (
                      <th key={h} scope="col" className="px-3 py-1.5 text-left font-semibold text-shodh-text border-b border-shodh-border">{h}</th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {d.results.map(r => (
                    <tr key={r.id}>
                      <th scope="row" className="px-3 py-1.5 text-left font-medium text-shodh-text border-b border-shodh-border-subtle">{r.method}</th>
                      <td className="px-3 py-1.5 border-b border-shodh-border-subtle text-shodh-text-secondary">{r.dataset}</td>
                      <td className="px-3 py-1.5 border-b border-shodh-border-subtle text-shodh-text-secondary">{r.metric}</td>
                      <td className="px-3 py-1.5 border-b border-shodh-border-subtle">
                        <button
                          type="button"
                          className={cn('tabular-nums text-shodh-text underline decoration-dotted underline-offset-2 rounded', FOCUS_RING)}
                          title={`Show the cell on page ${r.page}`}
                          onClick={() => showSourceBox({ filePath: r.filePath, fileName: r.fileName, page: r.page, regions: r.region ? [r.region] : null, label: `${r.method} · ${r.metric}` })}
                        >
                          {valueLabel(r)}
                        </button>
                        {r.setting && <span className="ml-1.5 text-[11px] text-shodh-text-muted">{r.setting}</span>}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
      )}

      {snippets.length > 0 && (
        <section aria-labelledby={`${headingId}-snippets`} className="flex flex-col gap-2">
          <h3 id={`${headingId}-snippets`} className={SECTION_TITLE}>{`Snippets (${snippets.length})`}</h3>
          <ul className="grid gap-3 grid-cols-[repeat(auto-fill,minmax(220px,1fr))]">
            {snippets.map(s => (
              <li key={s.id}>
                <SnippetCard snippet={s} showFile={false} onChange={() => setTick(t => t + 1)} />
              </li>
            ))}
          </ul>
        </section>
      )}

      {d.related.length > 0 && (
        <section aria-labelledby={`${headingId}-related`} className="flex flex-col gap-2">
          <h3 id={`${headingId}-related`} className={SECTION_TITLE}>Related papers (shared references)</h3>
          <ul className="flex flex-col divide-y divide-shodh-border-subtle rounded-xl border border-shodh-border">
            {d.related.map(r => (
              <li key={r.paper.id}>
                <button type="button" onClick={() => onOpenPaper(r.paper.id)} className={cn('w-full text-left px-3 py-2 hover:bg-shodh-raised', FOCUS_RING)}>
                  <span className="block text-[13px] text-shodh-text truncate">{paperTitle(r.paper)}</span>
                  <span className="block text-[11.5px] text-shodh-text-muted">{`${r.count} shared ${r.count === 1 ? 'reference' : 'references'}`}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </article>
  );
}
