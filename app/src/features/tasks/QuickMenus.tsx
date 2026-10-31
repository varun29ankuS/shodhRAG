import React, { useEffect, useId, useState } from 'react';
import { createPortal } from 'react-dom';
import { Briefcase, Check, CircleDot, Clock, Flag } from 'lucide-react';
import { cn } from '../../lib/utils';
import { MENU_ITEM, POPOVER, usePopover } from '../../hooks/usePopover';
import { dateInputValue, dueLabel, formatMoment, fromLocalDate, isOverdue, rescheduleTo } from './dueDate';
import { FOCUS_RING } from './fields';
import { workspaceName } from './board';
import { PRIORITIES, PRIORITY_LABELS, STATUS_LABELS, STATUSES } from './types';

export const PRIORITY_TEXT: Record<string, string> = {
  high: 'text-shodh-error',
  medium: 'text-shodh-warning',
  low: 'text-shodh-success',
};

export interface Choice {
  value: string;
  label: string;
  icon?: React.ReactNode;
}

/**
 * A quick menu of radio items under a small trigger (priority, status,
 * workspace). Arrow keys, Home and End move between items; Esc and Tab close
 * it and return focus to the trigger.
 */
function ChoiceMenu({
  value,
  choices,
  menuLabel,
  triggerLabel,
  triggerTitle,
  triggerClassName,
  triggerContent,
  tabIndex,
  onChange,
}: {
  value: string;
  choices: readonly Choice[];
  /** Accessible name of the open menu. */
  menuLabel: string;
  /** Accessible name of the trigger: the current value and what it changes. */
  triggerLabel: string;
  triggerTitle?: string;
  triggerClassName: string;
  triggerContent: React.ReactNode;
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
        aria-label={triggerLabel}
        title={triggerTitle}
        onClick={e => { e.stopPropagation(); setOpen(!open); }}
        className={cn(
          'h-6 px-1.5 rounded-md inline-flex items-center gap-1 text-caption hover:bg-shodh-raised-2 transition-colors duration-micro',
          triggerClassName,
          FOCUS_RING,
        )}
      >
        {triggerContent}
      </button>
      {open && createPortal(
        <div
          ref={popRef}
          id={menuId}
          role="menu"
          aria-label={menuLabel}
          onKeyDown={onMenuKeyDown}
          onClick={e => e.stopPropagation()}
          className={cn(POPOVER, 'max-h-[min(320px,60vh)] overflow-y-auto scrollbar-thin')}
          style={pos ? { left: pos.left, top: pos.top } : { opacity: 0, left: 0, top: 0 }}
        >
          {choices.map(choice => (
            <button
              key={choice.value}
              type="button"
              role="menuitemradio"
              aria-checked={choice.value === value}
              tabIndex={-1}
              onClick={() => {
                close(true);
                if (choice.value !== value) onChange(choice.value);
              }}
              className={MENU_ITEM}
            >
              {choice.icon}
              <span className="flex-1 truncate">{choice.label}</span>
              {choice.value === value && <Check className="w-3.5 h-3.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />}
            </button>
          ))}
        </div>,
        document.body,
      )}
    </>
  );
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
  const label = PRIORITY_LABELS[priority] ?? priority;
  return (
    <ChoiceMenu
      value={priority}
      choices={PRIORITIES.map(p => ({
        value: p,
        label: PRIORITY_LABELS[p],
        icon: <Flag className={cn('w-3.5 h-3.5', PRIORITY_TEXT[p])} aria-hidden="true" />,
      }))}
      menuLabel="Priority"
      triggerLabel={`Priority: ${label}. Change priority of ${taskTitle}`}
      triggerTitle={`Priority: ${label}`}
      triggerClassName={PRIORITY_TEXT[priority] ?? 'text-shodh-text-muted'}
      triggerContent={
        <>
          <Flag className="w-3 h-3" aria-hidden="true" />
          <span className="capitalize">{label}</span>
        </>
      }
      tabIndex={tabIndex}
      onChange={onChange}
    />
  );
}

/** Quick status menu: moves a task between To do, In progress and Done. */
export function StatusMenu({
  status,
  taskTitle,
  tabIndex,
  onChange,
}: {
  status: string;
  taskTitle: string;
  tabIndex: number;
  onChange: (next: string) => void;
}) {
  const label = STATUS_LABELS[status] ?? status;
  return (
    <ChoiceMenu
      value={status}
      choices={STATUSES.map(s => ({
        value: s,
        label: STATUS_LABELS[s],
        icon: <CircleDot className="w-3.5 h-3.5 text-shodh-text-muted" aria-hidden="true" />,
      }))}
      menuLabel="Status"
      triggerLabel={`Status: ${label}. Move ${taskTitle}`}
      triggerTitle="Move to another column"
      triggerClassName="text-shodh-text-muted"
      triggerContent={
        <>
          <CircleDot className="w-3 h-3" aria-hidden="true" />
          {label}
        </>
      }
      tabIndex={tabIndex}
      onChange={onChange}
    />
  );
}

/** Value of the "No workspace" choice. */
const NO_WORKSPACE = '';

/**
 * Quick workspace menu. A task naming a workspace that is not listed (it was
 * deleted, or the list has not loaded) reads as "No workspace"; choosing a
 * workspace or "No workspace" replaces it.
 */
export function WorkspaceMenu({
  workspaceId,
  workspaces,
  taskTitle,
  tabIndex,
  className,
  onChange,
}: {
  workspaceId: string | null | undefined;
  workspaces: readonly { id: string; name: string }[];
  taskTitle: string;
  tabIndex: number;
  className?: string;
  /** The new workspace id, or null for none. */
  onChange: (next: string | null) => void;
}) {
  const name = workspaceName(workspaceId, workspaces);
  // An unlisted id checks nothing, so choosing "No workspace" still clears it.
  const value = workspaceId || NO_WORKSPACE;
  const label = name ?? 'No workspace';
  return (
    <ChoiceMenu
      value={value}
      choices={[
        { value: NO_WORKSPACE, label: 'No workspace' },
        ...workspaces.map(w => ({
          value: w.id,
          label: w.name,
          icon: <Briefcase className="w-3.5 h-3.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />,
        })),
      ]}
      menuLabel="Workspace"
      triggerLabel={`Workspace: ${label}. Change workspace of ${taskTitle}`}
      triggerTitle={`Workspace: ${label}`}
      triggerClassName={cn(name ? 'text-shodh-text-muted max-w-[14rem]' : 'text-shodh-text-faint', className)}
      triggerContent={
        <>
          <Briefcase className="w-3 h-3 shrink-0" aria-hidden="true" />
          <span className="truncate">{label}</span>
        </>
      }
      tabIndex={tabIndex}
      onChange={next => onChange(next === NO_WORKSPACE ? null : next)}
    />
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
  /** The new due date, or null to remove it. */
  onChange: (next: string | null) => void;
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
          'h-6 px-1.5 rounded-md inline-flex items-center gap-1 text-caption hover:bg-shodh-raised-2 transition-colors duration-micro',
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
          {dueDate && (
            <button
              type="button"
              className={MENU_ITEM}
              onClick={() => {
                close(true);
                onChange(null);
              }}
            >
              No date
            </button>
          )}
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
              className="flex-1 min-w-0 h-8 rounded-lg border border-shodh-border bg-shodh-surface px-2 text-caption text-shodh-text focus:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            />
            <button
              type="submit"
              disabled={!picked}
              className={cn('h-8 px-2.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-caption font-medium disabled:opacity-50', FOCUS_RING)}
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
