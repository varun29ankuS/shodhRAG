import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import type React from 'react';
import { Download, Images, MoreHorizontal } from 'lucide-react';
import { cn } from '../../lib/utils';
import { VisualsDialog, useVisualCount } from '../visuals/GalleryDialog';
import { chatTokenUsage, tokenUsageLine } from './tokenUsage';
import type { ChatMessage } from './types';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const ITEM =
  'w-full flex items-center gap-2 px-3 py-1.5 text-small text-left text-shodh-text-secondary hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none';

interface ChatActionsMenuProps {
  conversationId: string;
  conversationTitle: string;
  messages: readonly ChatMessage[];
  /** Export the conversation as Markdown; null while it has no messages. */
  onExport: (() => void) | null;
}

/**
 * "⋯" in the Ask view's top bar: Export and Visuals, and the tokens the
 * conversation's answers reported using.
 */
export function ChatActionsMenu({ conversationId, conversationTitle, messages, onExport }: ChatActionsMenuProps) {
  const [open, setOpen] = useState(false);
  const [visualsOpen, setVisualsOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const visualCount = useVisualCount(conversationId);
  const usageLine = useMemo(() => tokenUsageLine(chatTokenUsage(messages)), [messages]);

  useEffect(() => {
    if (!open) return;
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    const onPointer = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  const close = (restore: boolean) => {
    setOpen(false);
    if (restore) buttonRef.current?.focus();
  };

  const onMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const buttons = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      buttons[(index + 1) % buttons.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      buttons[e.key === 'Home' ? 0 : buttons.length - 1]?.focus();
    } else if (e.key === 'Tab') {
      close(false);
    }
  };

  const onVisualsOpenChange = useCallback((next: boolean) => setVisualsOpen(next), []);

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-label="More chat actions"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        title="More chat actions"
        className={cn(
          'h-6 min-w-[24px] px-1.5 rounded-full inline-flex items-center justify-center bg-shodh-raised text-shodh-text-muted hover:text-shodh-text transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <MoreHorizontal className="w-3.5 h-3.5" aria-hidden="true" />
      </button>
      {open && (
        <div className="shell-pop absolute right-0 top-7 z-50 min-w-[220px] rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg overflow-hidden">
          <div id={menuId} ref={menuRef} role="menu" aria-label="More chat actions" onKeyDown={onMenuKeyDown} className="py-1">
            {onExport && (
              <button type="button" role="menuitem" onClick={() => { close(true); onExport(); }} className={ITEM}>
                <Download className="w-3.5 h-3.5" aria-hidden="true" />
                Export as Markdown
              </button>
            )}
            <button
              type="button"
              role="menuitem"
              onClick={() => { close(false); setVisualsOpen(true); }}
              className={ITEM}
              title="Diagrams, charts, sketches, plots, simulations, equations and tables from this conversation"
            >
              <Images className="w-3.5 h-3.5" aria-hidden="true" />
              {visualCount === null ? 'Visuals' : `Visuals (${visualCount})`}
            </button>
          </div>
          {usageLine && (
            <p className="m-0 px-3 py-2 border-t border-shodh-border-subtle text-caption text-shodh-text-muted tabular-nums">
              {usageLine}
            </p>
          )}
        </div>
      )}
      <VisualsDialog
        open={visualsOpen}
        onOpenChange={onVisualsOpenChange}
        conversationId={conversationId}
        conversationTitle={conversationTitle}
        onCloseAutoFocus={e => {
          e.preventDefault();
          buttonRef.current?.focus();
        }}
      />
    </div>
  );
}

export default ChatActionsMenu;
