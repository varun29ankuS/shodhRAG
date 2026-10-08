import React, { Suspense, lazy, useEffect, useId, useMemo, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { FileText, Scissors, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useChatSession } from '../ask/ChatSessionContext';
import { useFocus } from '../focus/FocusContext';
import { isInFocusOverlay } from '../focus/focusDom';
import type { FileHighlight } from '../library/FileViewer';
import { OPEN_SNIPPET_EVENT, SHOW_SOURCE_EVENT, onWindowEvent } from './snippetBus';
import type { SourceBoxRequest } from './snippetBus';
import { snippetLabel, snippetTarget } from './snippetModel';
import type { Snippet } from './types';
import { FOCUS_RING } from './ui';

// The snippet card (KaTeX, tables) and the file viewer (pdf.js) load when a dialog first opens.
const SnippetDetail = lazy(() => import('./SnippetDetail').then(m => ({ default: m.SnippetDetail })));
const FileViewer = lazy(() => import('../library/FileViewer').then(m => ({ default: m.FileViewer })));

function DialogLoading() {
  return <p className="m-0 text-[13px] text-shodh-text-muted" role="status">Loading…</p>;
}

function isInToast(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('[data-sonner-toaster]') !== null;
}

const DIALOG_CLASS =
  'ask-fade-in fixed inset-0 m-auto flex flex-col w-[min(1100px,calc(100vw-48px))] h-[min(860px,calc(100vh-48px))] rounded-[18px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_24px_80px_rgba(0,0,0,0.45)] focus:outline-none';

/**
 * Answers app-wide snippet requests: a snippet opens in the focus pop-out
 * when a conversation is open (so it can be discussed), else in a dialog;
 * "show the source box" opens the paper at the page with the box outlined.
 * Mounted once inside the focus provider.
 */
export function SnippetHost() {
  const focus = useFocus();
  const { activeConversationId } = useChatSession();
  const [dialogSnippet, setDialogSnippet] = useState<Snippet | null>(null);
  const [source, setSource] = useState<SourceBoxRequest | null>(null);

  useEffect(
    () =>
      onWindowEvent<Snippet>(OPEN_SNIPPET_EVENT, snippet => {
        if (!snippet) return;
        if (focus && activeConversationId) {
          focus.openFocus({
            target: snippetTarget(snippet),
            parentMessageId: null,
            trigger: document.activeElement instanceof HTMLElement ? document.activeElement : null,
          });
        } else {
          setDialogSnippet(snippet);
        }
      }),
    [focus, activeConversationId],
  );

  // Views showing the file themselves claim the request synchronously; the
  // dialog opens only for unclaimed ones.
  useEffect(
    () =>
      onWindowEvent<SourceBoxRequest>(SHOW_SOURCE_EVENT, request => {
        if (!request) return;
        window.setTimeout(() => {
          if (!request.claimed) setSource(request);
        }, 0);
      }),
    [],
  );

  return (
    <>
      <SnippetDialog snippet={dialogSnippet} onClose={() => setDialogSnippet(null)} onChanged={next => setDialogSnippet(next)} />
      <SourceBoxDialog request={source} onClose={() => setSource(null)} />
    </>
  );
}

function SnippetDialog({ snippet, onClose, onChanged }: { snippet: Snippet | null; onClose: () => void; onChanged: (next: Snippet | null) => void }) {
  const descriptionId = useId();
  return (
    <Dialog.Root open={snippet !== null} onOpenChange={open => { if (!open) onClose(); }}>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[56] bg-black/40" />
        <Dialog.Content
          aria-describedby={descriptionId}
          onInteractOutside={e => { if (isInFocusOverlay(e.target) || isInToast(e.target)) e.preventDefault(); }}
          className={cn(DIALOG_CLASS, 'z-[56]')}
        >
          <header className="shrink-0 flex items-center gap-3 px-5 py-3 border-b border-shodh-border-subtle">
            <Scissors className="w-4 h-4 text-shodh-text-muted" aria-hidden="true" />
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-[14px] font-semibold truncate">{snippet ? snippetLabel(snippet) : 'Snippet'}</Dialog.Title>
              <Dialog.Description id={descriptionId} className="text-[12px] text-shodh-text-muted">
                Open a conversation in Ask to discuss snippets in the focus view.
              </Dialog.Description>
            </div>
            <Dialog.Close aria-label="Close" className={cn('w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}>
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-6 py-5 flex justify-center bg-shodh-raised-2">
            {snippet && (
              <Suspense fallback={<DialogLoading />}>
                <SnippetDetail
                  key={snippet.id}
                  snippet={snippet}
                  mode="dialog"
                  onChanged={next => {
                    onChanged(next);
                  }}
                />
              </Suspense>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** A paper open at a page with a box outlined (a result cell, a snippet). */
function SourceBoxDialog({ request, onClose }: { request: SourceBoxRequest | null; onClose: () => void }) {
  const descriptionId = useId();
  const highlight = useMemo<FileHighlight | null>(
    () => (request ? { page: request.page, regions: request.regions ?? null, rects: request.rects ?? null } : null),
    [request],
  );
  return (
    <Dialog.Root open={request !== null} onOpenChange={open => { if (!open) onClose(); }}>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[66] bg-black/40" />
        <Dialog.Content
          aria-describedby={descriptionId}
          onInteractOutside={e => { if (isInToast(e.target)) e.preventDefault(); }}
          className={cn(DIALOG_CLASS, 'z-[66]')}
        >
          <header className="shrink-0 flex items-center gap-3 px-5 py-3 border-b border-shodh-border-subtle">
            <FileText className="w-4 h-4 text-shodh-text-muted" aria-hidden="true" />
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-[14px] font-semibold truncate">{request?.fileName || request?.filePath || 'Source'}</Dialog.Title>
              <Dialog.Description id={descriptionId} className="text-[12px] text-shodh-text-muted truncate">
                {request ? `Page ${request.page}${request.label ? ` · ${request.label}` : ''} · the source box is outlined` : ''}
              </Dialog.Description>
            </div>
            <Dialog.Close aria-label="Close" className={cn('w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}>
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <div className="flex-1 min-h-0 flex flex-col">
            {request && (
              <Suspense fallback={<DialogLoading />}>
                <FileViewer key={`${request.filePath}:${request.page}`} path={request.filePath} highlight={highlight} />
              </Suspense>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
