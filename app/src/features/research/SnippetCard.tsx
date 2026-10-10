import React, { useId, useState } from 'react';
import { AlertTriangle, GripVertical, Loader2, Scissors, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { removeWithUndo } from '../../lib/undoToast';
import { relativeTime } from '../../utils/time';
import { researchApi, toResearchError } from './api';
import { openSnippet } from './snippetBus';
import { startSnippetDrag } from './SnippetDropZone';
import { KIND_LABEL, snippetLabel } from './snippetModel';
import { pngDataUrl } from './snippetRender';
import type { Snippet } from './types';
import { FOCUS_RING } from './ui';
import { useSnippetImage } from './useSnippets';

/** The image of a snippet in a card (drawn from the PDF the first time when not stored). */
export function SnippetThumb({ snippet, className }: { snippet: Snippet; className?: string }) {
  const image = useSnippetImage(snippet);
  return (
    <div className={cn('h-[132px] bg-white flex items-center justify-center overflow-hidden', className)}>
      {image.status === 'ready' ? (
        <img src={pngDataUrl(image.png)} alt="" className="max-w-full max-h-full object-contain" draggable={false} />
      ) : image.status === 'loading' ? (
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none text-zinc-500" aria-hidden="true" />
      ) : (
        <span className="flex items-center gap-1.5 px-3 text-[11.5px] text-zinc-600" title={image.message}>
          <AlertTriangle className="w-3.5 h-3.5 text-shodh-warning" aria-hidden="true" />
          Not drawn
        </span>
      )}
    </div>
  );
}

function dateLabel(iso: string): string {
  const time = Date.parse(iso);
  return Number.isFinite(time) ? new Date(time).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' }) : '';
}

/**
 * A snippet card: opens the snippet, drags onto a composer as context, and
 * deletes it (with an undo window).
 */
export function SnippetCard({ snippet, showFile = true, onChange }: { snippet: Snippet; showFile?: boolean; onChange: (next: Snippet | null) => void }) {
  const titleId = useId();
  const [hidden, setHidden] = useState(false);
  const label = snippetLabel(snippet);

  // Hidden at once and deleted when the undo window ends; Undo shows it again.
  const remove = () => {
    const id = snippet.id;
    removeWithUndo({
      message: 'Snippet deleted',
      description: label,
      hide: () => setHidden(true),
      restore: () => setHidden(false),
      commit: async () => {
        await researchApi.deleteSnippet(id);
        onChange(null);
      },
      onError: error => notify.error('The snippet could not be deleted', { description: toResearchError(error).message }),
    });
  };

  if (hidden) return null;

  return (
    <li className="relative">
      <article
        aria-labelledby={titleId}
        draggable
        onDragStart={e => startSnippetDrag(e, snippet)}
        className="group relative flex flex-col rounded-[14px] border border-shodh-border bg-shodh-surface overflow-hidden hover:border-shodh-border-strong transition-colors duration-micro"
      >
        <button
          type="button"
          onClick={() => openSnippet(snippet)}
          aria-label={`Open snippet ${label}`}
          className={cn('text-left flex flex-col', FOCUS_RING, 'focus-visible:ring-offset-0 rounded-[14px]')}
        >
          <SnippetThumb snippet={snippet} className="border-b border-shodh-border-subtle" />
          <div className="px-3 pt-2.5 pb-2.5 flex flex-col gap-0.5 min-w-0">
            <h3 id={titleId} className="text-[13px] font-semibold text-shodh-text truncate" title={label}>
              <Scissors className="inline w-3 h-3 mr-1 -mt-0.5 text-shodh-text-muted" aria-hidden="true" />
              {label}
            </h3>
            <p className="text-[11.5px] text-shodh-text-muted truncate">
              <span>{KIND_LABEL[snippet.kind]}</span>
              <span>{showFile ? ` · ${snippet.fileName} p.${snippet.page}` : ` · page ${snippet.page}`}</span>
              <span>{' · '}</span>
              <time dateTime={snippet.createdAt} title={relativeTime(snippet.updatedAt)}>{dateLabel(snippet.createdAt)}</time>
            </p>
            {snippet.note && <p className="text-[11.5px] text-shodh-text-secondary line-clamp-2">{snippet.note}</p>}
          </div>
        </button>
        <span
          className="absolute left-2 top-2 w-6 h-6 inline-flex items-center justify-center rounded-md bg-shodh-surface/90 border border-shodh-border text-shodh-text-muted opacity-0 group-hover:opacity-100 transition-opacity duration-micro pointer-events-none"
          title="Drag onto the chat to add as context"
          aria-hidden="true"
        >
          <GripVertical className="w-3.5 h-3.5" />
        </span>
        <button
          type="button"
          aria-label={`Delete snippet ${label}`}
          title="Delete (you can undo)"
          onClick={remove}
          className={cn(
            'absolute right-2 top-2 w-7 h-7 inline-flex items-center justify-center rounded-lg bg-shodh-surface/90 border border-shodh-border text-shodh-text-muted hover:text-shodh-error',
            'opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 transition-opacity duration-micro',
            FOCUS_RING,
          )}
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
        </button>
      </article>
    </li>
  );
}
