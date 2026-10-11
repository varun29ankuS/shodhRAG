import React, { useEffect, useRef, useState } from 'react';
import { AlertTriangle, FileText, Loader2, MessageSquare, MessageSquareText } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useChatSession } from '../ask/ChatSessionContext';
import { useFocus } from '../focus/FocusContext';
import { figureTarget } from '../focus/targets';
import { FigureImage } from './FigureImage';
import { figureRegion } from './paperObjects';
import type { PaperFigure, PaperParts } from './paperObjects';
import { figureError, paperParts } from './paperObjectsApi';
import { insertIntoComposer, showSourceBox } from './snippetBus';
import { BUTTON } from './ui';

type Load = { status: 'loading' } | { status: 'ready'; parts: PaperParts } | { status: 'error'; message: string };

/** True once the element has come near the viewport (figures are drawn lazily). */
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

function FigureCard({ figure, parts, paperTitle }: { figure: PaperFigure; parts: PaperParts; paperTitle: string }) {
  const focus = useFocus();
  const { activeConversationId } = useChatSession();
  const ref = useRef<HTMLLIElement>(null);
  const near = useNearViewport(ref);
  const canDiscuss = Boolean(focus && activeConversationId);

  const discuss = (trigger: HTMLElement) => {
    const target = figureTarget({
      filePath: parts.filePath,
      fileName: parts.fileName,
      page: figure.page,
      bbox: figure.bbox,
      figureId: figure.id,
      caption: figure.caption,
      label: figure.label,
      mentions: figure.mentions,
    });
    if (!focus || !target) return;
    focus.openFocus({ target, parentMessageId: null, trigger });
  };

  return (
    <li ref={ref} className="flex flex-col rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
      <div className="bg-white p-2 flex items-center justify-center min-h-[140px]">
        {near ? (
          <FigureImage filePath={parts.filePath} page={figure.page} bbox={figure.bbox} alt={figure.caption || figure.label} size="thumb" className="max-h-[220px] w-auto" />
        ) : (
          <span className="sr-only">{`${figure.label} is drawn when scrolled into view`}</span>
        )}
      </div>
      <div className="flex-1 flex flex-col gap-1.5 px-3 py-2 border-t border-shodh-border-subtle">
        <p className="m-0 text-[12.5px] font-semibold text-shodh-text">
          {figure.label}
          <span className="font-normal text-shodh-text-muted">{` · page ${figure.page}`}</span>
        </p>
        <p className="m-0 text-[12.5px] leading-snug text-shodh-text-secondary line-clamp-3" title={figure.caption}>
          {figure.caption}
        </p>
        {!figure.regionFound && (
          <p className="m-0 text-[11.5px] text-shodh-text-muted">Only the caption could be located on the page.</p>
        )}
        <div className="mt-auto flex flex-wrap gap-1.5 pt-1">
          <button
            type="button"
            className={BUTTON}
            onClick={() =>
              showSourceBox({
                filePath: parts.filePath,
                fileName: parts.fileName,
                page: figure.page,
                regions: [figureRegion(figure.page, figure.bbox)],
                label: `${figure.label} · ${paperTitle}`,
              })
            }
            aria-label={`Show ${figure.label} in the PDF, page ${figure.page}`}
          >
            <FileText className="w-3.5 h-3.5" aria-hidden="true" />
            Show in PDF
          </button>
          {canDiscuss ? (
            <button type="button" className={BUTTON} onClick={e => discuss(e.currentTarget)} aria-label={`Expand ${figure.label}: zoom and ask about it`}>
              <MessageSquareText className="w-3.5 h-3.5" aria-hidden="true" />
              Expand &amp; ask
            </button>
          ) : (
            <button
              type="button"
              className={BUTTON}
              onClick={() => insertIntoComposer(`Explain ${figure.label} of "${paperTitle}" (${parts.filePath}): `)}
            >
              <MessageSquare className="w-3.5 h-3.5" aria-hidden="true" />
              Ask about it
            </button>
          )}
        </div>
      </div>
    </li>
  );
}

/** The paper page's Figures tab: every detected figure, drawn from the PDF. */
export function PaperFigures({ filePath, paperTitle }: { filePath: string; paperTitle: string }) {
  const [load, setLoad] = useState<Load>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    setLoad({ status: 'loading' });
    paperParts(filePath)
      .then(parts => !cancelled && setLoad({ status: 'ready', parts }))
      .catch(error => !cancelled && setLoad({ status: 'error', message: figureError(error) }));
    return () => {
      cancelled = true;
    };
  }, [filePath]);

  if (load.status === 'loading') {
    return (
      <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        Reading the paper’s figures…
      </p>
    );
  }
  if (load.status === 'error') {
    return (
      <p role="alert" className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
        <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        {load.message}
      </p>
    );
  }
  const { parts } = load;
  if (parts.figures.length === 0) {
    return <p className="text-[12.5px] text-shodh-text-muted">No figure captions (“Figure 1: …”) were found in this paper.</p>;
  }
  return (
    <ul className={cn('m-0 p-0 grid gap-3 grid-cols-[repeat(auto-fill,minmax(240px,1fr))]')} aria-label={`Figures of ${paperTitle}`}>
      {parts.figures.map(f => (
        <FigureCard key={f.id} figure={f} parts={parts} paperTitle={paperTitle} />
      ))}
    </ul>
  );
}
