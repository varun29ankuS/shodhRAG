import React from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { FOCUS_RING } from './fields';

/** Controls whose own Esc handling cancels an edit in progress. */
const EDITABLE = 'input, textarea, select, [contenteditable="true"]';

/**
 * Side sheet for a task or event: a modal dialog (focus trapped, Esc closes,
 * focus returns to the opener), sliding in from the right with the panel
 * motion tokens. Esc pressed inside a text or date field only cancels that
 * field's edit; the next Esc closes the sheet.
 */
export default function DetailSheet({
  open,
  onClose,
  title,
  kindLabel,
  children,
  footer,
}: {
  open: boolean;
  onClose: () => void;
  /** Accessible name of the dialog. */
  title: string;
  /** Visible eyebrow, e.g. "Task". */
  kindLabel: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={next => { if (!next) onClose(); }}>
      <Dialog.Portal>
        <Dialog.Overlay
          className="fixed inset-0 z-50 bg-black/30 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 duration-panel"
        />
        <Dialog.Content
          aria-describedby={undefined}
          onEscapeKeyDown={e => {
            const target = e.target instanceof Element ? e.target : null;
            if (target?.closest(EDITABLE)) e.preventDefault();
          }}
          className={cn(
            'fixed z-50 top-2 right-2 bottom-2 w-[min(460px,calc(100vw-16px))] flex flex-col overflow-hidden',
            'rounded-[18px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[-20px_0_60px_rgba(0,0,0,0.35)] focus:outline-none',
            'data-[state=open]:animate-in data-[state=open]:slide-in-from-right-8 data-[state=open]:fade-in-0 data-[state=open]:duration-screen data-[state=open]:ease-enter',
            'data-[state=closed]:animate-out data-[state=closed]:slide-out-to-right-8 data-[state=closed]:fade-out-0 data-[state=closed]:duration-exit data-[state=closed]:ease-exit',
          )}
        >
          <header className="flex items-center gap-2 pl-5 pr-3 pt-3 pb-2 shrink-0">
            <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint mr-auto" aria-hidden="true">
              {kindLabel}
            </span>
            <Dialog.Title className="sr-only">{title}</Dialog.Title>
            <Dialog.Close
              aria-label="Close"
              className={cn(
                'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                FOCUS_RING,
              )}
            >
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-3 pb-4 flex flex-col gap-4">
            {children}
          </div>
          {footer && (
            <footer className="shrink-0 flex items-center gap-2 px-5 py-3 border-t border-shodh-border-subtle">
              {footer}
            </footer>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
