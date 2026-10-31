import React, { Suspense, lazy, useEffect, useId, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { Images, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { isInFocusOverlay } from '../focus/focusDom';
import { onVisualsChanged, visualsApi } from './api';
import { subscribeReveal } from './reveal';

// The gallery (thumbnails render diagrams, charts and equations) loads when it is first opened.
const VisualGallery = lazy(() => import('./VisualGallery').then(m => ({ default: m.VisualGallery })));

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/** Whether an event target is inside a notice (e.g. its Undo button). */
function isInToast(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('[data-sonner-toaster]') !== null;
}

/** How many visuals a conversation has; follows changes. Null until known. */
export function useVisualCount(conversationId: string | null): number | null {
  const [count, setCount] = useState<number | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!conversationId) {
      setCount(null);
      return;
    }
    let cancelled = false;
    visualsApi.count(conversationId)
      .then(n => { if (!cancelled) setCount(n); })
      .catch(() => { if (!cancelled) setCount(null); });
    return () => { cancelled = true; };
  }, [conversationId, tick]);
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onVisualsChanged(changed => {
      if (changed === null || changed === conversationId) setTick(t => t + 1);
    }).then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [conversationId]);
  return count;
}

interface VisualsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  conversationId: string;
  conversationTitle: string;
  /** Where focus goes when the gallery closes (the control that opened it). */
  onCloseAutoFocus?: (event: Event) => void;
}

/**
 * The gallery of one conversation as a centred dialog, opened from the
 * Ask view's "More chat actions" menu. A card opens the focus pop-out above
 * it; closing the pop-out returns to the gallery.
 */
export function VisualsDialog({ open, onOpenChange, conversationId, conversationTitle, onCloseAutoFocus }: VisualsDialogProps) {
  const descriptionId = useId();

  // "Go to message" leaves the gallery for the answer.
  useEffect(() => subscribeReveal(() => onOpenChange(false)), [onOpenChange]);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[55] bg-black/40" />
        <Dialog.Content
          aria-describedby={descriptionId}
          onCloseAutoFocus={onCloseAutoFocus}
          // The focus pop-out opens above the gallery; using it must not close the gallery.
          onInteractOutside={e => { if (isInFocusOverlay(e.target) || isInToast(e.target)) e.preventDefault(); }}
          // Menus inside the gallery close themselves on Esc.
          onEscapeKeyDown={e => {
            if (e.target instanceof Element && e.target.closest('[data-esc-local]')) e.preventDefault();
          }}
          className="ask-fade-in fixed z-[55] inset-0 m-auto flex flex-col w-[min(1100px,calc(100vw-48px))] h-[min(820px,calc(100vh-48px))] rounded-[18px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_24px_80px_rgba(0,0,0,0.45)] focus:outline-none"
        >
          <header className="shrink-0 flex items-center gap-3 px-5 py-3 border-b border-shodh-border-subtle">
            <Images className="w-4 h-4 text-shodh-text-muted" aria-hidden="true" />
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-body font-semibold truncate">{`Visuals in “${conversationTitle || 'this conversation'}”`}</Dialog.Title>
              <Dialog.Description id={descriptionId} className="text-caption text-shodh-text-muted">
                Open one to explore, ask about or refine it. All conversations’ visuals are in Library.
              </Dialog.Description>
            </div>
            <Dialog.Close
              aria-label="Close"
              className={cn('w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
            >
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-5 py-4">
            {open && (
              <Suspense fallback={<p className="m-0 text-small text-shodh-text-muted" role="status">Loading visuals…</p>}>
                <VisualGallery conversationId={conversationId} autoFocusSearch emptyText="No visuals in this conversation yet. Diagrams, charts, sketches, plots, simulations, equations and tables in its answers appear here." />
              </Suspense>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
