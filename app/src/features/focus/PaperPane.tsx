import React, { useMemo } from 'react';
import { FileText, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useSourceDocument } from '../ask/useSourceDocument';
import type { SearchHit } from '../ask/types';
import type { PaperRef } from './targets';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/** A document place as the source viewer takes it: the passage on its page. */
export function paperHit(paper: PaperRef): SearchHit {
  return {
    number: 0,
    sourceFile: paper.sourceFile,
    fileName: paper.fileName,
    title: paper.fileName,
    text: paper.passage,
    snippet: paper.passage,
    score: 0,
    page: paper.page !== null ? { start: paper.page, end: paper.page } : null,
    lineRange: null,
    url: null,
  };
}

/**
 * "Show in paper": the document an object (or an outer level) came from,
 * open at its page with the passage highlighted, beside the focused object.
 */
export function PaperPane({ paper, onClose }: { paper: PaperRef; onClose: () => void }) {
  const hit = useMemo(() => paperHit(paper), [paper]);
  const doc = useSourceDocument(hit);
  const name = doc.info?.fileName ?? paper.fileName ?? paper.sourceFile;
  return (
    <section aria-label={`Source document: ${name}`} className="flex-1 min-w-0 min-h-0 flex flex-col border-l border-shodh-border-subtle bg-shodh-surface">
      <header className="shrink-0 flex items-center gap-2 pl-3 pr-1.5 h-9 border-b border-shodh-border-subtle text-[12.5px] text-shodh-text-secondary">
        <FileText className="w-3.5 h-3.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span className="truncate" title={name}>{name}</span>
        {paper.page !== null && <span className="shrink-0 text-shodh-text-muted">{`page ${paper.page}`}</span>}
        <button
          type="button"
          onClick={onClose}
          aria-label="Hide the source document"
          title="Hide the source document"
          className={cn('ml-auto w-7 h-7 shrink-0 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro', FOCUS_RING)}
        >
          <X className="w-3.5 h-3.5" aria-hidden="true" />
        </button>
      </header>
      <div className="flex-1 min-h-0 min-w-0 flex flex-col">{doc.body}</div>
    </section>
  );
}
