import React, { useEffect, useId, useState } from 'react';
import { AlertTriangle, ArrowLeft, Loader2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { toResearchError } from './api';
import { CompareResults } from './CompareResults';
import { graphApi } from './graphApi';
import type { ConceptView, PaperNode } from './graphTypes';
import { BUTTON, FOCUS_RING, SECTION_TITLE } from './ui';

type Load<T> = { status: 'loading' } | { status: 'ready'; value: T } | { status: 'error'; message: string };

function title(p: PaperNode): string {
  return p.title ?? (p.filePath ? p.filePath.split(/[\\/]/).pop() ?? p.filePath : p.id);
}

/**
 * A method or dataset page: the papers using it, the results reported for it
 * across papers (comparison table and chart, every value opening its cell)
 * and, for a method, the library paper that proposed it when its title or
 * abstract says so (otherwise nothing is claimed).
 */
export function ConceptPage({
  kind,
  conceptId,
  onBack,
  onOpenPaper,
}: {
  kind: 'method' | 'dataset';
  conceptId: string;
  onBack: () => void;
  onOpenPaper: (id: string) => void;
}) {
  const headingId = useId();
  const [view, setView] = useState<Load<ConceptView>>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    setView({ status: 'loading' });
    graphApi.concept(kind, conceptId).then(
      value => !cancelled && setView({ status: 'ready', value }),
      error => !cancelled && setView({ status: 'error', message: toResearchError(error).message }),
    );
    return () => {
      cancelled = true;
    };
  }, [kind, conceptId]);

  const back = (
    <button type="button" onClick={onBack} className={cn(BUTTON, 'self-start')}>
      <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
      Back
    </button>
  );
  if (view.status !== 'ready') {
    return (
      <div className="flex flex-col gap-3">
        {back}
        {view.status === 'loading' ? (
          <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Loading…
          </p>
        ) : (
          <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
            <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
            {view.message}
          </p>
        )}
      </div>
    );
  }
  const v = view.value;
  return (
    <article aria-labelledby={headingId} className="flex flex-col gap-5">
      {back}
      <header className="flex flex-col gap-1">
        <p className={SECTION_TITLE}>{kind === 'method' ? 'Method' : 'Dataset'}</p>
        <h2 id={headingId} className="m-0 text-[19px] font-semibold text-shodh-text">{v.concept.label}</h2>
        {v.proposedIn && (
          <p className="text-[12.5px] text-shodh-text-secondary">
            First proposed in{' '}
            <button type="button" onClick={() => v.proposedIn && onOpenPaper(v.proposedIn.id)} className={cn('underline underline-offset-2 text-shodh-text rounded', FOCUS_RING)}>
              {title(v.proposedIn)}
            </button>
            {v.proposedIn.year ? ` (${v.proposedIn.year})` : ''}
          </p>
        )}
      </header>
      <section aria-labelledby={`${headingId}-papers`} className="flex flex-col gap-2">
        <h3 id={`${headingId}-papers`} className={SECTION_TITLE}>{`Papers ${kind === 'method' ? 'using it' : 'evaluated on it'} (${v.papers.length})`}</h3>
        <ul className="flex flex-col divide-y divide-shodh-border-subtle rounded-xl border border-shodh-border">
          {v.papers.map(p => (
            <li key={p.id}>
              <button type="button" onClick={() => onOpenPaper(p.id)} className={cn('w-full text-left px-3 py-2 hover:bg-shodh-raised', FOCUS_RING)}>
                <span className="block text-[13px] text-shodh-text truncate">{title(p)}</span>
                <span className="block text-[11.5px] text-shodh-text-muted">{[p.year, p.inLibrary ? 'in your library' : null].filter(Boolean).join(' · ')}</span>
              </button>
            </li>
          ))}
        </ul>
      </section>
      <section aria-labelledby={`${headingId}-results`} className="flex flex-col gap-2">
        <h3 id={`${headingId}-results`} className={SECTION_TITLE}>Reported results across papers</h3>
        <CompareResults key={v.concept.id} initialMethod={kind === 'method' ? v.concept.id : ''} initialDataset={kind === 'dataset' ? v.concept.id : ''} />
      </section>
    </article>
  );
}
