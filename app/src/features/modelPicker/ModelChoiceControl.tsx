import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type React from 'react';
import { createPortal } from 'react-dom';
import { Check, ListFilter } from 'lucide-react';
import { cn } from '../../lib/utils';
import { nextMenuIndex } from '../../lib/menuKeys';
import type { DockModelLabel } from '../../lib/dockProtocol';
import { ModelTrigger } from './ModelTrigger';

const ITEM =
  'w-full flex items-start gap-2 px-2.5 py-2 rounded-lg text-start hover:bg-shodh-raised focus-visible:outline-none focus-visible:bg-shodh-raised focus-visible:ring-2 focus-visible:ring-ring';

const PANEL_WIDTH = 300;
const EDGE = 8;

interface ModelChoiceControlProps {
  model: DockModelLabel;
  /** Why the model cannot change now (an answer is running), or null. */
  lockedReason: string | null;
  /** One of `model.choices`, by its id. */
  onChoose: (id: string) => void;
  /** The full list, which is Ask's. */
  onAll: () => void;
}

/**
 * The model control of the floating chat over the preview: the composer row's trigger, and
 * the same short menu as Ask's chip (the picks and the recent models), drawn from the rows
 * main sent. A pick is sent to main, which applies it and sends the new state back. "All
 * models" opens Ask's full list.
 */
export function ModelChoiceControl({ model, lockedReason, onChoose, onAll }: ModelChoiceControlProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const locked = lockedReason !== null;

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  }, []);

  // Locked while an answer runs: the menu closes.
  useEffect(() => {
    if (locked) setOpen(false);
  }, [locked]);

  useLayoutEffect(() => {
    if (!open) {
      setPos(null);
      return;
    }
    const place = () => {
      const anchor = triggerRef.current?.getBoundingClientRect();
      if (!anchor) return;
      const width = Math.min(PANEL_WIDTH, window.innerWidth - EDGE * 2);
      const left = Math.min(Math.max(EDGE, anchor.left), window.innerWidth - width - EDGE);
      setPos({ left, bottom: Math.max(EDGE, window.innerHeight - anchor.top + 6) });
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (panelRef.current?.contains(target) || triggerRef.current?.contains(target)) return;
      close(false);
    };
    window.addEventListener('pointerdown', onPointer, true);
    return () => window.removeEventListener('pointerdown', onPointer, true);
  }, [open, close]);

  // Focus the row that answers now (the first one otherwise) once the menu is placed.
  const placed = open && pos !== null;
  useEffect(() => {
    if (!placed) return;
    const items = panelRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"],[role="menuitem"]');
    const checked = panelRef.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]');
    (checked ?? items?.[0])?.focus();
  }, [placed]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
      return;
    }
    const items = Array.from(panelRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"],[role="menuitem"]') ?? []);
    const next = nextMenuIndex(e.key, items.indexOf(document.activeElement as HTMLButtonElement), items.length);
    if (next === null) return;
    e.preventDefault();
    items[next]?.focus();
  };

  return (
    <>
      <ModelTrigger
        ref={triggerRef}
        label={model.label}
        ready={model.ready}
        dot={model.dot}
        lockedReason={lockedReason}
        onClick={() => setOpen(o => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={`Model: ${model.label}. Change model`}
        title={model.label}
      />
      {open &&
        createPortal(
          <div
            ref={panelRef}
            id={panelId}
            role="menu"
            aria-label="Models"
            data-testid="dock-model-menu"
            onKeyDown={onKeyDown}
            className="shell-pop fixed z-50 flex flex-col p-1.5 overflow-y-auto scrollbar-thin rounded-xl border border-shodh-border-strong bg-shodh-surface shadow-lg"
            style={{
              left: pos?.left ?? EDGE,
              bottom: pos?.bottom ?? EDGE,
              width: `min(${PANEL_WIDTH}px, calc(100vw - ${EDGE * 2}px))`,
              maxHeight: `calc(100vh - ${(pos?.bottom ?? EDGE) + EDGE}px)`,
              visibility: pos ? 'visible' : 'hidden',
            }}
          >
            {model.choices.length === 0 && (
              <p className="px-2.5 py-2 m-0 text-small text-shodh-text-muted">No model connected</p>
            )}
            {model.choices.map(choice => (
              <button
                key={choice.id}
                type="button"
                role="menuitemradio"
                aria-checked={choice.selected}
                onClick={() => {
                  close(true);
                  if (!choice.selected) onChoose(choice.id);
                }}
                className={ITEM}
              >
                <Check className={cn('w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-accent-text', !choice.selected && 'invisible')} aria-hidden="true" />
                <span className="min-w-0 flex flex-col">
                  <span className="text-small font-medium text-shodh-text truncate">{choice.title}</span>
                  <span className="text-caption text-shodh-text-muted truncate">{choice.detail}</span>
                </span>
              </button>
            ))}
            <div className="my-1 border-t border-shodh-border-subtle" />
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                close(false);
                onAll();
              }}
              className={ITEM}
            >
              <ListFilter className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
              <span className="text-small text-shodh-text">All models</span>
            </button>
          </div>,
          document.body,
        )}
    </>
  );
}

export default ModelChoiceControl;
