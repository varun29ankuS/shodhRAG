import React, { useEffect, useId, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { Images, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { isInFocusOverlay } from '../focus/focusDom';
import { onVisualsChanged, visualsApi } from './api';
import { subscribeReveal } from './reveal';
import { VisualGallery } from './VisualGallery';

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

/**
 * "Visuals (N)" in the conversation header: opens the gallery of this
 * conversation as a centred dialog. A card opens the focus pop-out above it;
 * closing the pop-out returns to the gallery.
 */
export function VisualsButton({ conversationId, conversationTitle }: { conversationId: string; conversationTitle: string }) {
  const [open, setOpen] = useState(false);
  const count = useVisualCount(conversationId);
  const descriptionId = useId();

  // "Go to message" leaves the gallery for the answer.
  useEffect(() => subscribeReveal(() => setOpen(false)), []);

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Trigger
        className={cn(
          'text-[10px] px-2 py-0.5 rounded-full font-medium inline-flex items-center gap-1 bg-shodh-raised text-shodh-text-muted hover:text-shodh-text transition-colors duration-micro',
          FOCUS_RING,
        )}
        title="Diagrams, charts, sketches, plots, simulations, equations and tables from this conversation"
      >
        <Images className="w-3 h-3" aria-hidden="true" />
        {count === null ? 'Visuals' : `Visuals (${count})`}
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[55] bg-black/40" />
        <Dialog.Content
          aria-describedby={descriptionId}
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
              <Dialog.Title className="text-[14px] font-semibold truncate">{`Visuals in “${conversationTitle || 'this conversation'}”`}</Dialog.Title>
              <Dialog.Description id={descriptionId} className="text-[12px] text-shodh-text-muted">
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
            {open && <VisualGallery conversationId={conversationId} autoFocusSearch emptyText="No visuals in this conversation yet. Diagrams, charts, sketches, plots, simulations, equations and tables in its answers appear here." />}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
