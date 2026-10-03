import React, { useRef } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { cn } from '../../lib/utils';
import { FOCUS_RING } from './fields';

/**
 * Small confirmation (alertdialog). Cancel has initial focus; Esc cancels.
 * Cancelling returns focus to the opener; after confirming, the caller
 * places focus (the confirmed action usually removes the opener).
 */
export default function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirmed = useRef(false);
  return (
    <Dialog.Root open={open} onOpenChange={next => { if (!next) onCancel(); }}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 data-[state=open]:animate-in data-[state=open]:fade-in-0 duration-panel" />
        <Dialog.Content
          role="alertdialog"
          onCloseAutoFocus={e => {
            if (confirmed.current) e.preventDefault();
            confirmed.current = false;
          }}
          className="fixed z-50 left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 w-[min(380px,calc(100vw-32px))] rounded-2xl border border-shodh-border-strong bg-shodh-surface p-5 shadow-[0_20px_60px_rgba(0,0,0,0.4)] focus:outline-none data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 duration-panel ease-enter"
        >
          <Dialog.Title className="text-[15px] font-semibold text-shodh-text">{title}</Dialog.Title>
          <Dialog.Description className="mt-1.5 text-[13px] text-shodh-text-secondary break-words">{description}</Dialog.Description>
          <div className="mt-5 flex justify-end gap-2">
            <Dialog.Close
              className={cn('h-8 px-3 rounded-lg border border-shodh-border text-[12.5px] text-shodh-text hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
            >
              Cancel
            </Dialog.Close>
            <button
              type="button"
              onClick={() => { confirmed.current = true; onConfirm(); }}
              className={cn('h-8 px-3 rounded-lg bg-destructive text-destructive-foreground text-[12.5px] font-medium hover:bg-destructive/90 transition-colors duration-micro', FOCUS_RING)}
            >
              {confirmLabel}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
