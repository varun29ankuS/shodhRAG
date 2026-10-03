import React, { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, Clock, Flag } from 'lucide-react';
import { cn } from '../../lib/utils';
import { dateInputValue, dueLabel, formatMoment, fromLocalDate, isOverdue, rescheduleTo } from './dueDate';
import { FOCUS_RING } from './fields';
import { PRIORITIES, PRIORITY_LABELS } from './types';

export const PRIORITY_TEXT: Record<string, string> = {
  high: 'text-shodh-error',
  medium: 'text-shodh-warning',
  low: 'text-shodh-success',
};

const POPOVER = 'fixed z-50 min-w-[180px] rounded-xl border border-shodh-border-strong bg-shodh-surface p-1 shadow-[0_12px_36px_rgba(0,0,0,0.32)] shell-pop';

const MENU_ITEM = cn(
  'w-full h-8 px-2.5 rounded-lg inline-flex items-center gap-2 text-left text-[12.5px] text-shodh-text hover:bg-shodh-raised focus:bg-shodh-raised focus:outline-none',
);

/**
 * Popover anchored under (or above, near the bottom) a trigger, rendered in a
 * portal so scrolling lists cannot clip it. Closes on outside press and Esc,
 * returning focus to the trigger.
 */
function usePopover() {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  }, []);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current) return;
    const anchor = triggerRef.current.getBoundingClientRect();
    const height = popRef.current?.offsetHeight ?? 0;
    const width = popRef.current?.offsetWidth ?? 0;
    const below = anchor.bottom + 4;
    const top = below + height > window.innerHeight - 8 ? Math.max(8, anchor.top - height - 4) : below;
    const left = Math.min(Math.max(8, anchor.left), window.innerWidth - width - 8);
    setPos({ left, top });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (popRef.current?.contains(target) || triggerRef.current?.contains(target)) return;
      close(false);
    };
    window.addEventListener('pointerdown', onPointer, true);
    return () => window.removeEventListener('pointerdown', onPointer, true);
  }, [open, close]);

  return { open, setOpen, close, pos, triggerRef, popRef };
}

/** Quick priority menu (menu of radio items). */
export function PriorityMenu({
  priority,
  taskTitle,
  tabIndex,
  onChange,
}: {
  priority: string;
  taskTitle: string;
  tabIndex: number;
  onChange: (next: string) => void;
}) {
  const { open, setOpen, close, pos, triggerRef, popRef } = usePopover();
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    const items = popRef.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"]');
    const checked = popRef.current?.querySelector<HTMLElement>('[aria-checked="true"]');
    (checked ?? items?.[0])?.focus();
  }, [open, popRef]);

  const onMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(popRef.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? []);
    const index = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const step = e.key === 'ArrowDown' ? 1 : -1;
      items[(index + step + items.length) % items.length]?.focus();
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      items[e.key === 'Home' ? 0 : items.length - 1]?.focus();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === 'Tab') {
      // The menu lives in a body-level portal; Tab would leave the page order.
      e.preventDefault();
      close(true);
    }
  };

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        tabIndex={tabIndex}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        aria-label={`Priority: ${PRIORITY_LABELS[priority] ?? priority}. Change priority of ${taskTitle}`}
        title={`Priority: ${PRIORITY_LABELS[priority] ?? priority}`}
        onClick={e => { e.stopPropagation(); setOpen(!open); }}
        className={cn(
          'h-6 px-1.5 rounded-md inline-flex items-center gap-1 text-[11.5px] hover:bg-shodh-raised-2 transition-colors duration-micro',
          PRIORITY_TEXT[priority] ?? 'text-shodh-text-muted',
          FOCUS_RING,
        )}
      >
        <Flag className="w-3 h-3" aria-hidden="true" />
        <span className="capitalize">{PRIORITY_LABELS[priority] ?? priority}</span>
      </button>
      {open && createPortal(
        <div
          ref={popRef}
          id={menuId}
          role="menu"
          aria-label="Priority"
          onKeyDown={onMenuKeyDown}
          onClick={e => e.stopPropagation()}
          className={POPOVER}
          style={pos ? { left: pos.left, top: pos.top } : { opacity: 0, left: 0, top: 0 }}
        >
          {PRIORITIES.map(p => (
            <button
              key={p}
              type="button"
              role="menuitemradio"
              aria-checked={p === priority}
              tabIndex={-1}
              onClick={() => {
                close(true);
                if (p !== priority) onChange(p);
              }}
              className={MENU_ITEM}
            >
              <Flag className={cn('w-3.5 h-3.5', PRIORITY_TEXT[p])} aria-hidden="true" />
              <span className="flex-1">{PRIORITY_LABELS[p]}</span>
              {p === priority && <Check className="w-3.5 h-3.5 text-shodh-text-muted" aria-hidden="true" />}
            </button>
          ))}
        </div>,
        document.body,
      )}
    </>
  );
}

/** A stored due value moved to `days` from today, keeping its time if it has one. */
export function dueInDays(current: string | null | undefined, days: number, now: Date = new Date()): string {
  const target = new Date(now.getFullYear(), now.getMonth(), now.getDate() + days);
  const key = formatMoment(fromLocalDate(target, 'date'));
  return (current && rescheduleTo(current, key)) || key;
}

/** Quick due-date popover: Today, Tomorrow, Next week, or a picked date. */
export function DueMenu({
  dueDate,
  done,
  taskTitle,
  tabIndex,
  onChange,
}: {
  dueDate: string | null | undefined;
  done: boolean;
  taskTitle: string;
  tabIndex: number;
  onChange: (next: string) => void;
}) {
  const { open, setOpen, close, pos, triggerRef, popRef } = usePopover();
  const dialogId = useId();
  const dateId = useId();
  const [picked, setPicked] = useState('');
  const overdue = !done && isOverdue(dueDate);

  useEffect(() => {
    if (!open) return;
    setPicked(dateInputValue(dueDate));
    popRef.current?.querySelector<HTMLElement>('button')?.focus();
  }, [open, dueDate, popRef]);

  const choose = (next: string | null) => {
    close(true);
    if (next) onChange(next);
  };

  const label = dueDate ? dueLabel(dueDate) : 'No date';

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        tabIndex={tabIndex}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? dialogId : undefined}
        aria-label={`Due: ${label}${overdue ? ', overdue' : ''}. Change due date of ${taskTitle}`}
        onClick={e => { e.stopPropagation(); setOpen(!open); }}
        className={cn(
          'h-6 px-1.5 rounded-md inline-flex items-center gap-1 text-[11.5px] hover:bg-shodh-raised-2 transition-colors duration-micro',
          overdue ? 'text-shodh-error' : dueDate ? 'text-shodh-text-muted' : 'text-shodh-text-faint',
          FOCUS_RING,
        )}
      >
        <Clock className="w-3 h-3" aria-hidden="true" />
        {label}
        {overdue && <span> · overdue</span>}
      </button>
      {open && createPortal(
        <div
          ref={popRef}
          id={dialogId}
          role="dialog"
          aria-label={`Due date for ${taskTitle}`}
          onClick={e => e.stopPropagation()}
          onKeyDown={e => {
            if (e.key === 'Escape') {
              e.preventDefault();
              e.stopPropagation();
              close(true);
            } else if (e.key === 'Tab') {
              // Keep Tab inside the small dialog.
              const focusables = Array.from(popRef.current?.querySelectorAll<HTMLElement>('button, input') ?? []);
              const first = focusables[0];
              const lastEl = focusables[focusables.length - 1];
              if (e.shiftKey && document.activeElement === first) { e.preventDefault(); lastEl?.focus(); }
              else if (!e.shiftKey && document.activeElement === lastEl) { e.preventDefault(); first?.focus(); }
            }
          }}
          className={cn(POPOVER, 'w-[220px]')}
          style={pos ? { left: pos.left, top: pos.top } : { opacity: 0, left: 0, top: 0 }}
        >
          {[
            { label: 'Today', days: 0 },
            { label: 'Tomorrow', days: 1 },
            { label: 'Next week', days: 7 },
          ].map(opt => (
            <button key={opt.label} type="button" className={MENU_ITEM} onClick={() => choose(dueInDays(dueDate, opt.days))}>
              {opt.label}
            </button>
          ))}
          <form
            className="flex items-center gap-1.5 px-1.5 pt-1.5 mt-1 border-t border-shodh-border-subtle"
            onSubmit={e => {
              e.preventDefault();
              if (!picked) return;
              choose((dueDate && rescheduleTo(dueDate, picked)) || picked);
            }}
          >
            <label htmlFor={dateId} className="sr-only">Pick a date</label>
            <input
              id={dateId}
              type="date"
              value={picked}
              onChange={e => setPicked(e.target.value)}
              className="flex-1 min-w-0 h-8 rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[12px] text-shodh-text focus:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            />
            <button
              type="submit"
              disabled={!picked}
              className={cn('h-8 px-2.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12px] font-medium disabled:opacity-50', FOCUS_RING)}
            >
              Set
            </button>
          </form>
        </div>,
        document.body,
      )}
    </>
  );
}
